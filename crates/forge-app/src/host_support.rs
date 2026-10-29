//! The extension host in the app: starting Node, activating extensions on their events,
//! keeping the host's view of open documents current, and acting on what extensions ask for
//! (status bar items, diagnostics, messages, edits, commands).

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::mpsc::Receiver;
use std::time::{Duration, Instant};

use forge_core::commands::Command;
use forge_core::settings::config_dir;
use forge_ext_host::host::{find_node, install_host_files, ActivationEvent, ExtHost, Incoming};
use forge_ext_host::Extension;
use forge_lsp::{Diagnostic, Position, Range, Severity, TextEditSpec};
use forge_syntax::Language;
use gpui::Context;
use serde_json::{json, Value};

use crate::workspace::{Doc, Item, ItemAction, Workspace};

/// Changes are sent to the host at most this often per tick.
const SYNC_INTERVAL: Duration = Duration::from_millis(100);
/// Output kept per channel.
const MAX_OUTPUT_BYTES: usize = 1 << 20;

#[derive(Clone, Debug, PartialEq)]
pub struct ExtStatus {
    pub text: String,
    pub tooltip: Option<String>,
    pub command: Option<String>,
    /// 1 = left, 2 = right.
    pub alignment: u8,
    pub priority: i64,
    pub visible: bool,
}

/// What a request to the host was for, so its answer can be routed.
enum Pending {
    Init,
    Activate(String),
    Command(String),
}

struct SentDoc {
    revision: u64,
    dirty: bool,
    path: String,
}

#[derive(Default)]
pub struct HostState {
    /// Node was not found or the host could not start: not retried until a restart is asked for.
    pub failed: bool,
    pub ready: bool,
    pub active: HashSet<String>,
    pub activating: HashSet<String>,
    /// Commands queued until their extension finishes activating.
    after_activation: HashMap<String, Vec<String>>,
    /// Commands extension code has registered.
    pub commands: HashSet<String>,
    pub status: BTreeMap<String, ExtStatus>,
    pub output: BTreeMap<String, String>,
    /// Diagnostics per collection and file, as extensions last published them.
    diagnostics: HashMap<String, HashMap<String, (PathBuf, Vec<Diagnostic>)>>,
    pending: Vec<(Receiver<Result<Value, String>>, Pending)>,
    docs: HashMap<String, SentDoc>,
    editor: Option<(String, Vec<[usize; 4]>)>,
    last_sync: Option<Instant>,
}

fn key_of(path: &Path) -> String {
    let text = path.to_string_lossy().replace('\\', "/");
    if cfg!(windows) {
        text.to_lowercase()
    } else {
        text
    }
}

/// `$(icon-name)` codicon references have no glyphs here: drop them.
pub fn strip_codicons(text: &str) -> String {
    let mut out = String::new();
    let mut rest = text;
    while let Some(start) = rest.find("$(") {
        match rest[start..].find(')') {
            Some(end) if rest[start + 2..start + end].chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '~') => {
                out.push_str(&rest[..start]);
                rest = &rest[start + end + 1..];
            }
            _ => {
                out.push_str(&rest[..start + 2]);
                rest = &rest[start + 2..];
            }
        }
    }
    out.push_str(rest);
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

pub fn diagnostic_from_wire(value: &Value) -> Option<Diagnostic> {
    let range = value.get("range")?.as_array()?;
    let n = |i: usize| range.get(i).and_then(Value::as_u64).map(|v| v as usize);
    let severity = match value.get("severity").and_then(Value::as_u64) {
        Some(0) | None => Severity::Error,
        Some(1) => Severity::Warning,
        Some(2) => Severity::Information,
        Some(_) => Severity::Hint,
    };
    Some(Diagnostic {
        range: Range { start: Position { line: n(0)?, character: n(1)? }, end: Position { line: n(2)?, character: n(3)? } },
        severity,
        message: value.get("message")?.as_str()?.to_owned(),
        source: value.get("source").and_then(Value::as_str).map(str::to_owned),
    })
}

