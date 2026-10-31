//! Inlay hints in a line of text: the hint strings are spliced into what is drawn, the colour runs are cut around
//! them, and a click position is mapped back to a column of the real text.

use std::ops::Range;

use forge_text::columns::display_width;

/// `shown` with each `(byte offset, hint)` of `inserts` put in at that offset, and `runs` (sorted, not overlapping)
/// moved to match: a run that spans an insertion is cut in two around it. The hints themselves get `hint_style`.
/// Returns the new text and runs, sorted.
pub fn splice_hints<S: Clone>(shown: &str, runs: Vec<(Range<usize>, S)>, inserts: &[(usize, &str)], hint_style: S) -> (String, Vec<(Range<usize>, S)>) {
    let mut inserts: Vec<(usize, &str)> = inserts.iter().map(|(b, l)| ((*b).min(shown.len()), *l)).collect();
    inserts.sort_by_key(|i| i.0);
    // Where an old byte offset ends up: a start moves past hints at or before it, an end only past hints before it.
    let start_of = |b: usize| b + inserts.iter().filter(|(ib, _)| *ib <= b).map(|(_, l)| l.len()).sum::<usize>();
    let end_of = |b: usize| b + inserts.iter().filter(|(ib, _)| *ib < b).map(|(_, l)| l.len()).sum::<usize>();

    let mut out = String::with_capacity(shown.len() + inserts.iter().map(|(_, l)| l.len()).sum::<usize>());
    let mut hint_runs = Vec::with_capacity(inserts.len());
    let mut done = 0;
    for (at, label) in &inserts {
        out.push_str(&shown[done..*at]);
        let from = out.len();
        out.push_str(label);
        hint_runs.push((from..out.len(), hint_style.clone()));
        done = *at;
    }
    out.push_str(&shown[done..]);

    let mut moved: Vec<(Range<usize>, S)> = Vec::with_capacity(runs.len() + hint_runs.len());
    for (range, style) in runs {
        let mut from = range.start;
        for (at, _) in inserts.iter().filter(|(b, _)| *b > range.start && *b < range.end) {
            if *at > from {
                moved.push((start_of(from)..end_of(*at), style.clone()));
            }
            from = *at;
        }
        if range.end > from {
            moved.push((start_of(from)..end_of(range.end), style.clone()));
        }
    }
    moved.extend(hint_runs);
    moved.sort_by_key(|(r, _)| r.start);
    (out, moved)
}

/// A pointer position in cells across a line drawn with `hints` (`(column, text)`, in order), as the cell it is over in
/// the line without them: hints it is past are taken out, and a position over a hint becomes the column it sits at.
pub fn cell_without_hints(text: &str, hints: &[(usize, String)], cell: f32) -> f32 {
    let mut x = cell;
    for (col, label) in hints {
        let width = label.chars().count() as f32;
        let at = display_width(text, *col) as f32;
        if x >= at + width {
            x -= width;
        } else if x >= at {
            return at;
        } else {
            break;
        }
    }
    x
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hints_are_spliced_in_and_runs_are_cut_around_them() {
        // "let x = 5;" with a type hint after the x (byte 5) and a run over "x = 5" (4..9).
        let (text, runs) = splice_hints("let x = 5;", vec![(0..3, 'k'), (4..9, 'v')], &[(5, ": i32")], 'h');
        assert_eq!(text, "let x: i32 = 5;");
        assert_eq!(runs, vec![(0..3, 'k'), (4..5, 'v'), (5..10, 'h'), (10..14, 'v')]);
    }

    #[test]
    fn runs_beside_a_hint_move_without_being_cut() {
        // The hint is at 3; a run ending at 3 stays, one starting at 3 moves after the hint.
        let (text, runs) = splice_hints("fn main", vec![(0..2, 'a'), (3..7, 'b')], &[(3, "<T>")], 'h');
        assert_eq!(text, "fn <T>main");
        assert_eq!(runs, vec![(0..2, 'a'), (3..6, 'h'), (6..10, 'b')]);
        let (same, none) = splice_hints("abc", vec![(0..3, 'x')], &[], 'h');
        assert_eq!((same.as_str(), none), ("abc", vec![(0..3, 'x')]));
    }

    #[test]
    fn two_hints_in_one_line() {
        let (text, runs) = splice_hints("f(1, 2)", Vec::new(), &[(2, "a: "), (5, "b: ")], 'h');
        assert_eq!(text, "f(a: 1, b: 2)");
        assert_eq!(runs, vec![(2..5, 'h'), (8..11, 'h')]);
    }

    #[test]
    fn a_click_is_mapped_back_past_the_hints_it_is_after() {
        let hints = vec![(5, ": i32".to_owned())];
        let text = "let x = 5;";
        // Before the hint: unchanged. Over it: the column it sits at (5). After it: shifted back by its width.
        assert_eq!(cell_without_hints(text, &hints, 2.0), 2.0);
        assert_eq!(cell_without_hints(text, &hints, 7.0), 5.0);
        assert_eq!(cell_without_hints(text, &hints, 12.0), 7.0);
        assert_eq!(cell_without_hints(text, &[], 3.5), 3.5);
    }
}
