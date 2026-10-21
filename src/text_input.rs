//! A one-line text field with a cursor, for the search box and the filter pickers.

use unicode_width::UnicodeWidthStr;

/// An edit to a [`TextInput`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Edit {
    Insert(char),
    /// Delete the character before the cursor.
    Backspace,
    /// Delete the character under the cursor.
    Delete,
    /// Delete the word before the cursor (`ctrl-w`).
    DeleteWord,
    /// Delete everything (`ctrl-u`).
    Clear,
    Left,
    Right,
    Home,
    End,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TextInput {
    text: String,
    /// Byte offset into `text`, always on a char boundary.
    cursor: usize,
}

impl TextInput {
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.text
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.text.is_empty()
    }

    /// Applies `edit`; returns whether the text changed (moving the cursor doesn't).
    pub fn apply(&mut self, edit: Edit) -> bool {
        match edit {
            Edit::Insert(c) => {
                self.text.insert(self.cursor, c);
                self.cursor += c.len_utf8();
                true
            }
            Edit::Backspace => {
                let Some(start) = self.prev_boundary() else {
                    return false;
                };
                self.text.drain(start..self.cursor);
                self.cursor = start;
                true
            }
            Edit::Delete => {
                let Some(end) = self.next_boundary() else {
                    return false;
                };
                self.text.drain(self.cursor..end);
                true
            }
            Edit::DeleteWord => {
                let before = &self.text[..self.cursor];
                let start = before
                    .trim_end()
                    .rfind(char::is_whitespace)
                    .map_or(0, |i| i + 1);
                if start == self.cursor {
                    return false;
                }
                self.text.drain(start..self.cursor);
                self.cursor = start;
                true
            }
            Edit::Clear => {
                self.cursor = 0;
                let changed = !self.text.is_empty();
                self.text.clear();
                changed
            }
            Edit::Left => {
                self.cursor = self.prev_boundary().unwrap_or(self.cursor);
                false
            }
            Edit::Right => {
                self.cursor = self.next_boundary().unwrap_or(self.cursor);
                false
            }
            Edit::Home => {
                self.cursor = 0;
                false
            }
            Edit::End => {
                self.cursor = self.text.len();
                false
            }
        }
    }

    fn prev_boundary(&self) -> Option<usize> {
        self.text[..self.cursor]
            .char_indices()
            .next_back()
            .map(|(i, _)| i)
    }

    fn next_boundary(&self) -> Option<usize> {
        self.text[self.cursor..]
            .chars()
            .next()
            .map(|c| self.cursor + c.len_utf8())
    }

    /// The part of the text to show in `width` cells so the cursor stays visible, and the
    /// cursor's column within it. Long text scrolls to keep the cursor in view.
    #[must_use]
    pub fn window(&self, width: usize) -> (&str, usize) {
        let width = width.max(1);
        // Start late enough that the cursor (plus one cell for it) fits.
        let mut start = 0;
        while self.text[start..self.cursor].width() >= width {
            start += self.text[start..].chars().next().map_or(1, char::len_utf8);
        }
        let mut end = self.text.len();
        while self.text[start..end].width() > width {
            end = self.text[..end]
                .char_indices()
                .next_back()
                .map_or(start, |(i, _)| i);
        }
        (
            &self.text[start..end],
            self.text[start..self.cursor].width(),
        )
    }
}

impl From<&str> for TextInput {
    fn from(text: &str) -> Self {
        Self {
            text: text.to_owned(),
            cursor: text.len(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn typed(text: &str) -> TextInput {
        let mut input = TextInput::default();
        for c in text.chars() {
            input.apply(Edit::Insert(c));
        }
        input
    }

    #[test]
    fn edits_at_the_cursor() {
        let mut input = typed("aphx");
        input.apply(Edit::Left);
        assert!(input.apply(Edit::Insert('e')));
        assert_eq!(input.as_str(), "aphex");
        input.apply(Edit::Home);
        assert!(input.apply(Edit::Delete));
        assert_eq!(input.as_str(), "phex");
        assert!(!input.apply(Edit::Backspace), "nothing before the cursor");
        input.apply(Edit::End);
        assert!(!input.apply(Edit::Delete), "nothing under the cursor");
        assert!(input.apply(Edit::Backspace));
        assert_eq!(input.as_str(), "phe");
    }

    #[test]
    fn handles_multibyte_characters() {
        let mut input = typed("björk");
        input.apply(Edit::Left);
        input.apply(Edit::Left);
        input.apply(Edit::Backspace);
        assert_eq!(input.as_str(), "bjrk");
        input.apply(Edit::Insert('ö'));
        assert_eq!(input.as_str(), "björk");
    }

    #[test]
    fn deletes_the_previous_word() {
        let mut input = typed("aphex twin  ");
        assert!(input.apply(Edit::DeleteWord));
        assert_eq!(input.as_str(), "aphex ");
        assert!(input.apply(Edit::DeleteWord));
        assert_eq!(input.as_str(), "");
        assert!(!input.apply(Edit::DeleteWord));
        let mut input = typed("one two");
        assert!(input.apply(Edit::Clear));
        assert_eq!((input.as_str(), input.window(10).1), ("", 0));
    }

    #[test]
    fn window_keeps_the_cursor_visible() {
        let input = typed("0123456789");
        assert_eq!(input.window(20), ("0123456789", 10));
        // Cursor at the end: the start scrolls off, with a cell left for the cursor.
        assert_eq!(input.window(5), ("6789", 4));
        let mut input = input;
        input.apply(Edit::Home);
        assert_eq!(input.window(5), ("01234", 0));
        // Wide characters are measured in cells.
        let input = typed("日本語");
        assert_eq!(input.window(5), ("本語", 4));
    }
}
