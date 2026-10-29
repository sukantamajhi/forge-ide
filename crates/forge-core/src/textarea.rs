//! A small multi-line text box with soft wrapping, for the commit message. State and key
//! handling only: caret and selection in chars, wrapping at a fixed column count (the editor font
//! is monospace), simple undo. Drawing and the clipboard are the caller's.

use forge_text::words::{next_word_end, prev_word_start, word_range_at};
use ropey::Rope;

use crate::input::Outcome;

/// One displayed row: a char range `start..end` of the text (a wrapped piece of a line, or a
/// whole short one). Rows never include the `\n` that ends their line.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Row {
    pub start: usize,
    pub end: usize,
}

impl Row {
    pub fn len(&self) -> usize {
        self.end - self.start
    }

    pub fn is_empty(&self) -> bool {
        self.start == self.end
    }
}

const UNDO_LIMIT: usize = 100;

#[derive(Clone, Debug)]
pub struct TextArea {
    text: String,
    /// Caret as a char index.
    cursor: usize,
    /// The other end of the selection, when there is one.
    anchor: Option<usize>,
    /// Column kept while moving up and down through rows of different lengths.
    goal: Option<usize>,
    /// Characters per displayed row.
    wrap: usize,
    undo: Vec<(String, usize)>,
    redo: Vec<(String, usize)>,
}

impl Default for TextArea {
    fn default() -> Self {
        Self { text: String::new(), cursor: 0, anchor: None, goal: None, wrap: usize::MAX / 4, undo: Vec::new(), redo: Vec::new() }
    }
}

impl TextArea {
    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn is_empty(&self) -> bool {
        self.text.is_empty()
    }

    pub fn len_chars(&self) -> usize {
        self.text.chars().count()
    }

    pub fn cursor(&self) -> usize {
        self.cursor
    }

    /// Replace everything; the caret goes to the end and the undo history starts over.
    pub fn set_text(&mut self, text: &str) {
        self.text = text.replace("\r\n", "\n").replace('\r', "\n");
        self.cursor = self.len_chars();
        self.anchor = None;
        self.goal = None;
        self.undo.clear();
        self.redo.clear();
    }

    /// How many characters fit on a displayed row (at least 1).
    pub fn set_wrap(&mut self, columns: usize) {
        self.wrap = columns.max(1);
    }

    fn byte_at(&self, char_index: usize) -> usize {
        self.text.char_indices().nth(char_index).map_or(self.text.len(), |(b, _)| b)
    }

    /// The displayed rows, top to bottom. Always at least one (empty text is one empty row).
    pub fn rows(&self) -> Vec<Row> {
        let chars: Vec<char> = self.text.chars().collect();
        let mut rows = Vec::new();
        let mut start = 0;
        loop {
            let mut end = start;
            while end < chars.len() && chars[end] != '\n' {
                end += 1;
            }
            if end == start {
                rows.push(Row { start, end });
            } else {
                let mut piece = start;
                while piece < end {
                    let stop = piece.saturating_add(self.wrap).min(end);
                    rows.push(Row { start: piece, end: stop });
                    piece = stop;
                }
            }
            if end >= chars.len() {
                break;
            }
            start = end + 1;
        }
        rows
    }

    fn row_of(rows: &[Row], offset: usize) -> usize {
        rows.iter().rposition(|r| r.start <= offset).unwrap_or(0)
    }

    /// The caret's row and column within that row.
    pub fn cursor_row_col(&self) -> (usize, usize) {
        let rows = self.rows();
        let row = Self::row_of(&rows, self.cursor);
        (row, self.cursor - rows[row].start)
    }

    /// The char offset for column `col` of displayed row `row` (both clamped).
    pub fn offset_at(&self, row: usize, col: usize) -> usize {
        let rows = self.rows();
        let r = rows[row.min(rows.len() - 1)];
        r.start + col.min(r.len())
    }

