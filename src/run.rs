//! The event loop: turns terminal input, API results and player events into [`Action`]s, feeds
//! them to [`App::update`], executes the resulting [`Effect`]s, and redraws only when something
//! changed.

use std::time::Duration;

use crossterm::event::{Event, EventStream};
use futures_util::StreamExt;
use ratatui::DefaultTerminal;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};
use tokio::time::{Instant, MissedTickBehavior};

use crate::app::{Action, App, Effect, PlayerCommand};
use crate::bandcamp::{BandcampClient, DiscoverQuery};
use crate::format::error_chain;
use crate::input;
use crate::player::{PlayerEvent, PlayerHandle};

/// How often the progress bar updates while playing. Nothing ticks when paused or idle.
const PROGRESS_TICK: Duration = Duration::from_millis(250);
/// Wait this long after the selection stops moving before loading release details.
const ALBUM_DEBOUNCE: Duration = Duration::from_millis(250);
/// Wait this long after typing stops before searching.
const SEARCH_DEBOUNCE: Duration = Duration::from_millis(250);

/// Pending debounced actions; each kind keeps only its latest request.
#[derive(Default)]
struct Timers {
    album: Option<(Instant, crate::bandcamp::ItemId)>,
    search: Option<(Instant, u64)>,
}

pub struct Services {
    pub client: BandcampClient,
    pub player: Option<PlayerHandle>,
    pub player_events: UnboundedReceiver<PlayerEvent>,
    /// Where to remember filters and volume; `None` disables saving.
    pub settings_path: Option<std::path::PathBuf>,
    pub initial_volume: f32,
    /// Set if the terminal shows cover art (see [`crate::art::detect`]).
    pub art: Option<ratatui_image::picker::Picker>,
    /// Shown at startup when art was requested but can't work (e.g. tmux passthrough off).
    pub art_note: Option<&'static str>,
    /// Start with autoplay on (from `config.toml`).
    pub autoplay: bool,
    /// Switch to the listening view when the queue starts a release (from `config.toml`).
    pub listen_on_queue: bool,
    /// Browsing or the listening view, as last time.
    pub screen: crate::app::Screen,
    /// The saved play queue (restored without starting playback).
    pub queue: Vec<crate::queue::QueueEntry>,
}

impl std::fmt::Debug for Services {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Services")
            .field("player", &self.player.is_some())
            .finish_non_exhaustive()
    }
}

/// Which key bindings apply: the topmost popup's, else the screen's.
fn input_mode(app: &App) -> input::Mode {
    if app.show_help {
        input::Mode::Help
    } else if app.picker.is_some() || app.search_open {
        input::Mode::Picker
    } else if app.queue_view.is_some() {
        input::Mode::Queue
    } else if app.screen == crate::app::Screen::Listening {
        input::Mode::Listening
    } else {
        input::Mode::Normal
    }
}

