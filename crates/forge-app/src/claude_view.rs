//! The Claude chat in the side bar: ask about the code, let Claude read and change it. It drives the `claude`
//! program (see `forge_core::claude`), so it uses the user's own Claude Code sign-in and settings.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use forge_core::claude::{self, Conversation, Decision, DiffRow, EditorContext, Entry, Event, ImageAttachment, Message, Options, PermissionMode, PermissionRequest, Run, SessionInfo};
use forge_core::commands::Command;
use forge_core::markdown::{self, Block};
use forge_core::input::Outcome;
use forge_core::textarea::TextArea;
use gpui::{div, point, prelude::*, px, rems, rgb, AnyElement, Context, FontWeight, Keystroke, MouseButton, MouseDownEvent, ScrollHandle};

use crate::context_menu::{ContextMenu, MenuAction, MenuEntry};
use crate::resize::{Edge, Resize};
use crate::workspace::{AreaId, Focus, Workspace};

/// Where to send someone who does not have the program.
const INSTALL_HINT: &str = "Claude Code is not installed. Install it with:  npm install -g @anthropic-ai/claude-code  (see docs.claude.com/claude-code), sign in once by running `claude` in a terminal, then try again. If it is installed somewhere unusual, set \"claude.path\" in settings.json.";

/// The `@name` being typed in the question box, and the project files that match it.
pub(crate) struct Mention {
    /// Where its `@` is, in chars.
    pub start: usize,
    pub items: Vec<String>,
    pub selected: usize,
}

pub(crate) struct ClaudeState {
    pub convo: Conversation,
    pub input: TextArea,
    pub run: Option<Run>,
    /// The program, once it was found (`None` until a look succeeded).
    pub binary: Option<PathBuf>,
    pub mode: PermissionMode,
    /// Send the open file and selection with the question.
    pub use_context: bool,
    pub scroll: ScrollHandle,
    /// The Markdown of each assistant entry, parsed once per change: `(length of the text it was made from, blocks)`.
    pub blocks: Vec<(usize, Vec<Block>)>,
    /// The program's recent error output, shown if it dies without an answer.
    pub stderr: Vec<String>,
    /// Each edit tool call's file as it was just before the call: `tool call id -> (file, text)`.
    pub snapshots: HashMap<String, (PathBuf, String)>,
    /// What each finished edit changed: `tool call id -> rows`.
    pub diffs: HashMap<String, Vec<DiffRow>>,
    /// What each question about an edit would change: `request id -> rows`.
    pub previews: HashMap<String, Vec<DiffRow>>,
    /// The edits whose diff is open in the chat.
    pub expanded: HashSet<String>,
    /// The saved conversations of this project, and whether they are listed instead of the chat.
    pub history: Vec<SessionInfo>,
    pub show_history: bool,
    /// The options picked so far for the questions Claude asks: `question text -> labels`.
    pub answers: HashMap<String, Vec<String>>,
    /// The thinking blocks (by entry) the reader opened after they were done.
    pub open_thoughts: HashSet<usize>,
    /// The turn that is running, to run it again if the program does not know a flag.
    pub last_turn: Option<(Options, String)>,
    /// This Claude Code does not know `--thinking-display`: leave it out.
    pub no_thinking_flag: bool,
    /// The texts drawn in the last frame, in order, so the mouse can pick among them; and the selection in them.
    pub texts: RefCell<Vec<crate::claude_select::Registered>>,
    pub sel: Option<crate::claude_select::Sel>,
    pub sel_dragging: bool,
    /// The chat was scrolled by a drag: the selection's head is re-placed once it has been laid out again.
    pub drag_follow: bool,
    /// The tool calls whose output is open in the chat.
    pub shown_output: HashSet<String>,
    /// Pictures that will go with the next question.
    pub attachments: Vec<ImageAttachment>,
    /// The file suggestions shown while an `@name` is being typed.
    pub mention: Option<Mention>,
}

impl Default for ClaudeState {
    fn default() -> Self {
        Self {
            convo: Conversation::default(),
            input: TextArea::default(),
            run: None,
            binary: None,
            mode: PermissionMode::default(),
            use_context: true,
            scroll: ScrollHandle::new(),
            blocks: Vec::new(),
            stderr: Vec::new(),
            snapshots: HashMap::new(),
            diffs: HashMap::new(),
            previews: HashMap::new(),
            expanded: HashSet::new(),
            history: Vec::new(),
            show_history: false,
            answers: HashMap::new(),
            open_thoughts: HashSet::new(),
            last_turn: None,
            no_thinking_flag: false,
            texts: RefCell::new(Vec::new()),
            sel: None,
            sel_dragging: false,
            drag_follow: false,
            shown_output: HashSet::new(),
            attachments: Vec::new(),
            mention: None,
        }
    }
}

impl Workspace {
    // ---- commands ----

    /// Show the chat on the right and put the caret in its question box.
    pub fn open_claude(&mut self) {
        self.claude_visible = true;
        self.focus = Focus::Claude;
        let _ = self.claude_binary();
        if self.claude.convo.entries.is_empty() {
            self.claude.history = claude::list_sessions(self.root());
        }
    }

    /// Show or hide the chat; hiding it while it has the keyboard hands the keyboard back to the editor.
    pub fn toggle_claude(&mut self) {
        if self.claude_visible {
            self.claude_visible = false;
            if self.focus == Focus::Claude {
                self.focus = Focus::Editor;
            }
        } else {
            self.open_claude();
        }
    }

    /// Drop what belongs to the conversation that is leaving the screen.
    fn claude_forget_details(&mut self) {
        let c = &mut self.claude;
        c.blocks.clear();
        c.stderr.clear();
        c.snapshots.clear();
        c.diffs.clear();
        c.previews.clear();
        c.expanded.clear();
        c.answers.clear();
        c.open_thoughts.clear();
        c.shown_output.clear();
        c.sel = None;
    }

