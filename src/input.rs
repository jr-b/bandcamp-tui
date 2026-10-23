//! Key bindings. Vim-style and arrow keys both work.

use ratatui::crossterm::event::{
    KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};

use crate::app::Action;
use crate::picker::PickerKind;
use crate::text_input::Edit;

/// One line of the key help, in display order: `(keys, what they do)`.
/// Ordered by importance: narrow terminals show only the first few.
pub const HINTS: &[(&str, &str)] = &[
    ("⏎", "play"),
    ("space", "pause"),
    ("tab", "pane"),
    ("q", "quit"),
    ("/", "search"),
    ("?", "help"),
    ("n/p", "track"),
    ("←→", "seek"),
    ("+/-", "volume"),
    ("f", "genre"),
    ("t", "time"),
    ("L", "place"),
    ("i", "info"),
    ("e", "queue"),
    ("v", "listen"),
    ("a", "autoplay"),
    ("s", "sort"),
    ("r", "refresh"),
    ("c", "playing"),
    ("C", "cover"),
    ("D", "density"),
    ("↑↓", "move"),
];

/// Hints shown while the queue popup is open.
pub const QUEUE_HINTS: &[(&str, &str)] = &[
    ("⏎", "play now"),
    ("d", "remove"),
    ("K/J", "move"),
    ("c", "clear"),
    ("esc", "close"),
];

/// Hints shown in the listening view.
pub const LISTENING_HINTS: &[(&str, &str)] = &[
    ("space", "pause"),
    ("v", "browse"),
    ("i", "info"),
    ("n/p", "track"),
    ("←→", "seek"),
    ("+/-", "volume"),
    ("u", "queue"),
    ("a", "autoplay"),
    ("o", "open"),
    ("?", "help"),
    ("q", "quit"),
];

/// Hints shown while the help overlay is open.
pub const HELP_HINTS: &[(&str, &str)] = &[("↑↓", "scroll"), ("esc", "close")];

/// Hints shown while a picker is open.
pub const PICKER_HINTS: &[(&str, &str)] = &[
    ("type", "to search"),
    ("↑↓", "move"),
    ("⏎", "select"),
    ("esc", "cancel"),
    ("ctrl-w", "delete word"),
    ("ctrl-u", "clear"),
];

/// Every binding, grouped, for the `?` help overlay.
pub const HELP: &[(&str, &[(&str, &str)])] = &[
    (
        "Browse",
        &[
            ("↑↓  j k", "move"),
            ("g G  Home End", "top / bottom"),
            ("PgUp PgDn  ^u ^d", "page"),
            ("tab  h l", "switch pane"),
            ("c", "jump to what's playing"),
            ("wheel", "scroll what's under the pointer"),
            ("click", "focus a pane, select a row"),
            ("i", "about / credits / artist bio"),
            ("C", "hide / show the cover"),
            ("D", "density: auto / compact / comfortable"),
            ("o", "open on bandcamp.com"),
        ],
    ),
    (
        "Play",
        &[
            ("⏎", "play album / play from track"),
            ("space", "pause / resume"),
            ("n p", "next / previous track"),
            ("← →  , .", "seek 10 s"),
            ("+ -", "volume"),
            ("a", "autoplay the next release"),
            ("v", "listening view (big cover)"),
        ],
    ),
    (
        "Queue",
        &[
            ("e", "queue release (track in tracklist)"),
            ("E", "play next"),
            ("u", "show the queue"),
            ("⏎ d K J c", "in the queue: play, remove, move, clear"),
        ],
    ),
    (
        "Discover",
        &[
            ("/", "search artists, albums, tracks"),
            ("esc", "back to Discover from search"),
            ("f", "genre or any tag"),
            ("t", "release time"),
            ("L", "location"),
            ("s", "best-selling / new / surprise me"),
            ("r", "refresh"),
        ],
    ),
    ("App", &[("?", "this help"), ("q  ^c", "quit")]),
];

/// Which bindings apply.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Normal,
    /// A filter picker or the search box is open: letters are text.
    Picker,
    /// The help overlay is open: movement keys scroll it, any other key closes it.
    Help,
    /// The queue popup is open.
    Queue,
    /// The listening view: transport keys only.
    Listening,
}

/// Maps a mouse event to an action: the wheel and left clicks are used.
#[must_use]
pub fn mouse_action(event: MouseEvent) -> Option<Action> {
    let down = match event.kind {
        MouseEventKind::ScrollDown => true,
        MouseEventKind::ScrollUp => false,
        MouseEventKind::Down(MouseButton::Left) => {
            return Some(Action::Click {
                column: event.column,
                row: event.row,
            });
        }
        _ => return None,
    };
    Some(Action::Wheel {
        down,
        column: event.column,
        row: event.row,
    })
}

