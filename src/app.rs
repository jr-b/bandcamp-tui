//! Application state and the pure `update` function.
//!
//! `App::update` takes an [`Action`] (a keypress, an API result, a player event) and returns
//! the [`Effect`]s to run (API calls, player commands). It never does I/O itself, so the whole
//! app's behaviour is unit-testable; `run.rs` executes the effects.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;

use ratatui::widgets::{ListState, TableState};

use crate::art::{Art, ArtBox};
use crate::bandcamp::images::ImageSize;
use crate::bandcamp::{
    Album, ArtId, Artist, BandId, Cursor, DiscoverItem, DiscoverPage, DiscoverQuery, Filters,
    ItemId, SearchResult, Slice, TralbumKind,
};
use crate::picker::{Picker, PickerKind};
use crate::player::{PlaybackState, PlayerEvent, QueueTrack, Status};
use crate::queue::{Queue, QueueEntry};
use crate::settings::Settings;

/// Fetch the next page when the selection gets this close to the end of the list.
const LOAD_MORE_THRESHOLD: usize = 10;
/// Stop paging after this many releases to keep memory bounded.
pub const MAX_ITEMS: usize = 1000;
const ALBUM_CACHE_SIZE: usize = 64;
/// Encoded covers (one per size), ~10–200 KB each depending on protocol and size.
const ART_CACHE_SIZE: usize = 16;
/// Downloaded cover JPEGs (~10–80 KB each), kept so a resize can re-encode without refetching.
const ART_SOURCE_CACHE_SIZE: usize = 32;
const VOLUME_STEP: f32 = 0.05;
const SEEK_STEP_MS: i64 = 10_000;
/// Autoplay gives up after this many releases in a row that can't be played.
const AUTOPLAY_MAX_SKIPS: u8 = 5;
/// Fetch the next queue entry's details this long before the current release ends, so the
/// switch only waits for the stream to start.
const QUEUE_PREFETCH: std::time::Duration = std::time::Duration::from_secs(30);

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
    /// Switch between browsing and the listening view (`v`).
    ToggleListening,
    /// Continue with the next Discover release when an album ends.
    ToggleAutoplay,
    /// Open the search box (`/`).
    OpenSearch,
    /// Show an informational message in the footer.
    Notice(String),
    /// Add the selected release (or, in the tracklist, track) to the end of the queue.
    Enqueue,
    /// Same, but play it next.
    EnqueueNext,
    /// Open the queue popup (`u`).
    OpenQueue,
    /// Queue popup: remove / move the highlighted entry, or clear everything.
    QueueRemove,
    QueueMoveUp,
    QueueMoveDown,
    QueueClear,
    /// Leave search results or an artist's releases and return to Discover.
    BackToDiscover,
    /// The debounce delay for search `generation` has passed.
    SearchDue(u64),
    SearchLoaded {
        generation: u64,
        result: Result<Vec<SearchResult>, String>,
    },
    ArtistLoaded {
        band_id: BandId,
        /// Boxed: a discography can be large.
        result: Result<Box<Artist>, String>,
    },
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
    ClosePopup,
    /// Text typed into the picker's search.
    TypeChar(char),
    Backspace,
    ClearInput,
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
    /// A cover's JPEG arrived (at `size`).
    ArtFetched {
        id: ArtId,
        size: ImageSize,
        result: Result<bytes::Bytes, String>,
    },
    /// A cover was encoded for the terminal at `Art::art_box`.
    ArtEncoded {
        id: ArtId,
        art_box: ArtBox,
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
    /// Download a cover JPEG (only when art is enabled).
    FetchArt {
        id: ArtId,
        size: ImageSize,
    },
    /// Encode a downloaded cover for the terminal at `art_box` (off the UI thread).
    EncodeArt {
        id: ArtId,
        jpeg: bytes::Bytes,
        art_box: ArtBox,
    },
    /// Send [`Action::SearchDue`] after a short delay, replacing any pending one.
    DebounceSearch(u64),
    FetchSearch {
        query: String,
        generation: u64,
    },
    FetchArtist(BandId),
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

/// What the body of the screen shows. Remembered between runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Screen {
    /// The release list and the album pane.
    #[default]
    Browse,
    /// Only what's playing: a large cover, the track and the progress.
    Listening,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Focus {
    #[default]
    Discover,
    Album,
}

/// What [`App::plays_next`] found.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum UpNext<'a> {
    /// The next track of the album playing.
    Track(&'a crate::bandcamp::Track),
    /// A queue entry, and how many more follow it.
    Queue(&'a QueueEntry, usize),
    /// Autoplay picks the next Discover release.
    Autoplay,
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

#[derive(Debug, Clone, Copy)]
enum Popup {
    Queue,
    Search,
    Help,
}

/// What the release list is showing.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum ListSource {
    #[default]
    Discover,
    /// Album and track results for this query.
    Search(String),
    /// An artist's or label's releases.
    Artist(String),
}

/// The `/` search popup. Kept while closed, so reopening shows the last search.
#[derive(Debug, Default)]
pub struct SearchBox {
    pub query: String,
    pub results: Vec<SearchResult>,
    pub list: ListState,
    pub loading: bool,
    pub error: Option<String>,
    /// Bumped on every edit so only the latest query's results are shown.
    generation: u64,
}

impl SearchBox {
    fn selected(&self) -> Option<&SearchResult> {
        self.results.get(self.list.selected()?)
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
    /// Where the cover fits in the pane as last drawn (`None`: no room, text only).
    pub art_box: Option<ArtBox>,
}

/// State of the listening view.
#[derive(Debug, Default)]
pub struct ListeningPane {
    /// Where the cover fits as last drawn (`None`: no room, art off, or the info text shows).
    pub art_box: Option<ArtBox>,
    /// Show the playing album's about/credits/bio instead of the cover (`i`).
    pub show_info: bool,
    pub info_scroll: u16,
    /// Which album `info_scroll` belongs to; rendering resets it for another album.
    pub info_for: Option<ItemId>,
    /// Rows of info text visible, set while rendering; used for page up/down.
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
    /// The release list on the left. Despite the name, it can hold search results or an
    /// artist's releases too; `source` says which.
    pub discover: DiscoverList,
    pub source: ListSource,
    /// The Discover list (and its selection) while search results or an artist are shown.
    saved_discover: Option<(DiscoverList, TableState)>,
    pub search: SearchBox,
    pub search_open: bool,
    /// The artist whose releases are loading into the list.
    pending_artist: Option<BandId>,
    /// Source of unique list generations, shared by every list the left pane shows.
    list_generation: u64,
    /// Selection and scroll offset of the discover table; the offset is maintained by rendering.
    pub table: TableState,
    /// Rows visible in the discover table, set while rendering; used for page up/down.
    pub page_rows: usize,
    pub focus: Focus,
    pub screen: Screen,
    /// Switch to the listening view whenever the queue starts a release (`config.toml`).
    pub listen_on_queue: bool,
    pub album_pane: AlbumPane,
    pub listening: ListeningPane,
    /// Open filter popup, if any. While open it gets all keys.
    pub picker: Option<Picker>,
    albums: Lru<ItemId, Arc<Album>>,
    /// Whether the terminal shows cover art (decided once at startup).
    pub art_enabled: bool,
    /// The terminal's cell size in pixels, for sizing covers (set with `art_enabled`).
    pub art_font: ratatui_image::FontSize,
    art_sources: Lru<ArtId, ArtSource>,
    arts: Lru<(ArtId, ArtBox), Art>,
    /// Downloads in flight.
    art_requests: HashSet<ArtId>,
    /// Encodes in flight.
    art_encodes: HashSet<(ArtId, ArtBox)>,
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
    /// Set when only one track of `now_playing` is playing (a single-track queue entry).
    now_playing_only: Option<usize>,
    /// Releases and tracks to play after the current one.
    pub queue: Queue,
    /// The queue popup, if open, with its highlighted entry.
    pub queue_view: Option<ListState>,
    /// The queue entry whose details are loading, to play when they arrive: (release, track).
    queue_attempt: Option<(ItemId, Option<usize>)>,
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
            source: ListSource::Discover,
            saved_discover: None,
            search: SearchBox::default(),
            search_open: false,
            pending_artist: None,
            list_generation: 0,
            table: TableState::default(),
            page_rows: 10,
            focus: Focus::Discover,
            screen: Screen::Browse,
            listen_on_queue: false,
            album_pane: AlbumPane::default(),
            listening: ListeningPane::default(),
            picker: None,
            albums: Lru::new(ALBUM_CACHE_SIZE),
            art_enabled: false,
            art_font: ratatui_image::FontSize::new(10, 20),
            art_sources: Lru::new(ART_SOURCE_CACHE_SIZE),
            arts: Lru::new(ART_CACHE_SIZE),
            art_requests: HashSet::new(),
            art_encodes: HashSet::new(),
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
            now_playing_only: None,
            queue: Queue::default(),
            queue_view: None,
            queue_attempt: None,
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

    /// Track count and length for a list row. Discover leaves these out for some releases
    /// (and search and artist pages never have them), so fall back to the album details once
    /// they're cached.
    #[must_use]
    pub fn release_size(&self, item: &DiscoverItem) -> (Option<u32>, Option<std::time::Duration>) {
        let album = self.albums.peek(item.id);
        let tracks = item
            .track_count
            .or_else(|| album.and_then(|a| u32::try_from(a.tracks.len()).ok()));
        let duration = item
            .duration
            .or_else(|| album.map(|a| a.duration()).filter(|d| !d.is_zero()));
        (tracks, duration)
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
        if self.search_open
            && let Some(effects) = self.search_update(&action)
        {
            return effects;
        }
        if self.queue_view.is_some()
            && let Some(effects) = self.queue_update(&action)
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
            Action::ToggleFocus
            | Action::FocusDiscover
            | Action::FocusAlbum
            | Action::ToggleInfo
            | Action::CloseHelp => {
                self.view_update(&action);
                vec![]
            }
            Action::ToggleListening => {
                self.screen = match self.screen {
                    Screen::Browse => Screen::Listening,
                    Screen::Listening => Screen::Browse,
                };
                vec![self.save_settings()]
            }
            Action::ToggleAutoplay => self.toggle_autoplay(),
            Action::Enqueue => self.enqueue(false),
            Action::EnqueueNext => self.enqueue(true),
            Action::OpenQueue => self.open_popup(Popup::Queue),
            Action::OpenSearch => self.open_popup(Popup::Search),
            Action::BackToDiscover => self.back_to_discover(),
            Action::OpenInBrowser => self.open_in_browser(),
            Action::ShowHelp => self.open_popup(Popup::Help),
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
            Action::ClosePopup
            | Action::TypeChar(_)
            | Action::Backspace
            | Action::ClearInput
            | Action::Resize
            | Action::QueueRemove
            | Action::QueueMoveUp
            | Action::QueueMoveDown
            | Action::QueueClear => vec![],
            other @ (Action::DiscoverLoaded { .. }
            | Action::AlbumDue(_)
            | Action::AlbumLoaded { .. }
            | Action::ArtFetched { .. }
            | Action::ArtEncoded { .. }
            | Action::Player { .. }
            | Action::Notice(_)
            | Action::SearchDue(_)
            | Action::SearchLoaded { .. }
            | Action::ArtistLoaded { .. }) => self.background_update(other),
        }
    }

    /// Results arriving from the network, the image encoder and the player.
    fn background_update(&mut self, action: Action) -> Vec<Effect> {
        match action {
            Action::DiscoverLoaded { generation, result } => {
                self.discover_loaded(generation, result)
            }
            Action::AlbumDue(id) => self.album_due(id),
            Action::AlbumLoaded { id, result } => self.album_loaded(id, result),
            Action::ArtFetched { id, size, result } => self.art_fetched(id, size, result),
            Action::ArtEncoded {
                id,
                art_box,
                result,
            } => self.art_encoded(id, art_box, result),
            Action::Player { event, status } => self.player_update(event, status),
            Action::Notice(text) => {
                self.info(text);
                vec![]
            }
            Action::SearchDue(generation) => self.search_due(generation),
            Action::SearchLoaded { generation, result } => self.search_loaded(generation, result),
            Action::ArtistLoaded { band_id, result } => self.artist_loaded(band_id, result),
            _ => vec![],
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
            Action::TypeChar(c) => picker.push(*c),
            Action::Backspace => picker.backspace(),
            Action::ClearInput => picker.clear(),
            Action::ClosePopup => self.picker = None,
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
        let mut settings = Settings::from_state(&self.query, self.player.volume);
        settings.queue = self.queue.to_saved();
        settings.screen = self.screen;
        settings
    }

    fn save_settings(&self) -> Effect {
        Effect::SaveSettings(self.settings())
    }

    // ---- Discover ------------------------------------------------------------------------------

    /// Starts a fresh discover load for the current query.
    fn reload(&mut self) -> Effect {
        // Filters and refresh always mean Discover.
        self.source = ListSource::Discover;
        self.saved_discover = None;
        self.pending_artist = None;
        let generation = self.next_list_generation();
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
            _ if self.screen == Screen::Listening => self.listening.page_rows,
            Focus::Discover => self.page_rows,
            Focus::Album => self.album_pane.page_rows,
        }
    }

    fn move_by(&mut self, delta: isize) -> Vec<Effect> {
        if self.screen == Screen::Listening {
            // Only the info text moves here; rendering clamps it to the text height.
            if self.listening.show_info {
                self.listening.info_scroll = scrolled(self.listening.info_scroll, delta);
            }
            return vec![];
        }
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
                self.album_pane.info_scroll = scrolled(self.album_pane.info_scroll, delta);
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

    /// The cover box of the screen showing, as last drawn: the album pane's or the listening
    /// view's. Only one is on screen at a time, so only that one is fetched and encoded for.
    fn art_box(&self) -> Option<ArtBox> {
        match self.screen {
            Screen::Browse => self.album_pane.art_box,
            Screen::Listening => self.listening.art_box,
        }
    }

    /// The image size needed for the cover box as last drawn.
    fn wanted_art_size(&self) -> Option<ImageSize> {
        let art_box = self.art_box()?;
        Some(crate::art::source_size(art_box, self.art_font))
    }

    /// Whether cover `id` still has to be downloaded (or re-downloaded bigger).
    fn needs_art(&self, id: ArtId) -> bool {
        let Some(wanted) = self.wanted_art_size().filter(|_| self.art_enabled) else {
            return false;
        };
        !self.art_requests.contains(&id)
            && !self.art_failed.contains(&id)
            && self.art_sources.peek(id).is_none_or(|s| s.size < wanted)
    }

    fn fetch_art(&mut self, id: Option<ArtId>) -> Option<Effect> {
        let id = id.filter(|&id| self.needs_art(id))?;
        let size = self.wanted_art_size()?;
        self.art_requests.insert(id);
        Some(Effect::FetchArt { id, size })
    }

    /// Encodes cover `id` for the current box if its JPEG is here and that size isn't.
    fn encode_art(&mut self, id: ArtId) -> Option<Effect> {
        let art_box = self.art_box()?;
        if self.arts.peek((id, art_box)).is_some() || self.art_encodes.contains(&(id, art_box)) {
            return None;
        }
        let source = self.art_sources.get(id)?;
        self.art_encodes.insert((id, art_box));
        Some(Effect::EncodeArt {
            id,
            jpeg: source.jpeg,
            art_box,
        })
    }

    /// Cover art for the album pane or listening view: `None` when there's none to show
    /// (disabled, no room, no art, or it failed), `Some(None)` while loading. While a resize
    /// re-encodes the cover, the largest earlier encoding that still fits is shown instead of a
    /// placeholder.
    #[must_use]
    pub fn art_for(&self, id: Option<ArtId>) -> Option<Option<&Art>> {
        let id = id.filter(|_| self.art_enabled)?;
        let art_box = self.art_box()?;
        if self.art_failed.contains(&id) {
            return None;
        }
        if let Some(art) = self.arts.peek((id, art_box)) {
            return Some(Some(art));
        }
        let fallback = self
            .arts
            .iter()
            .filter(|((art_id, b), _)| {
                *art_id == id && b.rows <= art_box.rows && b.cols <= art_box.cols
            })
            .max_by_key(|((_, b), _)| b.rows)
            .map(|(_, art)| art);
        Some(fallback)
    }

    fn art_fetched(
        &mut self,
        id: ArtId,
        size: ImageSize,
        result: Result<bytes::Bytes, String>,
    ) -> Vec<Effect> {
        self.art_requests.remove(&id);
        match result {
            Ok(jpeg) => {
                // A bigger source makes every earlier (smaller-source) encoding obsolete.
                self.arts.retain(|(art_id, _)| *art_id != id);
                self.art_sources.insert(id, ArtSource { jpeg, size });
                self.encode_art(id).into_iter().collect()
            }
            Err(e) => {
                tracing::info!(%id, error = %e, "cover art unavailable");
                self.art_failed.insert(id);
                vec![]
            }
        }
    }

    fn art_encoded(
        &mut self,
        id: ArtId,
        art_box: ArtBox,
        result: Result<Art, String>,
    ) -> Vec<Effect> {
        self.art_encodes.remove(&(id, art_box));
        match result {
            Ok(art) => self.arts.insert((id, art_box), art),
            Err(e) => {
                tracing::info!(%id, error = %e, "cover art couldn't be encoded");
                self.art_failed.insert(id);
            }
        }
        vec![]
    }

    /// Called after every draw, once the layout (and so the cover box) is known. Re-encodes or
    /// re-downloads the current cover when a resize changed the size it needs. While browsing
    /// it never starts a first download: those go through the selection debounce. The listening
    /// view shows a single cover that doesn't change with scrolling, so it fetches right away.
    pub fn after_draw(&mut self) -> Vec<Effect> {
        if !self.art_enabled {
            return vec![];
        }
        if self.screen == Screen::Listening {
            let Some(id) = self.now_playing.as_ref().and_then(|a| a.art_id) else {
                return vec![];
            };
            return match self.fetch_art(Some(id)) {
                Some(fetch) => vec![fetch],
                None => self.encode_art(id).into_iter().collect(),
            };
        }
        let id = match self.album_view() {
            AlbumView::Loaded(album) => album.art_id,
            AlbumView::Loading(item) | AlbumView::Failed(item, _) => item.art_id,
            AlbumView::None => None,
        };
        let Some(id) = id else {
            return vec![];
        };
        if self.art_sources.peek(id).is_none() {
            return vec![];
        }
        let mut effects: Vec<Effect> = self.fetch_art(Some(id)).into_iter().collect();
        if effects.is_empty() {
            effects.extend(self.encode_art(id));
        }
        effects
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
        let item = self.discover.items.iter().find(|i| i.id == id)?;
        let (band_id, kind) = (item.band_id, item.kind);
        self.fetch_release(id, band_id, kind)
    }

    /// Fetches details for any release, listed or not (e.g. a queue entry).
    fn fetch_release(&mut self, id: ItemId, band_id: BandId, kind: TralbumKind) -> Option<Effect> {
        if self.albums.peek(id).is_some() || self.album_requests.contains(&id) {
            return None;
        }
        self.album_errors.remove(&id);
        self.album_requests.insert(id);
        Some(Effect::FetchAlbum { band_id, id, kind })
    }

    fn album_loaded(&mut self, id: ItemId, result: Result<Box<Album>, String>) -> Vec<Effect> {
        self.album_requests.remove(&id);
        if let Some((_, track)) = self.queue_attempt.filter(|(q, _)| *q == id) {
            self.queue_attempt = None;
            return match result {
                Ok(album) => {
                    let album: Arc<Album> = Arc::from(album);
                    self.albums.insert(id, Arc::clone(&album));
                    if self.album_pane.for_album == Some(id) {
                        self.album_pane.for_album = None;
                    }
                    self.sync_album_pane();
                    self.play_queue_entry(&album, track)
                }
                Err(e) => {
                    self.album_errors.insert(id, e.clone());
                    self.error(format!("Skipped a queued release: {e}"));
                    self.next_from_queue(Some(id))
                }
            };
        }
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
        // Same album: jump within the existing queue instead of rebuilding it (unless only one
        // of its tracks is loaded, from a single-track queue entry).
        if self.now_playing_only.is_none()
            && self.now_playing.as_ref().is_some_and(|a| a.id == album.id)
        {
            return vec![Effect::Player(PlayerCommand::PlayIndex(start))];
        }
        let queue = album.tracks.iter().map(QueueTrack::from).collect();
        self.now_playing = Some(Arc::clone(album));
        self.now_playing_only = None;
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
                self.now_playing_only = None;
                return self.next_from_queue(finished);
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
        self.prefetch_queue_front().into_iter().collect()
    }

    // ---- Queue ---------------------------------------------------------------------------------

    /// Focus, the info view and the help overlay: view state only, never any effects.
    fn view_update(&mut self, action: &Action) {
        match action {
            Action::ToggleFocus => {
                self.focus = match self.focus {
                    Focus::Discover => Focus::Album,
                    Focus::Album => Focus::Discover,
                };
            }
            Action::FocusDiscover => self.focus = Focus::Discover,
            Action::FocusAlbum => self.focus = Focus::Album,
            Action::ToggleInfo if self.screen == Screen::Listening => {
                self.listening.show_info = !self.listening.show_info;
                self.listening.info_scroll = 0;
            }
            Action::ToggleInfo => {
                self.album_pane.show_info = !self.album_pane.show_info;
                self.album_pane.info_scroll = 0;
            }
            Action::CloseHelp => self.show_help = false,
            _ => {}
        }
    }

    fn open_popup(&mut self, popup: Popup) -> Vec<Effect> {
        match popup {
            Popup::Queue => self.queue_view = Some(ListState::default().with_selected(Some(0))),
            Popup::Search => self.search_open = true,
            Popup::Help => self.show_help = true,
        }
        vec![]
    }

    /// The next queue entry and how many more follow it, for the now-playing bar.
    #[must_use]
    pub fn up_next(&self) -> Option<(&QueueEntry, usize)> {
        let front = self.queue.front()?;
        Some((front, self.queue.len() - 1))
    }

    /// What plays after the current track, for the listening view: the rest of the album,
    /// then the queue, then autoplay's next release.
    #[must_use]
    pub fn plays_next(&self) -> Option<UpNext<'_>> {
        if let Some((album, index)) = self.current_track()
            && self.now_playing_only.is_none()
            && let Some(track) = album.tracks[index + 1..].iter().find(|t| t.is_playable())
        {
            return Some(UpNext::Track(track));
        }
        if let Some((entry, more)) = self.up_next() {
            return Some(UpNext::Queue(entry, more));
        }
        self.autoplay.then_some(UpNext::Autoplay)
    }

    /// `e` / `E`: queue the selected track (in the tracklist) or release.
    fn enqueue(&mut self, next: bool) -> Vec<Effect> {
        let entry = match (self.focus, self.album_view()) {
            (Focus::Album, AlbumView::Loaded(album)) if !self.album_pane.show_info => {
                let Some(index) = self.album_pane.tracks.selected() else {
                    return vec![];
                };
                if !album
                    .tracks
                    .get(index)
                    .is_some_and(crate::bandcamp::Track::is_playable)
                {
                    self.error("That track isn't available for streaming.");
                    return vec![];
                }
                QueueEntry::track(album, index)
            }
            _ => self.selected().map(QueueEntry::release),
        };
        let Some(entry) = entry else {
            return vec![];
        };
        if !self.player_available {
            self.error("No audio output available.");
            return vec![];
        }
        // Nothing playing: there's no "after" to wait for, so start it now.
        if self.now_playing.is_none() && self.pending_play.is_none() && self.queue_attempt.is_none()
        {
            let label = entry.label();
            let effects = self.start_queue_entry(&entry);
            self.info(format!("Playing {label}"));
            return effects;
        }
        let label = entry.label();
        let added = if next {
            self.queue.push_front(entry)
        } else {
            self.queue.push_back(entry)
        };
        if !added {
            self.error(format!(
                "The queue is full ({} entries).",
                crate::queue::MAX_ENTRIES
            ));
            return vec![];
        }
        self.info(if next {
            format!("Playing next: {label}")
        } else {
            format!("Added to queue: {label} ({} queued)", self.queue.len())
        });
        vec![self.save_settings()]
    }

    /// Handles `action` while the queue popup is open; `None` means "handle normally".
    fn queue_update(&mut self, action: &Action) -> Option<Vec<Effect>> {
        let view = self.queue_view.as_mut()?;
        let len = self.queue.len();
        let selected = view.selected().unwrap_or(0).min(len.saturating_sub(1));
        let mut changed = false;
        match action {
            Action::Up => move_list(view, len, -1),
            Action::Down => move_list(view, len, 1),
            Action::PageUp => move_list(view, len, -10),
            Action::PageDown => move_list(view, len, 10),
            Action::Top => move_list(view, len, isize::MIN),
            Action::Bottom => move_list(view, len, isize::MAX),
            Action::ClosePopup | Action::OpenQueue => self.queue_view = None,
            Action::QueueRemove => {
                changed = self.queue.remove(selected).is_some();
                view.select(Some(selected.min(self.queue.len().saturating_sub(1))));
            }
            Action::QueueMoveUp => {
                if let Some(i) = self.queue.move_up(selected) {
                    view.select(Some(i));
                    changed = true;
                }
            }
            Action::QueueMoveDown => {
                if let Some(i) = self.queue.move_down(selected) {
                    view.select(Some(i));
                    changed = true;
                }
            }
            Action::QueueClear => {
                changed = !self.queue.is_empty();
                self.queue.clear();
            }
            Action::Play => {
                // Play the highlighted entry now; the rest of the queue follows it.
                self.queue_view = None;
                let Some(entry) = self.queue.remove(selected) else {
                    return Some(vec![]);
                };
                let mut effects = self.start_queue_entry(&entry);
                effects.push(self.save_settings());
                return Some(effects);
            }
            // Quitting, and transport keys, work as usual with the popup open.
            Action::Quit | Action::TogglePause | Action::NextTrack | Action::PreviousTrack => {
                return None;
            }
            _ if action.is_user_input() => {}
            _ => return None,
        }
        Some(if changed {
            vec![self.save_settings()]
        } else {
            vec![]
        })
    }

    /// After a release ends: the next queue entry, else autoplay (if on).
    fn next_from_queue(&mut self, finished: Option<ItemId>) -> Vec<Effect> {
        if let Some(entry) = self.queue.pop_front() {
            let mut effects = self.start_queue_entry(&entry);
            effects.push(self.save_settings());
            return effects;
        }
        if self.autoplay {
            self.autoplay_skips = 0;
            return self.autoplay_next(finished);
        }
        vec![]
    }

    fn start_queue_entry(&mut self, entry: &QueueEntry) -> Vec<Effect> {
        self.cancel_autoplay_attempt();
        self.pending_play = None;
        if let Some(album) = self.albums.get(entry.id) {
            return self.play_queue_entry(&album, entry.track);
        }
        self.queue_attempt = Some((entry.id, entry.track));
        self.info(format!("Up next: {}", entry.label()));
        self.fetch_release(entry.id, entry.band_id, entry.kind)
            .into_iter()
            .collect()
    }

    /// Plays a queue entry whose details are loaded: the whole album, or just `only`. With
    /// `listen_on_queue`, also switches to the listening view.
    fn play_queue_entry(&mut self, album: &Arc<Album>, only: Option<usize>) -> Vec<Effect> {
        let effects = self.play_queue_entry_now(album, only);
        let started = effects
            .iter()
            .any(|e| matches!(e, Effect::Player(PlayerCommand::PlayQueue(..))));
        if self.listen_on_queue && started {
            self.screen = Screen::Listening;
        }
        effects
    }

    fn play_queue_entry_now(&mut self, album: &Arc<Album>, only: Option<usize>) -> Vec<Effect> {
        let Some(index) = only else {
            if !album.has_playable_tracks() {
                self.error(format!("Skipped {}: nothing streamable.", album.title));
                return self.next_from_queue(Some(album.id));
            }
            // Always rebuild: the same album may be loaded in single-track mode.
            self.now_playing = None;
            return self.play_album(album, 0);
        };
        if !album
            .tracks
            .get(index)
            .is_some_and(crate::bandcamp::Track::is_playable)
        {
            self.error(format!("Skipped a queued track from {}.", album.title));
            return self.next_from_queue(Some(album.id));
        }
        if !self.player_available {
            return vec![];
        }
        // The whole album with only this track streamable: the player skips the rest, and
        // track numbering in the display stays right.
        let queue = album
            .tracks
            .iter()
            .enumerate()
            .map(|(i, t)| {
                let mut q = QueueTrack::from(t);
                if i != index {
                    q.url = None;
                }
                q
            })
            .collect();
        self.now_playing = Some(Arc::clone(album));
        self.now_playing_only = Some(index);
        vec![Effect::Player(PlayerCommand::PlayQueue(queue, index))]
    }

    /// Near the end of the current release, fetch the next entry's details so the switch
    /// only waits for the stream.
    fn prefetch_queue_front(&mut self) -> Option<Effect> {
        let next = self.queue.front()?;
        let (album, index) = self.current_track()?;
        let last_track = self.now_playing_only.is_some()
            || !album.tracks[index + 1..]
                .iter()
                .any(crate::bandcamp::Track::is_playable);
        let remaining = self.player.duration.saturating_sub(self.player.position);
        if !last_track || self.player.duration.is_zero() || remaining > QUEUE_PREFETCH {
            return None;
        }
        let (id, band_id, kind) = (next.id, next.band_id, next.kind);
        self.fetch_release(id, band_id, kind)
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
        if self.screen == Screen::Listening {
            let Some(album) = &self.now_playing else {
                return vec![];
            };
            let url = album.url.clone();
            self.info(format!("Opening {url}"));
            return vec![Effect::OpenUrl(url)];
        }
        let Some(item) = self.selected() else {
            return vec![];
        };
        // Prefer the release's own page: artist discographies only give the artist's URL.
        let url = self
            .albums
            .peek(item.id)
            .map_or_else(|| item.url.clone(), |album| album.url.clone());
        self.info(format!("Opening {url}"));
        vec![Effect::OpenUrl(url)]
    }

    // ---- Search and artists --------------------------------------------------------------------

    fn next_list_generation(&mut self) -> u64 {
        self.list_generation += 1;
        self.list_generation
    }

    /// Handles `action` while the search box is open; `None` means "handle normally".
    fn search_update(&mut self, action: &Action) -> Option<Vec<Effect>> {
        let len = self.search.results.len();
        let mut edited = false;
        match action {
            Action::Up => move_list(&mut self.search.list, len, -1),
            Action::Down => move_list(&mut self.search.list, len, 1),
            Action::PageUp => move_list(&mut self.search.list, len, -10),
            Action::PageDown => move_list(&mut self.search.list, len, 10),
            Action::TypeChar(c) => {
                self.search.query.push(*c);
                edited = true;
            }
            Action::Backspace => {
                edited = self.search.query.pop().is_some();
            }
            Action::ClearInput => {
                edited = !self.search.query.is_empty();
                self.search.query.clear();
            }
            Action::ClosePopup => self.search_open = false,
            Action::Play => return Some(self.open_search_result()),
            Action::Quit => return None,
            _ if action.is_user_input() => {}
            _ => return None,
        }
        if !edited {
            return Some(vec![]);
        }
        self.search.generation += 1;
        self.search.error = None;
        if self.search.query.trim().is_empty() {
            self.search.results.clear();
            self.search.loading = false;
            return Some(vec![]);
        }
        self.search.loading = true;
        Some(vec![Effect::DebounceSearch(self.search.generation)])
    }

    fn search_due(&mut self, generation: u64) -> Vec<Effect> {
        let query = self.search.query.trim();
        if generation != self.search.generation || query.is_empty() {
            return vec![];
        }
        vec![Effect::FetchSearch {
            query: query.to_owned(),
            generation,
        }]
    }

    fn search_loaded(
        &mut self,
        generation: u64,
        result: Result<Vec<SearchResult>, String>,
    ) -> Vec<Effect> {
        if generation != self.search.generation {
            return vec![]; // The query changed since.
        }
        self.search.loading = false;
        match result {
            Ok(results) => {
                self.search.results = results;
                self.search.error = None;
            }
            Err(e) => {
                self.search.results.clear();
                self.search.error = Some(e);
            }
        }
        self.search.list = ListState::default().with_selected(Some(0));
        vec![]
    }

    /// `⏎` in the search box: a release opens all release results in the list; an artist
    /// opens their discography.
    fn open_search_result(&mut self) -> Vec<Effect> {
        let Some(chosen) = self.search.selected().cloned() else {
            return vec![];
        };
        self.search_open = false;
        match chosen {
            SearchResult::Release(item) => {
                let releases: Vec<DiscoverItem> = self
                    .search
                    .results
                    .iter()
                    .filter_map(|r| match r {
                        SearchResult::Release(item) => Some(item.clone()),
                        SearchResult::Artist(_) => None,
                    })
                    .collect();
                let index = releases.iter().position(|r| r.id == item.id);
                let query = self.search.query.trim().to_owned();
                self.show_list(ListSource::Search(query), releases, index, false)
            }
            SearchResult::Artist(artist) => {
                self.pending_artist = Some(artist.band_id);
                let mut effects =
                    self.show_list(ListSource::Artist(artist.name), Vec::new(), None, true);
                effects.push(Effect::FetchArtist(artist.band_id));
                effects
            }
        }
    }

    fn artist_loaded(
        &mut self,
        band_id: BandId,
        result: Result<Box<Artist>, String>,
    ) -> Vec<Effect> {
        if self.pending_artist != Some(band_id) {
            return vec![]; // The user moved on.
        }
        self.pending_artist = None;
        match result {
            Ok(artist) => {
                let artist = *artist;
                self.show_list(
                    ListSource::Artist(artist.name),
                    artist.releases,
                    Some(0),
                    false,
                )
            }
            Err(e) => {
                self.discover.loading = false;
                self.error(format!("Couldn't load artist: {e}"));
                vec![]
            }
        }
    }

    /// Replaces the release list, keeping Discover aside to come back to.
    fn show_list(
        &mut self,
        source: ListSource,
        items: Vec<DiscoverItem>,
        selected: Option<usize>,
        loading: bool,
    ) -> Vec<Effect> {
        if self.source == ListSource::Discover {
            let discover = std::mem::take(&mut self.discover);
            let table = std::mem::take(&mut self.table);
            self.saved_discover = Some((discover, table));
        }
        self.source = source;
        self.autoplay_after = None;
        let generation = self.next_list_generation();
        self.discover = DiscoverList {
            total: items.len() as u64,
            items,
            next_cursor: None,
            loading,
            generation,
        };
        let selected = selected.or((!self.discover.items.is_empty()).then_some(0));
        self.table = TableState::default().with_selected(selected);
        self.focus = Focus::Discover;
        self.selection_changed().into_iter().collect()
    }

    /// `Esc`: back to Discover as it was, without reloading.
    fn back_to_discover(&mut self) -> Vec<Effect> {
        if self.source == ListSource::Discover {
            return vec![];
        }
        self.source = ListSource::Discover;
        self.pending_artist = None;
        self.autoplay_after = None;
        if let Some((mut discover, table)) = self.saved_discover.take() {
            // A page that was loading may have been dropped while we were away.
            discover.loading = false;
            self.discover = discover;
            self.table = table;
            let mut effects: Vec<Effect> = self.selection_changed().into_iter().collect();
            effects.extend(self.maybe_load_more());
            effects
        } else {
            vec![self.reload()]
        }
    }

    // ---- Autoplay ------------------------------------------------------------------------------

    /// The user chose something to play: forget what autoplay or the queue was about to start.
    fn cancel_autoplay_attempt(&mut self) {
        self.autoplay_attempt = None;
        self.autoplay_after = None;
        self.queue_attempt = None;
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
            self.info("Autoplay: that was the last release in this list.");
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
        // Resume the same way: a single queued track stays a single track.
        if self.now_playing_only.take().is_some() {
            self.queue_attempt = Some((album.id, Some(index)));
        } else {
            self.pending_play = Some((album.id, index));
        }
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
                | Self::ArtFetched { .. }
                | Self::ArtEncoded { .. }
                | Self::SearchDue(_)
                | Self::Notice(_)
                | Self::SearchLoaded { .. }
                | Self::ArtistLoaded { .. }
                | Self::Player { .. }
        )
    }
}

/// Moves a list selection by `delta`, clamped to `len` rows.
fn move_list(list: &mut ListState, len: usize, delta: isize) {
    if len > 0 {
        let current = list.selected().unwrap_or(0);
        list.select(Some(current.saturating_add_signed(delta).min(len - 1)));
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

/// A text scroll offset moved by `delta` lines, never below the top.
fn scrolled(scroll: u16, delta: isize) -> u16 {
    let scroll = i64::from(scroll) + i64::try_from(delta.clamp(-1000, 1000)).unwrap_or(0);
    u16::try_from(scroll.max(0)).unwrap_or(u16::MAX)
}

fn page(rows: usize) -> isize {
    isize::try_from(rows.saturating_sub(1).max(1)).unwrap_or(isize::MAX)
}

/// A downloaded cover, kept compressed.
#[derive(Debug, Clone)]
struct ArtSource {
    jpeg: bytes::Bytes,
    size: ImageSize,
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

    fn iter(&self) -> impl Iterator<Item = (&K, &V)> {
        self.map.iter()
    }

    fn retain(&mut self, keep: impl Fn(&K) -> bool) {
        self.map.retain(|k, _| keep(k));
        self.order.retain(|k| keep(k));
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
            app.update(Action::TypeChar(c));
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
        app.update(Action::TypeChar('b'));
        assert_eq!(app.update(Action::ClosePopup), []);
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

    const SMALL_BOX: ArtBox = ArtBox { cols: 12, rows: 6 };
    const BIG_BOX: ArtBox = ArtBox { cols: 24, rows: 12 };

    /// An app with art on and the cover box as the UI would record it.
    fn with_art(art_box: ArtBox) -> App {
        let mut app = loaded();
        app.art_enabled = true;
        app.album_pane.art_box = Some(art_box);
        app
    }

    fn encoded(art_box: ArtBox) -> Art {
        crate::art::encode(
            &ratatui_image::picker::Picker::halfblocks(),
            &crate::art::tests::test_jpeg(),
            art_box,
        )
        .unwrap()
    }

    /// Downloads and encodes the selected release's cover at `art_box`.
    fn load_cover(app: &mut App, art_box: ArtBox) -> ArtId {
        let item = app.selected().unwrap().clone();
        let art_id = item.art_id.unwrap();
        let effects = app.update(Action::AlbumDue(item.id));
        let Some(&Effect::FetchArt { id, size }) = effects
            .iter()
            .find(|e| matches!(e, Effect::FetchArt { .. }))
        else {
            panic!("{effects:?}");
        };
        assert_eq!(id, art_id);
        let effects = app.update(Action::ArtFetched {
            id,
            size,
            result: Ok(crate::art::tests::test_jpeg().into()),
        });
        assert!(
            matches!(&effects[..], [Effect::EncodeArt { art_box: b, .. }] if *b == art_box),
            "{effects:?}"
        );
        app.update(Action::ArtEncoded {
            id,
            art_box,
            result: Ok(encoded(art_box)),
        });
        art_id
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

        let mut app = with_art(SMALL_BOX);
        let art_id = app.selected().unwrap().art_id.unwrap();
        assert_eq!(app.art_for(Some(art_id)), Some(None), "loading");
        load_cover(&mut app, SMALL_BOX);
        assert!(matches!(app.art_for(Some(art_id)), Some(Some(a)) if a.art_box == SMALL_BOX));

        // Leaving and coming back: details and art are cached, nothing to fetch.
        let item = app.selected().unwrap().id;
        app.update(Action::AlbumLoaded {
            id: item,
            result: Ok(album_fixture("album")),
        });
        app.update(Action::Down);
        assert!(matches!(app.update(Action::Up)[..], []));
        assert_eq!(app.after_draw(), []);
    }

    #[test]
    fn the_download_size_follows_the_box() {
        let mut app = with_art(SMALL_BOX);
        let item = app.selected().unwrap().id;
        let effects = app.update(Action::AlbumDue(item));
        assert!(effects.contains(&Effect::FetchArt {
            id: app.selected().unwrap().art_id.unwrap(),
            size: ImageSize::Px150,
        }));

        let mut app = with_art(BIG_BOX);
        let item = app.selected().unwrap().id;
        let effects = app.update(Action::AlbumDue(item));
        assert!(
            effects.iter().any(|e| matches!(
                e,
                Effect::FetchArt {
                    size: ImageSize::Px350,
                    ..
                }
            )),
            "{effects:?}"
        );
    }

    #[test]
    fn a_resize_re_encodes_and_shows_the_old_size_meanwhile() {
        let mut app = with_art(BIG_BOX);
        let art_id = load_cover(&mut app, BIG_BOX);

        // Shrinking: the source is big enough, so just re-encode.
        app.album_pane.art_box = Some(SMALL_BOX);
        let effects = app.after_draw();
        assert!(
            matches!(&effects[..], [Effect::EncodeArt { art_box, .. }] if *art_box == SMALL_BOX),
            "{effects:?}"
        );
        // Nothing smaller is encoded yet: the big one doesn't fit, so a placeholder.
        assert_eq!(app.art_for(Some(art_id)), Some(None));
        assert_eq!(app.after_draw(), [], "already encoding");
        app.update(Action::ArtEncoded {
            id: art_id,
            art_box: SMALL_BOX,
            result: Ok(encoded(SMALL_BOX)),
        });

        // Growing back: the big encoding is still cached, so no work at all.
        app.album_pane.art_box = Some(BIG_BOX);
        assert_eq!(app.after_draw(), []);
        assert!(matches!(app.art_for(Some(art_id)), Some(Some(a)) if a.art_box == BIG_BOX));
    }

    #[test]
    fn growing_past_the_source_downloads_a_bigger_one() {
        let mut app = with_art(SMALL_BOX);
        let art_id = load_cover(&mut app, SMALL_BOX);
        app.album_pane.art_box = Some(BIG_BOX);
        let effects = app.after_draw();
        assert_eq!(
            effects,
            [Effect::FetchArt {
                id: art_id,
                size: ImageSize::Px350
            }]
        );
        // While it downloads, the small cover stands in.
        assert!(matches!(app.art_for(Some(art_id)), Some(Some(a)) if a.art_box == SMALL_BOX));
    }

    #[test]
    fn no_room_means_no_cover_and_no_download() {
        let mut app = loaded();
        app.art_enabled = true;
        app.album_pane.art_box = None;
        let item = app.selected().unwrap().clone();
        assert!(matches!(
            app.update(Action::AlbumDue(item.id))[..],
            [Effect::FetchAlbum { .. }]
        ));
        assert_eq!(app.art_for(item.art_id), None);
    }

    #[test]
    fn failed_art_falls_back_to_text_and_isnt_retried() {
        let mut app = with_art(SMALL_BOX);
        let item = app.selected().unwrap().clone();
        let art_id = item.art_id.unwrap();
        app.update(Action::AlbumDue(item.id));
        app.update(Action::ArtFetched {
            id: art_id,
            size: ImageSize::Px150,
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

    fn search_fixture() -> Vec<SearchResult> {
        let json = include_str!("../tests/fixtures/search_aphex.json");
        crate::bandcamp::parse_search(json).unwrap()
    }

    fn artist_fixture() -> Box<Artist> {
        let json = include_str!("../tests/fixtures/band_aphex.json");
        Box::new(crate::bandcamp::parse_artist(json).unwrap())
    }

    /// Opens search, types `text`, and lets the results arrive.
    fn search_for(app: &mut App, text: &str) {
        app.update(Action::OpenSearch);
        let mut generation = 0;
        for c in text.chars() {
            let effects = app.update(Action::TypeChar(c));
            let [Effect::DebounceSearch(g)] = effects[..] else {
                panic!("{effects:?}");
            };
            generation = g;
        }
        let effects = app.update(Action::SearchDue(generation));
        assert!(
            matches!(&effects[..], [Effect::FetchSearch { query, .. }] if query == text),
            "{effects:?}"
        );
        app.update(Action::SearchLoaded {
            generation,
            result: Ok(search_fixture()),
        });
    }

    #[test]
    fn search_is_debounced_and_ignores_stale_results() {
        let mut app = loaded();
        app.update(Action::OpenSearch);
        let [Effect::DebounceSearch(first)] = app.update(Action::TypeChar('a'))[..] else {
            panic!();
        };
        let [Effect::DebounceSearch(second)] = app.update(Action::TypeChar('p'))[..] else {
            panic!();
        };
        assert_eq!(
            app.update(Action::SearchDue(first)),
            [],
            "superseded by more typing"
        );
        assert!(app.search.loading);
        // A late answer to the old query is dropped.
        app.update(Action::SearchLoaded {
            generation: first,
            result: Ok(search_fixture()),
        });
        assert_eq!(app.search.results, []);
        assert!(matches!(
            app.update(Action::SearchDue(second))[..],
            [Effect::FetchSearch { .. }]
        ));
        // Keys go to the box: `j` is text, not "move down".
        app.update(Action::TypeChar('j'));
        assert_eq!(app.search.query, "apj");
        assert_eq!(app.table.selected(), Some(0));
    }

    #[test]
    fn opening_a_release_shows_the_results_and_esc_restores_discover() {
        let mut app = loaded();
        app.update(Action::Down);
        let discover_items = app.discover.items.len();
        search_for(&mut app, "aphex twin");
        assert_eq!(app.search.results.len(), 7);
        // Results: artist, 4 albums, 2 tracks. Pick the second album (Syro).
        app.update(Action::Down);
        app.update(Action::Down);
        let effects = app.update(Action::Play);
        assert!(!app.search_open);
        assert_eq!(app.source, ListSource::Search("aphex twin".into()));
        assert_eq!(app.discover.items.len(), 6, "releases only, no artists");
        assert_eq!(app.selected().unwrap().title, "Syro");
        assert!(
            matches!(&effects[..], [Effect::DebounceAlbum(id)] if *id == app.selected().unwrap().id),
            "{effects:?}"
        );
        assert!(!app.discover.has_more(), "search has no more pages");

        let effects = app.update(Action::BackToDiscover);
        assert_eq!(app.source, ListSource::Discover);
        assert_eq!(app.discover.items.len(), discover_items);
        assert_eq!(app.table.selected(), Some(1), "selection restored");
        assert!(
            effects
                .iter()
                .all(|e| !matches!(e, Effect::FetchDiscover { cursor: None, .. }))
        );
    }

    #[test]
    fn opening_an_artist_lists_their_releases() {
        let mut app = loaded();
        search_for(&mut app, "aphex twin");
        let effects = app.update(Action::Play); // the artist is first
        let band_id = BandId(1_533_096_991);
        assert_eq!(effects.last(), Some(&Effect::FetchArtist(band_id)));
        assert_eq!(app.source, ListSource::Artist("Aphex Twin".into()));
        assert!(app.discover.loading);

        let effects = app.update(Action::ArtistLoaded {
            band_id,
            result: Ok(artist_fixture()),
        });
        assert_eq!(app.discover.items.len(), 5);
        assert!(!app.discover.loading);
        assert_eq!(app.selected().unwrap().title, "Drukqs");
        assert!(
            matches!(&effects[..], [Effect::DebounceAlbum(_)]),
            "{effects:?}"
        );
    }

    #[test]
    fn a_late_artist_page_is_ignored_after_leaving() {
        let mut app = loaded();
        search_for(&mut app, "aphex twin");
        app.update(Action::Play);
        app.update(Action::BackToDiscover);
        let effects = app.update(Action::ArtistLoaded {
            band_id: BandId(1_533_096_991),
            result: Ok(artist_fixture()),
        });
        assert_eq!(effects, []);
        assert_eq!(app.source, ListSource::Discover);
    }

    #[test]
    fn changing_filters_leaves_search() {
        let mut app = loaded();
        search_for(&mut app, "aphex twin");
        app.update(Action::Down);
        app.update(Action::Play);
        assert_ne!(app.source, ListSource::Discover);
        let effects = app.update(Action::CycleSlice);
        assert_eq!(app.source, ListSource::Discover);
        assert!(matches!(
            &effects[0],
            Effect::FetchDiscover { cursor: None, .. }
        ));
        // Nothing saved to go back to any more.
        assert_eq!(app.update(Action::BackToDiscover), []);
    }

    #[test]
    fn reopening_search_keeps_the_last_query_and_esc_closes_it() {
        let mut app = loaded();
        search_for(&mut app, "aphex twin");
        app.update(Action::ClosePopup);
        assert!(!app.search_open);
        assert_eq!(
            app.source,
            ListSource::Discover,
            "closing doesn't change the list"
        );
        app.update(Action::OpenSearch);
        assert_eq!(app.search.query, "aphex twin");
        assert_eq!(app.search.results.len(), 7);
        app.update(Action::ClearInput);
        assert_eq!(app.search.results, []);
    }

    #[test]
    fn open_in_browser_prefers_the_release_page_over_the_artist_page() {
        let mut app = loaded();
        search_for(&mut app, "aphex twin");
        app.update(Action::Play);
        app.update(Action::ArtistLoaded {
            band_id: BandId(1_533_096_991),
            result: Ok(artist_fixture()),
        });
        let item = app.selected().unwrap().clone();
        // Discography entries only know the artist page...
        assert!(
            matches!(&app.update(Action::OpenInBrowser)[..], [Effect::OpenUrl(u)] if *u == item.url)
        );
        // ...until the release's details arrive with its own URL.
        app.update(Action::AlbumDue(item.id));
        let album = album_fixture("album");
        let album_url = album.url.clone();
        app.update(Action::AlbumLoaded {
            id: item.id,
            result: Ok(album),
        });
        assert!(
            matches!(&app.update(Action::OpenInBrowser)[..], [Effect::OpenUrl(u)] if *u == album_url)
        );
    }

    #[test]
    fn list_rows_fall_back_to_album_details_for_size() {
        let mut app = loaded();
        // Discover sometimes sends no track count or duration (e.g. "Declan James - Cycle 2").
        app.discover.items[0].track_count = None;
        app.discover.items[0].duration = None;
        let item = app.discover.items[0].clone();
        assert_eq!(app.release_size(&item), (None, None));

        app.update(Action::AlbumDue(item.id));
        app.update(Action::AlbumLoaded {
            id: item.id,
            result: Ok(album_fixture("album")),
        });
        let (tracks, duration) = app.release_size(&item);
        assert_eq!(tracks, Some(10));
        assert_eq!(duration.map(|d| d.as_secs()), Some(2279));

        // Discover's own numbers win when present.
        let other = app.discover.items[1].clone();
        assert_eq!(
            app.release_size(&other),
            (other.track_count, other.duration)
        );
    }

    /// Starts playing the first release (Ocean Company) so the queue has something to wait for.
    fn playing_first() -> App {
        let mut app = with_album();
        app.update(Action::Play);
        assert!(app.now_playing.is_some());
        app
    }

    fn queue_ended(app: &mut App) -> Vec<Effect> {
        app.update(Action::Player {
            event: Some(PlayerEvent::QueueEnded),
            status: Status::default(),
        })
    }

    fn queue_ids(app: &App) -> Vec<ItemId> {
        app.queue.iter().map(|e| e.id).collect()
    }

    #[test]
    fn enqueue_adds_without_interrupting_and_saves() {
        let mut app = playing_first();
        app.update(Action::Down);
        let second = app.selected().unwrap().id;
        let effects = app.update(Action::Enqueue);
        assert!(
            matches!(&effects[..], [Effect::SaveSettings(s)] if s.queue.len() == 1),
            "{effects:?}"
        );
        assert_eq!(queue_ids(&app), [second]);
        assert_eq!(
            app.now_playing.as_ref().unwrap().title,
            "Ocean Company",
            "still playing"
        );
        assert!(
            app.message
                .as_ref()
                .unwrap()
                .text
                .starts_with("Added to queue")
        );

        // E puts it first.
        app.update(Action::Down);
        let third = app.selected().unwrap().id;
        app.update(Action::EnqueueNext);
        assert_eq!(queue_ids(&app), [third, second]);
        assert_eq!(
            app.up_next().map(|(e, more)| (e.id, more)),
            Some((third, 1))
        );
    }

    #[test]
    fn enqueue_with_nothing_playing_starts_it() {
        let mut app = with_album();
        let effects = app.update(Action::Enqueue);
        assert!(
            matches!(&effects[..], [Effect::Player(PlayerCommand::PlayQueue(q, 0))] if q.len() == 10),
            "{effects:?}"
        );
        assert!(app.queue.is_empty());
    }

    #[test]
    fn the_queue_plays_before_autoplay() {
        let mut app = playing_first();
        // Queue the third release; autoplay alone would pick the second.
        app.update(Action::Down);
        app.update(Action::Down);
        let third = app.selected().unwrap().id;
        app.update(Action::Enqueue);
        let effects = queue_ended(&mut app);
        assert_eq!(fetched_album(&effects), Some(third), "{effects:?}");
        assert!(
            effects
                .iter()
                .any(|e| matches!(e, Effect::SaveSettings(s) if s.queue.is_empty()))
        );
        assert!(app.queue.is_empty());

        let mut album = album_fixture("album");
        album.id = third;
        let effects = app.update(Action::AlbumLoaded {
            id: third,
            result: Ok(album),
        });
        assert!(
            matches!(
                &effects[..],
                [Effect::Player(PlayerCommand::PlayQueue(_, 0))]
            ),
            "{effects:?}"
        );
        // Queue empty now: the next end falls through to autoplay, which continues after the
        // queued release (so the fourth), not after the one played before it.
        let fourth = app.discover.items[3].id;
        let effects = queue_ended(&mut app);
        assert_eq!(fetched_album(&effects), Some(fourth), "{effects:?}");
    }

    #[test]
    fn a_queued_track_plays_alone() {
        let mut app = playing_first();
        // Queue track 3 from the tracklist (the same album, already cached).
        app.update(Action::FocusAlbum);
        app.update(Action::Down);
        app.update(Action::Down);
        app.update(Action::Enqueue);
        let entry = app.queue.front().unwrap().clone();
        assert_eq!(
            (entry.track, entry.track_title.as_deref()),
            (Some(2), Some("Passage"))
        );

        let effects = queue_ended(&mut app);
        let [
            Effect::Player(PlayerCommand::PlayQueue(queue, 2)),
            Effect::SaveSettings(_),
        ] = &effects[..]
        else {
            panic!("{effects:?}");
        };
        let playable: Vec<usize> = (0..queue.len())
            .filter(|&i| queue[i].url.is_some())
            .collect();
        assert_eq!(playable, [2], "only the queued track is streamable");

        // ⏎ on the album afterwards plays all of it again, not just that track.
        app.update(Action::FocusDiscover);
        app.update(Action::Top);
        let effects = app.update(Action::Play);
        assert!(
            matches!(&effects[..], [Effect::Player(PlayerCommand::PlayQueue(q, 0))]
                if q.iter().filter(|t| t.url.is_some()).count() == 10),
            "{effects:?}"
        );
    }

    #[test]
    fn unplayable_queue_entries_are_skipped() {
        let mut app = playing_first();
        let (second, third) = (app.discover.items[1].id, app.discover.items[2].id);
        app.update(Action::Down);
        app.update(Action::Enqueue);
        app.update(Action::Down);
        app.update(Action::Enqueue);
        let effects = queue_ended(&mut app);
        assert_eq!(fetched_album(&effects), Some(second));
        // The second turns out to be a pre-order: move on to the third.
        let effects = app.update(Action::AlbumLoaded {
            id: second,
            result: Ok(album_fixture("preorder")),
        });
        assert_eq!(fetched_album(&effects), Some(third), "{effects:?}");
    }

    #[test]
    fn queue_popup_edits_and_plays() {
        let mut app = playing_first();
        for _ in 0..3 {
            app.update(Action::Down);
            app.update(Action::Enqueue);
        }
        let ids = queue_ids(&app);
        app.update(Action::OpenQueue);
        // Keys go to the popup: Down moves its selection, not the list's.
        let list_selection = app.table.selected();
        app.update(Action::Down);
        assert_eq!(app.table.selected(), list_selection);
        app.update(Action::QueueMoveUp);
        assert_eq!(queue_ids(&app), [ids[1], ids[0], ids[2]]);
        app.update(Action::Bottom);
        assert!(matches!(
            app.update(Action::QueueRemove)[..],
            [Effect::SaveSettings(_)]
        ));
        assert_eq!(queue_ids(&app), [ids[1], ids[0]]);
        // ⏎ plays the highlighted entry now and closes the popup; the rest stays queued.
        app.update(Action::Top);
        let effects = app.update(Action::Play);
        assert_eq!(fetched_album(&effects), Some(ids[1]));
        assert!(app.queue_view.is_none());
        assert_eq!(queue_ids(&app), [ids[0]]);
        // Transport keys still reach the player with the popup open.
        app.update(Action::OpenQueue);
        assert_eq!(
            app.update(Action::TogglePause),
            [Effect::Player(PlayerCommand::TogglePause)]
        );
        app.update(Action::QueueClear);
        assert!(app.queue.is_empty());
    }

    #[test]
    fn queue_survives_a_restart_in_settings() {
        let mut app = playing_first();
        app.update(Action::Down);
        app.update(Action::Enqueue);
        let saved = app.settings().queue;
        assert_eq!(saved.len(), 1);
        let (mut restored, _) = App::new(DiscoverQuery::default(), true);
        restored.queue = Queue::from_saved(saved);
        assert_eq!(queue_ids(&restored), queue_ids(&app));
        assert!(
            restored.now_playing.is_none(),
            "restoring doesn't start playback"
        );
    }

    #[test]
    fn next_entry_is_prefetched_near_the_end() {
        let mut app = playing_first();
        app.update(Action::Down);
        let next = app.selected().unwrap().id;
        app.update(Action::Enqueue);
        let status = |index, position| Status {
            state: PlaybackState::Playing,
            index: Some(index),
            position: std::time::Duration::from_secs(position),
            duration: std::time::Duration::from_secs(200),
            volume: 1.0,
        };
        // Early on the last track, or late on an earlier one: nothing yet.
        let update = |app: &mut App, index, position| {
            app.update(Action::Player {
                event: None,
                status: status(index, position),
            })
        };
        assert_eq!(update(&mut app, 9, 60), []);
        assert_eq!(update(&mut app, 5, 190), []);
        // The last 30 s of the last track: fetch the next entry's details once.
        assert_eq!(fetched_album(&update(&mut app, 9, 175)), Some(next));
        assert_eq!(update(&mut app, 9, 180), [], "already in flight");
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

    fn playing_track(app: &mut App, index: usize) {
        app.update(Action::Player {
            event: None,
            status: Status {
                state: PlaybackState::Playing,
                index: Some(index),
                ..Status::default()
            },
        });
    }

    #[test]
    fn listening_view_toggles_keeps_focus_and_is_saved() {
        let mut app = with_album();
        app.update(Action::FocusAlbum);
        let effects = app.update(Action::ToggleListening);
        assert_eq!(app.screen, Screen::Listening);
        assert!(
            matches!(&effects[..], [Effect::SaveSettings(s)] if s.screen == Screen::Listening),
            "{effects:?}"
        );
        app.update(Action::ToggleListening);
        assert_eq!(app.screen, Screen::Browse);
        assert_eq!(app.focus, Focus::Album, "focus survives the round trip");
    }

    #[test]
    fn listening_view_fetches_the_playing_cover_not_the_selected_one() {
        let mut app = playing_first();
        let playing_art = app.now_playing.as_ref().unwrap().art_id.unwrap();
        app.update(Action::Down);
        assert_ne!(app.selected().unwrap().art_id, Some(playing_art));
        app.art_enabled = true;
        app.album_pane.art_box = Some(SMALL_BOX);
        app.update(Action::ToggleListening);
        app.listening.art_box = Some(BIG_BOX);

        // No debounce: the first draw fetches it, at the listening view's size.
        let effects = app.after_draw();
        let wanted = crate::art::source_size(BIG_BOX, app.art_font);
        assert!(
            matches!(&effects[..], [Effect::FetchArt { id, size }] if *id == playing_art && *size == wanted),
            "{effects:?}"
        );
        assert_eq!(app.after_draw(), [], "already in flight");

        let effects = app.update(Action::ArtFetched {
            id: playing_art,
            size: wanted,
            result: Ok(bytes::Bytes::from(crate::art::tests::test_jpeg())),
        });
        assert!(
            matches!(&effects[..], [Effect::EncodeArt { id, art_box, .. }] if *id == playing_art && *art_box == BIG_BOX),
            "{effects:?}"
        );
        app.update(Action::ArtEncoded {
            id: playing_art,
            art_box: BIG_BOX,
            result: Ok(encoded(BIG_BOX)),
        });
        assert!(matches!(app.art_for(Some(playing_art)), Some(Some(_))));
        assert_eq!(app.after_draw(), []);
    }

    #[test]
    fn plays_next_walks_the_album_then_the_queue_then_autoplay() {
        let mut app = playing_first();
        playing_track(&mut app, 0);
        let second_title = app.now_playing.as_ref().unwrap().tracks[1].title.clone();
        assert!(matches!(app.plays_next(), Some(UpNext::Track(t)) if t.title == second_title));

        let last = app.now_playing.as_ref().unwrap().tracks.len() - 1;
        playing_track(&mut app, last);
        assert_eq!(app.plays_next(), Some(UpNext::Autoplay));
        app.autoplay = false;
        assert_eq!(app.plays_next(), None);

        app.update(Action::Down);
        app.update(Action::Enqueue);
        assert!(matches!(app.plays_next(), Some(UpNext::Queue(_, 0))));
    }

    #[test]
    fn listen_on_queue_switches_when_the_queue_starts_a_release() {
        for listen in [false, true] {
            let mut app = playing_first();
            app.listen_on_queue = listen;
            app.update(Action::Enqueue);
            queue_ended(&mut app);
            let expected = if listen {
                Screen::Listening
            } else {
                Screen::Browse
            };
            assert_eq!(app.screen, expected, "listen_on_queue = {listen}");
        }
    }

    #[test]
    fn open_in_browser_in_the_listening_view_opens_what_is_playing() {
        let mut app = playing_first();
        let playing_url = app.now_playing.as_ref().unwrap().url.clone();
        app.update(Action::Down);
        app.update(Action::ToggleListening);
        assert!(
            matches!(&app.update(Action::OpenInBrowser)[..], [Effect::OpenUrl(u)] if *u == playing_url)
        );
    }

    #[test]
    fn info_in_the_listening_view_is_its_own_toggle_and_scrolls() {
        let mut app = playing_first();
        app.update(Action::ToggleListening);
        app.update(Action::ToggleInfo);
        assert!(app.listening.show_info);
        assert!(!app.album_pane.show_info, "the album pane keeps its own");
        app.update(Action::Down);
        app.update(Action::Down);
        assert_eq!(app.listening.info_scroll, 2);
        app.update(Action::Up);
        app.update(Action::Up);
        app.update(Action::Up);
        assert_eq!(app.listening.info_scroll, 0, "never above the top");
        let before = app.selected().map(|i| i.id);
        app.update(Action::Bottom);
        assert_eq!(
            app.selected().map(|i| i.id),
            before,
            "the list doesn't move"
        );
        // Back to browsing: `i` is the album pane's again.
        app.update(Action::ToggleListening);
        app.update(Action::ToggleInfo);
        assert!(app.album_pane.show_info);
        assert!(app.listening.show_info, "remembered for next time");
    }
}
