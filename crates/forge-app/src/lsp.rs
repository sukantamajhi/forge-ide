//! Owns the language-server processes for a workspace. Nothing here blocks the UI
//! thread: requests return receivers that [`LspManager::poll`] drains each tick.
//!
//! Servers are identified by a key and may serve several languages (one TypeScript
//! server handles `.js`, `.jsx`, `.ts` and `.tsx`).

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, TryRecvError};

use forge_core::servers::{self, ServerSpec};
use forge_lsp::{
    answer_server_request, did_change_params, did_open_params, parse_diagnostics, parse_progress, path_to_uri, ContentChange, Diagnostic,
    LspClient, ProgressKind, ServerMessage,
};
use forge_syntax::Language;
use serde_json::{json, Value};

type Response = Receiver<Result<Value, Value>>;

/// What a pending request was for, so its response can be routed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RequestTag {
    /// Carries the request number so stale responses can be dropped.
    Completion(u64),
    Definition,
    Hover(u64),
    Signature(u64),
    References,
    Rename,
    CodeActions,
    /// Format the document; save afterwards when set (format on save).
    Format { then_save: bool },
    Symbols,
}

pub enum Event {
    Ready { key: String },
    Diagnostics { path: PathBuf, diagnostics: Vec<Diagnostic> },
    Progress { title: String, text: Option<String> },
    Response { tag: RequestTag, result: Value },
    Failed { key: String, title: String, message: String },
}

enum State {
    Initializing(Response),
    Ready,
}

struct Server {
    client: LspClient,
    state: State,
    title: String,
    /// Active `$/progress` tokens and their latest text (e.g. indexing).
    progress: HashMap<String, String>,
}

pub struct LspManager {
    root: PathBuf,
    servers: HashMap<String, Server>,
    /// Servers that could not start or died, with the reason. Not retried until forgotten.
    unavailable: HashMap<String, String>,
    pending: Vec<(String, RequestTag, Response)>,
    /// `languageServers` from settings.json.
    overrides: BTreeMap<String, (String, Vec<String>)>,
}

impl LspManager {
    pub fn new(root: PathBuf) -> Self {
        Self { root, servers: HashMap::new(), unavailable: HashMap::new(), pending: Vec::new(), overrides: BTreeMap::new() }
    }

    pub fn set_overrides(&mut self, overrides: BTreeMap<String, (String, Vec<String>)>) {
        if overrides != self.overrides {
            self.overrides = overrides;
            // A changed configuration deserves a fresh attempt.
            self.unavailable.clear();
        }
    }

    fn spec(&self, language: Language) -> Option<ServerSpec> {
        servers::spec_for(language, &self.overrides)
    }

    /// The server key that handles `language`, if any is configured.
    pub fn key_for(&self, language: Language) -> Option<String> {
        self.spec(language).map(|s| s.key)
    }

    /// Why a file in `language` has no running server to talk to, for status messages and logs.
    pub fn unavailable_reason(&self, language: Language) -> String {
        let Some(spec) = self.spec(language) else {
            return format!("no language server is configured for {}", language.name());
        };
        if let Some(reason) = self.unavailable.get(&spec.key) {
            return format!("{} could not start: {reason}", spec.title);
        }
        match self.servers.get(&spec.key) {
            Some(server) if matches!(server.state, State::Ready) => format!("{} is running but this file was not opened on it", spec.title),
            Some(_) => format!("{} is still starting; try again in a moment", spec.title),
            None => format!("{} has not been started", spec.title),
        }
    }

    /// Allow servers that failed to start to be tried again (after an install).
    pub fn forget_unavailable(&mut self) {
        self.unavailable.clear();
    }

    /// Start the server for `language` if it is not running. `Err` says why it cannot.
    pub fn start(&mut self, language: Language) -> Result<(), String> {
        let spec = self.spec(language).ok_or_else(|| format!("no language server configured for {}", language.name()))?;
        if self.servers.contains_key(&spec.key) {
            return Ok(());
        }
        if let Some(reason) = self.unavailable.get(&spec.key) {
            return Err(reason.clone());
        }
        let started = self.spawn(&spec);
        if let Err(reason) = &started {
            self.unavailable.insert(spec.key.clone(), reason.clone());
        }
        started
    }

