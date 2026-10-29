//! The debugger: a session with a debug adapter (Python through debugpy), breakpoints, stepping,
//! the call stack, variables and the debug console. The protocol client is `forge_lsp::dap`.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::mpsc::Receiver;
use std::time::Instant;

use forge_core::commands::Command;
use forge_core::settings::config_dir;
use forge_lsp::dap::{codelldb_paths, find_python, js_debug_script, python_adapter, start_codelldb, start_js_debug, AdapterServer, DapClient, DapMessage};
use forge_lsp::find_executable;
use gpui::{div, prelude::*, px, relative, rems, rgb, rgba, AnyElement, Context, FontWeight};
use serde_json::{json, Value};

use crate::resize::{Edge, Resize};
use crate::workspace::{same_path, PanelView, Workspace};

const MAX_CONSOLE_LINES: usize = 2_000;

#[derive(Clone, Debug)]
pub(crate) struct Frame {
    pub id: i64,
    pub name: String,
    pub path: Option<PathBuf>,
    /// 0-based.
    pub line: usize,
}

#[derive(Clone, Debug)]
pub(crate) struct VarRow {
    pub name: String,
    pub value: String,
    pub depth: usize,
    /// Non-zero when the value has children to expand.
    pub reference: i64,
    pub expanded: bool,
}

type Response = Receiver<Result<Value, String>>;

enum Pending {
    Initialize,
    Launch,
    Breakpoints,
    ConfigurationDone,
    StackTrace,
    Scopes,
    /// The children of the row with this `variablesReference`.
    Variables(i64),
    Disconnect,
    Other,
}

/// What is being debugged, which decides the adapter's launch settings.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Target {
    Python,
    Node,
    Rust,
}

pub(crate) struct DebugSession {
    /// Connections to the adapter. Python has one; js-debug adds a child connection for the program.
    clients: Vec<DapClient>,
    /// The js-debug process the connections talk to; closed when the session ends.
    server: Option<AdapterServer>,
    target: Target,
    /// The connection that owns the running program: the child js-debug opened, else the only one.
    active: usize,
    /// How to start each child connection: the request name and its configuration.
    child_start: Vec<(usize, String, Value)>,
    pending: Vec<(usize, Pending, Response)>,
    program: PathBuf,
    cwd: PathBuf,
    interpreter: Option<PathBuf>,
    pub thread_id: Option<i64>,
    /// False while stopped at a breakpoint or after a step.
    pub running: bool,
    pub frames: Vec<Frame>,
    pub selected_frame: usize,
    pub vars: Vec<VarRow>,
    /// Console lines, `true` for error output.
    pub console: Vec<(bool, String)>,
    /// Where execution is stopped, for the editor to mark.
    pub stopped_at: Option<(PathBuf, usize)>,
    stopping: Option<Instant>,
    /// The last console line has not been ended by a newline yet.
    open_line: bool,
    /// The program runs in a terminal tab (its output and input are there, not in the console).
    uses_terminal: bool,
}

impl DebugSession {
    /// Send a request to the connection that owns the program.
    fn request(&mut self, kind: Pending, command: &str, arguments: Value) {
        let client = self.active;
        self.request_on(client, kind, command, arguments);
    }

    fn request_on(&mut self, client: usize, kind: Pending, command: &str, arguments: Value) {
        match self.clients[client].request(command, arguments) {
            Ok(rx) => self.pending.push((client, kind, rx)),
            Err(error) => self.log(true, format!("Could not send {command}: {error}")),
        }
    }

    fn new(clients: Vec<DapClient>, server: Option<AdapterServer>, target: Target, program: PathBuf, cwd: PathBuf, interpreter: Option<PathBuf>) -> Self {
        Self {
            clients,
            server,
            target,
            active: 0,
            child_start: Vec::new(),
            pending: Vec::new(),
            program,
            cwd,
            interpreter,
            thread_id: None,
            running: true,
            frames: Vec::new(),
            selected_frame: 0,
            vars: Vec::new(),
            console: Vec::new(),
            stopped_at: None,
            stopping: None,
            open_line: false,
            uses_terminal: false,
        }
    }

    fn initialize_arguments(&self) -> Value {
        let adapter = match self.target {
            Target::Python => "python",
            Target::Node => "pwa-node",
            Target::Rust => "lldb",
        };
        json!({ "clientID": "forge", "clientName": "Forge", "adapterID": adapter, "linesStartAt1": true, "columnsStartAt1": true, "pathFormat": "path", "supportsVariableType": true, "supportsRunInTerminalRequest": self.target == Target::Rust, "supportsStartDebuggingRequest": true })
    }