pub async fn run(
    terminal: &mut DefaultTerminal,
    services: Services,
    query: DiscoverQuery,
) -> std::io::Result<()> {
    let Services {
        client,
        player,
        mut player_events,
        settings_path,
        initial_volume,
        art,
        art_note,
        autoplay,
        listen_on_queue,
        screen,
        queue,
    } = services;
    let (results_tx, mut results) = unbounded_channel();
    let (mut app, effects) = App::new(query, player.is_some());
    app.player.volume = initial_volume;
    app.art_enabled = art.is_some();
    if let Some(picker) = &art {
        app.art_font = picker.font_size();
    }
    app.autoplay = autoplay;
    app.listen_on_queue = listen_on_queue;
    app.screen = screen;
    app.queue = crate::queue::Queue::from_saved(queue);
    if let Some(note) = art_note {
        app.update(Action::Notice(note.to_owned()));
    }
    let executor = Executor {
        client,
        player: player.clone(),
        results: results_tx,
        settings_path,
        art: art.map(std::sync::Arc::new),
    };
    // At most one pending debounced album load: (when, which).
    let mut timers = Timers::default();
    executor.run(effects, &mut timers);

    let mut input = EventStream::new();
    let mut ticker = tokio::time::interval(PROGRESS_TICK);
    ticker.set_missed_tick_behavior(MissedTickBehavior::Skip);
    let mut dirty = true;

    loop {
        if dirty {
            terminal.draw(|frame| crate::ui::draw(frame, &mut app))?;
            dirty = false;
            // The layout is known now: a resize may need the cover at another size.
            let effects = app.after_draw();
            executor.run(effects, &mut timers);
        }
        let action = tokio::select! {
            event = input.next() => match event {
                Some(Ok(Event::Key(key))) => input::action_for(key, input_mode(&app)),
                Some(Ok(Event::Resize(..))) => Some(Action::Resize),
                Some(Ok(_)) => None,
                Some(Err(e)) => return Err(e),
                None => break, // stdin closed
            },
            Some(action) = results.recv() => Some(action),
            Some(event) = player_events.recv() => Some(Action::Player {
                event: Some(event),
                status: player.as_ref().map(PlayerHandle::status).unwrap_or_default(),
            }),
            () = sleep_until(timers.album.map(|t| t.0)), if timers.album.is_some() => {
                timers.album.take().map(|(_, id)| Action::AlbumDue(id))
            },
            () = sleep_until(timers.search.map(|t| t.0)), if timers.search.is_some() => {
                timers.search.take().map(|(_, generation)| Action::SearchDue(generation))
            },
            _ = ticker.tick(), if app.is_playing() => player.as_ref().and_then(|p| {
                // Skip the redraw unless something visible changed (the clock shows whole
                // seconds), which cuts redraws from 4/s to 1/s while playing.
                let status = p.status();
                visibly_changed(&app.player, &status).then_some(Action::Player {
                    event: None,
                    status,
                })
            }),
        };
        if let Some(action) = action {
            let effects = app.update(action);
            executor.run(effects, &mut timers);
            dirty = true;
        }
        if app.should_quit {
            break;
        }
    }
    if let Some(player) = &player {
        player.stop();
    }
    Ok(())
}

async fn sleep_until(when: Option<Instant>) {
    if let Some(when) = when {
        tokio::time::sleep_until(when).await;
    }
}

fn visibly_changed(old: &crate::player::Status, new: &crate::player::Status) -> bool {
    old.state != new.state
        || old.index != new.index
        || old.duration != new.duration
        || old.position.as_secs() != new.position.as_secs()
}

struct Executor {
    client: BandcampClient,
    player: Option<PlayerHandle>,
    results: UnboundedSender<Action>,
    settings_path: Option<std::path::PathBuf>,
    art: Option<std::sync::Arc<ratatui_image::picker::Picker>>,
}

