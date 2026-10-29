//! Word wrap: which displayed row shows which part of which line.
//!
//! The editor is monospace and draws a line as its tab-expanded text, so a line's cells are
//! simply the chars of that text. A wrapped line is split into rows at cell positions; a row is
//! drawn as the whole line shifted left by the row's start cell and clipped.

use std::collections::HashMap;

/// Cell positions where each row of a line starts (the first is always 0). Rows break after the
/// last space that fits, unless that would leave the row nearly empty, and otherwise hard at `width`.
pub fn breaks(shown: &str, width: usize) -> Vec<usize> {
    let width = width.max(1);
    let mut starts = vec![0];
    let (mut row_start, mut cell) = (0usize, 0usize);
    let mut after_space: Option<usize> = None;
    for ch in shown.chars() {
        if cell + 1 - row_start > width {
            let at = match after_space {
                Some(p) if p > row_start + width / 3 => p,
                _ => cell,
            };
            starts.push(at);
            row_start = at;
            after_space = None;
        }
        cell += 1;
        if ch == ' ' {
            after_space = Some(cell);
        }
    }
    starts
}

/// One displayed row: line `line`, its `k`th row, covering cells `start..end` (`end` is `None` for the last row).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Row {
    pub line: usize,
    pub k: usize,
    pub start: usize,
    pub end: Option<usize>,
}

pub struct WrapMap {
    /// Document revision, row width and line count this was built for.
    pub key: (u64, usize, usize, usize),
    rows: Vec<(u32, u16)>,
    /// First row of each logical line (`u32::MAX` for lines that are not shown).
    first_row: Vec<u32>,
    /// Start cells for lines that wrap onto more than one row.
    starts: HashMap<u32, Vec<u32>>,
}

impl WrapMap {
    /// Build from `(line, tab-expanded text)` pairs, ascending, for the lines that are shown.
    pub fn build(key: (u64, usize, usize, usize), total_lines: usize, lines: impl Iterator<Item = (usize, String)>) -> Self {
        let width = key.1;
        let mut rows = Vec::with_capacity(total_lines);
        let mut first_row = vec![u32::MAX; total_lines];
        let mut starts = HashMap::new();
        for (line, shown) in lines {
            first_row[line] = rows.len() as u32;
            let cuts = breaks(&shown, width);
            for k in 0..cuts.len() {
                rows.push((line as u32, k.min(u16::MAX as usize) as u16));
            }
            if cuts.len() > 1 {
                starts.insert(line as u32, cuts.into_iter().map(|c| c as u32).collect());
            }
        }
        Self { key, rows, first_row, starts }
    }

    pub fn row_count(&self) -> usize {
        self.rows.len()
    }

    pub fn row(&self, index: usize) -> Option<Row> {
        let (line, k) = *self.rows.get(index)?;
        let (line, k) = (line as usize, k as usize);
        Some(match self.starts.get(&(line as u32)) {
            Some(cuts) => Row { line, k, start: cuts[k] as usize, end: cuts.get(k + 1).map(|c| *c as usize) },
            None => Row { line, k: 0, start: 0, end: None },
        })
    }

    /// The displayed row that holds cell `cell` of `line`. `None` for a line that is not shown.
    pub fn row_of(&self, line: usize, cell: usize) -> Option<usize> {
        let first = *self.first_row.get(line)?;
        if first == u32::MAX {
            return None;
        }
        let k = self.starts.get(&(line as u32)).map_or(0, |cuts| cuts.partition_point(|s| *s as usize <= cell).saturating_sub(1));
        Some(first as usize + k)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_lines_do_not_wrap() {
        assert_eq!(breaks("short", 20), vec![0]);
        assert_eq!(breaks("", 20), vec![0]);
        assert_eq!(breaks("exactly ten", 11), vec![0]);
    }

    #[test]
    fn rows_break_after_the_last_space_that_fits() {
        // "aaaa bbbb cccc": row 0 holds "aaaa " (cells 0..5), then "bbbb " and "cccc".
        assert_eq!(breaks("aaaa bbbb cccc", 8), vec![0, 5, 10]);
    }

    #[test]
    fn a_long_word_breaks_hard_at_the_width() {
        assert_eq!(breaks("abcdefghijklmnop", 6), vec![0, 6, 12]);
        // A space too early in the row is not worth breaking at.
        assert_eq!(breaks("a bcdefghijklmnop", 8), vec![0, 8, 16]);
    }

    #[test]
    fn rows_map_back_to_lines_and_cells() {
        let lines = vec![(0, "abc".to_owned()), (1, "aaaa bbbb cccc".to_owned()), (3, "z".to_owned())];
        let map = WrapMap::build((1, 8, 4, 3), 4, lines.into_iter());
        assert_eq!(map.row_count(), 1 + 3 + 1);
        assert_eq!(map.row(0), Some(Row { line: 0, k: 0, start: 0, end: None }));
        assert_eq!(map.row(2), Some(Row { line: 1, k: 1, start: 5, end: Some(10) }));
        assert_eq!(map.row(3), Some(Row { line: 1, k: 2, start: 10, end: None }));
        assert_eq!(map.row(4), Some(Row { line: 3, k: 0, start: 0, end: None }));
        assert_eq!(map.row_of(1, 0), Some(1));
        assert_eq!(map.row_of(1, 5), Some(2));
        assert_eq!(map.row_of(1, 13), Some(3));
        assert_eq!(map.row_of(2, 0), None, "line 2 is hidden (folded)");
        assert_eq!(map.row_of(3, 0), Some(4));
        assert_eq!(map.row(9), None);
    }
}
