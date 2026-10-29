//! Tree-sitter syntax highlighting over a [`ropey::Rope`].
//!
//! The syntax tree is kept in sync with the buffer incrementally (via
//! [`forge_text::TextEdit`]), and highlights are computed only for the lines
//! asked for, so cost tracks the viewport rather than the file.

mod language;
mod lines;

pub use language::{HighlightClass, Language};

use std::ops::Range;

use forge_text::TextEdit;
use ropey::Rope;
use streaming_iterator::StreamingIterator;
use language::Grammar;
use tree_sitter::{InputEdit, Node, Parser, Point, Query, QueryCursor, TextProvider, Tree};

/// Above this size the file is shown unhighlighted rather than stalling the UI thread.
pub const MAX_HIGHLIGHT_BYTES: usize = 4 * 1024 * 1024;

/// A highlighted run within one line, in char columns (end exclusive).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Span {
    pub start_col: usize,
    pub end_col: usize,
    pub class: HighlightClass,
}

pub struct Syntax {
    language: Language,
    /// Absent for line-highlighted formats (`.env`, INI, Dockerfile, ...).
    parser: Option<Parser>,
    query: Option<Query>,
    /// Highlight class per query capture index; `None` captures are ignored.
    classes: Vec<Option<HighlightClass>>,
    tree: Option<Tree>,
}

impl Syntax {
    /// `None` if the grammar and its bundled query cannot be loaded together.
    pub fn new(language: Language) -> Option<Self> {
        match language.grammar() {
            Grammar::Lines => Some(Self { language, parser: None, query: None, classes: Vec::new(), tree: None }),
            Grammar::Tree(ts_language, queries) => {
                let mut parser = Parser::new();
                parser.set_language(&ts_language).ok()?;
                // Combined queries can name nodes a grammar variant lacks; fall back to a simpler one.
                let query = queries.iter().find_map(|q| Query::new(&ts_language, q).ok())?;
                let classes = query.capture_names().iter().map(|name| HighlightClass::from_capture(name)).collect();
                Some(Self { language, parser: Some(parser), query: Some(query), classes, tree: None })
            }
        }
    }

    pub fn language(&self) -> Language {
        self.language
    }

    /// Bring the tree up to date. `edits` are those applied to `rope` since the last
    /// call; pass an empty slice for the initial parse.
    pub fn parse(&mut self, rope: &Rope, edits: &[TextEdit]) {
        let Some(parser) = self.parser.as_mut() else { return };
        if rope.len_bytes() > MAX_HIGHLIGHT_BYTES {
            self.tree = None;
            return;
        }
        if let Some(tree) = self.tree.as_mut() {
            for e in edits {
                tree.edit(&InputEdit {
                    start_byte: e.start_byte,
                    old_end_byte: e.old_end_byte,
                    new_end_byte: e.new_end_byte,
                    start_position: Point::new(e.start_point.0, e.start_point.1),
                    old_end_position: Point::new(e.old_end_point.0, e.old_end_point.1),
                    new_end_position: Point::new(e.new_end_point.0, e.new_end_point.1),
                });
            }
        }
        let total = rope.len_bytes();
        let mut read = |byte: usize, _: Point| -> &[u8] {
            if byte >= total {
                return &[];
            }
            let (chunk, chunk_start, _, _) = rope.chunk_at_byte(byte);
            &chunk.as_bytes()[byte - chunk_start..]
        };
        // On a cancelled/failed parse keep the previous tree: stale colors beat none.
        if let Some(tree) = parser.parse_with_options(&mut read, self.tree.as_ref(), None) {
            self.tree = Some(tree);
        }
    }

    /// The smallest syntax node strictly larger than the char range `start..end`, as char
    /// offsets: what Shift+Alt+Right selects next.
    pub fn enclosing_range(&self, rope: &Rope, start: usize, end: usize) -> Option<(usize, usize)> {
        let root = self.tree.as_ref()?.root_node();
        let (sb, eb) = (rope.char_to_byte(start), rope.char_to_byte(end));
        let mut node = root.descendant_for_byte_range(sb, eb)?;
        while node.start_byte() == sb && node.end_byte() == eb {
            node = node.parent()?;
        }
        Some((rope.byte_to_char(node.start_byte()), rope.byte_to_char(node.end_byte())))
    }

