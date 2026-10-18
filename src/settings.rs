//! Remembered state between runs: the last Discover filters, the volume, the queue and
//! whether the listening view was open.
//!
//! Stored as `state.toml` next to the log file. It's written by the app (not meant for hand
//! editing), so a missing or broken file just means defaults.

use std::io;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::app::Screen;
use crate::bandcamp::{DiscoverQuery, Slice};
use crate::queue::QueueEntry;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub discover: DiscoverSettings,
    /// `0.0`–`1.0`.
    pub volume: f32,
    /// The play queue, restored at startup without starting playback.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub queue: Vec<QueueEntry>,
    /// Browsing or the listening view.
    pub screen: Screen,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct DiscoverSettings {
    pub tags: Vec<String>,
    pub slice: Slice,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub time_facet_id: Option<i64>,
    pub geoname_id: i64,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            discover: DiscoverSettings::default(),
            volume: 1.0,
            queue: Vec::new(),
            screen: Screen::Browse,
        }
    }
}

impl Settings {
    /// Reads `path`. A missing file gives the defaults; a broken one is logged and ignored.
    #[must_use]
    pub fn load(path: &Path) -> Self {
        let text = match std::fs::read_to_string(path) {
            Ok(text) => text,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Self::default(),
            Err(e) => {
                tracing::warn!(path = %path.display(), error = %e, "couldn't read state");
                return Self::default();
            }
        };
        match toml::from_str::<Self>(&text) {
            Ok(mut settings) => {
                settings.volume = if settings.volume.is_finite() {
                    settings.volume.clamp(0.0, 1.0)
                } else {
                    1.0
                };
                settings
            }
            Err(e) => {
                tracing::warn!(path = %path.display(), error = %e, "ignoring broken state file");
                Self::default()
            }
        }
    }

    /// Writes atomically (temp file + rename), so a crash never leaves a half-written file.
    pub fn save(&self, path: &Path) -> io::Result<()> {
        let text = toml::to_string_pretty(self).map_err(io::Error::other)?;
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let tmp = path.with_extension("toml.tmp");
        std::fs::write(&tmp, text)?;
        std::fs::rename(&tmp, path)
    }

    #[must_use]
    pub fn query(&self) -> DiscoverQuery {
        DiscoverQuery {
            tags: self.discover.tags.clone(),
            slice: self.discover.slice,
            time_facet_id: self.discover.time_facet_id,
            geoname_id: self.discover.geoname_id,
            ..DiscoverQuery::default()
        }
    }

    #[must_use]
    pub fn from_state(query: &DiscoverQuery, volume: f32) -> Self {
        Self {
            discover: DiscoverSettings {
                tags: query.tags.clone(),
                slice: query.slice,
                time_facet_id: query.time_facet_id,
                geoname_id: query.geoname_id,
            },
            volume,
            queue: Vec::new(),
            screen: Screen::Browse,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_path(name: &str) -> std::path::PathBuf {
        let dir =
            std::env::temp_dir().join(format!("bandcamp-tui-test-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir.join("state.toml")
    }

    #[test]
    fn round_trips() {
        let path = temp_path("roundtrip");
        let query = DiscoverQuery {
            tags: vec!["electronic".into(), "techno".into()],
            slice: Slice::New,
            time_facet_id: Some(1),
            geoname_id: 2_950_159,
            ..DiscoverQuery::default()
        };
        let settings = Settings::from_state(&query, 0.4);
        settings.save(&path).unwrap();
        let loaded = Settings::load(&path);
        assert_eq!(loaded, settings);
        assert_eq!(loaded.query(), query);
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("slice = \"new\""), "{text}");
    }

    #[test]
    fn round_trips_the_queue() {
        let path = temp_path("queue");
        let entry = QueueEntry {
            id: crate::bandcamp::ItemId(3_457_085_593),
            band_id: crate::bandcamp::BandId(2_686_449_259),
            kind: crate::bandcamp::TralbumKind::Album,
            title: "Ocean Company".into(),
            artist: "Ocean Company".into(),
            art_id: Some(crate::bandcamp::ArtId(4_072_557_709)),
            url: "https://hotelpoolsmusic.bandcamp.com/album/ocean-company"
                .parse()
                .unwrap(),
            track: None,
            track_title: None,
        };
        let single = QueueEntry {
            track: Some(1),
            track_title: Some("Joyride".into()),
            ..entry.clone()
        };
        let mut settings = Settings::from_state(&DiscoverQuery::default(), 0.5);
        settings.queue = vec![entry, single];
        settings.screen = Screen::Listening;
        settings.save(&path).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("[[queue]]"), "{text}");
        assert!(text.contains("kind = \"album\""), "{text}");
        assert!(text.contains("screen = \"listening\""), "{text}");
        assert_eq!(Settings::load(&path), settings);
    }

    #[test]
    fn missing_or_broken_files_give_defaults() {
        let path = temp_path("missing");
        assert_eq!(Settings::load(&path), Settings::default());

        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "this is [not toml").unwrap();
        assert_eq!(Settings::load(&path), Settings::default());
    }

    #[test]
    fn partial_files_fill_in_defaults_and_clamp_volume() {
        let path = temp_path("partial");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "volume = 7.5\n[discover]\nslice = \"rand\"\n").unwrap();
        let loaded = Settings::load(&path);
        assert!((loaded.volume - 1.0).abs() < f32::EPSILON);
        assert_eq!(loaded.discover.slice, Slice::Random);
        assert_eq!(loaded.discover.tags, Vec::<String>::new());
        assert_eq!(loaded.screen, Screen::Browse);
    }
}
