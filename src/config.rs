//! Optional user configuration, `config.toml`.
//!
//! Hand-edited, so unlike `state.toml` a mistake is reported at startup rather than ignored.
//!
//! ```toml
//! # Cover art: "auto" (when the terminal supports it, never inside tmux), "on", or "off".
//! album_art = "auto"
//! ```

use std::path::Path;

use serde::Deserialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AlbumArt {
    /// Show art if the terminal supports kitty, iTerm2 or sixel graphics; text only in tmux.
    #[default]
    Auto,
    /// Also try inside tmux (needs `set -g allow-passthrough on`) and trust environment hints
    /// for terminals that don't answer the capability query.
    On,
    Off,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub album_art: AlbumArt,
}

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("couldn't read {path}")]
    Read {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("invalid config in {path}")]
    Parse {
        path: String,
        #[source]
        source: toml::de::Error,
    },
}

impl Config {
    /// Reads `path`; a missing file means defaults.
    pub fn load(path: &Path) -> Result<Self, ConfigError> {
        let text = match std::fs::read_to_string(path) {
            Ok(text) => text,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Self::default()),
            Err(source) => {
                return Err(ConfigError::Read {
                    path: path.display().to_string(),
                    source,
                });
            }
        };
        toml::from_str(&text).map_err(|source| ConfigError::Parse {
            path: path.display().to_string(),
            source,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(name: &str, text: &str) -> std::path::PathBuf {
        let dir =
            std::env::temp_dir().join(format!("bandcamp-tui-config-{}-{name}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.toml");
        std::fs::write(&path, text).unwrap();
        path
    }

    #[test]
    fn missing_file_is_default() {
        let path = std::env::temp_dir().join("bandcamp-tui-no-such-config.toml");
        assert_eq!(Config::load(&path).unwrap(), Config::default());
        assert_eq!(Config::default().album_art, AlbumArt::Auto);
    }

    #[test]
    fn parses_album_art() {
        for (text, expected) in [
            ("album_art = \"on\"", AlbumArt::On),
            ("album_art = \"off\"", AlbumArt::Off),
            ("# nothing set\n", AlbumArt::Auto),
        ] {
            let path = write("parse", text);
            assert_eq!(Config::load(&path).unwrap().album_art, expected, "{text}");
        }
    }

    #[test]
    fn mistakes_are_errors_with_a_location() {
        let path = write("typo", "album_art = \"yes\"\n");
        let err = Config::load(&path).unwrap_err();
        let chain = crate::format::error_chain(&err);
        assert!(chain.contains("invalid config"), "{chain}");
        assert!(chain.contains("yes"), "{chain}");

        let path = write("unknown", "albumart = \"on\"\n");
        assert!(Config::load(&path).is_err(), "unknown keys are rejected");
    }
}