    /// The `claude` program, looked up until it is found (so installing it needs no restart).
    fn claude_binary(&mut self) -> Option<PathBuf> {
        if self.claude.binary.is_none() {
            self.claude.binary = claude::find_claude(&self.settings.claude_path);
        }
        self.claude.binary.clone()
    }

    pub fn claude_new_chat(&mut self) {
        self.claude_stop();
        self.claude.convo.clear();
        self.claude_forget_details();
        self.claude.show_history = false;
        self.open_claude();
    }

    /// Stop the turn that is running.
    pub fn claude_stop(&mut self) {
        if let Some(run) = self.claude.run.take() {
            run.stop();
            self.claude.convo.busy = false;
            self.claude.convo.notice("Stopped.");
            self.claude_after_turn();
        }
    }

    /// The model menu, at the pointer: Claude Code's own choice, the aliases, and a full name set in settings.json.
    pub(crate) fn open_claude_model_menu(&mut self, position: gpui::Point<gpui::Pixels>, cx: &mut Context<Self>) {
        let current = self.settings.claude_model.trim().to_owned();
        let mark = |on: bool| if on { Some("\u{2713}".to_owned()) } else { None };
        let mut entries: Vec<MenuEntry> = claude::MODELS.iter().map(|(value, label)| self.menu_item(label, MenuAction::ClaudeModel((*value).to_owned()), mark(current.eq_ignore_ascii_case(value)), true)).collect();
        if !claude::MODELS.iter().any(|(v, _)| v.eq_ignore_ascii_case(&current)) {
            entries.push(self.menu_item(&current, MenuAction::ClaudeModel(current.clone()), mark(true), true));
        }
        self.context_menu = Some(ContextMenu { position, entries, target: None, bar: None });
        cx.notify();
    }

    /// The thinking menu, at the pointer.
    pub(crate) fn open_claude_effort_menu(&mut self, position: gpui::Point<gpui::Pixels>, cx: &mut Context<Self>) {
        let current = self.settings.claude_effort.clone();
        let mut entries: Vec<MenuEntry> = claude::EFFORTS
            .iter()
            .map(|(value, label)| self.menu_item(label, MenuAction::ClaudeEffort((*value).to_owned()), if current.eq_ignore_ascii_case(value) { Some("\u{2713}".to_owned()) } else { None }, true))
            .collect();
        entries.push(MenuEntry::Separator);
        entries.push(self.menu_item("Show Claude's thinking", MenuAction::ClaudeShowThinking, if self.settings.claude_show_thinking { Some("\u{2713}".to_owned()) } else { None }, true));
        self.context_menu = Some(ContextMenu { position, entries, target: None, bar: None });
        cx.notify();
    }

    /// Show or hide Claude's thinking in the chat, from the next question on; the choice is kept in settings.json.
    pub(crate) fn claude_toggle_thinking(&mut self) {
        let on = !self.settings.claude_show_thinking;
        self.set_setting("claude.showThinking", if on { "true" } else { "false" });
        self.set_status(if on { "Claude's thinking is shown" } else { "Claude's thinking is hidden" });
    }

    /// Open or close a block of thinking.
    pub(crate) fn claude_toggle_thought(&mut self, ix: usize) {
        if !self.claude.open_thoughts.remove(&ix) {
            self.claude.open_thoughts.insert(ix);
        }
    }

    /// Use this model from the next question on, and keep the choice in settings.json.
    pub(crate) fn claude_set_model(&mut self, value: &str) {
        self.set_setting("claude.model", &serde_json::Value::String(value.to_owned()).to_string());
        self.set_status(format!("Claude model: {}", claude::model_label(value)));
    }

    /// Think this hard from the next question on, and keep the choice in settings.json.
    pub(crate) fn claude_set_effort(&mut self, value: &str) {
        self.set_setting("claude.effort", &serde_json::Value::String(value.to_owned()).to_string());
        self.set_status(format!("Claude thinking: {}", claude::effort_label(value)));
    }

    pub fn claude_cycle_mode(&mut self) {
        self.claude.mode = self.claude.mode.next();
        self.set_status(format!("Claude: {}", self.claude.mode.label()));
    }

    /// Put an `@file#L1-9` reference to the selection (or just the file) in the question, like VS Code's Alt+K.
    pub fn claude_add_selection(&mut self) {
        let ctx = self.claude_context();
        self.open_claude();
        let Some(file) = ctx.file else { return };
        let mention = match ctx.selection {
            Some((_, from, to)) if from == to => format!("@{file}#L{from} "),
            Some((_, from, to)) => format!("@{file}#L{from}-{to} "),
            None => format!("@{file} "),
        };
        self.claude.input.insert(&mention);
    }

    /// The open file and the selected text.
    fn claude_context(&self) -> EditorContext {
        let (Some(doc), Some(view)) = (self.active_doc(), self.active_view()) else { return EditorContext::default() };
        // Only a real file on disk: not an untitled tab, a diff or another page made by the editor.
        if doc.untitled || !doc.path.is_file() {
            return EditorContext::default();
        }
        let file = claude::display_path(&doc.path.to_string_lossy(), self.root());
        let sel = view.selections.primary();
        let selection = (!sel.is_empty()).then(|| {
            let (from, to) = (doc.buffer.offset_to_point(sel.start()).0 + 1, doc.buffer.offset_to_point(sel.end()).0 + 1);
            (doc.buffer.rope().slice(sel.start()..sel.end()).to_string(), from, to)
        });
        EditorContext { file: Some(file), selection }
    }

