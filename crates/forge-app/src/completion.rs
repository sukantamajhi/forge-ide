//! Completion popup state and how an accepted item edits the buffer.

use forge_lsp::CompletionItem;
use forge_text::{Buffer, SelectionSet};

/// Rows visible at once; the popup scrolls its window to keep the selection in view.
pub const VISIBLE_ROWS: usize = 8;

pub struct CompletionState {
    pub items: Vec<CompletionItem>,
    pub selected: usize,
}

impl CompletionState {
    pub fn new(items: Vec<CompletionItem>) -> Self {
        Self { items, selected: 0 }
    }

    pub fn move_selection(&mut self, delta: isize) {
        let last = self.items.len().saturating_sub(1) as isize;
        self.selected = (self.selected as isize + delta).clamp(0, last) as usize;
    }

    pub fn selected_item(&self) -> Option<&CompletionItem> {
        self.items.get(self.selected)
    }

    /// Index range of the rows to draw, keeping the selection inside the window.
    pub fn visible_window(&self) -> std::ops::Range<usize> {
        let start = self.selected.saturating_sub(VISIBLE_ROWS - 1);
        start..(start + VISIBLE_ROWS).min(self.items.len())
    }
}

pub fn is_identifier_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// Keep items whose label contains `prefix` as a case-insensitive subsequence, with
/// prefix matches first. Servers such as rust-analyzer return every candidate in scope
/// and leave narrowing to the client. Server order is kept within each group.
pub fn filter_items(items: Vec<CompletionItem>, prefix: &str) -> Vec<CompletionItem> {
    if prefix.is_empty() {
        return items;
    }
    let needle: Vec<char> = prefix.to_lowercase().chars().collect();
    let matches = |label: &str| {
        let mut wanted = needle.iter();
        let mut next = wanted.next();
        for c in label.to_lowercase().chars() {
            if Some(&c) == next {
                next = wanted.next();
            }
        }
        next.is_none()
    };
    let (mut head, tail): (Vec<_>, Vec<_>) = items
        .into_iter()
        .filter(|item| matches(&item.label))
        .partition(|item| item.label.to_lowercase().starts_with(&prefix.to_lowercase()));
    head.extend(tail);
    head
}

/// Offset where the identifier ending at `offset` begins.
pub fn identifier_start(buffer: &Buffer, offset: usize) -> usize {
    let rope = buffer.rope();
    let mut start = offset.min(rope.len_chars());
    while start > 0 && is_identifier_char(rope.char(start - 1)) {
        start -= 1;
    }
    start
}

/// Apply `item` at the primary caret and return the resulting caret. The server's text
/// edit wins; otherwise the identifier typed so far is replaced by the insert text or label.
pub fn accept(buffer: &mut Buffer, selections: &SelectionSet, item: &CompletionItem) -> SelectionSet {
    let caret = selections.primary().head;
    let before = SelectionSet::single(selections.primary());
    let (start, end, text) = match &item.edit {
        Some(edit) => (
            buffer.offset_from_utf16(edit.range.start.line, edit.range.start.character),
            buffer.offset_from_utf16(edit.range.end.line, edit.range.end.character),
            edit.new_text.as_str(),
        ),
        None => (
            identifier_start(buffer, caret),
            caret,
            item.insert_text.as_deref().unwrap_or(&item.label),
        ),
    };
    buffer.replace_range(&before, start, end, text)
}

#[cfg(test)]
mod tests {
    use super::*;
    use forge_lsp::{Position, Range, TextEditSpec};
    use forge_text::Selection;

    fn item(label: &str) -> CompletionItem {
        CompletionItem {
            label: label.into(),
            detail: None,
            kind: None,
            sort_text: None,
            edit: None,
            insert_text: None,
        }
    }

    fn caret(at: usize) -> SelectionSet {
        SelectionSet::single(Selection::caret(at))
    }

    #[test]
    fn without_an_edit_the_typed_identifier_is_replaced() {
        let mut buf = Buffer::new("let x = String::ne");
        let after = accept(&mut buf, &caret(18), &item("new"));
        assert_eq!(buf.text(), "let x = String::new");
        assert_eq!(after.primary().head, 19);

        let mut with_insert = item("push_str");
        with_insert.insert_text = Some("push_str(".to_owned());
        let mut buf = Buffer::new("s.pu");
        accept(&mut buf, &caret(4), &with_insert);
        assert_eq!(buf.text(), "s.push_str(");
    }

    #[test]
    fn server_text_edit_uses_utf16_columns() {
        // "😀" is two UTF-16 units, so `fo` sits at columns 3..5 even though it is chars 1..3.
        let mut buf = Buffer::new("😀fo");
        let mut it = item("foo");
        it.edit = Some(TextEditSpec {
            range: Range {
                start: Position { line: 0, character: 2 },
                end: Position { line: 0, character: 4 },
            },
            new_text: "foo()".into(),
        });
        accept(&mut buf, &caret(3), &it);
        assert_eq!(buf.text(), "😀foo()");
    }

    #[test]
    fn accepting_collapses_to_the_primary_caret_and_undoes_cleanly() {
        let mut buf = Buffer::new("ab\nab");
        let sels = SelectionSet::new(vec![Selection::caret(2), Selection::caret(5)]);
        accept(&mut buf, &sels, &item("abc"));
        assert_eq!(buf.text(), "abc\nab");
        buf.undo().unwrap();
        assert_eq!(buf.text(), "ab\nab");
    }

    #[test]
    fn selection_clamps_and_window_follows_it() {
        let mut state = CompletionState::new((0..20).map(|i| item(&i.to_string())).collect());
        state.move_selection(-3);
        assert_eq!(state.selected, 0);
        assert_eq!(state.visible_window(), 0..8);
        state.move_selection(12);
        assert_eq!(state.visible_window(), 5..13);
        state.move_selection(100);
        assert_eq!(state.selected, 19);
        assert_eq!(state.visible_window(), 12..20);
        assert_eq!(state.selected_item().unwrap().label, "19");
    }

    #[test]
    fn filtering_is_fuzzy_with_prefix_matches_first() {
        let items: Vec<CompletionItem> = ["unwrap_or", "new", "renew", "with_capacity", "String"]
            .iter()
            .map(|l| item(l))
            .collect();
        let labels = |v: Vec<CompletionItem>| v.into_iter().map(|i| i.label).collect::<Vec<_>>();
        assert_eq!(labels(filter_items(items.clone(), "ne")), ["new", "renew"]);
        // Subsequence: w-c matches with_capacity; case-insensitive.
        assert_eq!(labels(filter_items(items.clone(), "WC")), ["with_capacity"]);
        assert_eq!(labels(filter_items(items.clone(), "zzz")), Vec::<String>::new());
        assert_eq!(filter_items(items, "").len(), 5);
    }

    #[test]
    fn identifier_start_stops_at_punctuation() {
        let buf = Buffer::new("a.b_c1");
        assert_eq!(identifier_start(&buf, 6), 2);
        assert_eq!(identifier_start(&buf, 2), 2);
        assert_eq!(identifier_start(&buf, 1), 0);
    }
}
