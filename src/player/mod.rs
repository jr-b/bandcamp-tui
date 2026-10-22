//! Streaming audio player.
//!
//! ```text
//! CDN ─► HttpSource ─► symphonia ─► PCM ring ─► Segment ─► rodio Player ─► audio device
//!        (1 MiB window,  (decode      (~0.5 s,    (audio callback;
//!         tokio task)     thread)      lock-free)  never blocks)
//! ```
//!
//! [`PlayerHandle`] is the only public entry point. Commands are fire-and-forget; results come
//! back as [`PlayerEvent`]s, and [`PlayerHandle::status`] gives a cheap snapshot for rendering.

mod engine;
mod http_source;
mod pipeline;
mod segment;

use std::sync::{Arc, Mutex, PoisonError, mpsc};
use std::thread;
use std::time::Duration;

use rodio::mixer::Mixer;
use tokio::sync::mpsc::UnboundedSender;
use url::Url;

pub use http_source::{HttpSource, SourceCloser, StreamConfig, UrlRefresher};

use crate::bandcamp::Track;
use engine::{Engine, Msg};
use pipeline::PipelineContext;

/// A track in the play queue.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueueTrack {
    /// `None` for tracks that can't be streamed; the player skips them.
    pub url: Option<Url>,
    /// From the API. Used for preloading and seek bounds, so it doesn't need to be exact.
    pub duration: Duration,
}

