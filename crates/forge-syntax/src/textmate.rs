//! TextMate grammars, the format VS Code extensions ship, as a second highlighting backend.
//!
//! Tree-sitter remains the default for the languages Forge bundles. A grammar contributed by
//! an extension is tokenized line by line with `irosashi` (a port of vscode-textmate), and
//! each token's scope stack (`source.ts meta.block keyword.control`) is mapped onto
//! Forge's [`HighlightClass`] so it takes its colors from the active theme like everything else.
//!
//! The state at the end of every line is kept, so an edit re-tokenizes only from the changed line,
//! and lines are tokenized lazily, only as far as the viewport needs.

use std::cell::RefCell;
use std::collections::HashMap;
use std::ops::Range;
use std::sync::Arc;

use forge_text::TextEdit;
use irosashi::{Highlighter, HighlighterBuilder, ScopeListId, Session, StateStack, TokenizeOptions};
use ropey::Rope;

use crate::{HighlightClass, Span};

/// Larger files are shown unhighlighted: TextMate tokenizing is sequential from the top.
pub const MAX_TEXTMATE_BYTES: usize = 1024 * 1024;

/// Grammars loaded from extensions, shared by every document.
#[derive(Clone)]
pub struct TextMateRegistry {
    highlighter: Arc<Highlighter>,
}

impl Default for TextMateRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl TextMateRegistry {
    /// A registry with no grammars: nothing is bundled.
    pub fn new() -> Self {
        let highlighter = HighlighterBuilder::without_bundled_assets().build().expect("an empty registry always builds");
        Self { highlighter: Arc::new(highlighter) }
    }

    /// Register a `.tmLanguage.json` grammar under `name` (its scope name is the usual choice).
    pub fn add_grammar(&self, name: &str, json: &[u8]) -> Result<(), String> {
        self.highlighter.load_language(name, json.to_vec()).map_err(|e| e.to_string())
    }

    pub fn has_grammar(&self, name: &str) -> bool {
        self.highlighter.languages().iter().any(|l| l == name)
    }

    /// A fresh tokenizer for the grammar called `name`.
    pub fn syntax(&self, name: &str) -> Option<TextMateSyntax> {
        let session = self.highlighter.session(name).ok()?;
        Some(TextMateSyntax { name: name.to_owned(), inner: RefCell::new(Inner { session, states: Vec::new(), spans: Vec::new(), classes: HashMap::new() }) })
    }
}

struct Inner {
    session: Session,
    /// `states[i]` is the state before line `i`; one more than `spans` once started.
    states: Vec<StateStack>,
    /// The spans of every line tokenized so far.
    spans: Vec<Vec<Span>>,
    /// Scope stack to class, cached: most tokens repeat a few stacks.
    classes: HashMap<ScopeListId, Option<HighlightClass>>,
}

pub struct TextMateSyntax {
    name: String,
    inner: RefCell<Inner>,
}

impl TextMateSyntax {
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Forget everything, for when the whole text was replaced.
    pub fn reset(&mut self) {
        let inner = self.inner.get_mut();
        inner.spans.clear();
        inner.states.clear();
    }

    /// Note edits. Everything from the first changed line on is tokenized again when next needed.
    pub fn parse(&mut self, _rope: &Rope, edits: &[TextEdit]) {
        let inner = self.inner.get_mut();
        if let Some(first) = edits.iter().map(|e| e.start_point.0).min() {
            let keep = first.min(inner.spans.len());
            inner.spans.truncate(keep);
            inner.states.truncate(keep + 1);
        }
    }

    /// Highlight spans for `lines`, one `Vec` per line, in char columns.
    pub fn highlight_lines(&self, rope: &Rope, lines: Range<usize>) -> Vec<Vec<Span>> {
        let end = lines.end.min(rope.len_lines());
        let start = lines.start.min(end);
        if rope.len_bytes() > MAX_TEXTMATE_BYTES {
            return vec![Vec::new(); end - start];
        }
        let mut inner = self.inner.borrow_mut();
        if inner.states.is_empty() {
            let initial = inner.session.initial_state();
            inner.states.push(initial);
        }
        while inner.spans.len() < end {
            let row = inner.spans.len();
            inner.tokenize_row(rope, row);
        }
        inner.spans[start..end].to_vec()
    }
}

impl Inner {
    fn tokenize_row(&mut self, rope: &Rope, row: usize) {
        let mut text = rope.line(row).to_string();
        while text.ends_with(['\n', '\r']) {
            text.pop();
        }
        let before = self.states[row].clone();
        let result = self.session.tokenize_line(&text, &before, row == 0, TokenizeOptions::default());

        let mut spans: Vec<Span> = Vec::new();
        // Tokens arrive in order, so byte offsets are converted to columns in one pass.
        let (mut byte, mut col) = (0usize, 0usize);
        let mut advance = |to: usize| {
            col += text[byte..to].chars().count();
            byte = to;
            col
        };
        for token in &result.tokens {
            let class = match self.classes.get(&token.scopes) {
                Some(class) => *class,
                None => {
                    let class = class_for_scopes(&self.session.scope_names(token.scopes));
                    self.classes.insert(token.scopes, class);
                    class
                }
            };
            let (from, to) = (token.start.min(text.len()), token.end.min(text.len()));
            if to <= from {
                continue;
            }
            let (start_col, end_col) = (advance(from), advance(to));
            let Some(class) = class else { continue };
            match spans.last_mut() {
                Some(prev) if prev.end_col == start_col && prev.class == class => prev.end_col = end_col,
                _ => spans.push(Span { start_col, end_col, class }),
            }
        }
        self.spans.push(spans);
        self.states.push(result.state);
    }
}

