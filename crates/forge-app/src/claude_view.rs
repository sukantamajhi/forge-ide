//! The Claude chat in the side bar: ask about the code, let Claude read and change it. It drives the `claude`
//! program (see `forge_core::claude`), so it uses the user's own Claude Code sign-in and settings.

use std::path::PathBuf;

use forge_core::claude::{self, Conversation, EditorContext, Entry, Message, Options, PermissionMode, Run};
use forge_core::commands::Command;
use forge_core::markdown::{self, Block};
use forge_core::input::Outcome;
use forge_core::textarea::TextArea;
use gpui::{div, point, prelude::*, px, rems, rgb, AnyElement, Context, FontWeight, Keystroke, MouseButton, MouseDownEvent, ScrollHandle};

use crate::workspace::{AreaId, Focus, SidebarView, Workspace};

/// Where to send someone who does not have the program.
const INSTALL_HINT: &str = "Claude Code is not installed. Install it with:  npm install -g @anthropic-ai/claude-code  (see docs.claude.com/claude-code), sign in once by running `claude` in a terminal, then try again. If it is installed somewhere unusual, set \"claude.path\" in settings.json.";

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
        }
    }
}

impl Workspace {
    // ---- commands ----

    /// Show the chat and put the caret in its question box.
    pub fn open_claude(&mut self) {
        self.sidebar_visible = true;
        self.sidebar_view = SidebarView::Claude;
        self.focus = Focus::Claude;
        let _ = self.claude_binary();
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
        self.claude.blocks.clear();
        self.claude.stderr.clear();
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
        if doc.untitled || doc.path.as_os_str().is_empty() {
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
        let opts = Options { binary, cwd: self.root().to_path_buf(), mode: self.claude.mode, model: self.settings.claude_model.clone(), resume: self.claude.convo.session_id.clone() };
        self.claude.convo.ask(&question);
        self.claude.input.set_text("");
        self.claude.stderr.clear();
        match claude::start(&opts, &prompt) {
            Ok(run) => self.claude.run = Some(run),
            Err(error) => {
                self.claude.convo.busy = false;
                self.claude.convo.notice(error);
            }
        }
        self.claude_refresh_blocks();
        self.scroll_claude_to_bottom();
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
                    finished |= matches!(event, claude::Event::Done { .. });
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
            if self.claude.convo.busy {
                // It ended without saying it was done: it could not start, or it was killed.
                self.claude.convo.busy = false;
                let why = self.claude.stderr.join("\n");
                self.claude.convo.notice(if why.trim().is_empty() { "Claude stopped without an answer.".to_owned() } else { why });
                finished = true;
            }
        }
        if finished {
            self.claude_explain_refusals();
            self.claude_after_turn();
        }
        self.claude_refresh_blocks();
        if follow || finished {
            self.scroll_claude_to_bottom();
        }
        true
    }

    /// Claude changed files, perhaps: show them as they are now.
    fn claude_after_turn(&mut self) {
        self.reload_clean_docs();
        self.tree.refresh();
        self.git_request_refresh();
    }

    /// In read-only mode a refused edit is the expected outcome: say how to allow it.
    fn claude_explain_refusals(&mut self) {
        if self.claude.mode != PermissionMode::ReadOnly {
            return;
        }
        let asked_at = self.claude.convo.entries.iter().rposition(|e| matches!(e, Entry::User(_))).unwrap_or(0);
        let refused = self.claude.convo.entries[asked_at..].iter().any(|e| {
            matches!(e, Entry::Tool { name, failed: true, result: Some(r), .. }
                if matches!(name.as_str(), "Edit" | "MultiEdit" | "Write" | "NotebookEdit" | "Bash") && r.to_ascii_lowercase().contains("permission"))
        });
        if refused {
            self.claude.convo.notice("Claude needed permission to change files or run commands. Change the mode above the question box (Shift+Tab) to \"Edit automatically\" and ask again.");
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
        let inner = self.sidebar_width() - 1.0 - 2.0 * 10.0 - 2.0 * (1.0 + 10.0);
        ((inner / f32::from(self.char_width).max(1.0)).floor() as usize).max(8)
    }

    // ---- keys ----

    pub(crate) fn claude_key(&mut self, k: &Keystroke, cx: &mut Context<Self>) -> bool {
        let ctrl = k.modifiers.control || k.modifiers.platform;
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
                }
                return true;
            }
            ("v", true, _) => {
                if let Some(text) = cx.read_from_clipboard().and_then(|i| i.text()) {
                    self.claude.input.insert(&text);
                }
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
        matches!(self.claude.input.apply(&k.key, ctrl, k.modifiers.shift, k.key_char.as_deref()), Outcome::Edited | Outcome::Moved | Outcome::Submit)
    }

