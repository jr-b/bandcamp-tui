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
//! The presets use the 16 ANSI colours, which the terminal's theme defines, so they follow the
//! user's palette. No hue reads well on every background, though: in common default palettes
//! cyan and yellow fall to about 2:1 on white, and blue to about 2:1 on black. Hence two
//! presets, picked by asking the terminal for its background colour.

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
    pub error: Style,
    /// The kind badge of artists and labels in search results (releases use `accent`).
    pub badge: Style,
    /// The app name in the header: the colour is reversed into a label.
    pub brand: Style,
}

const BRAND: Modifier = Modifier::REVERSED.union(Modifier::BOLD);

/// For dark backgrounds, and the fallback when the background is unknown: cyan accent, yellow
/// notes, red errors.
pub const DARK: Theme = Theme {
    accent: Style::new().fg(Color::Cyan),
    warning: Style::new().fg(Color::Yellow),
    error: Style::new().fg(Color::Red),
    badge: Style::new().fg(Color::Magenta),
    brand: Style::new().fg(Color::Cyan).add_modifier(BRAND),
};

/// For light backgrounds: blue accent, and magenta notes, since no yellow is readable on white.
/// Badges and notes share magenta, but badges only appear in search results, notes never do.
pub const LIGHT: Theme = Theme {
    accent: Style::new().fg(Color::Blue),
    warning: Style::new().fg(Color::Magenta),
    error: Style::new().fg(Color::Red),
    badge: Style::new().fg(Color::Magenta),
    brand: Style::new().fg(Color::Blue).add_modifier(BRAND),
};

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
    /// The theme for the config's choice and overrides.
    ///
    /// `NO_COLOR` only applies to `auto`, and then the overrides are dropped too: a theme set
    /// in the config wins over the environment, as no-color.org asks. `background` is only
    /// called for `auto`, since detecting it asks the terminal.
    #[must_use]
    pub fn resolve(
        choice: ThemeChoice,
        colors: &Colors,
        no_color: bool,
        background: impl FnOnce() -> Background,
    ) -> Self {
        let base = match choice {
            ThemeChoice::Auto if no_color => return MONO,
            ThemeChoice::Auto => match background() {
                Background::Dark => DARK,
                Background::Light => LIGHT,
            },
            ThemeChoice::Dark => DARK,
            ThemeChoice::Light => LIGHT,
            ThemeChoice::Mono => MONO,
        };
        base.with(colors)
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

    #[test]
    fn auto_follows_the_background() {
        let none = Colors::default();
        let auto = |bg| Theme::resolve(ThemeChoice::Auto, &none, false, move || bg);
        assert_eq!(auto(Background::Dark), DARK);
        assert_eq!(auto(Background::Light), LIGHT);
    }

    #[test]
    fn explicit_themes_dont_ask_the_terminal() {
        let none = Colors::default();
        assert_eq!(Theme::resolve(ThemeChoice::Dark, &none, false, never), DARK);
        assert_eq!(
            Theme::resolve(ThemeChoice::Light, &none, false, never),
            LIGHT
        );
        assert_eq!(Theme::resolve(ThemeChoice::Mono, &none, false, never), MONO);
    }

    #[test]
    fn no_color_only_overrides_auto() {
        let colors = Colors {
            accent: Some(Color::Green),
            ..Colors::default()
        };
        assert_eq!(
            Theme::resolve(ThemeChoice::Auto, &colors, true, never),
            MONO
        );
        let light = Theme::resolve(ThemeChoice::Light, &colors, true, never);
        assert_eq!(light.accent, Style::new().fg(Color::Green));
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