    fn spawn(&mut self, spec: &ServerSpec) -> Result<(), String> {
        let exe = servers::resolve(spec)?;
        let args = servers::expanded_args(spec);
        let arg_refs: Vec<&str> = args.iter().map(String::as_str).collect();
        let mut client = LspClient::spawn_in(&exe.to_string_lossy(), &arg_refs, Some(&self.root)).map_err(|e| format!("{e:#}"))?;
        let root_uri = path_to_uri(&self.root);
        eprintln!("[lsp] starting {} ({}) root {:?}", spec.title, exe.display(), root_uri);
        let init = client.begin_initialize_with(root_uri.as_deref(), servers::init_options(spec)).map_err(|e| format!("{e:#}"))?;
        self.servers.insert(
            spec.key.clone(),
            Server { client, state: State::Initializing(init), title: spec.title.clone(), progress: HashMap::new() },
        );
        Ok(())
    }

    fn ready(&mut self, language: Language) -> Option<&mut Server> {
        let key = self.key_for(language)?;
        self.servers.get_mut(&key).filter(|s| matches!(s.state, State::Ready))
    }

    /// Returns false when the server is not ready yet; the caller opens again on [`Event::Ready`].
    pub fn did_open(&mut self, language: Language, path: &Path, version: i32, text: &str) -> bool {
        let Some(uri) = path_to_uri(path) else { return false };
        let Some(server) = self.ready(language) else { return false };
        let params = did_open_params(&uri, language.language_id(), version, text);
        server.client.notify("textDocument/didOpen", params).is_ok()
    }

    pub fn did_change(&mut self, language: Language, path: &Path, version: i32, changes: &[ContentChange]) {
        let (Some(uri), Some(server)) = (path_to_uri(path), self.ready(language)) else { return };
        let _ = server.client.notify("textDocument/didChange", did_change_params(&uri, version, changes));
    }

    pub fn did_save(&mut self, language: Language, path: &Path) {
        let (Some(uri), Some(server)) = (path_to_uri(path), self.ready(language)) else { return };
        let _ = server.client.notify("textDocument/didSave", json!({ "textDocument": { "uri": uri } }));
    }

    pub fn did_close(&mut self, language: Language, path: &Path) {
        let (Some(uri), Some(server)) = (path_to_uri(path), self.ready(language)) else { return };
        let _ = server.client.notify("textDocument/didClose", json!({ "textDocument": { "uri": uri } }));
    }

    pub fn request(&mut self, language: Language, method: &str, params: Value, tag: RequestTag) {
        let Some(key) = self.key_for(language) else {
            eprintln!("[lsp] {method}: no server configured for {language:?}");
            return;
        };
        let Some(server) = self.servers.get_mut(&key).filter(|s| matches!(s.state, State::Ready)) else {
            eprintln!("[lsp] {method}: server {key} is not ready");
            return;
        };
        match server.client.request(method, params) {
            Ok(response) => {
                eprintln!("[lsp] {method} sent to {key}");
                self.pending.push((key, tag, response));
            }
            Err(err) => eprintln!("[lsp] {method}: could not send to {key}: {err}"),
        }
    }

