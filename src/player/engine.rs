//! The player thread: owns the audio output and the play queue, starts and stops pipelines,
//! and preloads the next track for gapless playback.
//!
//! It's event-driven: it blocks on its command channel, and only ticks (4×/s) while a track is
//! playing, so it costs nothing when idle or paused.

use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use tokio::sync::mpsc::UnboundedSender;

use super::pipeline::{Pipeline, PipelineContext};
use super::{Command, PlaybackState, PlayerEvent, QueueTrack, Status};

const TICK: Duration = Duration::from_millis(250);
/// Start streaming the next track this long before the current one ends.
const PRELOAD_AHEAD: Duration = Duration::from_secs(15);
/// "Previous" restarts the current track if we're further in than this.
const RESTART_THRESHOLD: Duration = Duration::from_secs(3);

/// Messages to the engine thread: user commands plus notifications from pipelines.
#[derive(Debug)]
pub(crate) enum Msg {
    Command(Command),
    /// A pipeline queued a segment; re-evaluate state now rather than at the next tick.
    Ready,
    Failed {
        id: u64,
        message: String,
    },
}

pub(crate) struct Engine {
    ctx: Arc<PipelineContext>,
    rx: mpsc::Receiver<Msg>,
    events: UnboundedSender<PlayerEvent>,
    status: Arc<Mutex<Status>>,
    queue: Vec<QueueTrack>,
    index: Option<usize>,
    current: Option<Pipeline>,
    /// Preloaded next track: (queue index, pipeline).
    next: Option<(usize, Pipeline)>,
    paused: bool,
    next_id: u64,
    last_state: PlaybackState,
}

impl Engine {
    pub(crate) fn new(
        ctx: Arc<PipelineContext>,
        rx: mpsc::Receiver<Msg>,
        events: UnboundedSender<PlayerEvent>,
        status: Arc<Mutex<Status>>,
    ) -> Self {
        Self {
            ctx,
            rx,
            events,
            status,
            queue: Vec::new(),
            index: None,
            current: None,
            next: None,
            paused: false,
            next_id: 0,
            last_state: PlaybackState::Stopped,
        }
    }

    pub(crate) fn run(mut self) {
        loop {
            let active = self.current.is_some() && !self.paused;
            let msg = if active {
                self.rx.recv_timeout(TICK)
            } else {
                self.rx.recv().map_err(|_| RecvTimeoutError::Disconnected)
            };
            match msg {
                Ok(Msg::Command(command)) => self.handle(command),
                Ok(Msg::Ready) | Err(RecvTimeoutError::Timeout) => {}
                Ok(Msg::Failed { id, message }) => self.on_failed(id, message),
                Err(RecvTimeoutError::Disconnected) => break,
            }
            self.tick();
        }
        // Handle dropped: stop everything before the output goes away.
        self.stop();
    }

    fn handle(&mut self, command: Command) {
        match command {
            Command::PlayQueue { tracks, start } => {
                self.stop();
                self.queue = tracks;
                match self.playable_from(start) {
                    Some(i) => self.start_track(i, Duration::ZERO),
                    None => self.emit(PlayerEvent::QueueEnded),
                }
            }
            Command::Play(index) => match self.queue.get(index) {
                Some(track) if track.url.is_some() => self.start_track(index, Duration::ZERO),
                Some(_) => self.emit(PlayerEvent::TrackFailed {
                    index,
                    message: "track is not available for streaming".into(),
                }),
                None => {}
            },
            Command::TogglePause => self.set_paused(!self.paused),
            Command::Pause => self.set_paused(true),
            Command::Resume => self.set_paused(false),
            Command::Next => self.skip_forward(),
            Command::Previous => self.skip_back(),
            Command::Seek(to) => self.seek(to),
            Command::SeekBy(delta) => {
                let pos = self.position();
                let to = if delta.is_negative() {
                    pos.saturating_sub(Duration::from_millis(delta.unsigned_abs()))
                } else {
                    pos + Duration::from_millis(delta.unsigned_abs())
                };
                self.seek(to);
            }
            Command::SetVolume(volume) => {
                let volume = volume.clamp(0.0, 1.0);
                self.ctx.player.set_volume(volume);
                lock(&self.status).volume = volume;
            }
            Command::Stop => {
                self.stop();
                self.queue.clear();
            }
        }
    }

