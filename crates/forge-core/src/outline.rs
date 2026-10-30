//! The Outline view's symbols when no language server supplies them: Markdown headings, and the
//! declarations (functions, classes, structs...) found by looking at the start of each line.

use std::path::Path;

/// One entry of the outline: where it is, how deep it is nested, and a short label for what it is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OutlineItem {
    pub name: String,
    pub kind: &'static str,
    pub line: usize,
    pub col: usize,
    pub depth: usize,
}

/// Files bigger than this (in bytes) are not scanned: the outline is rebuilt as you type.
pub const MAX_SCAN_BYTES: usize = 2_000_000;

pub fn scan(path: &Path, text: &str) -> Vec<OutlineItem> {
    if text.len() > MAX_SCAN_BYTES {
        return Vec::new();
    }
    let ext = path.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
    match ext.as_str() {
        "md" | "markdown" | "mdx" => markdown(text),
        _ => declarations(&ext, text),
    }
}

fn markdown(text: &str) -> Vec<OutlineItem> {
    let mut out = Vec::new();
    let mut fence: Option<&str> = None;
    for (line, raw) in text.lines().enumerate() {
        let trimmed = raw.trim_start();
        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            let marker = &trimmed[..3];
            match fence {
                Some(open) if open == marker => fence = None,
                None => fence = Some(if marker == "```" { "```" } else { "~~~" }),
                _ => {}
            }
            continue;
        }
        if fence.is_some() || raw.len() - trimmed.len() > 3 {
            continue;
        }
        let level = trimmed.chars().take_while(|c| *c == '#').count();
        if (1..=6).contains(&level) && trimmed[level..].starts_with(' ') {
            let name = trimmed[level..].trim().trim_end_matches('#').trim();
            if !name.is_empty() {
                out.push(OutlineItem { name: name.to_owned(), kind: "#", line, col: 0, depth: level - 1 });
            }
        }
    }
    out
}

/// Words that start a declaration, and what to call it, for a file extension.
fn keywords(ext: &str) -> &'static [(&'static str, &'static str)] {
    match ext {
        "rs" => &[("fn", "fn"), ("struct", "struct"), ("enum", "enum"), ("trait", "trait"), ("impl", "impl"), ("mod", "mod"), ("const", "const"), ("static", "const"), ("type", "type")],
        "py" | "pyi" => &[("def", "fn"), ("class", "class")],
        "js" | "jsx" | "mjs" | "cjs" | "ts" | "tsx" => &[("function", "fn"), ("class", "class"), ("interface", "interface"), ("enum", "enum"), ("type", "type")],
        "go" => &[("func", "fn"), ("type", "type")],
        "java" | "kt" | "cs" | "swift" | "scala" => &[("class", "class"), ("interface", "interface"), ("enum", "enum"), ("struct", "struct"), ("fun", "fn"), ("func", "fn"), ("object", "class")],
        "c" | "h" | "cpp" | "cc" | "hpp" => &[("struct", "struct"), ("class", "class"), ("enum", "enum"), ("namespace", "mod")],
        "rb" => &[("def", "fn"), ("class", "class"), ("module", "mod")],
        "php" => &[("function", "fn"), ("class", "class"), ("interface", "interface"), ("trait", "trait")],
        "sh" | "bash" | "zsh" => &[("function", "fn")],
        _ => &[("fn", "fn"), ("def", "fn"), ("function", "fn"), ("func", "fn"), ("class", "class"), ("struct", "struct"), ("interface", "interface"), ("enum", "enum")],
    }
}

/// Words that may come in front of the keyword (`pub async fn`, `export default class`).
const MODIFIERS: &[&str] = &["pub", "pub(crate)", "pub(super)", "async", "unsafe", "extern", "export", "default", "abstract", "public", "private", "protected", "static", "final", "open", "internal", "sealed", "declare", "const", "partial"];

fn declarations(ext: &str, text: &str) -> Vec<OutlineItem> {
    let words = keywords(ext);
    let mut out: Vec<OutlineItem> = Vec::new();
    // (indent, depth) of the declarations still open, to nest what is indented under them.
    let mut open: Vec<usize> = Vec::new();
    let mut in_block_comment = false;
    for (line, raw) in text.lines().enumerate() {
        let trimmed = raw.trim_start();
        if in_block_comment {
            in_block_comment = !trimmed.contains("*/");
            continue;
        }
        if trimmed.starts_with("/*") {
            in_block_comment = !trimmed.contains("*/");
            continue;
        }
        if trimmed.is_empty() || trimmed.starts_with("//") || trimmed.starts_with('#') && ext != "rs" && ext != "c" && ext != "h" && ext != "cpp" || trimmed.starts_with("///") {
            continue;
        }
        let indent = raw.len() - trimmed.len();
        let mut rest = trimmed;
        // Skip modifiers and attributes such as `#[test]` on the same line.
        loop {
            let word = rest.split_whitespace().next().unwrap_or("");
            if MODIFIERS.contains(&word) && rest.len() > word.len() {
                rest = rest[word.len()..].trim_start();
            } else {
                break;
            }
        }
        let Some(first) = rest.split(|c: char| c.is_whitespace() || c == '(' || c == '<' || c == '{').next() else { continue };
        let Some(&(_, kind)) = words.iter().find(|(w, _)| *w == first) else {
            // `const name = () =>` and `let name = function` in JavaScript and TypeScript.
            if matches!(ext, "js" | "jsx" | "ts" | "tsx" | "mjs") {
                if let Some(item) = arrow_function(trimmed, line, indent) {
                    nest(&mut open, &mut out, indent, item);
                }
            }
            continue;
        };
        // `const` and `static` in Rust are only worth listing at the top level of a module or impl.
        let after = rest[first.len()..].trim_start();
        let name: String = if kind == "impl" {
            after.split('{').next().unwrap_or(after).trim().trim_end_matches("where").trim().to_owned()
        } else {
            after.chars().take_while(|c| c.is_alphanumeric() || *c == '_' || *c == '$' || *c == '.' && ext == "go").collect()
        };
        // Go methods read `func (r *T) Name(`: take the name after the receiver.
        let name = if ext == "go" && name.is_empty() && after.starts_with('(') {
            let after_recv = after.split_once(')').map(|(_, r)| r.trim_start()).unwrap_or("");
            after_recv.chars().take_while(|c| c.is_alphanumeric() || *c == '_').collect()
        } else {
            name
        };
        if name.is_empty() {
            continue;
        }
        let col = raw.find(&name).unwrap_or(indent);
        nest(&mut open, &mut out, indent, OutlineItem { name, kind, line, col: raw[..col].chars().count(), depth: 0 });
    }
    out
}

