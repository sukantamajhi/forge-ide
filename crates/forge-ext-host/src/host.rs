//! The Node extension host: a child process that runs extension code against Forge's own
//! `vscode` module, and the messages Forge exchanges with it.
//!
//! The wire format is one JSON object per line on the child's stdin and stdout, in both
//! directions, in the shape of JSON-RPC: `{id?, method, params}` is a request (with an `id`)
//! or a notification, `{id, result}` or `{id, error: {message}}` answers a request. Extension
//! code never touches stdout directly: the host redirects `console` and `process.stdout`
//! into `log` notifications, so a stray `console.log` cannot corrupt the stream.
//!
//! Nothing here blocks the UI thread: sending goes through a writer thread, reading through a
//! reader thread, and [`ExtHost::poll`] drains what arrived.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{self, Receiver, Sender};

use anyhow::{bail, Context, Result};
use serde_json::{json, Value};

use crate::Extension;

/// The JavaScript that runs inside Node. Compiled into Forge and written out on start.
const HOST_JS: &str = include_str!("../host/host.js");
const VSCODE_JS: &str = include_str!("../host/vscode.js");

/// Something the host sent that the application must act on or answer.
#[derive(Debug, Clone, PartialEq)]
pub enum Incoming {
    Notification { method: String, params: Value },
    /// Answer with [`ExtHost::respond`].
    Request { id: Value, method: String, params: Value },
    /// The process ended (or its output closed).
    Exited(String),
}

type Pending = HashMap<u64, Sender<Result<Value, String>>>;

pub struct ExtHost {
    child: Child,
    writer: Sender<String>,
    incoming: Receiver<Incoming>,
    pending: Pending,
    next_id: u64,
    /// Responses to our requests, delivered by the reader thread.
    responses: Receiver<(u64, Result<Value, String>)>,
}

/// Write the host's JavaScript into `dir` (replacing stale copies) and return the entry script.
pub fn install_host_files(dir: &Path) -> Result<PathBuf> {
    std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    for (name, text) in [("host.js", HOST_JS), ("vscode.js", VSCODE_JS)] {
        let path = dir.join(name);
        if std::fs::read_to_string(&path).map_or(true, |existing| existing != text) {
            std::fs::write(&path, text).with_context(|| format!("writing {}", path.display()))?;
        }
    }
    Ok(dir.join("host.js"))
}

/// Find `node` on `PATH`, preferring a real executable over a script of the same name.
pub fn find_node() -> Option<PathBuf> {
    let suffixes: &[&str] = if cfg!(windows) { &[".exe", ".cmd", ".bat", ""] } else { &[""] };
    std::env::split_paths(&std::env::var_os("PATH")?).find_map(|dir| suffixes.iter().map(|s| dir.join(format!("node{s}"))).find(|c| c.is_file()))
}

impl ExtHost {
    /// Start `node host.js`. Fails if Node cannot be run.
    pub fn start(node: &Path, script: &Path, cwd: &Path) -> Result<Self> {
        let mut command = Command::new(node);
        command.arg(script).current_dir(cwd).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(0x0800_0000);
        }
        let mut child = command.spawn().with_context(|| format!("starting {}", node.display()))?;
        let mut stdin = child.stdin.take().context("no stdin")?;
        let stdout = child.stdout.take().context("no stdout")?;
        let stderr = child.stderr.take().context("no stderr")?;

        let (writer, to_write) = mpsc::channel::<String>();
        std::thread::spawn(move || {
            for line in to_write {
                if stdin.write_all(line.as_bytes()).and_then(|()| stdin.write_all(b"\n")).and_then(|()| stdin.flush()).is_err() {
                    break;
                }
            }
        });

