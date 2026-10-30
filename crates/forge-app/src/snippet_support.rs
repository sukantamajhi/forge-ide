//! Snippets in the editor: Tab expands a prefix and walks the tab stops, and the
//! "Insert Snippet..." command lists what applies to the current language.

use forge_core::settings::config_dir;
use forge_core::snippet::{expand, local_now, parse_snippet_file, Context as SnippetContext, Snippet};
use forge_lsp::CompletionItem;
use forge_syntax::Language;
use forge_ext_host::Extension;
use forge_text::{map_offset_through_edits, Selection, SelectionSet, TextEdit};
use gpui::Context;

use crate::completion::{identifier_start, is_identifier_char, CompletionState};
use crate::workspace::{Item, ItemAction, Workspace};

/// A completion item made here rather than by a language server is told apart by its `sort_text`.
const SNIPPET_MARK: &str = "\u{1}snippet:";
const CHOICE_MARK: &str = "\u{1}choice";

pub enum ItemKind {
    /// Snippet number `n` of the snippets list.
    Snippet(usize),
    /// One option of a `${1|a,b|}` placeholder.
    Choice,
    Server,
}

pub fn item_kind(sort_text: &str) -> ItemKind {
    if let Some(n) = sort_text.strip_prefix(SNIPPET_MARK).and_then(|n| n.parse().ok()) {
        ItemKind::Snippet(n)
    } else if sort_text == CHOICE_MARK {
        ItemKind::Choice
    } else {
        ItemKind::Server
    }
}

const SNIPPET_TEMPLATE: &str = "{\n\t// Place your snippets here. Each snippet has a name, a prefix (what you type, then press Tab), a body and a description.\n\t// In a body, $1, $2 are tab stops (Tab moves to the next one, $0 is where the caret ends), ${1:text} is a placeholder,\n\t// ${1|one,two|} offers a list, and the same number twice edits both places. Variables: $TM_FILENAME, $CLIPBOARD,\n\t// $CURRENT_YEAR, $UUID, ${TM_FILENAME_BASE/(.*)/${1:/upcase}/} and more.\n\t// Example:\n\t// \"Print to console\": {\n\t//   \"prefix\": \"log\",\n\t//   \"body\": [\"console.log('$1');\", \"$0\"],\n\t//   \"description\": \"Log output to the console\"\n\t// }\n}\n";

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
    /// Placeholders that offer a list of options, following edits like the stops do.
    pub choices: Vec<((usize, usize), Vec<String>)>,
}

/// Snippets from installed extensions, and from the user's `snippets` folder
/// (`<language id>.json` for one language, `*.code-snippets` for all).
/// The snippets Forge ships (`assets/snippets`): the language ids each file applies to (none means every language).
const BUILTIN: &[(&[&str], &str, &str)] = &[
    (&[], "all languages", include_str!("../../../assets/snippets/global.json")),
    (&["rust"], "Rust", include_str!("../../../assets/snippets/rust.json")),
    (&["python"], "Python", include_str!("../../../assets/snippets/python.json")),
    (&["javascript", "javascriptreact", "typescript", "typescriptreact"], "JavaScript", include_str!("../../../assets/snippets/javascript.json")),
    (&["typescript", "typescriptreact"], "TypeScript", include_str!("../../../assets/snippets/typescript.json")),
    (&["javascriptreact", "typescriptreact"], "React", include_str!("../../../assets/snippets/react.json")),
    (&["html"], "HTML", include_str!("../../../assets/snippets/html.json")),
    (&["css"], "CSS", include_str!("../../../assets/snippets/css.json")),
    (&["markdown"], "Markdown", include_str!("../../../assets/snippets/markdown.json")),
    (&["go"], "Go", include_str!("../../../assets/snippets/go.json")),
    (&["c", "cpp"], "C", include_str!("../../../assets/snippets/c.json")),
    (&["cpp"], "C++", include_str!("../../../assets/snippets/cpp.json")),
    (&["java"], "Java", include_str!("../../../assets/snippets/java.json")),
    (&["csharp"], "C#", include_str!("../../../assets/snippets/csharp.json")),
    (&["php"], "PHP", include_str!("../../../assets/snippets/php.json")),
    (&["ruby"], "Ruby", include_str!("../../../assets/snippets/ruby.json")),
    (&["lua"], "Lua", include_str!("../../../assets/snippets/lua.json")),
    (&["sql"], "SQL", include_str!("../../../assets/snippets/sql.json")),
    (&["shellscript"], "Shell", include_str!("../../../assets/snippets/shellscript.json")),
    (&["yaml"], "YAML", include_str!("../../../assets/snippets/yaml.json")),
    (&["toml"], "TOML", include_str!("../../../assets/snippets/toml.json")),
    (&["json"], "JSON", include_str!("../../../assets/snippets/json.json")),
    (&["dockerfile"], "Dockerfile", include_str!("../../../assets/snippets/dockerfile.json")),
    (&["makefile"], "Makefile", include_str!("../../../assets/snippets/makefile.json")),
];