    /// Send the question in the box.
    pub fn claude_send(&mut self) {
        let question = self.claude.input.text().trim().to_owned();
        if question.is_empty() || self.claude.convo.busy {
            return;
        }
        let Some(binary) = self.claude_binary() else {
            self.claude.convo.notice(INSTALL_HINT);
            self.scroll_claude_to_bottom();
            return;
        };
        let context = self.claude.use_context.then(|| self.claude_context());
        let prompt = claude::compose_prompt(&question, context.as_ref());
        let images = std::mem::take(&mut self.claude.attachments);
        let shown_question = images.iter().fold(question.clone(), |q, i| format!("{q}\n[Attached image: {}]", i.name));
        self.claude.mention = None;
        let opts = Options { binary, cwd: self.root().to_path_buf(), mode: self.claude.mode, model: self.settings.claude_model.clone(), effort: self.settings.claude_effort.clone(), show_thinking: self.settings.claude_show_thinking && !self.claude.no_thinking_flag, resume: self.claude.convo.session_id.clone(), images };
        self.claude.convo.ask(&shown_question);
        self.claude.sel = None;
        self.claude.input.set_text("");
        self.claude.stderr.clear();
        match claude::start(&opts, &prompt) {
            Ok(run) => {
                self.claude.run = Some(run);
                self.claude.last_turn = Some((opts, prompt));
            }
            Err(error) => {
                self.claude.convo.busy = false;
                self.claude.convo.notice(error);
            }
        }
        self.claude_refresh_blocks();
        self.scroll_claude_to_bottom();
    }

    // ---- @ files and attachments ----

    /// Look at what is being typed: while it is an `@name`, list the project files that match.
    pub(crate) fn claude_update_mention(&mut self) {
        let text = self.claude.input.text().to_owned();
        let Some((start, query)) = claude::mention_at(&text, self.claude.input.cursor()) else {
            self.claude.mention = None;
            return;
        };
        self.ensure_file_index();
        let items: Vec<String> = match self.file_index.as_ref() {
            Some(files) => forge_core::quick::file_entries(files, self.root(), &query, &self.recent_files, 8).into_iter().map(|e| e.label).collect(),
            None => Vec::new(),
        };
        let selected = self.claude.mention.as_ref().map_or(0, |m| m.selected).min(items.len().saturating_sub(1));
        self.claude.mention = (!items.is_empty()).then_some(Mention { start, items, selected });
    }

    /// Put the chosen file in the question in place of what was typed after the `@`.
    pub(crate) fn claude_accept_mention(&mut self) -> bool {
        let Some(m) = self.claude.mention.take() else { return false };
        let Some(path) = m.items.get(m.selected).cloned() else { return false };
        let caret = self.claude.input.cursor();
        self.claude.input.set_cursor(m.start, false);
        self.claude.input.set_cursor(caret, true);
        self.claude.input.insert(&format!("@{path} "));
        true
    }

    /// Choose files with the system's dialog and attach them.
    pub(crate) fn claude_pick_files(&mut self, cx: &mut Context<Self>) {
        let picked = cx.prompt_for_paths(gpui::PathPromptOptions { files: true, directories: false, multiple: true, prompt: None });
        cx.spawn(async move |this, cx| {
            if let Ok(Ok(Some(paths))) = picked.await {
                let _ = this.update(cx, |workspace, cx| {
                    workspace.claude_attach_paths(&paths);
                    cx.notify();
                });
            }
        })
        .detach();
    }

    /// Attach files to the next question: pictures go along with it, anything else is named in it as an `@file`.
    pub(crate) fn claude_attach_paths(&mut self, paths: &[PathBuf]) {
        for path in paths {
            let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            match claude::image_media_type(path) {
                Some(media) => match std::fs::read(path).map_err(|e| format!("cannot read {name}: {e}")).and_then(|bytes| claude::attach_image(&name, media, &bytes)) {
                    Ok(image) => self.claude_add_image(image),
                    Err(error) => self.set_status(error),
                },
                None => {
                    let label = claude::display_path(&path.to_string_lossy(), self.root());
                    self.claude.input.insert(&format!("@{label} "));
                }
            }
        }
        self.open_claude();
    }

    fn claude_add_image(&mut self, image: ImageAttachment) {
        if !self.claude.attachments.contains(&image) {
            self.claude.attachments.push(image);
        }
    }

    /// A picture on the clipboard goes with the next question; true when there was one.
    pub(crate) fn claude_paste_image(&mut self, item: &gpui::ClipboardItem) -> bool {
        for entry in item.entries() {
            let gpui::ClipboardEntry::Image(image) = entry else { continue };
            let (media, ext) = match image.format {
                gpui::ImageFormat::Png => ("image/png", "png"),
                gpui::ImageFormat::Jpeg => ("image/jpeg", "jpg"),
                gpui::ImageFormat::Webp => ("image/webp", "webp"),
                gpui::ImageFormat::Gif => ("image/gif", "gif"),
                _ => {
                    self.set_status("Claude reads PNG, JPEG, GIF and WebP pictures");
                    return true;
                }
            };
            let name = format!("pasted image {}.{ext}", self.claude.attachments.len() + 1);
            match claude::attach_image(&name, media, &image.bytes) {
                Ok(attached) => self.claude_add_image(attached),
                Err(error) => self.set_status(error),
            }
            return true;
        }
        false
    }

    // ---- the running turn ----

    /// Take what the program reported since the last call. Whether anything changed.
    pub fn poll_claude(&mut self) -> bool {
        let Some(run) = self.claude.run.as_mut() else { return false };
        let (messages, ended) = run.poll();
        if messages.is_empty() && !ended {
            return false;
        }
        // Follow the answer down only if the reader is already at the bottom.
        let follow = self.claude_at_bottom();
        let mut finished = false;
        for message in messages {
            match message {
                Message::Event(event) => {
                    finished |= matches!(event, Event::Done { .. });
                    self.claude_note_event(&event);
                    self.claude.convo.apply(event);
                }
                Message::Stderr(line) => {
                    self.claude.stderr.push(line);
                    let extra = self.claude.stderr.len().saturating_sub(20);
                    self.claude.stderr.drain(..extra);
                }
                Message::Exited(_) => {}
            }
        }
        if ended {
            self.claude.run = None;
            // An older Claude Code does not know the flag that asks for thinking summaries: run the turn again without it.
            let unknown_flag = self.claude.convo.busy && self.claude.stderr.iter().any(|l| l.contains("thinking-display"));
            if let (true, Some((mut opts, prompt))) = (unknown_flag, self.claude.last_turn.take()) {
                self.claude.no_thinking_flag = true;
                opts.show_thinking = false;
                self.claude.stderr.clear();
                match claude::start(&opts, &prompt) {
                    Ok(run) => {
                        self.claude.run = Some(run);
                        self.claude.last_turn = Some((opts, prompt));
                        return true;
                    }
                    Err(error) => self.claude.convo.notice(error),
                }
            }
            if self.claude.convo.busy {
                // It ended without saying it was done: it could not start, or it was killed.
                self.claude.convo.busy = false;
                self.claude.convo.pending.clear();
                let why = self.claude.stderr.join("\n");
                self.claude.convo.notice(if why.trim().is_empty() { "Claude stopped without an answer.".to_owned() } else { why });
                finished = true;
            }
        }
        if finished {
            self.claude_after_turn();
        }
        self.claude_finish_diffs();
        self.claude_refresh_blocks();
        if follow || finished || !self.claude.convo.pending.is_empty() {
            self.scroll_claude_to_bottom();
        }
        true
    }

