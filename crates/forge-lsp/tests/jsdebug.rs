//! Drives the real js-debug adapter (downloaded into Forge's server folder): launch a Node script,
//! follow the child session js-debug asks for, stop at a breakpoint, read variables, continue.
//! Skipped when Node or the adapter is not installed.

use std::path::PathBuf;
use std::sync::mpsc::Receiver;
use std::time::{Duration, Instant};

use forge_lsp::dap::{js_debug_script, start_js_debug, DapClient, DapMessage};
use forge_lsp::find_executable;
use serde_json::{json, Value};

const TIMEOUT: Duration = Duration::from_secs(60);

fn servers_dir() -> Option<PathBuf> {
    let base = if cfg!(windows) { PathBuf::from(std::env::var_os("APPDATA")?).join("Forge") } else { PathBuf::from(std::env::var_os("HOME")?).join(".config").join("forge") };
    Some(base.join("servers"))
}

/// Everything heard so far from one client.
#[derive(Default)]
struct Heard {
    events: Vec<(String, Value)>,
    requests: Vec<(i64, String, Value)>,
}

fn drain(client: &DapClient, heard: &mut Heard) {
    for message in client.poll() {
        match message {
            DapMessage::Event { event, body } => heard.events.push((event, body)),
            DapMessage::Request { seq, command, arguments } => heard.requests.push((seq, command, arguments)),
            DapMessage::Closed => {}
        }
    }
}

fn wait_response(client: &DapClient, heard: &mut Heard, rx: &Receiver<Result<Value, String>>) -> Result<Value, String> {
    let start = Instant::now();
    loop {
        drain(client, heard);
        if let Ok(result) = rx.try_recv() {
            return result;
        }
        assert!(start.elapsed() < TIMEOUT, "timed out waiting for a response; heard {:?}", heard.events);
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn wait_event(client: &DapClient, heard: &mut Heard, name: &str) -> Value {
    let start = Instant::now();
    loop {
        if let Some(at) = heard.events.iter().position(|(e, _)| e == name) {
            return heard.events.remove(at).1;
        }
        drain(client, heard);
        assert!(start.elapsed() < TIMEOUT, "timed out waiting for {name}; heard {:?}", heard.events);
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn real_js_debug_stops_at_a_breakpoint_in_a_node_script() {
    let (Some(node), Some(script)) = (find_executable("node"), servers_dir().and_then(|d| js_debug_script(&d))) else {
        eprintln!("skipped: Node or js-debug is not installed");
        return;
    };
    let dir = std::env::temp_dir().join(format!("forge-jsdebug-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let program = dir.join("sample.js");
    std::fs::write(&program, "function add(a, b) {\n  const total = a + b;\n  return total;\n}\nconst result = add(2, 3);\nconsole.log('done', result);\n").unwrap();

    let server = start_js_debug(&node, &script).unwrap();
    let init = json!({ "clientID": "forge", "adapterID": "pwa-node", "linesStartAt1": true, "columnsStartAt1": true, "pathFormat": "path", "supportsVariableType": true, "supportsStartDebuggingRequest": true, "supportsRunInTerminalRequest": false });

    // The parent session.
    let mut parent = DapClient::connect(server.port).unwrap();
    let mut parent_heard = Heard::default();
    let rx = parent.request("initialize", init.clone()).unwrap();
    wait_response(&parent, &mut parent_heard, &rx).unwrap();
    let _launch = parent
        .request("launch", json!({ "type": "pwa-node", "request": "launch", "name": "Forge", "program": program, "cwd": dir, "console": "internalConsole", "runtimeExecutable": node, "skipFiles": ["<node_internals>/**"] }))
        .unwrap();
    wait_event(&parent, &mut parent_heard, "initialized");
    let rx = parent.request("configurationDone", json!({})).unwrap();
    wait_response(&parent, &mut parent_heard, &rx).unwrap();

    // js-debug asks the client to open the session for the program it started.
    let start = Instant::now();
    let (seq, arguments) = loop {
        drain(&parent, &mut parent_heard);
        if let Some(at) = parent_heard.requests.iter().position(|(_, c, _)| c == "startDebugging") {
            let (seq, _, arguments) = parent_heard.requests.remove(at);
            break (seq, arguments);
        }
        assert!(start.elapsed() < TIMEOUT, "no startDebugging request; heard {:?}", parent_heard.events);
        std::thread::sleep(Duration::from_millis(20));
    };
    parent.respond(seq, "startDebugging", Ok(json!({}))).unwrap();

    let mut child = DapClient::connect(server.port).unwrap();
    let mut child_heard = Heard::default();
    let rx = child.request("initialize", init).unwrap();
    wait_response(&child, &mut child_heard, &rx).unwrap();
    let command = arguments["request"].as_str().unwrap_or("attach").to_owned();
    let _started = child.request(&command, arguments["configuration"].clone()).unwrap();
    wait_event(&child, &mut child_heard, "initialized");
    let rx = child.request("setBreakpoints", json!({ "source": { "path": program }, "breakpoints": [{ "line": 2 }] })).unwrap();
    wait_response(&child, &mut child_heard, &rx).unwrap();
    let rx = child.request("setExceptionBreakpoints", json!({ "filters": [] })).unwrap();
    wait_response(&child, &mut child_heard, &rx).unwrap();
    let rx = child.request("configurationDone", json!({})).unwrap();
    wait_response(&child, &mut child_heard, &rx).unwrap();

    let stopped = wait_event(&child, &mut child_heard, "stopped");
    assert_eq!(stopped["reason"], "breakpoint", "{stopped}");
    let thread = stopped["threadId"].as_i64().unwrap();
    let rx = child.request("stackTrace", json!({ "threadId": thread })).unwrap();
    let stack = wait_response(&child, &mut child_heard, &rx).unwrap();
    let top = &stack["stackFrames"][0];
    assert!(top["name"].as_str().is_some_and(|n| n.ends_with("add")) && top["line"] == 2, "{stack}");

    let rx = child.request("scopes", json!({ "frameId": top["id"] })).unwrap();
    let scopes = wait_response(&child, &mut child_heard, &rx).unwrap();
    let local = scopes["scopes"].as_array().unwrap().iter().find(|s| s["name"].as_str().is_some_and(|n| n.starts_with("Local"))).expect("a Local scope");
    let rx = child.request("variables", json!({ "variablesReference": local["variablesReference"] })).unwrap();
    let variables = wait_response(&child, &mut child_heard, &rx).unwrap();
    let value_of = |name: &str| variables["variables"].as_array().unwrap().iter().find(|v| v["name"] == name).and_then(|v| v["value"].as_str().map(str::to_owned));
    assert_eq!((value_of("a").as_deref(), value_of("b").as_deref()), (Some("2"), Some("3")), "{variables}");

    let rx = child.request("continue", json!({ "threadId": thread })).unwrap();
    wait_response(&child, &mut child_heard, &rx).unwrap();
    wait_event(&child, &mut child_heard, "terminated");
    let _ = std::fs::remove_dir_all(&dir);
}
