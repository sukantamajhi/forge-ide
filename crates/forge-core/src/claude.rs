//! Claude Code inside the editor: a chat that drives the `claude` command-line program, like the VS Code
//! extension does. The program runs headless (`claude -p --output-format stream-json`) in the project folder
//! with the user's own sign-in, and this module turns its JSON lines into a conversation to draw.
//!
//! Nothing here touches the interface: [`parse_line`] reads one line of output, [`Conversation::apply`] folds the
//! events into chat entries, [`find_claude`] locates the program, and [`start`] runs one turn of the chat.

use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::sync::{Arc, Mutex};

use serde_json::Value;

/// What Claude may do without asking. Anything the mode does not cover is put to the user in the chat.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PermissionMode {
    /// Read and search freely; ask before editing files or running commands.
    #[default]
    Ask,
    /// Edit files without asking (commands are still asked about).
    AcceptEdits,
    /// Look and propose a plan; change nothing until the plan is approved.
    Plan,
}

impl PermissionMode {
    pub const ALL: [PermissionMode; 3] = [PermissionMode::Ask, PermissionMode::AcceptEdits, PermissionMode::Plan];

    /// The value of `--permission-mode`, and of `claude.permissionMode` in settings.json.
    pub fn id(self) -> &'static str {
        match self {
            PermissionMode::Ask => "default",
            PermissionMode::AcceptEdits => "acceptEdits",
            PermissionMode::Plan => "plan",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            PermissionMode::Ask => "Ask before edits",
            PermissionMode::AcceptEdits => "Edit automatically",
            PermissionMode::Plan => "Plan mode",
        }
    }

    pub fn from_id(id: &str) -> Option<PermissionMode> {
        let id = id.trim();
        PermissionMode::ALL.into_iter().find(|m| m.id().eq_ignore_ascii_case(id))
    }

    /// The next mode in the list, for the button that cycles through them.
    pub fn next(self) -> PermissionMode {
        let all = PermissionMode::ALL;
        all[(all.iter().position(|m| *m == self).unwrap_or(0) + 1) % all.len()]
    }
}

/// The models offered in the chat: the value of `--model` (an alias for the latest model of that kind) and its name.
/// An empty value leaves the choice to Claude Code.
pub const MODELS: &[(&str, &str)] = &[("", "Default"), ("opus", "Opus"), ("sonnet", "Sonnet"), ("haiku", "Haiku"), ("fable", "Fable")];

/// How hard Claude thinks, the values of `--effort`: more effort is slower and costs more. Empty is Claude Code's own choice.
pub const EFFORTS: &[(&str, &str)] = &[("", "Default"), ("low", "Low"), ("medium", "Medium"), ("high", "High"), ("xhigh", "Extra high"), ("max", "Max")];

/// The name to show for a `--model` value: the alias's name, or the value itself for a full model name.
pub fn model_label(value: &str) -> String {
    MODELS.iter().find(|(v, _)| v.eq_ignore_ascii_case(value.trim())).map_or_else(|| value.trim().to_owned(), |(_, l)| (*l).to_owned())
}

/// The name to show for an `--effort` value.
pub fn effort_label(value: &str) -> String {
    EFFORTS.iter().find(|(v, _)| v.eq_ignore_ascii_case(value.trim())).map_or_else(|| value.trim().to_owned(), |(_, l)| (*l).to_owned())
}

/// Whether `value` is one of the effort levels `--effort` takes (or empty).
pub fn is_effort(value: &str) -> bool {
    EFFORTS.iter().any(|(v, _)| v.eq_ignore_ascii_case(value.trim()))
}

// ---- events ----

/// One thing the program reported.
#[derive(Clone, Debug, PartialEq)]
pub enum Event {
    /// The session started: its id (to resume it) and the model in use.
    Init { session_id: String, model: String },
    /// A piece of the answer, as it is written.
    TextDelta(String),
    /// A whole block of the answer, once it is complete.
    Text(String),
    /// A piece of Claude's thinking, as it is written (a summary: see [`Options::show_thinking`]).
    ThinkingDelta(String),
    /// A whole block of thinking, once it is complete.
    Thinking(String),
    /// Claude is using a tool.
    ToolUse { id: String, name: String, input: Value },
    /// What the tool returned.
    ToolResult { id: String, text: String, is_error: bool },
    /// Claude wants to use a tool and waits for a yes or no.
    Permission(PermissionRequest),
    /// The question with this request id is moot (the tool call was abandoned).
    PermissionCancelled(String),
    /// A request from the program this chat does not handle (it is answered with an error).
    Unsupported { request_id: String },
    /// A line the user typed, when a saved conversation is replayed.
    UserText(String),
    /// The turn ended.
    Done { error: Option<String>, session_id: String, cost_usd: Option<f64>, duration_ms: Option<u64> },
}

/// A question the program puts to the user: may it use this tool, like this?
#[derive(Clone, Debug, PartialEq)]
pub struct PermissionRequest {
    /// What the answer is addressed to.
    pub id: String,
    pub tool_name: String,
    pub tool_use_id: String,
    /// The program's own few words on it (a file name, a command).
    pub description: String,
    pub input: Value,
    /// Rule changes the program offers with an "allow" (for instance: accept all edits this session).
    pub suggestions: Vec<Value>,
}

/// The answer to a [`PermissionRequest`].
#[derive(Clone, Debug, PartialEq)]
pub enum Decision {
    /// Go ahead, with `input` (the tool's input, possibly changed) and any rule changes to keep.
    Allow { input: Value, permissions: Vec<Value> },
    /// Do not; `message` is what Claude is told.
    Deny { message: String },
}

/// The line that answers a request, in the program's control protocol.
pub fn control_response(request_id: &str, decision: &Decision) -> String {
    let response = match decision {
        Decision::Allow { input, permissions } => {
            let mut r = serde_json::json!({ "behavior": "allow", "updatedInput": input });
            if !permissions.is_empty() {
                r["updatedPermissions"] = Value::Array(permissions.clone());
            }
            r
        }
        Decision::Deny { message } => serde_json::json!({ "behavior": "deny", "message": message }),
    };
    serde_json::json!({ "type": "control_response", "response": { "subtype": "success", "request_id": request_id, "response": response } }).to_string()
}

/// The line that refuses a request this chat does not understand.
pub fn control_error(request_id: &str, error: &str) -> String {
    serde_json::json!({ "type": "control_response", "response": { "subtype": "error", "request_id": request_id, "error": error } }).to_string()
}

/// The message that starts a turn, in the program's streaming input format.
pub fn user_message(prompt: &str) -> String {
    serde_json::json!({ "type": "user", "message": { "role": "user", "content": prompt } }).to_string()
}