    /// What to remember of an event before it is folded into the chat: a file as it was before an edit, what an edit
    /// would change, and a polite refusal of requests this chat does not know.
    fn claude_note_event(&mut self, event: &Event) {
        match event {
            Event::ToolUse { id, name, input } if claude::is_edit_tool(name) => {
                if let Some(path) = input["file_path"].as_str() {
                    self.claude.snapshots.insert(id.clone(), (PathBuf::from(path), read_text(path).unwrap_or_default()));
                }
            }
            Event::Permission(request) if claude::is_edit_tool(&request.tool_name) => {
                let before = self.claude.snapshots.get(&request.tool_use_id).map(|(_, t)| t.clone()).or_else(|| request.input["file_path"].as_str().and_then(read_text)).unwrap_or_default();
                let rows = match claude::edit_preview(&before, &request.tool_name, &request.input) {
                    Some(after) => claude::diff_rows(&before, &after, 3),
                    None => claude::input_diff(&request.tool_name, &request.input),
                };
                self.claude.previews.insert(request.id.clone(), rows);
            }
            Event::Unsupported { request_id } => {
                if let Some(run) = &self.claude.run {
                    let _ = run.refuse(request_id, "this chat does not support that request");
                }
            }
            _ => {}
        }
    }

    /// Work out what each edit that has just finished changed, from the file before and after.
    fn claude_finish_diffs(&mut self) {
        let done: Vec<(String, String)> = self
            .claude
            .convo
            .entries
            .iter()
            .filter_map(|e| match e {
                Entry::Tool { id, name, result: Some(_), failed: false, .. } if claude::is_edit_tool(name) && !self.claude.diffs.contains_key(id) => Some((id.clone(), name.clone())),
                _ => None,
            })
            .collect();
        for (id, _) in done {
            let Some((path, before)) = self.claude.snapshots.get(&id).cloned() else { continue };
            // The file on disk is what Claude wrote, not an unsaved copy in the editor.
            if let Some(after) = read_text(&path.to_string_lossy()) {
                self.claude.diffs.insert(id, claude::diff_rows(&before, &after, 3));
            }
        }
    }

    /// Claude changed files, perhaps: show them as they are now.
    fn claude_after_turn(&mut self) {
        self.reload_clean_docs();
        self.tree.refresh();
        self.git_request_refresh();
    }

    // ---- answering Claude ----

    /// Answer the question `id` and let the turn go on.
    pub(crate) fn claude_decide(&mut self, id: &str, decision: Decision) {
        let Some(run) = &self.claude.run else { return };
        if let Err(error) = run.respond(id, &decision) {
            self.claude.convo.notice(error);
        }
        self.claude.convo.pending.retain(|r| r.id != id);
        self.claude.previews.remove(id);
    }

    /// Allow what Claude asked, as asked; with `remember`, also keep the rule change it offered (such as accepting all
    /// edits for the rest of the conversation).
    pub(crate) fn claude_allow(&mut self, request: &PermissionRequest, remember: bool) {
        let permissions = if remember { request.suggestions.clone() } else { Vec::new() };
        if remember && request.suggestions.iter().any(|s| s["mode"] == "acceptEdits") {
            self.claude.mode = PermissionMode::AcceptEdits;
        }
        self.claude_decide(&request.id, Decision::Allow { input: request.input.clone(), permissions });
    }

    pub(crate) fn claude_deny(&mut self, request: &PermissionRequest, message: &str) {
        self.claude_decide(&request.id, Decision::Deny { message: message.to_owned() });
    }

    /// The plan is approved; with `auto_accept`, Claude may then edit without asking.
    pub(crate) fn claude_approve_plan(&mut self, request: &PermissionRequest, auto_accept: bool) {
        let permissions = if auto_accept { vec![serde_json::json!({ "type": "setMode", "mode": "acceptEdits", "destination": "session" })] } else { Vec::new() };
        self.claude.mode = if auto_accept { PermissionMode::AcceptEdits } else { PermissionMode::Ask };
        self.claude_decide(&request.id, Decision::Allow { input: request.input.clone(), permissions });
    }

    /// An option picked for one of Claude's questions. Once every question has an answer they are sent.
    pub(crate) fn claude_pick(&mut self, request: &PermissionRequest, question: &str, label: &str, multi: bool) {
        let picked = self.claude.answers.entry(question.to_owned()).or_default();
        if multi {
            match picked.iter().position(|l| l == label) {
                Some(i) => {
                    picked.remove(i);
                }
                None => picked.push(label.to_owned()),
            }
            return;
        }
        *picked = vec![label.to_owned()];
        let questions = request.input["questions"].as_array().cloned().unwrap_or_default();
        let single_only = questions.iter().all(|q| !q["multiSelect"].as_bool().unwrap_or(false));
        if single_only && questions.iter().all(|q| q["question"].as_str().is_some_and(|t| self.claude.answers.get(t).is_some_and(|a| !a.is_empty()))) {
            self.claude_send_answers(request);
        }
    }

