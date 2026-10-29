//! Minimal Debug Adapter Protocol client: framed JSON over a debug adapter's stdio.
//!
//! The wire framing is the same `Content-Length` one the language server client uses; the messages
//! differ (`seq` / `type` instead of JSON-RPC). Responses come back on a channel per request, so the
//! UI thread never blocks; events and requests the adapter makes arrive through [`DapClient::poll`].

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{Shutdown, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{anyhow, Context, Result};
use serde_json::{json, Value};

use crate::{encode_message, read_message};

/// Something the adapter sent that is not the response to one of our requests.
#[derive(Debug, Clone, PartialEq)]
pub enum DapMessage {
    Event { event: String, body: Value },
    /// A request from the adapter (`runInTerminal`, `startDebugging`); answer with [`DapClient::respond`].
    Request { seq: i64, command: String, arguments: Value },
    /// The adapter process closed its output.
    Closed,
}

type Pending = Arc<Mutex<HashMap<i64, Sender<Result<Value, String>>>>>;

pub struct DapClient {
    /// The adapter process, when this client started it (over stdio); `None` for a socket connection.
    child: Option<Child>,
    writer: Box<dyn Write + Send>,
    /// Kept to close a socket connection.
    socket: Option<TcpStream>,
    alive: Arc<AtomicBool>,
    next_seq: i64,
    pending: Pending,
    incoming: Receiver<DapMessage>,
}

impl DapClient {
    /// Start an adapter. `env` is added to the adapter's environment.
    pub fn spawn(command: &Path, args: &[String], cwd: Option<&Path>, env: &[(String, String)]) -> Result<Self> {
        let mut cmd = Command::new(command);
        cmd.args(args).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null());
        if let Some(cwd) = cwd {
            cmd.current_dir(cwd);
        }
        for (key, value) in env {
            cmd.env(key, value);
        }
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            cmd.creation_flags(0x0800_0000);
        }
        let mut child = cmd.spawn().with_context(|| format!("failed to start debug adapter `{}`", command.display()))?;
        let stdin = child.stdin.take().ok_or_else(|| anyhow!("no stdin"))?;
        let stdout = child.stdout.take().ok_or_else(|| anyhow!("no stdout"))?;
        Ok(Self::from_streams(Some(child), Box::new(stdin), None, stdout))
    }

    /// Connect to an adapter that listens on a local port (js-debug does), retrying while it starts.
    pub fn connect(port: u16) -> Result<Self> {
        let started = Instant::now();
        let stream = loop {
            match TcpStream::connect(("127.0.0.1", port)) {
                Ok(stream) => break stream,
                Err(error) if started.elapsed() > Duration::from_secs(10) => return Err(anyhow!("could not connect to the debug adapter on port {port}: {error}")),
                Err(_) => thread::sleep(Duration::from_millis(100)),
            }
        };
        stream.set_nodelay(true).ok();
        let reader = stream.try_clone()?;
        let writer = stream.try_clone()?;
        Ok(Self::from_streams(None, Box::new(writer), Some(stream), reader))
    }

    fn from_streams(child: Option<Child>, writer: Box<dyn Write + Send>, socket: Option<TcpStream>, reader: impl Read + Send + 'static) -> Self {
        let pending: Pending = Arc::default();
        let alive = Arc::new(AtomicBool::new(true));
        let (tx, incoming) = mpsc::channel();
        let (reader_pending, reader_alive) = (Arc::clone(&pending), Arc::clone(&alive));
        thread::spawn(move || {
            let mut reader = BufReader::new(reader);
            while let Ok(Some(message)) = read_message(&mut reader) {
                dispatch(message, &reader_pending, &tx);
            }
            reader_alive.store(false, Ordering::Relaxed);
            for (_, waiter) in reader_pending.lock().unwrap().drain() {
                let _ = waiter.send(Err("the debug adapter exited".to_owned()));
            }
            let _ = tx.send(DapMessage::Closed);
        });
        Self { child, writer, socket, alive, next_seq: 1, pending, incoming }
    }

    /// Send a request; the receiver yields its response `body`, or the adapter's error message.
    pub fn request(&mut self, command: &str, arguments: Value) -> Result<Receiver<Result<Value, String>>> {
        let seq = self.next_seq;
        self.next_seq += 1;
        let (tx, rx) = mpsc::channel();
        self.pending.lock().unwrap().insert(seq, tx);
        self.send(&json!({ "seq": seq, "type": "request", "command": command, "arguments": arguments }))?;
        Ok(rx)
    }

    /// Answer a request the adapter made.
    pub fn respond(&mut self, request_seq: i64, command: &str, result: Result<Value, String>) -> Result<()> {
        let seq = self.next_seq;
        self.next_seq += 1;
        let message = match result {
            Ok(body) => json!({ "seq": seq, "type": "response", "request_seq": request_seq, "success": true, "command": command, "body": body }),
            Err(error) => json!({ "seq": seq, "type": "response", "request_seq": request_seq, "success": false, "command": command, "message": error }),
        };
        self.send(&message)
    }

    /// Everything the adapter sent since the last call, without blocking.
    pub fn poll(&self) -> Vec<DapMessage> {
        self.incoming.try_iter().collect()
    }

    pub fn is_alive(&mut self) -> bool {
        match self.child.as_mut() {
            Some(child) => matches!(child.try_wait(), Ok(None)),
            None => self.alive.load(Ordering::Relaxed),
        }
    }

    pub fn kill(&mut self) {
        if let Some(child) = self.child.as_mut() {
            let _ = child.kill();
            let _ = child.wait();
        }
        if let Some(socket) = self.socket.as_ref() {
            let _ = socket.shutdown(Shutdown::Both);
        }
    }

    fn send(&mut self, message: &Value) -> Result<()> {
        self.writer.write_all(&encode_message(message))?;
        self.writer.flush()?;
        Ok(())
    }
}