    /// The `launch` request's arguments for the main connection.
    fn launch_arguments(&self) -> Value {
        match self.target {
            Target::Python => json!({ "type": "python", "request": "launch", "name": "Forge", "program": self.program, "cwd": self.cwd, "python": self.interpreter, "console": "internalConsole", "justMyCode": true, "stopOnEntry": false }),
            Target::Rust => json!({ "type": "lldb", "request": "launch", "name": "Forge", "program": self.program, "cwd": self.cwd, "args": [], "stopOnEntry": false, "sourceLanguages": ["rust"], "terminal": "integrated" }),
            Target::Node => json!({ "type": "pwa-node", "request": "launch", "name": "Forge", "program": self.program, "cwd": self.cwd, "console": "internalConsole", "runtimeExecutable": self.interpreter, "skipFiles": ["<node_internals>/**"] }),
        }
    }

    fn exception_filters(&self) -> Value {
        if self.target == Target::Python { json!(["uncaught"]) } else { json!([]) }
    }

    /// Add program or adapter output. A chunk that does not end in a newline continues the same line.
    fn log(&mut self, error: bool, text: String) {
        let text = text.replace("\r\n", "\n");
        let ends_line = text.ends_with('\n');
        let mut pieces = text.trim_end_matches('\n').split('\n');
        if self.open_line {
            if let (Some(last), Some(first)) = (self.console.last_mut(), pieces.next()) {
                last.1.push_str(first);
            }
        }
        for line in pieces {
            self.console.push((error, line.to_owned()));
        }
        self.open_line = !ends_line && !text.is_empty();
        let excess = self.console.len().saturating_sub(MAX_CONSOLE_LINES);
        self.console.drain(..excess);
    }

    fn clear_stop(&mut self) {
        self.frames.clear();
        self.vars.clear();
        self.stopped_at = None;
        self.selected_frame = 0;
    }
}

fn frame_from(value: &Value) -> Frame {
    Frame {
        id: value["id"].as_i64().unwrap_or(0),
        name: value["name"].as_str().unwrap_or("?").to_owned(),
        path: value["source"]["path"].as_str().map(PathBuf::from),
        line: value["line"].as_u64().unwrap_or(1).saturating_sub(1) as usize,
    }
}

impl Workspace {
    // ---- breakpoints ----

    pub fn toggle_breakpoint(&mut self, path: PathBuf, line: usize) {
        let set = self.breakpoints.entry(path.clone()).or_default();
        if !set.remove(&line) {
            set.insert(line);
        }
        if set.is_empty() {
            self.breakpoints.remove(&path);
        }
        self.send_breakpoints(&path);
    }

    pub fn toggle_breakpoint_at_caret(&mut self) {
        let Some((view, doc)) = self.active_view().zip(self.active_doc()) else { return };
        let (line, _) = doc.buffer.offset_to_point(view.selections.primary().head);
        let path = doc.path.clone();
        self.toggle_breakpoint(path, line);
    }

    fn send_breakpoints(&mut self, path: &Path) {
        let lines: Vec<usize> = self.breakpoints.get(path).map(|s| s.iter().map(|l| l + 1).collect()).unwrap_or_default();
        if let Some(session) = self.debug.as_mut() {
            let breakpoints: Vec<Value> = lines.iter().map(|l| json!({ "line": l })).collect();
            for client in 0..session.clients.len() {
                session.request_on(client, Pending::Breakpoints, "setBreakpoints", json!({ "source": { "path": path }, "breakpoints": breakpoints }));
            }
        }
    }

    pub fn clear_all_breakpoints(&mut self) {
        let paths: Vec<PathBuf> = self.breakpoints.keys().cloned().collect();
        self.breakpoints.clear();
        for path in paths {
            self.send_breakpoints(&path);
        }
        self.set_status("All breakpoints removed");
    }

    /// Breakpoint lines (0-based) set in the file at `path`.
    pub fn breakpoint_lines(&self, path: &Path) -> BTreeSet<usize> {
        self.breakpoints.get(path).cloned().unwrap_or_default()
    }

    // ---- starting and controlling ----