/// Maps a key to an action for the current mode.
#[must_use]
pub fn action_for(key: KeyEvent, mode: Mode) -> Option<Action> {
    // Ignore key releases and repeats reported by terminals with the kitty keyboard protocol.
    if key.kind != KeyEventKind::Press {
        return None;
    }
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    match mode {
        Mode::Picker => return picker_action(key),
        Mode::Help => return Some(help_action(key)),
        Mode::Queue => return queue_action(key),
        Mode::Listening => return listening_action(key),
        Mode::Normal => {}
    }
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
        KeyCode::Char('C') => Action::ToggleCover,
        KeyCode::Char('D') => Action::CycleDensity,
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
        KeyCode::Char('o') => Action::OpenInBrowser,
        KeyCode::Char('c') => Action::JumpToPlaying,
        KeyCode::Char('a') => Action::ToggleAutoplay,
        KeyCode::Char('e') => Action::Enqueue,
        KeyCode::Char('E') => Action::EnqueueNext,
        KeyCode::Char('u') => Action::OpenQueue,
        KeyCode::Char('v') => Action::ToggleListening,
        KeyCode::Char('/') => Action::OpenSearch,
        KeyCode::Esc => Action::BackToDiscover,
        KeyCode::Char('?') => Action::ShowHelp,
        _ => return None,
    };
    Some(action)
}

/// The help overlay: movement keys scroll it, any other key closes it.
fn help_action(key: KeyEvent) -> Action {
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    match key.code {
        KeyCode::Char('c') if ctrl => Action::Quit,
        KeyCode::Char('d') if ctrl => Action::PageDown,
        KeyCode::Char('u') if ctrl => Action::PageUp,
        KeyCode::Char('k') | KeyCode::Up => Action::Up,
        KeyCode::Char('j') | KeyCode::Down => Action::Down,
        KeyCode::PageUp => Action::PageUp,
        KeyCode::PageDown => Action::PageDown,
        KeyCode::Char('g') | KeyCode::Home => Action::Top,
        KeyCode::Char('G') | KeyCode::End => Action::Bottom,
        _ => Action::CloseHelp,
    }
}

fn queue_action(key: KeyEvent) -> Option<Action> {
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    let action = match key.code {
        KeyCode::Char('c') if ctrl => Action::Quit,
        KeyCode::Char('k') | KeyCode::Up => Action::Up,
        KeyCode::Char('j') | KeyCode::Down => Action::Down,
        KeyCode::PageUp => Action::PageUp,
        KeyCode::PageDown => Action::PageDown,
        KeyCode::Char('g') | KeyCode::Home => Action::Top,
        KeyCode::Char('G') | KeyCode::End => Action::Bottom,
        KeyCode::Enter => Action::Play,
        KeyCode::Char('d' | 'x') | KeyCode::Delete | KeyCode::Backspace => Action::QueueRemove,
        KeyCode::Char('K') => Action::QueueMoveUp,
        KeyCode::Char('J') => Action::QueueMoveDown,
        KeyCode::Char('c') => Action::QueueClear,
        KeyCode::Esc | KeyCode::Char('u' | 'q') => Action::ClosePopup,
        // Transport keys keep working with the popup open.
        KeyCode::Char(' ') => Action::TogglePause,
        KeyCode::Char('n') => Action::NextTrack,
        KeyCode::Char('p') => Action::PreviousTrack,
        _ => return None,
    };
    Some(action)
}

fn listening_action(key: KeyEvent) -> Option<Action> {
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    let action = match key.code {
        KeyCode::Char('c') if ctrl => Action::Quit,
        KeyCode::Char('q') => Action::Quit,
        KeyCode::Char('v') | KeyCode::Esc => Action::ToggleListening,
        KeyCode::Char('i') => Action::ToggleInfo,
        // Scroll the info text.
        KeyCode::Char('d') if ctrl => Action::PageDown,
        KeyCode::Char('u') if ctrl => Action::PageUp,
        KeyCode::Char('k') | KeyCode::Up => Action::Up,
        KeyCode::Char('j') | KeyCode::Down => Action::Down,
        KeyCode::PageUp => Action::PageUp,
        KeyCode::PageDown => Action::PageDown,
        KeyCode::Char('g') | KeyCode::Home => Action::Top,
        KeyCode::Char('G') | KeyCode::End => Action::Bottom,
        KeyCode::Char(' ') => Action::TogglePause,
        KeyCode::Char('n') => Action::NextTrack,
        KeyCode::Char('p') => Action::PreviousTrack,
        KeyCode::Right | KeyCode::Char('.') => Action::SeekForward,
        KeyCode::Left | KeyCode::Char(',') => Action::SeekBackward,
        KeyCode::Char('+' | '=') => Action::VolumeUp,
        KeyCode::Char('-' | '_') => Action::VolumeDown,
        KeyCode::Char('a') => Action::ToggleAutoplay,
        KeyCode::Char('u') => Action::OpenQueue,
        KeyCode::Char('o') => Action::OpenInBrowser,
        KeyCode::Char('?') => Action::ShowHelp,
        _ => return None,
    };
    Some(action)
}

