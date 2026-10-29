//! Snippets in the editor: Tab expands a prefix and walks the tab stops, and the
//! "Insert Snippet..." command lists what applies to the current language.

use forge_core::settings::config_dir;
use forge_core::snippet::{civil_date, expand, parse_snippet_file, Context as SnippetContext, Snippet};
use forge_ext_host::Extension;
use forge_text::{map_offset_through_edits, Selection, SelectionSet, TextEdit};
use gpui::Context;

use crate::workspace::{Item, ItemAction, Workspace};

pub struct SnippetEntry {
    /// Language ids it applies to; empty means every language.
    pub languages: Vec<String>,
    pub snippet: Snippet,
    pub source: String,
}

/// An expansion in progress: the stops still to visit, as absolute char ranges that follow edits.
pub struct SnippetSession {
    pub doc: usize,
    pub stops: Vec<Vec<(usize, usize)>>,
    pub current: usize,
}

/// Snippets from installed extensions, and from the user's `snippets` folder
/// (`<language id>.json` for one language, `*.code-snippets` for all).
pub fn load_snippets(extensions: &[Extension]) -> Vec<SnippetEntry> {
    let mut entries = Vec::new();
    for extension in extensions {
        for contribution in &extension.manifest.contributes.snippets {
            let Some(path) = extension.resolve(&contribution.path) else { continue };
            let Ok(text) = std::fs::read_to_string(path) else { continue };
            let Ok(snippets) = parse_snippet_file(&text) else { continue };
            for snippet in snippets {
                entries.push(SnippetEntry { languages: contribution.language.iter().cloned().collect(), snippet, source: extension.display_name() });
            }
        }
    }
    if let Ok(dir) = std::fs::read_dir(config_dir().join("snippets")) {
        let mut files: Vec<_> = dir.flatten().map(|e| e.path()).collect();
        files.sort();
        for path in files {
            let stem = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
            let languages = match path.extension().and_then(|e| e.to_str()) {
                Some("json") => vec![stem.clone()],
                Some("code-snippets") => Vec::new(),
                _ => continue,
            };
            let Ok(text) = std::fs::read_to_string(&path) else { continue };
            for snippet in parse_snippet_file(&text).unwrap_or_default() {
                entries.push(SnippetEntry { languages: languages.clone(), snippet, source: "your snippets".to_owned() });
            }
        }
    }
    entries
}