pub fn edits_from_wire(changes: &Value) -> Vec<(PathBuf, Vec<TextEditSpec>)> {
    let Some(files) = changes.as_array() else { return Vec::new() };
    files
        .iter()
        .filter_map(|file| {
            let path = PathBuf::from(file.get("path")?.as_str()?);
            let edits = file
                .get("edits")?
                .as_array()?
                .iter()
                .filter_map(|e| {
                    let r = e.get("range")?.as_array()?;
                    let n = |i: usize| r.get(i).and_then(Value::as_u64).map(|v| v as usize);
                    Some(TextEditSpec { range: Range { start: Position { line: n(0)?, character: n(1)? }, end: Position { line: n(2)?, character: n(3)? } }, new_text: e.get("newText")?.as_str()?.to_owned() })
                })
                .collect();
            Some((path, edits))
        })
        .collect()
}

fn language_id_of(doc: &Doc) -> String {
    doc.language.map(|l| l.language_id().to_owned()).or_else(|| doc.ext_lang.as_ref().map(|l| l.id.clone())).unwrap_or_else(|| "plaintext".to_owned())
}

impl Workspace {
    fn host_extensions(&self) -> Vec<&Extension> {
        self.extensions.iter().filter(|e| e.needs_node_host()).collect()
    }

    /// The user's settings.json as an object, for `workspace.getConfiguration`.
    fn host_settings(&self) -> Value {
        std::fs::read_to_string(config_dir().join("settings.json"))
            .ok()
            .and_then(|text| forge_core::jsonc::parse(&text).ok())
            .filter(Value::is_object)
            .unwrap_or_else(|| json!({}))
    }

    /// Start the host if some installed extension needs it. False if it is not running.
    pub fn ensure_host(&mut self) -> bool {
        if self.host.is_some() {
            return true;
        }
        if self.host_state.failed || self.host_extensions().is_empty() {
            return false;
        }
        let Some(node) = find_node() else {
            self.host_state.failed = true;
            self.set_status("Extensions with code need Node.js on PATH; it was not found");
            return false;
        };
        let dir = config_dir().join("extension-host");
        let started = install_host_files(&dir.join("runtime")).and_then(|script| ExtHost::start(&node, &script, self.root()));
        let mut host = match started {
            Ok(host) => host,
            Err(error) => {
                self.host_state.failed = true;
                self.set_status(format!("Could not start the extension host: {error:#}"));
                return false;
            }
        };
        let mut languages: Vec<String> = Language::ALL.iter().map(|l| l.language_id().to_owned()).collect();
        languages.extend(self.ext_langs.iter().map(|l| l.id.clone()));
        languages.push("plaintext".to_owned());
        let init = host.request(
            "init",
            json!({
                "extensions": self.host_extensions().iter().map(|e| e.host_description()).collect::<Vec<_>>(),
                "settings": self.host_settings(),
                "folders": [self.root().to_string_lossy()],
                "languages": languages,
                "storageDir": dir.join("storage").to_string_lossy(),
            }),
        );
        self.host_state.pending.push((init, Pending::Init));
        self.host = Some(host);
        true
    }

    /// A document was opened: start the extensions that activate for its language.
    pub fn activate_for_doc(&mut self, doc_ix: usize) {
        let id = language_id_of(&self.docs[doc_ix]);
        self.activate_for(ActivationEvent::Language(id));
    }

    /// Start every extension that declares `event`.
    pub fn activate_for(&mut self, event: ActivationEvent) {
        let ids: Vec<String> = self.host_extensions().iter().filter(|e| e.activates_on(&event)).map(|e| e.id()).collect();
        for id in ids {
            self.activate_extension(&id, None);
        }
    }

    /// Activate `id`; `then` is a command to run once it is up.
    fn activate_extension(&mut self, id: &str, then: Option<String>) {
        if self.host_state.active.contains(id) {
            if let Some(command) = then {
                self.send_command(&command, Vec::new());
            }
            return;
        }
        if let Some(command) = then {
            self.host_state.after_activation.entry(id.to_owned()).or_default().push(command);
        }
        if self.host_state.activating.contains(id) || !self.ensure_host() {
            return;
        }
        self.host_state.activating.insert(id.to_owned());
        if let Some(host) = self.host.as_mut() {
            let rx = host.request("activate", json!({ "id": id }));
            self.host_state.pending.push((rx, Pending::Activate(id.to_owned())));
        }
    }