        let (tx, incoming) = mpsc::channel();
        let (response_tx, responses) = mpsc::channel();
        let exit_tx = tx.clone();
        let stderr_tx = tx.clone();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else { break };
                let Ok(message) = serde_json::from_str::<Value>(&line) else {
                    let _ = tx.send(Incoming::Notification { method: "log".to_owned(), params: json!({ "level": "warn", "text": format!("unparseable host output: {line}") }) });
                    continue;
                };
                route(message, &tx, &response_tx);
            }
            let _ = exit_tx.send(Incoming::Exited("the extension host's output closed".to_owned()));
        });
        // Anything Node itself prints (a syntax error in the host, a crash) arrives on stderr.
        std::thread::spawn(move || {
            for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                let _ = stderr_tx.send(Incoming::Notification { method: "log".to_owned(), params: json!({ "level": "stderr", "text": line }) });
            }
        });
        Ok(Self { child, writer, incoming, pending: HashMap::new(), next_id: 1, responses })
    }

    pub fn notify(&self, method: &str, params: Value) {
        let _ = self.writer.send(json!({ "method": method, "params": params }).to_string());
    }

    /// Send a request. The receiver yields the host's answer (or its error message) once
    /// [`poll`](Self::poll) has seen it.
    pub fn request(&mut self, method: &str, params: Value) -> Receiver<Result<Value, String>> {
        let id = self.next_id;
        self.next_id += 1;
        let (tx, rx) = mpsc::channel();
        self.pending.insert(id, tx);
        let _ = self.writer.send(json!({ "id": id, "method": method, "params": params }).to_string());
        rx
    }

    /// Answer a request the host made.
    pub fn respond(&self, id: Value, result: Result<Value, String>) {
        let message = match result {
            Ok(value) => json!({ "id": id, "result": value }),
            Err(message) => json!({ "id": id, "error": { "message": message } }),
        };
        let _ = self.writer.send(message.to_string());
    }

    /// Everything that arrived since the last call. Answers to our requests are delivered to
    /// their receivers; the rest is returned.
    pub fn poll(&mut self) -> Vec<Incoming> {
        while let Ok((id, result)) = self.responses.try_recv() {
            if let Some(waiting) = self.pending.remove(&id) {
                let _ = waiting.send(result);
            }
        }
        let mut out = Vec::new();
        while let Ok(message) = self.incoming.try_recv() {
            out.push(message);
        }
        out
    }

    pub fn is_alive(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(None))
    }

    /// Ask the host to deactivate extensions and exit, then make sure it is gone.
    pub fn shutdown(&mut self) {
        self.notify("shutdown", Value::Null);
        std::thread::sleep(std::time::Duration::from_millis(50));
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Drop for ExtHost {
    fn drop(&mut self) {
        let _ = self.child.kill();
    }
}

fn route(message: Value, incoming: &Sender<Incoming>, responses: &Sender<(u64, Result<Value, String>)>) {
    let method = message.get("method").and_then(Value::as_str).map(str::to_owned);
    let id = message.get("id").cloned();
    match (method, id) {
        (Some(method), Some(id)) => {
            let _ = incoming.send(Incoming::Request { id, method, params: message.get("params").cloned().unwrap_or(Value::Null) });
        }
        (Some(method), None) => {
            let _ = incoming.send(Incoming::Notification { method, params: message.get("params").cloned().unwrap_or(Value::Null) });
        }
        (None, Some(id)) => {
            let Some(id) = id.as_u64() else { return };
            let result = match message.get("error") {
                Some(error) => Err(error.get("message").and_then(Value::as_str).unwrap_or("the extension host reported an error").to_owned()),
                None => Ok(message.get("result").cloned().unwrap_or(Value::Null)),
            };
            let _ = responses.send((id, result));
        }
        (None, None) => {}
    }
}

// ---- activation ----

/// Something that can make an extension start.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ActivationEvent {
    /// The editor has started (`*` and `onStartupFinished` extensions run now).
    Startup,
    Language(String),
    Command(String),
    /// A file matching a glob exists in the workspace (checked by the caller).
    WorkspaceContains(String),
}