    /// Send the picked options as the answer to Claude's questions.
    pub(crate) fn claude_send_answers(&mut self, request: &PermissionRequest) {
        let answers: serde_json::Map<String, serde_json::Value> = request.input["questions"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|q| q["question"].as_str())
            .map(|q| (q.to_owned(), serde_json::Value::String(self.claude.answers.get(q).map(|a| a.join(", ")).unwrap_or_default())))
            .collect();
        let mut input = request.input.clone();
        input["answers"] = serde_json::Value::Object(answers);
        self.claude.answers.clear();
        self.claude_decide(&request.id, Decision::Allow { input, permissions: Vec::new() });
    }

    // ---- diffs ----

    /// Open a tab with the side-by-side diff of what an edit changed.
    pub(crate) fn claude_open_diff(&mut self, tool_id: &str, cx: &mut Context<Self>) {
        let Some((path, before)) = self.claude.snapshots.get(tool_id).cloned() else {
            self.set_status("The file as it was before this edit is not kept (it is from an earlier session)");
            return;
        };
        let after = read_text(&path.to_string_lossy()).unwrap_or_default();
        let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let text = forge_core::git::diff_texts(&before, &after, &name, &name);
        if text.is_empty() {
            self.set_status("This edit left the file unchanged");
            return;
        }
        self.open_virtual(&format!("claude:{tool_id}"), &format!("{name} (Claude)"), &text, true, cx);
    }

    /// Show or hide what a tool call printed.
    pub(crate) fn claude_toggle_output(&mut self, tool_id: &str) {
        if !self.claude.shown_output.remove(tool_id) {
            self.claude.shown_output.insert(tool_id.to_owned());
        }
    }

    pub(crate) fn claude_toggle_diff(&mut self, tool_id: &str) {
        if !self.claude.expanded.remove(tool_id) {
            self.claude.expanded.insert(tool_id.to_owned());
        }
    }

    // ---- history ----

    pub(crate) fn claude_toggle_history(&mut self) {
        self.claude.show_history = !self.claude.show_history;
        if self.claude.show_history {
            self.claude.history = claude::list_sessions(self.root());
        }
    }

    /// Show a saved conversation and carry on from it with the next question.
    pub(crate) fn claude_resume(&mut self, session: &SessionInfo) {
        match claude::load_session(&session.path) {
            Ok(convo) => {
                self.claude_stop();
                self.claude_forget_details();
                self.claude.convo = convo;
                self.claude.show_history = false;
                self.claude_refresh_blocks();
                self.focus = Focus::Claude;
                self.scroll_claude_to_bottom();
            }
            Err(error) => self.set_status(error),
        }
    }

    fn claude_refresh_blocks(&mut self) {
        let entries = &self.claude.convo.entries;
        self.claude.blocks.resize_with(entries.len(), || (0, Vec::new()));
        for (slot, entry) in self.claude.blocks.iter_mut().zip(entries) {
            if let Entry::Assistant { text, .. } = entry {
                if slot.0 != text.len() || (slot.1.is_empty() && !text.is_empty()) {
                    *slot = (text.len(), markdown::parse(text));
                }
            }
        }
    }

    fn claude_at_bottom(&self) -> bool {
        let (max, offset) = (self.claude.scroll.max_offset(), self.claude.scroll.offset());
        f32::from(max.height) <= 1.0 || f32::from(max.height + offset.y) < 32.0
    }

    fn scroll_claude_to_bottom(&self) {
        let max = self.claude.scroll.max_offset();
        self.claude.scroll.set_offset(point(px(0.0), -max.height - px(10_000.0)));
    }

    /// Characters that fit on a row of the question box.
    pub fn claude_wrap_cols(&self) -> usize {
        let inner = self.claude_width() - 1.0 - 2.0 * 10.0 - 2.0 * (1.0 + 10.0);
        ((inner / f32::from(self.char_width).max(1.0)).floor() as usize).max(8)
    }

    // ---- keys ----

    pub(crate) fn claude_key(&mut self, k: &Keystroke, cx: &mut Context<Self>) -> bool {
        let ctrl = k.modifiers.control || k.modifiers.platform;
        // A card is waiting: its buttons answer to the keyboard (numbers and Enter only while the question box is empty).
        if let Some(request) = self.claude.convo.pending.first().cloned() {
            let plain = !ctrl && !k.modifiers.alt && !k.modifiers.shift;
            let key_ok = k.key == "escape" || (plain && self.claude.input.is_empty());
            if key_ok {
                let actions = claude::card_actions(&request);
                if let Some(i) = claude::card_key(&actions, k.key.as_str()) {
                    let kind = actions[i].kind.clone();
                    self.claude_run_action(&request, &kind);
                    return true;
                }
            }
        }
        // The file suggestions under an `@`: arrows choose, Tab or Enter takes one, Escape closes the list.
        if let Some(m) = self.claude.mention.as_mut() {
            let last = m.items.len().saturating_sub(1);
            match (k.key.as_str(), k.modifiers.shift) {
                ("down", false) => {
                    m.selected = (m.selected + 1).min(last);
                    return true;
                }
                ("up", false) => {
                    m.selected = m.selected.saturating_sub(1);
                    return true;
                }
                ("tab" | "enter", false) if !ctrl && !k.modifiers.alt => {
                    if self.claude_accept_mention() {
                        return true;
                    }
                }
                ("escape", _) => {
                    self.claude.mention = None;
                    return true;
                }
                _ => {}
            }
        }
        match (k.key.as_str(), ctrl, k.modifiers.shift) {
            ("escape", _, _) => {
                if self.claude.convo.busy {
                    self.claude_stop();
                } else {
                    self.focus = Focus::Editor;
                }
                return true;
            }
            ("tab", false, true) => {
                self.claude_cycle_mode();
                return true;
            }
            ("c" | "x", true, _) => {
                if let Some(text) = self.claude.input.selected_text() {
                    cx.write_to_clipboard(gpui::ClipboardItem::new_string(text));
                    if k.key == "x" {
                        self.claude.input.delete_selection();
                    }
                } else if let Some(text) = self.claude_selection_text() {
                    cx.write_to_clipboard(gpui::ClipboardItem::new_string(text));
                    self.set_status("Copied the selected text");
                }
                return true;
            }
            ("v", true, _) => {
                if let Some(item) = cx.read_from_clipboard() {
                    if !self.claude_paste_image(&item) {
                        if let Some(text) = item.text() {
                            self.claude.input.insert(&text);
                        }
                    }
                }
                self.claude_update_mention();
                return true;
            }
            // Enter sends, Shift+Enter starts a new line.
            ("enter", false, false) if !k.modifiers.alt => {
                self.claude_send();
                return true;
            }
            ("enter", false, true) => {
                self.claude.input.insert("\n");
                return true;
            }
            _ => {}
        }
        let changed = matches!(self.claude.input.apply(&k.key, ctrl, k.modifiers.shift, k.key_char.as_deref()), Outcome::Edited | Outcome::Moved | Outcome::Submit);
        if changed {
            self.claude_update_mention();
        }
        changed
    }

