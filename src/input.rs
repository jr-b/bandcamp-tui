//! Key bindings. Vim-style and arrow keys both work.

use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

use crate::app::Action;

/// One line of the key help, in display order: `(keys, what they do)`.
pub const HINTS: &[(&str, &str)] = &[
    ("↑↓", "move"),
    ("⏎", "play"),
    ("space", "pause"),
    ("n/p", "track"),
    ("←→", "seek"),
    ("+/-", "volume"),
    ("s", "sort"),
    ("r", "refresh"),
    ("q", "quit"),
];

#[must_use]
pub fn action_for(key: KeyEvent) -> Option<Action> {
    // Ignore key releases and repeats reported by terminals with the kitty keyboard protocol.
    if key.kind != KeyEventKind::Press {
        return None;
    }
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    let action = match key.code {
        KeyCode::Char('c') if ctrl => Action::Quit,
        KeyCode::Char('d') if ctrl => Action::PageDown,
        KeyCode::Char('u') if ctrl => Action::PageUp,
        KeyCode::Char('q') => Action::Quit,
        KeyCode::Char('k') | KeyCode::Up => Action::Up,
        KeyCode::Char('j') | KeyCode::Down => Action::Down,
        KeyCode::PageUp => Action::PageUp,
        KeyCode::PageDown => Action::PageDown,
        KeyCode::Char('g') | KeyCode::Home => Action::Top,
        KeyCode::Char('G') | KeyCode::End => Action::Bottom,
        KeyCode::Enter => Action::PlaySelected,
        KeyCode::Char(' ') => Action::TogglePause,
        KeyCode::Char('n') => Action::NextTrack,
        KeyCode::Char('p') => Action::PreviousTrack,
        KeyCode::Right | KeyCode::Char('.') => Action::SeekForward,
        KeyCode::Left | KeyCode::Char(',') => Action::SeekBackward,
        KeyCode::Char('+' | '=') => Action::VolumeUp,
        KeyCode::Char('-' | '_') => Action::VolumeDown,
        KeyCode::Char('r') => Action::Refresh,
        KeyCode::Char('s') => Action::CycleSlice,
        _ => return None,
    };
    Some(action)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn press(code: KeyCode) -> Option<Action> {
        action_for(KeyEvent::new(code, KeyModifiers::NONE))
    }

    #[test]
    fn vim_and_arrow_keys_agree() {
        assert_eq!(press(KeyCode::Char('j')), press(KeyCode::Down));
        assert_eq!(press(KeyCode::Char('k')), press(KeyCode::Up));
        assert_eq!(press(KeyCode::Char('G')), press(KeyCode::End));
    }

    #[test]
    fn ctrl_c_quits_but_c_does_nothing() {
        assert_eq!(
            action_for(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL)),
            Some(Action::Quit)
        );
        assert_eq!(press(KeyCode::Char('c')), None);
    }

    #[test]
    fn releases_are_ignored() {
        let mut key = KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE);
        key.kind = KeyEventKind::Release;
        assert_eq!(action_for(key), None);
    }
}
