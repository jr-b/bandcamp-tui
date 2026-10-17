//! Application state and the pure `update` function.
//!
//! `App::update` takes an [`Action`] (a keypress, an API result, a player event) and returns
//! the [`Effect`]s to run (API calls, player commands). It never does I/O itself, so the whole
//! app's behaviour is unit-testable; `run.rs` executes the effects.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;

use ratatui::widgets::TableState;

use crate::art::Art;
use crate::bandcamp::{
    Album, ArtId, BandId, Cursor, DiscoverItem, DiscoverPage, DiscoverQuery, Filters, ItemId,
    Slice, TralbumKind,
};
use crate::picker::{Picker, PickerKind};
use crate::player::{PlaybackState, PlayerEvent, QueueTrack, Status};
use crate::settings::Settings;

/// Fetch the next page when the selection gets this close to the end of the list.
const LOAD_MORE_THRESHOLD: usize = 10;
/// Stop paging after this many releases to keep memory bounded.
pub const MAX_ITEMS: usize = 1000;
const ALBUM_CACHE_SIZE: usize = 64;
/// Encoded covers are ~10–50 KB each depending on the protocol.
const ART_CACHE_SIZE: usize = 16;
const VOLUME_STEP: f32 = 0.05;
const SEEK_STEP_MS: i64 = 10_000;
/// Autoplay gives up after this many releases in a row that can't be played.
const AUTOPLAY_MAX_SKIPS: u8 = 5;

#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    Quit,
    Up,
    Down,
    PageUp,
    PageDown,
    Top,
    Bottom,
    /// Discover pane: play the selected release from the start.
    /// Album pane: play from the selected track.
    Play,
    /// Move focus to the other pane.
    ToggleFocus,
    FocusDiscover,
    FocusAlbum,
    /// Album pane: switch between the tracklist and the about/credits/bio text.
    ToggleInfo,
    /// Continue with the next Discover release when an album ends.
    ToggleAutoplay,
    /// Open the selected release (or album pane's album) on bandcamp.com.
    OpenInBrowser,
    ShowHelp,
    CloseHelp,
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
    OpenPicker(PickerKind),
    /// Close the picker without changing anything.
    ClosePicker,
    /// Text typed into the picker's search.
    PickerInput(char),
    PickerBackspace,
    PickerClear,
    /// Terminal resized; just redraw.
    Resize,
    DiscoverLoaded {
        generation: u64,
        result: Result<DiscoverPage, String>,
    },
    /// The debounce delay for loading this release's details has passed.
    AlbumDue(ItemId),
    AlbumLoaded {
        id: ItemId,
        /// Boxed: an `Album` is much larger than every other action.
        result: Result<Box<Album>, String>,
    },
    ArtLoaded {
        id: ArtId,
        result: Result<Art, String>,
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
    /// Send [`Action::AlbumDue`] after a short delay, replacing any pending one. Keeps scrolling
    /// through the list from firing a request per row.
    DebounceAlbum(ItemId),
    FetchAlbum {
        band_id: BandId,
        id: ItemId,
        kind: TralbumKind,
    },
    /// Download and encode a cover (only when art is enabled).
    FetchArt(ArtId),
    Player(PlayerCommand),
    OpenUrl(url::Url),
    /// Remember filters and volume for next time.
    SaveSettings(Settings),
}

