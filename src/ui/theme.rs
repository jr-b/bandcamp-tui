//! Semantic styles. Rendering asks for a role ("accent", "error") rather than a colour, so a
//! monochrome theme can honour `NO_COLOR` (<https://no-color.org>) everywhere at once.
//!
//! Nothing relies on colour alone: playing rows have a ♪, states are spelled out, the selection
//! is reversed. The monochrome theme only drops the colours.

use std::cell::Cell;

use ratatui::style::{Color, Modifier, Style};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Theme {
    /// Focused borders, markers, keys in hints, progress, links.
    pub accent: Style,
    /// Secondary text next to primary text, e.g. the artist beside a title.
    pub secondary: Style,
    /// Something to note but not a failure: pre-orders, buffering, paused.
    pub warning: Style,
    pub error: Style,
    /// The kind badge of artists and labels in search results (releases use `accent`).
    pub badge: Style,
    /// The app name in the header.
    pub brand: Style,
}

/// The default: cyan accent, yellow notes, red errors.
pub const COLOR: Theme = Theme {
    accent: Style::new().fg(Color::Cyan),
    // Dim rather than `Color::Gray`, which is near-invisible on light backgrounds.
    secondary: Style::new().add_modifier(Modifier::DIM),
    warning: Style::new().fg(Color::Yellow),
    error: Style::new().fg(Color::Red),
    badge: Style::new().fg(Color::Magenta),
    brand: Style::new()
        .fg(Color::Black)
        .bg(Color::Cyan)
        .add_modifier(Modifier::BOLD),
};

/// For `NO_COLOR`: only bold, dim and reverse.
pub const MONO: Theme = Theme {
    accent: Style::new(),
    secondary: Style::new().add_modifier(Modifier::DIM),
    warning: Style::new(),
    error: Style::new().add_modifier(Modifier::BOLD),
    badge: Style::new(),
    brand: Style::new().add_modifier(Modifier::REVERSED.union(Modifier::BOLD)),
};

impl Theme {
    /// [`MONO`] if `NO_COLOR` is set to anything non-empty, else [`COLOR`].
    #[must_use]
    pub fn from_env() -> &'static Self {
        let no_color = std::env::var_os("NO_COLOR").is_some_and(|v| !v.is_empty());
        if no_color { &MONO } else { &COLOR }
    }
}

thread_local! {
    // Per thread so tests can render both themes side by side; the app renders on one thread.
    static CURRENT: Cell<&'static Theme> = Cell::new(Theme::from_env());
}

/// The theme rendering uses.
#[must_use]
pub fn theme() -> &'static Theme {
    CURRENT.with(Cell::get)
}

/// Switches the theme for this thread (used by tests).
pub fn set(theme: &'static Theme) {
    CURRENT.with(|current| current.set(theme));
}
