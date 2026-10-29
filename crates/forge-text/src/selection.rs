use crate::buffer::TextEdit;

/// Carry a char offset across edits applied in order (each relative to the text before
/// it). A position inside a replaced range collapses to that range's start.
pub fn map_offset_through_edits(mut pos: usize, edits: &[TextEdit]) -> usize {
    for e in edits {
        if pos >= e.old_end_char {
            // Past the edit: shift by how much the replacement grew or shrank.
            pos = pos - (e.old_end_char - e.start_char) + (e.new_end_char - e.start_char);
        } else if pos > e.start_char {
            pos = e.start_char;
        }
    }
    pos
}

/// A caret (`anchor == head`) or a range; `head` is the moving end.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Selection {
    pub anchor: usize,
    pub head: usize,
}

impl Selection {
    pub fn caret(at: usize) -> Self {
        Self { anchor: at, head: at }
    }

    pub fn start(&self) -> usize {
        self.anchor.min(self.head)
    }

    pub fn end(&self) -> usize {
        self.anchor.max(self.head)
    }

    pub fn is_empty(&self) -> bool {
        self.anchor == self.head
    }
}

/// Non-empty set of selections, kept sorted by start with overlaps merged.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SelectionSet {
    selections: Vec<Selection>,
}

impl SelectionSet {
    pub fn single(selection: Selection) -> Self {
        Self { selections: vec![selection] }
    }

    pub fn new(selections: Vec<Selection>) -> Self {
        let mut set = Self { selections };
        set.normalize();
        set
    }

    /// The same selections after `edits` made elsewhere (e.g. through another view).
    pub fn map_through_edits(&self, edits: &[TextEdit]) -> Self {
        self.map(|s| Selection {
            anchor: map_offset_through_edits(s.anchor, edits),
            head: map_offset_through_edits(s.head, edits),
        })
    }

    pub fn as_slice(&self) -> &[Selection] {
        &self.selections
    }

    pub fn primary(&self) -> Selection {
        self.selections[0]
    }

    /// Apply `f` to every selection, then re-normalize.
    pub fn map(&self, mut f: impl FnMut(Selection) -> Selection) -> Self {
        Self::new(self.selections.iter().copied().map(&mut f).collect())
    }

    fn normalize(&mut self) {
        if self.selections.is_empty() {
            self.selections.push(Selection::caret(0));
        }
        self.selections.sort_by_key(|s| (s.start(), s.end()));
        let mut merged: Vec<Selection> = Vec::with_capacity(self.selections.len());
        for s in self.selections.drain(..) {
            match merged.last_mut() {
                // Overlapping, or two carets at one spot, collapse into one selection.
                Some(prev) if s.start() < prev.end() || (s.start() == prev.end() && (s.is_empty() || prev.is_empty())) => {
                    let end = prev.end().max(s.end());
                    *prev = Selection { anchor: prev.start(), head: end };
                }
                _ => merged.push(s),
            }
        }
        self.selections = merged;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Buffer;

    #[test]
    fn positions_follow_edits_made_elsewhere() {
        let mut buf = Buffer::new("hello world");
        let before = SelectionSet::single(Selection { anchor: 6, head: 11 });
        // Another view inserts at the start and deletes a middle char.
        buf.insert(&SelectionSet::single(Selection::caret(0)), ">> ");
        buf.delete_backward(&SelectionSet::single(Selection::caret(5)));
        let mapped = before.map_through_edits(&buf.take_edits());
        assert_eq!(buf.text(), ">> hllo world");
        let r = &buf.rope().slice(mapped.primary().start()..mapped.primary().end()).to_string();
        assert_eq!(r, "world", "the selection still covers the same text");
    }

    #[test]
    fn positions_inside_a_replaced_range_collapse_to_its_start() {
        let mut buf = Buffer::new("abcdef");
        buf.insert(&SelectionSet::single(Selection { anchor: 1, head: 5 }), "X");
        let mapped = SelectionSet::single(Selection::caret(3)).map_through_edits(&buf.take_edits());
        assert_eq!(mapped.primary().head, 1);
    }
}