    /// Selected range as `(start, end)` in chars, if any.
    pub fn selection(&self) -> Option<(usize, usize)> {
        let anchor = self.anchor.filter(|a| *a != self.cursor)?;
        Some((anchor.min(self.cursor), anchor.max(self.cursor)))
    }

    pub fn selected_text(&self) -> Option<String> {
        let (a, b) = self.selection()?;
        Some(self.text[self.byte_at(a)..self.byte_at(b)].to_owned())
    }

    /// Put the caret at `offset`, extending the selection when `extend`.
    pub fn set_cursor(&mut self, offset: usize, extend: bool) {
        let offset = offset.min(self.len_chars());
        if extend {
            self.anchor.get_or_insert(self.cursor);
        } else {
            self.anchor = None;
        }
        self.cursor = offset;
        self.goal = None;
    }

    /// Select the word at `offset` (a double click). On whitespace, just place the caret.
    pub fn select_word_at(&mut self, offset: usize) {
        let offset = offset.min(self.len_chars());
        let (a, b) = word_range_at(&Rope::from_str(&self.text), offset).unwrap_or((offset, offset));
        self.anchor = Some(a);
        self.cursor = b;
        self.goal = None;
    }

    pub fn select_all(&mut self) {
        self.anchor = Some(0);
        self.cursor = self.len_chars();
        self.goal = None;
    }

    fn snapshot(&mut self) {
        self.undo.push((self.text.clone(), self.cursor));
        if self.undo.len() > UNDO_LIMIT {
            self.undo.remove(0);
        }
        self.redo.clear();
    }

    /// Remove the selection. True if there was one.
    pub fn delete_selection(&mut self) -> bool {
        let Some((a, b)) = self.selection() else {
            self.anchor = None;
            return false;
        };
        self.snapshot();
        let (ba, bb) = (self.byte_at(a), self.byte_at(b));
        self.text.replace_range(ba..bb, "");
        self.cursor = a;
        self.anchor = None;
        self.goal = None;
        true
    }

    /// Type or paste `s` at the caret, replacing the selection. Line breaks are kept.
    pub fn insert(&mut self, s: &str) {
        let s = s.replace("\r\n", "\n").replace('\r', "\n");
        if s.is_empty() {
            return;
        }
        let had_selection = self.selection().is_some();
        if had_selection {
            self.delete_selection();
        } else {
            self.snapshot();
        }
        let at = self.byte_at(self.cursor);
        self.text.insert_str(at, &s);
        self.cursor += s.chars().count();
        self.anchor = None;
        self.goal = None;
    }

    fn delete_range(&mut self, from: usize, to: usize) {
        self.snapshot();
        let (a, b) = (self.byte_at(from), self.byte_at(to));
        self.text.replace_range(a..b, "");
        self.cursor = from;
        self.anchor = None;
        self.goal = None;
    }

    fn undo_step(&mut self, redo: bool) -> Outcome {
        let from = if redo { &mut self.redo } else { &mut self.undo };
        let Some((text, cursor)) = from.pop() else { return Outcome::Moved };
        let current = (std::mem::replace(&mut self.text, text), self.cursor);
        if redo { self.undo.push(current) } else { self.redo.push(current) };
        self.cursor = cursor.min(self.len_chars());
        self.anchor = None;
        self.goal = None;
        Outcome::Edited
    }