/// The events in one line of the program's output (none for lines Forge has no use for).
pub fn parse_line(line: &str) -> Vec<Event> {
    let Ok(v) = serde_json::from_str::<Value>(line.trim()) else { return Vec::new() };
    let str_of = |v: &Value, key: &str| v.get(key).and_then(Value::as_str).unwrap_or_default().to_owned();
    match v.get("type").and_then(Value::as_str) {
        Some("system") if v.get("subtype").and_then(Value::as_str) == Some("init") => {
            vec![Event::Init { session_id: str_of(&v, "session_id"), model: str_of(&v, "model") }]
        }
        Some("stream_event") => {
            let event = &v["event"];
            let delta = &event["delta"];
            if event["type"] == "content_block_delta" && delta["type"] == "text_delta" {
                if let Some(text) = delta["text"].as_str() {
                    return vec![Event::TextDelta(text.to_owned())];
                }
            }
            // Without a summary requested the thinking arrives empty, with only a signature: nothing to show.
            if event["type"] == "content_block_delta" && delta["type"] == "thinking_delta" {
                if let Some(text) = delta["thinking"].as_str().filter(|t| !t.is_empty()) {
                    return vec![Event::ThinkingDelta(text.to_owned())];
                }
            }
            Vec::new()
        }
        Some("assistant") => {
            let blocks = v["message"]["content"].as_array().cloned().unwrap_or_default();
            blocks
                .iter()
                .filter_map(|b| match b["type"].as_str()? {
                    "text" => Some(Event::Text(str_of(b, "text"))),
                    "thinking" if !str_of(b, "thinking").is_empty() => Some(Event::Thinking(str_of(b, "thinking"))),
                    "tool_use" => Some(Event::ToolUse { id: str_of(b, "id"), name: str_of(b, "name"), input: b["input"].clone() }),
                    _ => None,
                })
                .collect()
        }
        Some("user") => {
            let blocks = v["message"]["content"].as_array().cloned().unwrap_or_default();
            blocks
                .iter()
                .filter(|b| b["type"] == "tool_result")
                .map(|b| Event::ToolResult { id: str_of(b, "tool_use_id"), text: tool_result_text(&b["content"]), is_error: b["is_error"].as_bool().unwrap_or(false) })
                .collect()
        }
        Some("control_request") => {
            let request_id = str_of(&v, "request_id");
            let r = &v["request"];
            if r["subtype"] != "can_use_tool" {
                return vec![Event::Unsupported { request_id }];
            }
            vec![Event::Permission(PermissionRequest {
                id: request_id,
                tool_name: str_of(r, "tool_name"),
                tool_use_id: str_of(r, "tool_use_id"),
                description: str_of(r, "description"),
                input: r["input"].clone(),
                suggestions: r["permission_suggestions"].as_array().cloned().unwrap_or_default(),
            })]
        }
        Some("control_cancel_request") => vec![Event::PermissionCancelled(str_of(&v, "request_id"))],
        Some("result") => {
            let is_error = v["is_error"].as_bool().unwrap_or(false) || v.get("subtype").and_then(Value::as_str).is_some_and(|s| s != "success");
            let error = is_error.then(|| {
                let text = str_of(&v, "result");
                if text.is_empty() { format!("Claude stopped: {}", str_of(&v, "subtype")) } else { text }
            });
            vec![Event::Done { error, session_id: str_of(&v, "session_id"), cost_usd: v["total_cost_usd"].as_f64(), duration_ms: v["duration_ms"].as_u64() }]
        }
        _ => Vec::new(),
    }
}

/// The events in one line of a saved conversation (`~/.claude/projects/...jsonl`): what [`parse_line`] reads, plus what
/// the user typed. Lines of side chats and of the program's own bookkeeping are left out.
pub fn parse_transcript_line(line: &str) -> Vec<Event> {
    let Ok(v) = serde_json::from_str::<Value>(line.trim()) else { return Vec::new() };
    if v["isSidechain"].as_bool().unwrap_or(false) || v["isMeta"].as_bool().unwrap_or(false) {
        return Vec::new();
    }
    if v["type"] == "user" {
        let content = &v["message"]["content"];
        let typed = match content {
            Value::String(s) => Some(s.clone()),
            Value::Array(blocks) if blocks.iter().all(|b| b["type"] == "text") && !blocks.is_empty() => Some(blocks.iter().filter_map(|b| b["text"].as_str()).collect::<Vec<_>>().join("\n")),
            _ => None,
        };
        return match typed {
            // Slash-command and hook bookkeeping is wrapped in tags and was never typed.
            Some(text) if !text.trim_start().starts_with("<command-") && !text.trim_start().starts_with("<local-command") && !text.trim().is_empty() => vec![Event::UserText(text)],
            Some(_) => Vec::new(),
            None => parse_line(line),
        };
    }
    // A saved turn has no partial text; only the finished blocks.
    parse_line(line).into_iter().filter(|e| matches!(e, Event::Text(_) | Event::Thinking(_) | Event::ToolUse { .. } | Event::ToolResult { .. })).collect()
}

/// A tool result is a string, or a list of `{type: text, text}` blocks.
fn tool_result_text(content: &Value) -> String {
    match content {
        Value::String(s) => s.clone(),
        Value::Array(blocks) => blocks.iter().filter_map(|b| b["text"].as_str()).collect::<Vec<_>>().join("\n"),
        _ => String::new(),
    }
}

// ---- the conversation ----

/// One row of the chat.
#[derive(Clone, Debug, PartialEq)]
pub enum Entry {
    User(String),
    /// Claude's words. `streaming` while they are still being written.
    Assistant { text: String, streaming: bool },
    /// Claude's thinking, as a summary. `streaming` while it is still being written.
    Thinking { text: String, streaming: bool },
    /// A tool Claude used: what it did in a few words, the file it touched, and what came back.
    Tool { id: String, name: String, summary: String, path: Option<String>, input: Value, result: Option<String>, failed: bool },
    /// Something the editor says (a refusal, an error).
    Notice(String),
}

#[derive(Clone, Debug, Default)]
pub struct Conversation {
    pub entries: Vec<Entry>,
    /// Resumes this chat on the next question.
    pub session_id: Option<String>,
    pub model: Option<String>,
    /// A turn is running.
    pub busy: bool,
    pub cost_usd: f64,
    /// Questions waiting for the user, oldest first.
    pub pending: Vec<PermissionRequest>,
}

impl Conversation {
    pub fn clear(&mut self) {
        *self = Conversation::default();
    }

    pub fn ask(&mut self, text: &str) {
        self.entries.push(Entry::User(text.to_owned()));
        self.busy = true;
    }

    pub fn notice(&mut self, text: impl Into<String>) {
        self.entries.push(Entry::Notice(text.into()));
    }

