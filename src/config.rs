//! Optional user configuration, `config.toml`.
//!
//! Hand-edited, so unlike `state.toml` a mistake is reported at startup rather than ignored.
//!
//! ```toml
//! # Cover art: "auto" (when the terminal supports it, never inside tmux), "on", or "off".
//! album_art = "auto"
//! # When an album ends, continue with the next Discover release. Toggle with `a`.
//! autoplay = true
//! # Switch to the listening view (`v`) whenever the queue starts a release.
//! listen_on_queue = false
//! # Colours: "auto" (dark or light, from the terminal's background), "dark", "light", or
//! # "mono" (no colour). `NO_COLOR` turns "auto" into "mono".
//! theme = "auto"
//!
//! # Optional: replace the colour of a role. Names follow the terminal's own palette:
//! # "black" "red" "green" "yellow" "blue" "magenta" "cyan" "white", each also with a
//! # "bright-" prefix. Also 0–255 (the 256-colour palette), "#rrggbb" (needs a truecolor
//! # terminal), or "default" (the terminal's foreground).
//! [colors]
//! accent = "blue"     # focus, markers, key hints, progress
//! warning = "magenta" # pre-orders, paused, buffering
//! error = "red"
//! badge = "magenta"   # artist and label badges in search
//! brand = "blue"      # the "bandcamp" label in the header
//! ```

use std::fmt;
use std::path::Path;

use ratatui::style::Color;
use serde::{Deserialize, Deserializer};

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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ThemeChoice {
    /// Dark or light from the terminal's background; mono with `NO_COLOR`.
    #[default]
    Auto,
    Dark,
    Light,
    /// No colours, whatever `NO_COLOR` says.
    Mono,
}

/// Per-role colours that replace the theme's. See [`crate::ui::theme::Theme`] for the roles.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Colors {
    #[serde(deserialize_with = "color")]
    pub accent: Option<Color>,
    #[serde(deserialize_with = "color")]
    pub warning: Option<Color>,
    #[serde(deserialize_with = "color")]
    pub error: Option<Color>,
    #[serde(deserialize_with = "color")]
    pub badge: Option<Color>,
    #[serde(deserialize_with = "color")]
    pub brand: Option<Color>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub album_art: AlbumArt,
    /// Continue with the next Discover release when an album ends.
    pub autoplay: bool,
    /// Switch to the listening view whenever the queue starts a release.
    pub listen_on_queue: bool,
    pub theme: ThemeChoice,
    pub colors: Colors,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            album_art: AlbumArt::default(),
            autoplay: true,
            listen_on_queue: false,
            theme: ThemeChoice::default(),
            colors: Colors::default(),
        }
    }
}

const ANSI: [(&str, Color, Color); 8] = [
    ("black", Color::Black, Color::DarkGray),
    ("red", Color::Red, Color::LightRed),
    ("green", Color::Green, Color::LightGreen),
    ("yellow", Color::Yellow, Color::LightYellow),
    ("blue", Color::Blue, Color::LightBlue),
    ("magenta", Color::Magenta, Color::LightMagenta),
    ("cyan", Color::Cyan, Color::LightCyan),
    // Ratatui calls ANSI white "gray" and bright white "white".
    ("white", Color::Gray, Color::White),
];

/// A colour name as the terminal calls it ("blue", "bright-blue"), "default", or "#rrggbb".
fn parse_color(text: &str) -> Option<Color> {
    let name = text.trim().to_ascii_lowercase().replace(['_', ' '], "-");
    if name == "default" {
        return Some(Color::Reset);
    }
    if let Some(hex) = name.strip_prefix('#') {
        if hex.len() != 6 || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
            return None;
        }
        let channel = |i: usize| u8::from_str_radix(&hex[i..i + 2], 16).ok();
        return Some(Color::Rgb(channel(0)?, channel(2)?, channel(4)?));
    }
    let (bright, base) = match name.strip_prefix("bright-") {
        Some(base) => (true, base),
        None => (false, name.as_str()),
    };
    let &(_, normal, light) = ANSI.iter().find(|(n, _, _)| *n == base)?;
    Some(if bright { light } else { normal })
}

/// `[colors]` values: a name or hex string ([`parse_color`]), or a 256-colour palette index.
fn color<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Option<Color>, D::Error> {
    struct Visitor;
    impl serde::de::Visitor<'_> for Visitor {
        type Value = Color;

        fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str(
                "a colour: a name like \"blue\" or \"bright-magenta\", 0–255, \"#rrggbb\" \
                 or \"default\"",
            )
        }

        fn visit_str<E: serde::de::Error>(self, v: &str) -> Result<Color, E> {
            parse_color(v).ok_or_else(|| E::invalid_value(serde::de::Unexpected::Str(v), &self))
        }

        fn visit_i64<E: serde::de::Error>(self, v: i64) -> Result<Color, E> {
            u8::try_from(v)
                .map(Color::Indexed)
                .map_err(|_| E::invalid_value(serde::de::Unexpected::Signed(v), &self))
        }
    }
    deserializer.deserialize_any(Visitor).map(Some)
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
    fn autoplay_defaults_on_and_can_be_turned_off() {
        let path = write("autoplay-default", "album_art = \"off\"\n");
        assert!(Config::load(&path).unwrap().autoplay);
        let path = write("autoplay-off", "autoplay = false\n");
        assert!(!Config::load(&path).unwrap().autoplay);
    }

    #[test]
    fn listen_on_queue_defaults_off() {
        assert!(!Config::default().listen_on_queue);
        let path = write("listen", "listen_on_queue = true\n");
        assert!(Config::load(&path).unwrap().listen_on_queue);
    }

    #[test]
    fn theme_defaults_to_auto() {
        assert_eq!(Config::default().theme, ThemeChoice::Auto);
        let path = write("theme", "theme = \"light\"\n");
        assert_eq!(Config::load(&path).unwrap().theme, ThemeChoice::Light);
    }

    #[test]
    fn parses_colors() {
        let path = write(
            "colors",
            "[colors]\naccent = \"Bright Blue\"\nwarning = \"#B58900\"\nerror = 160\n\
             badge = \"white\"\nbrand = \"default\"\n",
        );
        let colors = Config::load(&path).unwrap().colors;
        assert_eq!(
            colors,
            Colors {
                accent: Some(Color::LightBlue),
                warning: Some(Color::Rgb(0xb5, 0x89, 0x00)),
                error: Some(Color::Indexed(160)),
                badge: Some(Color::Gray),
                brand: Some(Color::Reset),
            }
        );
        let path = write("colors-partial", "[colors]\naccent = \"magenta\"\n");
        let colors = Config::load(&path).unwrap().colors;
        assert_eq!(colors.accent, Some(Color::Magenta));
        assert_eq!(colors.warning, None);
    }

    #[test]
    fn bad_colors_say_what_is_accepted() {
        for (name, text) in [
            ("name", "accent = \"bleu\""),
            ("hex", "accent = \"#12345\""),
            ("index", "accent = 256"),
            ("role", "highlight = \"red\""),
        ] {
            let path = write(&format!("bad-color-{name}"), &format!("[colors]\n{text}\n"));
            let chain = crate::format::error_chain(&Config::load(&path).unwrap_err());
            assert!(chain.contains("invalid config"), "{chain}");
            if name != "role" {
                assert!(chain.contains("bright-magenta"), "{chain}");
            }
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