    fn send_command(&mut self, id: &str, args: Vec<Value>) {
        if let Some(host) = self.host.as_mut() {
            let rx = host.request("executeCommand", json!({ "command": id, "args": args }));
            self.host_state.pending.push((rx, Pending::Command(id.to_owned())));
        }
    }

    /// Run a command: an extension's (starting it if needed) or one of Forge's own.
    pub fn run_ext_command(&mut self, id: &str, cx: &mut Context<Self>) {
        if self.host_state.commands.contains(id) {
            self.send_command(id, Vec::new());
            return;
        }
        let owner = self.host_extensions().iter().find(|e| e.contributes_command(id)).map(|e| e.id());
        match (owner, Command::from_id(id)) {
            (Some(owner), _) => {
                self.activate_extension(&owner, Some(id.to_owned()));
                if self.host.is_none() {
                    self.set_status(format!("{id} needs the extension host, which is not running"));
                }
            }
            (None, Some(command)) => self.run_command(command, false, cx),
            (None, None) => self.set_status(format!("Unknown command {id}")),
        }
    }

    /// Palette entries for the commands installed extensions contribute.
    pub fn ext_command_items(&self, query: &str) -> Vec<Item> {
        let query = query.to_lowercase();
        let mut items = Vec::new();
        for extension in self.host_extensions() {
            for command in &extension.manifest.contributes.commands {
                let title = extension.localize(command.title.as_deref().unwrap_or(&command.command));
                if query.is_empty() || title.to_lowercase().contains(&query) || command.command.to_lowercase().contains(&query) {
                    items.push(Item { label: title, detail: extension.display_name(), positions: Vec::new(), action: ItemAction::ExtCommand(command.command.clone()) });
                }
            }
        }
        items.truncate(30);
        items
    }

    /// Stop the host and start again with the installed extensions (after an install or a crash).
    pub fn restart_host(&mut self) {
        if let Some(mut host) = self.host.take() {
            host.shutdown();
        }
        for doc in 0..self.docs.len() {
            self.docs[doc].ext_diagnostics.clear();
            self.docs[doc].rebuild_diagnostics();
        }
        self.host_state = HostState::default();
        self.activate_for(ActivationEvent::Startup);
        let ids: Vec<String> = self.docs.iter().filter(|d| !d.readonly).map(language_id_of).collect();
        for id in ids.into_iter().collect::<HashSet<_>>() {
            self.activate_for(ActivationEvent::Language(id));
        }
    }