    /// Foldable regions as `(first line, last line)`: multi-line blocks, bodies, lists,
    /// objects and comments. One region per start line, keeping the largest.
    pub fn fold_regions(&self) -> Vec<(usize, usize)> {
        let Some(tree) = &self.tree else { return Vec::new() };
        let mut by_start: std::collections::BTreeMap<usize, usize> = std::collections::BTreeMap::new();
        let mut cursor = tree.walk();
        let mut descend = true;
        loop {
            let node = cursor.node();
            if descend && node.is_named() {
                let (first, last) = (node.start_position().row, node.end_position().row);
                if last > first && is_foldable(node.kind()) {
                    let entry = by_start.entry(first).or_insert(last);
                    *entry = (*entry).max(last);
                }
            }
            if descend && cursor.goto_first_child() {
                continue;
            }
            if cursor.goto_next_sibling() {
                descend = true;
                continue;
            }
            if !cursor.goto_parent() {
                break;
            }
            descend = false;
        }
        by_start.into_iter().collect()
    }

    /// Highlight spans for each line in `lines` (clamped to the buffer), one `Vec` per
    /// line, sorted by column and non-overlapping.
    pub fn highlight_lines(&self, rope: &Rope, lines: Range<usize>) -> Vec<Vec<Span>> {
        let end = lines.end.min(rope.len_lines());
        let start = lines.start.min(end);
        if self.parser.is_none() {
            // Line-highlighted format: no tree, and large files are still cheap per line.
            if rope.len_bytes() > MAX_HIGHLIGHT_BYTES {
                return vec![Vec::new(); end - start];
            }
            return (start..end).map(|l| lines::highlight_line(self.language, &rope.line(l).to_string())).collect();
        }
        let mut candidates: Vec<Vec<Candidate>> = vec![Vec::new(); end - start];
        let (Some(tree), Some(query)) = (&self.tree, &self.query) else { return vec![Vec::new(); end - start] };
        if start == end {
            return Vec::new();
        }

        let byte_start = rope.line_to_byte(start);
        let byte_end = if end >= rope.len_lines() { rope.len_bytes() } else { rope.line_to_byte(end) };
        let mut cursor = QueryCursor::new();
        cursor.set_byte_range(byte_start..byte_end);
        let mut captures = cursor.captures(query, tree.root_node(), RopeText(rope));
        while let Some((m, index)) = captures.next() {
            let capture = m.captures()[*index];
            let Some(class) = self.classes[capture.index as usize] else { continue };
            let node = capture.node;
            let first = node.start_position().row.max(start);
            let last = node.end_position().row.min(end - 1);
            for row in first..=last {
                if let Some((from, to)) = columns_in_row(rope, node, row) {
                    candidates[row - start].push(Candidate { from, to, class, priority: m.pattern_index });
                }
            }
        }
        candidates.into_iter().map(resolve_overlaps).collect()
    }
}

fn is_foldable(kind: &str) -> bool {
    matches!(
        kind,
        "object" | "array" | "table" | "table_array_element" | "block_comment" | "element" | "script_element" | "style_element"
            | "jsx_element" | "block_mapping" | "block_sequence" | "rule_set" | "namespace_declaration" | "class_declaration"
            | "function_declaration" | "section" | "fenced_code_block"
    )
        || kind.ends_with("block")
        || kind.ends_with("_body")
        || kind.ends_with("_list")
        || kind == "match_block"
        || kind == "arguments"
}

#[derive(Clone, Copy)]
struct Candidate {
    from: usize,
    to: usize,
    class: HighlightClass,
    /// Index of the query pattern; later patterns override earlier ones.
    priority: usize,
}

/// Char-column range of `node` within `row`, excluding the line terminator.
fn columns_in_row(rope: &Rope, node: Node, row: usize) -> Option<(usize, usize)> {
    let line_start_byte = rope.line_to_byte(row);
    let line = rope.line(row);
    let mut line_chars = line.len_chars();
    while line_chars > 0 && matches!(line.char(line_chars - 1), '\n' | '\r') {
        line_chars -= 1;
    }
    let line_start_char = rope.line_to_char(row);
    let line_end_byte = rope.char_to_byte(line_start_char + line_chars);

    let from = node.start_byte().max(line_start_byte);
    let to = node.end_byte().min(line_end_byte);
    if from >= to {
        return None;
    }
    Some((rope.byte_to_char(from) - line_start_char, rope.byte_to_char(to) - line_start_char))
}

