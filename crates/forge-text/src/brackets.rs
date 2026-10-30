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

/// One bracket on a line, for coloring by nesting.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LineBracket {
    /// Char column in the line.
    pub col: usize,
    /// How deeply nested it is (an opener and its closer share a depth); 0 is outermost.
    pub depth: i32,
    /// A closer with no opener before it.
    pub unmatched: bool,
}

/// The brackets of one line outside strings and line comments, given the nesting depth the line starts at,
/// and the depth the line ends at. Quotes are guessed per line: a `'` only opens a string when a closing `'`
/// follows on the same line (otherwise it is an apostrophe); with `lifetimes` (Rust) it must follow within three
/// chars, so `'a` and `&'a str` are not taken for a string.
pub fn line_brackets(line: &str, start_depth: i32, line_comment: Option<&str>, lifetimes: bool) -> (Vec<LineBracket>, i32) {
    const APOSTROPHE: char = '\u{27}';
    const BACKSLASH: char = '\u{5c}';
    let chars: Vec<char> = line.chars().collect();
    let comment: Vec<char> = line_comment.unwrap_or("").chars().collect();
    let mut out = Vec::new();
    let mut depth = start_depth.max(0);
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if !comment.is_empty() && chars[i..].starts_with(&comment) {
            break;
        }
        match c {
            '"' | '`' | APOSTROPHE => {
                let mut end = None;
                let mut k = i + 1;
                while k < chars.len() {
                    if chars[k] == BACKSLASH {
                        k += 2;
                        continue;
                    }
                    if chars[k] == c {
                        end = Some(k);
                        break;
                    }
                    if c == APOSTROPHE && lifetimes && k > i + 3 {
                        break;
                    }
                    k += 1;
                }
                match end {
                    Some(end) => {
                        i = end + 1;
                        continue;
                    }
                    // A lone `"` or backtick runs to the end of the line.
                    None if c != APOSTROPHE => break,
                    None => {}
                }
            }
            '(' | '[' | '{' => {
                out.push(LineBracket { col: i, depth, unmatched: false });
                depth += 1;
            }
            ')' | ']' | '}' => {
                if depth == 0 {
                    out.push(LineBracket { col: i, depth: 0, unmatched: true });
                } else {
                    depth -= 1;
                    out.push(LineBracket { col: i, depth, unmatched: false });
                }
            }
            _ => {}
        }
        i += 1;
    }
    (out, depth)
}

/// The nesting depth at the start of every line (plus one entry for the end), for [`line_brackets`].
pub fn depths_at_line_starts(rope: &Rope, line_comment: Option<&str>, lifetimes: bool) -> Vec<i32> {
    let mut depths = Vec::with_capacity(rope.len_lines() + 1);
    let mut depth = 0i32;
    for line in rope.lines() {
        depths.push(depth);
        depth = line_brackets(&line.to_string(), depth, line_comment, lifetimes).1;
    }
    depths.push(depth);
    depths
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

    #[test]
    fn bracket_colors_follow_nesting_and_skip_strings_and_comments() {
        let depths = |text: &str| line_brackets(text, 0, Some("//"), true).0.iter().map(|b| (b.col, b.depth, b.unmatched)).collect::<Vec<_>>();
        assert_eq!(depths("f(a[b]{c})"), vec![(1, 0, false), (3, 1, false), (5, 1, false), (6, 1, false), (8, 1, false), (9, 0, false)]);
        assert_eq!(depths("x = \"(\" + (y) // (z"), vec![(10, 0, false), (12, 0, false)]);
        assert_eq!(depths("fn f<'a>(x: &'a str) {"), vec![(8, 0, false), (19, 0, false), (21, 0, false)], "a lifetime is not a string");
        assert_eq!(depths("c = '('; d = (1)"), vec![(13, 0, false), (15, 0, false)], "a char literal hides its bracket");
        assert_eq!(depths(")"), vec![(0, 0, true)]);
        assert_eq!(line_brackets("if (a) {", 0, None, false).1, 1);
        let js = line_brackets("s = '(a b)' + (c)", 0, Some("//"), false).0;
        assert_eq!(js.iter().map(|b| b.col).collect::<Vec<_>>(), vec![14, 16], "a JS string hides its brackets");
    }

    #[test]
    fn depths_carry_across_lines() {
        let rope = Rope::from_str("fn f() {
    g(
        1,
    );
}
");
        // Before each line, then at the end: 0, then inside `{`, then inside `g(`, closed again, then outside.
        assert_eq!(depths_at_line_starts(&rope, Some("//"), true), vec![0, 1, 2, 2, 1, 0, 0]);
    }
}
