//! Tree-sitter syntax highlighting over a [`ropey::Rope`].
//!
//! The syntax tree is kept in sync with the buffer incrementally (via
//! [`forge_text::TextEdit`]), and highlights are computed only for the lines
//! asked for, so cost tracks the viewport rather than the file.

mod inject;
mod language;
mod lines;
mod textmate;

pub use language::{HighlightClass, Language};
pub use textmate::{class_for_scopes, TextMateRegistry, TextMateSyntax, MAX_TEXTMATE_BYTES};

use std::collections::HashMap;
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

/// A language's highlight query, compiled. Compiling it takes several milliseconds, so one copy is shared by every
/// document in that language.
struct Compiled {
    query: Query,
    /// Highlight class per query capture index; `None` captures are ignored.
    classes: Vec<Option<HighlightClass>>,
}

/// A tree-sitter parser (one per document, it keeps parse state) with its language's shared highlight query.
struct Engine {
    parser: Parser,
    compiled: std::sync::Arc<Compiled>,
}

impl Engine {
    /// `None` if the grammar and its bundled query cannot be loaded together.
    fn load(language: Language) -> Option<Self> {
        static CACHE: std::sync::Mutex<Option<HashMap<Language, Option<std::sync::Arc<Compiled>>>>> = std::sync::Mutex::new(None);
        let Grammar::Tree(ts_language, queries) = language.grammar() else { return None };
        let mut parser = Parser::new();
        parser.set_language(&ts_language).ok()?;
        let compiled = CACHE
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get_or_insert_with(HashMap::new)
            .entry(language)
            .or_insert_with(|| {
                // Combined queries can name nodes a grammar variant lacks; fall back to a simpler one.
                let query = queries.iter().find_map(|q| Query::new(&ts_language, q).ok())?;
                let classes = query.capture_names().iter().map(|name| HighlightClass::from_capture(name)).collect();
                Some(std::sync::Arc::new(Compiled { query, classes }))
            })
            .clone()?;
        Some(Self { parser, compiled })
    }
}

/// Code in another language inside the document: a `<script>`, a Markdown fence.
struct Layer {
    language: Language,
    /// Absent for line-highlighted languages.
    tree: Option<Tree>,
    range: tree_sitter::Range,
    /// 1 for a region of the document itself, 2 for one inside such a region.
    depth: usize,
}

impl Layer {
    /// The rows holding the region's text (a closing fence's row is not part of it).
    fn rows(&self) -> (usize, usize) {
        let first = self.range.start_point.row;
        let mut last = self.range.end_point.row;
        if self.range.end_point.column == 0 && last > first {
            last -= 1;
        }
        (first, last)
    }
}

/// How deep embedded code may nest (Markdown > HTML > JavaScript).
const MAX_NESTING: usize = 2;
/// Candidates from embedded code outrank the host's, a nested layer outranks its parent.
const LAYER_PRIORITY: usize = 1_000_000;

pub struct Syntax {
    language: Language,
    /// Absent for line-highlighted formats (`.env`, INI, Dockerfile, ...).
    engine: Option<Engine>,
    tree: Option<Tree>,
    /// Parsers for embedded languages, loaded on first use.
    embedded: HashMap<Language, Option<Engine>>,
    layers: Vec<Layer>,
}

fn parse_rope(parser: &mut Parser, rope: &Rope, old: Option<&Tree>) -> Option<Tree> {
    let total = rope.len_bytes();
    let mut read = |byte: usize, _: Point| -> &[u8] {
        if byte >= total {
            return &[];
        }
        let (chunk, chunk_start, _, _) = rope.chunk_at_byte(byte);
        &chunk.as_bytes()[byte - chunk_start..]
    };
    parser.parse_with_options(&mut read, old, None)
}

/// `base` (sorted, not overlapping) with `over` (sorted, not overlapping) laid on top: wherever they meet, `over`
/// wins and the span of `base` it covers is cut or split.
pub fn overlay_spans(base: &[Span], over: &[Span]) -> Vec<Span> {
    if over.is_empty() {
        return base.to_vec();
    }
    let mut out: Vec<Span> = Vec::with_capacity(base.len() + over.len());
    for b in base {
        let mut start = b.start_col;
        for o in over.iter().filter(|o| o.end_col > b.start_col && o.start_col < b.end_col) {
            if o.start_col > start {
                out.push(Span { start_col: start, end_col: o.start_col, class: b.class });
            }
            start = start.max(o.end_col);
        }
        if start < b.end_col {
            out.push(Span { start_col: start, end_col: b.end_col, class: b.class });
        }
    }
    out.extend_from_slice(over);
    out.sort_by_key(|s| (s.start_col, s.end_col));
    out
}

