//! Runs a real extension under real Node through Forge's extension host. Skipped when Node
//! is not installed.

use std::path::{Path, PathBuf};
use std::sync::mpsc::Receiver;
use std::time::{Duration, Instant};

use forge_ext_host::host::{find_node, install_host_files, ActivationEvent, ExtHost, Incoming};
use forge_ext_host::load;
use serde_json::{json, Value};

fn fixture() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests").join("fixtures").join("todo-lint")
}

/// Collect messages until `done` accepts one, or fail after a timeout.
fn wait_for(host: &mut ExtHost, seen: &mut Vec<Incoming>, what: &str, mut done: impl FnMut(&Incoming) -> bool) -> Incoming {
    let deadline = Instant::now() + Duration::from_secs(20);
    let mut checked = 0;
    loop {
        seen.extend(host.poll());
        while checked < seen.len() {
            checked += 1;
            if done(&seen[checked - 1]) {
                return seen[checked - 1].clone();
            }
        }
        assert!(Instant::now() < deadline, "timed out waiting for {what}; saw {seen:#?}");
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn answer(host: &mut ExtHost, seen: &mut Vec<Incoming>, what: &str, rx: &Receiver<Result<Value, String>>) -> Result<Value, String> {
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        seen.extend(host.poll());
        if let Ok(result) = rx.try_recv() {
            return result;
        }
        assert!(Instant::now() < deadline, "no answer to {what}; saw {seen:#?}");
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn is_note(m: &Incoming, name: &str) -> bool {
    matches!(m, Incoming::Notification { method, .. } if method == name)
}

fn count_of_diagnostics(m: &Incoming) -> Option<usize> {
    match m {
        Incoming::Notification { method, params } if method == "diagnostics.set" => params["entries"][0]["diagnostics"].as_array().map(Vec::len),
        _ => None,
    }
}

#[test]
fn a_real_extension_runs_lints_and_answers_commands() {
    let Some(node) = find_node() else {
        eprintln!("skipped: node is not installed");
        return;
    };
    let work = std::env::temp_dir().join(format!("forge-host-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&work);
    std::fs::create_dir_all(&work).unwrap();
    let script = install_host_files(&work.join("host")).unwrap();
    let extension = load(&fixture()).unwrap();
    assert!(extension.activates_on(&ActivationEvent::Language("plaintext".into())));
    assert!(!extension.activates_on(&ActivationEvent::Language("rust".into())));

    let mut host = ExtHost::start(&node, &script, &work).unwrap();
    let mut seen = Vec::new();
    wait_for(&mut host, &mut seen, "the ready message", |m| is_note(m, "ready"));

    let file = work.join("notes.txt");
    let file_text = file.to_string_lossy().into_owned();
    let rx = host.request(
        "init",
        json!({ "extensions": [extension.host_description()], "settings": {}, "folders": [work.to_string_lossy()], "languages": ["plaintext"], "storageDir": work.join("storage").to_string_lossy() }),
    );
    answer(&mut host, &mut seen, "init", &rx).expect("init");

    // A document opened before the extension starts is linted when it activates.
    host.notify("doc.open", json!({ "path": file_text, "languageId": "plaintext", "version": 1, "text": "ok\nTODO one\nthen TODO TODO\n" }));
    host.notify("editor.active", json!({ "path": file_text, "selections": [[0, 0, 0, 0]] }));
    let rx = host.request("activate", json!({ "id": extension.id() }));
    assert_eq!(answer(&mut host, &mut seen, "activate", &rx).expect("activates"), json!(true));

    let diagnostics = wait_for(&mut host, &mut seen, "diagnostics", |m| count_of_diagnostics(m) == Some(3));
    let Incoming::Notification { params, .. } = diagnostics else { unreachable!() };
    assert_eq!(params["collection"], "todo-lint");
    let first = &params["entries"][0]["diagnostics"][0];
    assert_eq!(first["range"], json!([1, 0, 1, 4]), "line 1, columns 0-4");
    assert_eq!(first["severity"], 1);
    assert_eq!(first["source"], "todo-lint");
    assert_eq!(first["message"], "Unfinished work: TODO");
    assert!(params["entries"][0]["path"].as_str().unwrap().to_lowercase().ends_with("notes.txt"));

    // The status bar item reflects the active document.
    let status = wait_for(&mut host, &mut seen, "a status update", |m| {
        matches!(m, Incoming::Notification { method, params } if method == "status.update" && params["text"].as_str().is_some_and(|t| t.contains("3 TODOs")))
    });
    let Incoming::Notification { params, .. } = status else { unreachable!() };
    assert_eq!(params["alignment"], 2);
    assert_eq!(params["visible"], true);
    assert_eq!(params["command"], "todoLint.count");

    // Commands run and return values; messages arrive as notifications.
    let rx = host.request("executeCommand", json!({ "command": "todoLint.hello", "args": [] }));
    assert_eq!(answer(&mut host, &mut seen, "hello", &rx).unwrap(), json!("hello"));
    wait_for(&mut host, &mut seen, "the message", |m| {
        matches!(m, Incoming::Notification { method, params } if method == "window.message" && params["text"] == "Hello from Todo Lint" && params["kind"] == "info")
    });
    let rx = host.request("executeCommand", json!({ "command": "no.such.command", "args": [] }));
    assert!(answer(&mut host, &mut seen, "a missing command", &rx).unwrap_err().contains("not found"));

    // Edits to the document reach the extension, which lints again.
    host.notify("doc.change", json!({ "path": file_text, "version": 2, "text": "ok\nTODO one\nthen done\nTODO\n" }));
    wait_for(&mut host, &mut seen, "the second lint", |m| count_of_diagnostics(m) == Some(2));

    // Settings reach getConfiguration, and changing them re-lints.
    host.notify("config.changed", json!({ "settings": { "todoLint.word": "done" } }));
    wait_for(&mut host, &mut seen, "a lint with the new word", |m| {
        matches!(m, Incoming::Notification { method, params } if method == "diagnostics.set" && params["entries"][0]["diagnostics"][0]["message"] == "Unfinished work: done")
    });

    // An extension asking the editor to edit a document is a request Forge must answer.
    host.notify("config.changed", json!({ "settings": {} }));
    let rx = host.request("executeCommand", json!({ "command": "todoLint.fix", "args": [] }));
    let request = wait_for(&mut host, &mut seen, "an edit request", |m| matches!(m, Incoming::Request { method, .. } if method == "workspace.applyEdit"));
    let Incoming::Request { id, params, .. } = request else { unreachable!() };
    assert_eq!(params["changes"][0]["edits"][0], json!({ "range": [1, 0, 1, 4], "newText": "DONE" }));
    host.respond(id, Ok(json!(true)));
    assert_eq!(answer(&mut host, &mut seen, "fix", &rx).unwrap(), json!(true), "the extension saw the edit succeed");

    // Closing a document clears its diagnostics.
    host.notify("doc.close", json!({ "path": file_text }));
    wait_for(&mut host, &mut seen, "cleared diagnostics", |m| count_of_diagnostics(m) == Some(0));

    assert!(!seen.iter().any(|m| is_note(m, "extension.error")), "{seen:#?}");

    host.shutdown();
    assert!(!host.is_alive());
    let _ = std::fs::remove_dir_all(&work);
}

#[test]
fn a_crashing_extension_reports_an_error_and_the_host_survives() {
    let Some(node) = find_node() else {
        eprintln!("skipped: node is not installed");
        return;
    };
    let work = std::env::temp_dir().join(format!("forge-host-crash-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&work);
    let ext_dir = work.join("bad");
    std::fs::create_dir_all(&ext_dir).unwrap();
    std::fs::write(ext_dir.join("package.json"), r#"{ "name": "bad", "publisher": "t", "main": "./x.js", "activationEvents": ["*"] }"#).unwrap();
    std::fs::write(ext_dir.join("x.js"), "exports.activate = () => { throw new Error('boom'); };").unwrap();
    let extension = load(&ext_dir).unwrap();

    let script = install_host_files(&work.join("host")).unwrap();
    let mut host = ExtHost::start(&node, &script, &work).unwrap();
    let mut seen = Vec::new();
    let rx = host.request("init", json!({ "extensions": [extension.host_description()], "settings": {}, "folders": [] }));
    answer(&mut host, &mut seen, "init", &rx).unwrap();
    let rx = host.request("activate", json!({ "id": "t.bad" }));
    assert!(answer(&mut host, &mut seen, "activate", &rx).unwrap_err().contains("boom"));
    wait_for(&mut host, &mut seen, "the error report", |m| matches!(m, Incoming::Notification { method, params } if method == "extension.error" && params["message"] == "boom"));
    // The host is still answering.
    let rx = host.request("hasCommand", json!({ "command": "x" }));
    assert_eq!(answer(&mut host, &mut seen, "hasCommand", &rx).unwrap(), json!(false));
    host.shutdown();
    let _ = std::fs::remove_dir_all(&work);
}
