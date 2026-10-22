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

use std::cell::Cell;

use ratatui::style::{Color, Modifier, Style};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Theme {
    /// Focused borders, markers, keys in hints, progress, links.
    pub accent: Style,
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
    warning: Style::new(),
    error: Style::new().add_modifier(Modifier::BOLD),
    badge: Style::new(),
    brand: Style::new().add_modifier(Modifier::REVERSED.union(Modifier::BOLD)),
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
