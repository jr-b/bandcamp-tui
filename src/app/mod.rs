//! Application state and the pure `update` function.
//!
//! `App::update` takes an [`Action`] (a keypress, an API result, a player event) and returns
//! the [`Effect`]s to run (API calls, player commands). It never does I/O itself, so the whole
//! app's behaviour is unit-testable; `run.rs` executes the effects.

use std::collections::{HashMap, HashSet};
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
use crate::text_input::{Edit, TextInput};

use art::ArtSource;
use lru::Lru;

mod album;
mod art;
mod autoplay;
mod discover;
mod lru;
mod playback;
mod queue;
mod search;
#[cfg(test)]
mod tests;

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
    /// Select the playing release in the list, and its playing track (`c`).
    JumpToPlaying,
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
    /// An edit to the picker's or search box's text.
    Edit(Edit),
    /// Terminal resized; just redraw.
    Resize,
    /// The info message with this serial has been shown long enough.
    ExpireMessage(u64),
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
    /// Why the last load failed, shown in place of the list while it's empty.
    pub error: Option<String>,
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
    pub query: TextInput,
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

/// State of the `?` help overlay.
#[derive(Debug, Default)]
pub struct HelpView {
    /// Lines scrolled off the top; rendering clamps it to the text height.
    pub scroll: u16,
    /// Rows of help visible, set while rendering; used for page up/down.
    pub page_rows: usize,
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
    /// Unique per message, so a timer set for one never clears a later one.
    pub serial: u64,
}

#[expect(
    clippy::struct_excessive_bools,
    reason = "independent flags (player present, art on, quitting), not a state machine"
)]
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
    /// Source of [`Message::serial`]s.
    message_serial: u64,
    pub should_quit: bool,
    /// The help overlay, if open.
    pub help: Option<HelpView>,
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
            message_serial: 0,
            should_quit: false,
            help: None,
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
        if let Some(effects) = self.popup_update(&action) {
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
            | Action::ToggleInfo => {
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
            Action::JumpToPlaying => self.jump_to_playing(),
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
            | Action::CloseHelp
            | Action::Edit(_)
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
            | Action::ExpireMessage(_)
            | Action::SearchDue(_)
            | Action::SearchLoaded { .. }
            | Action::ArtistLoaded { .. }) => self.background_update(other),
        }
    }

    /// The open popup handles `action` first; `None` means no popup took it.
    fn popup_update(&mut self, action: &Action) -> Option<Vec<Effect>> {
        if self.help.is_some() {
            return self.help_update(action);
        }
        if self.picker.is_some() {
            return self.picker_update(action);
        }
        if self.search_open {
            return self.search_update(action);
        }
        if self.queue_view.is_some() {
            return self.queue_update(action);
        }
        None
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
            Action::ExpireMessage(serial) => {
                if self.expiring_message() == Some(serial) {
                    self.message = None;
                }
                vec![]
            }
            Action::SearchDue(generation) => self.search_due(generation),
            Action::SearchLoaded { generation, result } => self.search_loaded(generation, result),
            Action::ArtistLoaded { band_id, result } => self.artist_loaded(band_id, result),
            _ => vec![],
        }
    }

    /// Handles `action` while the help overlay is open; `None` means "handle normally".
    fn help_update(&mut self, action: &Action) -> Option<Vec<Effect>> {
        let help = self.help.as_mut()?;
        let page = page(help.page_rows);
        match action {
            Action::Up => help.scroll = scrolled(help.scroll, -1),
            Action::Down => help.scroll = scrolled(help.scroll, 1),
            Action::PageUp => help.scroll = scrolled(help.scroll, -page),
            Action::PageDown => help.scroll = scrolled(help.scroll, page),
            Action::Top => help.scroll = 0,
            // Rendering clamps it to the last screenful.
            Action::Bottom => help.scroll = u16::MAX,
            Action::CloseHelp => self.help = None,
            Action::Quit => return None,
            _ if action.is_user_input() => {}
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

    /// Focus and the info view: view state only, never any effects.
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
            _ => {}
        }
    }

    fn open_popup(&mut self, popup: Popup) -> Vec<Effect> {
        match popup {
            Popup::Queue => self.queue_view = Some(ListState::default().with_selected(Some(0))),
            Popup::Search => self.search_open = true,
            Popup::Help => self.help = Some(HelpView::default()),
        }
        vec![]
    }

    fn info(&mut self, text: impl Into<String>) {
        self.show_message(MessageKind::Info, text.into());
    }

    fn error(&mut self, text: impl Into<String>) {
        let text = text.into();
        tracing::warn!(%text);
        self.show_message(MessageKind::Error, text);
    }

    fn show_message(&mut self, kind: MessageKind, text: String) {
        self.message_serial += 1;
        self.message = Some(Message {
            kind,
            text,
            serial: self.message_serial,
        });
    }

    /// The serial of the message that should disappear on its own: info messages do, errors
    /// stay until a keypress.
    #[must_use]
    pub fn expiring_message(&self) -> Option<u64> {
        self.message
            .as_ref()
            .filter(|m| m.kind == MessageKind::Info)
            .map(|m| m.serial)
    }
}

impl Action {
    fn is_user_input(&self) -> bool {
        !matches!(
            self,
            Self::Resize
                | Self::ExpireMessage(_)
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
