//! Find and replace over text, with VS Code's three toggles: match case, whole word, regex.

use regex::{Regex, RegexBuilder};

/// Above this many matches a query stops collecting, to keep huge files responsive.
pub const MAX_MATCHES: usize = 100_000;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SearchOptions {
    pub case_sensitive: bool,
    pub whole_word: bool,
    pub regex: bool,
}

#[derive(Debug, Clone)]
pub struct Finder {
    regex: Regex,
    literal_replacement: bool,
}

/// A match in char offsets.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Match {
    pub start: usize,
    pub end: usize,
}

impl Finder {
    /// `Err` carries a message suitable for showing next to the search box.
    pub fn new(query: &str, options: SearchOptions) -> Result<Self, String> {
        if query.is_empty() {
            return Err("empty query".to_owned());
        }
        let body = if options.regex { query.to_owned() } else { regex::escape(query) };
        let pattern = if options.whole_word { format!(r"\b(?:{body})\b") } else { body };
        let regex = RegexBuilder::new(&pattern)
            .case_insensitive(!options.case_sensitive)
            .multi_line(true)
            .build()
            .map_err(|e| e.to_string().lines().last().unwrap_or("invalid regular expression").trim().to_owned())?;
        Ok(Self { regex, literal_replacement: !options.regex })
    }

    /// All non-empty matches in `text`, as char offsets, capped at [`MAX_MATCHES`].
    pub fn find_all(&self, text: &str) -> Vec<Match> {
        let mut out = Vec::new();
        let mut converter = CharOffsets::new(text);
        for m in self.regex.find_iter(text) {
            if m.start() == m.end() {
                continue;
            }
            out.push(Match { start: converter.at(m.start()), end: converter.at(m.end()) });
            if out.len() >= MAX_MATCHES {
                break;
            }
        }
        out
    }

    /// Every match with its replacement text. In regex mode `$1` / `${name}` expand to
    /// capture groups; otherwise the replacement is literal.
    pub fn replacements(&self, text: &str, replacement: &str) -> Vec<(usize, usize, String)> {
        let mut out = Vec::new();
        let mut converter = CharOffsets::new(text);
        for caps in self.regex.captures_iter(text) {
            let Some(m) = caps.get(0) else { continue };
            if m.start() == m.end() {
                continue;
            }
            let mut expanded = String::new();
            if self.literal_replacement {
                expanded.push_str(replacement);
            } else {
                caps.expand(replacement, &mut expanded);
            }
            out.push((converter.at(m.start()), converter.at(m.end()), expanded));
            if out.len() >= MAX_MATCHES {
                break;
            }
        }
        out
    }
}

/// Convert ascending byte offsets to char offsets in one pass.
struct CharOffsets<'a> {
    text: &'a str,
    byte: usize,
    chars: usize,
}

impl<'a> CharOffsets<'a> {
    fn new(text: &'a str) -> Self {
        Self { text, byte: 0, chars: 0 }
    }

    fn at(&mut self, byte: usize) -> usize {
        self.chars += self.text[self.byte..byte].chars().count();
        self.byte = byte;
        self.chars
    }
}

/// Index of the match to jump to from `from`: the first starting at or after it,
/// wrapping to the first match. `forward = false` finds the last one ending at or before.
pub fn next_match(matches: &[Match], from: usize, forward: bool) -> Option<usize> {
    if matches.is_empty() {
        return None;
    }
    if forward {
        matches.iter().position(|m| m.start >= from).or(Some(0))
    } else {
        matches.iter().rposition(|m| m.end <= from).or(Some(matches.len() - 1))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn opts(case: bool, word: bool, regex: bool) -> SearchOptions {
        SearchOptions { case_sensitive: case, whole_word: word, regex }
    }

    fn spans(finder: &Finder, text: &str) -> Vec<(usize, usize)> {
        finder.find_all(text).into_iter().map(|m| (m.start, m.end)).collect()
    }

    #[test]
    fn plain_search_is_literal_and_case_insensitive_by_default() {
        let f = Finder::new("a.b", opts(false, false, false)).unwrap();
        assert_eq!(spans(&f, "A.B axb a.b"), vec![(0, 3), (8, 11)], "the dot is not a wildcard");
        let cs = Finder::new("a.b", opts(true, false, false)).unwrap();
        assert_eq!(spans(&cs, "A.B a.b"), vec![(4, 7)]);
    }

    #[test]
    fn whole_word_and_regex_modes() {
        let w = Finder::new("cat", opts(false, true, false)).unwrap();
        assert_eq!(spans(&w, "cat concat cat_x cat"), vec![(0, 3), (17, 20)]);
        let r = Finder::new(r"\d+", opts(false, false, true)).unwrap();
        assert_eq!(spans(&r, "a1 b22 c333"), vec![(1, 2), (4, 6), (8, 11)]);
        let lines = Finder::new(r"^x", opts(true, false, true)).unwrap();
        assert_eq!(spans(&lines, "x\nax\nx"), vec![(0, 1), (5, 6)], "^ matches at each line start");
    }

    #[test]
    fn offsets_are_chars_not_bytes() {
        let f = Finder::new("b", opts(true, false, false)).unwrap();
        assert_eq!(spans(&f, "é😀b"), vec![(2, 3)]);
    }

    #[test]
    fn invalid_regex_and_empty_query_are_errors_not_panics() {
        assert!(Finder::new("(", opts(false, false, true)).is_err());
        assert!(Finder::new("", opts(false, false, false)).is_err());
        // The same text in plain mode is fine.
        assert!(Finder::new("(", opts(false, false, false)).is_ok());
    }

    #[test]
    fn empty_matches_are_skipped() {
        let f = Finder::new("x*", opts(true, false, true)).unwrap();
        assert_eq!(spans(&f, "axxb"), vec![(1, 3)]);
    }

    #[test]
    fn replacements_expand_groups_only_in_regex_mode() {
        let re = Finder::new(r"(\w+)@(\w+)", opts(false, false, true)).unwrap();
        let reps = re.replacements("a@b c@d", "$2:$1");
        assert_eq!(reps, vec![(0, 3, "b:a".to_owned()), (4, 7, "d:c".to_owned())]);
        let lit = Finder::new("a", opts(false, false, false)).unwrap();
        assert_eq!(lit.replacements("a", "$1")[0].2, "$1");
    }

    #[test]
    fn next_match_wraps_in_both_directions() {
        let m = |s, e| Match { start: s, end: e };
        let all = [m(0, 1), m(5, 6), m(9, 10)];
        assert_eq!(next_match(&all, 1, true), Some(1));
        assert_eq!(next_match(&all, 10, true), Some(0), "wraps to the first");
        assert_eq!(next_match(&all, 6, false), Some(1));
        assert_eq!(next_match(&all, 0, false), Some(2), "wraps to the last");
        assert_eq!(next_match(&[], 0, true), None);
    }
}