    // ---- drawing ----

    /// The chat as a column at the right of the editor, with a handle on its left edge to change its width.
    pub(crate) fn render_claude_panel(&self, cx: &mut Context<Self>) -> AnyElement {
        let t = &self.theme;
        div()
            .flex()
            .flex_col()
            .flex_none()
            .w(px(self.claude_width()))
            .h_full()
            .relative()
            .font_family(self.ui_font.clone())
            .bg(rgb(t.sidebar))
            .border_l_1()
            .border_color(rgb(t.border))
            .child(self.resize_handle(("resize", 7), Resize::Claude, Edge::Left, cx))
            // Files dropped on the chat (from the Explorer or from the system) are attached to the next question.
            .drag_over::<gpui::ExternalPaths>(|s, _, _, _| s.bg(gpui::rgba(0x3b82f61a)))
            .drag_over::<crate::explorer_dnd::DraggedEntry>(|s, _, _, _| s.bg(gpui::rgba(0x3b82f61a)))
            .on_drop(cx.listener(|this, dropped: &gpui::ExternalPaths, _, cx| {
                this.claude_attach_paths(dropped.paths());
                cx.notify();
            }))
            .on_drop(cx.listener(|this, dragged: &crate::explorer_dnd::DraggedEntry, _, cx| {
                this.claude_attach_paths(std::slice::from_ref(&dragged.path));
                cx.notify();
            }))
            .child(self.render_claude(cx))
            .into_any_element()
    }

    pub(crate) fn render_claude(&self, cx: &mut Context<Self>) -> AnyElement {
        let t = self.theme.clone();
        let c = &self.claude;
        let busy = c.convo.busy;
        let icon_button = |id: &'static str, icon: char, tip: &'static str, command: Command, enabled: bool, cx: &mut Context<Self>| {
            div()
                .id(id)
                .w(rems(1.5))
                .h(rems(1.5))
                .flex()
                .flex_none()
                .items_center()
                .justify_center()
                .rounded_sm()
                .tooltip(self.tooltip(tip, Some(command)))
                .when(enabled, |d| d.cursor_pointer().hover(|s| s.bg(rgb(t.border))))
                .on_mouse_down(MouseButton::Left, cx.listener(|_, _: &MouseDownEvent, _, cx| cx.stop_propagation()))
                .on_click(cx.listener(move |this, _, _, cx| {
                    if enabled {
                        this.run_command(command, false, cx);
                    }
                }))
                .child(crate::icons::glyph(icon, 1.0, if enabled { t.text } else { t.dim }))
        };
        let header = div()
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            .pl_3()
            .pr_2()
            .py_1()
            .child(div().text_xs().text_color(rgb(t.dim)).child(match &c.convo.model {
                Some(model) => format!("CLAUDE  ·  {model}"),
                None => "CLAUDE".to_owned(),
            }))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .gap_0p5()
                    .child(icon_button("claude-history", crate::codicons::HISTORY, "Past Conversations", Command::ClaudeHistory, true, cx))
                    .child(icon_button("claude-stop", crate::codicons::DEBUG_STOP, "Stop", Command::ClaudeStop, busy, cx))
                    .child(icon_button("claude-new", crate::codicons::ADD, "New Conversation", Command::ClaudeNewChat, true, cx))
                    .child(icon_button("claude-close", crate::codicons::CLOSE, "Hide Claude Chat", Command::ToggleClaude, true, cx)),
            );

        let empty = c.convo.entries.is_empty();
        // The texts drawn below register themselves again, in order.
        c.texts.borrow_mut().clear();
        let mut list = div()
            .id("claude-scroll")
            .flex()
            .flex_col()
            .gap_3()
            .px_3()
            .pb_2()
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .track_scroll(&c.scroll)
            .cursor_text()
            .on_mouse_down(MouseButton::Left, cx.listener(|this, ev: &MouseDownEvent, _, cx| {
                this.claude_select_down(ev);
                cx.notify();
            }));
        if c.show_history {
            list = list.child(div().text_xs().text_color(rgb(t.dim)).child("PAST CONVERSATIONS"));
            list = if c.history.is_empty() {
                list.child(div().text_sm().text_color(rgb(t.dim)).child("Nothing yet. Conversations in this folder are saved by Claude Code and appear here."))
            } else {
                list.child(self.render_claude_history(&c.history, cx))
            };
            return self.claude_frame(header, list, cx);
        }
        if empty {
            let missing = self.claude.binary.is_none() && claude::find_claude(&self.settings.claude_path).is_none();
            list = list.child(
                div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .pt_2()
                    .text_sm()
                    .text_color(rgb(t.dim))
                    .child(div().text_color(rgb(t.text)).font_weight(FontWeight::SEMIBOLD).child("Ask Claude about this project"))
                    .child("It can read your files, search the code, and change them: you are asked before each edit or command. The open file and your selection go along with each question.")
                    .child("Alt+K adds the selection to the question as a reference.")
                    .when(missing, |d| d.child(div().text_color(rgb(t.warning)).child(INSTALL_HINT))),
            );
            if !c.history.is_empty() {
                list = list
                    .child(div().text_xs().text_color(rgb(t.dim)).child("RECENT"))
                    .child(self.render_claude_history(&c.history[..c.history.len().min(5)], cx));
            }
        }
        for (ix, entry) in c.convo.entries.iter().enumerate() {
            list = list.child(self.render_claude_entry(ix, entry, cx));
        }
        if let Some(request) = c.convo.pending.first() {
            list = list.child(self.render_claude_permission(request, cx));
        } else if busy {
            list = list.child(div().text_xs().text_color(rgb(t.dim)).child("Claude is working...  (Esc to stop)"));
        }

