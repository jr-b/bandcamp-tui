//! Cover art: which size to fetch, encoding it for the terminal, and what to show meanwhile.

use super::{AlbumView, App, Effect, Screen};
use crate::art::{Art, ArtBox};
use crate::bandcamp::ArtId;
use crate::bandcamp::images::ImageSize;

impl App {
    /// The cover box of the screen showing, as last drawn: the album pane's or the listening
    /// view's. Only one is on screen at a time, so only that one is fetched and encoded for.
    pub(super) fn art_box(&self) -> Option<ArtBox> {
        match self.screen {
            Screen::Browse => self.album_pane.art_box,
            Screen::Listening => self.listening.art_box,
        }
    }

    /// The image size needed for the cover box as last drawn.
    pub(super) fn wanted_art_size(&self) -> Option<ImageSize> {
        let art_box = self.art_box()?;
        Some(crate::art::source_size(art_box, self.art_font))
    }

    /// Whether cover `id` still has to be downloaded (or re-downloaded bigger).
    pub(super) fn needs_art(&self, id: ArtId) -> bool {
        let Some(wanted) = self.wanted_art_size().filter(|_| self.art_enabled) else {
            return false;
        };
        !self.art_requests.contains(&id)
            && !self.art_failed.contains(&id)
            && self.art_sources.peek(id).is_none_or(|s| s.size < wanted)
    }

    pub(super) fn fetch_art(&mut self, id: Option<ArtId>) -> Option<Effect> {
        let id = id.filter(|&id| self.needs_art(id))?;
        let size = self.wanted_art_size()?;
        self.art_requests.insert(id);
        Some(Effect::FetchArt { id, size })
    }

    /// Encodes cover `id` for the current box if its JPEG is here and that size isn't.
    pub(super) fn encode_art(&mut self, id: ArtId) -> Option<Effect> {
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

    pub(super) fn art_fetched(
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

    pub(super) fn art_encoded(
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
}

/// A downloaded cover, kept compressed.
#[derive(Debug, Clone)]
pub(super) struct ArtSource {
    jpeg: bytes::Bytes,
    size: ImageSize,
}
