//! The play queue: releases (or single tracks) to play after the current one, in order.
//!
//! Entries are references (ids, names, link), not stream URLs: those expire after about a day,
//! so details are fetched when an entry's turn comes. That also keeps a saved queue valid
//! across restarts.

use std::collections::VecDeque;

use serde::{Deserialize, Serialize};
use url::Url;

use crate::bandcamp::{Album, ArtId, BandId, DiscoverItem, ItemId, TralbumKind};

/// More than anyone queues by hand; keeps `state.toml` small.
pub const MAX_ENTRIES: usize = 200;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QueueEntry {
    pub id: ItemId,
    pub band_id: BandId,
    pub kind: TralbumKind,
    pub title: String,
    pub artist: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub art_id: Option<ArtId>,
    pub url: Url,
    /// Just this track (index into the release's tracks); `None` plays the whole release.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub track: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub track_title: Option<String>,
}

impl QueueEntry {
    /// The whole release.
    #[must_use]
    pub fn release(item: &DiscoverItem) -> Self {
        Self {
            id: item.id,
            band_id: item.band_id,
            kind: item.kind,
            title: item.title.clone(),
            artist: item.artist.clone(),
            art_id: item.art_id,
            url: item.url.clone(),
            track: None,
            track_title: None,
        }
    }

    /// One track of `album`.
    #[must_use]
    pub fn track(album: &Album, index: usize) -> Option<Self> {
        let track = album.tracks.get(index)?;
        Some(Self {
            id: album.id,
            band_id: album.band.id,
            kind: album.kind,
            title: album.title.clone(),
            artist: track.artist.clone(),
            art_id: album.art_id,
            url: album.url.clone(),
            track: Some(index),
            track_title: Some(track.title.clone()),
        })
    }

    /// What to call it in messages: the track title for single tracks, else the release's.
    #[must_use]
    pub fn label(&self) -> String {
        let title = self.track_title.as_deref().unwrap_or(&self.title);
        format!("{title} — {}", self.artist)
    }
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Queue {
    entries: VecDeque<QueueEntry>,
}

impl Queue {
    /// Restores a saved queue (keeping at most [`MAX_ENTRIES`]).
    #[must_use]
    pub fn from_saved(entries: Vec<QueueEntry>) -> Self {
        Self {
            entries: entries.into_iter().take(MAX_ENTRIES).collect(),
        }
    }

    #[must_use]
    pub fn to_saved(&self) -> Vec<QueueEntry> {
        self.entries.iter().cloned().collect()
    }

    /// Adds to the end. Returns `false` (and drops it) if the queue is full.
    pub fn push_back(&mut self, entry: QueueEntry) -> bool {
        if self.entries.len() >= MAX_ENTRIES {
            return false;
        }
        self.entries.push_back(entry);
        true
    }

    /// Adds as the next entry to play. Returns `false` if the queue is full.
    pub fn push_front(&mut self, entry: QueueEntry) -> bool {
        if self.entries.len() >= MAX_ENTRIES {
            return false;
        }
        self.entries.push_front(entry);
        true
    }

    pub fn pop_front(&mut self) -> Option<QueueEntry> {
        self.entries.pop_front()
    }

    pub fn remove(&mut self, index: usize) -> Option<QueueEntry> {
        self.entries.remove(index)
    }

    /// Moves entry `index` one place earlier; returns its new index.
    pub fn move_up(&mut self, index: usize) -> Option<usize> {
        (index > 0 && index < self.entries.len()).then(|| {
            self.entries.swap(index, index - 1);
            index - 1
        })
    }

    /// Moves entry `index` one place later; returns its new index.
    pub fn move_down(&mut self, index: usize) -> Option<usize> {
        (index + 1 < self.entries.len()).then(|| {
            self.entries.swap(index, index + 1);
            index + 1
        })
    }

    pub fn clear(&mut self) {
        self.entries.clear();
    }

    #[must_use]
    pub fn front(&self) -> Option<&QueueEntry> {
        self.entries.front()
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = &QueueEntry> {
        self.entries.iter()
    }

    /// Whether any entry plays (part of) release `id`; for the list marker.
    #[must_use]
    pub fn contains_release(&self, id: ItemId) -> bool {
        self.entries.iter().any(|e| e.id == id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(id: u64) -> QueueEntry {
        QueueEntry {
            id: ItemId(id),
            band_id: BandId(1),
            kind: TralbumKind::Album,
            title: format!("Album {id}"),
            artist: "Artist".into(),
            art_id: None,
            url: "https://x.bandcamp.com/album/a".parse().unwrap(),
            track: None,
            track_title: None,
        }
    }

    fn ids(q: &Queue) -> Vec<u64> {
        q.iter().map(|e| e.id.0).collect()
    }

    #[test]
    fn adds_to_either_end_and_pops_in_order() {
        let mut q = Queue::default();
        q.push_back(entry(1));
        q.push_back(entry(2));
        q.push_front(entry(3));
        assert_eq!(ids(&q), [3, 1, 2]);
        assert_eq!(q.pop_front().map(|e| e.id.0), Some(3));
        assert_eq!(q.front().map(|e| e.id.0), Some(1));
        assert!(q.contains_release(ItemId(2)));
        assert!(!q.contains_release(ItemId(3)));
    }

    #[test]
    fn moves_and_removes() {
        let mut q = Queue::from_saved((1..=3).map(entry).collect());
        assert_eq!(q.move_up(0), None, "already first");
        assert_eq!(q.move_up(2), Some(1));
        assert_eq!(ids(&q), [1, 3, 2]);
        assert_eq!(q.move_down(2), None, "already last");
        assert_eq!(q.move_down(0), Some(1));
        assert_eq!(ids(&q), [3, 1, 2]);
        assert_eq!(q.remove(1).map(|e| e.id.0), Some(1));
        assert_eq!(q.remove(9), None);
        q.clear();
        assert!(q.is_empty());
    }

    #[test]
    fn is_capped() {
        let mut q = Queue::from_saved((0..MAX_ENTRIES as u64 + 5).map(entry).collect());
        assert_eq!(q.len(), MAX_ENTRIES, "restoring truncates");
        assert!(!q.push_back(entry(999)));
        assert!(!q.push_front(entry(999)));
        assert_eq!(q.len(), MAX_ENTRIES);
    }

    #[test]
    fn labels_tracks_by_their_title() {
        let mut e = entry(1);
        assert_eq!(e.label(), "Album 1 — Artist");
        e.track = Some(2);
        e.track_title = Some("Joyride".into());
        assert_eq!(e.label(), "Joyride — Artist");
    }
}
