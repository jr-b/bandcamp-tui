//! Autoplay: when the queue runs out, play the next playable Discover release.

use super::{AUTOPLAY_MAX_SKIPS, App, Effect};
use crate::bandcamp::ItemId;

impl App {
    pub(super) fn toggle_autoplay(&mut self) -> Vec<Effect> {
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

    /// The user chose something to play: forget what autoplay or the queue was about to start.
    pub(super) fn cancel_autoplay_attempt(&mut self) {
        self.autoplay_attempt = None;
        self.autoplay_after = None;
        self.queue_attempt = None;
    }

    /// Plays the first playable Discover release after `after` (or from the selection if
    /// `after` isn't in the list anymore). Loads more pages if needed.
    pub(super) fn autoplay_next(&mut self, after: Option<ItemId>) -> Vec<Effect> {
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

    pub(super) fn autoplay_skip(&mut self, id: ItemId) -> Vec<Effect> {
        self.autoplay_skips += 1;
        if self.autoplay_skips >= AUTOPLAY_MAX_SKIPS {
            self.error("Autoplay stopped: the next few releases couldn't be played.");
            return vec![];
        }
        tracing::info!(%id, "autoplay: skipping unplayable release");
        self.autoplay_next(Some(id))
    }
}
