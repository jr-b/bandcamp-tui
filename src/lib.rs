//! bandcamp-tui: browse Bandcamp Discover and stream full albums from the terminal.
//!
//! The library half holds everything that can be tested without a terminal; `main.rs` wires it
//! into the TUI.

pub mod app;
pub mod bandcamp;
pub mod format;
pub mod fuzzy;
pub mod input;
pub mod picker;
pub mod player;
pub mod run;
pub mod settings;
pub mod ui;
