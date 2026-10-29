//! Word boundaries in VS Code's sense: runs of identifier chars, runs of punctuation,
//! and runs of whitespace are each one "word" for movement and double-click.

use ropey::Rope;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Class {
    Word,
    Space,
    Punct,
}

fn class(c: char) -> Class {
    if c.is_alphanumeric() || c == '_' {
        Class::Word
    } else if c.is_whitespace() {
        Class::Space
    } else {
        Class::Punct
    }
}

pub fn is_word_char(c: char) -> bool {
    class(c) == Class::Word
}

/// Offset after skipping the word to the right of `offset` (VS Code's Ctrl+Right:
/// leading whitespace is skipped, then one word of a single class).
pub fn next_word_end(rope: &Rope, offset: usize) -> usize {
    let len = rope.len_chars();
    let mut at = offset.min(len);
    while at < len && rope.char(at) != '\n' && class(rope.char(at)) == Class::Space {
        at += 1;
    }
    if at < len && rope.char(at) == '\n' {
        return (at + 1).min(len);
    }
    if at >= len {
        return len;
    }
    let kind = class(rope.char(at));
    while at < len && class(rope.char(at)) == kind {
        at += 1;
    }
    at
}

/// Offset after skipping the word to the left of `offset` (Ctrl+Left).
pub fn prev_word_start(rope: &Rope, offset: usize) -> usize {
    let mut at = offset.min(rope.len_chars());
    while at > 0 && rope.char(at - 1) != '\n' && class(rope.char(at - 1)) == Class::Space {
        at -= 1;
    }
    if at > 0 && rope.char(at - 1) == '\n' {
        return at - 1;
    }
    if at == 0 {
        return 0;
    }
    let kind = class(rope.char(at - 1));
    while at > 0 && class(rope.char(at - 1)) == kind {
        at -= 1;
    }
    at
}

/// The word under `offset` for double-click: the run of one class touching it, preferring
/// the char to the right. `None` on an empty line or at the end of the buffer.
pub fn word_range_at(rope: &Rope, offset: usize) -> Option<(usize, usize)> {
    let len = rope.len_chars();
    let at = offset.min(len);
    let on = |i: usize| (i < len && rope.char(i) != '\n').then(|| class(rope.char(i)));
    let pivot = if on(at).is_some() {
        at
    } else if at > 0 && on(at - 1).is_some() {
        at - 1
    } else {
        return None;
    };
    let kind = on(pivot)?;
    let mut start = pivot;
    while start > 0 && on(start - 1) == Some(kind) {
        start -= 1;
    }
    let mut end = pivot;
    while on(end) == Some(kind) {
        end += 1;
    }
    Some((start, end))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(s: &str) -> Rope {
        Rope::from_str(s)
    }

    #[test]
    fn word_motion_skips_one_class_at_a_time() {
        let t = r("foo.bar  baz");
        assert_eq!(next_word_end(&t, 0), 3); // foo
        assert_eq!(next_word_end(&t, 3), 4); // .
        assert_eq!(next_word_end(&t, 4), 7); // bar
        assert_eq!(next_word_end(&t, 7), 12); // spaces then baz
        assert_eq!(next_word_end(&t, 12), 12);
        assert_eq!(prev_word_start(&t, 12), 9);
        assert_eq!(prev_word_start(&t, 9), 4); // back over spaces and bar
        assert_eq!(prev_word_start(&t, 4), 3);
        assert_eq!(prev_word_start(&t, 0), 0);
    }

    #[test]
    fn word_motion_stops_at_line_breaks() {
        let t = r("ab\ncd");
        assert_eq!(next_word_end(&t, 2), 3, "from a line end, move onto the next line");
        assert_eq!(prev_word_start(&t, 3), 2, "from a line start, move to the previous line end");
        assert_eq!(next_word_end(&t, 3), 5);
    }

    #[test]
    fn double_click_picks_the_word_or_run_under_the_cursor() {
        let t = r("let foo_bar = 1;\n\nx");
        assert_eq!(word_range_at(&t, 5), Some((4, 11)));
        assert_eq!(word_range_at(&t, 4), Some((4, 11)), "left edge belongs to the word");
        assert_eq!(word_range_at(&t, 11), Some((11, 12)), "between word and space picks the space run");
        assert_eq!(word_range_at(&t, 15), Some((15, 16)), "the semicolon");
        assert_eq!(word_range_at(&t, 17), None, "empty line");
        assert_eq!(word_range_at(&t, 18), Some((18, 19)));
        assert_eq!(word_range_at(&t, 19), Some((18, 19)), "end of buffer falls back to the previous char");
    }

    #[test]
    fn unicode_words() {
        let t = r("héllo wörld");
        assert_eq!(word_range_at(&t, 1), Some((0, 5)));
        assert_eq!(next_word_end(&t, 0), 5);
    }
}
