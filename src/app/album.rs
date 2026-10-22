//! Release details: fetching them (once at a time) and what to do when they arrive.

use std::sync::Arc;

use super::{App, Effect};
use crate::bandcamp::{Album, BandId, ItemId, TralbumKind};

impl App {
    pub(super) fn album_due(&mut self, id: ItemId) -> Vec<Effect> {
        // Only fetch if the user is still on that release.
        let Some(item) = self.selected().filter(|i| i.id == id) else {
            return vec![];
        };
        let art_id = item.art_id;
        let mut effects: Vec<Effect> = self.fetch_album(id).into_iter().collect();
        effects.extend(self.fetch_art(art_id));
        effects
    }

    pub(super) fn fetch_album(&mut self, id: ItemId) -> Option<Effect> {
        let item = self.discover.items.iter().find(|i| i.id == id)?;
        let (band_id, kind) = (item.band_id, item.kind);
        self.fetch_release(id, band_id, kind)
    }

    /// Fetches details for any release, listed or not (e.g. a queue entry).
    pub(super) fn fetch_release(
        &mut self,
        id: ItemId,
        band_id: BandId,
        kind: TralbumKind,
    ) -> Option<Effect> {
        if self.albums.peek(id).is_some() || self.album_requests.contains(&id) {
            return None;
        }
        self.album_errors.remove(&id);
        self.album_requests.insert(id);
        Some(Effect::FetchAlbum { band_id, id, kind })
    }

    pub(super) fn album_loaded(
        &mut self,
        id: ItemId,
        result: Result<Box<Album>, String>,
    ) -> Vec<Effect> {
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

    /// Stream URLs are signed for about a day; re-fetch the album for fresh ones and resume at
    /// the track that failed.
    pub(super) fn refetch_expired(&mut self, index: usize) -> Vec<Effect> {
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
}
