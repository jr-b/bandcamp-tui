//! Application state and the pure `update` function.
//!
//! `App::update` takes an [`Action`] (a keypress, an API result, a player event) and returns
//! the [`Effect`]s to run (API calls, player commands). It never does I/O itself, so the whole
//! app's behaviour is unit-testable; `run.rs` executes the effects.

use std::collections::{HashMap, VecDeque};
use std::sync::Arc;

use ratatui::widgets::TableState;

use crate::bandcamp::{
    Album, BandId, Cursor, DiscoverItem, DiscoverPage, DiscoverQuery, ItemId, Slice, TralbumKind,
};
use crate::player::{PlaybackState, PlayerEvent, QueueTrack, Status};

/// Fetch the next page when the selection gets this close to the end of the list.
const LOAD_MORE_THRESHOLD: usize = 10;
/// Stop paging after this many releases to keep memory bounded.
pub const MAX_ITEMS: usize = 1000;
const ALBUM_CACHE_SIZE: usize = 64;
const VOLUME_STEP: f32 = 0.05;
const SEEK_STEP_MS: i64 = 10_000;

#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    Quit,
    Up,
    Down,
    PageUp,
    PageDown,
    Top,
    Bottom,
    /// Play the selected release from its first track.
    PlaySelected,
    TogglePause,
    NextTrack,
    PreviousTrack,
    SeekForward,
    SeekBackward,
    VolumeUp,
    VolumeDown,
    /// Reload the discover feed (reshuffles "surprise me").
    Refresh,
    /// Cycle best-selling → new arrivals → surprise me.
    CycleSlice,
    /// Terminal resized; just redraw.
    Resize,
    DiscoverLoaded {
        generation: u64,
        result: Result<DiscoverPage, String>,
    },
    AlbumLoaded {
        id: ItemId,
        /// Boxed: an `Album` is much larger than every other action.
        result: Result<Box<Album>, String>,
    },
    Player {
        event: Option<PlayerEvent>,
        status: Status,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub enum Effect {
    FetchDiscover {
        query: DiscoverQuery,
        cursor: Option<Cursor>,
        generation: u64,
    },
    FetchAlbum {
        band_id: BandId,
        id: ItemId,
        kind: TralbumKind,
    },
    Player(PlayerCommand),
}

#[derive(Debug, Clone, PartialEq)]
pub enum PlayerCommand {
    PlayQueue(Vec<QueueTrack>, usize),
    TogglePause,
    Next,
    Previous,
    SeekBy(i64),
    SetVolume(f32),
}

/// The discover feed as loaded so far.
#[derive(Debug, Default)]
pub struct DiscoverList {
    pub items: Vec<DiscoverItem>,
    pub total: u64,
    next_cursor: Option<Cursor>,
    pub loading: bool,
    /// Bumped on every reload so responses to outdated requests are ignored.
    generation: u64,
}

impl DiscoverList {
    /// Whether more pages exist and we're allowed to load them.
    #[must_use]
    pub fn has_more(&self) -> bool {
        self.next_cursor.is_some() && self.items.len() < MAX_ITEMS
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MessageKind {
    Info,
    Error,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Message {
    pub kind: MessageKind,
    pub text: String,
}

#[derive(Debug)]
pub struct App {
    pub query: DiscoverQuery,
    pub discover: DiscoverList,
    /// Selection and scroll offset of the discover table; the offset is maintained by rendering.
    pub table: TableState,
    /// Rows visible in the discover table, set while rendering; used for page up/down.
    pub page_rows: usize,
    albums: AlbumCache,
    /// Release the user asked to play while its details were still loading.
    pending_play: Option<ItemId>,
    /// The album the player's queue was built from.
    pub now_playing: Option<Arc<Album>>,
    pub player: Status,
    pub player_available: bool,
    pub message: Option<Message>,
    pub should_quit: bool,
}

impl App {
    /// Creates the app and the effects to run at startup.
    #[must_use]
    pub fn new(query: DiscoverQuery, player_available: bool) -> (Self, Vec<Effect>) {
        let mut app = Self {
            query,
            discover: DiscoverList::default(),
            table: TableState::default(),
            page_rows: 10,
            albums: AlbumCache::new(ALBUM_CACHE_SIZE),
            pending_play: None,
            now_playing: None,
            player: Status::default(),
            player_available,
            message: None,
            should_quit: false,
        };
        if !player_available {
            app.error("No audio output available; browsing only.");
        }
        let effects = vec![app.reload()];
        (app, effects)
    }

    #[must_use]
    pub fn selected(&self) -> Option<&DiscoverItem> {
        self.discover.items.get(self.table.selected()?)
    }

    /// Whether the progress bar needs periodic updates.
    #[must_use]
    pub fn is_playing(&self) -> bool {
        matches!(
            self.player.state,
            PlaybackState::Playing | PlaybackState::Buffering | PlaybackState::Loading
        )
    }

    /// The queue index currently playing, with its album.
    #[must_use]
    pub fn current_track(&self) -> Option<(&Album, usize)> {
        let album = self.now_playing.as_deref()?;
        let index = self.player.index?;
        (index < album.tracks.len()).then_some((album, index))
    }

    pub fn update(&mut self, action: Action) -> Vec<Effect> {
        // Any keypress dismisses the previous message.
        if action.is_user_input() {
            self.message = None;
        }
        match action {
            Action::Quit => {
                self.should_quit = true;
                vec![]
            }
            Action::Up => self.move_by(-1),
            Action::Down => self.move_by(1),
            Action::PageUp => self.move_by(-page(self.page_rows)),
            Action::PageDown => self.move_by(page(self.page_rows)),
            Action::Top => self.move_by(isize::MIN),
            Action::Bottom => self.move_by(isize::MAX),
            Action::PlaySelected => self.play_selected(),
            Action::TogglePause => self.player_command(PlayerCommand::TogglePause),
            Action::NextTrack => self.player_command(PlayerCommand::Next),
            Action::PreviousTrack => self.player_command(PlayerCommand::Previous),
            Action::SeekForward => self.player_command(PlayerCommand::SeekBy(SEEK_STEP_MS)),
            Action::SeekBackward => self.player_command(PlayerCommand::SeekBy(-SEEK_STEP_MS)),
            Action::VolumeUp => self.change_volume(VOLUME_STEP),
            Action::VolumeDown => self.change_volume(-VOLUME_STEP),
            Action::Refresh => vec![self.reload()],
            Action::CycleSlice => {
                self.query.slice = match self.query.slice {
                    Slice::Top => Slice::New,
                    Slice::New => Slice::Random,
                    Slice::Random => Slice::Top,
                };
                vec![self.reload()]
            }
            Action::Resize => vec![],
            Action::DiscoverLoaded { generation, result } => {
                self.discover_loaded(generation, result)
            }
            Action::AlbumLoaded { id, result } => self.album_loaded(id, result),
            Action::Player { event, status } => {
                self.player = status;
                match event {
                    Some(PlayerEvent::TrackFailed { index, message }) => {
                        let title = self
                            .now_playing
                            .as_ref()
                            .and_then(|a| a.tracks.get(index))
                            .map_or("track", |t| t.title.as_str());
                        self.error(format!("Couldn't play {title}: {message}"));
                    }
                    Some(PlayerEvent::QueueEnded) => self.now_playing = None,
                    _ => {}
                }
                vec![]
            }
        }
    }

    /// Starts a fresh discover load for the current query.
    fn reload(&mut self) -> Effect {
        let generation = self.discover.generation + 1;
        self.discover = DiscoverList {
            loading: true,
            generation,
            ..DiscoverList::default()
        };
        self.table = TableState::default();
        Effect::FetchDiscover {
            query: self.query.clone(),
            cursor: None,
            generation,
        }
    }

    fn discover_loaded(
        &mut self,
        generation: u64,
        result: Result<DiscoverPage, String>,
    ) -> Vec<Effect> {
        if generation != self.discover.generation {
            return vec![]; // Response to a request we've since replaced.
        }
        self.discover.loading = false;
        match result {
            Ok(page) => {
                let room = MAX_ITEMS.saturating_sub(self.discover.items.len());
                self.discover
                    .items
                    .extend(page.items.into_iter().take(room));
                self.discover.total = page.total;
                self.discover.next_cursor = page.next_cursor;
                if self.table.selected().is_none() && !self.discover.items.is_empty() {
                    self.table.select(Some(0));
                }
                // A tall terminal can show the whole first page: keep loading until the
                // selection is far enough from the end (or there's nothing more).
                self.maybe_load_more().into_iter().collect()
            }
            Err(e) => {
                self.error(format!("Couldn't load Discover: {e}"));
                vec![]
            }
        }
    }

    fn maybe_load_more(&mut self) -> Option<Effect> {
        let selected = self.table.selected().unwrap_or(0);
        let near_end = selected + LOAD_MORE_THRESHOLD >= self.discover.items.len();
        if !near_end || self.discover.loading || !self.discover.has_more() {
            return None;
        }
        self.discover.loading = true;
        Some(Effect::FetchDiscover {
            query: self.query.clone(),
            cursor: self.discover.next_cursor.clone(),
            generation: self.discover.generation,
        })
    }

    fn move_by(&mut self, delta: isize) -> Vec<Effect> {
        let len = self.discover.items.len();
        if len == 0 {
            return vec![];
        }
        let current = self.table.selected().unwrap_or(0);
        let next = current.saturating_add_signed(delta).min(len - 1);
        self.table.select(Some(next));
        self.maybe_load_more().into_iter().collect()
    }

    fn play_selected(&mut self) -> Vec<Effect> {
        let Some(item) = self.selected() else {
            return vec![];
        };
        let (band_id, id, kind) = (item.band_id, item.id, item.kind);
        if let Some(album) = self.albums.get(id) {
            return self.play_album(album);
        }
        self.pending_play = Some(id);
        self.info("Loading…");
        vec![Effect::FetchAlbum { band_id, id, kind }]
    }

    fn album_loaded(&mut self, id: ItemId, result: Result<Box<Album>, String>) -> Vec<Effect> {
        let wanted = self.pending_play == Some(id);
        match result {
            Ok(album) => {
                let album: Arc<Album> = Arc::from(album);
                self.albums.insert(id, Arc::clone(&album));
                if wanted {
                    self.pending_play = None;
                    self.message = None;
                    return self.play_album(album);
                }
            }
            Err(e) if wanted => {
                self.pending_play = None;
                self.error(format!("Couldn't load release: {e}"));
            }
            Err(_) => {}
        }
        vec![]
    }

    fn play_album(&mut self, album: Arc<Album>) -> Vec<Effect> {
        if !album.has_playable_tracks() {
            let why = if album.is_preorder {
                "it's a pre-order"
            } else {
                "no tracks are streamable"
            };
            self.error(format!("Can't play {}: {why}.", album.title));
            return vec![];
        }
        if !self.player_available {
            self.error("No audio output available.");
            return vec![];
        }
        let queue = album.tracks.iter().map(QueueTrack::from).collect();
        self.now_playing = Some(album);
        vec![Effect::Player(PlayerCommand::PlayQueue(queue, 0))]
    }

    fn player_command(&mut self, command: PlayerCommand) -> Vec<Effect> {
        if self.now_playing.is_none() || !self.player_available {
            return vec![];
        }
        vec![Effect::Player(command)]
    }

    fn change_volume(&mut self, delta: f32) -> Vec<Effect> {
        if !self.player_available {
            return vec![];
        }
        // Round to whole steps so repeated presses land on 0 % and 100 % exactly.
        let volume = ((self.player.volume + delta) / VOLUME_STEP).round() * VOLUME_STEP;
        let volume = volume.clamp(0.0, 1.0);
        self.player.volume = volume;
        vec![Effect::Player(PlayerCommand::SetVolume(volume))]
    }

    fn info(&mut self, text: impl Into<String>) {
        self.message = Some(Message {
            kind: MessageKind::Info,
            text: text.into(),
        });
    }

    fn error(&mut self, text: impl Into<String>) {
        let text = text.into();
        tracing::warn!(%text);
        self.message = Some(Message {
            kind: MessageKind::Error,
            text,
        });
    }
}

impl Action {
    fn is_user_input(&self) -> bool {
        !matches!(
            self,
            Self::Resize
                | Self::DiscoverLoaded { .. }
                | Self::AlbumLoaded { .. }
                | Self::Player { .. }
        )
    }
}

fn page(rows: usize) -> isize {
    isize::try_from(rows.saturating_sub(1).max(1)).unwrap_or(isize::MAX)
}

/// Least-recently-inserted cache of album details (metadata only, a few KB each).
#[derive(Debug)]
struct AlbumCache {
    capacity: usize,
    map: HashMap<ItemId, Arc<Album>>,
    order: VecDeque<ItemId>,
}

impl AlbumCache {
    fn new(capacity: usize) -> Self {
        Self {
            capacity,
            map: HashMap::with_capacity(capacity),
            order: VecDeque::with_capacity(capacity),
        }
    }

    fn get(&self, id: ItemId) -> Option<Arc<Album>> {
        self.map.get(&id).cloned()
    }

    fn insert(&mut self, id: ItemId, album: Arc<Album>) {
        if self.map.insert(id, album).is_none() {
            self.order.push_back(id);
            if self.order.len() > self.capacity
                && let Some(oldest) = self.order.pop_front()
            {
                self.map.remove(&oldest);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn page_fixture() -> DiscoverPage {
        let json = include_str!("../tests/fixtures/discover_electronic_top.json");
        crate::bandcamp::parse_discover(json).unwrap()
    }

    fn album_fixture(name: &str) -> Album {
        let json = match name {
            "album" => include_str!("../tests/fixtures/tralbum_album.json"),
            "preorder" => include_str!("../tests/fixtures/tralbum_preorder.json"),
            _ => unreachable!(),
        };
        crate::bandcamp::parse_album(json).unwrap()
    }

    /// An app with the fixture page loaded.
    fn loaded() -> App {
        let (mut app, effects) = App::new(DiscoverQuery::default(), true);
        let Some(Effect::FetchDiscover { generation, .. }) = effects.first().cloned() else {
            panic!("expected initial fetch, got {effects:?}");
        };
        app.page_rows = 3;
        let more = app.update(Action::DiscoverLoaded {
            generation,
            result: Ok(page_fixture()),
        });
        // Four items is within the load-more threshold, so a second page is requested.
        assert!(matches!(
            more.as_slice(),
            [Effect::FetchDiscover {
                cursor: Some(_),
                ..
            }]
        ));
        app
    }

    #[test]
    fn startup_fetches_first_page() {
        let (app, effects) = App::new(DiscoverQuery::default(), true);
        assert!(app.discover.loading);
        assert_eq!(
            effects,
            [Effect::FetchDiscover {
                query: DiscoverQuery::default(),
                cursor: None,
                generation: 1
            }]
        );
    }

    #[test]
    fn navigation_clamps_to_the_list() {
        let mut app = loaded();
        assert_eq!(app.table.selected(), Some(0));
        app.update(Action::Up);
        assert_eq!(app.table.selected(), Some(0));
        app.update(Action::Down);
        assert_eq!(app.table.selected(), Some(1));
        app.update(Action::Bottom);
        assert_eq!(app.table.selected(), Some(3));
        app.update(Action::PageUp);
        assert_eq!(app.table.selected(), Some(1));
        app.update(Action::Top);
        assert_eq!(app.table.selected(), Some(0));
    }

    #[test]
    fn appends_next_page_and_stops_at_the_end() {
        let mut app = loaded();
        let generation = app.discover.generation;
        // Second page arrives (reuse the fixture), then an empty last page.
        app.update(Action::DiscoverLoaded {
            generation,
            result: Ok(page_fixture()),
        });
        assert_eq!(app.discover.items.len(), 8);
        let effects = app.update(Action::DiscoverLoaded {
            generation,
            result: Ok(DiscoverPage {
                items: vec![],
                total: 8,
                next_cursor: None,
            }),
        });
        assert_eq!(effects, []);
        assert!(!app.discover.has_more());
        app.update(Action::Bottom);
        assert!(!app.discover.loading, "no fetch past the last page");
    }

    #[test]
    fn ignores_responses_for_a_replaced_query() {
        let mut app = loaded();
        let old = app.discover.generation;
        let effects = app.update(Action::CycleSlice);
        assert_eq!(app.query.slice, Slice::New);
        assert_eq!(app.discover.items, []);
        assert!(
            matches!(&effects[..], [Effect::FetchDiscover { query, cursor: None, .. }] if query.slice == Slice::New)
        );
        app.update(Action::DiscoverLoaded {
            generation: old,
            result: Ok(page_fixture()),
        });
        assert!(app.discover.items.is_empty(), "stale page was applied");
    }

    #[test]
    fn caps_the_list_size() {
        let mut app = loaded();
        let generation = app.discover.generation;
        while app.discover.items.len() < MAX_ITEMS {
            app.update(Action::DiscoverLoaded {
                generation,
                result: Ok(page_fixture()),
            });
        }
        assert_eq!(app.discover.items.len(), MAX_ITEMS);
        assert!(!app.discover.has_more());
    }

    #[test]
    fn play_fetches_album_then_queues_it() {
        let mut app = loaded();
        let effects = app.update(Action::PlaySelected);
        let [Effect::FetchAlbum { id, .. }] = effects[..] else {
            panic!("expected album fetch, got {effects:?}");
        };
        assert_eq!(id, ItemId(3_457_085_593));

        let effects = app.update(Action::AlbumLoaded {
            id,
            result: Ok(Box::new(album_fixture("album"))),
        });
        let [Effect::Player(PlayerCommand::PlayQueue(queue, 0))] = &effects[..] else {
            panic!("expected play, got {effects:?}");
        };
        assert_eq!(queue.len(), 10);
        assert_eq!(app.now_playing.as_ref().unwrap().title, "Ocean Company");

        // Playing it again uses the cache: no second fetch.
        let effects = app.update(Action::PlaySelected);
        assert!(matches!(
            &effects[..],
            [Effect::Player(PlayerCommand::PlayQueue(..))]
        ));
    }

    #[test]
    fn preorder_shows_an_error_instead_of_playing() {
        let mut app = loaded();
        let [Effect::FetchAlbum { id, .. }] = app.update(Action::PlaySelected)[..] else {
            panic!();
        };
        let effects = app.update(Action::AlbumLoaded {
            id,
            result: Ok(Box::new(album_fixture("preorder"))),
        });
        assert_eq!(effects, []);
        let message = app.message.unwrap();
        assert_eq!(message.kind, MessageKind::Error);
        assert!(message.text.contains("pre-order"), "{}", message.text);
    }

    #[test]
    fn transport_keys_do_nothing_until_something_plays() {
        let mut app = loaded();
        assert_eq!(app.update(Action::TogglePause), []);
        assert_eq!(app.update(Action::NextTrack), []);
        app.now_playing = Some(Arc::new(album_fixture("album")));
        assert_eq!(
            app.update(Action::SeekBackward),
            [Effect::Player(PlayerCommand::SeekBy(-10_000))]
        );
    }

    #[test]
    fn volume_steps_and_clamps() {
        let mut app = loaded();
        assert_eq!(
            app.update(Action::VolumeDown),
            [Effect::Player(PlayerCommand::SetVolume(0.95))]
        );
        for _ in 0..30 {
            app.update(Action::VolumeUp);
        }
        assert!((app.player.volume - 1.0).abs() < f32::EPSILON);
    }

    #[test]
    fn no_audio_output_still_allows_browsing() {
        let (mut app, _) = App::new(DiscoverQuery::default(), false);
        assert_eq!(app.message.as_ref().unwrap().kind, MessageKind::Error);
        let generation = app.discover.generation;
        app.update(Action::DiscoverLoaded {
            generation,
            result: Ok(page_fixture()),
        });
        app.update(Action::Down);
        assert_eq!(app.table.selected(), Some(1));
        assert_eq!(app.update(Action::VolumeUp), []);
    }

    #[test]
    fn album_cache_evicts_oldest() {
        let mut cache = AlbumCache::new(2);
        let album = Arc::new(album_fixture("album"));
        for id in 1..=3 {
            cache.insert(ItemId(id), Arc::clone(&album));
        }
        assert!(cache.get(ItemId(1)).is_none());
        assert!(cache.get(ItemId(3)).is_some());
        assert_eq!(cache.map.len(), 2);
    }
}
