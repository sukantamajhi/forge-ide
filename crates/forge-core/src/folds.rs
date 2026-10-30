//! Code folding state: which regions exist, which are collapsed, and which lines are shown.

use std::collections::BTreeSet;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Folds {
    /// `(first line, last line)` of every foldable region, sorted by first line.
    regions: Vec<(usize, usize)>,
    /// First lines of collapsed regions.
    collapsed: BTreeSet<usize>,
}

impl Folds {
    /// Replace the available regions (after a re-parse). Collapsed folds are kept only
    /// where a region still starts on the same line.
    pub fn set_regions(&mut self, mut regions: Vec<(usize, usize)>) {
        regions.sort_unstable();
        regions.dedup();
        self.collapsed.retain(|start| regions.iter().any(|r| r.0 == *start));
        self.regions = regions;
    }

    pub fn regions(&self) -> &[(usize, usize)] {
        &self.regions
    }

    /// The start lines of the regions that contain `top` but begin above it, outermost first, at most `max` of them
    /// (the innermost ones when there are more): the scopes "sticky scroll" keeps at the top of the editor.
    pub fn enclosing_starts(&self, top: usize, max: usize) -> Vec<usize> {
        let mut starts: Vec<usize> = self.regions().iter().filter(|(start, end)| *start < top && *end >= top).map(|(start, _)| *start).collect();
        starts.sort_unstable();
        starts.dedup();
        let skip = starts.len().saturating_sub(max);
        starts.split_off(skip)
    }

    pub fn has_region_at(&self, line: usize) -> bool {
        self.regions.iter().any(|r| r.0 == line)
    }

    pub fn is_collapsed(&self, start: usize) -> bool {
        self.collapsed.contains(&start)
    }

    /// Last hidden line of the collapsed region starting at `line`.
    pub fn collapsed_end(&self, line: usize) -> Option<usize> {
        self.collapsed.contains(&line).then(|| self.regions.iter().find(|r| r.0 == line).map(|r| r.1)).flatten()
    }

    /// Whether `line` is hidden inside a collapsed region (the region's own first line stays visible).
    pub fn is_hidden(&self, line: usize) -> bool {
        self.collapsed.iter().any(|s| *s < line && self.collapsed_end(*s).is_some_and(|e| line <= e))
    }

    /// The region Fold/Unfold should act on for a cursor on `line`: one starting there, else
    /// the innermost one containing it.
    fn region_for(&self, line: usize, want_collapsed: Option<bool>) -> Option<(usize, usize)> {
        let ok = |r: &&(usize, usize)| want_collapsed.is_none_or(|c| self.collapsed.contains(&r.0) == c);
        self.regions
            .iter()
            .filter(ok)
            .find(|r| r.0 == line)
            .or_else(|| self.regions.iter().filter(ok).filter(|r| r.0 <= line && line <= r.1).max_by_key(|r| r.0))
            .copied()
    }

    /// Collapse the region at `line`. Returns the line the cursor should move to (the
    /// region's first line) if something was folded.
    pub fn fold_at(&mut self, line: usize) -> Option<usize> {
        let region = self.region_for(line, Some(false))?;
        self.collapsed.insert(region.0);
        Some(region.0)
    }

    pub fn unfold_at(&mut self, line: usize) -> bool {
        // A cursor inside a hidden region is first moved by the caller; here find the
        // collapsed region that hides or starts at `line`.
        let target = self
            .collapsed
            .iter()
            .copied()
            .filter(|s| *s == line || (*s < line && self.collapsed_end(*s).is_some_and(|e| line <= e)))
            .max();
        target.is_some_and(|s| self.collapsed.remove(&s))
    }

    pub fn toggle_at(&mut self, line: usize) -> bool {
        if self.unfold_at(line) {
            return true;
        }
        self.fold_at(line).is_some()
    }

    pub fn fold_all(&mut self) {
        self.collapsed = self.regions.iter().map(|r| r.0).collect();
    }

    pub fn unfold_all(&mut self) {
        self.collapsed.clear();
    }

    pub fn any_collapsed(&self) -> bool {
        !self.collapsed.is_empty()
    }

    /// Lines to display, in order. `None` when nothing is collapsed (every line shows).
    pub fn visible_lines(&self, total: usize) -> Option<Vec<usize>> {
        if self.collapsed.is_empty() {
            return None;
        }
        let mut out = Vec::with_capacity(total);
        let mut line = 0;
        while line < total {
            out.push(line);
            line = match self.collapsed_end(line) {
                Some(end) => end + 1,
                None => line + 1,
            };
        }
        Some(out)
    }