impl Syntax {
    /// `None` if the grammar and its bundled query cannot be loaded together.
    pub fn new(language: Language) -> Option<Self> {
        let engine = if language.is_line_based() { None } else { Some(Engine::load(language)?) };
        Some(Self { language, engine, tree: None, embedded: HashMap::new(), layers: Vec::new() })
    }

    pub fn language(&self) -> Language {
        self.language
    }

    /// Bring the tree up to date. `edits` are those applied to `rope` since the last
    /// call; pass an empty slice for the initial parse.
    pub fn parse(&mut self, rope: &Rope, edits: &[TextEdit]) {
        let Some(engine) = self.engine.as_mut() else { return };
        if rope.len_bytes() > MAX_HIGHLIGHT_BYTES {
            self.tree = None;
            self.layers.clear();
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
        // On a cancelled/failed parse keep the previous tree: stale colors beat none.
        if let Some(tree) = parse_rope(&mut engine.parser, rope, self.tree.as_ref()) {
            self.tree = Some(tree);
        }
        self.rebuild_layers(rope);
    }

    /// Re-find and re-parse the embedded regions. They are small next to the document, so
    /// they are parsed afresh rather than incrementally.
    fn rebuild_layers(&mut self, rope: &Rope) {
        self.layers.clear();
        let Some(tree) = self.tree.clone() else { return };
        let mut work = vec![(self.language, tree, 1usize)];
        while let Some((host, tree, depth)) = work.pop() {
            for region in inject::find_regions(host, &tree, rope) {
                if region.range.end_byte - region.range.start_byte > inject::MAX_REGION_BYTES {
                    continue;
                }
                let language = region.language;
                let parsed = if language.is_line_based() {
                    None
                } else {
                    let Some(engine) = self.embedded.entry(language).or_insert_with(|| Engine::load(language)) else { continue };
                    if engine.parser.set_included_ranges(&[region.range]).is_err() {
                        continue;
                    }
                    let Some(parsed) = parse_rope(&mut engine.parser, rope, None) else { continue };
                    Some(parsed)
                };
                if let (Some(parsed), true) = (&parsed, depth < MAX_NESTING) {
                    work.push((language, parsed.clone(), depth + 1));
                }
                self.layers.push(Layer { language, tree: parsed, range: region.range, depth });
            }
        }
    }

    /// The language of the code at char offset `at`: the innermost embedded region that holds it (JavaScript inside
    /// `<script>`, ...), else the document's own language.
    pub fn language_at(&self, rope: &Rope, at: usize) -> Language {
        let byte = rope.char_to_byte(at.min(rope.len_chars()));
        self.layers
            .iter()
            .filter(|l| l.range.start_byte <= byte && byte <= l.range.end_byte)
            .max_by_key(|l| l.depth)
            .map_or(self.language, |l| l.language)
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
        let Some(engine) = &self.engine else {
            // Line-highlighted format: no tree, and large files are still cheap per line.
            if rope.len_bytes() > MAX_HIGHLIGHT_BYTES {
                return vec![Vec::new(); end - start];
            }
            return (start..end).map(|l| lines::highlight_line(self.language, &rope.line(l).to_string())).collect();
        };
        let mut candidates: Vec<Vec<Candidate>> = vec![Vec::new(); end - start];
        let Some(tree) = &self.tree else { return vec![Vec::new(); end - start] };
        if start == end {
            return Vec::new();
        }

        let byte_start = rope.line_to_byte(start);
        let byte_end = if end >= rope.len_lines() { rope.len_bytes() } else { rope.line_to_byte(end) };
        collect_captures(rope, tree, engine, byte_start..byte_end, (start, end), 0, &mut candidates);

        for layer in &self.layers {
            let (first, last) = layer.rows();
            if last < start || first >= end {
                continue;
            }
            let base = layer.depth * LAYER_PRIORITY;
            let rows = first.max(start)..=last.min(end - 1);
            // The embedded language owns its columns: hide whatever the host drew there.
            for row in rows.clone() {
                if let Some((from, to)) = columns_in_bytes(rope, layer.range.start_byte, layer.range.end_byte, row) {
                    candidates[row - start].retain(|c| !(c.from >= from && c.to <= to && c.priority < base));
                }
            }
            match &layer.tree {
                Some(layer_tree) => {
                    let Some(Some(embedded)) = self.embedded.get(&layer.language) else { continue };
                    let from = byte_start.max(layer.range.start_byte);
                    let to = byte_end.min(layer.range.end_byte);
                    if from < to {
                        collect_captures(rope, layer_tree, embedded, from..to, (start, end), base, &mut candidates);
                    }
                }
                None => {
                    for row in rows {
                        for span in lines::highlight_line(layer.language, &rope.line(row).to_string()) {
                            candidates[row - start].push(Candidate { from: span.start_col, to: span.end_col, class: span.class, priority: base });
                        }
                    }
                }
            }
        }
        candidates.into_iter().map(resolve_overlaps).collect()
    }
}

/// Add the captures of `engine`'s query over `bytes` to the per-row `candidates`
/// (rows `rows.0..rows.1`).
fn collect_captures(rope: &Rope, tree: &Tree, engine: &Engine, bytes: Range<usize>, rows: (usize, usize), base: usize, candidates: &mut [Vec<Candidate>]) {
    let (start, end) = rows;
    let mut cursor = QueryCursor::new();
    cursor.set_byte_range(bytes);
    let mut captures = cursor.captures(&engine.compiled.query, tree.root_node(), RopeText(rope));
    while let Some((m, index)) = captures.next() {
        let capture = m.captures()[*index];
        let Some(class) = engine.compiled.classes[capture.index as usize] else { continue };
        let node = capture.node;
        let first = node.start_position().row.max(start);
        let last = node.end_position().row.min(end - 1);
        for row in first..=last {
            if let Some((from, to)) = columns_in_bytes(rope, node.start_byte(), node.end_byte(), row) {
                candidates[row - start].push(Candidate { from, to, class, priority: base + m.pattern_index });
            }
        }
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

/// Char-column range of the byte range within `row`, excluding the line terminator.
fn columns_in_bytes(rope: &Rope, start_byte: usize, end_byte: usize, row: usize) -> Option<(usize, usize)> {
    let line_start_byte = rope.line_to_byte(row);
    let line = rope.line(row);
    let mut line_chars = line.len_chars();
    while line_chars > 0 && matches!(line.char(line_chars - 1), '\n' | '\r') {
        line_chars -= 1;
    }
    let line_start_char = rope.line_to_char(row);
    let line_end_byte = rope.char_to_byte(line_start_char + line_chars);

    let from = start_byte.max(line_start_byte);
    let to = end_byte.min(line_end_byte);
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

    fn rows_class_at(lines: &[Vec<Span>], row: usize, col: usize) -> Option<HighlightClass> {
        class_at(&lines[row], col)
    }

    #[test]
    fn html_script_and_style_are_highlighted_as_javascript_and_css() {
        let src = "<script>\nconst x = 1;\n</script>\n<style>\na { color: red; }\n</style>\n<p>text</p>\n";
        let lines = highlight(Language::Html, src);
        assert_eq!(rows_class_at(&lines, 1, 0), Some(HighlightClass::Keyword), "const");
        assert_eq!(rows_class_at(&lines, 1, 10), Some(HighlightClass::Number), "1");
        assert_eq!(rows_class_at(&lines, 4, 4), Some(HighlightClass::Property), "color");
        // The surrounding markup is still HTML.
        assert_eq!(rows_class_at(&lines, 0, 1), Some(HighlightClass::Tag), "script");
        assert_eq!(rows_class_at(&lines, 6, 1), Some(HighlightClass::Tag), "p");
    }

    #[test]
    fn html_script_on_one_line_and_typed_scripts() {
        let lines = highlight(Language::Html, "<script>let a = 2;</script>\n<script lang=\"ts\">let b: number = 3;</script>\n<script type=\"application/json\">{\"k\": 1}</script>\n<script type=\"text/template\">let c</script>\n");
        assert_eq!(rows_class_at(&lines, 0, 8), Some(HighlightClass::Keyword), "let in a plain script");
        assert_eq!(rows_class_at(&lines, 1, 25), Some(HighlightClass::Type), "number in a TypeScript script");
        assert_eq!(rows_class_at(&lines, 2, 28), Some(HighlightClass::String), "json key");
        assert_ne!(rows_class_at(&lines, 3, 31), Some(HighlightClass::Keyword), "templates are not code");
    }

    #[test]
    fn markdown_fences_use_the_language_they_name() {
        let src = "# Title\n\n```rust\nfn main() {}\n```\n\n```js\nconst a = 1;\n```\n\n```diff\n+added\n-removed\n```\n\n```nonsense\nfn main() {}\n```\n";
        let lines = highlight(Language::Markdown, src);
        assert_eq!(rows_class_at(&lines, 3, 0), Some(HighlightClass::Keyword), "fn in a rust fence");
        assert_eq!(rows_class_at(&lines, 3, 3), Some(HighlightClass::Function), "main");
        assert_eq!(rows_class_at(&lines, 7, 0), Some(HighlightClass::Keyword), "const in a js fence");
        assert_eq!(rows_class_at(&lines, 11, 1), Some(HighlightClass::String), "diff addition");
        assert_eq!(rows_class_at(&lines, 12, 1), Some(HighlightClass::Tag), "diff removal");
        assert_ne!(rows_class_at(&lines, 16, 0), Some(HighlightClass::Keyword), "unknown languages stay plain");
        assert_eq!(rows_class_at(&lines, 0, 2), Some(HighlightClass::Type), "the heading is still Markdown");
    }

    #[test]
    fn embedded_languages_nest() {
        let src = "```html\n<script>let a = 1;</script>\n```\n";
        let lines = highlight(Language::Markdown, src);
        assert_eq!(rows_class_at(&lines, 1, 1), Some(HighlightClass::Tag), "script tag");
        assert_eq!(rows_class_at(&lines, 1, 8), Some(HighlightClass::Keyword), "let, two layers deep");
    }

    #[test]
    fn embedded_regions_follow_edits_like_a_fresh_parse() {
        let mut buffer = Buffer::new("<p>a</p>\n<script>\nlet a = 1;\n</script>\n");
        let mut syntax = Syntax::new(Language::Html).unwrap();
        syntax.parse(buffer.rope(), &[]);
        let steps: Vec<(Vec<usize>, &str)> = vec![
            (vec![20], "const "), // inside the script
            (vec![3], "<style>b{c:d}</style>"), // a new region appears
            (vec![0], "<!-- "), // markup before the regions shifts them
        ];
        for (carets, text) in steps {
            let set = SelectionSet::new(carets.into_iter().map(Selection::caret).collect());
            buffer.insert(&set, text);
            let edits = buffer.take_edits();
            syntax.parse(buffer.rope(), &edits);
            assert_eq!(syntax.highlight_lines(buffer.rope(), 0..buffer.len_lines()), highlight(Language::Html, &buffer.text()), "after inserting {text:?}");
        }
    }

    #[test]
    fn big_embedded_regions_are_left_alone_and_other_languages_have_none() {
        let big = format!("<script>{}</script>", "let a = 1;\n".repeat(60_000));
        let lines = highlight(Language::Html, &big);
        assert!(lines[1].iter().all(|s| s.class != HighlightClass::Keyword), "a 660 KB script is not parsed");
        let rust = highlight(Language::Rust, "let s = \"```js\\nconst a\";\n");
        assert!(rust[0].iter().any(|s| s.class == HighlightClass::String));
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

#[cfg(test)]
mod overlay_tests {
    use super::*;

    fn span(a: usize, b: usize, class: HighlightClass) -> Span {
        Span { start_col: a, end_col: b, class }
    }

    #[test]
    fn the_overlay_wins_where_the_spans_meet() {
        use HighlightClass::*;
        let base = [span(0, 10, Keyword), span(12, 20, String)];
        let over = [span(3, 5, Function), span(8, 14, Type)];
        let got = overlay_spans(&base, &over);
        assert_eq!(got, vec![span(0, 3, Keyword), span(3, 5, Function), span(5, 8, Keyword), span(8, 14, Type), span(14, 20, String)]);
        assert_eq!(overlay_spans(&base, &[]), base.to_vec());
        assert_eq!(overlay_spans(&[], &over), over.to_vec());
    }
}
