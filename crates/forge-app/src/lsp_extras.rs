//! What a language server adds on top of the text: semantic tokens (colours that know what a name is) and inlay hints
//! (inferred types and parameter names shown inside the line). Both are asked for when the file settles after a change.

use std::time::Duration;

use forge_lsp::{inlay_hint_params, parse_inlay_hints, parse_semantic_tokens};
use forge_syntax::{HighlightClass, Span};
use serde_json::{json, Value};
use std::time::Instant;

use crate::lsp::RequestTag;
use crate::workspace::{same_path, Workspace};

/// How long a file has to stay unchanged before the server is asked again.
const SETTLE: Duration = Duration::from_millis(400);
/// Files with more lines than this get no hints (the request covers the whole file).
const MAX_HINT_LINES: usize = 6000;

/// The colour class for a semantic token type, or `None` to leave the syntax colour alone. Plain variables and
/// parameters are left to the syntax: servers also call immutable ones constants, which would paint them orange.
pub fn semantic_class(kind: &str) -> Option<HighlightClass> {
    Some(match kind {
        "namespace" | "type" | "class" | "enum" | "interface" | "struct" | "typeParameter" | "builtinType" | "typeAlias" | "selfTypeKeyword" | "generic" => HighlightClass::Type,
        "enumMember" | "constParameter" => HighlightClass::Constant,
        "function" | "method" | "macro" => HighlightClass::Function,
        "decorator" | "attribute" | "derive" | "builtinAttribute" | "deriveHelper" | "lifetime" => HighlightClass::Attribute,
        "property" => HighlightClass::Property,
        "keyword" | "modifier" | "selfKeyword" => HighlightClass::Keyword,
        "string" | "regexp" => HighlightClass::String,
        "number" => HighlightClass::Number,
        "comment" => HighlightClass::Comment,
        "operator" => HighlightClass::Operator,
        "formatSpecifier" => HighlightClass::Escape,
        _ => return None,
    })
}

impl Workspace {
    /// Ask the server for semantic tokens and inlay hints of the open file once it has settled. True if a change on
    /// screen follows (a switched-off feature drops what it had).
    pub fn refresh_lsp_extras(&mut self) -> bool {
        let want_semantic = self.settings.semantic_highlighting;
        let want_hints = self.settings.inlay_hints;
        let mut changed = false;
        if !want_semantic || !want_hints {
            for doc in &mut self.docs {
                if !want_semantic && !doc.semantic.is_empty() {
                    doc.semantic.clear();
                    doc.revision += 1;
                    changed = true;
                }
                if !want_hints && !doc.hints.is_empty() {
                    doc.hints.clear();
                    changed = true;
                }
            }
        }
        if !want_semantic && !want_hints {
            return changed;
        }
        if self.extras_retry_at.is_some_and(|at| at > Instant::now()) {
            return changed;
        }
        let Some(doc) = self.active_doc().filter(|d| d.lsp_open && !d.readonly && !d.diff) else { return changed };
        let key = (doc.path.clone(), doc.revision);
        if self.extras_key.as_ref() == Some(&key) {
            return changed;
        }
        if self.extras_seen.as_ref() != Some(&key) {
            let other_file = self.extras_seen.as_ref().is_none_or(|(p, _)| p != &key.0);
            self.extras_seen = Some(key.clone());
            self.extras_changed = Instant::now();
            // A different file is asked at once; the same file waits for typing to pause.
            if !other_file {
                return changed;
            }
        } else if self.extras_changed.elapsed() < SETTLE {
            return changed;
        }
        let Some((language, uri, _)) = self.lsp_target() else { return changed };
        let lines = self.active_doc().map_or(0, |d| d.buffer.len_lines());
        self.extras_key = Some(key.clone());
        self.extras_requested = Some(key);
        if want_semantic && self.lsp.semantic_legend(language).is_some() {
            self.lsp.request(language, "textDocument/semanticTokens/full", json!({ "textDocument": { "uri": uri } }), RequestTag::SemanticTokens);
        }
        if want_hints && lines <= MAX_HINT_LINES && self.lsp.supports_inlay_hints(language) {
            // The range is the whole file, ending where the text ends.
            let end = self.active_doc().map_or((0, 0), |d| d.buffer.utf16_point(d.buffer.len_chars()));
            self.lsp.request(language, "textDocument/inlayHint", inlay_hint_params(&uri, end), RequestTag::InlayHints);
        }
        changed
    }

    /// The document an answer belongs to, if it has not changed since it was asked.
    fn extras_target(&self) -> Option<usize> {
        let (path, revision) = self.extras_requested.as_ref()?;
        let ix = self.docs.iter().position(|d| same_path(&d.path, path))?;
        (self.docs[ix].revision == *revision).then_some(ix)
    }

    pub fn apply_semantic_tokens(&mut self, result: Value) {
        let Some(ix) = self.extras_target() else { return };
        let Some(language) = self.docs[ix].language else { return };
        let Some(legend) = self.lsp.semantic_legend(language) else { return };
        let tokens = parse_semantic_tokens(&result, &legend);
        let doc = &mut self.docs[ix];
        let mut lines: Vec<Vec<Span>> = vec![Vec::new(); doc.buffer.len_lines()];
        for token in tokens {
            let Some(class) = semantic_class(&token.kind) else { continue };
            if token.line >= lines.len() {
                continue;
            }
            let line_start = doc.buffer.line_start(token.line);
            let start = doc.buffer.offset_from_utf16(token.line, token.start) - line_start;
            let end = doc.buffer.offset_from_utf16(token.line, token.start + token.length) - line_start;
            if end > start {
                lines[token.line].push(Span { start_col: start, end_col: end, class });
            }
        }
        doc.semantic = lines;
        doc.semantic_skip.clear();
        // The colours of the lines on screen were cached for this revision.
        doc.span_cache.replace((u64::MAX, 0, Vec::new()));
    }

    pub fn apply_inlay_hints(&mut self, result: Value) {
        let Some(ix) = self.extras_target() else { return };
        let hints = parse_inlay_hints(&result);
        let doc = &mut self.docs[ix];
        let total = doc.buffer.len_lines();
        let mut by_line: std::collections::HashMap<usize, Vec<(usize, String)>> = std::collections::HashMap::new();
        for hint in hints.into_iter().filter(|h| h.line < total) {
            let line_start = doc.buffer.line_start(hint.line);
            let col = doc.buffer.offset_from_utf16(hint.line, hint.character) - line_start;
            let label: String = hint.label.chars().take(80).collect();
            by_line.entry(hint.line).or_default().push((col, label));
        }
        doc.hints = by_line;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_types_map_to_colour_classes() {
        assert_eq!(semantic_class("function"), Some(HighlightClass::Function));
        assert_eq!(semantic_class("struct"), Some(HighlightClass::Type));
        assert_eq!(semantic_class("enumMember"), Some(HighlightClass::Constant));
        assert_eq!(semantic_class("unresolvedReference"), None);
        assert_eq!(semantic_class("variable"), None);
        assert_eq!(semantic_class("angle"), None);
    }
}