    /// Called after every message and tick: follows track ends, preloads, publishes status.
    fn tick(&mut self) {
        if let Some(current) = &self.current
            && let Some(segment) = current.segment()
            && segment.is_finished()
        {
            self.on_track_end();
        }

        if self.next.is_none()
            && let (Some(index), Some(current)) = (self.index, &self.current)
            && current.segment().is_some()
        {
            let remaining = self.queue[index].duration.saturating_sub(self.position());
            if remaining <= PRELOAD_AHEAD
                && let Some(next) = self.playable_from(index + 1)
            {
                tracing::debug!(next, "preloading");
                let pipeline = self.spawn_pipeline(next, Duration::ZERO);
                self.next = pipeline.map(|p| (next, p));
            }
        }

        let state = self.state();
        let position = self.position();
        {
            let mut status = lock(&self.status);
            status.state = state;
            status.index = self.index;
            status.position = position;
            status.duration = self
                .index
                .map_or(Duration::ZERO, |i| self.queue[i].duration);
        }
        if state != self.last_state {
            self.last_state = state;
            self.emit(PlayerEvent::StateChanged(state));
        }
    }

    fn on_track_end(&mut self) {
        if let Some((index, pipeline)) = self.next.take() {
            // Already queued in rodio right behind the finished track: nothing to start.
            self.current = Some(pipeline);
            self.set_index(Some(index));
        } else if let Some(next) = self.index.and_then(|i| self.playable_from(i + 1)) {
            self.start_track(next, Duration::ZERO);
        } else {
            self.stop();
            self.emit(PlayerEvent::QueueEnded);
        }
    }

    fn on_failed(&mut self, id: u64, message: String) {
        if self.next.as_ref().is_some_and(|(_, p)| p.id == id) {
            // It will be retried, and reported if it fails again, when it's due to play.
            tracing::warn!(%message, "preloading next track failed");
            self.next = None;
            return;
        }
        if self.current.as_ref().is_some_and(|p| p.id == id) {
            if let Some(index) = self.index {
                self.emit(PlayerEvent::TrackFailed { index, message });
            }
            self.current = None;
            self.on_track_end();
        }
    }

    fn skip_forward(&mut self) {
        if let Some(next) = self.index.and_then(|i| self.playable_from(i + 1)) {
            self.start_track(next, Duration::ZERO);
        } else {
            self.stop();
            self.emit(PlayerEvent::QueueEnded);
        }
    }

    fn skip_back(&mut self) {
        let Some(index) = self.index else { return };
        let previous = (0..index).rev().find(|&i| self.queue[i].url.is_some());
        match previous {
            Some(prev) if self.position() <= RESTART_THRESHOLD => {
                self.start_track(prev, Duration::ZERO);
            }
            _ => self.seek(Duration::ZERO),
        }
    }

    fn seek(&mut self, to: Duration) {
        let Some(index) = self.index else { return };
        if to >= self.queue[index].duration {
            self.skip_forward();
            return;
        }
        // The preloaded segment sits behind the current one in rodio's queue; the new one must
        // come first, so drop the preload (it'll be redone near the end).
        self.next = None;
        if let Some(current) = &self.current {
            current.seek(to);
        }
    }

    fn start_track(&mut self, index: usize, start: Duration) {
        self.current = None;
        self.next = None;
        self.current = self.spawn_pipeline(index, start);
        self.set_index(Some(index));
        self.set_paused(false);
    }

    fn spawn_pipeline(&mut self, index: usize, start: Duration) -> Option<Pipeline> {
        let url = self.queue[index].url.clone()?;
        self.next_id += 1;
        Some(Pipeline::spawn(&self.ctx, self.next_id, url, start))
    }

    fn stop(&mut self) {
        self.current = None;
        self.next = None;
        self.set_index(None);
        self.paused = false;
        self.ctx.player.play();
    }

    fn set_paused(&mut self, paused: bool) {
        if self.current.is_none() {
            return;
        }
        self.paused = paused;
        if paused {
            self.ctx.player.pause();
        } else {
            self.ctx.player.play();
        }
    }

    fn set_index(&mut self, index: Option<usize>) {
        if self.index != index {
            self.index = index;
            self.emit(PlayerEvent::TrackChanged(index));
        }
    }

    fn playable_from(&self, start: usize) -> Option<usize> {
        (start..self.queue.len()).find(|&i| self.queue[i].url.is_some())
    }

    fn position(&self) -> Duration {
        self.current
            .as_ref()
            .and_then(Pipeline::segment)
            .map_or(Duration::ZERO, |s| s.position())
    }

    fn state(&self) -> PlaybackState {
        match &self.current {
            None => PlaybackState::Stopped,
            Some(_) if self.paused => PlaybackState::Paused,
            Some(p) => match p.segment() {
                None => PlaybackState::Loading,
                Some(s) if s.starving.load(std::sync::atomic::Ordering::Relaxed) => {
                    PlaybackState::Buffering
                }
                Some(_) => PlaybackState::Playing,
            },
        }
    }

    fn emit(&self, event: PlayerEvent) {
        // The receiver may be gone during shutdown; nothing to do then.
        let _ = self.events.send(event);
    }
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}
