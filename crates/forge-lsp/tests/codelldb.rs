//! Drives the real CodeLLDB adapter (downloaded into Forge's server folder): build a small Rust
//! program, launch it, stop at a breakpoint, read the stack and the variables, continue.
//! Skipped when CodeLLDB or cargo is not available.

use std::path::PathBuf;
use std::process::Command;
use std::sync::mpsc::Receiver;
use std::time::{Duration, Instant};

use forge_lsp::dap::{codelldb_paths, start_codelldb, DapClient, DapMessage};
use forge_lsp::find_executable;
use serde_json::{json, Value};

const TIMEOUT: Duration = Duration::from_secs(60);

fn servers_dir() -> Option<PathBuf> {
    let base = if cfg!(windows) { PathBuf::from(std::env::var_os("APPDATA")?).join("Forge") } else { PathBuf::from(std::env::var_os("HOME")?).join(".config").join("forge") };
    Some(base.join("servers"))
}

fn drain(client: &DapClient, events: &mut Vec<(String, Value)>) {
    for message in client.poll() {
        if let DapMessage::Event { event, body } = message {
            events.push((event, body));
        }
    }
}

fn wait_response(client: &DapClient, events: &mut Vec<(String, Value)>, rx: &Receiver<Result<Value, String>>) -> Result<Value, String> {
    let start = Instant::now();
    loop {
        drain(client, events);
        if let Ok(result) = rx.try_recv() {
            return result;
        }
        assert!(start.elapsed() < TIMEOUT, "timed out waiting for a response; heard {events:?}");
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn wait_event(client: &DapClient, events: &mut Vec<(String, Value)>, name: &str) -> Value {
    let start = Instant::now();
    loop {
        if let Some(at) = events.iter().position(|(e, _)| e == name) {
            return events.remove(at).1;
        }
        drain(client, events);
        assert!(start.elapsed() < TIMEOUT, "timed out waiting for {name}; heard {events:?}");
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn real_codelldb_stops_at_a_breakpoint_in_a_rust_program() {
    let (Some((exe, liblldb)), Some(_cargo)) = (servers_dir().and_then(|d| codelldb_paths(&d)), find_executable("cargo")) else {
        eprintln!("skipped: CodeLLDB or cargo is not available");
        return;
    };
    let dir = std::env::temp_dir().join(format!("forge-codelldb-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::write(dir.join("Cargo.toml"), "[package]\nname = \"sample\"\nversion = \"0.1.0\"\nedition = \"2021\"\n").unwrap();
    let source = dir.join("src").join("main.rs");
    std::fs::write(&source, "fn add(a: i32, b: i32) -> i32 {\n    let total = a + b;\n    total\n}\n\nfn main() {\n    let result = add(2, 3);\n    println!(\"done {}\", result);\n}\n").unwrap();
    let built = Command::new("cargo").arg("build").current_dir(&dir).output().unwrap();
    assert!(built.status.success(), "{}", String::from_utf8_lossy(&built.stderr));
    let program = dir.join("target").join("debug").join(if cfg!(windows) { "sample.exe" } else { "sample" });

    let server = start_codelldb(&exe, &liblldb).unwrap();
    let client = DapClient::connect(server.port).unwrap();
    let mut client = client;
    let mut events = Vec::new();
    let rx = client.request("initialize", json!({ "clientID": "forge", "adapterID": "lldb", "linesStartAt1": true, "columnsStartAt1": true, "pathFormat": "path", "supportsVariableType": true, "supportsRunInTerminalRequest": false })).unwrap();
    wait_response(&client, &mut events, &rx).unwrap();
    let launch = client.request("launch", json!({ "type": "lldb", "request": "launch", "name": "Forge", "program": program, "cwd": dir, "args": [], "stopOnEntry": false, "sourceLanguages": ["rust"], "terminal": "console" })).unwrap();
    wait_event(&client, &mut events, "initialized");
    let rx = client.request("setBreakpoints", json!({ "source": { "path": source }, "breakpoints": [{ "line": 2 }] })).unwrap();
    let body = wait_response(&client, &mut events, &rx).unwrap();
    assert_eq!(body["breakpoints"][0]["verified"], true, "{body}");
    let rx = client.request("configurationDone", json!({})).unwrap();
    wait_response(&client, &mut events, &rx).unwrap();
    wait_response(&client, &mut events, &launch).unwrap();

    let stopped = wait_event(&client, &mut events, "stopped");
    assert_eq!(stopped["reason"], "breakpoint", "{stopped}");
    let thread = stopped["threadId"].as_i64().unwrap();
    let rx = client.request("stackTrace", json!({ "threadId": thread })).unwrap();
    let stack = wait_response(&client, &mut events, &rx).unwrap();
    let top = &stack["stackFrames"][0];
    assert!(top["name"].as_str().is_some_and(|n| n.contains("add")) && top["line"] == 2, "{stack}");

    let rx = client.request("scopes", json!({ "frameId": top["id"] })).unwrap();
    let scopes = wait_response(&client, &mut events, &rx).unwrap();
    let locals = &scopes["scopes"][0];
    let rx = client.request("variables", json!({ "variablesReference": locals["variablesReference"] })).unwrap();
    let variables = wait_response(&client, &mut events, &rx).unwrap();
    let value_of = |name: &str| variables["variables"].as_array().unwrap().iter().find(|v| v["name"] == name).and_then(|v| v["value"].as_str().map(str::to_owned));
    assert_eq!((value_of("a").as_deref(), value_of("b").as_deref()), (Some("2"), Some("3")), "{variables}");

    let rx = client.request("continue", json!({ "threadId": thread })).unwrap();
    wait_response(&client, &mut events, &rx).unwrap();
    wait_event(&client, &mut events, "terminated");
    let _ = std::fs::remove_dir_all(&dir);
}
