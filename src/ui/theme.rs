//! Semantic styles. Rendering asks for a role ("accent", "error") rather than a colour, so a
//! monochrome theme can honour `NO_COLOR` (<https://no-color.org>) everywhere at once.
//!
//! Nothing relies on colour alone: playing rows have a ♪, states are spelled out, the selection
//! is reversed. The monochrome theme only drops the colours.
//!
//! Dim is for chrome and labels around content (hints, borders, separators, counts, "by "),
//! never for the content itself: names of releases, artists, albums and tracks stay in the
//! terminal's own foreground. How dim renders is up to the terminal, and on many it blends the
//! text halfway into the background, well under the 4.5:1 contrast text needs (WCAG 1.4.3).
//!
//! The presets use the Wong palette (Nature Methods 8, 2011), chosen so its hues stay apart
//! under the common colour-vision deficiencies. Unlike the 16 ANSI colours they don't follow
//! the terminal's scheme, which is the point: the roles look the same, and stay distinguishable,
//! everywhere. No hue reads well on every background, though: sky blue and orange fall to about
//! 2:1 on white. Hence two presets, picked by asking the terminal for its background colour.
//! Terminals that don't advertise truecolor get the nearest of the 256 indexed colours.

use std::cell::Cell;
use std::time::Duration;

use ratatui::style::{Color, Modifier, Style};

use crate::config::{Colors, ThemeChoice};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Theme {
    /// Focused borders, markers, keys in hints, progress, links.
    pub accent: Style,
    /// Something to note but not a failure: pre-orders, buffering, paused.
    pub warning: Style,
    /// Always bold too: on light backgrounds it shares its hue with `warning`.
    pub error: Style,
    /// The kind badge of artists and labels in search results (releases use `accent`).
    pub badge: Style,
    /// The app name in the header: the colour is reversed into a label.
    pub brand: Style,
}

const BRAND: Modifier = Modifier::REVERSED.union(Modifier::BOLD);

/// A Wong colour and its nearest xterm-256 stand-in.
#[derive(Debug, Clone, Copy)]
struct Swatch {
    rgb: Color,
    indexed: Color,
}

const SKY_BLUE: Swatch = Swatch {
    rgb: Color::Rgb(0x56, 0xb4, 0xe9),
    indexed: Color::Indexed(74),
};
const BLUE: Swatch = Swatch {
    rgb: Color::Rgb(0x00, 0x72, 0xb2),
    indexed: Color::Indexed(25),
};
const ORANGE: Swatch = Swatch {
    rgb: Color::Rgb(0xe6, 0x9f, 0x00),
    indexed: Color::Indexed(178),
};
const VERMILLION: Swatch = Swatch {
    rgb: Color::Rgb(0xd5, 0x5e, 0x00),
    indexed: Color::Indexed(166),
};
const REDDISH_PURPLE: Swatch = Swatch {
    rgb: Color::Rgb(0xcc, 0x79, 0xa7),
    indexed: Color::Indexed(175),
};

/// How many colours the terminal can show.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Depth {
    TrueColor,
    Ansi256,
}

impl Swatch {
    const fn at(self, depth: Depth) -> Color {
        match depth {
            Depth::TrueColor => self.rgb,
            Depth::Ansi256 => self.indexed,
        }
    }
}

/// A preset's roles, before choosing a colour depth.
struct Palette {
    accent: Swatch,
    warning: Swatch,
    error: Swatch,
    badge: Swatch,
}

impl Palette {
    const fn theme(&self, depth: Depth) -> Theme {
        let accent = self.accent.at(depth);
        Theme {
            accent: Style::new().fg(accent),
            warning: Style::new().fg(self.warning.at(depth)),
            error: Style::new()
                .fg(self.error.at(depth))
                .add_modifier(Modifier::BOLD),
            badge: Style::new().fg(self.badge.at(depth)),
            brand: Style::new().fg(accent).add_modifier(BRAND),
        }
    }
}

/// For dark backgrounds, and the fallback when the background is unknown: sky blue accent,
/// orange notes, vermillion errors.
const DARK_PALETTE: Palette = Palette {
    accent: SKY_BLUE,
    warning: ORANGE,
    error: VERMILLION,
    badge: REDDISH_PURPLE,
};

/// For light backgrounds: blue accent, and vermillion notes, since orange is unreadable on
/// white. Errors are vermillion too; bold sets them apart.
const LIGHT_PALETTE: Palette = Palette {
    accent: BLUE,
    warning: VERMILLION,
    error: VERMILLION,
    badge: REDDISH_PURPLE,
};

/// The dark preset in truecolor.
pub const DARK: Theme = DARK_PALETTE.theme(Depth::TrueColor);
/// The light preset in truecolor.
pub const LIGHT: Theme = LIGHT_PALETTE.theme(Depth::TrueColor);

/// For `NO_COLOR`: only bold, dim and reverse.
pub const MONO: Theme = Theme {
    accent: Style::new(),
    warning: Style::new(),
    error: Style::new().add_modifier(Modifier::BOLD),
    badge: Style::new(),
    brand: Style::new().add_modifier(BRAND),
};