    /// Work through everything the host has said since the last tick.
    pub fn poll_host(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(host) = self.host.as_mut() else { return false };
        let incoming = host.poll();
        let mut changed = !incoming.is_empty();
        for message in incoming {
            match message {
                Incoming::Notification { method, params } => self.on_host_notification(&method, &params, cx),
                Incoming::Request { id, method, params } => {
                    let result = self.on_host_request(&method, &params, cx);
                    if let Some(host) = self.host.as_ref() {
                        host.respond(id, result);
                    }
                }
                Incoming::Exited(reason) => {
                    self.host = None;
                    let had_work = !self.host_state.active.is_empty();
                    self.host_state = HostState { failed: true, ..HostState::default() };
                    for doc in &mut self.docs {
                        doc.ext_diagnostics.clear();
                        doc.rebuild_diagnostics();
                    }
                    if had_work {
                        self.set_status(format!("The extension host stopped ({reason}). Run \"Extensions: Restart Extension Host\"."));
                    }
                    return true;
                }
            }
        }
        // Answers to our own requests.
        let mut still = Vec::new();
        for (rx, kind) in std::mem::take(&mut self.host_state.pending) {
            match rx.try_recv() {
                Ok(result) => {
                    changed = true;
                    self.on_host_answer(kind, result);
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => still.push((rx, kind)),
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {}
            }
        }
        still.append(&mut self.host_state.pending);
        self.host_state.pending = still;
        if self.host_state.ready {
            changed |= self.sync_host_docs();
        }
        changed
    }

    fn on_host_answer(&mut self, kind: Pending, result: Result<Value, String>) {
        match (kind, result) {
            (Pending::Init, Ok(_)) => self.host_state.ready = true,
            (Pending::Init, Err(error)) => {
                self.set_status(format!("The extension host could not start: {error}"));
                self.host_state.failed = true;
            }
            (Pending::Activate(id), Ok(_)) => {
                self.host_state.activating.remove(&id);
                self.host_state.active.insert(id.clone());
                for command in self.host_state.after_activation.remove(&id).unwrap_or_default() {
                    self.send_command(&command, Vec::new());
                }
            }
            (Pending::Activate(id), Err(error)) => {
                self.host_state.activating.remove(&id);
                self.host_state.after_activation.remove(&id);
                self.set_status(format!("Extension {id} failed to start: {error}"));
            }
            (Pending::Command(_), Ok(_)) => {}
            (Pending::Command(id), Err(error)) => self.set_status(format!("{id} failed: {error}")),
        }
    }

    fn on_host_notification(&mut self, method: &str, params: &Value, cx: &mut Context<Self>) {
        let text = |name: &str| params.get(name).and_then(Value::as_str).unwrap_or_default().to_owned();
        match method {
            "log" => eprintln!("[ext {}] {}", text("level"), text("text").trim_end()),
            "window.message" => {
                let prefix = match text("kind").as_str() {
                    "error" => "Error: ",
                    "warning" => "Warning: ",
                    _ => "",
                };
                self.set_status(format!("{prefix}{}", text("text")));
            }
            "status.update" => {
                let id = text("id");
                let item = ExtStatus {
                    text: strip_codicons(&text("text")),
                    tooltip: params.get("tooltip").and_then(Value::as_str).map(str::to_owned),
                    command: params.get("command").and_then(Value::as_str).map(str::to_owned),
                    alignment: params.get("alignment").and_then(Value::as_u64).unwrap_or(1) as u8,
                    priority: params.get("priority").and_then(Value::as_i64).unwrap_or(0),
                    visible: params.get("visible").and_then(Value::as_bool).unwrap_or(false),
                };
                self.host_state.status.insert(id, item);
            }
            "status.dispose" => {
                self.host_state.status.remove(&text("id"));
            }
            "diagnostics.set" => self.on_host_diagnostics(&text("collection"), params.get("entries")),
            "output.append" => {
                let channel = self.host_state.output.entry(text("channel")).or_default();
                channel.push_str(&text("text"));
                if channel.len() > MAX_OUTPUT_BYTES {
                    let cut = channel.len() - MAX_OUTPUT_BYTES / 2;
                    let boundary = (cut..channel.len()).find(|i| channel.is_char_boundary(*i)).unwrap_or(channel.len());
                    channel.drain(..boundary);
                }
            }
            "output.clear" => {
                self.host_state.output.insert(text("channel"), String::new());
            }
            "output.show" => {
                let name = text("channel");
                let body = self.host_state.output.get(&name).cloned().unwrap_or_default();
                self.open_virtual(&format!("output:{name}"), &format!("Output: {name}"), &body, false, cx);
            }
            "commands.registered" => {
                self.host_state.commands.insert(text("id"));
            }
            "commands.unregistered" => {
                self.host_state.commands.remove(&text("id"));
            }
            "extension.error" => self.set_status(format!("Extension {} failed: {}", text("id"), text("message"))),
            _ => {}
        }
    }

    fn on_host_diagnostics(&mut self, collection: &str, entries: Option<&Value>) {
        let Some(entries) = entries.and_then(Value::as_array) else { return };
        let mut touched = Vec::new();
        for entry in entries {
            let Some(path) = entry.get("path").and_then(Value::as_str).map(PathBuf::from) else { continue };
            let diagnostics: Vec<Diagnostic> = entry.get("diagnostics").and_then(Value::as_array).map(|d| d.iter().filter_map(diagnostic_from_wire).collect()).unwrap_or_default();
            let key = key_of(&path);
            let per_file = self.host_state.diagnostics.entry(collection.to_owned()).or_default();
            if diagnostics.is_empty() {
                per_file.remove(&key);
            } else {
                per_file.insert(key, (path.clone(), diagnostics));
            }
            touched.push(path);
        }
        for path in touched {
            if let Some(ix) = self.doc_index(&path) {
                self.refresh_ext_diagnostics(ix);
            }
        }
    }

    /// Rebuild a document's extension diagnostics from everything extensions published for it.
    pub fn refresh_ext_diagnostics(&mut self, doc_ix: usize) {
        let key = key_of(&self.docs[doc_ix].path);
        let mut collected = Vec::new();
        for per_file in self.host_state.diagnostics.values() {
            if let Some((_, diagnostics)) = per_file.get(&key) {
                collected.extend(diagnostics.iter().cloned());
            }
        }
        let doc = &mut self.docs[doc_ix];
        doc.ext_diagnostics = collected;
        doc.rebuild_diagnostics();
    }

    fn on_host_request(&mut self, method: &str, params: &Value, cx: &mut Context<Self>) -> Result<Value, String> {
        match method {
            "workspace.applyEdit" => {
                let files = edits_from_wire(params.get("changes").unwrap_or(&Value::Null));
                if files.is_empty() {
                    return Ok(json!(false));
                }
                self.apply_workspace_edit(files, cx);
                Ok(json!(true))
            }
            "commands.execute" => {
                let id = params.get("id").and_then(Value::as_str).ok_or("a command id is required")?.to_owned();
                if !self.host_state.commands.contains(&id) && Command::from_id(&id).is_none() && !self.host_extensions().iter().any(|e| e.contributes_command(&id)) {
                    return Err(format!("command '{id}' not found"));
                }
                self.run_ext_command(&id, cx);
                Ok(Value::Null)
            }
            "window.showTextDocument" => {
                let path = PathBuf::from(params.get("path").and_then(Value::as_str).ok_or("a path is required")?);
                self.open_file(&path, None, cx);
                Ok(Value::Null)
            }
            "document.save" => {
                let path = PathBuf::from(params.get("path").and_then(Value::as_str).ok_or("a path is required")?);
                let ix = self.doc_index(&path).ok_or("that document is not open")?;
                self.write_doc(ix);
                Ok(json!(true))
            }
            // Settings are not written back from extensions, and URLs are not opened on their say-so.
            "config.update" => Ok(Value::Null),
            "env.openExternal" => Err("Forge does not open links for extensions".to_owned()),
            other => Err(format!("unsupported request {other}")),
        }
    }

    /// Tell the host about opened, changed, saved and closed documents and the active editor.
    fn sync_host_docs(&mut self) -> bool {
        let due = self.host_state.last_sync.is_none_or(|t| t.elapsed() >= SYNC_INTERVAL);
        if !due || self.host.is_none() {
            return false;
        }
        self.host_state.last_sync = Some(Instant::now());
        let mut sent = false;
        let mut present = HashSet::new();
        for doc in self.docs.iter().filter(|d| !d.readonly) {
            let key = key_of(&doc.path);
            present.insert(key.clone());
            let path = doc.path.to_string_lossy().into_owned();
            let Some(host) = self.host.as_ref() else { break };
            match self.host_state.docs.get_mut(&key) {
                None => {
                    host.notify("doc.open", json!({ "path": path, "languageId": language_id_of(doc), "version": doc.revision + 1, "text": doc.buffer.text() }));
                    self.host_state.docs.insert(key, SentDoc { revision: doc.revision, dirty: doc.dirty, path });
                    sent = true;
                }
                Some(known) => {
                    if known.revision != doc.revision {
                        host.notify("doc.change", json!({ "path": path, "version": doc.revision + 1, "text": doc.buffer.text() }));
                        known.revision = doc.revision;
                        sent = true;
                    }
                    if known.dirty && !doc.dirty {
                        host.notify("doc.save", json!({ "path": path }));
                        sent = true;
                    }
                    known.dirty = doc.dirty;
                }
            }
        }
        let closed: Vec<String> = self.host_state.docs.keys().filter(|k| !present.contains(*k)).cloned().collect();
        for key in closed {
            if let (Some(host), Some(known)) = (self.host.as_ref(), self.host_state.docs.remove(&key)) {
                host.notify("doc.close", json!({ "path": known.path }));
                sent = true;
            }
        }

        // The active editor: which file and where the selections are.
        let active = self.active_view().zip(self.active_doc()).filter(|(_, doc)| !doc.readonly).map(|(view, doc)| {
            let selections: Vec<[usize; 4]> = view
                .selections
                .as_slice()
                .iter()
                .map(|s| {
                    let (al, ac) = doc.buffer.utf16_point(s.anchor);
                    let (hl, hc) = doc.buffer.utf16_point(s.head);
                    [al, ac, hl, hc]
                })
                .collect();
            (key_of(&doc.path), doc.path.to_string_lossy().into_owned(), selections)
        });
        let current = active.as_ref().map(|(key, _, selections)| (key.clone(), selections.clone()));
        if current != self.host_state.editor {
            if let Some(host) = self.host.as_ref() {
                match &active {
                    Some((_, path, selections)) => host.notify("editor.active", json!({ "path": path, "selections": selections })),
                    None => host.notify("editor.active", json!({ "path": null, "selections": [] })),
                }
            }
            self.host_state.editor = current;
            sent = true;
        }
        sent
    }

    /// Let the host see a changed settings.json.
    pub fn host_config_changed(&mut self) {
        if let Some(host) = self.host.as_ref() {
            host.notify("config.changed", json!({ "settings": self.host_settings() }));
        }
    }

    /// Status bar items of one alignment, in priority order (highest first).
    pub fn ext_status_items(&self, alignment: u8) -> Vec<(&String, &ExtStatus)> {
        let mut items: Vec<_> = self.host_state.status.iter().filter(|(_, s)| s.visible && s.alignment == alignment && !s.text.is_empty()).collect();
        items.sort_by_key(|(_, s)| std::cmp::Reverse(s.priority));
        items
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codicon_references_are_removed_from_status_text() {
        assert_eq!(strip_codicons("$(checklist) 3 TODOs"), "3 TODOs");
        assert_eq!(strip_codicons("a $(sync~spin) b $(git-branch)"), "a b");
        assert_eq!(strip_codicons("cost $5 (est)"), "cost $5 (est)");
        assert_eq!(strip_codicons("$( not an icon )"), "$( not an icon )");
        assert_eq!(strip_codicons("$(unterminated"), "$(unterminated");
        assert_eq!(strip_codicons(""), "");
    }

    #[test]
    fn diagnostics_arrive_with_their_severity_and_source() {
        let d = diagnostic_from_wire(&json!({ "range": [1, 2, 3, 4], "message": "bad", "severity": 1, "source": "lint", "code": null })).unwrap();
        assert_eq!((d.range.start.line, d.range.start.character, d.range.end.line, d.range.end.character), (1, 2, 3, 4));
        assert_eq!((d.severity, d.message.as_str(), d.source.as_deref()), (Severity::Warning, "bad", Some("lint")));
        assert_eq!(diagnostic_from_wire(&json!({ "range": [0, 0, 0, 1], "message": "m" })).unwrap().severity, Severity::Error, "the default");
        assert_eq!(diagnostic_from_wire(&json!({ "range": [0, 0, 0, 1], "message": "m", "severity": 3 })).unwrap().severity, Severity::Hint);
        assert!(diagnostic_from_wire(&json!({ "range": [0, 0, 0], "message": "m" })).is_none());
        assert!(diagnostic_from_wire(&json!({ "range": [0, 0, 0, 1] })).is_none());
        assert!(diagnostic_from_wire(&json!({ "range": [-1, 0, 0, 1], "message": "m" })).is_none(), "negative positions are rejected");
    }

    #[test]
    fn edits_are_read_per_file_and_bad_entries_are_dropped() {
        let files = edits_from_wire(&json!([
            { "path": "C:/a.txt", "edits": [{ "range": [1, 0, 1, 4], "newText": "DONE" }, { "range": [9], "newText": "x" }, { "newText": "no range" }] },
            { "edits": [] },
            { "path": "C:/b.txt", "edits": [] }
        ]));
        assert_eq!(files.len(), 2);
        assert_eq!(files[0].0, PathBuf::from("C:/a.txt"));
        assert_eq!(files[0].1.len(), 1);
        assert_eq!(files[0].1[0].new_text, "DONE");
        assert!(edits_from_wire(&json!("nope")).is_empty());
    }

    #[test]
    fn paths_that_differ_only_by_case_or_slashes_share_a_key_on_windows() {
        if cfg!(windows) {
            assert_eq!(key_of(Path::new("C:\\Users\\A\\x.txt")), key_of(Path::new("c:/users/a/X.TXT")));
        }
        assert_ne!(key_of(Path::new("/a/b")), key_of(Path::new("/a/c")));
    }
}