fn arrow_function(trimmed: &str, line: usize, indent: usize) -> Option<OutlineItem> {
    let rest = ["export const ", "export let ", "const ", "let ", "var "].iter().find_map(|p| trimmed.strip_prefix(p))?;
    let name: String = rest.chars().take_while(|c| c.is_alphanumeric() || *c == '_' || *c == '$').collect();
    let after = rest[name.len()..].trim_start();
    let after = after.strip_prefix(':').map_or(after, |_| after.split_once('=').map_or("", |(_, r)| r.trim_start().trim_start_matches('=').trim_start()));
    let after = after.strip_prefix('=').unwrap_or(after).trim_start();
    let is_function = after.starts_with("function") || after.starts_with("async") || after.starts_with('(') && after.contains("=>") || after.contains("=>") && !after.contains(';') && after.split("=>").next().is_some_and(|a| !a.contains('{') || a.trim_start().starts_with('('));
    (!name.is_empty() && is_function).then_some(OutlineItem { name, kind: "fn", line, col: indent + trimmed.find(rest).unwrap_or(0), depth: 0 })
}

/// Give `item` its depth from the declarations whose indentation is smaller, and add it.
fn nest(open: &mut Vec<usize>, out: &mut Vec<OutlineItem>, indent: usize, mut item: OutlineItem) {
    while open.last().is_some_and(|last| *last >= indent) {
        open.pop();
    }
    item.depth = open.len();
    open.push(indent);
    out.push(item);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(items: &[OutlineItem]) -> Vec<(usize, &str, &str)> {
        items.iter().map(|i| (i.depth, i.kind, i.name.as_str())).collect()
    }

    #[test]
    fn markdown_headings_nest_by_level_and_skip_code_fences() {
        let text = "# Title\n\nintro\n\n## Part one\n```\n# not a heading\n```\n### Deep ###\n## Part two\n#nospace\n";
        let items = scan(Path::new("a.md"), text);
        assert_eq!(names(&items), vec![(0, "#", "Title"), (1, "#", "Part one"), (2, "#", "Deep"), (1, "#", "Part two")]);
        assert_eq!(items[1].line, 4);
    }

    #[test]
    fn rust_items_nest_inside_impls_and_modules() {
        let text = "pub struct Point { x: i32 }\n\nimpl Point {\n    pub fn new() -> Self { todo!() }\n    async fn load(&self) {}\n}\n\n/// doc\npub(crate) enum Kind { A }\nmod inner {\n    fn helper() {}\n}\n";
        let items = scan(Path::new("lib.rs"), text);
        assert_eq!(
            names(&items),
            vec![(0, "struct", "Point"), (0, "impl", "Point"), (1, "fn", "new"), (1, "fn", "load"), (0, "enum", "Kind"), (0, "mod", "inner"), (1, "fn", "helper")]
        );
        assert_eq!((items[2].line, items[2].col), (3, 11));
    }

    #[test]
    fn python_and_typescript_declarations() {
        let py = scan(Path::new("a.py"), "class A:\n    def f(self):\n        pass\n\n# def commented():\ndef g():\n    pass\n");
        assert_eq!(names(&py), vec![(0, "class", "A"), (1, "fn", "f"), (0, "fn", "g")]);
        let ts = scan(Path::new("a.ts"), "export class View {\n  render() {}\n}\nexport const make = (a: number) => a;\nconst x = 5;\nexport default function main() {}\n");
        assert_eq!(names(&ts), vec![(0, "class", "View"), (0, "fn", "make"), (0, "fn", "main")]);
    }

    #[test]
    fn go_methods_take_the_name_after_the_receiver() {
        let items = scan(Path::new("a.go"), "type S struct{}\nfunc (s *S) Run() {}\nfunc Top() {}\n");
        assert_eq!(names(&items), vec![(0, "type", "S"), (0, "fn", "Run"), (0, "fn", "Top")]);
    }

    #[test]
    fn huge_files_are_skipped() {
        assert!(scan(Path::new("a.rs"), &"fn a() {}\n".repeat(300_000)).is_empty());
    }
}