    /// Fold one event into the chat.
    pub fn apply(&mut self, event: Event) {
        // Anything but more thinking means the thinking that was being streamed is complete.
        if !matches!(event, Event::ThinkingDelta(_) | Event::Thinking(_)) {
            if let Some(Entry::Thinking { streaming, .. }) = self.entries.last_mut() {
                *streaming = false;
            }
        }
        match event {
            Event::Init { session_id, model } => {
                if !session_id.is_empty() {
                    self.session_id = Some(session_id);
                }
                if !model.is_empty() {
                    self.model = Some(model);
                }
            }
            Event::ThinkingDelta(piece) => match self.entries.last_mut() {
                Some(Entry::Thinking { text, streaming: true }) => text.push_str(&piece),
                _ => self.entries.push(Entry::Thinking { text: piece, streaming: true }),
            },
            Event::Thinking(full) => match self.entries.last_mut() {
                // The finished block replaces what was streamed of it.
                Some(Entry::Thinking { text, streaming }) if *streaming => {
                    *text = full;
                    *streaming = false;
                }
                _ if full.trim().is_empty() => {}
                _ => self.entries.push(Entry::Thinking { text: full, streaming: false }),
            },
            Event::TextDelta(piece) => match self.entries.last_mut() {
                Some(Entry::Assistant { text, streaming: true }) => text.push_str(&piece),
                _ => self.entries.push(Entry::Assistant { text: piece, streaming: true }),
            },
            Event::Text(full) => match self.entries.last_mut() {
                // The finished block replaces what was streamed of it.
                Some(Entry::Assistant { text, streaming }) if *streaming => {
                    *text = full;
                    *streaming = false;
                }
                _ if full.trim().is_empty() => {}
                _ => self.entries.push(Entry::Assistant { text: full, streaming: false }),
            },
            Event::ToolUse { id, name, input } => {
                // Whatever was being streamed is complete once a tool starts.
                if let Some(Entry::Assistant { streaming, .. }) = self.entries.last_mut() {
                    *streaming = false;
                }
                let (summary, path) = describe_tool(&name, &input);
                self.entries.push(Entry::Tool { id, name, summary, path, input, result: None, failed: false });
            }
            Event::ToolResult { id, text, is_error } => {
                if let Some(Entry::Tool { result, failed, .. }) = self.entries.iter_mut().rev().find(|e| matches!(e, Entry::Tool { id: i, .. } if *i == id)) {
                    // A tool can return a whole file; the chat keeps the start of it.
                    *result = Some(text.chars().take(MAX_RESULT_CHARS).collect());
                    *failed = is_error;
                }
            }
            Event::Permission(request) => self.pending.push(request),
            Event::PermissionCancelled(id) => self.pending.retain(|r| r.id != id),
            Event::Unsupported { .. } => {}
            Event::UserText(text) => self.entries.push(Entry::User(strip_context(&text))),
            Event::Done { error, session_id, cost_usd, .. } => {
                self.busy = false;
                self.pending.clear();
                if let Some(Entry::Assistant { streaming, .. }) = self.entries.last_mut() {
                    *streaming = false;
                }
                if !session_id.is_empty() {
                    self.session_id = Some(session_id);
                }
                self.cost_usd += cost_usd.unwrap_or(0.0);
                if let Some(error) = error {
                    self.notice(error);
                }
            }
        }
    }
}

/// The longest tool result the chat keeps, in characters.
const MAX_RESULT_CHARS: usize = 4000;

/// A question as the user typed it: without the editor context [`compose_prompt`] put in front of it.
pub fn strip_context(text: &str) -> String {
    match text.strip_prefix("<editor_context>").and_then(|rest| rest.split_once("</editor_context>")) {
        Some((_, question)) => question.trim_start().to_owned(),
        None => text.to_owned(),
    }
}

/// A few words on what a tool call does, and the file it works on (so the chat can open it).
pub fn describe_tool(name: &str, input: &Value) -> (String, Option<String>) {
    let text = |key: &str| input.get(key).and_then(Value::as_str).map(str::to_owned);
    let path = text("file_path").or_else(|| text("notebook_path")).or_else(|| text("path"));
    let summary = match name {
        "Read" | "Edit" | "MultiEdit" | "Write" | "NotebookEdit" => path.clone().unwrap_or_default(),
        "Bash" => text("command").unwrap_or_default(),
        "Grep" | "Glob" => text("pattern").unwrap_or_default(),
        "WebFetch" => text("url").unwrap_or_default(),
        "WebSearch" => text("query").unwrap_or_default(),
        "Task" | "Agent" => text("description").unwrap_or_default(),
        _ => text("description").or_else(|| text("command")).or_else(|| text("query")).unwrap_or_default(),
    };
    let only_file = matches!(name, "Read" | "Edit" | "MultiEdit" | "Write" | "NotebookEdit");
    (summary.lines().next().unwrap_or_default().chars().take(200).collect(), if only_file || path.is_some() { path } else { None })
}

/// A path as the chat shows it: relative to the project when it is inside it.
pub fn display_path(path: &str, root: &Path) -> String {
    Path::new(path).strip_prefix(root).map(|p| p.to_string_lossy().replace('\\', "/")).unwrap_or_else(|_| path.to_owned())
}

// ---- finding and running the program ----

/// The `claude` program: on the PATH, or where its installers put it.
pub fn find_claude(configured: &str) -> Option<PathBuf> {
    let configured = configured.trim();
    if !configured.is_empty() {
        let p = PathBuf::from(configured);
        return p.is_file().then_some(p);
    }
    let names: &[&str] = if cfg!(windows) { &["claude.exe", "claude.cmd", "claude.bat"] } else { &["claude"] };
    let mut dirs: Vec<PathBuf> = std::env::var_os("PATH").map(|p| std::env::split_paths(&p).collect()).unwrap_or_default();
    let home = std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" }).map(PathBuf::from);
    if let Some(home) = &home {
        dirs.extend([home.join(".local/bin"), home.join(".claude/local"), home.join(".npm-global/bin"), home.join(".bun/bin"), home.join(".volta/bin")]);
    }
    if cfg!(windows) {
        if let Some(appdata) = std::env::var_os("APPDATA") {
            dirs.push(PathBuf::from(appdata).join("npm"));
        }
    } else {
        dirs.extend([PathBuf::from("/usr/local/bin"), PathBuf::from("/opt/homebrew/bin"), PathBuf::from("/usr/bin")]);
    }
    dirs.iter().flat_map(|d| names.iter().map(move |n| d.join(n))).find(|p| p.is_file())
}

/// What to run: the program, the folder, and how this turn continues the chat.
#[derive(Clone, Debug)]
pub struct Options {
    pub binary: PathBuf,
    pub cwd: PathBuf,
    pub mode: PermissionMode,
    pub model: String,
    /// How hard Claude thinks (`low` .. `max`); empty is the program's own choice.
    pub effort: String,
    /// Ask for summaries of the thinking, so there is something to show. (Without it the text of the thinking is left out.)
    pub show_thinking: bool,
    /// The session to continue, when the chat has already begun.
    pub resume: Option<String>,
}

/// The program's arguments for one turn. The question itself goes in on standard input, as a JSON line.
pub fn build_args(opts: &Options) -> Vec<String> {
    // Streaming input, and the program asks this side (not a terminal) before it uses a tool the mode does not allow.
    let mut args: Vec<String> = ["-p", "--input-format", "stream-json", "--output-format", "stream-json", "--verbose", "--include-partial-messages", "--permission-prompt-tool", "stdio", "--permission-mode", opts.mode.id()]
        .map(String::from)
        .to_vec();
    if !opts.model.trim().is_empty() {
        args.extend(["--model".to_owned(), opts.model.trim().to_owned()]);
    }
    if opts.show_thinking {
        args.extend(["--thinking-display".to_owned(), "summarized".to_owned()]);
    }
    if !opts.effort.trim().is_empty() {
        args.extend(["--effort".to_owned(), opts.effort.trim().to_owned()]);
    }
    if let Some(id) = opts.resume.as_ref().filter(|s| !s.is_empty()) {
        args.extend(["--resume".to_owned(), id.clone()]);
    }
    args
}

/// What a running turn sends back.
#[derive(Debug)]
pub enum Message {
    Event(Event),
    /// A line the program wrote on its error output.
    Stderr(String),
    /// The program ended, with its exit code.
    Exited(Option<i32>),
}

