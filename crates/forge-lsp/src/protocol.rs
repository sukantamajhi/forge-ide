//! Typed views of the LSP messages Forge uses, parsed leniently from JSON.
//!
//! Servers vary in which optional fields they send, so every parser here tolerates
//! missing fields and skips malformed entries instead of failing the whole message.

use std::path::{Path, PathBuf};

use serde_json::{json, Value};
use url::Url;

/// A position in LSP terms: zero-based line, UTF-16 code unit column.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Position {
    pub line: usize,
    pub character: usize,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Range {
    pub start: Position,
    pub end: Position,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Severity {
    Error,
    Warning,
    Information,
    Hint,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Diagnostic {
    pub range: Range,
    pub severity: Severity,
    pub message: String,
    pub source: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Location {
    pub path: PathBuf,
    pub range: Range,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TextEditSpec {
    pub range: Range,
    pub new_text: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CompletionItem {
    pub label: String,
    pub detail: Option<String>,
    pub kind: Option<u32>,
    pub sort_text: Option<String>,
    /// Edit to apply when accepted; takes priority over `insert_text`.
    pub edit: Option<TextEditSpec>,
    pub insert_text: Option<String>,
}

/// One change of a `textDocument/didChange` notification.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ContentChange {
    pub range: Range,
    pub text: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProgressKind {
    Begin,
    Report,
    End,
}

/// A `$/progress` work-done update, e.g. rust-analyzer indexing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Progress {
    pub token: String,
    pub kind: ProgressKind,
    pub title: Option<String>,
    pub message: Option<String>,
}

pub fn path_to_uri(path: &Path) -> Option<String> {
    Url::from_file_path(path).ok().map(String::from)
}

pub fn uri_to_path(uri: &str) -> Option<PathBuf> {
    Url::parse(uri).ok()?.to_file_path().ok()
}

fn position(value: &Value) -> Option<Position> {
    Some(Position {
        line: value.get("line")?.as_u64()? as usize,
        character: value.get("character")?.as_u64()? as usize,
    })
}

fn range(value: &Value) -> Option<Range> {
    Some(Range { start: position(value.get("start")?)?, end: position(value.get("end")?)? })
}

fn range_json(r: Range) -> Value {
    json!({
        "start": { "line": r.start.line, "character": r.start.character },
        "end": { "line": r.end.line, "character": r.end.character },
    })
}

fn string_field(value: &Value, key: &str) -> Option<String> {
    value.get(key)?.as_str().map(str::to_owned)
}

/// `textDocument/publishDiagnostics` params. `None` when the URI is not a file URI.
pub fn parse_diagnostics(params: &Value) -> Option<(PathBuf, Vec<Diagnostic>)> {
    let path = uri_to_path(params.get("uri")?.as_str()?)?;
    let diagnostics = params
        .get("diagnostics")?
        .as_array()?
        .iter()
        .filter_map(|d| {
            let severity = match d.get("severity").and_then(Value::as_u64) {
                Some(2) => Severity::Warning,
                Some(3) => Severity::Information,
                Some(4) => Severity::Hint,
                // Severity is optional; servers leave the interpretation to the client.
                _ => Severity::Error,
            };
            Some(Diagnostic {
                range: range(d.get("range")?)?,
                severity,
                message: string_field(d, "message")?,
                source: string_field(d, "source"),
            })
        })
        .collect();
    Some((path, diagnostics))
}

/// `textDocument/completion` result: a bare array or a `CompletionList`.
pub fn parse_completion(result: &Value) -> Vec<CompletionItem> {
    let items = result.get("items").unwrap_or(result);
    let Some(items) = items.as_array() else { return Vec::new() };
    let mut parsed: Vec<CompletionItem> = items
        .iter()
        .filter_map(|item| {
            let edit = item.get("textEdit").and_then(|edit| {
                // InsertReplaceEdit carries `insert` and `replace` ranges; use `insert`.
                let r = edit.get("range").or_else(|| edit.get("insert"))?;
                Some(TextEditSpec { range: range(r)?, new_text: string_field(edit, "newText")? })
            });
            Some(CompletionItem {
                label: string_field(item, "label")?,
                detail: string_field(item, "detail"),
                kind: item.get("kind").and_then(Value::as_u64).map(|k| k as u32),
                sort_text: string_field(item, "sortText"),
                edit,
                insert_text: string_field(item, "insertText"),
            })
        })
        .collect();
    parsed.sort_by(|a, b| {
        a.sort_text.as_deref().unwrap_or(&a.label).cmp(b.sort_text.as_deref().unwrap_or(&b.label))
    });
    parsed
}

/// `textDocument/definition` result: `Location`, `Location[]` or `LocationLink[]`.
pub fn parse_locations(result: &Value) -> Vec<Location> {
    let one = |value: &Value| -> Option<Location> {
        let (uri, r) = match value.get("targetUri") {
            Some(uri) => (uri, value.get("targetSelectionRange").or_else(|| value.get("targetRange"))?),
            None => (value.get("uri")?, value.get("range")?),
        };
        Some(Location { path: uri_to_path(uri.as_str()?)?, range: range(r)? })
    };
    match result {
        Value::Array(all) => all.iter().filter_map(one).collect(),
        Value::Object(_) => one(result).into_iter().collect(),
        _ => Vec::new(),
    }
}

pub fn parse_progress(params: &Value) -> Option<Progress> {
    let token = match params.get("token")? {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    };
    let value = params.get("value")?;
    let kind = match value.get("kind")?.as_str()? {
        "begin" => ProgressKind::Begin,
        "report" => ProgressKind::Report,
        "end" => ProgressKind::End,
        _ => return None,
    };
    Some(Progress { token, kind, title: string_field(value, "title"), message: string_field(value, "message") })
}

/// The reply to a request a server sends to the client. Servers block on these, so
/// unknown methods get a null result rather than silence.
pub fn answer_server_request(method: &str, params: &Value) -> Value {
    match method {
        // One (null) setting per requested item.
        "workspace/configuration" => {
            let count = params.get("items").and_then(Value::as_array).map_or(0, Vec::len);
            Value::Array(vec![Value::Null; count])
        }
        // Forge applies edits from its own requests; it declines ones a server pushes.
        "workspace/applyEdit" => json!({ "applied": false }),
        _ => Value::Null,
    }
}

/// A symbol from `textDocument/documentSymbol`, flattened with its nesting depth.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Symbol {
    pub name: String,
    pub kind: u32,
    pub range: Range,
    pub depth: usize,
}

impl Symbol {
    /// Short label for a symbol kind, as shown in the outline.
    pub fn kind_label(&self) -> &'static str {
        match self.kind {
            2..=4 => "mod",
            5 => "class",
            6 | 12 => "fn",
            7 | 8 => "field",
            9 | 10 => "enum",
            11 => "trait",
            13 | 14 => "const",
            22 => "variant",
            23 => "struct",
            _ => "sym",
        }
    }
}

/// A file's worth of edits from a `WorkspaceEdit`.
pub type FileEdits = (PathBuf, Vec<TextEditSpec>);

#[derive(Clone, Debug, PartialEq)]
pub struct CodeAction {
    pub title: String,
    pub kind: Option<String>,
    pub preferred: bool,
    pub edit: Vec<FileEdits>,
    /// A server-side command to run; Forge applies `edit` but cannot run commands.
    pub command: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SignatureHelp {
    pub label: String,
    /// Char range of the active parameter within `label`.
    pub active: Option<(usize, usize)>,
    pub documentation: Option<String>,
}

fn markup_text(value: &Value) -> Option<String> {
    match value {
        Value::String(s) => Some(s.clone()),
        Value::Object(o) => o.get("value").and_then(Value::as_str).map(str::to_owned),
        _ => None,
    }
}

/// `textDocument/hover` result as plain text (markdown fences are stripped).
pub fn parse_hover(result: &Value) -> Option<String> {
    let contents = result.get("contents")?;
    let text = match contents {
        Value::Array(parts) => parts.iter().filter_map(markup_text).collect::<Vec<_>>().join("\n\n"),
        other => markup_text(other)?,
    };
    let text = text.replace("```rust", "").replace("```", "").trim().to_owned();
    (!text.is_empty()).then_some(text)
}

pub fn parse_signature_help(result: &Value) -> Option<SignatureHelp> {
    let signatures = result.get("signatures")?.as_array()?;
    let which = result.get("activeSignature").and_then(Value::as_u64).unwrap_or(0) as usize;
    let sig = signatures.get(which).or_else(|| signatures.first())?;
    let label = string_field(sig, "label")?;
    let active_index = sig.get("activeParameter").or_else(|| result.get("activeParameter")).and_then(Value::as_u64);
    let active = active_index.and_then(|i| {
        let param = sig.get("parameters")?.as_array()?.get(i as usize)?;
        match param.get("label")? {
            Value::String(text) => label.find(text.as_str()).map(|byte| {
                let start = label[..byte].chars().count();
                (start, start + text.chars().count())
            }),
            Value::Array(pair) => Some((pair.first()?.as_u64()? as usize, pair.get(1)?.as_u64()? as usize)),
            _ => None,
        }
    });
    Some(SignatureHelp { label, active, documentation: sig.get("documentation").and_then(markup_text) })
}

pub fn parse_text_edits(value: &Value) -> Vec<TextEditSpec> {
    let Some(edits) = value.as_array() else { return Vec::new() };
    edits
        .iter()
        .filter_map(|e| Some(TextEditSpec { range: range(e.get("range")?)?, new_text: string_field(e, "newText")? }))
        .collect()
}

/// A `WorkspaceEdit`: both the `changes` map and `documentChanges` forms. File
/// create/rename/delete operations are skipped.
pub fn parse_workspace_edit(edit: &Value) -> Vec<FileEdits> {
    let mut out: Vec<FileEdits> = Vec::new();
    if let Some(changes) = edit.get("changes").and_then(Value::as_object) {
        for (uri, edits) in changes {
            if let Some(path) = uri_to_path(uri) {
                out.push((path, parse_text_edits(edits)));
            }
        }
    }
    if let Some(changes) = edit.get("documentChanges").and_then(Value::as_array) {
        for change in changes {
            let uri = change.get("textDocument").and_then(|t| t.get("uri")).and_then(Value::as_str);
            let (Some(uri), Some(edits)) = (uri, change.get("edits")) else { continue };
            if let Some(path) = uri_to_path(uri) {
                out.push((path, parse_text_edits(edits)));
            }
        }
    }
    out
}

/// `textDocument/documentSymbol`: hierarchical `DocumentSymbol[]` or flat `SymbolInformation[]`.
pub fn parse_document_symbols(result: &Value) -> Vec<Symbol> {
    fn walk(items: &[Value], depth: usize, out: &mut Vec<Symbol>) {
        for item in items {
            let (Some(name), Some(kind)) = (string_field(item, "name"), item.get("kind").and_then(Value::as_u64)) else { continue };
            let r = item
                .get("selectionRange")
                .or_else(|| item.get("range"))
                .or_else(|| item.get("location").and_then(|l| l.get("range")))
                .and_then(range);
            let Some(r) = r else { continue };
            out.push(Symbol { name, kind: kind as u32, range: r, depth });
            if let Some(children) = item.get("children").and_then(Value::as_array) {
                walk(children, depth + 1, out);
            }
        }
    }
    let mut out = Vec::new();
    if let Some(items) = result.as_array() {
        walk(items, 0, &mut out);
    }
    out
}

pub fn parse_code_actions(result: &Value) -> Vec<CodeAction> {
    let Some(items) = result.as_array() else { return Vec::new() };
    items
        .iter()
        .filter_map(|item| {
            let title = string_field(item, "title")?;
            // A bare `Command` has a string `command`; a CodeAction nests it.
            let command = match item.get("command") {
                Some(Value::String(c)) => Some(c.clone()),
                Some(obj) => string_field(obj, "command"),
                None => None,
            };
            Some(CodeAction {
                title,
                kind: string_field(item, "kind"),
                preferred: item.get("isPreferred").and_then(Value::as_bool).unwrap_or(false),
                edit: item.get("edit").map(parse_workspace_edit).unwrap_or_default(),
                command,
            })
        })
        .collect()
}

pub fn diagnostic_json(d: &Diagnostic) -> Value {
    let severity = match d.severity {
        Severity::Error => 1,
        Severity::Warning => 2,
        Severity::Information => 3,
        Severity::Hint => 4,
    };
    json!({ "range": range_json(d.range), "severity": severity, "message": d.message, "source": d.source })
}

pub fn rename_params(uri: &str, at: Position, new_name: &str) -> Value {
    let mut params = text_document_position(uri, at);
    params["newName"] = json!(new_name);
    params
}

pub fn references_params(uri: &str, at: Position) -> Value {
    let mut params = text_document_position(uri, at);
    params["context"] = json!({ "includeDeclaration": true });
    params
}

pub fn code_action_params(uri: &str, at: Range, diagnostics: &[Diagnostic]) -> Value {
    json!({
        "textDocument": { "uri": uri },
        "range": range_json(at),
        "context": { "diagnostics": diagnostics.iter().map(diagnostic_json).collect::<Vec<_>>() },
    })
}

pub fn formatting_params(uri: &str, tab_size: usize, insert_spaces: bool) -> Value {
    json!({ "textDocument": { "uri": uri }, "options": { "tabSize": tab_size, "insertSpaces": insert_spaces } })
}

pub fn document_symbol_params(uri: &str) -> Value {
    json!({ "textDocument": { "uri": uri } })
}

pub fn did_open_params(uri: &str, language_id: &str, version: i32, text: &str) -> Value {
    json!({ "textDocument": { "uri": uri, "languageId": language_id, "version": version, "text": text } })
}

pub fn did_change_params(uri: &str, version: i32, changes: &[ContentChange]) -> Value {
    let changes: Vec<Value> =
        changes.iter().map(|c| json!({ "range": range_json(c.range), "text": c.text })).collect();
    json!({ "textDocument": { "uri": uri, "version": version }, "contentChanges": changes })
}

pub fn text_document_position(uri: &str, at: Position) -> Value {
    json!({
        "textDocument": { "uri": uri },
        "position": { "line": at.line, "character": at.character },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uri_roundtrip_handles_spaces_and_unicode() {
        let path = if cfg!(windows) { PathBuf::from(r"C:\My Proj\é.rs") } else { PathBuf::from("/my proj/é.rs") };
        let uri = path_to_uri(&path).unwrap();
        assert!(uri.starts_with("file:///"), "{uri}");
        assert!(uri.contains("%20"));
        assert_eq!(uri_to_path(&uri).unwrap(), path);
        assert!(uri_to_path("https://example.com/a.rs").is_none());
    }

    #[test]
    fn diagnostics_parse_with_defaults_and_skip_malformed() {
        let uri = path_to_uri(&std::env::temp_dir().join("a.rs")).unwrap();
        let params = json!({
            "uri": uri,
            "diagnostics": [
                { "range": {"start":{"line":1,"character":2},"end":{"line":1,"character":5}},
                  "severity": 2, "message": "unused", "source": "rustc" },
                { "range": {"start":{"line":0,"character":0},"end":{"line":0,"character":1}}, "message": "no severity" },
                { "message": "no range" }
            ]
        });
        let (path, diags) = parse_diagnostics(&params).unwrap();
        assert_eq!(path, std::env::temp_dir().join("a.rs"));
        assert_eq!(diags.len(), 2);
        assert_eq!(diags[0].severity, Severity::Warning);
        assert_eq!(diags[0].range.start, Position { line: 1, character: 2 });
        assert_eq!(diags[1].severity, Severity::Error);
        assert!(parse_diagnostics(&json!({"uri": "untitled:1", "diagnostics": []})).is_none());
    }

    #[test]
    fn completion_accepts_list_or_array_and_sorts() {
        let list = json!({ "isIncomplete": false, "items": [
            { "label": "zeta", "sortText": "2" },
            { "label": "alpha", "kind": 3, "detail": "fn()", "sortText": "1",
              "textEdit": { "range": {"start":{"line":0,"character":0},"end":{"line":0,"character":2}}, "newText": "alpha()" } },
            { "label": "ins", "textEdit": { "newText": "ins",
              "insert": {"start":{"line":0,"character":1},"end":{"line":0,"character":2}},
              "replace": {"start":{"line":0,"character":1},"end":{"line":0,"character":9}} } },
            { "kind": 1 }
        ]});
        let items = parse_completion(&list);
        let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
        // Sorted by sortText, falling back to the label; the item without a label is dropped.
        assert_eq!(labels, ["alpha", "zeta", "ins"]);
        assert_eq!(items[0].edit.as_ref().unwrap().new_text, "alpha()");
        // InsertReplaceEdit uses the `insert` range.
        let ins = items.iter().find(|i| i.label == "ins").unwrap();
        assert_eq!(ins.edit.as_ref().unwrap().range.end.character, 2);

        assert_eq!(parse_completion(&json!([{ "label": "x" }])).len(), 1);
        assert!(parse_completion(&Value::Null).is_empty());
    }

    #[test]
    fn locations_parse_all_three_shapes() {
        let path = std::env::temp_dir().join("t.rs");
        let uri = path_to_uri(&path).unwrap();
        let r = json!({"start":{"line":4,"character":1},"end":{"line":4,"character":3}});
        let single = json!({ "uri": uri, "range": r });
        let link = json!({ "targetUri": uri, "targetRange": r, "targetSelectionRange": r });

        assert_eq!(parse_locations(&single).len(), 1);
        assert_eq!(parse_locations(&json!([single, link])).len(), 2);
        assert_eq!(parse_locations(&json!([link]))[0].range.start.line, 4);
        assert!(parse_locations(&Value::Null).is_empty());
    }

    #[test]
    fn progress_and_server_requests() {
        let begin = json!({ "token": "rustAnalyzer/Indexing", "value": { "kind": "begin", "title": "Indexing" } });
        let p = parse_progress(&begin).unwrap();
        assert_eq!((p.kind, p.title.as_deref()), (ProgressKind::Begin, Some("Indexing")));
        assert_eq!(parse_progress(&json!({ "token": 7, "value": {"kind": "end"} })).unwrap().token, "7");

        let cfg = answer_server_request("workspace/configuration", &json!({ "items": [{}, {}, {}] }));
        assert_eq!(cfg, json!([null, null, null]));
        assert_eq!(answer_server_request("client/registerCapability", &json!({})), Value::Null);
        assert_eq!(answer_server_request("workspace/applyEdit", &json!({})), json!({ "applied": false }));
    }

    #[test]
    fn did_change_carries_ranges_and_text() {
        let change = ContentChange {
            range: Range { start: Position { line: 1, character: 0 }, end: Position { line: 1, character: 2 } },
            text: "x".into(),
        };
        let params = did_change_params("file:///a", 3, &[change]);
        assert_eq!(params["textDocument"]["version"], 3);
        assert_eq!(params["contentChanges"][0]["range"]["end"]["character"], 2);
        assert_eq!(params["contentChanges"][0]["text"], "x");
    }

    #[test]
    fn hover_accepts_markup_strings_and_arrays() {
        let markup = json!({ "contents": { "kind": "markdown", "value": "```rust\nfn f()\n```\n\nDocs" } });
        assert_eq!(parse_hover(&markup).unwrap(), "fn f()\n\n\nDocs");
        assert_eq!(parse_hover(&json!({ "contents": ["a", { "language": "x", "value": "b" }] })).unwrap(), "a\n\nb");
        assert!(parse_hover(&json!({ "contents": "" })).is_none());
        assert!(parse_hover(&Value::Null).is_none());
    }

    #[test]
    fn signature_help_finds_the_active_parameter_range() {
        let by_text = json!({ "signatures": [{ "label": "f(a: i32, b: u8)", "activeParameter": 1,
            "parameters": [{ "label": "a: i32" }, { "label": "b: u8" }] }] });
        let h = parse_signature_help(&by_text).unwrap();
        assert_eq!(h.active, Some((10, 15)));
        let by_offsets = json!({ "activeParameter": 0, "signatures": [{ "label": "g(x)", "parameters": [{ "label": [2, 3] }] }] });
        assert_eq!(parse_signature_help(&by_offsets).unwrap().active, Some((2, 3)));
        assert!(parse_signature_help(&json!({ "signatures": [] })).is_none());
    }

    #[test]
    fn workspace_edits_parse_both_shapes() {
        let path = std::env::temp_dir().join("w.rs");
        let uri = path_to_uri(&path).unwrap();
        let edit = json!({ "range": {"start":{"line":0,"character":0},"end":{"line":0,"character":1}}, "newText": "z" });
        let changes = json!({ "changes": { uri.clone(): [edit.clone()] } });
        let doc_changes = json!({ "documentChanges": [{ "textDocument": { "uri": uri, "version": 1 }, "edits": [edit] }, { "kind": "rename" }] });
        for form in [changes, doc_changes] {
            let parsed = parse_workspace_edit(&form);
            assert_eq!(parsed.len(), 1);
            assert_eq!(parsed[0].0, path);
            assert_eq!(parsed[0].1[0].new_text, "z");
        }
    }

    #[test]
    fn document_symbols_flatten_with_depth_and_fall_back_to_flat_form() {
        let r = json!({"start":{"line":0,"character":0},"end":{"line":3,"character":1}});
        let nested = json!([{ "name": "S", "kind": 23, "range": r, "selectionRange": r,
            "children": [{ "name": "f", "kind": 6, "range": r, "selectionRange": r }] }]);
        let symbols = parse_document_symbols(&nested);
        let shape: Vec<(&str, usize, &str)> = symbols.iter().map(|s| (s.name.as_str(), s.depth, s.kind_label())).collect();
        assert_eq!(shape, [("S", 0, "struct"), ("f", 1, "fn")]);
        let flat = json!([{ "name": "g", "kind": 12, "location": { "uri": "file:///a", "range": r } }]);
        assert_eq!(parse_document_symbols(&flat).len(), 1);
    }

    #[test]
    fn code_actions_carry_edits_and_commands() {
        let path = std::env::temp_dir().join("c.rs");
        let uri = path_to_uri(&path).unwrap();
        let actions = json!([
            { "title": "Fix", "kind": "quickfix", "isPreferred": true, "edit": { "changes": { uri: [
                { "range": {"start":{"line":0,"character":0},"end":{"line":0,"character":0}}, "newText": "x" }] } } },
            { "title": "Run", "command": { "title": "Run", "command": "rust-analyzer.run" } },
            { "title": "Bare", "command": "do.it" },
            { "kind": "no title" }
        ]);
        let parsed = parse_code_actions(&actions);
        assert_eq!(parsed.len(), 3);
        assert!(parsed[0].preferred && parsed[0].edit[0].1.len() == 1);
        assert_eq!(parsed[1].command.as_deref(), Some("rust-analyzer.run"));
        assert_eq!(parsed[2].command.as_deref(), Some("do.it"));
    }

    #[test]
    fn request_params_have_the_documented_shapes() {
        let at = Position { line: 2, character: 3 };
        assert_eq!(rename_params("file:///a", at, "n")["newName"], "n");
        assert_eq!(references_params("file:///a", at)["context"]["includeDeclaration"], true);
        let d = Diagnostic { range: Range::default(), severity: Severity::Warning, message: "m".into(), source: None };
        let p = code_action_params("file:///a", Range::default(), &[d]);
        assert_eq!(p["context"]["diagnostics"][0]["severity"], 2);
        assert_eq!(formatting_params("file:///a", 2, false)["options"]["insertSpaces"], false);
    }
}
