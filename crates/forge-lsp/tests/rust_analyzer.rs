//! End-to-end check against a real rust-analyzer. Skipped when it is not installed.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use forge_lsp::{
    did_change_params, did_open_params, document_symbol_params, find_executable, parse_completion,
    parse_diagnostics, parse_document_symbols, parse_hover, parse_locations, parse_signature_help,
    parse_workspace_edit, path_to_uri, references_params, rename_params, text_document_position,
    ContentChange, Diagnostic, LspClient, Position, Range, ServerMessage,
};
use serde_json::Value;

const SOURCE: &str = "fn helper() -> i32 { 1 }\nfn main() {\n    let x: i32 = \"s\";\n    let _ = helper();\n    let _ = String::ne\n}\n";

struct Probe {
    dir: PathBuf,
}

impl Probe {
    fn new() -> Self {
        let dir = std::env::temp_dir().join(format!("forge-lsp-ra-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("src")).unwrap();
        std::fs::write(dir.join("Cargo.toml"), "[package]\nname = \"probe\"\nversion = \"0.1.0\"\nedition = \"2021\"\n").unwrap();
        std::fs::write(dir.join("src").join("main.rs"), SOURCE).unwrap();
        Self { dir }
    }
}

impl Drop for Probe {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// Answer server requests and collect the latest diagnostics for `file`.
fn pump(client: &mut LspClient, file: &Path, latest: &mut Option<Vec<Diagnostic>>) {
    for message in client.poll() {
        match message {
            ServerMessage::Request { id, method, params } => {
                client.respond(id, forge_lsp::answer_server_request(&method, &params)).unwrap();
            }
            ServerMessage::Notification { method, params } if method == "textDocument/publishDiagnostics" => {
                if let Some((path, diagnostics)) = parse_diagnostics(&params) {
                    if path == file {
                        *latest = Some(diagnostics);
                    }
                }
            }
            ServerMessage::Notification { .. } => {}
        }
    }
}

fn wait_until(
    client: &mut LspClient,
    file: &Path,
    latest: &mut Option<Vec<Diagnostic>>,
    what: &str,
    mut done: impl FnMut(&mut LspClient, &Option<Vec<Diagnostic>>) -> bool,
) {
    let deadline = Instant::now() + Duration::from_secs(120);
    while Instant::now() < deadline {
        pump(client, file, latest);
        if done(client, latest) {
            return;
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    panic!("timed out waiting for {what}; latest diagnostics: {latest:#?}");
}

fn request(client: &mut LspClient, method: &str, params: Value) -> Value {
    client
        .request(method, params)
        .unwrap()
        .recv_timeout(Duration::from_secs(60))
        .expect("response in time")
        .unwrap_or(Value::Null)
}

fn has_mismatch(diagnostics: &Option<Vec<Diagnostic>>) -> bool {
    diagnostics.as_ref().is_some_and(|d| d.iter().any(|d| d.message.contains("mismatched types")))
}

#[test]
fn real_rust_analyzer_diagnostics_completion_definition_and_incremental_sync() {
    let Some(exe) = find_executable("rust-analyzer") else {
        eprintln!("skipped: rust-analyzer is not installed");
        return;
    };
    let probe = Probe::new();
    let file = probe.dir.join("src").join("main.rs");
    let uri = path_to_uri(&file).unwrap();
    let root_uri = path_to_uri(&probe.dir).unwrap();

    let mut client = LspClient::spawn_in(exe.to_str().unwrap(), &[], Some(&probe.dir)).unwrap();
    let init = client.initialize(Some(&root_uri)).expect("initialize");
    assert!(init["capabilities"]["completionProvider"].is_object());
    client.notify("textDocument/didOpen", did_open_params(&uri, "rust", 1, SOURCE)).unwrap();

    let mut latest = None;
    wait_until(&mut client, &file, &mut latest, "a type-mismatch diagnostic", |_, d| has_mismatch(d));
    let mismatch = latest.as_ref().unwrap().iter().find(|d| d.message.contains("mismatched types")).unwrap();
    assert_eq!(mismatch.range.start.line, 2, "diagnostic is on the `let x` line");

    // Definition of `helper` at its call site (line 3, column 12) is line 0.
    let params = text_document_position(&uri, Position { line: 3, character: 12 });
    let mut definition = Vec::new();
    wait_until(&mut client, &file, &mut latest, "a definition", |c, _| {
        definition = parse_locations(&request(c, "textDocument/definition", params.clone()));
        !definition.is_empty()
    });
    assert_eq!(definition[0].path, file);
    assert_eq!(definition[0].range.start.line, 0);

    // Completion after `String::ne` offers `new`.
    let params = text_document_position(&uri, Position { line: 4, character: 22 });
    let mut labels = Vec::new();
    wait_until(&mut client, &file, &mut latest, "completion items", |c, _| {
        let items = parse_completion(&request(c, "textDocument/completion", params.clone()));
        labels = items.into_iter().map(|i| i.label).collect();
        labels.iter().any(|l| l == "new")
    });
    assert!(labels.iter().any(|l| l == "new"), "got {labels:?}");

    // Incremental sync: insert a line at the top of the file (nothing is saved, so the
    // server only knows about it through our ranged edit). `helper`'s definition and its
    // call site must both shift down one line.
    let top = Position { line: 0, character: 0 };
    let change = ContentChange { range: Range { start: top, end: top }, text: "fn added() {}\n".into() };
    client.notify("textDocument/didChange", did_change_params(&uri, 2, &[change])).unwrap();
    let params = text_document_position(&uri, Position { line: 4, character: 12 });
    let mut shifted = Vec::new();
    wait_until(&mut client, &file, &mut latest, "the definition to move down a line", |c, _| {
        shifted = parse_locations(&request(c, "textDocument/definition", params.clone()));
        shifted.first().is_some_and(|l| l.range.start.line == 1)
    });
    assert_eq!(shifted[0].range.start.line, 1);

    // After the inserted line, `helper` is defined on line 1 and called on line 4.
    let on_call = text_document_position(&uri, Position { line: 4, character: 13 });

    let mut hover = None;
    wait_until(&mut client, &file, &mut latest, "hover text", |c, _| {
        hover = parse_hover(&request(c, "textDocument/hover", on_call.clone()));
        hover.is_some()
    });
    assert!(hover.unwrap().contains("helper"));

    let mut references = Vec::new();
    wait_until(&mut client, &file, &mut latest, "references", |c, _| {
        references = parse_locations(&request(c, "textDocument/references", references_params(&uri, Position { line: 1, character: 4 })));
        references.len() >= 2
    });
    assert!(references.iter().any(|l| l.range.start.line == 1) && references.iter().any(|l| l.range.start.line == 4));

    let mut renamed = Vec::new();
    wait_until(&mut client, &file, &mut latest, "rename edits", |c, _| {
        let edit = request(c, "textDocument/rename", rename_params(&uri, Position { line: 1, character: 4 }, "helper_renamed"));
        renamed = parse_workspace_edit(&edit);
        renamed.iter().map(|(_, e)| e.len()).sum::<usize>() >= 2
    });
    assert!(renamed[0].1.iter().all(|e| e.new_text == "helper_renamed"));

    let mut symbols = Vec::new();
    wait_until(&mut client, &file, &mut latest, "document symbols", |c, _| {
        symbols = parse_document_symbols(&request(c, "textDocument/documentSymbol", document_symbol_params(&uri)));
        symbols.iter().any(|s| s.name == "main")
    });
    assert!(symbols.iter().any(|s| s.name == "helper" && s.kind_label() == "fn"));

    // Inside the call's parentheses, signature help describes `helper`.
    let inside = text_document_position(&uri, Position { line: 4, character: 19 });
    let mut signature = None;
    wait_until(&mut client, &file, &mut latest, "signature help", |c, _| {
        signature = parse_signature_help(&request(c, "textDocument/signatureHelp", inside.clone()));
        signature.is_some()
    });
    assert!(signature.unwrap().label.contains("helper"));

    client.shutdown().unwrap();
}