/// One running turn.
pub struct Run {
    child: Arc<Mutex<Child>>,
    /// Where answers to the program's questions go. Closed when the turn ends, which lets the program exit.
    stdin: Arc<Mutex<Option<ChildStdin>>>,
    rx: Receiver<Message>,
}

/// Start one turn: run the program and hand it `prompt`.
pub fn start(opts: &Options, prompt: &str) -> Result<Run, String> {
    let mut command = Command::new(&opts.binary);
    command.args(build_args(opts)).current_dir(&opts.cwd).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
    // The chat is not a terminal: no colours, no progress bars.
    command.env("NO_COLOR", "1").env("TERM", "dumb");
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000);
    }
    let mut child = command.spawn().map_err(|e| format!("could not run {}: {e}", opts.binary.display()))?;
    let stdin = child.stdin.take().ok_or("could not talk to Claude")?;
    let stdout = child.stdout.take().ok_or("could not read Claude's output")?;
    let stderr = child.stderr.take().ok_or("could not read Claude's errors")?;
    let stdin = Arc::new(Mutex::new(Some(stdin)));
    // The question can be long: write it from a thread so a full pipe never stalls the editor.
    let first = stdin.clone();
    let line = user_message(prompt);
    std::thread::spawn(move || {
        if let Some(pipe) = first.lock().ok().as_mut().and_then(|g| g.as_mut()) {
            let _ = writeln!(pipe, "{line}");
            let _ = pipe.flush();
        }
    });
    let (tx, rx) = mpsc::channel();
    let child = Arc::new(Mutex::new(child));

    let err_tx = tx.clone();
    std::thread::spawn(move || {
        for line in BufReader::new(stderr).lines().map_while(Result::ok) {
            if !line.trim().is_empty() && err_tx.send(Message::Stderr(line)).is_err() {
                break;
            }
        }
    });
    let waiter = child.clone();
    let closer = stdin.clone();
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            for event in parse_line(&line) {
                let done = matches!(event, Event::Done { .. });
                if tx.send(Message::Event(event)).is_err() {
                    return;
                }
                if done {
                    // The turn is over: closing the input is how the program is told to finish.
                    if let Ok(mut pipe) = closer.lock() {
                        *pipe = None;
                    }
                }
            }
        }
        let code = waiter.lock().ok().and_then(|mut c| c.wait().ok()).and_then(|s| s.code());
        let _ = tx.send(Message::Exited(code));
    });
    Ok(Run { child, stdin, rx })
}

impl Run {
    /// Everything reported since the last call, and whether the program has ended.
    pub fn poll(&mut self) -> (Vec<Message>, bool) {
        let mut out = Vec::new();
        loop {
            match self.rx.try_recv() {
                Ok(m @ Message::Exited(_)) => {
                    out.push(m);
                    return (out, true);
                }
                Ok(m) => out.push(m),
                Err(TryRecvError::Empty) => return (out, false),
                Err(TryRecvError::Disconnected) => return (out, true),
            }
        }
    }

    /// Answer one of the program's questions.
    pub fn respond(&self, request_id: &str, decision: &Decision) -> Result<(), String> {
        self.write_line(&control_response(request_id, decision))
    }

    /// Tell the program it cannot have what it asked for.
    pub fn refuse(&self, request_id: &str, error: &str) -> Result<(), String> {
        self.write_line(&control_error(request_id, error))
    }

    fn write_line(&self, line: &str) -> Result<(), String> {
        let mut guard = self.stdin.lock().map_err(|_| "the connection to Claude is broken".to_owned())?;
        let pipe = guard.as_mut().ok_or("Claude is no longer listening")?;
        writeln!(pipe, "{line}").and_then(|_| pipe.flush()).map_err(|e| format!("could not answer Claude: {e}"))
    }

    /// Stop the program.
    pub fn stop(&self) {
        if let Ok(mut child) = self.child.lock() {
            let _ = child.kill();
        }
    }
}

impl Drop for Run {
    fn drop(&mut self) {
        self.stop();
    }
}

// ---- what the question carries ----

/// The editor's state that goes with a question: the open file and the selected text, like the VS Code extension's
/// automatic context.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct EditorContext {
    /// The open file, relative to the project when it is inside it.
    pub file: Option<String>,
    /// The selected text, with its first and last line (1-based).
    pub selection: Option<(String, usize, usize)>,
}

/// The longest selection sent, in characters.
const MAX_SELECTION_CHARS: usize = 20_000;

/// The text handed to Claude: the question, with the editor context in front when there is any.
pub fn compose_prompt(question: &str, context: Option<&EditorContext>) -> String {
    let Some(ctx) = context.filter(|c| c.file.is_some() || c.selection.is_some()) else { return question.to_owned() };
    let mut out = String::from("<editor_context>\n");
    if let Some(file) = &ctx.file {
        out.push_str(&format!("The user has {file} open in their editor.\n"));
    }
    if let Some((text, from, to)) = &ctx.selection {
        let shown: String = text.chars().take(MAX_SELECTION_CHARS).collect();
        let lines = if from == to { format!("line {from}") } else { format!("lines {from}-{to}") };
        let clipped = if shown.len() < text.len() { " (cut short)" } else { "" };
        out.push_str(&format!("Selected text, {lines}{clipped}:\n```\n{shown}\n```\n"));
    }
    out.push_str("</editor_context>\n\n");
    out.push_str(question);
    out
}

// ---- showing a change ----

/// What a row of a diff is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RowKind {
    Same,
    Add,
    Del,
    /// Lines left out between two changes.
    Gap,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiffRow {
    pub kind: RowKind,
    pub text: String,
}

/// The most rows of a diff kept for display.
const MAX_DIFF_ROWS: usize = 400;

/// The changed lines between `old` and `new` with `context` unchanged lines around each change.
pub fn diff_rows(old: &str, new: &str, context: usize) -> Vec<DiffRow> {
    use similar::{ChangeTag, TextDiff};
    let diff = TextDiff::from_lines(old, new);
    let mut rows = Vec::new();
    for (gi, group) in diff.grouped_ops(context).iter().enumerate() {
        if gi > 0 {
            rows.push(DiffRow { kind: RowKind::Gap, text: String::new() });
        }
        for op in group {
            for change in diff.iter_changes(op) {
                let kind = match change.tag() {
                    ChangeTag::Equal => RowKind::Same,
                    ChangeTag::Insert => RowKind::Add,
                    ChangeTag::Delete => RowKind::Del,
                };
                rows.push(DiffRow { kind, text: change.value().trim_end_matches(['\n', '\r']).to_owned() });
            }
        }
        if rows.len() >= MAX_DIFF_ROWS {
            rows.truncate(MAX_DIFF_ROWS);
            break;
        }
    }
    rows
}

/// How many lines were added and removed.
pub fn diff_counts(rows: &[DiffRow]) -> (usize, usize) {
    (rows.iter().filter(|r| r.kind == RowKind::Add).count(), rows.iter().filter(|r| r.kind == RowKind::Del).count())
}

/// Whether a tool changes a file's text.
pub fn is_edit_tool(name: &str) -> bool {
    matches!(name, "Edit" | "MultiEdit" | "Write")
}