impl Extension {
    /// Whether `event` should start this extension. Extensions without code never do.
    pub fn activates_on(&self, event: &ActivationEvent) -> bool {
        if !self.needs_node_host() {
            return false;
        }
        self.manifest.activation_events.iter().any(|declared| match event {
            ActivationEvent::Startup => declared == "*" || declared == "onStartupFinished",
            ActivationEvent::Language(id) => declared.strip_prefix("onLanguage:").is_some_and(|l| l == id),
            ActivationEvent::Command(id) => declared.strip_prefix("onCommand:").is_some_and(|c| c == id),
            ActivationEvent::WorkspaceContains(pattern) => declared.strip_prefix("workspaceContains:").is_some_and(|p| p == pattern),
        })
    }

    /// Whether the extension can only start by one of its commands (VS Code 1.74+ activates an
    /// extension on its contributed commands without listing them).
    pub fn contributes_command(&self, id: &str) -> bool {
        self.manifest.contributes.commands.iter().any(|c| c.command == id)
    }

    /// `contributes.configuration` defaults as a flat `key -> value` map.
    pub fn configuration_defaults(&self) -> serde_json::Map<String, Value> {
        let mut defaults = serde_json::Map::new();
        let sections: Vec<&Value> = match &self.manifest.contributes.configuration {
            Value::Array(items) => items.iter().collect(),
            Value::Object(_) => vec![&self.manifest.contributes.configuration],
            _ => Vec::new(),
        };
        for section in sections {
            if let Some(properties) = section.get("properties").and_then(Value::as_object) {
                for (key, property) in properties {
                    if let Some(default) = property.get("default") {
                        defaults.insert(key.clone(), default.clone());
                    }
                }
            }
        }
        defaults
    }

    /// The `init` description of this extension for the host.
    pub fn host_description(&self) -> Value {
        json!({
            "id": self.id(),
            "root": self.root.to_string_lossy(),
            "main": self.manifest.main,
            "version": self.manifest.version,
            "displayName": self.display_name(),
            "packageJSON": self.manifest_json,
            "configurationDefaults": self.configuration_defaults(),
        })
    }
}