    /// Keep folds attached to their text across an edit that replaced rows `first..=old_last`
    /// with `first..=new_last`. Collapsed folds below shift; a collapsed fold whose header or
    /// hidden body the edit touches is opened so typed text is never hidden.
    pub fn apply_edit(&mut self, first: usize, old_last: usize, new_last: usize) {
        let delta = new_last as isize - old_last as isize;
        let shift = |line: usize| (line as isize + delta).max(0) as usize;
        let mut kept = BTreeSet::new();
        for &start in &self.collapsed {
            let end = self.regions.iter().find(|r| r.0 == start).map(|r| r.1);
            if start > old_last {
                kept.insert(shift(start));
            } else if start < first && end.is_some_and(|e| e < first) {
                kept.insert(start);
            }
        }
        self.collapsed = kept;
        for r in &mut self.regions {
            if r.0 > old_last {
                r.0 = shift(r.0);
                r.1 = shift(r.1);
            } else if r.0 < first && r.1 >= first {
                r.1 = shift(r.1).max(r.0);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn folds() -> Folds {
        let mut f = Folds::default();
        f.set_regions(vec![(0, 9), (2, 4), (6, 8)]);
        f
    }

    #[test]
    fn fold_picks_the_region_at_or_around_the_cursor() {
        let mut f = folds();
        assert_eq!(f.fold_at(3), Some(2), "innermost region containing the line");
        assert!(f.is_hidden(3) && f.is_hidden(4) && !f.is_hidden(2) && !f.is_hidden(5));
        assert_eq!(f.fold_at(0), Some(0), "a region starting on the line wins");
        assert!(f.is_hidden(2), "inside the outer fold now");
        assert_eq!(f.fold_at(20), None);
    }

    #[test]
    fn unfold_from_inside_a_hidden_region_or_its_header() {
        let mut f = folds();
        f.fold_at(3);
        assert!(f.unfold_at(3), "a hidden line unfolds its region");
        assert!(!f.any_collapsed());
        f.fold_at(2);
        assert!(f.unfold_at(2), "the header line unfolds too");
        assert!(!f.unfold_at(2));
    }

    #[test]
    fn visible_lines_skip_collapsed_bodies() {
        let mut f = folds();
        assert_eq!(f.visible_lines(10), None);
        f.fold_at(2);
        f.fold_at(6);
        assert_eq!(f.visible_lines(10), Some(vec![0, 1, 2, 5, 6, 9]));
        f.fold_all();
        assert_eq!(f.visible_lines(10), Some(vec![0]), "the outer fold hides everything else");
        f.unfold_all();
        assert_eq!(f.visible_lines(10), None);
    }

    #[test]
    fn toggle_and_regions_refresh_keep_matching_folds_only() {
        let mut f = folds();
        assert!(f.toggle_at(2));
        assert!(f.is_collapsed(2));
        f.set_regions(vec![(0, 9), (6, 8)]);
        assert!(!f.is_collapsed(2), "its region disappeared");
        assert!(f.toggle_at(6) && f.is_collapsed(6), "toggling folds an open region");
        assert!(f.toggle_at(6) && !f.is_collapsed(6), "and unfolds a collapsed one");
        assert!(!f.toggle_at(1) || f.is_collapsed(0), "a line inside only the outer region folds that one");
    }

    #[test]
    fn edits_shift_folds_below_and_open_touched_ones() {
        let mut f = folds();
        f.fold_at(6);
        // Two lines inserted at row 1: the fold at 6 moves to 8.
        f.apply_edit(1, 1, 3);
        assert!(f.is_collapsed(8) && !f.is_collapsed(6));
        assert!(f.has_region_at(8));

        // Typing inside the folded region's header opens it.
        let mut g = folds();
        g.fold_at(2);
        g.apply_edit(2, 2, 2);
        assert!(!g.any_collapsed(), "editing the header line reveals the body");

        // An edit inside the hidden body opens the fold; one in a later, unrelated row does not.
        let mut body = folds();
        body.fold_at(2);
        body.apply_edit(3, 3, 3);
        assert!(!body.any_collapsed());
        let mut below = folds();
        below.fold_at(2);
        below.apply_edit(5, 5, 5);
        assert!(below.is_collapsed(2));

        // Editing above it leaves a collapsed fold alone apart from shifting.
        let mut h = folds();
        h.fold_at(6);
        h.apply_edit(1, 1, 1);
        assert!(h.is_collapsed(6));
    }
}