fn picker_action(key: KeyEvent) -> Option<Action> {
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    let alt = key.modifiers.contains(KeyModifiers::ALT);
    let action = match key.code {
        KeyCode::Char('c') if ctrl => Action::Quit,
        KeyCode::Char('u') if ctrl => Action::Edit(Edit::Clear),
        KeyCode::Char('w') if ctrl => Action::Edit(Edit::DeleteWord),
        KeyCode::Backspace if alt => Action::Edit(Edit::DeleteWord),
        KeyCode::Char('a') if ctrl => Action::Edit(Edit::Home),
        KeyCode::Char('e') if ctrl => Action::Edit(Edit::End),
        KeyCode::Char('p' | 'k') if ctrl => Action::Up,
        KeyCode::Char('n' | 'j') if ctrl => Action::Down,
        KeyCode::Esc => Action::ClosePopup,
        KeyCode::Enter => Action::Play,
        KeyCode::Up => Action::Up,
        KeyCode::Down => Action::Down,
        KeyCode::PageUp => Action::PageUp,
        KeyCode::PageDown => Action::PageDown,
        KeyCode::Left => Action::Edit(Edit::Left),
        KeyCode::Right => Action::Edit(Edit::Right),
        KeyCode::Home => Action::Edit(Edit::Home),
        KeyCode::End => Action::Edit(Edit::End),
        KeyCode::Backspace => Action::Edit(Edit::Backspace),
        KeyCode::Delete => Action::Edit(Edit::Delete),
        KeyCode::Char(c) if !ctrl => Action::Edit(Edit::Insert(c)),
        _ => return None,
    };
    Some(action)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn press(code: KeyCode) -> Option<Action> {
        action_for(KeyEvent::new(code, KeyModifiers::NONE), Mode::Normal)
    }

    fn press_in_picker(code: KeyCode) -> Option<Action> {
        action_for(KeyEvent::new(code, KeyModifiers::NONE), Mode::Picker)
    }

    #[test]
    fn picker_takes_letters_as_text() {
        assert_eq!(
            press_in_picker(KeyCode::Char('q')),
            Some(Action::Edit(Edit::Insert('q')))
        );
        assert_eq!(
            press_in_picker(KeyCode::Char('j')),
            Some(Action::Edit(Edit::Insert('j')))
        );
        assert_eq!(press_in_picker(KeyCode::Esc), Some(Action::ClosePopup));
        assert_eq!(press_in_picker(KeyCode::Enter), Some(Action::Play));
        assert_eq!(
            action_for(
                KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL),
                Mode::Picker
            ),
            Some(Action::Quit)
        );
        // Esc never quits: outside popups it goes back to Discover.
        assert_eq!(press(KeyCode::Esc), Some(Action::BackToDiscover));
    }

    #[test]
    fn picker_text_editing_keys() {
        let key = |code, modifiers| action_for(KeyEvent::new(code, modifiers), Mode::Picker);
        let edit = |e| Some(Action::Edit(e));
        assert_eq!(key(KeyCode::Left, KeyModifiers::NONE), edit(Edit::Left));
        assert_eq!(key(KeyCode::End, KeyModifiers::NONE), edit(Edit::End));
        assert_eq!(key(KeyCode::Delete, KeyModifiers::NONE), edit(Edit::Delete));
        assert_eq!(
            key(KeyCode::Char('w'), KeyModifiers::CONTROL),
            edit(Edit::DeleteWord)
        );
        assert_eq!(
            key(KeyCode::Backspace, KeyModifiers::ALT),
            edit(Edit::DeleteWord)
        );
        assert_eq!(
            key(KeyCode::Char('a'), KeyModifiers::CONTROL),
            edit(Edit::Home)
        );
        assert_eq!(
            key(KeyCode::Char('u'), KeyModifiers::CONTROL),
            edit(Edit::Clear)
        );
    }

    #[test]
    fn the_wheel_and_left_clicks_are_used() {
        let mouse = |kind| MouseEvent {
            kind,
            column: 3,
            row: 4,
            modifiers: KeyModifiers::NONE,
        };
        assert_eq!(
            mouse_action(mouse(MouseEventKind::ScrollDown)),
            Some(Action::Wheel {
                down: true,
                column: 3,
                row: 4
            })
        );
        assert!(matches!(
            mouse_action(mouse(MouseEventKind::ScrollUp)),
            Some(Action::Wheel { down: false, .. })
        ));
        assert_eq!(
            mouse_action(mouse(MouseEventKind::Down(MouseButton::Left))),
            Some(Action::Click { column: 3, row: 4 })
        );
        assert_eq!(
            mouse_action(mouse(MouseEventKind::Down(MouseButton::Right))),
            None
        );
        assert_eq!(mouse_action(mouse(MouseEventKind::Moved)), None);
    }

    #[test]
    fn vim_and_arrow_keys_agree() {
        assert_eq!(press(KeyCode::Char('j')), press(KeyCode::Down));
        assert_eq!(press(KeyCode::Char('k')), press(KeyCode::Up));
        assert_eq!(press(KeyCode::Char('G')), press(KeyCode::End));
    }

    #[test]
    fn ctrl_c_quits_and_c_jumps_to_playing() {
        assert_eq!(
            action_for(
                KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL),
                Mode::Normal
            ),
            Some(Action::Quit)
        );
        assert_eq!(press(KeyCode::Char('c')), Some(Action::JumpToPlaying));
    }

    #[test]
    fn help_scrolls_and_closes_on_any_other_key() {
        let help = |code| action_for(KeyEvent::new(code, KeyModifiers::NONE), Mode::Help);
        assert_eq!(help(KeyCode::Char('q')), Some(Action::CloseHelp));
        assert_eq!(help(KeyCode::Esc), Some(Action::CloseHelp));
        assert_eq!(help(KeyCode::Char('?')), Some(Action::CloseHelp));
        assert_eq!(help(KeyCode::Char('j')), Some(Action::Down));
        assert_eq!(help(KeyCode::Up), Some(Action::Up));
        assert_eq!(help(KeyCode::PageDown), Some(Action::PageDown));
        assert_eq!(press(KeyCode::Char('?')), Some(Action::ShowHelp));
    }

    #[test]
    fn help_lists_every_hint_key() {
        // Compare without spaces: the hint bar writes `←→`, the help `← →`.
        let help_keys: String = HELP
            .iter()
            .flat_map(|(_, keys)| keys.iter().map(|(k, _)| *k))
            .collect::<String>()
            .replace(' ', "");
        for (key, _) in HINTS {
            let first = key.split('/').next().unwrap_or(key).replace(' ', "");
            assert!(help_keys.contains(&first), "{key} missing from help");
        }
    }

    #[test]
    fn queue_popup_keys() {
        let queue = |code| action_for(KeyEvent::new(code, KeyModifiers::NONE), Mode::Queue);
        assert_eq!(queue(KeyCode::Char('d')), Some(Action::QueueRemove));
        assert_eq!(queue(KeyCode::Char('J')), Some(Action::QueueMoveDown));
        assert_eq!(
            queue(KeyCode::Char('q')),
            Some(Action::ClosePopup),
            "q closes, not quits"
        );
        assert_eq!(queue(KeyCode::Char(' ')), Some(Action::TogglePause));
        assert_eq!(press(KeyCode::Char('e')), Some(Action::Enqueue));
        assert_eq!(press(KeyCode::Char('E')), Some(Action::EnqueueNext));
        assert_eq!(press(KeyCode::Char('u')), Some(Action::OpenQueue));
    }

    #[test]
    fn listening_view_keys() {
        let listen = |code| action_for(KeyEvent::new(code, KeyModifiers::NONE), Mode::Listening);
        assert_eq!(press(KeyCode::Char('v')), Some(Action::ToggleListening));
        assert_eq!(listen(KeyCode::Char('v')), Some(Action::ToggleListening));
        assert_eq!(listen(KeyCode::Esc), Some(Action::ToggleListening));
        assert_eq!(listen(KeyCode::Char(' ')), Some(Action::TogglePause));
        assert_eq!(listen(KeyCode::Right), Some(Action::SeekForward));
        assert_eq!(listen(KeyCode::Char('u')), Some(Action::OpenQueue));
        assert_eq!(listen(KeyCode::Char('q')), Some(Action::Quit));
        assert_eq!(listen(KeyCode::Char('i')), Some(Action::ToggleInfo));
        assert_eq!(
            listen(KeyCode::Char('j')),
            Some(Action::Down),
            "scrolls the info"
        );
        // Browsing keys do nothing: there's no list here.
        assert_eq!(listen(KeyCode::Enter), None);
        assert_eq!(listen(KeyCode::Char('f')), None);
        assert_eq!(listen(KeyCode::Char('/')), None);
    }

    #[test]
    fn releases_are_ignored() {
        let mut key = KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE);
        key.kind = KeyEventKind::Release;
        assert_eq!(action_for(key, Mode::Normal), None);
    }
}
