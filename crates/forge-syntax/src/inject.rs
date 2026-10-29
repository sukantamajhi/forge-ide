//! Finding code written in another language inside a document: `<script>` and `<style>`
//! in HTML, fenced code blocks in Markdown.

use ropey::Rope;
use tree_sitter::{Node, Range as TsRange, Tree};

use crate::Language;

/// Larger embedded regions are left to the host's colors rather than re-parsed on every keystroke.
pub(crate) const MAX_REGION_BYTES: usize = 512 * 1024;

pub(crate) struct Region {
    pub language: Language,
    pub range: TsRange,
}

/// Every embedded region of `tree`, a parse of a `host` document, outermost first.
pub(crate) fn find_regions(host: Language, tree: &Tree, rope: &Rope) -> Vec<Region> {
    let mut regions = Vec::new();
    if !matches!(host, Language::Html | Language::Markdown) {
        return regions;
    }
    let mut cursor = tree.walk();
    let mut descend = true;
    loop {
        let node = cursor.node();
        let mut found = false;
        if descend {
            let region = match host {
                Language::Html => html_region(node, rope),
                _ if node.kind() == "fenced_code_block" => fence_region(node, rope),
                _ => None,
            };
            found = region.is_some() || (host == Language::Markdown && node.kind() == "fenced_code_block");
            regions.extend(region);
        }
        // A region's own contents belong to the embedded language.
        if descend && !found && cursor.goto_first_child() {
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
    regions
}

fn text(rope: &Rope, node: Node) -> String {
    rope.byte_slice(node.byte_range()).to_string()
}

fn html_region(node: Node, rope: &Rope) -> Option<Region> {
    let is_script = match node.kind() {
        "script_element" => true,
        "style_element" => false,
        _ => return None,
    };
    let mut start_tag = String::new();
    let mut body = None;
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        match child.kind() {
            "start_tag" => start_tag = text(rope, child),
            "raw_text" => body = Some(child.range()),
            _ => {}
        }
    }
    let language = if is_script { script_language(&start_tag)? } else { Language::Css };
    Some(Region { language, range: body? })
}

/// The language of a `<script>` from its opening tag. `None` for templates and other non-code.
fn script_language(start_tag: &str) -> Option<Language> {
    let tag = start_tag.to_ascii_lowercase();
    let has = |needles: &[&str]| needles.iter().any(|n| tag.contains(n));
    if has(&["application/json", "ld+json", "importmap"]) {
        Some(Language::Json)
    } else if has(&["text/template", "text/x-template", "text/html", "text/x-handlebars"]) {
        None
    } else if has(&["lang=\"tsx\"", "lang='tsx'"]) {
        Some(Language::TypeScriptReact)
    } else if has(&["lang=\"ts\"", "lang='ts'", "text/typescript"]) {
        Some(Language::TypeScript)
    } else if has(&["lang=\"jsx\"", "lang='jsx'"]) {
        Some(Language::JavaScriptReact)
    } else {
        Some(Language::JavaScript)
    }
}

fn fence_region(node: Node, rope: &Rope) -> Option<Region> {
    let mut info = String::new();
    let mut body = None;
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        match child.kind() {
            "info_string" => info = text(rope, child),
            "code_fence_content" => body = Some(child.range()),
            _ => {}
        }
    }
    let name = info.trim().split(|c: char| c.is_whitespace() || c == '{' || c == ',').next().unwrap_or("");
    Some(Region { language: Language::from_name(name)?, range: body? })
}