    /// Drain everything that happened since the last call.
    pub fn poll(&mut self) -> Vec<Event> {
        let mut events = Vec::new();
        let mut dead: Vec<(String, String, String)> = Vec::new();

        for (key, server) in &mut self.servers {
            if !server.client.is_alive() {
                dead.push((key.clone(), server.title.clone(), "language server exited".to_owned()));
                continue;
            }
            if let State::Initializing(response) = &server.state {
                match response.try_recv() {
                    Ok(Ok(_)) => {
                        if server.client.initialized().is_ok() {
                            eprintln!("[lsp] {} ready", server.title);
                            server.state = State::Ready;
                            events.push(Event::Ready { key: key.clone() });
                        } else {
                            dead.push((key.clone(), server.title.clone(), "language server closed during startup".to_owned()));
                        }
                    }
                    Ok(Err(error)) => dead.push((key.clone(), server.title.clone(), format!("initialize failed: {error}"))),
                    Err(TryRecvError::Disconnected) => {
                        dead.push((key.clone(), server.title.clone(), "language server closed during startup".to_owned()))
                    }
                    Err(TryRecvError::Empty) => {}
                }
            }

            for message in server.client.poll() {
                match message {
                    ServerMessage::Request { id, method, params } => {
                        let _ = server.client.respond(id, answer_server_request(&method, &params));
                    }
                    ServerMessage::Notification { method, params } => match method.as_str() {
                        "textDocument/publishDiagnostics" => {
                            if let Some((path, diagnostics)) = parse_diagnostics(&params) {
                                events.push(Event::Diagnostics { path, diagnostics });
                            }
                        }
                        "$/progress" => {
                            if let Some(update) = parse_progress(&params) {
                                match update.kind {
                                    ProgressKind::End => {
                                        server.progress.remove(&update.token);
                                    }
                                    _ => {
                                        let entry = server.progress.entry(update.token).or_default();
                                        let text = [update.title, update.message].into_iter().flatten().collect::<Vec<_>>().join(": ");
                                        if !text.is_empty() {
                                            *entry = text;
                                        }
                                    }
                                }
                                let text = server.progress.values().find(|t| !t.is_empty()).cloned();
                                events.push(Event::Progress { title: server.title.clone(), text });
                            }
                        }
                        _ => {}
                    },
                }
            }
        }

        for (key, title, message) in dead {
            eprintln!("[lsp] {title} failed: {message}");
            self.servers.remove(&key);
            self.unavailable.insert(key.clone(), message.clone());
            self.pending.retain(|(k, _, _)| *k != key);
            events.push(Event::Failed { key, title, message });
        }

        self.pending.retain(|(_, tag, response)| match response.try_recv() {
            Ok(Ok(result)) => {
                events.push(Event::Response { tag: *tag, result });
                false
            }
            // An error reply (e.g. "content modified") just means no answer this time.
            Ok(Err(error)) => {
                eprintln!("[lsp] {tag:?} failed: {error}");
                if *tag == RequestTag::Definition {
                    // Null parses to no locations, so the status bar says "No definition found".
                    events.push(Event::Response { tag: *tag, result: Value::Null });
                }
                false
            }
            Err(TryRecvError::Disconnected) => {
                eprintln!("[lsp] {tag:?} dropped: server closed the reply channel");
                false
            }
            Err(TryRecvError::Empty) => true,
        });
        events
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_key_serves_the_whole_javascript_family() {
        let manager = LspManager::new(std::env::temp_dir());
        let keys: Vec<_> = [Language::JavaScript, Language::JavaScriptReact, Language::TypeScript, Language::TypeScriptReact]
            .iter()
            .map(|l| manager.key_for(*l))
            .collect();
        assert!(keys.iter().all(|k| k.as_deref() == Some("typescript")), "{keys:?}");
        assert_eq!(manager.key_for(Language::Env), None);
    }

    #[test]
    fn languages_without_a_server_are_reported_once_and_remembered() {
        let mut manager = LspManager::new(std::env::temp_dir());
        let first = manager.start(Language::Env).unwrap_err();
        assert!(first.contains("no language server configured"));
        assert_eq!(manager.start(Language::Env).unwrap_err(), first);
        // Nothing is ready, so notifications and requests are quietly dropped.
        assert!(!manager.did_open(Language::Env, Path::new("x.env"), 1, "A=1"));
        manager.request(Language::Env, "x", Value::Null, RequestTag::Definition);
        assert!(manager.poll().is_empty());
    }

    #[test]
    fn a_missing_server_explains_itself_and_is_retried_after_forgetting() {
        let mut manager = LspManager::new(std::env::temp_dir());
        let mut overrides = BTreeMap::new();
        overrides.insert("lua".to_owned(), ("definitely-not-a-real-lua-ls".to_owned(), Vec::new()));
        manager.set_overrides(overrides.clone());
        let error = manager.start(Language::Lua).unwrap_err();
        assert!(error.contains("definitely-not-a-real-lua-ls") || error.contains("not installed"), "{error}");
        // Remembered: same answer without another attempt.
        assert_eq!(manager.start(Language::Lua).unwrap_err(), error);
        // Same overrides keep the memory; changed overrides or an explicit forget reset it.
        manager.set_overrides(overrides);
        assert!(manager.unavailable.contains_key("custom:lua"));
        manager.forget_unavailable();
        assert!(manager.unavailable.is_empty());
    }
}

/// Starts each installed server for real through [`LspManager`] and checks it answers.
/// A server that is not installed is skipped (install it with "Language Servers: Manage and Install...").
#[cfg(test)]
mod real_servers {
    use std::time::{Duration, Instant};

    use super::*;