/// The class for a token with the given scope stack (outermost first), or `None` for plain text.
/// The innermost scope that means something wins; among a scope's prefixes the longest does.
pub fn class_for_scopes(scopes: &[&str]) -> Option<HighlightClass> {
    scopes.iter().rev().find_map(|scope| class_for_scope(scope))
}

fn class_for_scope(scope: &str) -> Option<HighlightClass> {
    use HighlightClass::*;
    // Longest prefix first: the first entry whose prefix matches on a dot boundary wins.
    const TABLE: &[(&str, HighlightClass)] = &[
        ("comment", Comment),
        ("constant.character.escape", Escape),
        ("constant.numeric", Number),
        ("constant.language", Constant),
        ("constant.other.placeholder", Escape),
        ("constant", Constant),
        ("support.constant", Constant),
        ("string.regexp", Escape),
        ("string", String),
        ("keyword.operator", Operator),
        ("keyword.other.unit", Number),
        ("keyword", Keyword),
        ("storage", Keyword),
        ("entity.name.function", Function),
        ("entity.name.tag", Tag),
        ("entity.name.type", Type),
        ("entity.name.class", Type),
        ("entity.name.namespace", Type),
        ("entity.name.section", Type),
        ("entity.other.attribute-name", Attribute),
        ("entity.other.inherited-class", Type),
        ("support.function", Function),
        ("support.class", Type),
        ("support.type.property-name", Property),
        ("support.type", Type),
        ("meta.function-call", Function),
        ("meta.object-literal.key", Property),
        ("meta.tag", Tag),
        ("variable.other.property", Property),
        ("variable.other.object.property", Property),
        ("variable.language", Keyword),
        ("variable", Variable),
        ("punctuation.definition.tag", Tag),
        ("punctuation", Punctuation),
        ("meta.brace", Punctuation),
        ("markup.heading", Type),
        ("markup.bold", Keyword),
        ("markup.italic", Keyword),
        ("markup.inline.raw", String),
        ("markup.fenced_code", String),
        ("markup.raw", String),
        ("markup.underline.link", Attribute),
        ("markup.quote", Comment),
        ("markup.list", Operator),
        ("markup.inserted", String),
        ("markup.deleted", Tag),
        ("invalid", Tag),
    ];
    TABLE
        .iter()
        .filter(|(prefix, _)| scope == *prefix || (scope.starts_with(prefix) && scope.as_bytes().get(prefix.len()) == Some(&b'.')))
        .max_by_key(|(prefix, _)| prefix.len())
        .map(|(_, class)| *class)
}

#[cfg(test)]
mod tests {
    use forge_text::{Buffer, Selection, SelectionSet};

    use super::*;