    /// F5: start debugging the active file, or continue if a session is already running.
    pub fn start_debugging(&mut self, cx: &mut Context<Self>) {
        if self.debug.is_some() {
            self.debug_control(Command::DebugStart, cx);
            return;
        }
        let Some((g, vi)) = self.active_ix() else {
            self.set_status("Open a file to debug");
            return;
        };
        let doc_ix = self.groups[g].views[vi].doc;
        let path = self.docs[doc_ix].path.clone();
        let extension = path.extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
        if self.docs[doc_ix].dirty {
            self.write_doc(doc_ix);
        }
        match extension.as_str() {
            "py" => self.start_python(path),
            "js" | "mjs" | "cjs" => self.start_node(path),
            "rs" => self.start_rust(path),
            _ => self.set_status("There is no debugger for this file type yet. Python files can be debugged."),
        }
        if self.debug.is_some() {
            self.panel_visible = true;
            self.panel_view = PanelView::Debug;
        }
    }

    fn start_python(&mut self, program: PathBuf) {
        let root = self.root().to_path_buf();
        let start = program.parent().unwrap_or(&root).to_path_buf();
        let Some(python) = find_python(&start, &root) else {
            self.set_status("Python was not found. Install Python or create a .venv in the project.");
            return;
        };
        let adapter = match python_adapter(&python) {
            Ok(adapter) => adapter,
            Err(reason) => {
                self.set_status(reason);
                return;
            }
        };
        let client = match DapClient::spawn(&adapter.command, &adapter.args, Some(&root), &adapter.env) {
            Ok(client) => client,
            Err(error) => {
                self.set_status(format!("Could not start the debug adapter: {error:#}"));
                return;
            }
        };
        let mut session = DebugSession::new(vec![client], None, Target::Python, program.clone(), root, adapter.interpreter);
        session.log(false, format!("Debugging {} with {}", program.display(), python.display()));
        let init = session.initialize_arguments();
        session.request(Pending::Initialize, "initialize", init);
        self.debug = Some(session);
        self.set_status("Starting the debugger...");
    }