    enum Expect {
        /// Any diagnostic for the file.
        Diagnostic,
        /// A document symbol with this name.
        Symbol(&'static str),
    }

    fn exercise(language: Language, file: &str, text: &str, expect: &Expect) -> Result<String, String> {
        let dir = std::env::temp_dir().join(format!("forge-real-{}-{}", std::process::id(), file.replace('.', "_")));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        let path = dir.join(file);
        std::fs::write(&path, text).map_err(|e| e.to_string())?;
        let result = run(language, &dir, &path, text, expect);
        let _ = std::fs::remove_dir_all(&dir);
        result
    }

    fn run(language: Language, dir: &Path, path: &Path, text: &str, expect: &Expect) -> Result<String, String> {
        let mut manager = LspManager::new(dir.to_path_buf());
        manager.start(language)?;
        let uri = path_to_uri(path).ok_or("no uri")?;
        let deadline = Instant::now() + Duration::from_secs(90);
        let mut seen = Vec::new();
        while Instant::now() < deadline {
            for event in manager.poll() {
                match event {
                    Event::Ready { .. } => {
                        if !manager.did_open(language, path, 1, text) {
                            return Err("didOpen was refused".to_owned());
                        }
                        if matches!(expect, Expect::Symbol(_)) {
                            manager.request(language, "textDocument/documentSymbol", json!({ "textDocument": { "uri": uri } }), RequestTag::Symbols);
                        }
                    }
                    Event::Diagnostics { path: p, diagnostics } if p == path && matches!(expect, Expect::Diagnostic) => {
                        if let Some(first) = diagnostics.first() {
                            return Ok(format!("diagnostic: {}", first.message.lines().next().unwrap_or("")));
                        }
                    }
                    Event::Response { tag: RequestTag::Symbols, result } => {
                        let Expect::Symbol(name) = expect else { continue };
                        if result.to_string().contains(name) {
                            return Ok(format!("symbol {name} found"));
                        }
                        // Servers answer with nothing until they have parsed the file; ask again.
                        let empty = result.as_array().is_some_and(Vec::is_empty) || result.is_null();
                        if !empty {
                            return Err(format!("no symbol {name} in {result}"));
                        }
                        std::thread::sleep(Duration::from_millis(500));
                        manager.request(language, "textDocument/documentSymbol", json!({ "textDocument": { "uri": uri } }), RequestTag::Symbols);
                    }
                    Event::Failed { title, message, .. } => return Err(format!("{title}: {message}")),
                    other => seen.push(std::mem::discriminant(&other)),
                }
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        Err("timed out waiting for an answer".to_owned())
    }

    #[test]
    fn every_installed_server_answers_for_its_language() {
        let cases: Vec<(Language, &str, &str, Expect)> = vec![
            (Language::Css, "a.css", "a { color: ; }\n", Expect::Diagnostic),
            (Language::Json, "a.json", "{ \"a\": 1, }\n", Expect::Diagnostic),
            (Language::Yaml, "a.yaml", "a: [1, 2\nb: 3\n", Expect::Diagnostic),
            (Language::Python, "a.py", "x: int = \"text\"\n", Expect::Diagnostic),
            (Language::Dockerfile, "Dockerfile", "FROM\n", Expect::Diagnostic),
            (Language::Php, "a.php", "<?php\nfunction (;\n", Expect::Diagnostic),
            (Language::Markdown, "a.md", "# Title\n\ntext\n", Expect::Symbol("Title")),
            (Language::Shell, "a.sh", "foo() { echo hi; }\nfoo\n", Expect::Symbol("foo")),
            (Language::Html, "a.html", "<html><body><p>hi</p></body></html>\n", Expect::Symbol("body")),
        ];
        let mut failures = Vec::new();
        for (language, file, text, expect) in &cases {
            let spec = servers::spec_for(*language, &BTreeMap::new()).expect("a server is configured");
            if !servers::is_installed(&spec) {
                eprintln!("skipped {}: not installed", spec.title);
                continue;
            }
            match exercise(*language, file, text, expect) {
                Ok(what) => eprintln!("ok      {}: {what}", spec.title),
                Err(why) => {
                    eprintln!("FAILED  {}: {why}", spec.title);
                    failures.push(format!("{}: {why}", spec.title));
                }
            }
        }
        assert!(failures.is_empty(), "{failures:#?}");
    }
}