        // The mode, the context, and the question box.
        let ctx = self.claude_context();
        let context_label = match (&ctx.file, &ctx.selection) {
            (Some(file), Some((_, from, to))) if from == to => format!("{file}:{from}"),
            (Some(file), Some((_, from, to))) => format!("{file}:{from}-{to}"),
            (Some(file), None) => file.clone(),
            _ => String::new(),
        };
        let use_context = c.use_context;
        let chip = div()
            .id("claude-mode")
            .px_2()
            .py(px(1.0))
            .rounded_sm()
            .border_1()
            .border_color(rgb(t.border))
            .text_xs()
            .text_color(rgb(if matches!(c.mode, PermissionMode::Ask | PermissionMode::Auto) { t.dim } else { t.warning }))
            .cursor_pointer()
            .hover(|s| s.bg(rgb(t.border)))
            .tooltip(self.tooltip("Change what Claude may do without asking", Some(Command::ClaudeCycleMode)))
            .on_mouse_down(MouseButton::Left, cx.listener(|this, _: &MouseDownEvent, _, cx| {
                cx.stop_propagation();
                this.claude_cycle_mode();
                cx.notify();
            }))
            .child(c.mode.label());
        // The model and how hard it thinks: each opens a menu where the chip is.
        let picker = |id: &'static str, text: String, open: fn(&mut Workspace, gpui::Point<gpui::Pixels>, &mut Context<Workspace>), cx: &mut Context<Self>| {
            div()
                .id(id)
                .flex_none()
                .px_2()
                .py(px(1.0))
                .rounded_sm()
                .border_1()
                .border_color(rgb(t.border))
                .text_xs()
                .text_color(rgb(t.dim))
                .cursor_pointer()
                .hover(|s| s.bg(rgb(t.border)))
                .on_mouse_down(MouseButton::Left, cx.listener(move |this, ev: &MouseDownEvent, _, cx| {
                    cx.stop_propagation();
                    open(this, ev.position, cx);
                }))
                .child(text)
        };
        let model_chip = picker("claude-model", format!("Model: {}", claude::model_label(&self.settings.claude_model)), Workspace::open_claude_model_menu, cx);
        let effort_chip = picker("claude-effort", format!("Thinking: {}", claude::effort_label(&self.settings.claude_effort)), Workspace::open_claude_effort_menu, cx);
        let context_chip = (!context_label.is_empty()).then(|| {
            div()
                .id("claude-context")
                .min_w_0()
                .overflow_hidden()
                .whitespace_nowrap()
                .px_2()
                .py(px(1.0))
                .rounded_sm()
                .border_1()
                .border_color(rgb(t.border))
                .text_xs()
                .text_color(rgb(if use_context { t.text } else { t.dim }))
                .when(!use_context, |d| d.line_through())
                .cursor_pointer()
                .hover(|s| s.bg(rgb(t.border)))
                .tooltip(self.tooltip(if use_context { "Sent with the question. Click to leave it out" } else { "Not sent. Click to include it" }, None))
                .on_mouse_down(MouseButton::Left, cx.listener(|this, _: &MouseDownEvent, _, cx| {
                    cx.stop_propagation();
                    this.claude.use_context = !this.claude.use_context;
                    cx.notify();
                }))
                .child(context_label)
        });
        let send_enabled = !busy && !c.input.is_empty();
        let send = div()
            .id("claude-send")
            .absolute()
            .bottom(px(5.0))
            .right(px(5.0))
            .w(px(22.0))
            .h(px(22.0))
            .flex()
            .items_center()
            .justify_center()
            .rounded(px(4.0))
            .when(send_enabled, |d| d.cursor_pointer().hover(|s| s.bg(rgb(t.border))))
            .tooltip(self.tooltip("Send (Enter)", None))
            .on_mouse_down(MouseButton::Left, cx.listener(|_, _: &MouseDownEvent, _, cx| cx.stop_propagation()))
            .on_click(cx.listener(move |this, _, _, cx| {
                this.claude_send();
                cx.notify();
            }))
            .child(crate::icons::glyph(crate::codicons::SEND, 1.0, if send_enabled { t.text } else { t.dim }));
        let input = self.render_text_area(AreaId::Claude, Focus::Claude, "claude-input", "Ask Claude...", vec![send.into_any_element()], 26.0, cx);
        // Attach: the system's file dialog; pictures can also be pasted or dropped on the chat.
        let attach = div()
            .id("claude-attach")
            .w(px(22.0))
            .h(px(22.0))
            .flex()
            .flex_none()
            .items_center()
            .justify_center()
            .rounded_sm()
            .cursor_pointer()
            .hover(|s| s.bg(rgb(t.border)))
            .tooltip(self.tooltip("Attach files or pictures (you can also paste or drop them)", None))
            .on_mouse_down(MouseButton::Left, cx.listener(|_, _: &MouseDownEvent, _, cx| cx.stop_propagation()))
            .on_click(cx.listener(|this, _, _, cx| this.claude_pick_files(cx)))
            .child(crate::icons::glyph(crate::codicons::ATTACH, 0.9375, t.dim));
        let picture_chips: Vec<AnyElement> = c
            .attachments
            .iter()
            .enumerate()
            .map(|(i, a)| {
                div()
                    .id(("claude-picture", i))
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_1()
                    .min_w_0()
                    .px_2()
                    .py(px(1.0))
                    .rounded_sm()
                    .border_1()
                    .border_color(rgb(t.border))
                    .text_xs()
                    .text_color(rgb(t.text))
                    .child(crate::icons::glyph(crate::codicons::FILE_MEDIA, 0.75, t.dim))
                    .child(div().min_w_0().overflow_hidden().whitespace_nowrap().child(a.name.clone()))
                    .child(
                        div()
                            .id(("claude-picture-x", i))
                            .w(px(14.0))
                            .h(px(14.0))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded_sm()
                            .cursor_pointer()
                            .hover(|s| s.bg(rgb(t.border)))
                            .tooltip(self.tooltip("Remove", None))
                            .on_mouse_down(MouseButton::Left, cx.listener(|_, _: &MouseDownEvent, _, cx| cx.stop_propagation()))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if i < this.claude.attachments.len() {
                                    this.claude.attachments.remove(i);
                                }
                                cx.notify();
                            }))
                            .child(crate::icons::glyph(crate::codicons::CLOSE, 0.625, t.dim)),
                    )
                    .into_any_element()
            })
            .collect();
        let suggestions = c.mention.as_ref().map(|m| {
            div()
                .flex()
                .flex_col()
                .py_1()
                .rounded_md()
                .border_1()
                .border_color(rgb(t.border))
                .bg(rgb(t.popup))
                .shadow_sm()
                .children(m.items.iter().enumerate().map(|(i, label)| {
                    div()
                        .id(("claude-mention", i))
                        .px_2()
                        .py(px(2.0))
                        .text_xs()
                        .text_color(rgb(t.text))
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .cursor_pointer()
                        .when(i == m.selected, |d| d.bg(rgb(t.selection)))
                        .when(i != m.selected, |d| d.hover(|s| s.bg(rgb(t.tab_active))))
                        .on_mouse_down(MouseButton::Left, cx.listener(|_, _: &MouseDownEvent, _, cx| cx.stop_propagation()))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if let Some(m) = this.claude.mention.as_mut() {
                                m.selected = i;
                            }
                            this.claude_accept_mention();
                            cx.notify();
                        }))
                        .child(label.clone())
                }))
                .into_any_element()
        });

        let mut chips = vec![chip.into_any_element(), model_chip.into_any_element(), effort_chip.into_any_element(), attach.into_any_element()];
        chips.extend(picture_chips);
        self.claude_frame_with_input(header, list, chips, context_chip, suggestions, input)
    }

    /// The panel with only a list in it (no question box): the past conversations.
    fn claude_frame(&self, header: impl IntoElement, list: impl IntoElement, _cx: &mut Context<Self>) -> AnyElement {
        div().flex().flex_col().flex_1().min_h_0().child(header).child(list).into_any_element()
    }

    fn claude_frame_with_input(&self, header: impl IntoElement, list: impl IntoElement, chips: Vec<AnyElement>, context_chip: Option<impl IntoElement>, suggestions: Option<AnyElement>, input: impl IntoElement) -> AnyElement {
        let t = &self.theme;
        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .child(header)
            .child(list)
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_none()
                    .gap_1()
                    .p(px(10.0))
                    .child(div().flex().flex_row().flex_wrap().items_center().gap_1().children(chips).children(context_chip))
                    .children(suggestions)
                    .child(input)
                    .child(div().text_xs().text_color(rgb(t.dim)).child("Enter to send, Shift+Enter for a new line")),
            )
            .into_any_element()
    }

    fn render_claude_entry(&self, ix: usize, entry: &Entry, cx: &mut Context<Self>) -> AnyElement {
        let t = self.theme.clone();
        match entry {
            Entry::User(text) => div()
                .w_full()
                .px_2()
                .py_1()
                .rounded_md()
                .bg(rgb(t.tab_active))
                .text_sm()
                .text_color(rgb(t.text))
                .child(self.claude_text(text))
                .into_any_element(),
            Entry::Assistant { text, .. } => {
                let blocks = self.claude.blocks.get(ix).map(|b| b.1.as_slice()).unwrap_or_default();
                let body: Vec<AnyElement> = if blocks.is_empty() {
                    vec![div().text_sm().child(self.claude_text(text)).into_any_element()]
                } else {
                    blocks.iter().enumerate().map(|(bi, b)| self.claude_markdown_block(b, self.root().to_path_buf(), ix * 1000 + bi * 40, cx)).collect()
                };
                let copy_text = text.clone();
                div()
                    .group("claude-answer")
                    .relative()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .w_full()
                    .text_sm()
                    .text_color(rgb(t.text))
                    .children(body)
                    .child(
                        div()
                            .id(("claude-copy", ix))
                            .absolute()
                            .top_0()
                            .right_0()
                            .w(px(20.0))
                            .h(px(20.0))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded_sm()
                            .bg(rgb(t.sidebar))
                            .opacity(0.0)
                            .group_hover("claude-answer", |s| s.opacity(1.0))
                            .cursor_pointer()
                            .hover(|s| s.bg(rgb(t.border)))
                            .tooltip(self.tooltip("Copy", None))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                cx.write_to_clipboard(gpui::ClipboardItem::new_string(copy_text.clone()));
                                this.set_status("Copied Claude's answer");
                                cx.notify();
                            }))
                            .child(crate::icons::glyph(crate::codicons::COPY, 0.75, t.dim)),
                    )
                    .into_any_element()
            }
            Entry::Tool { .. } => self.render_claude_tool(ix, entry, cx),
            Entry::Thinking { text, streaming } => self.render_claude_thinking(ix, text, *streaming, cx),
            Entry::Notice(text) => div().w_full().text_xs().text_color(rgb(t.warning)).child(self.claude_text(text)).into_any_element(),
        }
    }
}

/// A file's text, when it is there and is text of a sensible size.
fn read_text(path: &str) -> Option<String> {
    const LIMIT: u64 = 2_000_000;
    let meta = std::fs::metadata(path).ok()?;
    if !meta.is_file() || meta.len() > LIMIT {
        return None;
    }
    String::from_utf8(std::fs::read(path).ok()?).ok()
}