/// The file as it would be after the tool ran on `before`. `None` when it cannot be told (a text to replace that is not
/// there, a tool that is not an edit).
pub fn edit_preview(before: &str, tool: &str, input: &Value) -> Option<String> {
    let replace = |text: &str, edit: &Value| -> Option<String> {
        let (old, new) = (edit["old_string"].as_str()?, edit["new_string"].as_str()?);
        if old.is_empty() {
            return Some(format!("{new}{text}"));
        }
        if !text.contains(old) {
            return None;
        }
        Some(if edit["replace_all"].as_bool().unwrap_or(false) { text.replace(old, new) } else { text.replacen(old, new, 1) })
    };
    match tool {
        "Write" => input["content"].as_str().map(str::to_owned),
        "Edit" => replace(before, input),
        "MultiEdit" => input["edits"].as_array()?.iter().try_fold(before.to_owned(), |text, edit| replace(&text, edit)),
        _ => None,
    }
}

/// A change drawn from the tool's own input, when the file as it was is not known (a saved conversation): the old text
/// as removed lines and the new as added ones.
pub fn input_diff(tool: &str, input: &Value) -> Vec<DiffRow> {
    let pair = |old: &str, new: &str| diff_rows(old, new, 0);
    match tool {
        "Write" => pair("", input["content"].as_str().unwrap_or_default()),
        "Edit" => pair(input["old_string"].as_str().unwrap_or_default(), input["new_string"].as_str().unwrap_or_default()),
        "MultiEdit" => input["edits"].as_array().map(|edits| edits.iter().flat_map(|e| pair(e["old_string"].as_str().unwrap_or_default(), e["new_string"].as_str().unwrap_or_default())).collect()).unwrap_or_default(),
        _ => Vec::new(),
    }
}

// ---- saved conversations ----

/// Claude Code's folder (`CLAUDE_CONFIG_DIR`, or `~/.claude`).
pub fn claude_home() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os("CLAUDE_CONFIG_DIR").filter(|d| !d.is_empty()) {
        return Some(PathBuf::from(dir));
    }
    std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" }).map(|h| PathBuf::from(h).join(".claude"))
}

/// The name of the folder that holds a project's conversations: its path with everything but letters and digits as `-`.
pub fn project_dir_name(cwd: &Path) -> String {
    cwd.to_string_lossy().chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '-' }).collect()
}

/// One saved conversation.
#[derive(Clone, Debug, PartialEq)]
pub struct SessionInfo {
    pub id: String,
    /// What was first asked, in a few words.
    pub title: String,
    pub modified: std::time::SystemTime,
    pub path: PathBuf,
}

/// The most conversations listed.
const MAX_SESSIONS: usize = 40;

/// The saved conversations of the project in `cwd`, newest first.
pub fn list_sessions(cwd: &Path) -> Vec<SessionInfo> {
    let Some(dir) = claude_home().map(|h| h.join("projects").join(project_dir_name(cwd))) else { return Vec::new() };
    list_sessions_in(&dir)
}

/// The saved conversations in one project folder, newest first.
pub fn list_sessions_in(dir: &Path) -> Vec<SessionInfo> {
    let Ok(read) = std::fs::read_dir(dir) else { return Vec::new() };
    let mut found: Vec<SessionInfo> = read
        .filter_map(Result::ok)
        .filter(|e| e.path().extension().is_some_and(|x| x == "jsonl"))
        .filter_map(|e| {
            let path = e.path();
            let modified = e.metadata().ok()?.modified().ok()?;
            let title = first_question(&path)?;
            Some(SessionInfo { id: path.file_stem()?.to_string_lossy().into_owned(), title, modified, path })
        })
        .collect();
    found.sort_by(|a, b| b.modified.cmp(&a.modified));
    // Continuing a conversation can leave the earlier file behind with the same opening question: keep the newest.
    let mut seen = std::collections::HashSet::new();
    found.retain(|s| seen.insert(s.title.clone()));
    found.truncate(MAX_SESSIONS);
    found
}

/// The first thing the user asked in a saved conversation, shortened.
fn first_question(path: &Path) -> Option<String> {
    let file = std::fs::File::open(path).ok()?;
    for line in BufReader::new(file).lines().map_while(Result::ok).take(400) {
        for event in parse_transcript_line(&line) {
            if let Event::UserText(text) = event {
                let text = strip_context(&text);
                let first = text.lines().find(|l| !l.trim().is_empty())?.trim();
                return Some(first.chars().take(80).collect());
            }
        }
    }
    None
}

/// A saved conversation, ready to show and to continue.
pub fn load_session(path: &Path) -> Result<Conversation, String> {
    let file = std::fs::File::open(path).map_err(|e| format!("could not open the conversation: {e}"))?;
    let mut convo = Conversation { session_id: path.file_stem().map(|s| s.to_string_lossy().into_owned()), ..Default::default() };
    for line in BufReader::new(file).lines().map_while(Result::ok) {
        for event in parse_transcript_line(&line) {
            convo.apply(event);
        }
    }
    Ok(convo)
}

