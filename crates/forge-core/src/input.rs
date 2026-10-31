//! A single-line text field's state and key handling, for the find box, command
//! palette, rename box and similar inputs.

use forge_text::words::{next_word_end, prev_word_start, word_range_at};
use ropey::Rope;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LineInput {
    text: String,
    /// Caret as a char index.
    cursor: usize,
    /// The other end of the selection, when there is one.
    anchor: Option<usize>,
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
    /// A field holding `text`, all selected (like VS Code's find box) so typing replaces it.
    pub fn new(text: &str) -> Self {
        let len = text.chars().count();
        Self { text: text.to_owned(), cursor: len, anchor: (len > 0).then_some(0) }
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn cursor(&self) -> usize {
        self.cursor
    }

    /// Selected range as `(start, end)` in chars, if any.
    pub fn selection(&self) -> Option<(usize, usize)> {
        let anchor = self.anchor.filter(|a| *a != self.cursor)?;
        Some((anchor.min(self.cursor), anchor.max(self.cursor)))
    }

    pub fn is_all_selected(&self) -> bool {
        !self.text.is_empty() && self.selection() == Some((0, self.len()))
    }

    pub fn selected_text(&self) -> Option<String> {
        let (a, b) = self.selection()?;
        Some(self.text[self.byte_at(a)..self.byte_at(b)].to_owned())
    }

    /// The same field with every char shown as `mask` (for secrets), keeping the caret and selection.
    pub fn masked(&self, mask: char) -> Self {
        Self { text: self.text.chars().map(|_| mask).collect(), cursor: self.cursor, anchor: self.anchor }
    }

    pub fn set_text(&mut self, text: &str) {
        self.text = text.to_owned();
        self.cursor = text.chars().count();
        self.anchor = None;
    }

    /// Put the caret at char `offset` (clamped), extending the selection when `extend`. This is what a click
    /// or a drag does, and Shift+arrows.
    pub fn set_cursor(&mut self, offset: usize, extend: bool) {
        let offset = offset.min(self.len());
        if extend {
            self.anchor.get_or_insert(self.cursor);
        } else {
            self.anchor = None;
        }
        self.cursor = offset;
    }

    /// Select the word at `offset` (a double click). On whitespace, just place the caret.
    pub fn select_word_at(&mut self, offset: usize) {
        let offset = offset.min(self.len());
        let (a, b) = word_range_at(&Rope::from_str(&self.text), offset).unwrap_or((offset, offset));
        self.anchor = Some(a);
        self.cursor = b;
    }

    pub fn select_all(&mut self) {
        self.anchor = Some(0);
        self.cursor = self.len();
    }

    /// Remove the selection and return its text, for Cut.
    pub fn cut(&mut self) -> Option<String> {
        let text = self.selected_text()?;
        self.delete_selection();
        Some(text)
    }

    fn byte_at(&self, char_index: usize) -> usize {
        self.text.char_indices().nth(char_index).map_or(self.text.len(), |(b, _)| b)
    }

    fn len(&self) -> usize {
        self.text.chars().count()
    }

    /// Delete the selected text, if any. True when something was deleted.
    fn delete_selection(&mut self) -> bool {
        let Some((a, b)) = self.selection() else {
            self.anchor = None;
            return false;
        };
        self.delete_range(a, b);
        self.anchor = None;
        true
    }

    /// Insert at the caret, replacing the selection if there is one.
    pub fn insert(&mut self, s: &str) {
        let s: String = s.chars().filter(|c| !matches!(c, '\n' | '\r')).collect();
        self.delete_selection();
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
        self.apply_with(key, ctrl, false, key_char)
    }

    /// Like [`apply`](Self::apply), with Shift: Shift+arrows, Home and End extend the selection.
    pub fn apply_with(&mut self, key: &str, ctrl: bool, shift: bool, key_char: Option<&str>) -> Outcome {
        match (key, ctrl) {
            ("enter", _) => Outcome::Submit,
            ("escape", _) => Outcome::Cancel,
            ("a", true) => {
                self.select_all();
                Outcome::Moved
            }
            ("left", _) | ("right", _) | ("home", _) | ("end", _) => {
                let len = self.len();
                let selection = if shift { None } else { self.selection() };
                let target = match (key, ctrl) {
                    ("left", true) => prev_word_start(&Rope::from_str(&self.text), self.cursor),
                    ("right", true) => next_word_end(&Rope::from_str(&self.text), self.cursor),
                    // Without Shift, an arrow collapses a selection to the edge it points at.
                    ("left", _) if selection.is_some() => selection.map_or(0, |s| s.0),
                    ("left", _) => self.cursor.saturating_sub(1),
                    ("right", _) if selection.is_some() => selection.map_or(len, |s| s.1),
                    ("right", _) => (self.cursor + 1).min(len),
                    ("home", _) => 0,
                    _ => len,
                };
                self.set_cursor(target, shift);
                Outcome::Moved
            }
            ("backspace", _) | ("delete", _) => {
                if self.delete_selection() {
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

    #[test]
    fn clicking_places_the_caret_between_characters() {
        let mut i = LineInput::new("hello");
        i.set_cursor(2, false);
        assert_eq!((i.cursor(), i.selection()), (2, None));
        typed(&mut i, "X");
        assert_eq!(i.text(), "heXllo");
        i.set_cursor(99, false);
        assert_eq!(i.cursor(), 6, "clamped to the end");
    }

    #[test]
    fn drag_and_shift_arrows_select_a_range() {
        let mut i = LineInput::new("hello world");
        i.set_cursor(2, false);
        i.set_cursor(7, true);
        assert_eq!(i.selection(), Some((2, 7)));
        assert_eq!(i.selected_text().as_deref(), Some("llo w"));
        typed(&mut i, "_");
        assert_eq!((i.text(), i.cursor()), ("he_orld", 3));

        let mut j = LineInput::default();
        typed(&mut j, "abc");
        j.apply_with("left", false, true, None);
        j.apply_with("left", false, true, None);
        assert_eq!(j.selection(), Some((1, 3)));
        j.apply_with("left", false, false, None);
        assert_eq!((j.cursor(), j.selection()), (1, None), "a plain arrow collapses to the left edge");
    }

    #[test]
    fn double_click_selects_a_word_and_cut_removes_it() {
        let mut i = LineInput::new("foo bar baz");
        i.select_word_at(5);
        assert_eq!(i.selected_text().as_deref(), Some("bar"));
        assert_eq!(i.cut().as_deref(), Some("bar"));
        assert_eq!(i.text(), "foo  baz");
        assert_eq!(i.cut(), None);
    }
}