impl Executor {
    fn run(&self, effects: Vec<Effect>, timers: &mut Timers) {
        for effect in effects {
            match effect {
                Effect::DebounceAlbum(id) => {
                    timers.album = Some((Instant::now() + ALBUM_DEBOUNCE, id));
                }
                Effect::DebounceSearch(generation) => {
                    timers.search = Some((Instant::now() + SEARCH_DEBOUNCE, generation));
                }
                Effect::FetchSearch { query, generation } => {
                    tracing::debug!(%query, "searching");
                    let client = self.client.clone();
                    let tx = self.results.clone();
                    tokio::spawn(async move {
                        let result = client.search(&query).await.map_err(|e| error_chain(&e));
                        let _ = tx.send(Action::SearchLoaded { generation, result });
                    });
                }
                Effect::FetchArtist(band_id) => {
                    let client = self.client.clone();
                    let tx = self.results.clone();
                    tokio::spawn(async move {
                        let result = client
                            .artist(band_id)
                            .await
                            .map(Box::new)
                            .map_err(|e| error_chain(&e));
                        let _ = tx.send(Action::ArtistLoaded { band_id, result });
                    });
                }
                Effect::FetchDiscover {
                    query,
                    cursor,
                    generation,
                } => {
                    let client = self.client.clone();
                    let tx = self.results.clone();
                    tokio::spawn(async move {
                        let result = client
                            .discover(&query, cursor.as_ref())
                            .await
                            .map_err(|e| error_chain(&e));
                        let _ = tx.send(Action::DiscoverLoaded { generation, result });
                    });
                }
                Effect::FetchAlbum { band_id, id, kind } => {
                    tracing::debug!(%id, "fetching album details");
                    let client = self.client.clone();
                    let tx = self.results.clone();
                    tokio::spawn(async move {
                        let result = client
                            .album(band_id, id, kind)
                            .await
                            .map(Box::new)
                            .map_err(|e| error_chain(&e));
                        let _ = tx.send(Action::AlbumLoaded { id, result });
                    });
                }
                Effect::Player(command) => {
                    if let Some(player) = &self.player {
                        apply(player, command);
                    }
                }
                Effect::FetchArt { id, size } => {
                    tracing::debug!(%id, ?size, "fetching cover");
                    let http = self.client.http().clone();
                    let tx = self.results.clone();
                    tokio::spawn(async move {
                        let result = fetch_art(&http, id, size).await;
                        let _ = tx.send(Action::ArtFetched { id, size, result });
                    });
                }
                Effect::EncodeArt { id, jpeg, art_box } => {
                    tracing::debug!(%id, ?art_box, "encoding cover");
                    let Some(picker) = self.art.clone() else {
                        continue;
                    };
                    let tx = self.results.clone();
                    // CPU-bound (decode, scale, encode): keep it off the async workers.
                    tokio::task::spawn_blocking(move || {
                        let result = crate::art::encode(&picker, &jpeg, art_box);
                        let _ = tx.send(Action::ArtEncoded {
                            id,
                            art_box,
                            result,
                        });
                    });
                }
                Effect::OpenUrl(url) => open_url(&url),
                Effect::SaveSettings(settings) => {
                    // Synchronous on purpose: the file is ~200 bytes, and writing in order
                    // guarantees the last change wins.
                    if let Some(path) = &self.settings_path
                        && let Err(e) = settings.save(path)
                    {
                        tracing::warn!(path = %path.display(), error = %e, "couldn't save state");
                    }
                }
            }
        }
    }
}

/// Downloads a cover's JPEG.
async fn fetch_art(
    http: &reqwest::Client,
    id: crate::bandcamp::ArtId,
    size: crate::bandcamp::images::ImageSize,
) -> Result<bytes::Bytes, String> {
    let response = http
        .get(crate::bandcamp::images::art_url(id, size))
        .timeout(Duration::from_secs(15))
        .send()
        .await
        .and_then(reqwest::Response::error_for_status)
        .map_err(|e| error_chain(&e))?;
    response.bytes().await.map_err(|e| error_chain(&e))
}

/// Opens `url` in the default browser without blocking or touching the terminal.
fn open_url(url: &url::Url) {
    use std::process::{Command, Stdio};
    #[cfg(target_os = "macos")]
    let mut command = Command::new("open");
    #[cfg(target_os = "windows")]
    let mut command = {
        let mut c = Command::new("cmd");
        c.args(["/C", "start", ""]);
        c
    };
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    let mut command = Command::new("xdg-open");
    let spawned = command
        .arg(url.as_str())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();
    match spawned {
        // Reap it in the background so it doesn't linger as a zombie.
        Ok(mut child) => {
            std::thread::spawn(move || child.wait());
        }
        Err(e) => tracing::warn!(%url, error = %e, "couldn't open browser"),
    }
}

fn apply(player: &PlayerHandle, command: PlayerCommand) {
    match command {
        PlayerCommand::PlayQueue(queue, start) => player.play_queue(queue, start),
        PlayerCommand::PlayIndex(index) => player.play(index),
        PlayerCommand::TogglePause => player.toggle_pause(),
        PlayerCommand::Next => player.next(),
        PlayerCommand::Previous => player.previous(),
        PlayerCommand::SeekBy(ms) => player.seek_by(ms),
        PlayerCommand::SetVolume(v) => player.set_volume(v),
    }
}
