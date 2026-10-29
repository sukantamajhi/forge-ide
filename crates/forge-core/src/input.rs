//! A single-line text field's state and key handling, for the find box, command
//! palette, rename box and similar inputs.

use forge_text::words::{next_word_end, prev_word_start};
use ropey::Rope;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LineInput {
    text: String,
    /// Caret as a char index.
    cursor: usize,
    /// When set, the whole text is selected and the next edit replaces it.
    all_selected: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// The text changed.
    Edited,
    /// Only the caret or selection moved.
    Moved,
    Submit,
    Cancel,
    /// Not a key this field handles; the caller may use it.
    Ignored,
}

impl LineInput {
    pub fn new(text: &str) -> Self {
        Self { text: text.to_owned(), cursor: text.chars().count(), all_selected: !text.is_empty() }
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn cursor(&self) -> usize {
        self.cursor
    }

    pub fn is_all_selected(&self) -> bool {
        self.all_selected
    }

    pub fn set_text(&mut self, text: &str) {
        self.text = text.to_owned();
        self.cursor = text.chars().count();
        self.all_selected = false;
    }

    fn byte_at(&self, char_index: usize) -> usize {
        self.text.char_indices().nth(char_index).map_or(self.text.len(), |(b, _)| b)
    }

    fn len(&self) -> usize {
        self.text.chars().count()
    }

    /// Insert at the caret, replacing everything if it is all selected.
    pub fn insert(&mut self, s: &str) {
        let s: String = s.chars().filter(|c| !matches!(c, '\n' | '\r')).collect();
        if self.all_selected {
            self.text.clear();
            self.cursor = 0;
            self.all_selected = false;
        }
        let at = self.byte_at(self.cursor);
        self.text.insert_str(at, &s);
        self.cursor += s.chars().count();
    }

    fn delete_range(&mut self, from: usize, to: usize) {
        let (a, b) = (self.byte_at(from), self.byte_at(to));
        self.text.replace_range(a..b, "");
        self.cursor = from;
    }

    /// Handle a key. `key` is GPUI's key name; `key_char` is the typed text, if any.
    pub fn apply(&mut self, key: &str, ctrl: bool, key_char: Option<&str>) -> Outcome {
        match (key, ctrl) {
            ("enter", _) => Outcome::Submit,
            ("escape", _) => Outcome::Cancel,
            ("a", true) => {
                self.all_selected = !self.text.is_empty();
                self.cursor = self.len();
                Outcome::Moved
            }
            ("left", _) | ("right", _) | ("home", _) | ("end", _) => {
                let len = self.len();
                let was_selected = std::mem::take(&mut self.all_selected);
                self.cursor = match (key, ctrl) {
                    ("left", true) => prev_word_start(&Rope::from_str(&self.text), self.cursor),
                    ("right", true) => next_word_end(&Rope::from_str(&self.text), self.cursor),
                    ("left", _) if was_selected => 0,
                    ("left", _) => self.cursor.saturating_sub(1),
                    ("right", _) if was_selected => len,
                    ("right", _) => (self.cursor + 1).min(len),
                    ("home", _) => 0,
                    _ => len,
                };
                Outcome::Moved
            }
            ("backspace", _) | ("delete", _) => {
                if self.all_selected {
                    self.text.clear();
                    self.cursor = 0;
                    self.all_selected = false;
                    return Outcome::Edited;
                }
                let (from, to) = match (key, ctrl) {
                    ("backspace", true) => (prev_word_start(&Rope::from_str(&self.text), self.cursor), self.cursor),
                    ("backspace", false) => (self.cursor.saturating_sub(1), self.cursor),
                    ("delete", true) => (self.cursor, next_word_end(&Rope::from_str(&self.text), self.cursor)),
                    _ => (self.cursor, (self.cursor + 1).min(self.len())),
                };
                if from == to {
                    return Outcome::Moved;
                }
                self.delete_range(from, to);
                Outcome::Edited
            }
            // On Windows Space is reported as a named key with no typed character.
            ("space", false) => {
                self.insert(" ");
                Outcome::Edited
            }
            _ => match key_char {
                Some(text) if !ctrl && !text.is_empty() && text.chars().all(|c| !c.is_control()) => {
                    self.insert(text);
                    Outcome::Edited
                }
                _ => Outcome::Ignored,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn typed(input: &mut LineInput, s: &str) {
        for c in s.chars() {
            input.apply("x", false, Some(&c.to_string()));
        }
    }

    #[test]
    fn typing_and_editing_at_the_caret() {
        let mut i = LineInput::default();
        typed(&mut i, "helo");
        i.apply("left", false, None);
        i.apply("x", false, Some("l"));
        assert_eq!((i.text(), i.cursor()), ("hello", 4));
        assert_eq!(i.apply("backspace", false, None), Outcome::Edited);
        assert_eq!(i.text(), "helo");
        i.apply("home", false, None);
        i.apply("delete", false, None);
        assert_eq!(i.text(), "elo");
        i.apply("end", false, None);
        assert_eq!(i.cursor(), 3);
    }

    #[test]
    fn select_all_is_replaced_by_the_next_edit() {
        let mut i = LineInput::new("old text");
        assert!(i.is_all_selected(), "prefilled text starts selected, like VS Code's find box");
        typed(&mut i, "n");
        assert_eq!(i.text(), "n");

        let mut j = LineInput::new("abc");
        j.apply("backspace", false, None);
        assert_eq!(j.text(), "");
        let mut k = LineInput::new("abc");
        k.apply("left", false, None);
        assert_eq!(k.cursor(), 0);
        assert!(!k.is_all_selected());
    }

    #[test]
    fn word_deletion_and_movement() {
        let mut i = LineInput::default();
        typed(&mut i, "foo bar");
        i.apply("backspace", true, None);
        assert_eq!(i.text(), "foo ");
        i.apply("left", true, None);
        assert_eq!(i.cursor(), 0);
        i.apply("right", true, None);
        assert_eq!(i.cursor(), 3);
    }

    #[test]
    fn space_types_a_space_even_without_a_typed_character() {
        // Windows reports Space as the key "space" with no key_char.
        let mut i = LineInput::default();
        typed(&mut i, "a");
        assert_eq!(i.apply("space", false, None), Outcome::Edited);
        typed(&mut i, "b");
        assert_eq!(i.text(), "a b");
        assert_eq!(i.apply("space", true, None), Outcome::Ignored, "Ctrl+Space is a command, not text");
    }

    #[test]
    fn enter_escape_and_unhandled_keys() {
        let mut i = LineInput::default();
        assert_eq!(i.apply("enter", false, None), Outcome::Submit);
        assert_eq!(i.apply("escape", false, None), Outcome::Cancel);
        assert_eq!(i.apply("f5", false, None), Outcome::Ignored);
        assert_eq!(i.apply("c", true, Some("c")), Outcome::Ignored, "ctrl chords are the caller's");
    }

    #[test]
    fn unicode_and_pasted_newlines() {
        let mut i = LineInput::default();
        i.insert("é😀\nx");
        assert_eq!((i.text(), i.cursor()), ("é😀x", 3));
        i.apply("backspace", false, None);
        i.apply("backspace", false, None);
        assert_eq!(i.text(), "é");
    }
}
