//! Drives a real debugpy adapter: launch a script, stop at a breakpoint, read the stack and the
//! variables, continue to the end. Skipped when no Python with debugpy (installed or bundled with
//! VS Code's Python debugger) is available.

use std::path::Path;
use std::sync::mpsc::Receiver;
use std::time::{Duration, Instant};

use forge_lsp::dap::{find_python, python_adapter, DapClient, DapMessage};
use serde_json::{json, Value};

const TIMEOUT: Duration = Duration::from_secs(40);

/// Wait for a response while collecting every event that arrives meanwhile.
fn wait_response(client: &DapClient, rx: &Receiver<Result<Value, String>>, events: &mut Vec<(String, Value)>) -> Result<Value, String> {
    let start = Instant::now();
    loop {
        for message in client.poll() {
            if let DapMessage::Event { event, body } = message {
                events.push((event, body));
            }
        }
        if let Ok(result) = rx.try_recv() {
            return result;
        }
        assert!(start.elapsed() < TIMEOUT, "timed out waiting for a response; events so far: {events:?}");
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn wait_event(client: &DapClient, events: &mut Vec<(String, Value)>, name: &str) -> Value {
    let start = Instant::now();
    loop {
        if let Some(at) = events.iter().position(|(e, _)| e == name) {
            return events.remove(at).1;
        }
        for message in client.poll() {
            if let DapMessage::Event { event, body } = message {
                events.push((event, body));
            }
        }
        assert!(start.elapsed() < TIMEOUT, "timed out waiting for the {name} event; events so far: {events:?}");
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn real_debugpy_stops_at_a_breakpoint_and_reports_stack_and_variables() {
    let dir = std::env::temp_dir().join(format!("forge-debugpy-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let script = dir.join("sample.py");
    std::fs::write(&script, "def add(a, b):\n    total = a + b\n    return total\n\nresult = add(2, 3)\nprint('done', result)\n").unwrap();

    let Some(python) = find_python(&dir, &dir) else {
        eprintln!("skipped: no Python on PATH");
        return;
    };
    let adapter = match python_adapter(&python) {
        Ok(a) => a,
        Err(reason) => {
            eprintln!("skipped: {reason}");
            return;
        }
    };
    let mut client = DapClient::spawn(&adapter.command, &adapter.args, Some(&dir), &adapter.env).unwrap();
    let mut events = Vec::new();

    let rx = client
        .request("initialize", json!({ "clientID": "forge", "adapterID": "python", "linesStartAt1": true, "columnsStartAt1": true, "pathFormat": "path", "supportsRunInTerminalRequest": false }))
        .unwrap();
    wait_response(&client, &rx, &mut events).unwrap();

    // `launch` only answers once configuration is done, so do not wait for it yet.
    let launch = client
        .request("launch", json!({ "type": "python", "request": "launch", "program": script, "cwd": dir, "python": adapter.interpreter, "console": "internalConsole", "justMyCode": true }))
        .unwrap();
    wait_event(&client, &mut events, "initialized");

    let rx = client.request("setBreakpoints", json!({ "source": { "path": script }, "breakpoints": [{ "line": 2 }] })).unwrap();
    let body = wait_response(&client, &rx, &mut events).unwrap();
    assert_eq!(body["breakpoints"][0]["verified"], true, "{body}");
    let rx = client.request("configurationDone", json!({})).unwrap();
    wait_response(&client, &rx, &mut events).unwrap();
    wait_response(&client, &launch, &mut events).unwrap();

    let stopped = wait_event(&client, &mut events, "stopped");
    assert_eq!(stopped["reason"], "breakpoint", "{stopped}");
    let thread_id = stopped["threadId"].as_i64().unwrap();

    let rx = client.request("stackTrace", json!({ "threadId": thread_id })).unwrap();
    let stack = wait_response(&client, &rx, &mut events).unwrap();
    let top = &stack["stackFrames"][0];
    assert_eq!((top["name"].as_str(), top["line"].as_i64()), (Some("add"), Some(2)), "{stack}");
    assert!(Path::new(top["source"]["path"].as_str().unwrap()).ends_with("sample.py"));

    let rx = client.request("scopes", json!({ "frameId": top["id"] })).unwrap();
    let scopes = wait_response(&client, &rx, &mut events).unwrap();
    let locals = scopes["scopes"].as_array().unwrap().iter().find(|s| s["name"] == "Locals").expect("a Locals scope");
    let rx = client.request("variables", json!({ "variablesReference": locals["variablesReference"] })).unwrap();
    let variables = wait_response(&client, &rx, &mut events).unwrap();
    let value_of = |name: &str| variables["variables"].as_array().unwrap().iter().find(|v| v["name"] == name).and_then(|v| v["value"].as_str().map(str::to_owned));
    assert_eq!((value_of("a").as_deref(), value_of("b").as_deref()), (Some("2"), Some("3")), "{variables}");

    // Step over the assignment: `total` is now 5 and the program is on the next line.
    let rx = client.request("next", json!({ "threadId": thread_id })).unwrap();
    wait_response(&client, &rx, &mut events).unwrap();
    let stopped = wait_event(&client, &mut events, "stopped");
    assert_eq!(stopped["reason"], "step", "{stopped}");
    let rx = client.request("stackTrace", json!({ "threadId": thread_id })).unwrap();
    let stack = wait_response(&client, &rx, &mut events).unwrap();
    assert_eq!(stack["stackFrames"][0]["line"], 3);

    let rx = client.request("continue", json!({ "threadId": thread_id })).unwrap();
    wait_response(&client, &rx, &mut events).unwrap();
    wait_event(&client, &mut events, "terminated");
    let rx = client.request("disconnect", json!({})).unwrap();
    let _ = wait_response(&client, &rx, &mut events);
    let _ = std::fs::remove_dir_all(&dir);
}