    /// Handle a key. `key` is GPUI's key name; `key_char` is the typed text, if any. Ctrl+Enter is
    /// `Submit`; Ctrl+C, X and V are left to the caller (`Ignored`).
    pub fn apply(&mut self, key: &str, ctrl: bool, shift: bool, key_char: Option<&str>) -> Outcome {
        match (key, ctrl) {
            ("enter", true) => return Outcome::Submit,
            ("enter", false) => {
                self.insert("\n");
                return Outcome::Edited;
            }
            ("escape", _) => return Outcome::Cancel,
            ("a", true) => {
                self.select_all();
                return Outcome::Moved;
            }
            ("z", true) => return self.undo_step(false),
            ("y", true) => return self.undo_step(true),
            ("left" | "right" | "up" | "down" | "home" | "end", _) => return self.move_caret(key, ctrl, shift),
            ("backspace" | "delete", _) => {
                if self.delete_selection() {
                    return Outcome::Edited;
                }
                let rope = Rope::from_str(&self.text);
                let len = self.len_chars();
                let (from, to) = match (key, ctrl) {
                    ("backspace", true) => (prev_word_start(&rope, self.cursor), self.cursor),
                    ("backspace", false) => (self.cursor.saturating_sub(1), self.cursor),
                    ("delete", true) => (self.cursor, next_word_end(&rope, self.cursor)),
                    _ => (self.cursor, (self.cursor + 1).min(len)),
                };
                if from == to {
                    return Outcome::Moved;
                }
                self.delete_range(from, to);
                return Outcome::Edited;
            }
            // On Windows Space is reported as a named key with no typed character.
            ("space", false) => {
                self.insert(" ");
                return Outcome::Edited;
            }
            _ => {}
        }
        match key_char {
            Some(text) if !ctrl && !text.is_empty() && text.chars().all(|c| !c.is_control()) => {
                self.insert(text);
                Outcome::Edited
            }
            _ => Outcome::Ignored,
        }
    }

