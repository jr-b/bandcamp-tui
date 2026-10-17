//! Key bindings. Vim-style and arrow keys both work.

use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

use crate::app::Action;
use crate::picker::PickerKind;

/// One line of the key help, in display order: `(keys, what they do)`.
/// Ordered by importance: narrow terminals show only the first few.
pub const HINTS: &[(&str, &str)] = &[
    ("⏎", "play"),
    ("space", "pause"),
    ("tab", "pane"),
    ("q", "quit"),
    ("n/p", "track"),
    ("←→", "seek"),
    ("+/-", "volume"),
    ("f", "genre"),
    ("t", "time"),
    ("L", "place"),
    ("i", "info"),
    ("s", "sort"),
    ("r", "refresh"),
    ("↑↓", "move"),
];

/// Hints shown while a picker is open.
pub const PICKER_HINTS: &[(&str, &str)] = &[
    ("type", "to search"),
    ("↑↓", "move"),
    ("⏎", "select"),
    ("esc", "cancel"),
    ("ctrl-u", "clear"),
];

/// Maps a key to an action. `picker_open` switches to text-entry bindings.
#[must_use]
pub fn action_for(key: KeyEvent, picker_open: bool) -> Option<Action> {
    // Ignore key releases and repeats reported by terminals with the kitty keyboard protocol.
    if key.kind != KeyEventKind::Press {
        return None;
    }
    if picker_open {
        return picker_action(key);
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
        KeyCode::Enter => Action::Play,
        KeyCode::Tab | KeyCode::BackTab => Action::ToggleFocus,
        KeyCode::Char('h') => Action::FocusDiscover,
        KeyCode::Char('l') => Action::FocusAlbum,
        KeyCode::Char('i') => Action::ToggleInfo,
        KeyCode::Char(' ') => Action::TogglePause,
        KeyCode::Char('n') => Action::NextTrack,
        KeyCode::Char('p') => Action::PreviousTrack,
        KeyCode::Right | KeyCode::Char('.') => Action::SeekForward,
        KeyCode::Left | KeyCode::Char(',') => Action::SeekBackward,
        KeyCode::Char('+' | '=') => Action::VolumeUp,
        KeyCode::Char('-' | '_') => Action::VolumeDown,
        KeyCode::Char('r') => Action::Refresh,
        KeyCode::Char('s') => Action::CycleSlice,
        KeyCode::Char('f') => Action::OpenPicker(PickerKind::Genre),
        KeyCode::Char('t') => Action::OpenPicker(PickerKind::Time),
        KeyCode::Char('L') => Action::OpenPicker(PickerKind::Location),
        _ => return None,
    };
    Some(action)
}

fn picker_action(key: KeyEvent) -> Option<Action> {
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    let action = match key.code {
        KeyCode::Char('c') if ctrl => Action::Quit,
        KeyCode::Char('u') if ctrl => Action::PickerClear,
        KeyCode::Char('p' | 'k') if ctrl => Action::Up,
        KeyCode::Char('n' | 'j') if ctrl => Action::Down,
        KeyCode::Esc => Action::ClosePicker,
        KeyCode::Enter => Action::Play,
        KeyCode::Up => Action::Up,
        KeyCode::Down => Action::Down,
        KeyCode::PageUp => Action::PageUp,
        KeyCode::PageDown => Action::PageDown,
        KeyCode::Backspace => Action::PickerBackspace,
        KeyCode::Char(c) if !ctrl => Action::PickerInput(c),
        _ => return None,
    };
    Some(action)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn press(code: KeyCode) -> Option<Action> {
        action_for(KeyEvent::new(code, KeyModifiers::NONE), false)
    }

    fn press_in_picker(code: KeyCode) -> Option<Action> {
        action_for(KeyEvent::new(code, KeyModifiers::NONE), true)
    }

    #[test]
    fn picker_takes_letters_as_text() {
        assert_eq!(
            press_in_picker(KeyCode::Char('q')),
            Some(Action::PickerInput('q'))
        );
        assert_eq!(
            press_in_picker(KeyCode::Char('j')),
            Some(Action::PickerInput('j'))
        );
        assert_eq!(press_in_picker(KeyCode::Esc), Some(Action::ClosePicker));
        assert_eq!(press_in_picker(KeyCode::Enter), Some(Action::Play));
        assert_eq!(
            action_for(
                KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL),
                true
            ),
            Some(Action::Quit)
        );
        // Esc does nothing outside the picker (it doesn't quit).
        assert_eq!(press(KeyCode::Esc), None);
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
            action_for(
                KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL),
                false
            ),
            Some(Action::Quit)
        );
        assert_eq!(press(KeyCode::Char('c')), None);
    }

    #[test]
    fn releases_are_ignored() {
        let mut key = KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE);
        key.kind = KeyEventKind::Release;
        assert_eq!(action_for(key, false), None);
    }
}
