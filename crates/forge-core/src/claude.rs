//! Claude Code inside the editor: a chat that drives the `claude` command-line program, like the VS Code
//! extension does. The program runs headless (`claude -p --output-format stream-json`) in the project folder
//! with the user's own sign-in, and this module turns its JSON lines into a conversation to draw.
//!
//! Nothing here touches the interface: [`parse_line`] reads one line of output, [`Conversation::apply`] folds the
//! events into chat entries, [`find_claude`] locates the program, and [`start`] runs one turn of the chat.

use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::sync::{Arc, Mutex};

use serde_json::Value;

/// What Claude may do without asking. The chat runs headless, so there is nobody to ask mid-turn: a tool that needs
/// permission the mode does not give is refused, and Claude says so.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PermissionMode {
    /// Read files and search; edits and commands are refused.
    #[default]
    ReadOnly,
    /// Also edit files without asking (commands are still refused).
    AcceptEdits,
    /// Look and propose a plan; change nothing.
    Plan,
}

impl PermissionMode {
    pub const ALL: [PermissionMode; 3] = [PermissionMode::ReadOnly, PermissionMode::AcceptEdits, PermissionMode::Plan];

    /// The value of `--permission-mode`, and of `claude.permissionMode` in settings.json.
    pub fn id(self) -> &'static str {
        match self {
            PermissionMode::ReadOnly => "default",
            PermissionMode::AcceptEdits => "acceptEdits",
            PermissionMode::Plan => "plan",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            PermissionMode::ReadOnly => "Read only",
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
    /// Claude is using a tool.
    ToolUse { id: String, name: String, input: Value },
    /// What the tool returned.
    ToolResult { id: String, text: String, is_error: bool },
    /// The turn ended.
    Done { error: Option<String>, session_id: String, cost_usd: Option<f64>, duration_ms: Option<u64> },
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
            Vec::new()
        }
        Some("assistant") => {
            let blocks = v["message"]["content"].as_array().cloned().unwrap_or_default();
            blocks
                .iter()
                .filter_map(|b| match b["type"].as_str()? {
                    "text" => Some(Event::Text(str_of(b, "text"))),
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
    /// A tool Claude used: what it did in a few words, the file it touched, and what came back.
    Tool { id: String, name: String, summary: String, path: Option<String>, result: Option<String>, failed: bool },
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
        match event {
            Event::Init { session_id, model } => {
                if !session_id.is_empty() {
                    self.session_id = Some(session_id);
                }
                if !model.is_empty() {
                    self.model = Some(model);
                }
            }
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
                self.entries.push(Entry::Tool { id, name, summary, path, result: None, failed: false });
            }
            Event::ToolResult { id, text, is_error } => {
                if let Some(Entry::Tool { result, failed, .. }) = self.entries.iter_mut().rev().find(|e| matches!(e, Entry::Tool { id: i, .. } if *i == id)) {
                    *result = Some(text);
                    *failed = is_error;
                }
            }
            Event::Done { error, session_id, cost_usd, .. } => {
                self.busy = false;
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
    /// The session to continue, when the chat has already begun.
    pub resume: Option<String>,
}

/// The program's arguments for one turn. The question itself goes in on standard input.
pub fn build_args(opts: &Options) -> Vec<String> {
    let mut args: Vec<String> = ["-p", "--output-format", "stream-json", "--verbose", "--include-partial-messages", "--permission-mode", opts.mode.id()].map(String::from).to_vec();
    if !opts.model.trim().is_empty() {
        args.extend(["--model".to_owned(), opts.model.trim().to_owned()]);
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
    let mut stdin = child.stdin.take().ok_or("could not talk to Claude")?;
    let stdout = child.stdout.take().ok_or("could not read Claude's output")?;
    let stderr = child.stderr.take().ok_or("could not read Claude's errors")?;
    let prompt = prompt.to_owned();
    // The question can be long: write it from a thread so a full pipe never stalls the editor.
    std::thread::spawn(move || {
        let _ = stdin.write_all(prompt.as_bytes());
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
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            for event in parse_line(&line) {
                if tx.send(Message::Event(event)).is_err() {
                    return;
                }
            }
        }
        let code = waiter.lock().ok().and_then(|mut c| c.wait().ok()).and_then(|s| s.code());
        let _ = tx.send(Message::Exited(code));
    });
    Ok(Run { child, rx })
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
            Entry::Tool { id: "t1".into(), name: "Read".into(), summary: "/p/a.txt".into(), path: Some("/p/a.txt".into()), result: Some("1\thello".into()), failed: false }
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
        let mut o = Options { binary: "claude".into(), cwd: ".".into(), mode: PermissionMode::AcceptEdits, model: String::new(), resume: None };
        let a = build_args(&o);
        assert!(a.windows(2).any(|w| w == ["--permission-mode", "acceptEdits"]));
        assert!(a.contains(&"--include-partial-messages".to_owned()) && !a.contains(&"--resume".to_owned()) && !a.contains(&"--model".to_owned()));
        o.resume = Some("abc".into());
        o.model = " sonnet ".into();
        let a = build_args(&o);
        assert!(a.windows(2).any(|w| w == ["--resume", "abc"]) && a.windows(2).any(|w| w == ["--model", "sonnet"]));
    }

    #[test]
    fn permission_modes_round_trip_and_cycle() {
        for m in PermissionMode::ALL {
            assert_eq!(PermissionMode::from_id(m.id()), Some(m));
        }
        assert_eq!(PermissionMode::from_id("bogus"), None);
        assert_eq!(PermissionMode::ReadOnly.next().next().next(), PermissionMode::ReadOnly);
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
        let body = format!("#!/bin/sh\ncat > \"{0}/prompt.txt\"\necho '{1}'\necho '{2}'\necho '{3}'\necho oops >&2\n", dir.display(), INIT, FINAL, DONE);
        std::fs::write(&script, body).unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();

        let opts = Options { binary: script, cwd: dir.clone(), mode: PermissionMode::ReadOnly, model: String::new(), resume: None };
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
        assert_eq!(std::fs::read_to_string(dir.join("prompt.txt")).unwrap(), "hello there");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
