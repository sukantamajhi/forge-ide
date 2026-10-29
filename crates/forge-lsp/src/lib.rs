//! Minimal Language Server Protocol client: framed JSON-RPC over a child's stdio.

mod framing;
mod protocol;

pub use framing::{encode_message, read_message};
pub use protocol::*;

use std::collections::HashMap;
use std::io::{BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread;

use anyhow::{anyhow, Context, Result};
use serde_json::{json, Value};

/// A message from the server that is not the response to one of our requests.
#[derive(Debug, Clone, PartialEq)]
pub enum ServerMessage {
    Notification { method: String, params: Value },
    /// Server-to-client request; the caller must answer it with [`LspClient::respond`].
    Request { id: Value, method: String, params: Value },
}

type Pending = Arc<Mutex<HashMap<i64, Sender<Result<Value, Value>>>>>;

pub struct LspClient {
    child: Child,
    stdin: ChildStdin,
    next_id: i64,
    pending: Pending,
    incoming: Receiver<ServerMessage>,
}

/// Locate `name` on `PATH`, trying the executable suffixes Windows uses for shims
/// (`.cmd` for npm-installed servers). Returns `None` when it is not installed.
pub fn find_executable(name: &str) -> Option<PathBuf> {
    // On Windows a bare name is often a shell script that cannot be executed; prefer runnable suffixes.
    let suffixes: &[&str] = if cfg!(windows) { &[".exe", ".cmd", ".bat", ""] } else { &[""] };
    std::env::split_paths(&std::env::var_os("PATH")?).find_map(|dir| {
        suffixes.iter().map(|s| dir.join(format!("{name}{s}"))).find(|candidate| candidate.is_file())
    })
}

impl LspClient {
    pub fn spawn(command: &str, args: &[&str]) -> Result<Self> {
        Self::spawn_in(command, args, None)
    }

    /// Start a server with `cwd` as its working directory.
    pub fn spawn_in(command: &str, args: &[&str], cwd: Option<&Path>) -> Result<Self> {
        let mut cmd = Command::new(command);
        if let Some(cwd) = cwd {
            cmd.current_dir(cwd);
        }
        let mut child = cmd
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .with_context(|| format!("failed to start language server `{command}`"))?;
        let stdin = child.stdin.take().ok_or_else(|| anyhow!("no stdin"))?;
        let stdout = child.stdout.take().ok_or_else(|| anyhow!("no stdout"))?;

        let pending: Pending = Arc::default();
        let (tx, incoming) = mpsc::channel();
        let reader_pending = Arc::clone(&pending);
        thread::spawn(move || {
            let mut reader = BufReader::new(stdout);
            while let Ok(Some(message)) = read_message(&mut reader) {
                dispatch(message, &reader_pending, &tx);
            }
            // Server exited: fail every request still waiting.
            for (_, waiter) in reader_pending.lock().unwrap().drain() {
                let _ = waiter.send(Err(json!({ "message": "language server exited" })));
            }
        });

        Ok(Self { child, stdin, next_id: 1, pending, incoming })
    }

    /// Send a request and return a receiver for its response.
    pub fn request(&mut self, method: &str, params: Value) -> Result<Receiver<Result<Value, Value>>> {
        let id = self.next_id;
        self.next_id += 1;
        let (tx, rx) = mpsc::channel();
        self.pending.lock().unwrap().insert(id, tx);
        self.send(&json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params }))?;
        Ok(rx)
    }

    pub fn notify(&mut self, method: &str, params: Value) -> Result<()> {
        self.send(&json!({ "jsonrpc": "2.0", "method": method, "params": params }))
    }

    pub fn respond(&mut self, id: Value, result: Value) -> Result<()> {
        self.send(&json!({ "jsonrpc": "2.0", "id": id, "result": result }))
    }

    /// Drain pending server notifications and requests without blocking.
    pub fn poll(&self) -> Vec<ServerMessage> {
        self.incoming.try_iter().collect()
    }

    /// Send `initialize` without waiting, so a UI thread never blocks on a slow server.
    /// Call [`LspClient::initialized`] once the returned receiver yields a response.
    pub fn begin_initialize(&mut self, root_uri: Option<&str>) -> Result<Receiver<Result<Value, Value>>> {
        self.begin_initialize_with(root_uri, None)
    }

    /// Like [`LspClient::begin_initialize`], with server-specific `initializationOptions`.
    pub fn begin_initialize_with(&mut self, root_uri: Option<&str>, options: Option<Value>) -> Result<Receiver<Result<Value, Value>>> {
        let mut params = initialize_params(root_uri);
        if let Some(options) = options {
            params["initializationOptions"] = options;
        }
        self.request("initialize", params)
    }

    /// The `initialized` notification that completes the handshake.
    pub fn initialized(&mut self) -> Result<()> {
        self.notify("initialized", json!({}))
    }

    /// Blocking `initialize` + `initialized`, advertising the capabilities Forge uses.
    pub fn initialize(&mut self, root_uri: Option<&str>) -> Result<Value> {
        let rx = self.begin_initialize(root_uri)?;
        let result = rx
            .recv()
            .map_err(|_| anyhow!("language server closed during initialize"))?
            .map_err(|e| anyhow!("initialize failed: {e}"))?;
        self.initialized()?;
        Ok(result)
    }

    /// False once the server process has exited.
    pub fn is_alive(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(None))
    }

    pub fn shutdown(mut self) -> Result<()> {
        if let Ok(rx) = self.request("shutdown", Value::Null) {
            let _ = rx.recv();
        }
        let _ = self.notify("exit", Value::Null);
        let _ = self.child.wait();
        Ok(())
    }

    fn send(&mut self, message: &Value) -> Result<()> {
        self.stdin.write_all(&encode_message(message))?;
        self.stdin.flush()?;
        Ok(())
    }
}

