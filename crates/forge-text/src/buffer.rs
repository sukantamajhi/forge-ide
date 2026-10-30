use ropey::Rope;

use crate::selection::{Selection, SelectionSet};

/// One replacement in char offsets, recorded so it can be inverted.
#[derive(Clone, Debug)]
struct Edit {
    at: usize,
    removed: String,
    inserted_chars: usize,
    inserted: String,
}

/// All edits of one user action, in application order, with the selections around it.
#[derive(Clone, Debug)]
struct Transaction {
    edits: Vec<Edit>,
    before: SelectionSet,
    after: SelectionSet,
}

/// One rope mutation in byte terms, in the shape incremental parsers expect.
/// Points are `(row, byte column)`. Each edit is relative to the text as it was
/// immediately before that edit, so a list must be replayed in order.
///
/// Also carries the replaced range as `(line, UTF-16 column)` pairs and the inserted
/// text, which is exactly what a language server's incremental `didChange` needs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TextEdit {
    pub start_byte: usize,
    pub old_end_byte: usize,
    pub new_end_byte: usize,
    pub start_point: (usize, usize),
    pub old_end_point: (usize, usize),
    pub new_end_point: (usize, usize),
    pub start_utf16: (usize, usize),
    pub old_end_utf16: (usize, usize),
    pub inserted: String,
    /// The replaced range and its replacement length in chars, for remapping positions.
    pub start_char: usize,
    pub old_end_char: usize,
    pub new_end_char: usize,
}

#[derive(Default)]
pub struct Buffer {
    rope: Rope,
    undo: Vec<Transaction>,
    redo: Vec<Transaction>,
    pending_edits: Vec<TextEdit>,
}

impl Buffer {
    pub fn new(text: &str) -> Self {
        Self { rope: Rope::from_str(text), ..Self::default() }
    }

    /// Apply several replacements as one undoable action. `reps` are `(start, end, text)`
    /// in char offsets, sorted and non-overlapping. `after` computes the selections to
    /// leave behind from the edited buffer and the new start offset of each replacement.
    pub fn edit_many(
        &mut self,
        before: &SelectionSet,
        reps: &[(usize, usize, String)],
        after: impl FnOnce(&Buffer, &[usize]) -> SelectionSet,
    ) -> SelectionSet {
        debug_assert!(reps.windows(2).all(|w| w[0].1 <= w[1].0), "replacements must not overlap");
        let mut edits = Vec::with_capacity(reps.len());
        for (start, end, text) in reps.iter().rev() {
            let removed = self.rope.slice(*start..*end).to_string();
            self.replace(*start, *end, text);
            edits.push(Edit { at: *start, removed, inserted_chars: text.chars().count(), inserted: text.clone() });
        }
        let mut shift: isize = 0;
        let starts: Vec<usize> = reps
            .iter()
            .map(|(s, e, t)| {
                let new_start = (*s as isize + shift) as usize;
                shift += t.chars().count() as isize - (*e - *s) as isize;
                new_start
            })
            .collect();
        let after = after(self, &starts);
        if edits.iter().any(|e| !e.removed.is_empty() || e.inserted_chars > 0) {
            self.undo.push(Transaction { edits, before: before.clone(), after: after.clone() });
            self.redo.clear();
        }
        after
    }

    /// Fold the two most recent undo steps into one, so a run of typing undoes together.
    /// No-op unless the later step starts where the earlier one ended.
    pub fn merge_last_transactions(&mut self) -> bool {
        let n = self.undo.len();
        if n < 2 || self.undo[n - 2].after != self.undo[n - 1].before {
            return false;
        }
        let last = self.undo.pop().expect("length checked");
        let prev = self.undo.last_mut().expect("length checked");
        prev.edits.extend(last.edits);
        prev.after = last.after;
        true
    }