/// "3 min ago", for the conversation list.
pub fn ago(then: std::time::SystemTime, now: std::time::SystemTime) -> String {
    let secs = now.duration_since(then).map(|d| d.as_secs()).unwrap_or(0);
    match secs {
        0..=59 => "just now".to_owned(),
        60..=3599 => format!("{} min ago", secs / 60),
        3600..=86_399 => format!("{} h ago", secs / 3600),
        _ => format!("{} d ago", secs / 86_400),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn feed(convo: &mut Conversation, lines: &[&str]) {
        for line in lines {
            for event in parse_line(line) {
                convo.apply(event);
            }
        }
    }

    // Lines as `claude -p --output-format stream-json --verbose --include-partial-messages` writes them.
    const INIT: &str = r#"{"type":"system","subtype":"init","session_id":"abc-1","model":"claude-opus-5-5","tools":["Read"]}"#;
    const TOOL: &str = r#"{"type":"assistant","message":{"content":[{"type":"tool_use","id":"t1","name":"Read","input":{"file_path":"/p/a.txt"}}]}}"#;
    const RESULT_OF_TOOL: &str = r#"{"type":"user","message":{"content":[{"tool_use_id":"t1","type":"tool_result","content":"1\thello"}]}}"#;
    const DELTA1: &str = r#"{"type":"stream_event","event":{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"a.txt has"}}}"#;
    const DELTA2: &str = r#"{"type":"stream_event","event":{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":" hello."}}}"#;
    const FINAL: &str = r#"{"type":"assistant","message":{"content":[{"type":"text","text":"a.txt has hello."}]}}"#;
    const DONE: &str = r#"{"type":"result","subtype":"success","is_error":false,"result":"a.txt has hello.","session_id":"abc-1","total_cost_usd":0.5,"duration_ms":3400}"#;

    #[test]
    fn a_whole_turn_becomes_chat_entries() {
        let mut c = Conversation::default();
        c.ask("what is in a.txt?");
        feed(&mut c, &[INIT, TOOL, RESULT_OF_TOOL, DELTA1, DELTA2, FINAL, DONE]);
        assert_eq!(c.session_id.as_deref(), Some("abc-1"));
        assert_eq!(c.model.as_deref(), Some("claude-opus-5-5"));
        assert!(!c.busy);
        assert_eq!(c.cost_usd, 0.5);
        assert_eq!(c.entries.len(), 3, "{:?}", c.entries);
        assert_eq!(c.entries[0], Entry::User("what is in a.txt?".into()));
        assert_eq!(
            c.entries[1],
            Entry::Tool { id: "t1".into(), name: "Read".into(), summary: "/p/a.txt".into(), path: Some("/p/a.txt".into()), input: json!({"file_path": "/p/a.txt"}), result: Some("1\thello".into()), failed: false }
        );
        // The streamed pieces and the finished block are one answer, not two.
        assert_eq!(c.entries[2], Entry::Assistant { text: "a.txt has hello.".into(), streaming: false });
    }

    #[test]
    fn streaming_text_grows_in_place_until_the_block_arrives() {
        let mut c = Conversation::default();
        feed(&mut c, &[DELTA1]);
        assert_eq!(c.entries, vec![Entry::Assistant { text: "a.txt has".into(), streaming: true }]);
        feed(&mut c, &[DELTA2]);
        assert_eq!(c.entries, vec![Entry::Assistant { text: "a.txt has hello.".into(), streaming: true }]);
        feed(&mut c, &[FINAL]);
        assert_eq!(c.entries, vec![Entry::Assistant { text: "a.txt has hello.".into(), streaming: false }]);
    }

    #[test]
    fn without_partial_messages_the_block_alone_is_the_answer() {
        let mut c = Conversation::default();
        feed(&mut c, &[FINAL, FINAL]);
        assert_eq!(c.entries.len(), 2);
    }

    #[test]
    fn a_failed_turn_is_reported_and_ends_the_busy_state() {
        let mut c = Conversation::default();
        c.ask("hi");
        feed(&mut c, &[r#"{"type":"result","subtype":"success","is_error":true,"result":"Invalid API key","session_id":"s"}"#]);
        assert!(!c.busy);
        assert_eq!(c.entries.last(), Some(&Entry::Notice("Invalid API key".into())));
        let mut c = Conversation::default();
        feed(&mut c, &[r#"{"type":"result","subtype":"error_max_turns","session_id":"s"}"#]);
        assert_eq!(c.entries.last(), Some(&Entry::Notice("Claude stopped: error_max_turns".into())));
    }

    #[test]
    fn tool_errors_are_kept_with_their_tool() {
        let mut c = Conversation::default();
        feed(&mut c, &[TOOL, r#"{"type":"user","message":{"content":[{"tool_use_id":"t1","type":"tool_result","is_error":true,"content":[{"type":"text","text":"permission denied"}]}]}}"#]);
        assert!(matches!(&c.entries[0], Entry::Tool { failed: true, result: Some(r), .. } if r == "permission denied"));
    }

    #[test]
    fn unknown_and_broken_lines_are_ignored() {
        assert!(parse_line("").is_empty());
        assert!(parse_line("not json").is_empty());
        assert!(parse_line(r#"{"type":"rate_limit_event"}"#).is_empty());
        assert!(parse_line(r#"{"type":"stream_event","event":{"type":"message_stop"}}"#).is_empty());
    }

    #[test]
    fn tools_are_described_in_a_few_words() {
        assert_eq!(describe_tool("Bash", &json!({"command": "cargo test\n--all"})), ("cargo test".to_owned(), None));
        assert_eq!(describe_tool("Edit", &json!({"file_path": "/p/src/main.rs"})), ("/p/src/main.rs".to_owned(), Some("/p/src/main.rs".to_owned())));
        assert_eq!(describe_tool("Grep", &json!({"pattern": "fn main"})), ("fn main".to_owned(), None));
        assert_eq!(describe_tool("Mystery", &json!({})), (String::new(), None));
        assert_eq!(display_path("/p/src/main.rs", Path::new("/p")), "src/main.rs");
        assert_eq!(display_path("/elsewhere/x", Path::new("/p")), "/elsewhere/x");
    }

    #[test]
    fn arguments_continue_the_session_and_set_the_mode() {
        let mut o = Options { binary: "claude".into(), cwd: ".".into(), mode: PermissionMode::AcceptEdits, model: String::new(), effort: String::new(), show_thinking: false, resume: None };
        let a = build_args(&o);
        assert!(a.windows(2).any(|w| w == ["--permission-mode", "acceptEdits"]));
        assert!(a.windows(2).any(|w| w == ["--permission-prompt-tool", "stdio"]) && a.windows(2).any(|w| w == ["--input-format", "stream-json"]));
        assert!(a.contains(&"--include-partial-messages".to_owned()) && !a.contains(&"--resume".to_owned()) && !a.contains(&"--model".to_owned()));
        o.resume = Some("abc".into());
        o.model = " sonnet ".into();
        o.effort = "high".into();
        o.show_thinking = true;
        let a = build_args(&o);
        assert!(a.windows(2).any(|w| w == ["--thinking-display", "summarized"]));
        o.show_thinking = false;
        let a = build_args(&o);
        assert!(!a.contains(&"--thinking-display".to_owned()));
        assert!(a.windows(2).any(|w| w == ["--resume", "abc"]) && a.windows(2).any(|w| w == ["--model", "sonnet"]) && a.windows(2).any(|w| w == ["--effort", "high"]));
    }

    #[test]
    fn thinking_streams_in_place_and_ends_when_the_answer_starts() {
        let delta = |t: &str| format!(r#"{{"type":"stream_event","event":{{"type":"content_block_delta","index":0,"delta":{{"type":"thinking_delta","thinking":{t:?}}}}}}}"#);
        let done_block = r#"{"type":"assistant","message":{"content":[{"type":"thinking","thinking":"Counting multiples.","signature":"x"}]}}"#;
        let mut c = Conversation::default();
        feed(&mut c, &[&delta("Counting"), &delta(" multiples.")]);
        assert_eq!(c.entries, vec![Entry::Thinking { text: "Counting multiples.".into(), streaming: true }]);
        feed(&mut c, &[done_block]);
        assert_eq!(c.entries, vec![Entry::Thinking { text: "Counting multiples.".into(), streaming: false }]);
        // The answer after it is a separate entry, and a second thought after a tool is a second block.
        feed(&mut c, &[&delta("More"), DELTA1]);
        assert!(matches!(c.entries.as_slice(), [Entry::Thinking { streaming: false, .. }, Entry::Thinking { streaming: false, .. }, Entry::Assistant { streaming: true, .. }]), "{:?}", c.entries);
        // Thinking that came back empty (no summary asked for) shows nothing.
        let mut e = Conversation::default();
        feed(&mut e, &[&delta(""), r#"{"type":"assistant","message":{"content":[{"type":"thinking","thinking":"","signature":"abc"}]}}"#]);
        assert!(e.entries.is_empty(), "{:?}", e.entries);
        // A saved conversation keeps its thinking.
        let mut saved = Conversation::default();
        for ev in parse_transcript_line(done_block) {
            saved.apply(ev);
        }
        assert_eq!(saved.entries, vec![Entry::Thinking { text: "Counting multiples.".into(), streaming: false }]);
    }

    #[test]
    fn models_and_effort_levels_have_names() {
        assert_eq!((model_label(""), model_label("opus"), model_label("claude-opus-4-1")), ("Default".to_owned(), "Opus".to_owned(), "claude-opus-4-1".to_owned()));
        assert_eq!((effort_label(""), effort_label("xhigh"), effort_label("odd")), ("Default".to_owned(), "Extra high".to_owned(), "odd".to_owned()));
        assert!(is_effort("") && is_effort("max") && is_effort("LOW") && !is_effort("extreme"));
        assert_eq!(MODELS[0].0, "", "the first choice leaves it to Claude Code");
    }

    #[test]
    fn permission_modes_round_trip_and_cycle() {
        for m in PermissionMode::ALL {
            assert_eq!(PermissionMode::from_id(m.id()), Some(m));
        }
        assert_eq!(PermissionMode::from_id("bogus"), None);
        assert_eq!(PermissionMode::Ask.next().next().next(), PermissionMode::Ask);
    }

    #[test]
    fn the_prompt_carries_the_open_file_and_selection() {
        assert_eq!(compose_prompt("hi", None), "hi");
        assert_eq!(compose_prompt("hi", Some(&EditorContext::default())), "hi");
        let ctx = EditorContext { file: Some("src/a.rs".into()), selection: Some(("let x = 1;".into(), 3, 3)) };
        let p = compose_prompt("what is x?", Some(&ctx));
        assert!(p.starts_with("<editor_context>") && p.contains("src/a.rs") && p.contains("line 3") && p.contains("let x = 1;"));
        assert!(p.ends_with("what is x?"));
        let long = EditorContext { file: None, selection: Some(("x".repeat(MAX_SELECTION_CHARS + 5), 1, 9)) };
        let p = compose_prompt("q", Some(&long));
        assert!(p.contains("lines 1-9") && p.contains("cut short") && p.len() < MAX_SELECTION_CHARS + 200);
    }

    /// A stand-in program that prints a canned turn, to run the whole path: process, threads, parsing, polling.
    #[cfg(unix)]
    #[test]
    fn a_turn_runs_end_to_end_against_a_stand_in_program() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("forge-claude-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let script = dir.join("claude");
        let body = format!("#!/bin/sh\nread -r line\necho \"$line\" > \"{0}/prompt.txt\"\necho '{1}'\necho '{2}'\necho '{3}'\necho oops >&2\n", dir.display(), INIT, FINAL, DONE);
        std::fs::write(&script, body).unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();

        let opts = Options { binary: script, cwd: dir.clone(), mode: PermissionMode::Ask, model: String::new(), effort: String::new(), show_thinking: false, resume: None };
        let mut run = start(&opts, "hello there").unwrap();
        let mut convo = Conversation::default();
        convo.ask("hello there");
        let (mut stderr, mut exited) = (Vec::new(), None);
        for _ in 0..200 {
            let (messages, ended) = run.poll();
            for m in messages {
                match m {
                    Message::Event(e) => convo.apply(e),
                    Message::Stderr(s) => stderr.push(s),
                    Message::Exited(code) => exited = Some(code),
                }
            }
            if ended {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(25));
        }
        assert_eq!(exited, Some(Some(0)));
        assert_eq!(stderr, vec!["oops".to_owned()]);
        assert!(!convo.busy);
        assert_eq!(convo.entries.last(), Some(&Entry::Assistant { text: "a.txt has hello.".into(), streaming: false }));
        // The question went in as one JSON line.
        let sent: Value = serde_json::from_str(std::fs::read_to_string(dir.join("prompt.txt")).unwrap().trim()).unwrap();
        assert_eq!((sent["type"].as_str(), sent["message"]["content"].as_str()), (Some("user"), Some("hello there")));
        let _ = std::fs::remove_dir_all(&dir);
    }

    const ASK_WRITE: &str = r#"{"type":"control_request","request_id":"r1","request":{"subtype":"can_use_tool","tool_name":"Write","display_name":"Write","input":{"file_path":"/p/a.txt","content":"hi\n"},"description":"a.txt","permission_suggestions":[{"type":"setMode","mode":"acceptEdits","destination":"session"}],"tool_use_id":"t9"}}"#;

    #[test]
    fn a_permission_request_is_read_kept_and_cleared() {
        let mut c = Conversation::default();
        c.ask("write it");
        feed(&mut c, &[ASK_WRITE]);
        let [r] = c.pending.as_slice() else { panic!("{:?}", c.pending) };
        assert_eq!((r.id.as_str(), r.tool_name.as_str(), r.tool_use_id.as_str(), r.description.as_str()), ("r1", "Write", "t9", "a.txt"));
        assert_eq!(r.input["content"], "hi\n");
        assert_eq!(r.suggestions, vec![json!({"type": "setMode", "mode": "acceptEdits", "destination": "session"})]);
        // The tool call being abandoned withdraws the question.
        feed(&mut c, &[r#"{"type":"control_cancel_request","request_id":"r1"}"#]);
        assert!(c.pending.is_empty());
        // A finished turn leaves no question open.
        feed(&mut c, &[ASK_WRITE, DONE]);
        assert!(c.pending.is_empty() && !c.busy);
        // A request of another kind is not a question for the user.
        let other = parse_line(r#"{"type":"control_request","request_id":"r2","request":{"subtype":"hook_callback"}}"#);
        assert_eq!(other, vec![Event::Unsupported { request_id: "r2".into() }]);
    }

    #[test]
    fn answers_are_written_in_the_programs_protocol() {
        let allow: Value = serde_json::from_str(&control_response("r1", &Decision::Allow { input: json!({"a": 1}), permissions: vec![] })).unwrap();
        assert_eq!(allow, json!({"type": "control_response", "response": {"subtype": "success", "request_id": "r1", "response": {"behavior": "allow", "updatedInput": {"a": 1}}}}));
        let keep: Value = serde_json::from_str(&control_response("r1", &Decision::Allow { input: json!({}), permissions: vec![json!({"type": "setMode"})] })).unwrap();
        assert_eq!(keep["response"]["response"]["updatedPermissions"], json!([{"type": "setMode"}]));
        let deny: Value = serde_json::from_str(&control_response("r1", &Decision::Deny { message: "no".into() })).unwrap();
        assert_eq!(deny["response"]["response"], json!({"behavior": "deny", "message": "no"}));
        let err: Value = serde_json::from_str(&control_error("r2", "unsupported")).unwrap();
        assert_eq!((err["response"]["subtype"].as_str(), err["response"]["request_id"].as_str()), (Some("error"), Some("r2")));
        assert!(!control_response("r", &Decision::Deny { message: "a\nb".into() }).contains('\n'), "one line per message");
    }

    #[test]
    fn edits_are_previewed_before_they_happen() {
        let before = "one\ntwo\nthree\n";
        assert_eq!(edit_preview(before, "Edit", &json!({"old_string": "two", "new_string": "2"})).as_deref(), Some("one\n2\nthree\n"));
        assert_eq!(edit_preview("a a a", "Edit", &json!({"old_string": "a", "new_string": "b"})).as_deref(), Some("b a a"));
        assert_eq!(edit_preview("a a a", "Edit", &json!({"old_string": "a", "new_string": "b", "replace_all": true})).as_deref(), Some("b b b"));
        assert_eq!(edit_preview(before, "Edit", &json!({"old_string": "zzz", "new_string": "y"})), None);
        assert_eq!(edit_preview("x y", "MultiEdit", &json!({"edits": [{"old_string": "x", "new_string": "1"}, {"old_string": "y", "new_string": "2"}]})).as_deref(), Some("1 2"));
        assert_eq!(edit_preview("old", "Write", &json!({"content": "new"})).as_deref(), Some("new"));
        assert_eq!(edit_preview(before, "Bash", &json!({})), None);
        assert!(is_edit_tool("Edit") && is_edit_tool("Write") && !is_edit_tool("Read"));
    }

    #[test]
    fn a_diff_shows_the_change_with_context_and_gaps() {
        let old: String = (1..=20).map(|n| format!("line {n}\n")).collect();
        let new = old.replace("line 3\n", "three\n").replace("line 18\n", "eighteen\nextra\n");
        let rows = diff_rows(&old, &new, 1);
        let kinds: Vec<RowKind> = rows.iter().map(|r| r.kind).collect();
        assert!(kinds.contains(&RowKind::Gap), "two far-apart changes are two groups: {rows:?}");
        assert_eq!(diff_counts(&rows), (3, 2));
        assert!(rows.contains(&DiffRow { kind: RowKind::Del, text: "line 3".into() }) && rows.contains(&DiffRow { kind: RowKind::Add, text: "eighteen".into() }));
        assert!(!rows.iter().any(|r| r.text == "line 10"), "unchanged lines far from a change are left out");
        assert!(diff_rows("same\n", "same\n", 3).is_empty());
        // From the tool's own input, when the file is not known.
        assert_eq!(diff_counts(&input_diff("Edit", &json!({"old_string": "a\nb", "new_string": "c"}))), (1, 2));
        assert_eq!(diff_counts(&input_diff("Write", &json!({"content": "x\ny\n"}))), (2, 0));
        assert!(input_diff("Bash", &json!({})).is_empty());
    }

    #[test]
    fn a_saved_conversation_is_replayed() {
        let lines = [
            r#"{"type":"queue-operation","sessionId":"s"}"#,
            r#"{"type":"user","message":{"role":"user","content":"<editor_context>\nThe user has a.rs open in their editor.\n</editor_context>\n\nwhat is this?"},"sessionId":"s"}"#,
            r#"{"type":"user","isMeta":true,"message":{"role":"user","content":"<command-name>/x</command-name>"}}"#,
            r#"{"type":"assistant","message":{"content":[{"type":"tool_use","id":"t1","name":"Read","input":{"file_path":"/p/a.rs"}}]}}"#,
            r#"{"type":"user","message":{"content":[{"tool_use_id":"t1","type":"tool_result","content":"fn x() {}"}]}}"#,
            r#"{"type":"assistant","isSidechain":true,"message":{"content":[{"type":"text","text":"side chat"}]}}"#,
            r#"{"type":"assistant","message":{"content":[{"type":"text","text":"It is a function."}]}}"#,
            r#"{"type":"attachment"}"#,
            r#"{"type":"last-prompt"}"#,
        ];
        let mut c = Conversation::default();
        for l in lines {
            for e in parse_transcript_line(l) {
                c.apply(e);
            }
        }
        assert_eq!(c.entries.len(), 3, "{:?}", c.entries);
        assert_eq!(c.entries[0], Entry::User("what is this?".into()), "the editor context was not typed");
        assert!(matches!(&c.entries[1], Entry::Tool { name, result: Some(r), .. } if name == "Read" && r == "fn x() {}"));
        assert_eq!(c.entries[2], Entry::Assistant { text: "It is a function.".into(), streaming: false });
        assert_eq!(strip_context("plain"), "plain");
    }

    #[test]
    fn conversations_of_a_project_are_listed_newest_first_and_loaded() {
        assert_eq!(project_dir_name(Path::new("/home/me/my project/a.b")), "-home-me-my-project-a-b");
        let dir = std::env::temp_dir().join(format!("forge-claude-sessions-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let write = |name: &str, question: &str| {
            let text = format!("{{\"type\":\"queue-operation\"}}\n{{\"type\":\"user\",\"message\":{{\"content\":\"{question}\"}}}}\n{{\"type\":\"assistant\",\"message\":{{\"content\":[{{\"type\":\"text\",\"text\":\"ok\"}}]}}}}\n");
            std::fs::write(dir.join(name), text).unwrap();
        };
        write("old.jsonl", "first question");
        std::thread::sleep(std::time::Duration::from_millis(30));
        write("new.jsonl", "second question");
        std::fs::write(dir.join("empty.jsonl"), "{\"type\":\"attachment\"}\n").unwrap();
        std::fs::write(dir.join("notes.txt"), "x").unwrap();
        let list = list_sessions_in(&dir);
        assert_eq!(list.iter().map(|s| (s.id.as_str(), s.title.as_str())).collect::<Vec<_>>(), vec![("new", "second question"), ("old", "first question")]);
        let c = load_session(&list[0].path).unwrap();
        assert_eq!((c.session_id.as_deref(), c.entries.len()), (Some("new"), 2));
        assert!(load_session(&dir.join("missing.jsonl")).is_err());
        assert!(list_sessions_in(&dir.join("nope")).is_empty());
        let _ = std::fs::remove_dir_all(&dir);
        let now = std::time::SystemTime::now();
        assert_eq!(ago(now, now), "just now");
        assert_eq!(ago(now - std::time::Duration::from_secs(300), now), "5 min ago");
        assert_eq!(ago(now - std::time::Duration::from_secs(7200), now), "2 h ago");
        assert_eq!(ago(now - std::time::Duration::from_secs(3 * 86_400), now), "3 d ago");
    }

    /// The question and answer cross the pipe both ways while the program waits.
    #[cfg(unix)]
    #[test]
    fn a_permission_is_asked_and_answered_while_the_turn_runs() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("forge-claude-perm-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let script = dir.join("claude");
        let body = format!("#!/bin/sh\nread -r first\nprintf '%s\\n' '{0}'\nread -r answer\nprintf '%s\\n' \"$answer\" > \"{1}/answer.txt\"\nprintf '%s\\n' '{2}'\nread -r more || echo closed > \"{1}/closed.txt\"\n", ASK_WRITE, dir.display(), DONE);
        std::fs::write(&script, body).unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();

        let opts = Options { binary: script, cwd: dir.clone(), mode: PermissionMode::Ask, model: String::new(), effort: String::new(), show_thinking: false, resume: None };
        let mut run = start(&opts, "write it").unwrap();
        let mut convo = Conversation::default();
        convo.ask("write it");
        let mut answered = false;
        let mut exited = false;
        for _ in 0..400 {
            let (messages, ended) = run.poll();
            for m in messages {
                match m {
                    Message::Event(e) => convo.apply(e),
                    Message::Exited(_) => exited = true,
                    Message::Stderr(_) => {}
                }
            }
            if !answered {
                if let Some(request) = convo.pending.first().cloned() {
                    run.respond(&request.id, &Decision::Allow { input: request.input.clone(), permissions: request.suggestions.clone() }).unwrap();
                    convo.pending.clear();
                    answered = true;
                }
            }
            if ended {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(25));
        }
        assert!(answered && exited && !convo.busy);
        let got: Value = serde_json::from_str(std::fs::read_to_string(dir.join("answer.txt")).unwrap().trim()).unwrap();
        assert_eq!(got["response"]["request_id"], "r1");
        assert_eq!(got["response"]["response"]["behavior"], "allow");
        assert_eq!(got["response"]["response"]["updatedInput"]["content"], "hi\n");
        // When the turn ended the input was closed, so the program could finish.
        assert_eq!(std::fs::read_to_string(dir.join("closed.txt")).unwrap().trim(), "closed");
        assert!(run.respond("r1", &Decision::Deny { message: "late".into() }).is_err(), "nobody is listening after the turn");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