pub fn builtin_snippets() -> Vec<SnippetEntry> {
    let mut entries = Vec::new();
    for (languages, label, text) in BUILTIN {
        for snippet in parse_snippet_file(text).unwrap_or_default() {
            entries.push(SnippetEntry { languages: languages.iter().map(|l| (*l).to_owned()).collect(), snippet, source: format!("built-in, {label}") });
        }
    }
    entries
}

pub fn load_snippets(extensions: &[Extension], builtin: bool) -> Vec<SnippetEntry> {
    let mut entries = if builtin { builtin_snippets() } else { Vec::new() };
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
        let now = local_now();
        let ext = doc.ext_lang.as_ref().map(|l| &l.config);
        let rope = doc.buffer.rope();
        let word_start = identifier_start(&doc.buffer, range.0);
        let mut word_end = range.0;
        while word_end < rope.len_chars() && is_identifier_char(rope.char(word_end)) {
            word_end += 1;
        }
        let current_word = rope.slice(word_start..word_end).to_string();
        let context = SnippetContext {
            file_path: doc.path.to_string_lossy().into_owned(),
            workspace_name: self.root().file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
            selected_text: doc.buffer.rope().slice(range.0..range.1).to_string(),
            current_line,
            line_comment: doc.language.and_then(|l| l.line_comment()).map(str::to_owned).or_else(|| ext.and_then(|c| c.line_comment.clone())),
            block_comment: doc.language.and_then(|l| l.block_comment()).map(|(o, c)| (o.to_owned(), c.to_owned())).or_else(|| ext.and_then(|c| c.block_comment.clone())),
            indent: before.chars().take_while(|c| *c == ' ' || *c == '\t').collect(),
            date: now.date,
            time: now.time,
            weekday: now.weekday,
            unix: now.unix,
            clipboard: cx.read_from_clipboard().and_then(|c| c.text()).unwrap_or_default(),
            current_word,
            line_index: line,
            relative_path: self.display_path(&doc.path),
            workspace_folder: self.root().to_string_lossy().into_owned(),
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
                let choices = expanded.choices.iter().map(|((a, b), options)| ((start + a, start + b), options.clone())).collect();
                self.snippet_session = Some(SnippetSession { doc, stops, current: 0, choices });
                self.show_snippet_choices();
            }
        }
    }

    /// When the stop being visited is a `${1|a,b|}` placeholder, list its options in the completion popup.
    fn show_snippet_choices(&mut self) {
        let Some(session) = &self.snippet_session else { return };
        let ranges = &session.stops[session.current];
        let options = session.choices.iter().find(|(range, _)| ranges.contains(range)).map(|(_, options)| options.clone());
        self.completion = options.map(|options| {
            CompletionState::new(
                options
                    .into_iter()
                    .map(|label| CompletionItem { label, detail: Some("choice".to_owned()), kind: Some(12), sort_text: Some(CHOICE_MARK.to_owned()), edit: None, insert_text: None })
                    .collect(),
            )
        });
    }

    /// Snippets whose prefix starts with the word being typed, as completion items.
    pub fn snippet_completion_items(&self, word: &str) -> Vec<CompletionItem> {
        if word.is_empty() || !self.settings.snippet_suggestions {
            return Vec::new();
        }
        let language = self.active_language_id();
        let word = word.to_lowercase();
        self.snippets
            .iter()
            .enumerate()
            .filter(|(_, e)| self.snippet_applies(e, &language))
            .filter_map(|(i, e)| {
                let prefix = e.snippet.prefixes.iter().find(|p| p.to_lowercase().starts_with(&word))?;
                let detail = if e.snippet.description.is_empty() { e.snippet.name.clone() } else { e.snippet.description.clone() };
                Some(CompletionItem { label: prefix.clone(), detail: Some(format!("snippet: {detail}")), kind: Some(15), sort_text: Some(format!("{SNIPPET_MARK}{i}")), edit: None, insert_text: None })
            })
            .take(30)
            .collect()
    }

    /// Open the popup with only snippets (no language server answered, or there is none).
    pub fn show_snippet_popup(&mut self) {
        let Some(view) = self.active_view() else { return };
        let Some(doc) = self.active_doc() else { return };
        let caret = view.selections.primary().head;
        let word = doc.buffer.rope().slice(identifier_start(&doc.buffer, caret)..caret).to_string();
        let items = self.snippet_completion_items(&word);
        self.completion = (!items.is_empty()).then(|| CompletionState::new(items));
    }

    /// The popup chose snippet `index`: it replaces the word before the caret.
    pub fn accept_snippet_item(&mut self, index: usize, cx: &mut Context<Self>) {
        let Some(snippet) = self.snippets.get(index).map(|e| e.snippet.clone()) else { return };
        let Some(view) = self.active_view() else { return };
        let Some(doc) = self.active_doc() else { return };
        let caret = view.selections.primary().head;
        let start = identifier_start(&doc.buffer, caret);
        self.insert_snippet(&snippet, (start, caret), cx);
    }

    /// The languages Configure User Snippets can make a file for.
    pub fn snippet_file_items(&self, query: &str) -> Vec<Item> {
        let query = query.to_lowercase();
        let dir = config_dir().join("snippets");
        let mut choices: Vec<(String, String, String)> = vec![("New Global Snippets file...".to_owned(), "global".to_owned(), "global.code-snippets".to_owned())];
        choices.extend(Language::ALL.iter().map(|l| (l.name().to_owned(), l.language_id().to_owned(), format!("{}.json", l.language_id()))));
        choices.push(("Plain Text".to_owned(), "plaintext".to_owned(), "plaintext.json".to_owned()));
        choices
            .into_iter()
            .filter(|(name, id, _)| query.is_empty() || name.to_lowercase().contains(&query) || id.contains(&query))
            .map(|(name, id, file)| {
                let detail = if dir.join(&file).exists() { format!("{file} (exists)") } else { file };
                Item { label: name, detail, positions: Vec::new(), action: ItemAction::SnippetFile(id) }
            })
            .collect()
    }

    /// Open the user's snippets file for `id` (a language id or `global`), creating it with an explanation first.
    pub fn open_snippet_file(&mut self, id: &str, cx: &mut Context<Self>) {
        let dir = config_dir().join("snippets");
        let file = if id == "global" { "global.code-snippets".to_owned() } else { format!("{id}.json") };
        let path = dir.join(file);
        if !path.exists() {
            if let Err(e) = std::fs::create_dir_all(&dir).and_then(|()| std::fs::write(&path, SNIPPET_TEMPLATE)) {
                self.set_status(format!("Cannot create {}: {e}", path.display()));
                return;
            }
        }
        self.open_file(&path, None, cx);
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
                // On a tie the later one wins: user snippets come after built-in ones.
                if prefix_matches(&before, prefix) && best.is_none_or(|(_, best_len)| len >= best_len) {
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
            self.completion = None;
            if next + 1 == session.stops.len() {
                // The last stop ends the snippet.
                self.snippet_session = None;
            }
            if let Some((g, vi)) = self.active_ix() {
                self.groups[g].views[vi].selections = SelectionSet::new(ranges.iter().map(|(a, b)| Selection { anchor: *a, head: *b }).collect());
                self.reveal_caret(g, vi, false);
            }
            self.show_snippet_choices();
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
        for ((start, end), _) in &mut session.choices {
            *start = map_offset_through_edits(*start, edits);
            *end = map_offset_through_edits(*end, edits).max(*start);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_builtin_file_parses_and_its_snippets_expand() {
        let all = builtin_snippets();
        assert!(all.len() > 250, "{} built-in snippets", all.len());
        let context = forge_core::snippet::Context { file_path: "C:/p/my_file.rs".to_owned(), clipboard: "x".to_owned(), ..Default::default() };
        for entry in &all {
            let expanded = expand(&entry.snippet.body, &context);
            assert!(!expanded.text.is_empty(), "{} expands to nothing", entry.snippet.name);
            assert!(!expanded.stops.is_empty());
            assert!(!entry.snippet.prefixes.iter().any(|p| p.contains(' ')), "{}: a prefix has a space", entry.snippet.name);
        }
        // Every language Forge knows has its own snippets, and "log" and "fn" are where people expect them.
        for id in ["rust", "python", "javascript", "typescript", "html", "css", "go", "c", "cpp", "java", "csharp", "php", "ruby", "lua", "sql", "shellscript", "markdown", "yaml", "toml", "json", "dockerfile", "makefile"] {
            assert!(all.iter().any(|e| e.languages.iter().any(|l| l == id)), "no snippets for {id}");
        }
        let has = |lang: &str, prefix: &str| all.iter().any(|e| e.languages.iter().any(|l| l == lang) && e.snippet.prefixes.iter().any(|p| p == prefix));
        assert!(has("rust", "fn") && has("python", "def") && has("typescript", "log") && has("typescriptreact", "rfc") && has("go", "iferr"));
        assert!(all.iter().any(|e| e.languages.is_empty() && e.snippet.prefixes.iter().any(|p| p == "todo")));
    }

    #[test]
    fn a_snippet_without_a_name_clash_between_files_of_one_language_is_not_needed() {
        // The same prefix may exist in several files (javascript and react both have none of each other's), but
        // a single file never repeats a prefix.
        for (_, label, text) in BUILTIN {
            let snippets = parse_snippet_file(text).unwrap();
            let mut seen = std::collections::BTreeSet::new();
            for s in &snippets {
                for p in &s.prefixes {
                    assert!(seen.insert(p.clone()), "{label}: prefix {p} appears twice");
                }
            }
        }
    }

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
