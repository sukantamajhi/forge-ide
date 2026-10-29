//! End-to-end check against the real typescript-language-server on a `.tsx` file.
//! Skipped unless Forge's "Language Servers: Manage and Install..." has installed it.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use forge_lsp::{did_open_params, parse_diagnostics, path_to_uri, LspClient, ServerMessage};
use serde_json::json;

const SOURCE: &str = "const bad: number = \"not a number\";\nconst view = <div className=\"box\">{bad}</div>;\nexport { view };\n";

fn managed_dir() -> Option<PathBuf> {
    let base = if cfg!(windows) { PathBuf::from(std::env::var_os("APPDATA")?).join("Forge") } else { PathBuf::from(std::env::var_os("HOME")?).join(".config").join("forge") };
    Some(base.join("servers"))
}

#[test]
fn real_typescript_server_checks_tsx_and_understands_jsx() {
    let Some(managed) = managed_dir() else { return };
    let bin = managed.join("node_modules").join(".bin").join(if cfg!(windows) { "typescript-language-server.cmd" } else { "typescript-language-server" });
    let tsserver = managed.join("node_modules").join("typescript").join("lib").join("tsserver.js");
    if !bin.is_file() || !tsserver.is_file() {
        eprintln!("skipped: typescript-language-server is not installed in {}", managed.display());
        return;
    }

    let dir = std::env::temp_dir().join(format!("forge-lsp-ts-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("app.tsx");
    std::fs::write(&file, SOURCE).unwrap();
    let uri = path_to_uri(&file).unwrap();

    let mut client = LspClient::spawn_in(bin.to_str().unwrap(), &["--stdio"], Some(&dir)).unwrap();
    let options = json!({ "tsserver": { "path": tsserver.to_string_lossy() } });
    let init = client.begin_initialize_with(path_to_uri(&dir).as_deref(), Some(options)).unwrap();
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        match init.recv_timeout(Duration::from_millis(100)) {
            Ok(result) => {
                result.expect("initialize succeeded");
                break;
            }
            Err(_) if Instant::now() < deadline => {
                for message in client.poll() {
                    if let ServerMessage::Request { id, method, params } = message {
                        client.respond(id, forge_lsp::answer_server_request(&method, &params)).unwrap();
                    }
                }
            }
            Err(_) => panic!("initialize timed out"),
        }
    }
    client.initialized().unwrap();
    client.notify("textDocument/didOpen", did_open_params(&uri, "typescriptreact", 1, SOURCE)).unwrap();

    let mut diagnostics = None;
    let deadline = Instant::now() + Duration::from_secs(60);
    while diagnostics.is_none() && Instant::now() < deadline {
        for message in client.poll() {
            match message {
                ServerMessage::Request { id, method, params } => client.respond(id, forge_lsp::answer_server_request(&method, &params)).unwrap(),
                ServerMessage::Notification { method, params } if method == "textDocument/publishDiagnostics" => {
                    if let Some((path, found)) = parse_diagnostics(&params) {
                        if path == file && !found.is_empty() {
                            diagnostics = Some(found);
                        }
                    }
                }
                ServerMessage::Notification { .. } => {}
            }
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    let _ = std::fs::remove_dir_all(&dir);
    let diagnostics = diagnostics.expect("diagnostics for the .tsx file");
    let mismatch = diagnostics.iter().find(|d| d.message.contains("not assignable to type 'number'")).unwrap_or_else(|| panic!("{diagnostics:#?}"));
    assert_eq!(mismatch.range.start.line, 0, "the error is on the `const bad` line");
    // The JSX on line 2 parsed as JSX, so no syntax error is reported against it.
    assert!(
        !diagnostics.iter().any(|d| d.range.start.line == 1 && (d.message.contains("expected") || d.message.contains("Unexpected"))),
        "{diagnostics:#?}"
    );
}
