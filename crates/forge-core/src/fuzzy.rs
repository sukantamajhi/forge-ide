//! Fuzzy matching for Quick Open, the command palette and symbol lists.

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FuzzyMatch {
    pub score: i32,
    /// Char positions in the candidate that matched, for highlighting.
    pub positions: Vec<usize>,
}

fn is_boundary(prev: Option<char>, c: char) -> bool {
    match prev {
        None => true,
        Some(p) => !p.is_alphanumeric() || (p.is_lowercase() && c.is_uppercase()),
    }
}

/// Case-insensitive subsequence match scored for consecutive runs, word starts and
/// compactness. `None` when `query` is not a subsequence of `candidate`.
pub fn fuzzy_match(query: &str, candidate: &str) -> Option<FuzzyMatch> {
    let q: Vec<char> = query.chars().filter(|c| !c.is_whitespace()).flat_map(char::to_lowercase).collect();
    if q.is_empty() {
        return Some(FuzzyMatch { score: 0, positions: Vec::new() });
    }
    let cand: Vec<char> = candidate.chars().collect();
    let lower: Vec<char> = cand.iter().flat_map(|c| c.to_lowercase().next()).collect();
    if lower.len() != cand.len() || lower.len() < q.len() {
        return None;
    }

    const NEG: i32 = i32::MIN / 2;
    let (n, m) = (cand.len(), q.len());
    // best[i][j]: best score matching q[..=i] with q[i] placed at candidate position j.
    let mut best = vec![vec![NEG; n]; m];
    let mut from = vec![vec![usize::MAX; n]; m];
    for j in 0..n {
        if lower[j] == q[0] {
            let boundary = is_boundary(j.checked_sub(1).map(|p| cand[p]), cand[j]);
            best[0][j] = 1 + if boundary { 8 } else { 0 } + if j == 0 { 6 } else { 0 } - (j as i32).min(10);
        }
    }
    for i in 1..m {
        for j in i..n {
            if lower[j] != q[i] {
                continue;
            }
            let boundary = is_boundary(Some(cand[j - 1]), cand[j]);
            for k in i - 1..j {
                if best[i - 1][k] == NEG {
                    continue;
                }
                let gap = (j - k - 1) as i32;
                // A contiguous run must beat skipping ahead to a word start.
                let bonus = if gap == 0 { 10 } else { -(gap * 2).min(8) } + if boundary { 8 } else { 0 };
                let score = best[i - 1][k] + 1 + bonus;
                if score > best[i][j] {
                    best[i][j] = score;
                    from[i][j] = k;
                }
            }
        }
    }
    let (end, score) = (0..n).map(|j| (j, best[m - 1][j])).filter(|(_, s)| *s != NEG).max_by_key(|(j, s)| (*s, std::cmp::Reverse(*j)))?;
    let mut positions = vec![0; m];
    let mut j = end;
    for i in (0..m).rev() {
        positions[i] = j;
        if i > 0 {
            j = from[i][j];
        }
    }
    // Prefer shorter candidates on otherwise equal scores.
    Some(FuzzyMatch { score: score * 4 - (n as i32 / 8), positions })
}

/// Indices of `items` matching `query`, best first (stable for ties), at most `limit`.
/// Words of at least three chars in `text` that start with `prefix` (case-insensitively) and are longer than it,
/// most frequent first, then alphabetical, at most `limit`. The word being typed (exactly `prefix`) is not offered.
pub fn words_with_prefix(text: &str, prefix: &str, limit: usize) -> Vec<String> {
    let prefix_lower = prefix.to_lowercase();
    let mut counts: std::collections::HashMap<&str, usize> = std::collections::HashMap::new();
    for word in text.split(|c: char| !(c.is_alphanumeric() || c == '_')) {
        if word.chars().count() >= 3 && word.chars().count() > prefix.chars().count() && word.to_lowercase().starts_with(&prefix_lower) {
            *counts.entry(word).or_default() += 1;
        }
    }
    let mut words: Vec<(&str, usize)> = counts.into_iter().collect();
    words.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(b.0)));
    words.into_iter().take(limit).map(|(w, _)| w.to_owned()).collect()
}

