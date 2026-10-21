//! Starting playback and following the player.

use std::sync::Arc;

use super::{AlbumView, App, Effect, Focus, PlayerCommand, Screen, VOLUME_STEP};
use crate::bandcamp::{Album, ItemId};
use crate::player::{PlaybackState, PlayerEvent, QueueTrack, Status};

impl App {
    pub(super) fn play_selected_release(&mut self) -> Vec<Effect> {
        let Some(id) = self.selected().map(|i| i.id) else {
            return vec![];
        };
        self.cancel_autoplay_attempt();
        self.play_or_fetch(id, None)
    }

    pub(super) fn play_selected_track(&mut self) -> Vec<Effect> {
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
    pub(super) fn play_or_fetch(&mut self, id: ItemId, track: Option<usize>) -> Vec<Effect> {
        if let Some(album) = self.albums.get(id) {
            return self.play_album(&album, track.unwrap_or(0));
        }
        self.pending_play = Some((id, track.unwrap_or(0)));
        self.info("Loading…");
        // Skip the debounce: the user explicitly asked for it.
        self.fetch_album(id).into_iter().collect()
    }

    pub(super) fn play_album(&mut self, album: &Arc<Album>, start: usize) -> Vec<Effect> {
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

    pub(super) fn player_update(
        &mut self,
        event: Option<PlayerEvent>,
        status: Status,
    ) -> Vec<Effect> {
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

    pub(super) fn player_command(&mut self, command: PlayerCommand) -> Vec<Effect> {
        if self.now_playing.is_none() || !self.player_available {
            return vec![];
        }
        vec![Effect::Player(command)]
    }

    pub(super) fn change_volume(&mut self, delta: f32) -> Vec<Effect> {
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

    pub(super) fn open_in_browser(&mut self) -> Vec<Effect> {
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
}