    // ---- drawing ----

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
                    .child(icon_button("claude-stop", crate::codicons::DEBUG_STOP, "Stop", Command::ClaudeStop, busy, cx))
                    .child(icon_button("claude-new", crate::codicons::ADD, "New Conversation", Command::ClaudeNewChat, true, cx)),
            );

        let empty = c.convo.entries.is_empty();
        let mut list = div().id("claude-scroll").flex().flex_col().gap_3().px_3().pb_2().flex_1().min_h_0().overflow_y_scroll().track_scroll(&c.scroll);
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
                    .child("It can read your files, search the code, and with \"Edit automatically\" change them. The open file and your selection go along with each question.")
                    .child("Alt+K adds the selection to the question as a reference.")
                    .when(missing, |d| d.child(div().text_color(rgb(t.warning)).child(INSTALL_HINT))),
            );
        }
        for (ix, entry) in c.convo.entries.iter().enumerate() {
            list = list.child(self.render_claude_entry(ix, entry, cx));
        }
        if busy {
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
            .text_color(rgb(if c.mode == PermissionMode::ReadOnly { t.dim } else { t.warning }))
            .cursor_pointer()
            .hover(|s| s.bg(rgb(t.border)))
            .tooltip(self.tooltip("Change what Claude may do without asking", Some(Command::ClaudeCycleMode)))
            .on_mouse_down(MouseButton::Left, cx.listener(|this, _: &MouseDownEvent, _, cx| {
                cx.stop_propagation();
                this.claude_cycle_mode();
                cx.notify();
            }))
            .child(c.mode.label());
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
                    .child(div().flex().flex_row().items_center().gap_1().child(chip).children(context_chip))
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
                .child(text.clone())
                .into_any_element(),
            Entry::Assistant { text, .. } => {
                let blocks = self.claude.blocks.get(ix).map(|b| b.1.as_slice()).unwrap_or_default();
                let body: Vec<AnyElement> = if blocks.is_empty() {
                    vec![div().text_sm().child(text.clone()).into_any_element()]
                } else {
                    blocks.iter().enumerate().map(|(bi, b)| self.markdown_block(b, self.root().to_path_buf(), ix * 1000 + bi * 40, cx)).collect()
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
            Entry::Tool { name, summary, path, result, failed, .. } => {
                let icon = match name.as_str() {
                    "Read" => crate::codicons::EYE,
                    "Edit" | "MultiEdit" | "Write" | "NotebookEdit" => crate::codicons::EDIT,
                    "Bash" => crate::codicons::TERMINAL,
                    "Grep" | "Glob" => crate::codicons::SEARCH,
                    "WebFetch" | "WebSearch" => crate::codicons::GLOBE,
                    _ => crate::codicons::TOOLS,
                };
                let shown = match path {
                    Some(p) => claude::display_path(p, self.root()),
                    None => summary.clone(),
                };
                let open = path.clone().map(PathBuf::from);
                let color = if *failed { t.error } else { t.dim };
                let detail = (*failed).then(|| result.clone()).flatten().map(|r| r.lines().next().unwrap_or_default().chars().take(160).collect::<String>());
                div()
                    .flex()
                    .flex_col()
                    .w_full()
                    .child(
                        div()
                            .id(("claude-tool", ix))
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap_1()
                            .text_xs()
                            .text_color(rgb(color))
                            .when(open.is_some(), |d| d.cursor_pointer().hover(|s| s.text_color(rgb(t.text))))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if let Some(path) = &open {
                                    this.open_file(path, None, cx);
                                    cx.notify();
                                }
                            }))
                            .child(crate::icons::glyph(icon, 0.875, color))
                            .child(div().flex_none().font_weight(FontWeight::MEDIUM).child(name.clone()))
                            .child(div().min_w_0().flex_1().overflow_hidden().whitespace_nowrap().child(shown)),
                    )
                    .children(detail.map(|d| div().pl_4().text_xs().text_color(rgb(t.error)).child(d)))
                    .into_any_element()
            }
            Entry::Notice(text) => div().w_full().text_xs().text_color(rgb(t.warning)).child(text.clone()).into_any_element(),
        }
    }
}