    const MINI: &str = r#"{
        "scopeName": "source.mini",
        "patterns": [
            { "match": "\\b(let|fn)\\b", "name": "keyword.control.mini" },
            { "match": "\\b[0-9]+\\b", "name": "constant.numeric.mini" },
            { "begin": "\"", "end": "\"", "name": "string.quoted.double.mini",
              "patterns": [{ "match": "\\\\.", "name": "constant.character.escape.mini" }] },
            { "begin": "/\\*", "end": "\\*/", "name": "comment.block.mini" },
            { "match": "//.*$", "name": "comment.line.mini" }
        ]
    }"#;

    fn mini() -> TextMateSyntax {
        let registry = TextMateRegistry::new();
        registry.add_grammar("source.mini", MINI.as_bytes()).expect("the grammar loads");
        registry.syntax("source.mini").expect("a tokenizer")
    }

    fn class_at(spans: &[Span], col: usize) -> Option<HighlightClass> {
        spans.iter().find(|s| s.start_col <= col && col < s.end_col).map(|s| s.class)
    }

    fn all(syntax: &TextMateSyntax, source: &str) -> Vec<Vec<Span>> {
        let rope = Rope::from_str(source);
        syntax.highlight_lines(&rope, 0..rope.len_lines())
    }

    #[test]
    fn scopes_map_to_the_nearest_meaningful_class() {
        use HighlightClass::*;
        assert_eq!(class_for_scopes(&["source.ts", "meta.block", "keyword.control.ts"]), Some(Keyword));
        assert_eq!(class_for_scopes(&["source.ts", "string.quoted.double", "constant.character.escape"]), Some(Escape));
        assert_eq!(class_for_scopes(&["source.ts", "string.quoted.double"]), Some(String));
        assert_eq!(class_for_scopes(&["source.ts", "keyword.operator.assignment"]), Some(Operator));
        assert_eq!(class_for_scopes(&["source.ts", "variable.other.property.ts"]), Some(Property));
        assert_eq!(class_for_scopes(&["source.ts", "meta.block"]), None, "plain text");
        assert_eq!(class_for_scopes(&["source.ts"]), None);
        // A scope only matches whole dot-separated segments.
        assert_eq!(class_for_scopes(&["keywordy"]), None);
        assert_eq!(class_for_scopes(&[]), None);
    }

    #[test]
    fn a_grammar_colors_tokens_with_char_columns() {
        let lines = all(&mini(), "let x = 42 // hi\nfn s = \"é\\n\" 7\n");
        use HighlightClass::*;
        assert_eq!(class_at(&lines[0], 0), Some(Keyword), "let");
        assert_eq!(class_at(&lines[0], 4), None, "x is plain");
        assert_eq!(class_at(&lines[0], 8), Some(Number), "42");
        assert_eq!(class_at(&lines[0], 12), Some(Comment), "// hi");
        // Columns count chars, not bytes: the string holds a two-byte `é`.
        assert_eq!(class_at(&lines[1], 0), Some(Keyword), "fn");
        assert_eq!(class_at(&lines[1], 7), Some(String), "opening quote");
        assert_eq!(class_at(&lines[1], 8), Some(String), "é");
        assert_eq!(class_at(&lines[1], 9), Some(Escape), "backslash of the escape");
        assert_eq!(class_at(&lines[1], 11), Some(String), "closing quote");
        assert_eq!(class_at(&lines[1], 13), Some(Number), "7");
    }

    #[test]
    fn state_carries_across_lines() {
        let lines = all(&mini(), "a /* start\nlet inside\nend */ let\n");
        use HighlightClass::*;
        assert_eq!(class_at(&lines[1], 0), Some(Comment), "`let` inside a block comment is a comment");
        assert_eq!(class_at(&lines[2], 4), Some(Comment));
        assert_eq!(class_at(&lines[2], 7), Some(Keyword), "the comment has ended");
    }

    #[test]
    fn only_the_requested_lines_are_tokenized_and_requests_clamp() {
        let syntax = mini();
        let rope = Rope::from_str("let a\nlet b\nlet c\nlet d\n");
        let middle = syntax.highlight_lines(&rope, 1..2);
        assert_eq!(middle.len(), 1);
        assert_eq!(syntax.inner.borrow().spans.len(), 2, "lines 0 and 1 only, for the state");
        assert!(syntax.highlight_lines(&rope, 50..60).is_empty());
    }

    #[test]
    fn edits_agree_with_a_fresh_tokenization() {
        let mut buffer = Buffer::new("let a = 1\nlet b = 2\nlet c = 3\n");
        let mut syntax = mini();
        let _ = syntax.highlight_lines(buffer.rope(), 0..buffer.len_lines());
        let steps: Vec<(Vec<usize>, &str)> = vec![
            (vec![10], "/* "),  // opens a comment that swallows the lines below
            (vec![0], "\"é"),   // an unterminated string on line 0
            (vec![5], "\n"),    // a new line shifts everything down
        ];
        for (carets, text) in steps {
            let set = SelectionSet::new(carets.into_iter().map(Selection::caret).collect());
            buffer.insert(&set, text);
            let edits = buffer.take_edits();
            syntax.parse(buffer.rope(), &edits);
            let incremental = syntax.highlight_lines(buffer.rope(), 0..buffer.len_lines());
            assert_eq!(incremental, all(&mini(), &buffer.text()), "after inserting {text:?}");
        }
        buffer.undo().unwrap();
        let edits = buffer.take_edits();
        syntax.parse(buffer.rope(), &edits);
        assert_eq!(syntax.highlight_lines(buffer.rope(), 0..buffer.len_lines()), all(&mini(), &buffer.text()), "after undo");
    }

    #[test]
    fn bad_grammars_and_unknown_names_are_reported_not_fatal() {
        let registry = TextMateRegistry::new();
        assert!(registry.add_grammar("bad", b"{ not json").is_err());
        assert!(registry.add_grammar("empty", b"{}").is_err() || !registry.has_grammar("nothing"));
        assert!(registry.syntax("never-registered").is_none());
        registry.add_grammar("source.mini", MINI.as_bytes()).unwrap();
        assert!(registry.has_grammar("source.mini"));
    }

    #[test]
    fn huge_files_stay_plain() {
        let syntax = mini();
        let big = "let a = 1\n".repeat(MAX_TEXTMATE_BYTES / 10 + 10);
        let rope = Rope::from_str(&big);
        let lines = syntax.highlight_lines(&rope, 0..3);
        assert!(lines.iter().all(Vec::is_empty));
        assert!(syntax.inner.borrow().spans.is_empty(), "nothing was tokenized");
    }
}