/// The selected row of a focused list. Dim parts of the row are lifted: reversed dim text is
/// a blend of the foreground on itself and close to unreadable.
pub const SELECTED: Style = Style::new()
    .add_modifier(Modifier::REVERSED)
    .remove_modifier(Modifier::DIM);

/// The selected row of a list that doesn't have focus: still findable, but quieter. Bold and dim
/// share an off switch (SGR 22), so many terminals can't show both; dim gives way here too.
pub const SELECTED_UNFOCUSED: Style = Style::new()
    .add_modifier(Modifier::BOLD.union(Modifier::UNDERLINED))
    .remove_modifier(Modifier::DIM);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Background {
    Dark,
    Light,
}

impl Theme {
    /// The theme for the config's choice and overrides, at the terminal's colour `depth`.
    ///
    /// `NO_COLOR` only applies to `auto`, and then the overrides are dropped too: a theme set
    /// in the config wins over the environment, as no-color.org asks. `background` is only
    /// called for `auto`, since detecting it asks the terminal.
    #[must_use]
    pub fn resolve(
        choice: ThemeChoice,
        colors: &Colors,
        no_color: bool,
        depth: Depth,
        background: impl FnOnce() -> Background,
    ) -> Self {
        let palette = match choice {
            ThemeChoice::Auto if no_color => return MONO,
            ThemeChoice::Auto => match background() {
                Background::Dark => &DARK_PALETTE,
                Background::Light => &LIGHT_PALETTE,
            },
            ThemeChoice::Dark => &DARK_PALETTE,
            ThemeChoice::Light => &LIGHT_PALETTE,
            ThemeChoice::Mono => return MONO.with(colors),
        };
        palette.theme(depth).with(colors)
    }

    /// Replaces the colour of each role `colors` sets.
    #[must_use]
    pub fn with(self, colors: &Colors) -> Self {
        let set = |style: Style, color: Option<Color>| match color {
            Some(color) => style.fg(color),
            None => style,
        };
        Self {
            accent: set(self.accent, colors.accent),
            warning: set(self.warning, colors.warning),
            error: set(self.error, colors.error),
            badge: set(self.badge, colors.badge),
            brand: set(self.brand, colors.brand),
        }
    }
}

/// Truecolor if `COLORTERM` says so, else 256 colours. `COLORTERM` often doesn't survive ssh or
/// sudo; the fallback then only rounds the hues slightly.
#[must_use]
pub fn detect_depth() -> Depth {
    depth_from(std::env::var("COLORTERM").ok().as_deref())
}

fn depth_from(colorterm: Option<&str>) -> Depth {
    match colorterm {
        Some("truecolor" | "24bit") => Depth::TrueColor,
        _ => Depth::Ansi256,
    }
}

/// Whether `NO_COLOR` is set to anything non-empty.
#[must_use]
pub fn no_color() -> bool {
    std::env::var_os("NO_COLOR").is_some_and(|v| !v.is_empty())
}

/// Asks the terminal for its background colour (OSC 11). Terminals that don't support the query
/// still answer the device-attributes query sent after it, so this doesn't wait out the timeout
/// on them. Without an answer, `COLORFGBG` (set by rxvt, Konsole and others) decides, else dark.
///
/// Must run before the app reads input: the reply arrives on stdin.
#[must_use]
pub fn detect_background() -> Background {
    let mut options = terminal_colorsaurus::QueryOptions::default();
    // Long enough for ssh; the timeout only applies when nothing answers at all.
    options.timeout = Duration::from_millis(500);
    match terminal_colorsaurus::theme_mode(options) {
        Ok(terminal_colorsaurus::ThemeMode::Dark) => Background::Dark,
        Ok(terminal_colorsaurus::ThemeMode::Light) => Background::Light,
        Err(e) => {
            let fallback = std::env::var("COLORFGBG")
                .ok()
                .and_then(|v| from_colorfgbg(&v))
                .unwrap_or(Background::Dark);
            tracing::info!(error = %e, ?fallback, "terminal background query failed");
            fallback
        }
    }
}

/// `COLORFGBG` is "fg;bg" (sometimes "fg;default;bg") in ANSI indices; the background is last.
/// White and bright white (7, 15) are light backgrounds, as vim reads it.
fn from_colorfgbg(value: &str) -> Option<Background> {
    match value.rsplit(';').next()?.parse::<u8>().ok()? {
        7 | 15 => Some(Background::Light),
        _ => Some(Background::Dark),
    }
}

thread_local! {
    // Per thread so tests can render several themes side by side; the app renders on one thread.
    static CURRENT: Cell<Theme> = const { Cell::new(DARK) };
}

/// The theme rendering uses.
#[must_use]
pub fn theme() -> Theme {
    CURRENT.with(Cell::get)
}