    /// The line terminator this buffer uses, judged by its first line break.
    pub fn eol(&self) -> &'static str {
        let first = self.rope.line(0);
        let n = first.len_chars();
        if n >= 2 && first.char(n - 2) == '\r' && first.char(n - 1) == '\n' { "\r\n" } else { "\n" }
    }

    /// Char offset of the start of `line`.
    pub fn line_start(&self, line: usize) -> usize {
        self.rope.line_to_char(line.min(self.len_lines() - 1))
    }

    /// Char offset of the end of `line`'s content (before its terminator).
    pub fn line_end(&self, line: usize) -> usize {
        let line = line.min(self.len_lines() - 1);
        self.line_start(line) + self.line_len(line)
    }

    /// Edits applied since the last call, in application order.
    pub fn take_edits(&mut self) -> Vec<TextEdit> {
        std::mem::take(&mut self.pending_edits)
    }

    fn byte_point(&self, byte: usize) -> (usize, usize) {
        let row = self.rope.byte_to_line(byte);
        (row, byte - self.rope.line_to_byte(row))
    }

    /// Replace chars `start..end` with `text`, recording the byte-level edit.
    fn replace(&mut self, start: usize, end: usize, text: &str) {
        let start_byte = self.rope.char_to_byte(start);
        let old_end_byte = self.rope.char_to_byte(end);
        let start_point = self.byte_point(start_byte);
        let old_end_point = self.byte_point(old_end_byte);
        let start_utf16 = self.utf16_point(start);
        let old_end_utf16 = self.utf16_point(end);
        if start != end {
            self.rope.remove(start..end);
        }
        if !text.is_empty() {
            self.rope.insert(start, text);
        }
        let new_end_byte = start_byte + text.len();
        let new_end_point = self.byte_point(new_end_byte);
        self.pending_edits.push(TextEdit {
            start_byte,
            old_end_byte,
            new_end_byte,
            start_point,
            old_end_point,
            new_end_point,
            start_utf16,
            old_end_utf16,
            inserted: text.to_owned(),
            start_char: start,
            old_end_char: end,
            new_end_char: start + text.chars().count(),
        });
    }

    /// `(line, UTF-16 column)` of a char offset, the position unit of the LSP.
    pub fn utf16_point(&self, offset: usize) -> (usize, usize) {
        let (line, col) = self.offset_to_point(offset);
        let start = self.rope.line_to_char(line);
        let units = self.rope.slice(start..start + col).chars().map(char::len_utf16).sum();
        (line, units)
    }

    /// Char offset of an LSP position. A column past the end of the line clamps to its
    /// end, and one that lands inside a surrogate pair rounds up past the pair.
    pub fn offset_from_utf16(&self, line: usize, units: usize) -> usize {
        let line = line.min(self.len_lines() - 1);
        let start = self.rope.line_to_char(line);
        let mut used = 0;
        let mut cols = 0;
        for c in self.rope.slice(start..start + self.line_len(line)).chars() {
            if used >= units {
                break;
            }
            used += c.len_utf16();
            cols += 1;
        }
        start + cols
    }

    pub fn rope(&self) -> &Rope {
        &self.rope
    }

    pub fn len_chars(&self) -> usize {
        self.rope.len_chars()
    }

    pub fn len_lines(&self) -> usize {
        self.rope.len_lines()
    }

    pub fn text(&self) -> String {
        self.rope.to_string()
    }

    /// The first `max_chars` characters of the text (all of it when it is shorter).
    pub fn text_head(&self, max_chars: usize) -> String {
        self.rope.slice(..max_chars.min(self.rope.len_chars())).to_string()
    }

    /// Text of line `idx` without its trailing newline.
    pub fn line(&self, idx: usize) -> String {
        let line = self.rope.line(idx).to_string();
        line.trim_end_matches(['\n', '\r']).to_owned()
    }

    /// Length of line `idx` in chars, excluding its line terminator.
    pub fn line_len(&self, idx: usize) -> usize {
        let line = self.rope.line(idx);
        let mut n = line.len_chars();
        while n > 0 && matches!(line.char(n - 1), '\n' | '\r') {
            n -= 1;
        }
        n
    }

    /// `(line, column)` of a char offset; offsets past the end clamp to the end.
    pub fn offset_to_point(&self, offset: usize) -> (usize, usize) {
        let offset = offset.min(self.len_chars());
        let line = self.rope.char_to_line(offset);
        (line, offset - self.rope.line_to_char(line))
    }

    /// Char offset of `(line, column)`, clamping both to the buffer.
    pub fn point_to_offset(&self, line: usize, column: usize) -> usize {
        let line = line.min(self.len_lines() - 1);
        self.rope.line_to_char(line) + column.min(self.line_len(line))
    }

    /// Move every head by `delta` chars; `extend` keeps the anchors (shift-select).
    pub fn move_horizontal(&self, selections: &SelectionSet, delta: isize, extend: bool) -> SelectionSet {
        let len = self.len_chars() as isize;
        selections.map(|s| {
            let head = if !extend && !s.is_empty() {
                // Collapsing a range jumps to its edge in the direction of travel.
                if delta < 0 { s.start() } else { s.end() }
            } else {
                (s.head as isize + delta).clamp(0, len) as usize
            };
            Selection { anchor: if extend { s.anchor } else { head }, head }
        })
    }

    /// Move every head by `delta` lines, keeping its column where the line is long enough.
    pub fn move_vertical(&self, selections: &SelectionSet, delta: isize, extend: bool) -> SelectionSet {
        selections.map(|s| {
            let (line, column) = self.offset_to_point(s.head);
            let target = (line as isize + delta).clamp(0, self.len_lines() as isize - 1) as usize;
            let head = self.point_to_offset(target, column);
            Selection { anchor: if extend { s.anchor } else { head }, head }
        })
    }

    /// Move every head to the start or end of its line.
    pub fn move_to_line_edge(&self, selections: &SelectionSet, end: bool, extend: bool) -> SelectionSet {
        selections.map(|s| {
            let (line, _) = self.offset_to_point(s.head);
            let head = self.point_to_offset(line, if end { usize::MAX } else { 0 });
            Selection { anchor: if extend { s.anchor } else { head }, head }
        })
    }

    /// Add a caret one line above (`delta < 0`) or below the outermost existing head.
    pub fn add_cursor_vertical(&self, selections: &SelectionSet, delta: isize) -> SelectionSet {
        let edge = if delta < 0 { selections.as_slice().first() } else { selections.as_slice().last() };
        let Some(edge) = edge else { return selections.clone() };
        let (line, column) = self.offset_to_point(edge.head);
        let target = line as isize + delta;
        if target < 0 || target >= self.len_lines() as isize {
            return selections.clone();
        }
        let mut all = selections.as_slice().to_vec();
        all.push(Selection::caret(self.point_to_offset(target as usize, column)));
        SelectionSet::new(all)
    }

    /// Replace every selection with `text`; returns the resulting carets.
    pub fn insert(&mut self, selections: &SelectionSet, text: &str) -> SelectionSet {
        let ranges: Vec<(usize, usize)> =
            selections.as_slice().iter().map(|s| (s.start(), s.end())).collect();
        self.apply(selections, ranges, text)
    }

    /// Replace chars `start..end` (clamped to the buffer) with `text` as one undoable
    /// edit. Returns a single caret after the inserted text; `before` is what undo restores.
    pub fn replace_range(&mut self, before: &SelectionSet, start: usize, end: usize, text: &str) -> SelectionSet {
        let len = self.len_chars();
        let start = start.min(len);
        let end = end.clamp(start, len);
        self.apply(before, vec![(start, end)], text)
    }

    /// Delete each selection, or the char before an empty caret.
    pub fn delete_backward(&mut self, selections: &SelectionSet) -> SelectionSet {
        let ranges: Vec<(usize, usize)> = selections
            .as_slice()
            .iter()
            .map(|s| if s.is_empty() { (s.start().saturating_sub(1), s.start()) } else { (s.start(), s.end()) })
            .collect();
        self.apply(selections, ranges, "")
    }

    /// Delete each selection, or the char after an empty caret.
    pub fn delete_forward(&mut self, selections: &SelectionSet) -> SelectionSet {
        let len = self.len_chars();
        let ranges: Vec<(usize, usize)> = selections
            .as_slice()
            .iter()
            .map(|s| if s.is_empty() { (s.start(), (s.start() + 1).min(len)) } else { (s.start(), s.end()) })
            .collect();
        self.apply(selections, ranges, "")
    }

    pub fn undo(&mut self) -> Option<SelectionSet> {
        let tx = self.undo.pop()?;
        // Edits ran back-to-front by position; undoing in reverse order keeps offsets valid.
        for edit in tx.edits.iter().rev() {
            self.replace(edit.at, edit.at + edit.inserted_chars, &edit.removed);
        }
        let selections = tx.before.clone();
        self.redo.push(tx);
        Some(selections)
    }

    pub fn redo(&mut self) -> Option<SelectionSet> {
        let tx = self.redo.pop()?;
        for edit in &tx.edits {
            self.replace(edit.at, edit.at + edit.removed.chars().count(), &edit.inserted);
        }
        let selections = tx.after.clone();
        self.undo.push(tx);
        Some(selections)
    }

    /// `ranges` must be sorted and non-overlapping (guaranteed by `SelectionSet`).
    fn apply(&mut self, before: &SelectionSet, ranges: Vec<(usize, usize)>, text: &str) -> SelectionSet {
        let inserted_chars = text.chars().count();
        let mut edits = Vec::with_capacity(ranges.len());
        // Back-to-front so earlier offsets stay valid while we mutate.
        for &(start, end) in ranges.iter().rev() {
            let removed = self.rope.slice(start..end).to_string();
            self.replace(start, end, text);
            edits.push(Edit { at: start, removed, inserted_chars, inserted: text.to_owned() });
        }

        let mut shift: isize = 0;
        let mut carets = Vec::with_capacity(ranges.len());
        for &(start, end) in &ranges {
            let at = (start as isize + shift) as usize + inserted_chars;
            carets.push(Selection::caret(at));
            shift += inserted_chars as isize - (end - start) as isize;
        }
        let after = SelectionSet::new(carets);

        if edits.iter().any(|e| !e.removed.is_empty() || e.inserted_chars > 0) {
            self.undo.push(Transaction { edits, before: before.clone(), after: after.clone() });
            self.redo.clear();
        }
        after
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn carets(offsets: &[usize]) -> SelectionSet {
        SelectionSet::new(offsets.iter().map(|&o| Selection::caret(o)).collect())
    }

    #[test]
    fn insert_at_multiple_carets() {
        let mut buf = Buffer::new("a\nb\nc");
        let after = buf.insert(&carets(&[1, 3, 5]), "!");
        assert_eq!(buf.text(), "a!\nb!\nc!");
        let at: Vec<usize> = after.as_slice().iter().map(|s| s.head).collect();
        assert_eq!(at, vec![2, 5, 8]);
    }

    #[test]
    fn replace_selection_and_undo_redo() {
        let mut buf = Buffer::new("hello world");
        let sel = SelectionSet::single(Selection { anchor: 0, head: 5 });
        let after = buf.insert(&sel, "howdy");
        assert_eq!(buf.text(), "howdy world");
        assert_eq!(after.primary().head, 5);

        let restored = buf.undo().unwrap();
        assert_eq!(buf.text(), "hello world");
        assert_eq!(restored, sel);

        buf.redo().unwrap();
        assert_eq!(buf.text(), "howdy world");
        assert!(buf.redo().is_none());
    }

    #[test]
    fn multi_cursor_undo_restores_text() {
        let mut buf = Buffer::new("one two three");
        let sel = SelectionSet::new(vec![
            Selection { anchor: 0, head: 3 },
            Selection { anchor: 8, head: 13 },
        ]);
        buf.insert(&sel, "X");
        assert_eq!(buf.text(), "X two X");
        buf.undo().unwrap();
        assert_eq!(buf.text(), "one two three");
    }

    #[test]
    fn backspace_handles_start_of_buffer_and_unicode() {
        let mut buf = Buffer::new("é😀x");
        let after = buf.delete_backward(&carets(&[0]));
        assert_eq!(buf.text(), "é😀x");
        assert_eq!(after.primary().head, 0);

        buf.delete_backward(&carets(&[2]));
        assert_eq!(buf.text(), "éx");
    }

    #[test]
    fn delete_forward_stops_at_end() {
        let mut buf = Buffer::new("ab");
        buf.delete_forward(&carets(&[2]));
        assert_eq!(buf.text(), "ab");
        buf.delete_forward(&carets(&[0]));
        assert_eq!(buf.text(), "b");
    }

    #[test]
    fn noop_edits_do_not_pollute_history() {
        let mut buf = Buffer::new("ab");
        buf.delete_backward(&carets(&[0]));
        assert!(buf.undo().is_none());
    }

    #[test]
    fn overlapping_selections_merge() {
        let set = SelectionSet::new(vec![
            Selection { anchor: 0, head: 4 },
            Selection { anchor: 2, head: 6 },
            Selection::caret(6),
        ]);
        assert_eq!(set.as_slice().len(), 1);
        assert_eq!((set.primary().start(), set.primary().end()), (0, 6));
    }

    #[test]
    fn point_offset_roundtrip_and_clamping() {
        let buf = Buffer::new("ab\ncdef\n\nx");
        assert_eq!(buf.offset_to_point(4), (1, 1));
        assert_eq!(buf.point_to_offset(1, 3), 6);
        // Column past the end of a short line clamps before the newline.
        assert_eq!(buf.point_to_offset(0, 99), 2);
        assert_eq!(buf.point_to_offset(99, 99), buf.len_chars());
        assert_eq!(buf.line_len(2), 0);
    }

    #[test]
    fn vertical_move_keeps_column_and_clamps() {
        let buf = Buffer::new("abcd\nx\nabcd");
        let down = buf.move_vertical(&carets(&[3]), 1, false);
        assert_eq!(down.primary().head, 6); // end of the short line
        let down = buf.move_vertical(&down, 1, false);
        assert_eq!(buf.offset_to_point(down.primary().head), (2, 1));
        let top = buf.move_vertical(&carets(&[0]), -1, false);
        assert_eq!(top.primary().head, 0);
    }

    #[test]
    fn horizontal_move_collapses_range_and_extends() {
        let buf = Buffer::new("hello");
        let range = SelectionSet::single(Selection { anchor: 1, head: 4 });
        assert_eq!(buf.move_horizontal(&range, -1, false).primary(), Selection::caret(1));
        assert_eq!(buf.move_horizontal(&range, 1, false).primary(), Selection::caret(4));
        let ext = buf.move_horizontal(&carets(&[2]), 1, true).primary();
        assert_eq!((ext.anchor, ext.head), (2, 3));
        assert_eq!(buf.move_horizontal(&carets(&[5]), 1, false).primary().head, 5);
    }

    #[test]
    fn line_edges_and_added_cursors() {
        let buf = Buffer::new("one\ntwo\nthree");
        assert_eq!(buf.move_to_line_edge(&carets(&[5]), false, false).primary().head, 4);
        assert_eq!(buf.move_to_line_edge(&carets(&[5]), true, false).primary().head, 7);
        let two = buf.add_cursor_vertical(&carets(&[1]), 1);
        let heads: Vec<usize> = two.as_slice().iter().map(|s| s.head).collect();
        assert_eq!(heads, vec![1, 5]);
        // No line below the last one: unchanged.
        let last = carets(&[10]);
        assert_eq!(buf.add_cursor_vertical(&last, 1), last);
    }

    #[test]
    fn edits_are_recorded_in_bytes_and_replay_in_order() {
        let mut buf = Buffer::new("é\nxy");
        // Replace "xy" (chars 2..4) with "€": multibyte on both sides.
        buf.insert(&SelectionSet::single(Selection { anchor: 2, head: 4 }), "€");
        let edits = buf.take_edits();
        assert_eq!(edits.len(), 1);
        let e = &edits[0];
        assert_eq!((e.start_byte, e.old_end_byte, e.new_end_byte), (3, 5, 6));
        assert_eq!((e.start_point, e.old_end_point, e.new_end_point), ((1, 0), (1, 2), (1, 3)));
        assert_eq!((e.start_utf16, e.old_end_utf16, e.inserted.as_str()), ((1, 0), (1, 2), "€"));
        assert!(buf.take_edits().is_empty());

        // A newline insert moves the end point to the next row.
        buf.insert(&carets(&[2]), "\n");
        let edits = buf.take_edits();
        assert_eq!((edits[0].start_point, edits[0].new_end_point), ((1, 0), (2, 0)));

        // Undo is recorded too, so a parser stays in sync across it.
        buf.undo().unwrap();
        assert_eq!(buf.take_edits().len(), 1);
    }

    #[test]
    fn replace_range_is_one_undo_step_and_clamps() {
        let mut buf = Buffer::new("hello world");
        let before = carets(&[11]);
        let after = buf.replace_range(&before, 6, 99, "there");
        assert_eq!(buf.text(), "hello there");
        assert_eq!(after.primary().head, 11);
        assert_eq!(buf.undo().unwrap(), before);
        assert_eq!(buf.text(), "hello world");
    }

    #[test]
    fn merged_typing_undoes_and_redoes_as_one_step() {
        let mut buf = Buffer::new("");
        let mut sel = carets(&[0]);
        for c in ["a", "b", "c"] {
            sel = buf.insert(&sel, c);
        }
        assert!(buf.merge_last_transactions());
        assert!(buf.merge_last_transactions());
        assert!(!buf.merge_last_transactions(), "only one step is left");
        assert_eq!(buf.text(), "abc");
        assert_eq!(buf.undo().unwrap(), carets(&[0]));
        assert_eq!(buf.text(), "");
        assert_eq!(buf.redo().unwrap().primary().head, 3);
        assert_eq!(buf.text(), "abc");
    }

    #[test]
    fn non_adjacent_steps_do_not_merge() {
        let mut buf = Buffer::new("ab");
        buf.insert(&carets(&[0]), "x");
        buf.insert(&carets(&[2]), "y"); // the caret jumped: not where the last step ended
        assert!(!buf.merge_last_transactions());
    }

    #[test]
    fn utf16_positions_count_surrogate_pairs() {
        // 😀 is one char but two UTF-16 units.
        let buf = Buffer::new("a😀b\nxy");
        assert_eq!(buf.utf16_point(2), (0, 3));
        assert_eq!(buf.utf16_point(3), (0, 4));
        assert_eq!(buf.offset_from_utf16(0, 3), 2);
        // Inside the pair rounds up past it; past the line end clamps to the line end.
        assert_eq!(buf.offset_from_utf16(0, 2), 2);
        assert_eq!(buf.offset_from_utf16(0, 99), 3);
        assert_eq!(buf.offset_from_utf16(1, 1), 5);
        assert_eq!(buf.offset_from_utf16(9, 99), buf.len_chars());
    }

    #[test]
    fn line_strips_newline() {
        let buf = Buffer::new("a\r\nb\n");
        assert_eq!(buf.line(0), "a");
        assert_eq!(buf.line(1), "b");
    }
}