impl From<&Track> for QueueTrack {
    fn from(track: &Track) -> Self {
        Self {
            url: track.stream_url.clone(),
            duration: track.duration,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PlaybackState {
    #[default]
    Stopped,
    /// Opening the stream.
    Loading,
    Playing,
    /// Playing, but waiting for data (slow network). Output is silent.
    Buffering,
    Paused,
}

/// A snapshot of the player, cheap to read on every frame.
#[derive(Debug, Clone, PartialEq)]
pub struct Status {
    pub state: PlaybackState,
    /// Index into the current queue.
    pub index: Option<usize>,
    pub position: Duration,
    pub duration: Duration,
    pub volume: f32,
}

impl Default for Status {
    fn default() -> Self {
        Self {
            state: PlaybackState::Stopped,
            index: None,
            position: Duration::ZERO,
            duration: Duration::ZERO,
            volume: 1.0,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlayerEvent {
    StateChanged(PlaybackState),
    /// The playing queue index changed (`None` when stopped).
    TrackChanged(Option<usize>),
    /// A track couldn't be played. Normally the player moves on to the next one; if the
    /// track's URL had `expired`, it stops instead, so the queue can be rebuilt with fresh URLs.
    TrackFailed {
        index: usize,
        message: String,
        expired: bool,
    },
    /// The last track finished.
    QueueEnded,
}

#[derive(Debug)]
enum Command {
    PlayQueue {
        tracks: Vec<QueueTrack>,
        start: usize,
    },
    Play(usize),
    TogglePause,
    Pause,
    Resume,
    Next,
    Previous,
    Seek(Duration),
    /// Relative seek in milliseconds.
    SeekBy(i64),
    SetVolume(f32),
    Stop,
}

/// Where audio goes.
pub enum Output {
    /// The system's default output device.
    DefaultDevice,
    /// A rodio mixer you drive yourself. Used in tests, where there may be no sound card.
    Mixer(Mixer),
}

impl std::fmt::Debug for Output {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::DefaultDevice => "DefaultDevice",
            Self::Mixer(_) => "Mixer",
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PlayerConfig {
    pub stream: StreamConfig,
    /// Decoded audio buffered ahead of the audio device.
    pub pcm_buffer: Duration,
}

impl Default for PlayerConfig {
    fn default() -> Self {
        Self {
            stream: StreamConfig::default(),
            pcm_buffer: Duration::from_millis(500),
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum PlayerError {
    #[error("could not open audio output")]
    Output(#[from] rodio::DeviceSinkError),
    #[error("could not start player thread")]
    Thread(#[from] std::io::Error),
}

/// Handle to the player thread. Dropping it (and all clones) stops playback.
#[derive(Debug, Clone)]
pub struct PlayerHandle {
    tx: mpsc::Sender<Msg>,
    status: Arc<Mutex<Status>>,
}

impl PlayerHandle {
    /// Starts the player thread. Downloads run on `runtime`, using `http`'s connection pool.
    /// `refresher` renews CDN URLs that expire mid-track (e.g. after a long pause).
    pub fn spawn(
        runtime: tokio::runtime::Handle,
        http: reqwest::Client,
        output: Output,
        config: PlayerConfig,
        refresher: Option<UrlRefresher>,
        events: UnboundedSender<PlayerEvent>,
    ) -> Result<Self, PlayerError> {
        let (tx, rx) = mpsc::channel();
        let status = Arc::new(Mutex::new(Status::default()));
        let (init_tx, init_rx) = mpsc::sync_channel(1);

        let engine_tx = tx.clone();
        let engine_status = Arc::clone(&status);
        thread::Builder::new()
            .name("player".into())
            .spawn(move || {
                // The device sink isn't `Send`, so it's opened (and kept alive) on this thread.
                let (_device, mixer) = match output {
                    Output::DefaultDevice => match rodio::DeviceSinkBuilder::open_default_sink() {
                        Ok(mut sink) => {
                            sink.log_on_drop(false);
                            let mixer = sink.mixer().clone();
                            (Some(sink), mixer)
                        }
                        Err(e) => {
                            let _ = init_tx.send(Err(e));
                            return;
                        }
                    },
                    Output::Mixer(mixer) => (None, mixer),
                };
                let player = Arc::new(rodio::Player::connect_new(&mixer));
                let ctx = Arc::new(PipelineContext {
                    runtime,
                    http,
                    player,
                    stream: config.stream,
                    refresher,
                    pcm_buffer: config.pcm_buffer,
                    engine: engine_tx,
                });
                let _ = init_tx.send(Ok(()));
                Engine::new(ctx, rx, events, engine_status).run();
            })?;

        match init_rx.recv() {
            Ok(Ok(())) => Ok(Self { tx, status }),
            Ok(Err(e)) => Err(PlayerError::Output(e)),
            Err(_) => Err(PlayerError::Thread(std::io::Error::other(
                "player thread exited during startup",
            ))),
        }
    }

    /// Replaces the queue and starts playing at `start` (or the next playable track after it).
    pub fn play_queue(&self, tracks: Vec<QueueTrack>, start: usize) {
        self.send(Command::PlayQueue { tracks, start });
    }

    /// Jumps to a track in the current queue.
    pub fn play(&self, index: usize) {
        self.send(Command::Play(index));
    }

    pub fn toggle_pause(&self) {
        self.send(Command::TogglePause);
    }

    pub fn pause(&self) {
        self.send(Command::Pause);
    }

    pub fn resume(&self) {
        self.send(Command::Resume);
    }

    pub fn next(&self) {
        self.send(Command::Next);
    }

    /// Restarts the current track, or goes to the previous one if near the start.
    pub fn previous(&self) {
        self.send(Command::Previous);
    }

    pub fn seek(&self, to: Duration) {
        self.send(Command::Seek(to));
    }

    /// Seeks relative to the current position, e.g. `seek_by(-10_000)` for back 10 s.
    pub fn seek_by(&self, millis: i64) {
        self.send(Command::SeekBy(millis));
    }

    /// `0.0`–`1.0`.
    pub fn set_volume(&self, volume: f32) {
        self.send(Command::SetVolume(volume));
    }

    pub fn stop(&self) {
        self.send(Command::Stop);
    }

    #[must_use]
    pub fn status(&self) -> Status {
        self.status
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    fn send(&self, command: Command) {
        // Fails only if the player thread is gone, in which case there's nothing to control.
        let _ = self.tx.send(Msg::Command(command));
    }
}
