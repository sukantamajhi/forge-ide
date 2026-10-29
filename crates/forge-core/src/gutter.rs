//! Which lines of a file differ from its committed version, for the editor's change gutter.

use similar::{DiffOp, TextDiff};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChangeKind {
    Added,
    Modified,
    /// Lines were deleted just before `start`.
    Removed,
}

/// A run of changed lines in the *new* text (0-based).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LineChange {
    pub kind: ChangeKind,
    pub start: usize,
    /// Number of lines in the new text; 0 for [`ChangeKind::Removed`].
    pub len: usize,
}

fn normalize(text: &str) -> String {
    // A file saved with CRLF against an LF blob is not a change on every line.
    text.replace("\r\n", "\n")
}

/// Line-level differences from `old` (HEAD) to `new` (the buffer), sorted by position.
pub fn line_changes(old: &str, new: &str) -> Vec<LineChange> {
    let (old, new) = (normalize(old), normalize(new));
    let diff = TextDiff::from_lines(&old, &new);
    let mut out = Vec::new();
    for op in diff.ops() {
        match *op {
            DiffOp::Equal { .. } => {}
            DiffOp::Insert { new_index, new_len, .. } => out.push(LineChange { kind: ChangeKind::Added, start: new_index, len: new_len }),
            DiffOp::Delete { new_index, .. } => out.push(LineChange { kind: ChangeKind::Removed, start: new_index, len: 0 }),
            DiffOp::Replace { old_len, new_index, new_len, .. } => {
                // The overlapping lines were edited; extra new lines were added, and
                // extra old lines were removed.
                let modified = old_len.min(new_len);
                out.push(LineChange { kind: ChangeKind::Modified, start: new_index, len: modified });
                if new_len > modified {
                    out.push(LineChange { kind: ChangeKind::Added, start: new_index + modified, len: new_len - modified });
                } else if old_len > modified {
                    out.push(LineChange { kind: ChangeKind::Removed, start: new_index + modified, len: 0 });
                }
            }
        }
    }
    out
}

/// The change affecting `line`, if any. A removal marks the line it sits above.
pub fn change_at(changes: &[LineChange], line: usize) -> Option<ChangeKind> {
    let i = changes.partition_point(|c| c.start + c.len.max(1) <= line);
    changes.get(i).filter(|c| c.start <= line).map(|c| c.kind)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(old: &str, new: &str) -> Vec<(ChangeKind, usize, usize)> {
        line_changes(old, new).into_iter().map(|c| (c.kind, c.start, c.len)).collect()
    }

    #[test]
    fn identical_text_has_no_changes() {
        assert!(line_changes("a\nb\n", "a\nb\n").is_empty());
        assert!(line_changes("", "").is_empty());
    }

    #[test]
    fn insert_modify_and_delete() {
        assert_eq!(kinds("a\nb\n", "a\nx\nb\n"), vec![(ChangeKind::Added, 1, 1)]);
        assert_eq!(kinds("a\nb\nc\n", "a\nB\nc\n"), vec![(ChangeKind::Modified, 1, 1)]);
        assert_eq!(kinds("a\nb\nc\n", "a\nc\n"), vec![(ChangeKind::Removed, 1, 0)]);
        assert_eq!(kinds("a\nb\n", ""), vec![(ChangeKind::Removed, 0, 0)]);
    }

    #[test]
    fn replacements_split_into_modified_plus_added_or_removed() {
        // One line became three: first is edited, two are new.
        assert_eq!(kinds("a\nb\nc\n", "a\nX\nY\nZ\nc\n"), vec![(ChangeKind::Modified, 1, 1), (ChangeKind::Added, 2, 2)]);
        // Two lines became one: it is edited and a deletion is marked after it.
        assert_eq!(kinds("a\nb\nc\nd\n", "a\nX\nd\n"), vec![(ChangeKind::Modified, 1, 1), (ChangeKind::Removed, 2, 0)]);
    }

    #[test]
    fn brand_new_file_is_all_added_and_crlf_is_not_a_change() {
        assert_eq!(kinds("", "x\ny\nz\n"), vec![(ChangeKind::Added, 0, 3)]);
        assert!(line_changes("a\nb\n", "a\r\nb\r\n").is_empty());
        // A file without a trailing newline edited on its last line.
        assert_eq!(kinds("a\nb", "a\nc"), vec![(ChangeKind::Modified, 1, 1)]);
    }

    #[test]
    fn change_at_finds_the_run_covering_a_line_and_removal_markers() {
        let changes = line_changes("a\nb\nc\nd\ne\n", "a\nB\nc\ne\nf\n");
        // b->B modified (line 1); d deleted (marker on line 3, now `e`); f added (line 4).
        assert_eq!(change_at(&changes, 0), None);
        assert_eq!(change_at(&changes, 1), Some(ChangeKind::Modified));
        assert_eq!(change_at(&changes, 2), None);
        assert_eq!(change_at(&changes, 3), Some(ChangeKind::Removed));
        assert_eq!(change_at(&changes, 4), Some(ChangeKind::Added));
        assert_eq!(change_at(&changes, 99), None);
        assert_eq!(change_at(&[], 0), None);
    }
}