pub fn rank<S: AsRef<str>>(query: &str, items: &[S], limit: usize) -> Vec<(usize, FuzzyMatch)> {
    let mut hits: Vec<(usize, FuzzyMatch)> =
        items.iter().enumerate().filter_map(|(i, s)| fuzzy_match(query, s.as_ref()).map(|m| (i, m))).collect();
    hits.sort_by(|a, b| b.1.score.cmp(&a.1.score).then(a.0.cmp(&b.0)));
    hits.truncate(limit);
    hits
}

#[cfg(test)]
mod word_tests {
    use super::words_with_prefix;

    #[test]
    fn words_are_offered_by_frequency_and_never_the_typed_one() {
        let text = "render render render_line renderer rend ren Render xrender";
        assert_eq!(words_with_prefix(text, "ren", 10), vec!["render", "Render", "rend", "render_line", "renderer"]);
        assert_eq!(words_with_prefix(text, "render_", 10), vec!["render_line"]);
        assert_eq!(words_with_prefix(text, "REND", 2), vec!["render", "Render"]);
        assert!(words_with_prefix(text, "zzz", 10).is_empty());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn top(query: &str, items: &[&str]) -> Vec<String> {
        rank(query, items, 10).into_iter().map(|(i, _)| items[i].to_owned()).collect()
    }

    #[test]
    fn requires_a_subsequence_case_insensitively() {
        assert!(fuzzy_match("abc", "xaxbxc").is_some());
        assert!(fuzzy_match("ABC", "abc").is_some());
        assert!(fuzzy_match("abd", "abc").is_none());
        assert!(fuzzy_match("abcd", "abc").is_none());
        assert_eq!(fuzzy_match("", "anything").unwrap().positions, Vec::<usize>::new());
    }

    #[test]
    fn positions_point_at_the_matched_chars() {
        let m = fuzzy_match("fb", "foo_bar").unwrap();
        assert_eq!(m.positions, vec![0, 4], "prefers the start of each word");
    }

    #[test]
    fn word_starts_and_exact_runs_rank_first() {
        let files = ["src/workspace.rs", "src/main.rs", "docs/ARCHITECTURE.md", "crates/forge-app/src/main.rs"];
        assert_eq!(top("main", &files)[0], "src/main.rs", "shorter path wins an equal run");
        assert_eq!(top("arch", &files)[0], "docs/ARCHITECTURE.md");
        let cmds = ["Toggle Terminal", "Toggle Tab Size", "Format Document", "Find in Files"];
        assert_eq!(top("tt", &cmds)[0], "Toggle Terminal");
        assert_eq!(top("fd", &cmds)[0], "Format Document");
        assert_eq!(top("fif", &cmds)[0], "Find in Files");
    }

    #[test]
    fn a_contiguous_run_beats_hopping_to_a_later_word_start() {
        // Regression: "main" matched m-a-i plus the `n` of "notes" and outscored main.rs.
        let names = ["main_notes.md", "main.rs"];
        assert_eq!(top("main", &names)[0], "main.rs");
        assert_eq!(fuzzy_match("main", "main_notes.md").unwrap().positions, vec![0, 1, 2, 3]);
    }

    #[test]
    fn camel_case_boundaries_count_as_word_starts() {
        let items = ["fooBarBaz", "foobarbaz"];
        assert_eq!(top("fbb", &items)[0], "fooBarBaz");
    }

    #[test]
    fn limit_and_stable_order_for_ties() {
        let items = ["a1", "a2", "a3"];
        let r = rank("a", &items, 2);
        assert_eq!(r.iter().map(|(i, _)| *i).collect::<Vec<_>>(), vec![0, 1]);
    }

    #[test]
    fn unicode_candidates_do_not_panic() {
        assert!(fuzzy_match("é", "Café au lait").is_some());
        assert!(fuzzy_match("😀", "a😀b").is_some());
    }
}