/// Reject a `main` path that would load code from outside the extension folder.
pub fn confined_main(extension: &Extension) -> Result<PathBuf> {
    let main = extension.manifest.main.as_deref().context("the extension has no main file")?;
    let mut path = extension.resolve(main).with_context(|| format!("main {main:?} leaves the extension folder"))?;
    // `main` may omit the extension, as `require` does.
    if path.extension().is_none() && !path.exists() {
        path.set_extension("js");
    }
    if !path.is_file() {
        bail!("{} does not exist", path.display());
    }
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse_manifest;

    fn extension(json: &str) -> Extension {
        Extension::new(PathBuf::from("C:/exts/x"), parse_manifest(json).unwrap())
    }

    #[test]
    fn extensions_start_only_on_the_events_they_declare() {
        let e = extension(r#"{ "name": "a", "main": "./m.js", "activationEvents": ["onLanguage:python", "onCommand:a.run", "onStartupFinished", "workspaceContains:**/Cargo.toml"] }"#);
        assert!(e.activates_on(&ActivationEvent::Startup));
        assert!(e.activates_on(&ActivationEvent::Language("python".into())));
        assert!(!e.activates_on(&ActivationEvent::Language("pythonic".into())));
        assert!(e.activates_on(&ActivationEvent::Command("a.run".into())));
        assert!(!e.activates_on(&ActivationEvent::Command("a.other".into())));
        assert!(e.activates_on(&ActivationEvent::WorkspaceContains("**/Cargo.toml".into())));
        let star = extension(r#"{ "name": "b", "main": "./m.js", "activationEvents": ["*"] }"#);
        assert!(star.activates_on(&ActivationEvent::Startup));
        assert!(!star.activates_on(&ActivationEvent::Language("rust".into())), "`*` means startup only");
    }

    #[test]
    fn extensions_without_code_never_activate() {
        let theme = extension(r#"{ "name": "t", "activationEvents": ["*"] }"#);
        assert!(!theme.activates_on(&ActivationEvent::Startup));
    }

    #[test]
    fn contributed_commands_are_known_by_id() {
        let e = extension(r#"{ "name": "a", "main": "m.js", "contributes": { "commands": [{ "command": "a.run", "title": "Run" }] } }"#);
        assert!(e.contributes_command("a.run") && !e.contributes_command("a.stop"));
    }

    #[test]
    fn configuration_defaults_come_from_one_section_or_several() {
        let one = extension(r#"{ "name": "a", "contributes": { "configuration": { "properties": { "a.x": { "default": 1 }, "a.y": { "type": "string" }, "a.z": { "default": [1, 2] } } } } }"#);
        let defaults = one.configuration_defaults();
        assert_eq!(defaults.len(), 2, "a property without a default has none");
        assert_eq!(defaults["a.x"], json!(1));
        assert_eq!(defaults["a.z"], json!([1, 2]));
        let many = extension(r#"{ "name": "a", "contributes": { "configuration": [{ "properties": { "a.p": { "default": true } } }, { "properties": { "a.q": { "default": "s" } } }] } }"#);
        assert_eq!(many.configuration_defaults().len(), 2);
        assert!(extension(r#"{ "name": "a" }"#).configuration_defaults().is_empty());
    }

    #[test]
    fn main_must_stay_inside_the_extension_and_exist() {
        let dir = std::env::temp_dir().join(format!("forge-main-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("out")).unwrap();
        std::fs::write(dir.join("out").join("extension.js"), "").unwrap();
        let with_main = |main: &str| {
            let mut e = extension(&format!(r#"{{ "name": "a", "main": {main:?} }}"#));
            e.root = dir.clone();
            e
        };
        assert!(confined_main(&with_main("./out/extension.js")).is_ok());
        assert!(confined_main(&with_main("./out/extension")).is_ok(), "`require` may omit .js");
        assert!(confined_main(&with_main("../escape.js")).is_err());
        assert!(confined_main(&with_main("./missing.js")).is_err());
        let mut none = extension(r#"{ "name": "a" }"#);
        none.root = dir.clone();
        assert!(confined_main(&none).is_err());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn host_files_are_written_and_refreshed() {
        let dir = std::env::temp_dir().join(format!("forge-hostfiles-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let script = install_host_files(&dir).unwrap();
        assert!(script.ends_with("host.js") && dir.join("vscode.js").is_file());
        std::fs::write(dir.join("vscode.js"), "stale").unwrap();
        install_host_files(&dir).unwrap();
        assert_ne!(std::fs::read_to_string(dir.join("vscode.js")).unwrap(), "stale", "an outdated copy is replaced");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn messages_are_routed_by_their_shape() {
        let (incoming, got) = mpsc::channel();
        let (responses, answers) = mpsc::channel();
        route(json!({ "method": "log", "params": { "text": "x" } }), &incoming, &responses);
        route(json!({ "id": "r1", "method": "workspace.applyEdit", "params": {} }), &incoming, &responses);
        route(json!({ "id": 7, "result": { "ok": true } }), &incoming, &responses);
        route(json!({ "id": 8, "error": { "message": "nope" } }), &incoming, &responses);
        route(json!({ "id": "not-ours", "result": 1 }), &incoming, &responses);
        route(json!({}), &incoming, &responses);
        let messages: Vec<Incoming> = got.try_iter().collect();
        assert_eq!(messages.len(), 2);
        assert!(matches!(&messages[0], Incoming::Notification { method, .. } if method == "log"));
        assert!(matches!(&messages[1], Incoming::Request { id, method, .. } if id == "r1" && method == "workspace.applyEdit"));
        let results: Vec<_> = answers.try_iter().collect();
        assert_eq!(results, vec![(7, Ok(json!({ "ok": true }))), (8, Err("nope".to_owned()))]);
    }
}
