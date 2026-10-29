//! The state behind the Find / Replace widget.

use forge_text::search::{next_match, Finder, Match, SearchOptions};

use crate::input::LineInput;

#[derive(Debug, Default)]
pub struct FindState {
    pub open: bool,
    pub replace_visible: bool,
    pub query: LineInput,
    pub replacement: LineInput,
    pub options: SearchOptions,
    /// Whether keystrokes go to the replace field.
    pub focus_replace: bool,
    pub matches: Vec<Match>,
    pub current: Option<usize>,
    pub error: Option<String>,
    finder: Option<Finder>,
}

impl FindState {
    /// Recompute matches for `text`; the current match becomes the first one at or after `from`.
    pub fn refresh(&mut self, text: &str, from: usize) {
        self.error = None;
        self.finder = None;
        self.matches.clear();
        self.current = None;
        if self.query.text().is_empty() {
            return;
        }
        match Finder::new(self.query.text(), self.options) {
            Ok(finder) => {
                self.matches = finder.find_all(text);
                self.finder = Some(finder);
                self.current = next_match(&self.matches, from, true);
            }
            Err(message) => self.error = Some(message),
        }
    }

    /// Move to the next or previous match relative to `from` (wrapping) and return it.
    pub fn step(&mut self, forward: bool, from: usize) -> Option<Match> {
        // Stepping from inside the current match must leave it, not stay on it.
        let anchor = match (self.current.and_then(|i| self.matches.get(i)), forward) {
            (Some(m), true) if m.start == from || (m.start..m.end).contains(&from) => m.end.max(from),
            (Some(m), false) if m.end == from || (m.start..m.end).contains(&from) => m.start.min(from),
            _ => from,
        };
        let index = next_match(&self.matches, anchor, forward)?;
        self.current = Some(index);
        self.matches.get(index).copied()
    }

    pub fn current_match(&self) -> Option<Match> {
        self.current.and_then(|i| self.matches.get(i)).copied()
    }

    /// The edit that replaces the current match, with capture groups expanded in regex mode.
    pub fn replace_current_edit(&self, text: &str) -> Option<(usize, usize, String)> {
        let finder = self.finder.as_ref()?;
        let current = self.current_match()?;
        finder.replacements(text, self.replacement.text()).into_iter().find(|(s, e, _)| (*s, *e) == (current.start, current.end))
    }

    pub fn replace_all_edits(&self, text: &str) -> Vec<(usize, usize, String)> {
        self.finder.as_ref().map(|f| f.replacements(text, self.replacement.text())).unwrap_or_default()
    }

    /// `"3 of 12"`, `"No results"`, or the regex error.
    pub fn status_label(&self) -> String {
        if let Some(error) = &self.error {
            return error.clone();
        }
        if self.query.text().is_empty() {
            return String::new();
        }
        match self.current {
            Some(i) => format!("{} of {}", i + 1, self.matches.len()),
            None => "No results".to_owned(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state(query: &str, text: &str, from: usize) -> FindState {
        let mut s = FindState { query: LineInput::new(query), ..FindState::default() };
        s.refresh(text, from);
        s
    }

    #[test]
    fn refresh_selects_the_first_match_at_or_after_the_cursor() {
        let s = state("ab", "ab xx ab yy ab", 4);
        assert_eq!(s.matches.len(), 3);
        assert_eq!(s.current_match().unwrap().start, 6);
        assert_eq!(s.status_label(), "2 of 3");
        let past = state("ab", "ab xx ab", 7);
        assert_eq!(past.current_match().unwrap().start, 0, "wraps around");
    }

    #[test]
    fn stepping_moves_off_the_current_match_both_ways_and_wraps() {
        let mut s = state("ab", "ab ab ab", 0);
        assert_eq!(s.step(true, 0).unwrap().start, 3, "from the selected first match, next is the second");
        assert_eq!(s.step(true, 5).unwrap().start, 6);
        assert_eq!(s.step(true, 8).unwrap().start, 0, "wraps");
        assert_eq!(s.step(false, 0).unwrap().start, 6, "wraps backwards");
        assert_eq!(s.step(false, 6).unwrap().start, 3);
    }

    #[test]
    fn no_results_empty_query_and_regex_errors_are_reported() {
        assert_eq!(state("zzz", "abc", 0).status_label(), "No results");
        assert_eq!(state("", "abc", 0).status_label(), "");
        let mut bad = FindState::default();
        bad.options.regex = true;
        bad.query = LineInput::new("(");
        bad.refresh("abc", 0);
        assert!(bad.error.is_some() && bad.matches.is_empty());
        assert!(!bad.status_label().is_empty());
    }

    #[test]
    fn options_change_what_matches() {
        let mut s = state("Cat", "cat Cat concat", 0);
        assert_eq!(s.matches.len(), 3);
        s.options.case_sensitive = true;
        s.refresh("cat Cat concat", 0);
        assert_eq!(s.matches.len(), 1);
        s.options = SearchOptions { case_sensitive: false, whole_word: true, regex: false };
        s.refresh("cat Cat concat", 0);
        assert_eq!(s.matches.len(), 2);
    }

    #[test]
    fn replacements_for_the_current_match_and_for_all() {
        let mut s = state(r"(\w+)-(\w+)", "a-b c-d", 4);
        s.options.regex = true;
        s.replacement = LineInput::new("$2+$1");
        s.refresh("a-b c-d", 4);
        assert_eq!(s.replace_current_edit("a-b c-d"), Some((4, 7, "d+c".to_owned())));
        assert_eq!(s.replace_all_edits("a-b c-d"), vec![(0, 3, "b+a".to_owned()), (4, 7, "d+c".to_owned())]);

        let mut literal = state("a", "aaa", 0);
        literal.replacement = LineInput::new("$1");
        assert_eq!(literal.replace_all_edits("aaa").len(), 3);
        assert_eq!(literal.replace_all_edits("aaa")[0].2, "$1", "no capture expansion outside regex mode");
    }
}
