//! The play queue: adding to it, the `u` popup, and playing what comes next.

use std::sync::Arc;

use super::{
    Action, AlbumView, App, Effect, Focus, PlayerCommand, QUEUE_PREFETCH, Screen, UpNext, move_list,
};
use crate::bandcamp::{Album, ItemId};
use crate::player::QueueTrack;
use crate::queue::QueueEntry;

impl App {
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
    pub(super) fn enqueue(&mut self, next: bool) -> Vec<Effect> {
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
    pub(super) fn queue_update(&mut self, action: &Action) -> Option<Vec<Effect>> {
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
    pub(super) fn next_from_queue(&mut self, finished: Option<ItemId>) -> Vec<Effect> {
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

    pub(super) fn start_queue_entry(&mut self, entry: &QueueEntry) -> Vec<Effect> {
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
    pub(super) fn play_queue_entry(
        &mut self,
        album: &Arc<Album>,
        only: Option<usize>,
    ) -> Vec<Effect> {
        let effects = self.play_queue_entry_now(album, only);
        let started = effects
            .iter()
            .any(|e| matches!(e, Effect::Player(PlayerCommand::PlayQueue(..))));
        if self.listen_on_queue && started {
            self.screen = Screen::Listening;
        }
        effects
    }

    pub(super) fn play_queue_entry_now(
        &mut self,
        album: &Arc<Album>,
        only: Option<usize>,
    ) -> Vec<Effect> {
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
    pub(super) fn prefetch_queue_front(&mut self) -> Option<Effect> {
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
}