fn is_word(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// Whether `prefix` ends `before` and starts a word, so `x` does not fire inside `max`.
/// A prefix that itself starts with a symbol (`@x`, `-`) may follow anything.
fn prefix_matches(before: &[char], prefix: &str) -> bool {
    let p: Vec<char> = prefix.chars().collect();
    if p.is_empty() || p.len() > before.len() || before[before.len() - p.len()..] != p[..] {
        return false;
    }
    before.len() == p.len() || !is_word(before[before.len() - p.len() - 1]) || !is_word(p[0])
}

impl Workspace {
    fn active_language_id(&self) -> Option<String> {
        let doc = self.active_doc()?;
        doc.language.map(|l| l.language_id().to_owned()).or_else(|| doc.ext_lang.as_ref().map(|l| l.id.clone()))
    }

    fn snippet_applies(&self, entry: &SnippetEntry, language: &Option<String>) -> bool {
        entry.languages.is_empty() || language.as_ref().is_some_and(|l| entry.languages.iter().any(|e| e == l))
    }

    pub fn snippet_items(&self, query: &str) -> Vec<Item> {
        let language = self.active_language_id();
        let query = query.to_lowercase();
        self.snippets
            .iter()
            .enumerate()
            .filter(|(_, e)| self.snippet_applies(e, &language))
            .filter(|(_, e)| query.is_empty() || e.snippet.name.to_lowercase().contains(&query) || e.snippet.prefixes.iter().any(|p| p.to_lowercase().contains(&query)))
            .map(|(i, e)| Item {
                label: format!("{}  ({})", e.snippet.prefixes[0], e.snippet.name),
                detail: if e.snippet.description.is_empty() { e.source.clone() } else { e.snippet.description.clone() },
                positions: Vec::new(),
                action: ItemAction::Snippet(i),
            })
            .collect()
    }

    /// Insert snippet `index` at the selection.
    pub fn insert_snippet_entry(&mut self, index: usize, cx: &mut Context<Self>) {
        let Some(entry) = self.snippets.get(index) else { return };
        let snippet = entry.snippet.clone();
        let Some(view) = self.active_view() else { return };
        let primary = view.selections.primary();
        self.insert_snippet(&snippet, (primary.start(), primary.end()), cx);
    }

    /// Replace `range` (char offsets) with the expansion of `snippet` and start visiting its stops.
    pub fn insert_snippet(&mut self, snippet: &Snippet, range: (usize, usize), cx: &mut Context<Self>) {
        let Some(doc) = self.active_doc() else { return };
        let (line, col) = doc.buffer.offset_to_point(range.0);
        let line_start = doc.buffer.line_start(line);
        let before: String = doc.buffer.rope().slice(line_start..range.0).to_string();
        let current_line = doc.buffer.rope().line(line).to_string().trim_end_matches(['\n', '\r']).to_owned();
        let _ = col;
        let seconds = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0);
        let ext = doc.ext_lang.as_ref().map(|l| &l.config);
        let context = SnippetContext {
            file_path: doc.path.to_string_lossy().into_owned(),
            workspace_name: self.root().file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
            selected_text: doc.buffer.rope().slice(range.0..range.1).to_string(),
            current_line,
            line_comment: doc.language.and_then(|l| l.line_comment()).map(str::to_owned).or_else(|| ext.and_then(|c| c.line_comment.clone())),
            block_comment: doc.language.and_then(|l| l.block_comment()).map(|(o, c)| (o.to_owned(), c.to_owned())).or_else(|| ext.and_then(|c| c.block_comment.clone())),
            indent: before.chars().take_while(|c| *c == ' ' || *c == '\t').collect(),
            date: civil_date(seconds),
        };
        let expanded = expand(&snippet.body, &context);
        let start = range.0;
        let first = expanded.stops[0].clone();
        let text = expanded.text.clone();
        self.mutate(cx, move |buffer, _| {
            buffer.insert(&SelectionSet::single(Selection { anchor: range.0, head: range.1 }), &text);
            SelectionSet::new(first.iter().map(|(a, b)| Selection { anchor: start + a, head: start + b }).collect())
        });
        self.snippet_session = None;
        // A snippet with a single stop (just its end) has nothing left to visit.
        if expanded.stops.len() > 1 {
            if let Some(doc) = self.active_ix().map(|(g, vi)| self.groups[g].views[vi].doc) {
                let stops = expanded.stops.iter().map(|ranges| ranges.iter().map(|(a, b)| (start + a, start + b)).collect()).collect();
                self.snippet_session = Some(SnippetSession { doc, stops, current: 0 });
            }
        }
    }

    /// The word-like prefix before a lone caret that names a snippet for this language.
    fn prefix_at_caret(&self) -> Option<(usize, (usize, usize))> {
        let view = self.active_view()?;
        let doc = self.active_doc()?;
        let selection = view.selections.primary();
        if view.selections.as_slice().len() != 1 || !selection.is_empty() {
            return None;
        }
        let caret = selection.head;
        let (line, _) = doc.buffer.offset_to_point(caret);
        let before: Vec<char> = doc.buffer.rope().slice(doc.buffer.line_start(line)..caret).chars().collect();
        let language = self.active_language_id();
        let mut best: Option<(usize, usize)> = None;
        for (index, entry) in self.snippets.iter().enumerate().filter(|(_, e)| self.snippet_applies(e, &language)) {
            for prefix in &entry.snippet.prefixes {
                let len = prefix.chars().count();
                if prefix_matches(&before, prefix) && best.is_none_or(|(_, best_len)| len > best_len) {
                    best = Some((index, len));
                }
            }
        }
        best.map(|(index, len)| (index, (caret - len, caret)))
    }

    /// Tab (or Shift+Tab when `back`) with snippets: visit the next stop of a running snippet,
    /// or expand a prefix at the caret. Returns whether the key was used.
    pub fn snippet_tab(&mut self, back: bool, cx: &mut Context<Self>) -> bool {
        if let Some(session) = &self.snippet_session {
            let active_doc = self.active_ix().map(|(g, vi)| self.groups[g].views[vi].doc);
            if active_doc != Some(session.doc) {
                self.snippet_session = None;
            }
        }
        if let Some(session) = self.snippet_session.as_mut() {
            let next = if back { session.current.checked_sub(1) } else { Some(session.current + 1).filter(|n| *n < session.stops.len()) };
            let Some(next) = next else { return false };
            session.current = next;
            let ranges = session.stops[next].clone();
            if next + 1 == session.stops.len() {
                // The last stop ends the snippet.
                self.snippet_session = None;
            }
            if let Some((g, vi)) = self.active_ix() {
                self.groups[g].views[vi].selections = SelectionSet::new(ranges.iter().map(|(a, b)| Selection { anchor: *a, head: *b }).collect());
                self.reveal_caret(g, vi, false);
            }
            cx.notify();
            return true;
        }
        if back {
            return false;
        }
        let Some((index, range)) = self.prefix_at_caret() else { return false };
        let snippet = self.snippets[index].snippet.clone();
        self.insert_snippet(&snippet, range, cx);
        true
    }

    /// Keep a running snippet's stops attached to the text as `edits` change it.
    pub fn snippet_follow(&mut self, doc_ix: usize, edits: &[TextEdit]) {
        let Some(session) = self.snippet_session.as_mut() else { return };
        if session.doc != doc_ix {
            return;
        }
        for ranges in &mut session.stops {
            for (start, end) in ranges.iter_mut() {
                *start = map_offset_through_edits(*start, edits);
                *end = map_offset_through_edits(*end, edits).max(*start);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn matches(before: &str, prefix: &str) -> bool {
        prefix_matches(&before.chars().collect::<Vec<_>>(), prefix)
    }

    #[test]
    fn a_prefix_must_end_the_text_and_start_a_word() {
        assert!(matches("cl", "cl"));
        assert!(matches("  cl", "cl"));
        assert!(matches("(cl", "cl"), "a bracket is a boundary");
        assert!(matches("x.cl", "cl"), "so is a dot");
        assert!(!matches("excl", "cl"), "not in the middle of a word");
        assert!(!matches("cl ", "cl"), "the caret must be right after it");
        assert!(!matches("c", "cl"));
        assert!(!matches("anything", ""));
    }

    #[test]
    fn symbol_prefixes_may_follow_a_word() {
        assert!(matches("name@x", "@x"));
        assert!(matches("a-b", "-b"));
        assert!(!matches("ab", "@x"));
    }
}