    /// Build the project with cargo on a background thread, then debug the program it produced.
    fn start_rust(&mut self, file: PathBuf) {
        if self.debug_build_rx.is_some() {
            self.set_status("A build is already running");
            return;
        }
        let Some(package) = forge_core::cargo_build::package_dir(&file) else {
            self.set_status("This file is not inside a Cargo project");
            return;
        };
        if codelldb_paths(&config_dir().join("servers")).is_none() {
            self.set_status("The Rust debugger is not installed. Run \"Debug: Install Rust Debugger (CodeLLDB)\".");
            return;
        }
        self.debug_console = vec![(false, format!("Building {} with cargo...", package.display()))];
        self.panel_visible = true;
        self.panel_view = PanelView::Debug;
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(forge_core::cargo_build::build(&file));
        });
        self.debug_build_rx = Some(rx);
        self.set_status("Building with cargo...");
    }

    /// Pick up a finished build and start the debugger on its program.
    fn poll_debug_build(&mut self) -> bool {
        let Some(rx) = &self.debug_build_rx else { return false };
        let outcome = match rx.try_recv() {
            Ok(outcome) => outcome,
            Err(std::sync::mpsc::TryRecvError::Empty) => return false,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => forge_core::cargo_build::BuildOutcome { executable: Err("the build stopped unexpectedly".to_owned()), log: String::new(), package_dir: PathBuf::new() },
        };
        self.debug_build_rx = None;
        for line in outcome.log.lines() {
            self.debug_console.push((line.contains("error") || line.contains("warning:"), line.to_owned()));
        }
        match outcome.executable {
            Err(reason) => {
                self.debug_console.push((true, format!("Build failed: {reason}")));
                self.set_status(format!("Build failed: {reason}"));
            }
            Ok(program) => self.start_rust_session(program, outcome.package_dir),
        }
        true
    }

    fn start_rust_session(&mut self, program: PathBuf, package_dir: PathBuf) {
        let Some((exe, liblldb)) = codelldb_paths(&config_dir().join("servers")) else {
            self.set_status("The Rust debugger is not installed");
            return;
        };
        let server = match start_codelldb(&exe, &liblldb) {
            Ok(server) => server,
            Err(error) => {
                self.set_status(format!("Could not start the Rust debugger: {error:#}"));
                return;
            }
        };
        let client = match DapClient::connect(server.port) {
            Ok(client) => client,
            Err(error) => {
                self.set_status(format!("Could not connect to the Rust debugger: {error:#}"));
                return;
            }
        };
        let carried = std::mem::take(&mut self.debug_console);
        let mut session = DebugSession::new(vec![client], Some(server), Target::Rust, program.clone(), package_dir, None);
        session.console = carried;
        session.log(false, format!("Debugging {}", program.display()));
        let init = session.initialize_arguments();
        session.request(Pending::Initialize, "initialize", init);
        self.debug = Some(session);
        self.set_status("Starting the debugger...");
    }

    /// Open the adapter's launcher in a new terminal tab and show it.
    fn run_in_terminal(&mut self, arguments: &Value) -> Result<(), String> {
        let args: Vec<String> = arguments["args"].as_array().map(|a| a.iter().filter_map(|v| v.as_str().map(str::to_owned)).collect()).unwrap_or_default();
        let (program, rest) = args.split_first().ok_or("the adapter sent no command to run")?;
        let cwd = arguments["cwd"].as_str().filter(|c| !c.is_empty()).map(PathBuf::from).unwrap_or_else(|| self.root().to_path_buf());
        let mut terminal = forge_core::terminal::Terminal::spawn(program, rest, &cwd, 12, 100).map_err(|e| format!("{e:#}"))?;
        terminal.keep_when_dead = true;
        // The previous run's terminal has served its purpose.
        self.terminals.retain(|t| !t.keep_when_dead);
        self.terminals.push(terminal);
        self.active_terminal = self.terminals.len() - 1;
        self.panel_visible = true;
        self.panel_view = PanelView::Terminal;
        Ok(())
    }

    fn start_node(&mut self, program: PathBuf) {
        let Some(node) = find_executable("node") else {
            self.set_status("Node.js was not found on PATH");
            return;
        };
        let Some(script) = js_debug_script(&config_dir().join("servers")) else {
            self.set_status("The JavaScript debugger is not installed. Run \"Debug: Install JavaScript Debugger\".");
            return;
        };
        let root = self.root().to_path_buf();
        let server = match start_js_debug(&node, &script) {
            Ok(server) => server,
            Err(error) => {
                self.set_status(format!("Could not start the JavaScript debugger: {error:#}"));
                return;
            }
        };
        let client = match DapClient::connect(server.port) {
            Ok(client) => client,
            Err(error) => {
                self.set_status(format!("Could not connect to the JavaScript debugger: {error:#}"));
                return;
            }
        };
        let mut session = DebugSession::new(vec![client], Some(server), Target::Node, program.clone(), root, Some(node.clone()));
        session.log(false, format!("Debugging {} with {}", program.display(), node.display()));
        let init = session.initialize_arguments();
        session.request(Pending::Initialize, "initialize", init);
        self.debug = Some(session);
        self.set_status("Starting the debugger...");
    }

    pub fn debug_control(&mut self, command: Command, cx: &mut Context<Self>) {
        let Some(session) = self.debug.as_mut() else {
            if command == Command::DebugStart {
                self.start_debugging(cx);
            } else {
                self.set_status("There is no debug session. Press F5 to start one.");
            }
            return;
        };
        if command == Command::DebugStop {
            session.stopping = Some(Instant::now());
            for client in 0..session.clients.len() {
                session.request_on(client, Pending::Disconnect, "disconnect", json!({ "terminateDebuggee": true }));
            }
            return;
        }
        let (Some(thread), false) = (session.thread_id, session.running) else {
            self.set_status("The program is running: it can be stopped or paused at a breakpoint");
            return;
        };
        let name = match command {
            Command::DebugStart => "continue",
            Command::DebugStepOver => "next",
            Command::DebugStepInto => "stepIn",
            Command::DebugStepOut => "stepOut",
            _ => return,
        };
        session.running = true;
        session.clear_stop();
        session.request(Pending::Other, name, json!({ "threadId": thread }));
        // Running freely, the program prints: show the terminal it runs in.
        if command == Command::DebugStart && session.uses_terminal {
            self.panel_view = PanelView::Terminal;
        }
    }

    pub fn debug_select_frame(&mut self, index: usize, cx: &mut Context<Self>) {
        let Some(session) = self.debug.as_mut() else { return };
        let Some(frame) = session.frames.get(index).cloned() else { return };
        session.selected_frame = index;
        session.vars.clear();
        session.request(Pending::Scopes, "scopes", json!({ "frameId": frame.id }));
        self.show_frame(&frame, cx);
    }

    fn show_frame(&mut self, frame: &Frame, cx: &mut Context<Self>) {
        let Some(path) = frame.path.clone().filter(|p| p.is_file()) else { return };
        if let Some(session) = self.debug.as_mut() {
            session.stopped_at = Some((path.clone(), frame.line));
        }
        self.open_file(&path, Some((frame.line, 0)), cx);
    }

    /// Expand or collapse a variable row.
    pub fn debug_toggle_variable(&mut self, index: usize) {
        let Some(session) = self.debug.as_mut() else { return };
        let Some(row) = session.vars.get(index).cloned() else { return };
        if row.reference == 0 {
            return;
        }
        if row.expanded {
            let end = session.vars[index + 1..].iter().position(|r| r.depth <= row.depth).map_or(session.vars.len(), |p| index + 1 + p);
            session.vars.drain(index + 1..end);
            session.vars[index].expanded = false;
        } else {
            session.request(Pending::Variables(row.reference), "variables", json!({ "variablesReference": row.reference }));
        }
    }

    // ---- reacting to the adapter ----

    /// Handle what the adapter sent. Returns true if anything changed.
    pub fn debug_tick(&mut self, cx: &mut Context<Self>) -> bool {
        let mut changed = self.poll_debug_build();
        let Some(mut session) = self.debug.take() else { return changed };
        let mut ended = false;

        for ci in 0..session.clients.len() {
            for message in session.clients[ci].poll() {
                changed = true;
                match message {
                    DapMessage::Event { event, body } => match event.as_str() {
                        "initialized" => {
                            let files: Vec<PathBuf> = self.breakpoints.keys().cloned().collect();
                            for path in files {
                                let lines: Vec<Value> = self.breakpoints[&path].iter().map(|l| json!({ "line": l + 1 })).collect();
                                session.request_on(ci, Pending::Breakpoints, "setBreakpoints", json!({ "source": { "path": path }, "breakpoints": lines }));
                            }
                            let filters = session.exception_filters();
                            session.request_on(ci, Pending::Other, "setExceptionBreakpoints", json!({ "filters": filters }));
                            session.request_on(ci, Pending::ConfigurationDone, "configurationDone", json!({}));
                        }
                        "stopped" => {
                            session.running = false;
                            session.active = ci;
                            session.thread_id = body["threadId"].as_i64().or(session.thread_id);
                            if let Some(thread) = session.thread_id {
                                session.request_on(ci, Pending::StackTrace, "stackTrace", json!({ "threadId": thread, "startFrame": 0, "levels": 40 }));
                            }
                            let reason = body["reason"].as_str().unwrap_or("paused").to_owned();
                            self.set_status(format!("Paused ({reason})"));
                            // Back from the program's terminal to the variables and call stack.
                            self.panel_view = PanelView::Debug;
                        }
                        "continued" => {
                            session.running = true;
                            session.clear_stop();
                        }
                        "output" => {
                            let category = body["category"].as_str().unwrap_or("console");
                            if category != "telemetry" {
                                if let Some(text) = body["output"].as_str() {
                                    session.log(category == "stderr", text.to_owned());
                                }
                            }
                        }
                        "terminated" => {
                            // js-debug reports the program's end on both connections; the main one decides.
                            if ci == 0 {
                                session.log(false, "The program finished".to_owned());
                                if session.stopping.is_none() {
                                    session.stopping = Some(Instant::now());
                                    for client in 0..session.clients.len() {
                                        session.request_on(client, Pending::Disconnect, "disconnect", json!({}));
                                    }
                                }
                            }
                        }
                        "exited" => {
                            let code = body["exitCode"].as_i64().unwrap_or(0);
                            session.log(code != 0, format!("Exited with code {code}"));
                        }
                        _ => {}
                    },
                    DapMessage::Request { seq, command, arguments } if command == "startDebugging" && session.server.is_some() => {
                        // js-debug asks for a second connection for the program it started.
                        let _ = session.clients[ci].respond(seq, &command, Ok(json!({})));
                        let port = session.server.as_ref().map(|s| s.port).unwrap_or(0);
                        match DapClient::connect(port) {
                            Ok(child) => {
                                session.clients.push(child);
                                let index = session.clients.len() - 1;
                                let how = arguments["request"].as_str().unwrap_or("attach").to_owned();
                                session.child_start.push((index, how, arguments["configuration"].clone()));
                                session.active = index;
                                let init = session.initialize_arguments();
                                session.request_on(index, Pending::Initialize, "initialize", init);
                            }
                            Err(error) => session.log(true, format!("Could not open the program's debug session: {error:#}")),
                        }
                    }
                    DapMessage::Request { seq, command, arguments } if command == "runInTerminal" => {
                        // The adapter wants the program to run in a terminal, so its input and output work.
                        let result = self.run_in_terminal(&arguments).map(|()| json!({}));
                        session.uses_terminal |= result.is_ok();
                        let _ = session.clients[ci].respond(seq, &command, result);
                    }
                    DapMessage::Request { seq, command, .. } => {
                        let _ = session.clients[ci].respond(seq, &command, Err(format!("Forge does not support {command}")));
                    }
                    DapMessage::Closed => {
                        if ci == 0 {
                            ended = true;
                        }
                    }
                }
            }
        }

        let waiting = std::mem::take(&mut session.pending);
        for (ci, kind, rx) in waiting {
            match rx.try_recv() {
                Err(std::sync::mpsc::TryRecvError::Empty) => session.pending.push((ci, kind, rx)),
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {}
                Ok(result) => {
                    changed = true;
                    self.handle_debug_response(&mut session, ci, kind, result, cx);
                }
            }
        }

        if session.stopping.is_some_and(|since| since.elapsed().as_secs() >= 3) || !session.clients[0].is_alive() {
            ended = true;
        }
        if ended {
            for client in &mut session.clients {
                client.kill();
            }
            self.set_status("Debug session ended");
            self.debug_console = std::mem::take(&mut session.console);
            self.debug = None;
            return true;
        }
        self.debug = Some(session);
        changed
    }

    fn handle_debug_response(&mut self, session: &mut DebugSession, ci: usize, kind: Pending, result: Result<Value, String>, cx: &mut Context<Self>) {
        match (kind, result) {
            (Pending::Initialize, Ok(_)) => {
                if let Some((_, how, config)) = session.child_start.iter().find(|(i, ..)| *i == ci).cloned() {
                    session.request_on(ci, Pending::Launch, &how, config);
                } else {
                    let launch = session.launch_arguments();
                    session.request_on(ci, Pending::Launch, "launch", launch);
                }
            }
            (Pending::Launch, Ok(_)) => self.set_status("Running under the debugger"),
            (Pending::StackTrace, Ok(body)) => {
                // Frames inside Node itself ("deemphasize") are noise when stepping through your own code.
                session.frames = body["stackFrames"].as_array().map(|f| f.iter().filter(|v| v["presentationHint"] != "deemphasize").map(frame_from).collect()).unwrap_or_default();
                session.selected_frame = 0;
                if let Some(frame) = session.frames.first().cloned() {
                    session.request_on(ci, Pending::Scopes, "scopes", json!({ "frameId": frame.id }));
                    if let Some(path) = frame.path.clone().filter(|p| p.is_file()) {
                        session.stopped_at = Some((path.clone(), frame.line));
                        self.open_file(&path, Some((frame.line, 0)), cx);
                    }
                }
            }
            (Pending::Scopes, Ok(body)) => {
                session.vars = body["scopes"]
                    .as_array()
                    .map(|scopes| {
                        scopes
                            .iter()
                            .map(|s| VarRow { name: s["name"].as_str().unwrap_or("").to_owned(), value: String::new(), depth: 0, reference: s["variablesReference"].as_i64().unwrap_or(0), expanded: false })
                            .collect()
                    })
                    .unwrap_or_default();
                // The first scope (Locals) is the one wanted most: open it.
                if let Some(first) = session.vars.first().map(|r| r.reference).filter(|r| *r != 0) {
                    session.request_on(ci, Pending::Variables(first), "variables", json!({ "variablesReference": first }));
                }
            }
            (Pending::Variables(parent), Ok(body)) => {
                let Some(at) = session.vars.iter().position(|r| r.reference == parent) else { return };
                let depth = session.vars[at].depth + 1;
                session.vars[at].expanded = true;
                let children: Vec<VarRow> = body["variables"]
                    .as_array()
                    .map(|vars| {
                        vars.iter()
                            .map(|v| VarRow {
                                name: v["name"].as_str().unwrap_or("").to_owned(),
                                value: v["value"].as_str().unwrap_or("").to_owned(),
                                depth,
                                reference: v["variablesReference"].as_i64().unwrap_or(0),
                                expanded: false,
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                let tail = session.vars.split_off(at + 1);
                session.vars.extend(children);
                session.vars.extend(tail);
            }
            (Pending::Breakpoints | Pending::ConfigurationDone | Pending::Disconnect | Pending::Other, Ok(_)) => {}
            (Pending::Launch | Pending::Initialize, Err(error)) => {
                session.log(true, format!("The debugger could not start: {error}"));
                self.set_status(format!("The debugger could not start: {error}"));
                session.stopping = Some(Instant::now() - std::time::Duration::from_secs(3));
            }
            (_, Err(error)) => session.log(true, error),
        }
    }

    // ---- drawing ----

    /// The Debug panel: variables, call stack and console side by side.
    pub fn render_debug_panel(&self, cx: &mut Context<Self>) -> AnyElement {
        let t = &self.theme;
        let Some(session) = self.debug.as_ref() else {
            if self.debug_console.is_empty() {
                return div().p_3().text_color(rgb(t.dim)).child("No debug session. Open a Python file and press F5. F9 sets a breakpoint, or click the margin left of a line number.").into_any_element();
            }
            // The last session is over, but what it printed is still worth reading.
            let lines = self.debug_console.iter().enumerate().map(|(i, (error, text))| div().id(("debug-last", i)).px_2().whitespace_nowrap().text_color(rgb(if *error { t.error } else { t.text })).child(text.clone()));
            return div()
                .flex()
                .flex_col()
                .size_full()
                .child(div().px_2().h(rems(1.75)).flex().items_center().text_xs().font_weight(FontWeight::BOLD).text_color(rgb(t.dim)).child(if self.debug_build_rx.is_some() { "DEBUG CONSOLE" } else { "DEBUG CONSOLE (LAST SESSION)" }))
                .child(div().id("debug-last-console").flex_1().min_h_0().overflow_y_scroll().text_sm().children(lines))
                .into_any_element();
        };
        let column = |index: usize, title: &'static str, cx: &mut Context<Self>| {
            let base = div().relative().flex().flex_col().min_w_0().h_full().border_r_1().border_color(rgb(t.border));
            let sized = if index < 2 { base.flex_none().w(relative(self.debug_cols[index])) } else { base.flex_1() };
            sized
                .when(index < 2, |d| d.child(self.resize_handle(("resize-debug", index), Resize::Debug(index), Edge::Right, cx)))
                .child(div().px_2().h(rems(1.75)).flex().items_center().text_xs().font_weight(FontWeight::BOLD).text_color(rgb(t.dim)).child(title))
        };

        let variables = session.vars.iter().enumerate().map(|(i, v)| {
            let expandable = v.reference != 0;
            div()
                .id(("debug-var", i))
                .flex()
                .flex_row()
                .gap(rems(0.375))
                .items_center()
                .h(rems(1.5))
                .pl(rems(0.5 + v.depth as f32 * 0.875))
                .pr_2()
                .whitespace_nowrap()
                .overflow_hidden()
                .when(expandable, |d| d.cursor_pointer().hover(|s| s.bg(rgb(t.tab_active))))
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.debug_toggle_variable(i);
                    cx.notify();
                }))
                .child(div().w(rems(1.0)).flex_none().children(expandable.then(|| crate::icons::glyph(if v.expanded { crate::codicons::CHEVRON_DOWN } else { crate::codicons::CHEVRON_RIGHT }, 0.875, t.dim))))
                .child(div().text_color(rgb(t.info)).child(v.name.clone()))
                .child(div().text_color(rgb(t.dim)).child(if v.value.is_empty() { String::new() } else { format!("= {}", v.value) }))
        })
        .collect::<Vec<_>>();
        let frames = session.frames.iter().enumerate().map(|(i, f)| {
            let place = f.path.as_ref().and_then(|p| p.file_name()).map(|n| format!("{}:{}", n.to_string_lossy(), f.line + 1)).unwrap_or_default();
            div()
                .id(("debug-frame", i))
                .flex()
                .flex_row()
                .justify_between()
                .gap_2()
                .items_center()
                .h(rems(1.5))
                .px_2()
                .whitespace_nowrap()
                .overflow_hidden()
                .cursor_pointer()
                .when(session.selected_frame == i, |d| d.bg(rgb(t.selection)))
                .hover(|s| s.bg(rgb(t.tab_active)))
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.debug_select_frame(i, cx);
                    cx.notify();
                }))
                .child(f.name.clone())
                .child(div().text_xs().text_color(rgb(t.dim)).child(place))
        })
        .collect::<Vec<_>>();
        let console = session.console.iter().enumerate().map(|(i, (error, text))| div().id(("debug-line", i)).px_2().whitespace_nowrap().text_color(rgb(if *error { t.error } else { t.text })).child(text.clone()));

        let status = if session.running { "Running" } else { "Paused" };
        div()
            .flex()
            .flex_row()
            .size_full()
            .child(column(0, "VARIABLES", cx).child(div().id("debug-vars").flex_1().min_h_0().overflow_y_scroll().children(variables)))
            .child(column(1, "CALL STACK", cx).child(div().px_2().pb_1().text_xs().text_color(rgb(t.dim)).child(status)).child(div().id("debug-frames").flex_1().min_h_0().overflow_y_scroll().children(frames)))
            .child(column(2, "DEBUG CONSOLE", cx).child(div().id("debug-console").flex_1().min_h_0().overflow_y_scroll().text_sm().children(console)))
            .into_any_element()
    }

    /// The floating bar of stepping controls shown while a session is running.
    pub fn render_debug_toolbar(&self, window_width: f32, cx: &mut Context<Self>) -> Option<AnyElement> {
        let session = self.debug.as_ref()?;
        let t = &self.theme;
        let stopped = !session.running;
        let button = |id: &'static str, glyph: char, name: &'static str, command: Command, enabled: bool, color: u32, cx: &mut Context<Self>| {
            div()
                .id(id)
                .w(rems(2.0))
                .h(rems(2.0))
                .flex()
                .items_center()
                .justify_center()
                .rounded_sm()
                .tooltip(self.tooltip(name, Some(command)))
                .when(enabled, |d| d.cursor_pointer().hover(|s| s.bg(rgb(t.border))))
                .on_click(cx.listener(move |this, _, _, cx| {
                    if enabled {
                        this.run_command(command, false, cx);
                    }
                }))
                .child(crate::icons::glyph(glyph, 1.25, if enabled { color } else { t.dim }))
        };
        let left = self.sidebar_width() + ((window_width - self.sidebar_width()) / 2.0 - 112.0).max(0.0);
        Some(
            div()
                .absolute()
                .top(rems(3.0))
                .left(px(left))
                .flex()
                .flex_row()
                .p_1()
                .gap_1()
                .bg(rgb(t.popup))
                .border_1()
                .border_color(rgb(t.border))
                .rounded_md()
                .shadow_lg()
                .occlude()
                .child(button("debug-continue", crate::codicons::DEBUG_CONTINUE, "Continue", Command::DebugStart, stopped, t.git_added, cx))
                .child(button("debug-over", crate::codicons::DEBUG_STEP_OVER, "Step Over", Command::DebugStepOver, stopped, t.info, cx))
                .child(button("debug-into", crate::codicons::DEBUG_STEP_INTO, "Step Into", Command::DebugStepInto, stopped, t.info, cx))
                .child(button("debug-out", crate::codicons::DEBUG_STEP_OUT, "Step Out", Command::DebugStepOut, stopped, t.info, cx))
                .child(button("debug-stop", crate::codicons::DEBUG_STOP, "Stop", Command::DebugStop, true, t.error, cx))
                .into_any_element(),
        )
    }

    /// The editor marks for `path`: breakpoint lines and the line execution is stopped on.
    pub fn debug_marks(&self, path: &Path) -> (BTreeSet<usize>, Option<usize>) {
        let stopped = self.debug.as_ref().and_then(|s| s.stopped_at.as_ref()).filter(|(p, _)| same_path(p, path)).map(|(_, line)| *line);
        (self.breakpoint_lines(path), stopped)
    }
}

/// `rgba` for a translucent highlight of a theme colour.
pub(crate) fn tint(color: u32, alpha: u8) -> gpui::Rgba {
    rgba((color << 8) | u32::from(alpha))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session() -> DebugSession {
        let holder = DapClient::spawn(std::path::Path::new(if cfg!(windows) { "cmd.exe" } else { "cat" }), &[], None, &[]).expect("a process to hold");
        DebugSession::new(vec![holder], None, Target::Python, PathBuf::new(), PathBuf::new(), None)
    }

    #[test]
    fn console_chunks_without_a_newline_continue_the_line() {
        let mut s = session();
        s.log(false, "done".to_owned());
        s.log(false, " 5\n".to_owned());
        s.log(true, "oops\r\nsecond\n".to_owned());
        s.log(false, "\n".to_owned());
        let lines: Vec<&str> = s.console.iter().map(|(_, l)| l.as_str()).collect();
        assert_eq!(lines, vec!["done 5", "oops", "second", ""]);
        assert!(s.console[1].0 && !s.console[0].0, "error output keeps its flag");
    }
}