#[derive(Debug, Clone, PartialEq)]
pub enum PlayerCommand {
    PlayQueue(Vec<QueueTrack>, usize),
    /// Jump within the current queue.
    PlayIndex(usize),
    TogglePause,
    Next,
    Previous,
    SeekBy(i64),
    SetVolume(f32),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Focus {
    #[default]
    Discover,
    Album,
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

/// What the album pane knows about the release selected in the discover list.
#[derive(Debug, Clone, Copy)]
pub enum AlbumView<'a> {
    /// Nothing selected.
    None,
    /// Details not fetched yet (or in flight); show what discover told us.
    Loading(&'a DiscoverItem),
    Failed(&'a DiscoverItem, &'a str),
    Loaded(&'a Arc<Album>),
}

/// State of the album pane's tracklist.
#[derive(Debug, Default)]
pub struct AlbumPane {
    /// Which album `tracks` refers to; reset when the selection moves to another release.
    for_album: Option<ItemId>,
    pub tracks: TableState,
    pub show_info: bool,
    pub info_scroll: u16,
    /// Rows visible in the tracklist, set while rendering.
    pub page_rows: usize,
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

// Independent flags (player present, art on, help open, quitting), not a state machine.
#[allow(clippy::struct_excessive_bools)]
#[derive(Debug)]
pub struct App {
    pub query: DiscoverQuery,
    pub discover: DiscoverList,
    /// Selection and scroll offset of the discover table; the offset is maintained by rendering.
    pub table: TableState,
    /// Rows visible in the discover table, set while rendering; used for page up/down.
    pub page_rows: usize,
    pub focus: Focus,
    pub album_pane: AlbumPane,
    /// Open filter popup, if any. While open it gets all keys.
    pub picker: Option<Picker>,
    albums: Lru<ItemId, Arc<Album>>,
    /// Whether the terminal shows cover art (decided once at startup).
    pub art_enabled: bool,
    arts: Lru<ArtId, Art>,
    art_requests: HashSet<ArtId>,
    art_failed: HashSet<ArtId>,
    /// Album requests in flight, so we never fetch the same release twice at once.
    album_requests: HashSet<ItemId>,
    album_errors: HashMap<ItemId, String>,
    /// Albums whose expired stream URLs we've already re-fetched once since playback last
    /// worked; a second expiry is reported instead of retried, so we can't loop.
    refetched_for_expiry: HashSet<ItemId>,
    /// Release the user asked to play while its details were still loading: (id, track).
    pending_play: Option<(ItemId, usize)>,
    /// When an album ends, play the next Discover release. On by default; see `config.toml`.
    pub autoplay: bool,
    /// The release autoplay is currently loading, so a failure moves on instead of stopping.
    autoplay_attempt: Option<ItemId>,
    /// Consecutive releases autoplay had to skip.
    autoplay_skips: u8,
    /// Autoplay ran out of loaded releases and is waiting for the next page to continue after
    /// this one.
    autoplay_after: Option<ItemId>,
    /// The album the player's queue was built from.
    pub now_playing: Option<Arc<Album>>,
    pub player: Status,
    pub player_available: bool,
    pub message: Option<Message>,
    pub should_quit: bool,
    pub show_help: bool,
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
            focus: Focus::Discover,
            album_pane: AlbumPane::default(),
            picker: None,
            albums: Lru::new(ALBUM_CACHE_SIZE),
            art_enabled: false,
            arts: Lru::new(ART_CACHE_SIZE),
            art_requests: HashSet::new(),
            art_failed: HashSet::new(),
            album_requests: HashSet::new(),
            album_errors: HashMap::new(),
            refetched_for_expiry: HashSet::new(),
            pending_play: None,
            autoplay: true,
            autoplay_attempt: None,
            autoplay_skips: 0,
            autoplay_after: None,
            now_playing: None,
            player: Status::default(),
            player_available,
            message: None,
            should_quit: false,
            show_help: false,
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

    /// What to show in the album pane for the selected release.
    #[must_use]
    pub fn album_view(&self) -> AlbumView<'_> {
        let Some(item) = self.selected() else {
            return AlbumView::None;
        };
        if let Some(album) = self.albums.peek(item.id) {
            return AlbumView::Loaded(album);
        }
        match self.album_errors.get(&item.id) {
            Some(e) => AlbumView::Failed(item, e),
            None => AlbumView::Loading(item),
        }
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

    /// The track index playing in `album`, if it's the album playing.
    #[must_use]
    pub fn playing_index_in(&self, album: &Album) -> Option<usize> {
        let (playing, index) = self.current_track()?;
        (playing.id == album.id).then_some(index)
    }

    pub fn update(&mut self, action: Action) -> Vec<Effect> {
        // Any keypress dismisses the previous message.
        if action.is_user_input() {
            self.message = None;
        }
        if self.picker.is_some()
            && let Some(effects) = self.picker_update(&action)
        {
            return effects;
        }
        match action {
            Action::Quit => {
                self.should_quit = true;
                vec![]
            }
            Action::Up => self.move_by(-1),
            Action::Down => self.move_by(1),
            Action::PageUp => self.move_by(-page(self.focused_page_rows())),
            Action::PageDown => self.move_by(page(self.focused_page_rows())),
            Action::Top => self.move_by(isize::MIN),
            Action::Bottom => self.move_by(isize::MAX),
            Action::Play => match self.focus {
                Focus::Discover => self.play_selected_release(),
                Focus::Album => self.play_selected_track(),
            },
            Action::ToggleFocus => {
                self.focus = match self.focus {
                    Focus::Discover => Focus::Album,
                    Focus::Album => Focus::Discover,
                };
                vec![]
            }
            Action::FocusDiscover => {
                self.focus = Focus::Discover;
                vec![]
            }
            Action::FocusAlbum => {
                self.focus = Focus::Album;
                vec![]
            }
            Action::ToggleInfo => {
                self.album_pane.show_info = !self.album_pane.show_info;
                self.album_pane.info_scroll = 0;
                vec![]
            }
            Action::ToggleAutoplay => self.toggle_autoplay(),
            Action::OpenInBrowser => self.open_in_browser(),
            Action::ShowHelp => {
                self.show_help = true;
                vec![]
            }
            Action::CloseHelp => {
                self.show_help = false;
                vec![]
            }
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
                vec![self.reload(), self.save_settings()]
            }
            Action::OpenPicker(kind) => {
                self.picker = Some(Picker::new(kind, Filters::snapshot(), &self.query));
                vec![]
            }
            // Picker keys with no picker open (handled above when one is), and resizes, which
            // only need a redraw.
            Action::ClosePicker
            | Action::PickerInput(_)
            | Action::PickerBackspace
            | Action::PickerClear
            | Action::Resize => vec![],
            Action::DiscoverLoaded { generation, result } => {
                self.discover_loaded(generation, result)
            }
            Action::AlbumDue(id) => self.album_due(id),
            Action::AlbumLoaded { id, result } => self.album_loaded(id, result),
            Action::ArtLoaded { id, result } => self.art_loaded(id, result),
            Action::Player { event, status } => self.player_update(event, status),
        }
    }

    // ---- Filters -------------------------------------------------------------------------------

    /// Handles `action` while the picker is open. `None` means "not a picker action": handle
    /// it normally (e.g. API results and player events keep flowing).
    fn picker_update(&mut self, action: &Action) -> Option<Vec<Effect>> {
        let picker = self.picker.as_mut()?;
        match action {
            Action::Up => picker.move_by(-1),
            Action::Down => picker.move_by(1),
            Action::PageUp => picker.move_by(-10),
            Action::PageDown => picker.move_by(10),
            Action::Top => picker.move_by(isize::MIN),
            Action::Bottom => picker.move_by(isize::MAX),
            Action::PickerInput(c) => picker.push(*c),
            Action::PickerBackspace => picker.backspace(),
            Action::PickerClear => picker.clear(),
            Action::ClosePicker => self.picker = None,
            Action::Play => {
                let choice = picker.selected().cloned();
                self.picker = None;
                if let Some(choice) = choice {
                    let mut query = self.query.clone();
                    choice.apply(&mut query);
                    if query != self.query {
                        self.query = query;
                        return Some(vec![self.reload(), self.save_settings()]);
                    }
                }
            }
            Action::Quit => return None,
            _ if action.is_user_input() => {} // other keys are swallowed by the popup
            _ => return None,
        }
        Some(vec![])
    }

    /// Current filters and volume, to restore next time.
    #[must_use]
    pub fn settings(&self) -> Settings {
        Settings::from_state(&self.query, self.player.volume)
    }

    fn save_settings(&self) -> Effect {
        Effect::SaveSettings(self.settings())
    }

    // ---- Discover ------------------------------------------------------------------------------

    /// Starts a fresh discover load for the current query.
    fn reload(&mut self) -> Effect {
        let generation = self.discover.generation + 1;
        self.discover = DiscoverList {
            loading: true,
            generation,
            ..DiscoverList::default()
        };
        self.table = TableState::default();
        self.focus = Focus::Discover;
        // Errors may have been transient; let them be retried.
        self.album_errors.clear();
        // The list autoplay was walking is gone.
        self.autoplay_after = None;
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
                let mut effects = Vec::new();
                if self.table.selected().is_none() && !self.discover.items.is_empty() {
                    self.table.select(Some(0));
                    effects.extend(self.selection_changed());
                }
                // A tall terminal can show the whole first page: keep loading until the
                // selection is far enough from the end (or there's nothing more).
                effects.extend(self.maybe_load_more());
                if let Some(after) = self.autoplay_after.take() {
                    effects.extend(self.autoplay_next(Some(after)));
                }
                effects
            }
            Err(e) => {
                self.autoplay_after = None;
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

    // ---- Navigation ----------------------------------------------------------------------------

    fn focused_page_rows(&self) -> usize {
        match self.focus {
            Focus::Discover => self.page_rows,
            Focus::Album => self.album_pane.page_rows,
        }
    }

    fn move_by(&mut self, delta: isize) -> Vec<Effect> {
        match self.focus {
            Focus::Discover => {
                let before = self.table.selected();
                if !move_selection(&mut self.table, self.discover.items.len(), delta) {
                    return vec![];
                }
                let mut effects = Vec::new();
                if self.table.selected() != before {
                    effects.extend(self.selection_changed());
                }
                effects.extend(self.maybe_load_more());
                effects
            }
            Focus::Album if self.album_pane.show_info => {
                // Scroll the info text; rendering clamps it to the text height.
                let scroll = i64::from(self.album_pane.info_scroll)
                    + i64::try_from(delta.clamp(-1000, 1000)).unwrap_or(0);
                self.album_pane.info_scroll = u16::try_from(scroll.max(0)).unwrap_or(u16::MAX);
                vec![]
            }
            Focus::Album => {
                let len = match self.album_view() {
                    AlbumView::Loaded(album) => album.tracks.len(),
                    _ => 0,
                };
                self.sync_album_pane();
                move_selection(&mut self.album_pane.tracks, len, delta);
                vec![]
            }
        }
    }

    /// Called whenever the discover selection moves to another release.
    fn selection_changed(&mut self) -> Option<Effect> {
        self.sync_album_pane();
        let item = self.selected()?;
        let id = item.id;
        let needs_album = self.albums.peek(id).is_none()
            && !self.album_requests.contains(&id)
            && !self.album_errors.contains_key(&id);
        let needs_art = item.art_id.is_some_and(|art| self.needs_art(art));
        (needs_album || needs_art).then_some(Effect::DebounceAlbum(id))
    }

    fn needs_art(&self, id: ArtId) -> bool {
        self.art_enabled
            && self.arts.peek(id).is_none()
            && !self.art_requests.contains(&id)
            && !self.art_failed.contains(&id)
    }

    fn fetch_art(&mut self, id: Option<ArtId>) -> Option<Effect> {
        let id = id.filter(|&id| self.needs_art(id))?;
        self.art_requests.insert(id);
        Some(Effect::FetchArt(id))
    }

    /// Cover art for the album pane: `None` when there's none to show (disabled, no art, or
    /// it failed), `Some(None)` while loading.
    #[must_use]
    pub fn art_for(&self, id: Option<ArtId>) -> Option<Option<&Art>> {
        let id = id.filter(|_| self.art_enabled)?;
        if self.art_failed.contains(&id) {
            return None;
        }
        Some(self.arts.peek(id))
    }

    fn art_loaded(&mut self, id: ArtId, result: Result<Art, String>) -> Vec<Effect> {
        self.art_requests.remove(&id);
        match result {
            Ok(art) => self.arts.insert(id, art),
            Err(e) => {
                tracing::info!(%id, error = %e, "cover art unavailable");
                self.art_failed.insert(id);
            }
        }
        vec![]
    }

    /// Resets the tracklist when the album pane starts showing a different release.
    fn sync_album_pane(&mut self) {
        let id = self.selected().map(|i| i.id);
        if self.album_pane.for_album == id {
            return;
        }
        self.album_pane.for_album = id;
        self.album_pane.info_scroll = 0;
        let playing = match self.album_view() {
            AlbumView::Loaded(album) => Some(self.playing_index_in(album).unwrap_or(0)),
            _ => None,
        };
        self.album_pane.tracks = TableState::default().with_selected(playing);
    }

    // ---- Album details -------------------------------------------------------------------------

    fn album_due(&mut self, id: ItemId) -> Vec<Effect> {
        // Only fetch if the user is still on that release.
        let Some(item) = self.selected().filter(|i| i.id == id) else {
            return vec![];
        };
        let art_id = item.art_id;
        let mut effects: Vec<Effect> = self.fetch_album(id).into_iter().collect();
        effects.extend(self.fetch_art(art_id));
        effects
    }

    fn fetch_album(&mut self, id: ItemId) -> Option<Effect> {
        if self.albums.peek(id).is_some() || self.album_requests.contains(&id) {
            return None;
        }
        let item = self.discover.items.iter().find(|i| i.id == id)?;
        let effect = Effect::FetchAlbum {
            band_id: item.band_id,
            id,
            kind: item.kind,
        };
        self.album_errors.remove(&id);
        self.album_requests.insert(id);
        Some(effect)
    }

    fn album_loaded(&mut self, id: ItemId, result: Result<Box<Album>, String>) -> Vec<Effect> {
        self.album_requests.remove(&id);
        let from_autoplay = self.autoplay_attempt == Some(id);
        if from_autoplay {
            self.autoplay_attempt = None;
            let playable = matches!(&result, Ok(album) if album.has_playable_tracks());
            if !playable {
                self.pending_play = None;
                if let Err(e) = &result {
                    self.album_errors.insert(id, e.clone());
                } else if let Ok(album) = result {
                    self.albums.insert(id, Arc::from(album));
                }
                return self.autoplay_skip(id);
            }
        }
        let pending = self.pending_play.filter(|(p, _)| *p == id);
        if pending.is_some() {
            self.pending_play = None;
            // Clear "Loading…", but keep autoplay's "Up next: …" visible.
            if !from_autoplay {
                self.message = None;
            }
        }
        match result {
            Ok(album) => {
                let album: Arc<Album> = Arc::from(album);
                self.albums.insert(id, Arc::clone(&album));
                // The tracklist can now get its selection.
                if self.album_pane.for_album == Some(id) {
                    self.album_pane.for_album = None;
                }
                self.sync_album_pane();
                if let Some((_, track)) = pending {
                    return self.play_album(&album, track);
                }
            }
            Err(e) => {
                if pending.is_some() {
                    self.error(format!("Couldn't load release: {e}"));
                }
                self.album_errors.insert(id, e);
            }
        }
        vec![]
    }

    // ---- Playback ------------------------------------------------------------------------------

    fn play_selected_release(&mut self) -> Vec<Effect> {
        let Some(id) = self.selected().map(|i| i.id) else {
            return vec![];
        };
        self.cancel_autoplay_attempt();
        self.play_or_fetch(id, None)
    }

    fn play_selected_track(&mut self) -> Vec<Effect> {
        let AlbumView::Loaded(album) = self.album_view() else {
            return vec![];
        };
        let album = Arc::clone(album);
        let Some(index) = self.album_pane.tracks.selected() else {
            return vec![];
        };
        self.cancel_autoplay_attempt();
        if let Some(track) = album.tracks.get(index)
            && !track.is_playable()
        {
            self.error(format!("{} isn't available for streaming.", track.title));
            return vec![];
        }
        self.play_or_fetch(album.id, Some(index))
    }

    /// Plays release `id` from `track` (or its first playable track), fetching it first if
    /// needed.
    fn play_or_fetch(&mut self, id: ItemId, track: Option<usize>) -> Vec<Effect> {
        if let Some(album) = self.albums.get(id) {
            return self.play_album(&album, track.unwrap_or(0));
        }
        self.pending_play = Some((id, track.unwrap_or(0)));
        self.info("Loading…");
        // Skip the debounce: the user explicitly asked for it.
        self.fetch_album(id).into_iter().collect()
    }

    fn play_album(&mut self, album: &Arc<Album>, start: usize) -> Vec<Effect> {
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
        // Same album: jump within the existing queue instead of rebuilding it.
        if self.now_playing.as_ref().is_some_and(|a| a.id == album.id) {
            return vec![Effect::Player(PlayerCommand::PlayIndex(start))];
        }
        let queue = album.tracks.iter().map(QueueTrack::from).collect();
        self.now_playing = Some(Arc::clone(album));
        vec![Effect::Player(PlayerCommand::PlayQueue(queue, start))]
    }

    fn player_update(&mut self, event: Option<PlayerEvent>, status: Status) -> Vec<Effect> {
        let track_changed = status.index != self.player.index;
        self.player = status;
        if self.player.state == PlaybackState::Playing {
            self.refetched_for_expiry.clear();
        }
        match event {
            Some(PlayerEvent::TrackFailed {
                index,
                expired: true,
                ..
            }) if self
                .now_playing
                .as_ref()
                .is_some_and(|a| !self.refetched_for_expiry.contains(&a.id)) =>
            {
                return self.refetch_expired(index);
            }
            Some(PlayerEvent::TrackFailed { index, message, .. }) => {
                let title = self
                    .now_playing
                    .as_ref()
                    .and_then(|a| a.tracks.get(index))
                    .map_or("track", |t| t.title.as_str());
                self.error(format!("Couldn't play {title}: {message}"));
            }
            Some(PlayerEvent::QueueEnded) => {
                let finished = self.now_playing.take().map(|a| a.id);
                if self.autoplay {
                    self.autoplay_skips = 0;
                    return self.autoplay_next(finished);
                }
            }
            _ => {}
        }
        // Follow the playing track in the tracklist, unless the user is browsing it.
        if track_changed
            && self.focus != Focus::Album
            && let Some(index) = self.player.index
            && let AlbumView::Loaded(album) = self.album_view()
            && self.playing_index_in(album).is_some()
        {
            self.album_pane.tracks.select(Some(index));
        }
        vec![]
    }

    fn toggle_autoplay(&mut self) -> Vec<Effect> {
        self.autoplay = !self.autoplay;
        if !self.autoplay {
            self.cancel_autoplay_attempt();
        }
        self.info(if self.autoplay {
            "Autoplay on: the next Discover release plays when an album ends."
        } else {
            "Autoplay off: playback stops at the end of the album."
        });
        vec![]
    }

    fn open_in_browser(&mut self) -> Vec<Effect> {
        let Some(url) = self.selected().map(|i| i.url.clone()) else {
            return vec![];
        };
        self.info(format!("Opening {url}"));
        vec![Effect::OpenUrl(url)]
    }

    // ---- Autoplay ------------------------------------------------------------------------------

    /// The user chose something to play: forget what autoplay was about to start.
    fn cancel_autoplay_attempt(&mut self) {
        self.autoplay_attempt = None;
        self.autoplay_after = None;
    }

    /// Plays the first playable Discover release after `after` (or from the selection if
    /// `after` isn't in the list anymore). Loads more pages if needed.
    fn autoplay_next(&mut self, after: Option<ItemId>) -> Vec<Effect> {
        if !self.player_available {
            return vec![];
        }
        let start = after
            .and_then(|id| self.discover.items.iter().position(|i| i.id == id))
            .map_or_else(|| self.table.selected().unwrap_or(0), |i| i + 1);
        let candidate = self
            .discover
            .items
            .iter()
            .enumerate()
            .skip(start)
            .find(|(_, item)| !item.is_preorder && Some(item.id) != after)
            .map(|(index, item)| (index, item.id, item.title.clone(), item.artist.clone()));

        let Some((index, id, title, artist)) = candidate else {
            if self.discover.has_more() && !self.discover.loading {
                // Wait for the next page, then continue from the last loaded release.
                self.autoplay_after = self.discover.items.last().map(|i| i.id).or(after);
                return self.load_next_page().into_iter().collect();
            }
            if self.discover.loading {
                self.autoplay_after = self.discover.items.last().map(|i| i.id).or(after);
                return vec![];
            }
            self.info("Autoplay: that was the last release in Discover.");
            return vec![];
        };

        let mut effects = Vec::new();
        // Follow along in the list, unless the user has moved off the album that just ended.
        if after.is_none() || self.selected().map(|i| i.id) == after {
            self.table.select(Some(index));
            effects.extend(self.selection_changed());
            effects.extend(self.maybe_load_more());
        }

        if let Some(album) = self.albums.get(id) {
            if !album.has_playable_tracks() {
                effects.extend(self.autoplay_skip(id));
                return effects;
            }
            effects.extend(self.play_album(&album, 0));
        } else {
            self.autoplay_attempt = Some(id);
            effects.extend(self.play_or_fetch(id, None));
        }
        self.info(format!("Up next: {title} — {artist}"));
        effects
    }

    fn autoplay_skip(&mut self, id: ItemId) -> Vec<Effect> {
        self.autoplay_skips += 1;
        if self.autoplay_skips >= AUTOPLAY_MAX_SKIPS {
            self.error("Autoplay stopped: the next few releases couldn't be played.");
            return vec![];
        }
        tracing::info!(%id, "autoplay: skipping unplayable release");
        self.autoplay_next(Some(id))
    }

    /// Requests the next discover page regardless of where the selection is.
    fn load_next_page(&mut self) -> Option<Effect> {
        if self.discover.loading || !self.discover.has_more() {
            return None;
        }
        self.discover.loading = true;
        Some(Effect::FetchDiscover {
            query: self.query.clone(),
            cursor: self.discover.next_cursor.clone(),
            generation: self.discover.generation,
        })
    }

    /// Stream URLs are signed for about a day; re-fetch the album for fresh ones and resume at
    /// the track that failed.
    fn refetch_expired(&mut self, index: usize) -> Vec<Effect> {
        let Some(album) = self.now_playing.take() else {
            return vec![];
        };
        tracing::info!(album = %album.id, "stream URLs expired; re-fetching album");
        self.refetched_for_expiry.insert(album.id);
        self.albums.remove(album.id);
        self.album_requests.insert(album.id);
        self.pending_play = Some((album.id, index));
        self.info("Stream links expired; refreshing…");
        vec![Effect::FetchAlbum {
            band_id: album.band.id,
            id: album.id,
            kind: album.kind,
        }]
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
        vec![
            Effect::Player(PlayerCommand::SetVolume(volume)),
            self.save_settings(),
        ]
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
                | Self::AlbumDue(_)
                | Self::AlbumLoaded { .. }
                | Self::ArtLoaded { .. }
                | Self::Player { .. }
        )
    }
}

/// Moves a table selection by `delta`, clamped to `len` rows. Returns `false` if empty.
fn move_selection(table: &mut TableState, len: usize, delta: isize) -> bool {
    if len == 0 {
        return false;
    }
    let current = table.selected().unwrap_or(0);
    table.select(Some(current.saturating_add_signed(delta).min(len - 1)));
    true
}

fn page(rows: usize) -> isize {
    isize::try_from(rows.saturating_sub(1).max(1)).unwrap_or(isize::MAX)
}

/// Small least-recently-used cache: album details (a few KB each) and encoded cover art.
#[derive(Debug)]
struct Lru<K, V> {
    capacity: usize,
    map: HashMap<K, V>,
    /// Most recently used at the back.
    order: VecDeque<K>,
}

impl<K: Copy + Eq + std::hash::Hash, V: Clone> Lru<K, V> {
    /// Allocates nothing until the first insert, so an unused cache (e.g. art while it's
    /// disabled) costs nothing.
    fn new(capacity: usize) -> Self {
        Self {
            capacity,
            map: HashMap::new(),
            order: VecDeque::new(),
        }
    }

    /// Looks up without counting as a use (for rendering).
    fn peek(&self, key: K) -> Option<&V> {
        self.map.get(&key)
    }

    /// Looks up and marks as recently used.
    fn get(&mut self, key: K) -> Option<V> {
        let value = self.map.get(&key)?.clone();
        self.touch(key);
        Some(value)
    }

    fn insert(&mut self, key: K, value: V) {
        if self.map.insert(key, value).is_some() {
            self.touch(key);
            return;
        }
        self.order.push_back(key);
        if self.order.len() > self.capacity
            && let Some(oldest) = self.order.pop_front()
        {
            self.map.remove(&oldest);
        }
    }

    fn remove(&mut self, key: K) {
        if self.map.remove(&key).is_some() {
            self.order.retain(|&k| k != key);
        }
    }

    fn touch(&mut self, key: K) {
        if let Some(pos) = self.order.iter().position(|&k| k == key) {
            self.order.remove(pos);
            self.order.push_back(key);
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

    fn album_fixture(name: &str) -> Box<Album> {
        let json = match name {
            "album" => include_str!("../tests/fixtures/tralbum_album.json"),
            "preorder" => include_str!("../tests/fixtures/tralbum_preorder.json"),
            _ => unreachable!(),
        };
        Box::new(crate::bandcamp::parse_album(json).unwrap())
    }

    /// An app with the fixture page loaded. The first item (Ocean Company, the album fixture)
    /// is selected and its details are debounced.
    fn loaded() -> App {
        let (mut app, effects) = App::new(DiscoverQuery::default(), true);
        let Some(Effect::FetchDiscover { generation, .. }) = effects.first().cloned() else {
            panic!("expected initial fetch, got {effects:?}");
        };
        app.page_rows = 3;
        let effects = app.update(Action::DiscoverLoaded {
            generation,
            result: Ok(page_fixture()),
        });
        assert!(
            matches!(
                effects.as_slice(),
                [
                    Effect::DebounceAlbum(ItemId(3_457_085_593)),
                    Effect::FetchDiscover {
                        cursor: Some(_),
                        ..
                    }
                ]
            ),
            "{effects:?}"
        );
        app
    }

    /// `loaded()` plus the first album's details.
    fn with_album() -> App {
        let mut app = loaded();
        let id = app.selected().unwrap().id;
        assert!(matches!(
            app.update(Action::AlbumDue(id))[..],
            [Effect::FetchAlbum { .. }]
        ));
        app.update(Action::AlbumLoaded {
            id,
            result: Ok(album_fixture("album")),
        });
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
            matches!(&effects[..], [Effect::FetchDiscover { query, cursor: None, .. }, Effect::SaveSettings(_)] if query.slice == Slice::New)
        );
        app.update(Action::DiscoverLoaded {
            generation: old,
            result: Ok(page_fixture()),
        });
        assert_eq!(app.discover.items, [], "stale page was applied");
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
    fn details_are_debounced_and_only_fetched_if_still_selected() {
        let mut app = loaded();
        let first = app.selected().unwrap().id;
        // Scroll past the first release before its delay is up.
        let effects = app.update(Action::Down);
        let [Effect::DebounceAlbum(second)] = effects[..] else {
            panic!("{effects:?}");
        };
        assert_eq!(
            app.update(Action::AlbumDue(first)),
            [],
            "moved away; no fetch"
        );
        assert!(matches!(
            app.update(Action::AlbumDue(second))[..],
            [Effect::FetchAlbum { id, .. }] if id == second
        ));
        // In flight: neither selecting it again nor another due fires a second request.
        app.update(Action::Up);
        assert_eq!(app.update(Action::Down), []);
        assert_eq!(app.update(Action::AlbumDue(second)), []);
    }

    #[test]
    fn album_view_follows_loading_states() {
        let mut app = loaded();
        assert!(matches!(app.album_view(), AlbumView::Loading(_)));
        let id = app.selected().unwrap().id;
        app.update(Action::AlbumDue(id));
        app.update(Action::AlbumLoaded {
            id,
            result: Err("boom".into()),
        });
        assert!(matches!(app.album_view(), AlbumView::Failed(_, "boom")));
        assert!(
            app.message.is_none(),
            "background failures stay in the pane"
        );
        // Refresh clears errors so it can be retried.
        app.update(Action::Refresh);
        assert!(app.album_errors.is_empty());

        let app = with_album();
        let AlbumView::Loaded(album) = app.album_view() else {
            panic!();
        };
        assert_eq!(album.title, "Ocean Company");
        assert_eq!(app.album_pane.tracks.selected(), Some(0));
    }

    #[test]
    fn play_from_discover_fetches_immediately_then_queues() {
        let mut app = loaded();
        let effects = app.update(Action::Play);
        let [Effect::FetchAlbum { id, .. }] = effects[..] else {
            panic!("expected album fetch, got {effects:?}");
        };
        let effects = app.update(Action::AlbumLoaded {
            id,
            result: Ok(album_fixture("album")),
        });
        let [Effect::Player(PlayerCommand::PlayQueue(queue, 0))] = &effects[..] else {
            panic!("expected play, got {effects:?}");
        };
        assert_eq!(queue.len(), 10);
        assert_eq!(app.now_playing.as_ref().unwrap().title, "Ocean Company");
    }

    #[test]
    fn play_from_tracklist_starts_at_that_track() {
        let mut app = with_album();
        app.update(Action::ToggleFocus);
        assert_eq!(app.focus, Focus::Album);
        app.update(Action::Down);
        app.update(Action::Down);
        let effects = app.update(Action::Play);
        assert!(
            matches!(&effects[..], [Effect::Player(PlayerCommand::PlayQueue(q, 2))] if q.len() == 10),
            "{effects:?}"
        );
        // Same album again: jump within the queue rather than rebuilding it.
        app.update(Action::Down);
        assert_eq!(
            app.update(Action::Play),
            [Effect::Player(PlayerCommand::PlayIndex(3))]
        );
    }

    #[test]
    fn unplayable_track_shows_an_error() {
        let mut app = loaded();
        let id = app.selected().unwrap().id;
        app.update(Action::AlbumDue(id));
        app.update(Action::AlbumLoaded {
            id,
            result: Ok(album_fixture("preorder")),
        });
        app.update(Action::FocusAlbum);
        assert_eq!(app.update(Action::Play), []);
        let message = app.message.clone().unwrap();
        assert_eq!(message.kind, MessageKind::Error);
        assert!(message.text.contains("isn't available"), "{}", message.text);

        app.update(Action::FocusDiscover);
        assert_eq!(app.update(Action::Play), []);
        assert!(app.message.unwrap().text.contains("pre-order"));
    }

    #[test]
    fn tracklist_follows_the_playing_track() {
        let mut app = with_album();
        app.update(Action::Play);
        let status = |index| Status {
            state: PlaybackState::Playing,
            index: Some(index),
            ..Status::default()
        };
        app.update(Action::Player {
            event: Some(PlayerEvent::TrackChanged(Some(4))),
            status: status(4),
        });
        assert_eq!(app.album_pane.tracks.selected(), Some(4));
        // Not while the user is browsing the tracklist.
        app.update(Action::FocusAlbum);
        app.update(Action::Up);
        app.update(Action::Player {
            event: Some(PlayerEvent::TrackChanged(Some(5))),
            status: status(5),
        });
        assert_eq!(app.album_pane.tracks.selected(), Some(3));
    }

    #[test]
    fn info_view_scrolls_instead_of_moving_tracks() {
        let mut app = with_album();
        app.update(Action::FocusAlbum);
        app.update(Action::ToggleInfo);
        app.update(Action::Down);
        app.update(Action::Down);
        assert_eq!(app.album_pane.info_scroll, 2);
        assert_eq!(app.album_pane.tracks.selected(), Some(0));
        app.update(Action::Top);
        assert_eq!(app.album_pane.info_scroll, 0);
    }

    #[test]
    fn picking_a_genre_reloads_and_saves() {
        let mut app = loaded();
        app.update(Action::OpenPicker(PickerKind::Genre));
        // Keys go to the picker, not the list.
        app.update(Action::Down);
        assert_eq!(app.table.selected(), Some(0));
        for c in "techno".chars() {
            app.update(Action::PickerInput(c));
        }
        let effects = app.update(Action::Play);
        assert!(app.picker.is_none());
        assert_eq!(app.query.tags, ["electronic", "techno"]);
        assert!(
            matches!(&effects[..], [Effect::FetchDiscover { query, cursor: None, .. }, Effect::SaveSettings(s)]
                if query.tags == ["electronic", "techno"] && s.discover.tags == ["electronic", "techno"]),
            "{effects:?}"
        );
    }

    #[test]
    fn picking_the_current_value_or_cancelling_changes_nothing() {
        let mut app = loaded();
        app.update(Action::OpenPicker(PickerKind::Time));
        assert_eq!(app.update(Action::Play), [], "Any time is already selected");
        app.update(Action::OpenPicker(PickerKind::Location));
        app.update(Action::PickerInput('b'));
        assert_eq!(app.update(Action::ClosePicker), []);
        assert!(app.picker.is_none());
        assert_eq!(app.query.geoname_id, 0);
    }

    #[test]
    fn api_results_still_arrive_while_the_picker_is_open() {
        let mut app = loaded();
        app.update(Action::OpenPicker(PickerKind::Genre));
        let generation = app.discover.generation;
        app.update(Action::DiscoverLoaded {
            generation,
            result: Ok(page_fixture()),
        });
        assert_eq!(app.discover.items.len(), 8);
        assert!(app.picker.is_some());
    }

    #[test]
    fn expired_stream_urls_refetch_the_album_once() {
        let mut app = with_album();
        app.update(Action::Play);
        let id = app.now_playing.as_ref().unwrap().id;
        let expired = |index| Action::Player {
            event: Some(PlayerEvent::TrackFailed {
                index,
                message: "HTTP 403".into(),
                expired: true,
            }),
            status: Status::default(),
        };

        let effects = app.update(expired(3));
        assert!(
            matches!(&effects[..], [Effect::FetchAlbum { id: fetched, .. }] if *fetched == id),
            "{effects:?}"
        );
        assert!(app.message.as_ref().unwrap().text.contains("expired"));
        // Fresh details arrive: resume at the track that failed, with a new queue.
        let effects = app.update(Action::AlbumLoaded {
            id,
            result: Ok(album_fixture("album")),
        });
        assert!(
            matches!(
                &effects[..],
                [Effect::Player(PlayerCommand::PlayQueue(_, 3))]
            ),
            "{effects:?}"
        );

        // Expiring again before anything played: report it rather than loop.
        assert_eq!(app.update(expired(3)), []);
        assert_eq!(app.message.as_ref().unwrap().kind, MessageKind::Error);

        // Once playback works again, a later expiry is retried again.
        app.update(Action::Play);
        app.update(Action::Player {
            event: None,
            status: Status {
                state: PlaybackState::Playing,
                index: Some(0),
                ..Status::default()
            },
        });
        assert!(matches!(
            app.update(expired(0))[..],
            [Effect::FetchAlbum { .. }]
        ));
    }

    #[test]
    fn open_in_browser_uses_the_selected_release() {
        let mut app = loaded();
        app.update(Action::Down);
        let effects = app.update(Action::OpenInBrowser);
        assert!(
            matches!(&effects[..], [Effect::OpenUrl(url)] if url.as_str()
                == "https://n4records.bandcamp.com/album/pete-cannon-from-the-manor-house-n4lp001"),
            "{effects:?}"
        );
    }

    #[test]
    fn art_is_fetched_with_details_only_when_enabled() {
        // Disabled (the default): no art effects at all.
        let mut app = loaded();
        let id = app.selected().unwrap().id;
        assert!(matches!(
            app.update(Action::AlbumDue(id))[..],
            [Effect::FetchAlbum { .. }]
        ));

        let mut app = loaded();
        app.art_enabled = true;
        let item = app.selected().unwrap().clone();
        let art_id = item.art_id.unwrap();
        assert_eq!(app.art_for(Some(art_id)), Some(None), "loading");
        let effects = app.update(Action::AlbumDue(item.id));
        assert!(
            matches!(&effects[..], [Effect::FetchAlbum { .. }, Effect::FetchArt(a)] if *a == art_id),
            "{effects:?}"
        );
        let art = crate::art::encode(
            &ratatui_image::picker::Picker::halfblocks(),
            &crate::art::tests::test_jpeg(),
        )
        .unwrap();
        app.update(Action::ArtLoaded {
            id: art_id,
            result: Ok(art.clone()),
        });
        assert_eq!(app.art_for(Some(art_id)), Some(Some(&art)));

        // Leaving and coming back: details and art are cached, nothing to fetch.
        app.update(Action::AlbumLoaded {
            id: item.id,
            result: Ok(album_fixture("album")),
        });
        app.update(Action::Down);
        assert!(matches!(app.update(Action::Up)[..], []));
    }

    #[test]
    fn failed_art_falls_back_to_text_and_isnt_retried() {
        let mut app = loaded();
        app.art_enabled = true;
        let item = app.selected().unwrap().clone();
        let art_id = item.art_id.unwrap();
        app.update(Action::AlbumDue(item.id));
        app.update(Action::ArtLoaded {
            id: art_id,
            result: Err("404".into()),
        });
        assert_eq!(app.art_for(Some(art_id)), None);
        assert!(app.fetch_art(Some(art_id)).is_none());
        assert_eq!(app.art_for(None), None, "releases without art have none");
    }

    /// Plays the first release (Ocean Company) and lets it finish.
    fn finish_first_album(app: &mut App) -> Vec<Effect> {
        app.update(Action::Play);
        assert_eq!(app.now_playing.as_ref().unwrap().title, "Ocean Company");
        app.update(Action::Player {
            event: Some(PlayerEvent::QueueEnded),
            status: Status::default(),
        })
    }

    fn fetched_album(effects: &[Effect]) -> Option<ItemId> {
        effects.iter().find_map(|e| match e {
            Effect::FetchAlbum { id, .. } => Some(*id),
            _ => None,
        })
    }

    /// A page of new releases: the fixture with fresh ids.
    fn more_releases(offset: u64) -> DiscoverPage {
        let mut page = page_fixture();
        for item in &mut page.items {
            item.id = ItemId(item.id.0 + offset);
        }
        page
    }

    #[test]
    fn autoplay_continues_with_the_next_release_and_follows_in_the_list() {
        let mut app = with_album();
        let next = app.discover.items[1].id;
        let effects = finish_first_album(&mut app);
        assert_eq!(fetched_album(&effects), Some(next), "{effects:?}");
        assert_eq!(app.table.selected(), Some(1), "selection follows");
        assert!(
            app.message
                .as_ref()
                .unwrap()
                .text
                .starts_with("Up next: Pete Cannon")
        );

        let effects = app.update(Action::AlbumLoaded {
            id: next,
            result: Ok(album_fixture("album")),
        });
        assert!(
            matches!(
                &effects[..],
                [Effect::Player(PlayerCommand::PlayQueue(_, 0))]
            ),
            "{effects:?}"
        );
        assert!(
            app.message.as_ref().unwrap().text.starts_with("Up next"),
            "message kept"
        );
    }

    #[test]
    fn autoplay_skips_preorders_and_unplayable_releases() {
        let mut app = with_album();
        app.discover.items[1].is_preorder = true;
        let (third, fourth) = (app.discover.items[2].id, app.discover.items[3].id);
        let effects = finish_first_album(&mut app);
        assert_eq!(fetched_album(&effects), Some(third), "pre-order skipped");

        // The third has nothing streamable: move on to the fourth.
        let effects = app.update(Action::AlbumLoaded {
            id: third,
            result: Ok(album_fixture("preorder")),
        });
        assert_eq!(fetched_album(&effects), Some(fourth), "{effects:?}");
        assert_ne!(app.message.as_ref().unwrap().kind, MessageKind::Error);
    }

    #[test]
    fn autoplay_does_not_move_the_selection_while_browsing() {
        let mut app = with_album();
        app.update(Action::Play);
        app.update(Action::Bottom);
        let browsing = app.table.selected();
        let effects = app.update(Action::Player {
            event: Some(PlayerEvent::QueueEnded),
            status: Status::default(),
        });
        assert_eq!(fetched_album(&effects), Some(app.discover.items[1].id));
        assert_eq!(app.table.selected(), browsing);
    }

    #[test]
    fn autoplay_loads_the_next_page_at_the_end_of_the_list() {
        let mut app = with_album();
        let generation = app.discover.generation;
        // Finish the last loaded release while the next page is still on its way.
        app.update(Action::Bottom);
        let last = app.selected().unwrap().id;
        app.update(Action::AlbumDue(last));
        let mut album = album_fixture("album");
        album.id = last; // details of the release we asked for
        app.update(Action::AlbumLoaded {
            id: last,
            result: Ok(album),
        });
        app.update(Action::Play);
        let effects = app.update(Action::Player {
            event: Some(PlayerEvent::QueueEnded),
            status: Status::default(),
        });
        assert_eq!(fetched_album(&effects), None, "nothing after it yet");

        let effects = app.update(Action::DiscoverLoaded {
            generation,
            result: Ok(more_releases(1)),
        });
        assert_eq!(
            fetched_album(&effects),
            Some(app.discover.items[4].id),
            "{effects:?}"
        );
    }

    #[test]
    fn autoplay_gives_up_after_repeated_failures() {
        let mut app = with_album();
        let generation = app.discover.generation;
        for offset in [1, 2] {
            app.update(Action::DiscoverLoaded {
                generation,
                result: Ok(more_releases(offset)),
            });
        }
        let mut effects = finish_first_album(&mut app);
        let mut attempts = 0;
        while let Some(id) = fetched_album(&effects) {
            attempts += 1;
            effects = app.update(Action::AlbumLoaded {
                id,
                result: Err("HTTP 500".into()),
            });
        }
        assert_eq!(attempts, 5);
        assert_eq!(app.message.as_ref().unwrap().kind, MessageKind::Error);
        assert!(
            app.message
                .as_ref()
                .unwrap()
                .text
                .contains("Autoplay stopped")
        );
    }

    #[test]
    fn autoplay_off_stops_at_the_end_and_can_be_toggled() {
        let mut app = with_album();
        app.update(Action::ToggleAutoplay);
        assert!(!app.autoplay);
        assert!(
            app.message
                .as_ref()
                .unwrap()
                .text
                .starts_with("Autoplay off")
        );
        assert_eq!(fetched_album(&finish_first_album(&mut app)), None);
        assert!(app.now_playing.is_none());

        app.update(Action::ToggleAutoplay);
        assert!(app.autoplay);
    }

    #[test]
    fn choosing_something_else_cancels_autoplay() {
        let mut app = with_album();
        let next = app.discover.items[1].id;
        finish_first_album(&mut app);
        // While the next release loads, the user picks the third one themselves.
        app.update(Action::Down);
        app.update(Action::Play);
        let effects = app.update(Action::AlbumLoaded {
            id: next,
            result: Err("HTTP 500".into()),
        });
        assert_eq!(
            effects,
            [],
            "a cancelled autoplay attempt doesn't skip onwards"
        );
    }

    #[test]
    fn transport_keys_do_nothing_until_something_plays() {
        let mut app = loaded();
        assert_eq!(app.update(Action::TogglePause), []);
        assert_eq!(app.update(Action::NextTrack), []);
        app.now_playing = Some(Arc::from(album_fixture("album")));
        assert_eq!(
            app.update(Action::SeekBackward),
            [Effect::Player(PlayerCommand::SeekBy(-10_000))]
        );
    }

    #[test]
    fn volume_steps_and_clamps() {
        let mut app = loaded();
        let effects = app.update(Action::VolumeDown);
        assert_eq!(effects[0], Effect::Player(PlayerCommand::SetVolume(0.95)));
        assert!(
            matches!(&effects[1], Effect::SaveSettings(s) if (s.volume - 0.95).abs() < 1e-6),
            "{effects:?}"
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
    fn album_cache_evicts_least_recently_used() {
        let mut cache: Lru<ItemId, Arc<Album>> = Lru::new(2);
        let album: Arc<Album> = Arc::from(album_fixture("album"));
        cache.insert(ItemId(1), Arc::clone(&album));
        cache.insert(ItemId(2), Arc::clone(&album));
        cache.get(ItemId(1)); // 1 is now more recent than 2
        cache.insert(ItemId(3), Arc::clone(&album));
        assert!(cache.peek(ItemId(1)).is_some());
        assert!(cache.peek(ItemId(2)).is_none());
        assert!(cache.peek(ItemId(3)).is_some());
        assert_eq!(cache.map.len(), 2);
    }
}