    fn move_caret(&mut self, key: &str, ctrl: bool, shift: bool) -> Outcome {
        let rows = self.rows();
        let len = self.len_chars();
        let selection = self.selection();
        // Without Shift, Left and Right first collapse an existing selection.
        if let (false, Some((a, b)), "left" | "right") = (shift, selection, key) {
            self.cursor = if key == "left" { a } else { b };
            self.anchor = None;
            self.goal = None;
            return Outcome::Moved;
        }
        let rope = Rope::from_str(&self.text);
        let row = Self::row_of(&rows, self.cursor);
        let col = self.cursor - rows[row].start;
        let mut keeps_goal = false;
        let target = match (key, ctrl) {
            ("left", true) => prev_word_start(&rope, self.cursor),
            ("right", true) => next_word_end(&rope, self.cursor),
            ("left", false) => self.cursor.saturating_sub(1),
            ("right", false) => (self.cursor + 1).min(len),
            ("home", true) => 0,
            ("end", true) => len,
            ("home", false) => rows[row].start,
            ("end", false) => rows[row].end,
            ("up" | "down", _) => {
                keeps_goal = true;
                let goal = *self.goal.get_or_insert(col);
                if key == "up" {
                    if row == 0 { 0 } else { rows[row - 1].start + goal.min(rows[row - 1].len()) }
                } else if row + 1 >= rows.len() {
                    len
                } else {
                    rows[row + 1].start + goal.min(rows[row + 1].len())
                }
            }
            _ => self.cursor,
        };
        if shift {
            self.anchor.get_or_insert(self.cursor);
        } else {
            self.anchor = None;
        }
        self.cursor = target;
        if !keeps_goal {
            self.goal = None;
        }
        Outcome::Moved
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn typed(a: &mut TextArea, s: &str) {
        for c in s.chars() {
            if c == '\n' {
                a.apply("enter", false, false, None);
            } else {
                a.apply("x", false, false, Some(&c.to_string()));
            }
        }
    }

    #[test]
    fn enter_adds_a_line_and_ctrl_enter_submits() {
        let mut a = TextArea::default();
        typed(&mut a, "subject\n\nbody");
        assert_eq!(a.text(), "subject\n\nbody");
        assert_eq!(a.rows().len(), 3);
        assert_eq!(a.apply("enter", true, false, None), Outcome::Submit);
        assert_eq!(a.text(), "subject\n\nbody", "Ctrl+Enter must not add a line");
    }

    #[test]
    fn long_lines_wrap_at_the_column_count() {
        let mut a = TextArea::default();
        a.set_wrap(5);
        a.set_text("abcdefghijkl\nxy");
        let rows: Vec<(usize, usize)> = a.rows().iter().map(|r| (r.start, r.end)).collect();
        assert_eq!(rows, vec![(0, 5), (5, 10), (10, 12), (13, 15)]);
        a.set_cursor(7, false);
        assert_eq!(a.cursor_row_col(), (1, 2));
        assert_eq!(a.offset_at(1, 99), 10, "a column past the end lands on the row's end");
    }

    #[test]
    fn up_and_down_move_by_displayed_row_and_remember_the_column() {
        let mut a = TextArea::default();
        a.set_wrap(10);
        a.set_text("abcdefgh\nab\nabcdefgh");
        a.set_cursor(7, false); // row 0, col 7
        a.apply("down", false, false, None);
        assert_eq!(a.cursor_row_col(), (1, 2), "the short row clamps the column");
        a.apply("down", false, false, None);
        assert_eq!(a.cursor_row_col(), (2, 7), "the remembered column returns");
        a.apply("down", false, false, None);
        assert_eq!(a.cursor(), a.len_chars(), "Down on the last row goes to the end");
        a.set_cursor(3, false);
        a.apply("up", false, false, None);
        assert_eq!(a.cursor(), 0, "Up on the first row goes to the start");
    }

    #[test]
    fn shift_extends_and_typing_replaces_the_selection() {
        let mut a = TextArea::default();
        typed(&mut a, "hello world");
        a.apply("left", true, true, None);
        assert_eq!(a.selected_text().as_deref(), Some("world"));
        typed(&mut a, "there");
        assert_eq!(a.text(), "hello there");
        assert_eq!(a.selection(), None);
    }

    #[test]
    fn left_and_right_collapse_a_selection_to_its_edges() {
        let mut a = TextArea::default();
        a.set_text("abcdef");
        a.set_cursor(1, false);
        a.set_cursor(4, true);
        a.apply("left", false, false, None);
        assert_eq!((a.cursor(), a.selection()), (1, None));
        a.set_cursor(4, true);
        a.apply("right", false, false, None);
        assert_eq!((a.cursor(), a.selection()), (4, None));
    }

    #[test]
    fn select_all_delete_and_pasted_line_breaks() {
        let mut a = TextArea::default();
        typed(&mut a, "abc");
        a.apply("a", true, false, None);
        assert_eq!(a.selected_text().as_deref(), Some("abc"));
        assert_eq!(a.apply("backspace", false, false, None), Outcome::Edited);
        assert_eq!(a.text(), "");
        a.insert("one\r\ntwo\rthree");
        assert_eq!(a.text(), "one\ntwo\nthree");
    }

    #[test]
    fn home_end_and_word_keys() {
        let mut a = TextArea::default();
        typed(&mut a, "foo bar\nbaz");
        a.apply("home", false, false, None);
        assert_eq!(a.cursor_row_col(), (1, 0));
        a.apply("home", true, false, None);
        assert_eq!(a.cursor(), 0);
        a.apply("right", true, false, None);
        assert_eq!(a.cursor(), 3);
        a.apply("end", false, false, None);
        assert_eq!(a.cursor(), 7);
        a.apply("backspace", true, false, None);
        assert_eq!(a.text(), "foo bar\nbaz".replace("bar", "").as_str().replace("foo ", "foo "));
    }

    #[test]
    fn undo_and_redo_restore_text() {
        let mut a = TextArea::default();
        typed(&mut a, "ab");
        a.apply("z", true, false, None);
        assert_eq!(a.text(), "a");
        a.apply("y", true, false, None);
        assert_eq!(a.text(), "ab");
    }

    #[test]
    fn unicode_and_space_and_clipboard_keys() {
        let mut a = TextArea::default();
        typed(&mut a, "é😀");
        assert_eq!((a.len_chars(), a.cursor()), (2, 2));
        a.apply("space", false, false, None);
        assert_eq!(a.text(), "é😀 ");
        a.apply("backspace", false, false, None);
        a.apply("backspace", false, false, None);
        assert_eq!(a.text(), "é");
        assert_eq!(a.apply("c", true, false, Some("c")), Outcome::Ignored, "copy is the caller's");
        assert_eq!(a.apply("space", true, false, None), Outcome::Ignored);
    }
}