impl Drop for DapClient {
    fn drop(&mut self) {
        self.kill();
    }
}

fn dispatch(message: Value, pending: &Pending, tx: &Sender<DapMessage>) {
    match message.get("type").and_then(Value::as_str) {
        Some("response") => {
            let Some(seq) = message.get("request_seq").and_then(Value::as_i64) else { return };
            let Some(waiter) = pending.lock().unwrap().remove(&seq) else { return };
            let result = if message.get("success").and_then(Value::as_bool).unwrap_or(false) {
                Ok(message.get("body").cloned().unwrap_or(Value::Null))
            } else {
                Err(message.get("message").and_then(Value::as_str).unwrap_or("the debug adapter reported an error").to_owned())
            };
            let _ = waiter.send(result);
        }
        Some("event") => {
            let event = message.get("event").and_then(Value::as_str).unwrap_or_default().to_owned();
            let _ = tx.send(DapMessage::Event { event, body: message.get("body").cloned().unwrap_or(Value::Null) });
        }
        Some("request") => {
            let _ = tx.send(DapMessage::Request {
                seq: message.get("seq").and_then(Value::as_i64).unwrap_or(0),
                command: message.get("command").and_then(Value::as_str).unwrap_or_default().to_owned(),
                arguments: message.get("arguments").cloned().unwrap_or(Value::Null),
            });
        }
        _ => {}
    }
}

/// A running debug adapter that listens on a local port (js-debug, CodeLLDB). Closed when dropped.
pub struct AdapterServer {
    child: Child,
    pub port: u16,
}

impl Drop for AdapterServer {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Where Forge keeps the js-debug adapter: `<config>/servers/js-debug/js-debug/src/dapDebugServer.js`.
pub fn js_debug_script(servers_dir: &Path) -> Option<PathBuf> {
    let script = servers_dir.join("js-debug").join("js-debug").join("src").join("dapDebugServer.js");
    script.is_file().then_some(script)
}

/// Start `dapDebugServer.js` on a free port and wait until it says it is listening.
pub fn start_js_debug(node: &Path, script: &Path) -> Result<AdapterServer> {
    let port = TcpListener::bind("127.0.0.1:0")?.local_addr()?.port();
    let mut cmd = Command::new(node);
    cmd.arg(script).arg(port.to_string()).arg("127.0.0.1").stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000);
    }
    let mut child = cmd.spawn().with_context(|| format!("failed to start node `{}`", node.display()))?;
    let stdout = child.stdout.take().ok_or_else(|| anyhow!("no stdout"))?;
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            if line.contains("listening") {
                let _ = tx.send(());
            }
        }
    });
    let server = AdapterServer { child, port };
    rx.recv_timeout(Duration::from_secs(15)).map_err(|_| anyhow!("the JavaScript debugger did not start"))?;
    Ok(server)
}

/// Where Forge keeps CodeLLDB: its adapter program and the LLDB library it loads.
pub fn codelldb_paths(servers_dir: &Path) -> Option<(PathBuf, PathBuf)> {
    let base = servers_dir.join("codelldb").join("extension");
    let exe = base.join("adapter").join(if cfg!(windows) { "codelldb.exe" } else { "codelldb" });
    let lib = base.join("lldb").join(if cfg!(windows) { "bin" } else { "lib" }).join(if cfg!(windows) { "liblldb.dll" } else { "liblldb.so" });
    (exe.is_file() && lib.is_file()).then_some((exe, lib))
}

/// Start CodeLLDB listening on a free local port.
pub fn start_codelldb(exe: &Path, liblldb: &Path) -> Result<AdapterServer> {
    let port = TcpListener::bind("127.0.0.1:0")?.local_addr()?.port();
    let mut cmd = Command::new(exe);
    cmd.arg("--port").arg(port.to_string()).arg("--liblldb").arg(liblldb).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000);
    }
    let child = cmd.spawn().with_context(|| format!("failed to start CodeLLDB `{}`", exe.display()))?;
    Ok(AdapterServer { child, port })
}