/// Switches the theme for this thread: at startup, on the thread that renders, and in tests.
pub fn set(theme: Theme) {
    CURRENT.with(|current| current.set(theme));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn never() -> Background {
        panic!("the background is only asked for with `auto`")
    }

    const TRUE: Depth = Depth::TrueColor;

    #[test]
    fn auto_follows_the_background() {
        let none = Colors::default();
        let auto = |bg| Theme::resolve(ThemeChoice::Auto, &none, false, TRUE, move || bg);
        assert_eq!(auto(Background::Dark), DARK);
        assert_eq!(auto(Background::Light), LIGHT);
    }

    #[test]
    fn explicit_themes_dont_ask_the_terminal() {
        let none = Colors::default();
        let resolve = |choice| Theme::resolve(choice, &none, false, TRUE, never);
        assert_eq!(resolve(ThemeChoice::Dark), DARK);
        assert_eq!(resolve(ThemeChoice::Light), LIGHT);
        assert_eq!(resolve(ThemeChoice::Mono), MONO);
    }

    #[test]
    fn no_color_only_overrides_auto() {
        let colors = Colors {
            accent: Some(Color::Green),
            ..Colors::default()
        };
        assert_eq!(
            Theme::resolve(ThemeChoice::Auto, &colors, true, TRUE, never),
            MONO
        );
        let light = Theme::resolve(ThemeChoice::Light, &colors, true, TRUE, never);
        assert_eq!(light.accent, Style::new().fg(Color::Green));
    }

    #[test]
    fn presets_use_the_wong_palette() {
        let fg = |style: Style| style.fg.expect("a colour");
        // Dark backgrounds: the light Wong hues.
        assert_eq!(fg(DARK.accent), Color::Rgb(0x56, 0xb4, 0xe9), "sky blue");
        assert_eq!(fg(DARK.warning), Color::Rgb(0xe6, 0x9f, 0x00), "orange");
        assert_eq!(fg(DARK.error), Color::Rgb(0xd5, 0x5e, 0x00), "vermillion");
        assert_eq!(fg(DARK.badge), Color::Rgb(0xcc, 0x79, 0xa7), "purple");
        // Light backgrounds: sky blue and orange fall to about 2:1 on white.
        assert_eq!(fg(LIGHT.accent), Color::Rgb(0x00, 0x72, 0xb2), "blue");
        assert_eq!(fg(LIGHT.warning), Color::Rgb(0xd5, 0x5e, 0x00));
        // Errors share vermillion with warnings there; bold keeps them apart, in every preset.
        for theme in [DARK, LIGHT, MONO] {
            assert!(theme.error.add_modifier.contains(Modifier::BOLD));
        }
        assert_eq!(fg(DARK.brand), fg(DARK.accent));
        assert_eq!(fg(LIGHT.brand), fg(LIGHT.accent));
    }

    #[test]
    fn without_truecolor_the_nearest_256_colours_stand_in() {
        let none = Colors::default();
        let dark = Theme::resolve(ThemeChoice::Dark, &none, false, Depth::Ansi256, never);
        assert_eq!(dark.accent.fg, Some(Color::Indexed(74)));
        assert_eq!(dark.warning.fg, Some(Color::Indexed(178)));
        assert_eq!(dark.error.fg, Some(Color::Indexed(166)));
        assert_eq!(dark.badge.fg, Some(Color::Indexed(175)));
        let light = Theme::resolve(ThemeChoice::Light, &none, false, Depth::Ansi256, never);
        assert_eq!(light.accent.fg, Some(Color::Indexed(25)));
        assert_eq!(light.warning.fg, Some(Color::Indexed(166)));
        // Overrides still win, at either depth.
        let colors = Colors {
            accent: Some(Color::Cyan),
            ..Colors::default()
        };
        let overridden = Theme::resolve(ThemeChoice::Dark, &colors, false, Depth::Ansi256, never);
        assert_eq!(overridden.accent.fg, Some(Color::Cyan));
    }

    #[test]
    fn reads_colorterm() {
        assert_eq!(depth_from(Some("truecolor")), Depth::TrueColor);
        assert_eq!(depth_from(Some("24bit")), Depth::TrueColor);
        assert_eq!(depth_from(Some("")), Depth::Ansi256);
        assert_eq!(depth_from(Some("256color")), Depth::Ansi256);
        assert_eq!(depth_from(None), Depth::Ansi256);
    }

    #[test]
    fn overrides_replace_only_the_colour() {
        let colors = Colors {
            error: Some(Color::Rgb(0xd2, 0x0f, 0x39)),
            brand: Some(Color::Indexed(33)),
            ..Colors::default()
        };
        let theme = MONO.with(&colors);
        assert_eq!(
            theme.error,
            Style::new().fg(Color::Rgb(0xd2, 0x0f, 0x39)).bold()
        );
        assert_eq!(
            theme.brand,
            Style::new().fg(Color::Indexed(33)).add_modifier(BRAND)
        );
        assert_eq!(theme.accent, MONO.accent);
    }

    #[test]
    fn reads_colorfgbg() {
        assert_eq!(from_colorfgbg("0;15"), Some(Background::Light));
        assert_eq!(from_colorfgbg("0;default;7"), Some(Background::Light));
        assert_eq!(from_colorfgbg("15;0"), Some(Background::Dark));
        assert_eq!(from_colorfgbg("15;8"), Some(Background::Dark));
        assert_eq!(from_colorfgbg("garbage"), None);
    }
}