pub fn initialize_params(root_uri: Option<&str>) -> Value {
    json!({
        "processId": std::process::id(),
        "clientInfo": { "name": "forge", "version": env!("CARGO_PKG_VERSION") },
        "rootUri": root_uri,
        "workspaceFolders": root_uri.map(|uri| vec![json!({ "uri": uri, "name": "workspace" })]),
        "capabilities": {
            "textDocument": {
                "synchronization": { "didSave": true },
                "publishDiagnostics": {},
                "completion": { "completionItem": { "snippetSupport": false } },
                "definition": {},
                "references": {},
                "hover": { "contentFormat": ["plaintext", "markdown"] },
                "signatureHelp": { "signatureInformation": { "parameterInformation": { "labelOffsetSupport": true } } },
                "rename": { "prepareSupport": false },
                "formatting": {},
                "documentSymbol": { "hierarchicalDocumentSymbolSupport": true },
                "codeAction": { "codeActionLiteralSupport": { "codeActionKind": { "valueSet": ["quickfix", "refactor", "source"] } } }
            },
            "window": { "workDoneProgress": true },
            "workspace": { "workspaceFolders": true, "configuration": true }
        }
    })
}

fn dispatch(message: Value, pending: &Pending, tx: &Sender<ServerMessage>) {
    let method = message.get("method").and_then(Value::as_str).map(str::to_owned);
    let id = message.get("id").cloned();
    match (method, id) {
        (Some(method), Some(id)) => {
            let params = message.get("params").cloned().unwrap_or(Value::Null);
            let _ = tx.send(ServerMessage::Request { id, method, params });
        }
        (Some(method), None) => {
            let params = message.get("params").cloned().unwrap_or(Value::Null);
            let _ = tx.send(ServerMessage::Notification { method, params });
        }
        (None, Some(id)) => {
            let Some(id) = id.as_i64() else { return };
            if let Some(waiter) = pending.lock().unwrap().remove(&id) {
                let outcome = match message.get("error") {
                    Some(error) => Err(error.clone()),
                    None => Ok(message.get("result").cloned().unwrap_or(Value::Null)),
                };
                let _ = waiter.send(outcome);
            }
        }
        (None, None) => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dispatch_routes_response_to_waiter() {
        let pending: Pending = Arc::default();
        let (tx, rx) = mpsc::channel();
        let (wtx, wrx) = mpsc::channel();
        pending.lock().unwrap().insert(7, wtx);

        dispatch(json!({"jsonrpc":"2.0","id":7,"result":{"ok":true}}), &pending, &tx);
        assert_eq!(wrx.recv().unwrap().unwrap(), json!({"ok": true}));
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn dispatch_routes_error_response() {
        let pending: Pending = Arc::default();
        let (tx, _rx) = mpsc::channel();
        let (wtx, wrx) = mpsc::channel();
        pending.lock().unwrap().insert(1, wtx);
        dispatch(json!({"id":1,"error":{"code":-32601,"message":"nope"}}), &pending, &tx);
        assert_eq!(wrx.recv().unwrap().unwrap_err()["code"], -32601);
    }

    #[test]
    fn dispatch_separates_notifications_from_server_requests() {
        let pending: Pending = Arc::default();
        let (tx, rx) = mpsc::channel();
        dispatch(json!({"method":"textDocument/publishDiagnostics","params":{"uri":"file:///a"}}), &pending, &tx);
        dispatch(json!({"id":"abc","method":"workspace/configuration","params":{}}), &pending, &tx);

        assert!(matches!(rx.recv().unwrap(), ServerMessage::Notification { method, .. } if method == "textDocument/publishDiagnostics"));
        assert!(matches!(rx.recv().unwrap(), ServerMessage::Request { id, .. } if id == json!("abc")));
    }

    #[test]
    fn initialization_options_are_only_sent_when_given() {
        let mut params = initialize_params(Some("file:///p"));
        assert!(params.get("initializationOptions").is_none());
        params["initializationOptions"] = json!({ "tsserver": { "path": "x" } });
        assert_eq!(params["initializationOptions"]["tsserver"]["path"], "x");
    }

    #[test]
    fn initialize_params_carry_root_and_client_info() {
        let params = initialize_params(Some("file:///proj"));
        assert_eq!(params["rootUri"], "file:///proj");
        assert_eq!(params["clientInfo"]["name"], "forge");
    }
}