/// How to start a debug adapter for a language.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AdapterCommand {
    pub command: PathBuf,
    pub args: Vec<String>,
    pub env: Vec<(String, String)>,
    /// The interpreter the program itself runs on, when the adapter needs to be told.
    pub interpreter: Option<PathBuf>,
}

/// A `.venv` or `venv` Python at or above `start` (stopping at `root`), else `python` on `PATH`.
pub fn find_python(start: &Path, root: &Path) -> Option<PathBuf> {
    let venv_python = |dir: &Path| {
        ["Scripts/python.exe", "bin/python"].iter().flat_map(|rel| [".venv", "venv"].map(|v| dir.join(v).join(rel))).find(|p| p.is_file())
    };
    for dir in start.ancestors() {
        if let Some(found) = venv_python(dir) {
            return Some(found);
        }
        if dir == root {
            break;
        }
    }
    crate::find_executable("python").or_else(|| crate::find_executable("python3"))
}

/// The debugpy adapter folder (`.../debugpy/adapter`) bundled with the VS Code Python debugger
/// extension, newest first.
fn bundled_debugpy() -> Option<PathBuf> {
    let home = std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME")).map(PathBuf::from)?;
    let mut candidates: Vec<PathBuf> = std::fs::read_dir(home.join(".vscode").join("extensions"))
        .ok()?
        .filter_map(Result::ok)
        .filter(|e| e.file_name().to_string_lossy().starts_with("ms-python.debugpy-"))
        .map(|e| e.path().join("bundled").join("libs").join("debugpy").join("adapter"))
        .filter(|p| p.is_dir())
        .collect();
    candidates.sort();
    candidates.pop()
}

/// How to run the debugpy adapter: the interpreter's own `debugpy` if installed there, else the
/// copy bundled with VS Code's Python debugger.
pub fn python_adapter(python: &Path) -> Result<AdapterCommand, String> {
    let mut probe = Command::new(python);
    probe.args(["-c", "import debugpy"]).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        probe.creation_flags(0x0800_0000);
    }
    let installed = probe.status().is_ok_and(|s| s.success());
    if installed {
        return Ok(AdapterCommand { command: python.to_path_buf(), args: vec!["-m".into(), "debugpy.adapter".into()], env: Vec::new(), interpreter: Some(python.to_path_buf()) });
    }
    match bundled_debugpy() {
        Some(adapter) => Ok(AdapterCommand { command: python.to_path_buf(), args: vec![adapter.to_string_lossy().into_owned()], env: Vec::new(), interpreter: Some(python.to_path_buf()) }),
        None => Err(format!("debugpy is not installed for {}. Run: {} -m pip install debugpy", python.display(), python.display())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dispatch_routes_responses_events_and_requests() {
        let pending: Pending = Arc::default();
        let (tx, rx) = mpsc::channel();
        let (resp_tx, resp_rx) = mpsc::channel();
        pending.lock().unwrap().insert(3, resp_tx);

        dispatch(json!({ "type": "response", "request_seq": 3, "success": true, "command": "threads", "body": { "threads": [] } }), &pending, &tx);
        assert_eq!(resp_rx.try_recv().unwrap().unwrap(), json!({ "threads": [] }));

        let (fail_tx, fail_rx) = mpsc::channel();
        pending.lock().unwrap().insert(4, fail_tx);
        dispatch(json!({ "type": "response", "request_seq": 4, "success": false, "message": "no such thread" }), &pending, &tx);
        assert_eq!(fail_rx.try_recv().unwrap().unwrap_err(), "no such thread");

        dispatch(json!({ "type": "event", "event": "stopped", "body": { "reason": "breakpoint", "threadId": 1 } }), &pending, &tx);
        dispatch(json!({ "type": "request", "seq": 9, "command": "runInTerminal", "arguments": { "args": ["x"] } }), &pending, &tx);
        assert_eq!(rx.try_recv().unwrap(), DapMessage::Event { event: "stopped".into(), body: json!({ "reason": "breakpoint", "threadId": 1 }) });
        assert_eq!(rx.try_recv().unwrap(), DapMessage::Request { seq: 9, command: "runInTerminal".into(), arguments: json!({ "args": ["x"] }) });
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn a_project_venv_wins_over_the_system_python() {
        let root = std::env::temp_dir().join(format!("forge-venv-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let script_dir = root.join("backend").join("app");
        std::fs::create_dir_all(&script_dir).unwrap();
        let venv = root.join("backend").join(".venv").join(if cfg!(windows) { "Scripts" } else { "bin" });
        std::fs::create_dir_all(&venv).unwrap();
        let python = venv.join(if cfg!(windows) { "python.exe" } else { "python" });
        std::fs::write(&python, "").unwrap();
        assert_eq!(find_python(&script_dir, &root), Some(python));
        let _ = std::fs::remove_dir_all(&root);
    }
}
