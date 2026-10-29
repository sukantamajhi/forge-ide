//! Mapping between char columns and on-screen cell columns for monospace rendering.
//!
//! Assumes one cell per char except tabs, which take [`TAB_WIDTH`] cells. East Asian
//! wide chars and combining marks are not handled yet.

pub const TAB_WIDTH: usize = 4;

fn cells(c: char) -> usize {
    if c == '\t' { TAB_WIDTH } else { 1 }
}

/// Cells occupied by the first `cols` chars of `text`.
pub fn display_width(text: &str, cols: usize) -> usize {
    text.chars().take(cols).map(cells).sum()
}

/// Text as drawn: tabs expanded to spaces.
pub fn expand_tabs(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        if c == '\t' {
            out.extend(std::iter::repeat_n(' ', TAB_WIDTH));
        } else {
            out.push(c);
        }
    }
    out
}

/// Like [`expand_tabs`], plus for each char column (and one past the end) the byte
/// offset of that column in the expanded text, for styling runs by byte range.
pub fn expand_tabs_mapped(text: &str) -> (String, Vec<usize>) {
    let mut out = String::with_capacity(text.len());
    let mut byte_of_col = Vec::with_capacity(text.len() + 1);
    for c in text.chars() {
        byte_of_col.push(out.len());
        if c == '\t' {
            out.extend(std::iter::repeat_n(' ', TAB_WIDTH));
        } else {
            out.push(c);
        }
    }
    byte_of_col.push(out.len());
    (out, byte_of_col)
}

/// Char column whose left edge is nearest to `cell_x` (in fractional cells).
pub fn col_at_cell(text: &str, cell_x: f32) -> usize {
    let mut left = 0.0_f32;
    let mut count = 0;
    for (i, c) in text.chars().enumerate() {
        let w = cells(c) as f32;
        if cell_x < left + w / 2.0 {
            return i;
        }
        left += w;
        count = i + 1;
    }
    count
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tabs_widen_columns() {
        assert_eq!(display_width("\tab", 1), 4);
        assert_eq!(display_width("\tab", 3), 6);
        assert_eq!(display_width("ab", 99), 2);
        assert_eq!(expand_tabs("a\tb"), "a    b");
    }

    #[test]
    fn mapped_expansion_gives_byte_offsets_per_column() {
        let (text, map) = expand_tabs_mapped("é\tx");
        assert_eq!(text, "é    x");
        // é is 2 bytes; the tab becomes 4 spaces; one entry per column plus the end.
        assert_eq!(map, vec![0, 2, 6, 7]);
    }

    #[test]
    fn click_maps_to_nearest_boundary() {
        assert_eq!(col_at_cell("abc", 0.2), 0);
        assert_eq!(col_at_cell("abc", 0.6), 1);
        assert_eq!(col_at_cell("abc", 2.4), 2);
        assert_eq!(col_at_cell("abc", 50.0), 3);
        assert_eq!(col_at_cell("abc", -5.0), 0);
        // A tab spans four cells: clicking past its midpoint lands after it.
        assert_eq!(col_at_cell("\tx", 1.9), 0);
        assert_eq!(col_at_cell("\tx", 2.1), 1);
    }
}