/// Flatten overlapping candidates: at every column the highest-priority one wins,
/// ties going to the narrower capture. Adjacent runs of one class are merged.
fn resolve_overlaps(candidates: Vec<Candidate>) -> Vec<Span> {
    let mut bounds: Vec<usize> = candidates.iter().flat_map(|c| [c.from, c.to]).collect();
    bounds.sort_unstable();
    bounds.dedup();

    let mut spans: Vec<Span> = Vec::new();
    for pair in bounds.windows(2) {
        let (a, b) = (pair[0], pair[1]);
        let winner = candidates
            .iter()
            .filter(|c| c.from <= a && c.to >= b)
            .max_by_key(|c| (c.priority, std::cmp::Reverse(c.to - c.from)));
        let Some(winner) = winner else { continue };
        match spans.last_mut() {
            Some(prev) if prev.end_col == a && prev.class == winner.class => prev.end_col = b,
            _ => spans.push(Span { start_col: a, end_col: b, class: winner.class }),
        }
    }
    spans
}

/// Feeds query predicates (`#eq?`, `#match?`) the text of a node straight from the rope.
struct RopeText<'a>(&'a Rope);

impl<'a> TextProvider<&'a [u8]> for RopeText<'a> {
    type I = Box<dyn Iterator<Item = &'a [u8]> + 'a>;

    fn text(&mut self, node: Node) -> Self::I {
        Box::new(self.0.byte_slice(node.byte_range()).chunks().map(str::as_bytes))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use forge_text::{Buffer, Selection, SelectionSet};

    fn highlight(language: Language, source: &str) -> Vec<Vec<Span>> {
        let rope = Rope::from_str(source);
        let mut syntax = Syntax::new(language).expect("grammar loads");
        syntax.parse(&rope, &[]);
        syntax.highlight_lines(&rope, 0..rope.len_lines())
    }

    fn class_at(spans: &[Span], col: usize) -> Option<HighlightClass> {
        spans.iter().find(|s| s.start_col <= col && col < s.end_col).map(|s| s.class)
    }

    #[test]
    fn rust_basics() {
        let lines = highlight(Language::Rust, "fn main() {\n    let x = 42; // hi\n}\n");
        assert_eq!(class_at(&lines[0], 0), Some(HighlightClass::Keyword), "fn");
        assert_eq!(class_at(&lines[0], 3), Some(HighlightClass::Function), "main");
        assert_eq!(class_at(&lines[1], 4), Some(HighlightClass::Keyword), "let");
        // The Rust query tags integer literals as constants, not numbers.
        assert_eq!(class_at(&lines[1], 12), Some(HighlightClass::Constant), "42");
        assert_eq!(class_at(&lines[1], 17), Some(HighlightClass::Comment), "// hi");
    }

    #[test]
    fn rust_strings_types_and_multiline_comments() {
        let lines = highlight(Language::Rust, "/* a\n b */\nlet s: String = \"é\";\n");
        // A block comment is split per line and covers the whole of each.
        assert_eq!(class_at(&lines[0], 0), Some(HighlightClass::Comment));
        assert_eq!(class_at(&lines[1], 3), Some(HighlightClass::Comment));
        assert_eq!(class_at(&lines[2], 7), Some(HighlightClass::Type), "String");
        // Columns are chars, not bytes: the string literal spans cols 16..19.
        assert_eq!(class_at(&lines[2], 16), Some(HighlightClass::String));
        assert_eq!(class_at(&lines[2], 17), Some(HighlightClass::String), "é");
        assert_eq!(class_at(&lines[2], 18), Some(HighlightClass::String));
        assert_eq!(class_at(&lines[2], 19), Some(HighlightClass::Punctuation), ";");
    }

    #[test]
    fn other_languages_load_and_color() {
        let json = highlight(Language::Json, "{\"k\": 1, \"ok\": true}");
        assert_eq!(class_at(&json[0], 1), Some(HighlightClass::String), "key");
        assert_eq!(class_at(&json[0], 6), Some(HighlightClass::Number));

        let py = highlight(Language::Python, "def f(x):\n    return 1  # c\n");
        assert_eq!(class_at(&py[0], 0), Some(HighlightClass::Keyword));
        assert_eq!(class_at(&py[0], 4), Some(HighlightClass::Function));
        assert_eq!(class_at(&py[1], 4), Some(HighlightClass::Keyword));

        let js = highlight(Language::JavaScript, "const a = \"x\"; // t\n");
        assert_eq!(class_at(&js[0], 0), Some(HighlightClass::Keyword));
        assert_eq!(class_at(&js[0], 10), Some(HighlightClass::String));

        let toml = highlight(Language::Toml, "[pkg]\nname = \"forge\"\n");
        assert!(toml[1].iter().any(|s| s.class == HighlightClass::String));
    }

    #[test]
    fn line_window_only_returns_requested_lines() {
        let rope = Rope::from_str("let a = 1;\nlet b = 2;\nlet c = 3;\n");
        let mut syntax = Syntax::new(Language::Rust).unwrap();
        syntax.parse(&rope, &[]);
        let all = syntax.highlight_lines(&rope, 0..4);
        let middle = syntax.highlight_lines(&rope, 1..2);
        assert_eq!(middle.len(), 1);
        assert_eq!(middle[0], all[1]);
        // Out-of-range requests clamp instead of panicking.
        assert!(syntax.highlight_lines(&rope, 50..60).is_empty());
    }

    /// Incremental updates must agree with parsing the final text from scratch.
    #[test]
    fn incremental_parse_matches_fresh_parse() {
        let mut buffer = Buffer::new("fn a() {}\nfn b() {}\n");
        let mut syntax = Syntax::new(Language::Rust).unwrap();
        syntax.parse(buffer.rope(), &[]);

        let steps: Vec<(Vec<usize>, &str)> = vec![
            (vec![0, 10], "// "),      // two cursors comment out both lines' starts
            (vec![8, 21], "x"),        // inside the code
            (vec![3], "\n\"é"),        // open a string literal across a newline
        ];
        for (carets, text) in steps {
            let set = SelectionSet::new(carets.into_iter().map(Selection::caret).collect());
            buffer.insert(&set, text);
            let edits = buffer.take_edits();
            syntax.parse(buffer.rope(), &edits);

            let incremental = syntax.highlight_lines(buffer.rope(), 0..buffer.len_lines());
            let fresh = highlight(Language::Rust, &buffer.text());
            assert_eq!(incremental, fresh, "after inserting {text:?}");
        }

        buffer.undo().unwrap();
        let edits = buffer.take_edits();
        syntax.parse(buffer.rope(), &edits);
        assert_eq!(
            syntax.highlight_lines(buffer.rope(), 0..buffer.len_lines()),
            highlight(Language::Rust, &buffer.text()),
            "after undo"
        );
    }

    #[test]
    fn expansion_grows_to_the_enclosing_node() {
        let rope = Rope::from_str("fn f() { let x = (1 + 2); }");
        let mut syntax = Syntax::new(Language::Rust).unwrap();
        syntax.parse(&rope, &[]);
        // From inside `1`, growing repeatedly reaches ever larger ranges and ends at the file.
        let mut range = (18, 18);
        let mut seen = Vec::new();
        while let Some(next) = syntax.enclosing_range(&rope, range.0, range.1) {
            assert!(next.0 <= range.0 && next.1 >= range.1 && next != range);
            seen.push(next);
            range = next;
        }
        assert!(seen.len() >= 3, "{seen:?}");
        assert_eq!(*seen.last().unwrap(), (0, rope.len_chars()));
        let (s, e) = seen[0];
        assert_eq!(&rope.slice(s..e).to_string(), "1");
    }

    #[test]
    fn fold_regions_cover_blocks_and_keep_the_largest_per_line() {
        let src = "fn a() {\n    if x {\n        y();\n    }\n}\nstruct S {\n    f: i32,\n}\n";
        let rope = Rope::from_str(src);
        let mut syntax = Syntax::new(Language::Rust).unwrap();
        syntax.parse(&rope, &[]);
        let regions = syntax.fold_regions();
        assert!(regions.contains(&(0, 4)), "fn body: {regions:?}");
        assert!(regions.contains(&(1, 3)), "inner if block: {regions:?}");
        assert!(regions.contains(&(5, 7)), "struct fields: {regions:?}");
        assert_eq!(regions.iter().filter(|r| r.0 == 0).count(), 1);

        let json = Rope::from_str("{\n  \"a\": [\n    1,\n    2\n  ]\n}");
        let mut js = Syntax::new(Language::Json).unwrap();
        js.parse(&json, &[]);
        assert!(js.fold_regions().contains(&(1, 4)));
    }

    #[test]
    fn oversized_files_are_left_plain() {
        let rope = Rope::from_str(&"a".repeat(MAX_HIGHLIGHT_BYTES + 1));
        let mut syntax = Syntax::new(Language::Rust).unwrap();
        syntax.parse(&rope, &[]);
        assert!(syntax.highlight_lines(&rope, 0..1)[0].is_empty());
    }

    fn class_of(language: Language, text: &str, needle: &str) -> Option<HighlightClass> {
        let rope = Rope::from_str(text);
        let mut syntax = Syntax::new(language).unwrap_or_else(|| panic!("{language:?} grammar failed to load"));
        syntax.parse(&rope, &[]);
        for (i, spans) in syntax.highlight_lines(&rope, 0..rope.len_lines()).into_iter().enumerate() {
            let chars: Vec<char> = rope.line(i).to_string().trim_end().chars().collect();
            if let Some(s) = spans.iter().find(|s| chars[s.start_col..s.end_col].iter().collect::<String>() == needle) {
                return Some(s.class);
            }
        }
        None
    }

    /// Every supported language loads and colors real code (the regression for TSX/JSX/XML/.env).
    #[test]
    fn every_language_highlights_real_samples() {
        use HighlightClass::*;
        type Case<'a> = (Language, &'a str, Vec<(&'a str, HighlightClass)>);
        let cases: Vec<Case> = vec![
            (Language::TypeScript, "const x: number = 5;\ninterface Foo { a: string }", vec![("const", Keyword), ("number", Type), ("interface", Keyword), ("Foo", Type)]),
            (Language::TypeScriptReact, "const A = () => <div className=\"x\">{y}</div>;", vec![("const", Keyword), ("div", Tag), ("className", Attribute), ("\"x\"", String)]),
            (Language::JavaScriptReact, "const A = () => <div className=\"x\">{y}</div>;", vec![("div", Tag), ("className", Attribute)]),
            (Language::JavaScript, "const a = <b id=\"1\"/>;", vec![("const", Keyword), ("b", Tag)]),
            (Language::Html, "<div class=\"a\">hi</div>", vec![("div", Tag), ("class", Attribute)]),
            (Language::Css, "a { color: red; }", vec![("color", Property)]),
            (Language::Xml, "<?xml version=\"1.0\"?><root attr=\"v\">text</root>", vec![("root", Tag), ("attr", Property), ("v", String)]),
            (Language::Yaml, "key: value\nlist:\n  - 1", vec![("key", Property), ("1", Number)]),
            (Language::Markdown, "# Title\n\n- item", vec![("Title", Type)]),
            (Language::Shell, "if [ -f x ]; then echo hi; fi", vec![("if", Keyword), ("echo", Function)]),
            (Language::Go, "package main\nfunc main() {}", vec![("package", Keyword), ("func", Keyword)]),
            (Language::C, "int main(void) { return 0; }", vec![("int", Type), ("return", Keyword)]),
            (Language::Cpp, "class A { public: int x; };", vec![("class", Keyword), ("public", Keyword)]),
            (Language::Java, "public class A { void f() {} }", vec![("class", Keyword), ("f", Function)]),
            (Language::CSharp, "public class A { }", vec![("class", Keyword), ("A", Type)]),
            (Language::Php, "<?php\nfunction f() { return 1; }", vec![("function", Keyword), ("f", Function)]),
            (Language::Ruby, "def hello\n  puts 'x'\nend", vec![("def", Keyword), ("'x'", String)]),
            (Language::Lua, "local function f() return 1 end", vec![("local", Keyword), ("f", Function)]),
            (Language::Sql, "SELECT a FROM t WHERE b = 1;", vec![("SELECT", Keyword), ("FROM", Keyword)]),
            (Language::Env, "export PORT=8080 # port\nNAME=\"a ${B}\"", vec![("export", Keyword), ("PORT", Property), ("8080", Number), ("# port", Comment), ("${B}", Escape)]),
            (Language::Ini, "[main]\nkey = value", vec![("[main]", Type), ("key", Property)]),
            (Language::Dockerfile, "FROM rust:1 AS build\nRUN cargo build", vec![("FROM", Keyword), ("RUN", Keyword)]),
            (Language::Makefile, "all: main.o\n\tcc -o $@ main.o", vec![("all", Function), ("$@", Escape)]),
            (Language::Ignore, "target/\n!keep.txt", vec![("target", String), ("!", Operator)]),
            (Language::Diff, "+added\n-removed", vec![("+added", String), ("-removed", Tag)]),
        ];
        for (language, text, expected) in cases {
            for (needle, class) in expected {
                assert_eq!(class_of(language, text, needle), Some(class), "{language:?}: {needle:?} in {text:?}");
            }
        }
    }

    #[test]
    fn line_languages_stay_correct_across_edits_and_never_need_a_tree() {
        let mut buffer = Buffer::new("A=1\n");
        let mut syntax = Syntax::new(Language::Env).unwrap();
        syntax.parse(buffer.rope(), &[]);
        buffer.insert(&SelectionSet::single(Selection::caret(3)), "2");
        let edits = buffer.take_edits();
        syntax.parse(buffer.rope(), &edits);
        let spans = syntax.highlight_lines(buffer.rope(), 0..1);
        assert_eq!(spans[0].last().map(|s| (s.start_col, s.end_col, s.class)), Some((2, 4, HighlightClass::Number)));
        assert!(syntax.fold_regions().is_empty() && syntax.enclosing_range(buffer.rope(), 0, 0).is_none());
    }
}
