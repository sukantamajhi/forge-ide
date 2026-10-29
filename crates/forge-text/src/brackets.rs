//! Matching bracket lookup for highlighting and Go to Bracket.

use ropey::Rope;

/// Give up scanning after this many chars so a stray bracket in a huge file stays cheap.
const MAX_SCAN: usize = 200_000;

const PAIRS: [(char, char); 3] = [('(', ')'), ('[', ']'), ('{', '}')];

/// `(bracket, partner)` char offsets for the bracket at `offset` or just before it,
/// where only brackets of the same kind nest. `None` if there is no bracket or no partner.
pub fn matching_bracket(rope: &Rope, offset: usize) -> Option<(usize, usize)> {
    let len = rope.len_chars();
    let at = |i: usize| (i < len).then(|| rope.char(i));
    let candidates = [Some(offset), offset.checked_sub(1)];
    for pos in candidates.into_iter().flatten() {
        let Some(c) = at(pos) else { continue };
        if let Some(&(open, close)) = PAIRS.iter().find(|(o, _)| *o == c) {
            let mut depth = 0usize;
            for i in pos..len.min(pos + MAX_SCAN) {
                let ch = rope.char(i);
                if ch == open {
                    depth += 1;
                } else if ch == close {
                    depth -= 1;
                    if depth == 0 {
                        return Some((pos, i));
                    }
                }
            }
            return None;
        }
        if let Some(&(open, close)) = PAIRS.iter().find(|(_, cl)| *cl == c) {
            let mut depth = 0usize;
            for i in (pos.saturating_sub(MAX_SCAN)..=pos).rev() {
                let ch = rope.char(i);
                if ch == close {
                    depth += 1;
                } else if ch == open {
                    depth -= 1;
                    if depth == 0 {
                        return Some((pos, i));
                    }
                }
            }
            return None;
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn m(text: &str, offset: usize) -> Option<(usize, usize)> {
        matching_bracket(&Rope::from_str(text), offset)
    }

    #[test]
    fn matches_forward_and_backward_with_nesting() {
        assert_eq!(m("f(a(b)c)", 1), Some((1, 7)));
        assert_eq!(m("f(a(b)c)", 7), Some((7, 1)));
        assert_eq!(m("f(a(b)c)", 3), Some((3, 5)));
        assert_eq!(m("{ [x] }", 0), Some((0, 6)));
    }

    #[test]
    fn caret_just_after_a_bracket_counts() {
        // Offset 2 is just after the `(` at 1; the char at 2 is not a bracket.
        assert_eq!(m("f(x)", 2), Some((1, 3)));
        assert_eq!(m("f(x)", 4), Some((3, 1)), "just after the closer at the end");
    }

    #[test]
    fn unmatched_and_non_brackets_are_none() {
        assert_eq!(m("f(x", 1), None);
        assert_eq!(m("x)", 1), None);
        assert_eq!(m("abc", 1), None);
        assert_eq!(m("", 0), None);
    }

    #[test]
    fn only_the_same_kind_nests() {
        // The `]` inside does not close the `(`.
        assert_eq!(m("(a]b)", 0), Some((0, 4)));
    }
}
