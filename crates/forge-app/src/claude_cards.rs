//! The pieces of the Claude chat that are not plain text: what Claude asks permission for, the diff of an edit, a tool
//! call, and the list of past conversations.

use std::path::PathBuf;

use forge_core::claude::{self, DiffRow, Entry, PermissionRequest, RowKind, SessionInfo};
use forge_core::markdown;
use gpui::{div, prelude::*, px, rgb, rgba, AnyElement, Context, Div, FontWeight, MouseButton, MouseDownEvent, Stateful};

use crate::workspace::Workspace;

/// The most diff rows drawn inline; the rest are one click away in the diff tab.
const MAX_INLINE_ROWS: usize = 40;

impl Workspace {
    /// A button in a card. `primary` is the one to press.
    fn claude_button(&self, id: impl Into<gpui::ElementId>, label: impl Into<gpui::SharedString>, primary: bool, cx: &mut Context<Self>, on_click: impl Fn(&mut Workspace) + 'static) -> Stateful<Div> {
        let t = &self.theme;
        div()
            .id(id)
            .px_2()
            .py(px(2.0))
            .rounded_sm()
            .text_xs()
            .border_1()
            .border_color(rgb(if primary { t.button } else { t.border }))
            .bg(rgb(if primary { t.button } else { t.sidebar }))
            .text_color(rgb(if primary { t.button_text } else { t.text }))
            .cursor_pointer()
            .hover(|s| s.opacity(0.85))
            .on_mouse_down(MouseButton::Left, cx.listener(|_, _: &MouseDownEvent, _, cx| cx.stop_propagation()))
            .on_click(cx.listener(move |this, _, _, cx| {
                on_click(this);
                cx.notify();
            }))
            .child(label.into())
    }

    /// Changed lines, tinted: added in green, removed in red, with `...` where unchanged lines were left out.
    pub(crate) fn render_diff_rows(&self, rows: &[DiffRow]) -> Div {
        let t = &self.theme;
        let tint = |color: u32| rgba((color << 8) | 0x30);
        let shown = rows.iter().take(MAX_INLINE_ROWS).map(|row| {
            let (sign, bg) = match row.kind {
                RowKind::Add => ("+", Some(tint(t.git_added))),
                RowKind::Del => ("-", Some(tint(t.git_deleted))),
                RowKind::Same => (" ", None),
                RowKind::Gap => ("\u{22ef}", None),
            };
            let line = if row.kind == RowKind::Gap { String::new() } else { row.text.clone() };
            div()
                .flex()
                .flex_row()
                .gap_1()
                .px_1()
                .w_full()
                .when_some(bg, |d, color| d.bg(color))
                .child(div().w(px(10.0)).flex_none().text_color(rgb(t.dim)).child(sign))
                .child(div().min_w_0().flex_1().overflow_hidden().whitespace_nowrap().child(line))
        });
        let more = rows.len().saturating_sub(MAX_INLINE_ROWS);
        div()
            .flex()
            .flex_col()
            .w_full()
            .py_1()
            .rounded_sm()
            .border_1()
            .border_color(rgb(t.border))
            .bg(rgb(t.bg))
            .font_family(self.code_font.clone())
            .text_xs()
            .overflow_hidden()
            .children(shown)
            .children((more > 0).then(|| div().px_1().text_color(rgb(t.dim)).child(format!("... {more} more lines"))))
    }

    /// Claude's thinking: open while it is being written, then folded into one line that opens on a click.
    pub(crate) fn render_claude_thinking(&self, ix: usize, text: &str, streaming: bool, cx: &mut Context<Self>) -> AnyElement {
        let t = self.theme.clone();
        let open = streaming || self.claude.open_thoughts.contains(&ix);
        let first = text.lines().find(|l| !l.trim().is_empty()).unwrap_or_default().trim().to_owned();
        let title = if streaming { "Thinking...".to_owned() } else { "Thought".to_owned() };
        div()
            .flex()
            .flex_col()
            .gap_1()
            .w_full()
            .child(
                div()
                    .id(("claude-thought", ix))
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_1()
                    .text_xs()
                    .text_color(rgb(t.dim))
                    .cursor_pointer()
                    .hover(|s| s.text_color(rgb(t.text)))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.claude_toggle_thought(ix);
                        cx.notify();
                    }))
                    .child(crate::icons::glyph(if open { crate::codicons::CHEVRON_DOWN } else { crate::codicons::CHEVRON_RIGHT }, 0.75, t.dim))
                    .child(div().flex_none().italic().child(title))
                    .children((!open).then(|| div().min_w_0().flex_1().overflow_hidden().whitespace_nowrap().text_ellipsis().child(first))),
            )
            .children(open.then(|| {
                div()
                    .ml(px(5.0))
                    .pl_2()
                    .border_l_1()
                    .border_color(rgb(t.border))
                    .text_xs()
                    .italic()
                    .text_color(rgb(t.dim))
                    .whitespace_normal()
                    .child(text.trim_end().to_owned())
            }))
            .into_any_element()
    }

    /// A tool call: what it did, in a line; for an edit, what it changed (click to see it).
    pub(crate) fn render_claude_tool(&self, ix: usize, entry: &Entry, cx: &mut Context<Self>) -> AnyElement {
        let t = self.theme.clone();
        let Entry::Tool { id, name, summary, path, input, result, failed } = entry else { return div().into_any_element() };
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
        let color = if *failed { t.error } else { t.dim };
        let detail = (*failed).then(|| result.clone()).flatten().map(|r| r.lines().next().unwrap_or_default().chars().take(160).collect::<String>());
        // What the tool printed: a command's output, search hits, a file read, an error message in full.
        let has_output = !claude::is_edit_tool(name) && result.as_ref().is_some_and(|r| !r.trim().is_empty());
        let open_output = has_output && self.claude.shown_output.contains(id);
        let output_id = id.clone();
        let row_output_id = id.clone();

        // What an edit changed: from the file before and after, else from the tool's own input.
        let edit = claude::is_edit_tool(name) && !*failed && result.is_some();
        let rows: Vec<DiffRow> = if edit { self.claude.diffs.get(id).cloned().unwrap_or_else(|| claude::input_diff(name, input)) } else { Vec::new() };
        let (added, removed) = claude::diff_counts(&rows);
        let open_rows = edit && !rows.is_empty() && self.claude.expanded.contains(id);
        let can_diff = edit && !rows.is_empty();
        let has_snapshot = self.claude.snapshots.contains_key(id);

        let open_path = path.clone().map(PathBuf::from);
        let toggle_id = id.clone();
        let diff_id = id.clone();
        let file_path = open_path.clone();
        let small_button = |id: (&'static str, usize), glyph: char, tip: &'static str, cx: &mut Context<Self>, act: Box<dyn Fn(&mut Workspace, &mut Context<Workspace>)>| {
            div()
                .id(id)
                .w(px(18.0))
                .h(px(18.0))
                .flex()
                .flex_none()
                .items_center()
                .justify_center()
                .rounded_sm()
                .cursor_pointer()
                .hover(|s| s.bg(rgb(t.border)))
                .tooltip(self.tooltip(tip, None))
                .on_mouse_down(MouseButton::Left, cx.listener(|_, _: &MouseDownEvent, _, cx| cx.stop_propagation()))
                .on_click(cx.listener(move |this, _, _, cx| {
                    act(this, cx);
                    cx.notify();
                }))
                .child(crate::icons::glyph(glyph, 0.75, t.dim))
        };

        let row = div()
            .id(("claude-tool", ix))
            .flex()
            .flex_row()
            .items_center()
            .gap_1()
            .text_xs()
            .text_color(rgb(color))
            .cursor_pointer()
            .hover(|s| s.text_color(rgb(t.text)))
            .on_click(cx.listener(move |this, _, _, cx| {
                if can_diff {
                    this.claude_toggle_diff(&toggle_id);
                } else if let Some(path) = &open_path {
                    this.open_file(path, None, cx);
                } else if has_output {
                    this.claude_toggle_output(&row_output_id);
                }
                cx.notify();
            }))
            .when(has_output, |d| {
                d.child(
                    div()
                        .id(("claude-output-toggle", ix))
                        .w(px(14.0))
                        .h(px(14.0))
                        .flex()
                        .flex_none()
                        .items_center()
                        .justify_center()
                        .rounded_sm()
                        .hover(|s| s.bg(rgb(t.border)))
                        .tooltip(self.tooltip(if open_output { "Hide output" } else { "Show output" }, None))
                        .on_mouse_down(MouseButton::Left, cx.listener(|_, _: &MouseDownEvent, _, cx| cx.stop_propagation()))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            cx.stop_propagation();
                            this.claude_toggle_output(&output_id);
                            cx.notify();
                        }))
                        .child(crate::icons::glyph(if open_output { crate::codicons::CHEVRON_DOWN } else { crate::codicons::CHEVRON_RIGHT }, 0.75, color)),
                )
            })
            .when(can_diff, |d| d.child(crate::icons::glyph(if open_rows { crate::codicons::CHEVRON_DOWN } else { crate::codicons::CHEVRON_RIGHT }, 0.75, color)))
            .child(crate::icons::glyph(icon, 0.875, color))
            .child(div().flex_none().font_weight(FontWeight::MEDIUM).child(name.clone()))
            .child(div().min_w_0().flex_1().overflow_hidden().whitespace_nowrap().child(shown))
            .when(can_diff && (added > 0 || removed > 0), |d| {
                d.child(div().flex_none().text_color(rgb(t.git_added)).child(format!("+{added}"))).child(div().flex_none().text_color(rgb(t.git_deleted)).child(format!("\u{2212}{removed}")))
            })
            .when(edit && file_path.is_some(), |d| {
                let open = file_path.clone();
                d.child(small_button(("claude-open", ix), crate::codicons::GO_TO_FILE, "Open File", cx, Box::new(move |this, cx| {
                    if let Some(path) = &open {
                        this.open_file(path, None, cx);
                    }
                })))
            })
            .when(can_diff && has_snapshot, |d| d.child(small_button(("claude-diff", ix), crate::codicons::DIFF, "Open Diff", cx, Box::new(move |this, cx| this.claude_open_diff(&diff_id, cx)))));

        div()
            .flex()
            .flex_col()
            .gap_1()
            .w_full()
            .child(row)
            .children(detail.filter(|_| !open_output).map(|d| div().pl_4().text_xs().text_color(rgb(t.error)).child(d)))
            .children(open_output.then(|| {
                let text = result.clone().unwrap_or_default();
                let cut = text.chars().count() >= claude::MAX_RESULT_CHARS;
                div()
                    .id(("claude-output", ix))
                    .ml_4()
                    .p_1()
                    .rounded_sm()
                    .border_1()
                    .border_color(rgb(t.border))
                    .bg(rgb(t.bg))
                    .font_family(self.code_font.clone())
                    .text_xs()
                    .text_color(rgb(if *failed { t.error } else { t.dim }))
                    .max_h(px(280.0))
                    .overflow_y_scroll()
                    .flex()
                    .flex_col()
                    .children(text.lines().map(|l| self.claude_code_line(l)).collect::<Vec<_>>())
                    .children(cut.then(|| div().pt_1().text_color(rgb(t.dim)).child(format!("... the output is cut after {} characters", claude::MAX_RESULT_CHARS))))
            }))
            .children(open_rows.then(|| self.render_diff_rows(&rows)))
            .into_any_element()
    }

    /// Run what a card button says.
    pub(crate) fn claude_run_action(&mut self, request: &PermissionRequest, kind: &claude::CardKind) {
        use claude::CardKind::*;
        match kind {
            Allow => self.claude_allow(request, false),
            AllowAndRemember => self.claude_allow(request, true),
            Deny => self.claude_deny(request, "The user denied this action."),
            PlanAuto => self.claude_approve_plan(request, true),
            PlanAsk => self.claude_approve_plan(request, false),
            PlanKeep => self.claude_deny(request, "The user wants to keep planning. Revise the plan."),
            Option { question, label, multi } => self.claude_pick(request, question, label, *multi),
            SendAnswers => self.claude_send_answers(request),
            Skip => self.claude_deny(request, "The user chose not to answer."),
        }
    }

    /// A card button with the key that presses it.
    fn claude_action_button(&self, id: impl Into<gpui::ElementId>, label: &str, key: Option<usize>, primary: bool, cx: &mut Context<Self>, on_click: impl Fn(&mut Workspace) + 'static) -> Stateful<Div> {
        let t = &self.theme;
        self.claude_button(id, label.to_owned(), primary, cx, on_click).children(key.filter(|n| *n < 9).map(|n| {
            div().ml_1().px(px(4.0)).rounded_sm().bg(rgba(0xffffff1f)).text_color(rgb(if primary { t.button_text } else { t.dim })).child((n + 1).to_string())
        }))
    }

    /// What Claude is waiting on: a yes or no, a plan to approve, or a question to answer. The keyboard answers too:
    /// the number on a button, Enter for the main one, Escape for no.
    pub(crate) fn render_claude_permission(&self, request: &PermissionRequest, cx: &mut Context<Self>) -> AnyElement {
        let t = self.theme.clone();
        let req = request.clone();
        let file = request.input["file_path"].as_str().map(|p| claude::display_path(p, self.root()));
        let more_waiting = self.claude.convo.pending.len().saturating_sub(1);
        let actions = claude::card_actions(request);
        let button = |index: usize, cx: &mut Context<Self>| {
            let action = actions[index].clone();
            let (r, kind) = (req.clone(), action.kind.clone());
            let picked = match &action.kind {
                claude::CardKind::Option { question, label, .. } => self.claude.answers.get(question).is_some_and(|a| a.contains(label)),
                _ => action.primary,
            };
            self.claude_action_button(("claude-card", index), &action.label, Some(index), picked, cx, move |this| this.claude_run_action(&r, &kind)).into_any_element()
        };
        let plain_buttons = |cx: &mut Context<Self>| -> Vec<AnyElement> { (0..actions.len()).filter(|i| !matches!(actions[*i].kind, claude::CardKind::Option { .. })).map(|i| button(i, cx)).collect() };

        let (title, body, buttons): (String, Option<AnyElement>, Vec<AnyElement>) = match request.tool_name.as_str() {
            "ExitPlanMode" => {
                let plan = request.input["plan"].as_str().unwrap_or_default();
                let blocks: Vec<AnyElement> = markdown::parse(plan).iter().enumerate().map(|(i, b)| self.claude_markdown_block(b, self.root().to_path_buf(), 900_000 + i * 40, cx)).collect();
                ("Claude has a plan".to_owned(), Some(div().flex().flex_col().gap_1().max_h(px(260.0)).overflow_hidden().text_sm().children(blocks).into_any_element()), plain_buttons(cx))
            }
            "AskUserQuestion" => {
                let questions = request.input["questions"].as_array().cloned().unwrap_or_default();
                let mut col = div().flex().flex_col().gap_2();
                // The options are numbered across all the questions, in the order the buttons are listed.
                let mut next = 0;
                for q in &questions {
                    let text = q["question"].as_str().unwrap_or_default().to_owned();
                    let multi = q["multiSelect"].as_bool().unwrap_or(false);
                    let mut options = div().flex().flex_row().flex_wrap().gap_1();
                    for _ in q["options"].as_array().into_iter().flatten() {
                        options = options.child(button(next, cx));
                        next += 1;
                    }
                    col = col.child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .child(div().text_sm().font_weight(FontWeight::SEMIBOLD).child(self.claude_text(&text)))
                            .children((multi).then(|| div().text_xs().text_color(rgb(t.dim)).child("Pick any number")))
                            .child(options),
                    );
                }
                ("Claude has a question".to_owned(), Some(col.into_any_element()), plain_buttons(cx))
            }
            name => {
                let title = match name {
                    "Edit" | "MultiEdit" => format!("Edit {}?", file.clone().unwrap_or_default()),
                    "Write" => format!("Write {}?", file.clone().unwrap_or_default()),
                    "NotebookEdit" => format!("Edit {}?", file.clone().unwrap_or_default()),
                    "Bash" => "Run this command?".to_owned(),
                    "WebFetch" => "Fetch this page?".to_owned(),
                    other => format!("Use {other}?"),
                };
                let body: Option<AnyElement> = if claude::is_edit_tool(name) {
                    self.claude.previews.get(&request.id).filter(|r| !r.is_empty()).map(|rows| self.render_diff_rows(rows).into_any_element())
                } else if name == "Bash" {
                    let command = request.input["command"].as_str().unwrap_or_default().to_owned();
                    Some(
                        div()
                            .px_2()
                            .py_1()
                            .rounded_sm()
                            .border_1()
                            .border_color(rgb(t.border))
                            .bg(rgb(t.bg))
                            .font_family(self.code_font.clone())
                            .text_xs()
                            .whitespace_normal()
                            .child(command)
                            .into_any_element(),
                    )
                } else {
                    let detail = request.input["url"].as_str().map(str::to_owned).unwrap_or_else(|| request.input.to_string().chars().take(300).collect());
                    Some(div().text_xs().text_color(rgb(t.dim)).child(detail).into_any_element())
                };
                (title, body, plain_buttons(cx))
            }
        };
        let typing = !self.claude.input.is_empty();
        let hint = if typing { "Clear the question box to answer from the keyboard".to_owned() } else { "Press the number on a button, Enter for the main one, Esc for no".to_owned() };

        div()
            .id("claude-card")
            .flex()
            .flex_col()
            .gap_2()
            .w_full()
            .p_2()
            .rounded_md()
            .border_1()
            .border_color(rgb(t.warning))
            .bg(rgb(t.sidebar))
            // A click on the card gives the chat the keyboard, so the keys above reach it.
            .on_mouse_down(MouseButton::Left, cx.listener(|this, _: &MouseDownEvent, _, cx| {
                this.focus = crate::workspace::Focus::Claude;
                cx.notify();
            }))
            .child(div().text_sm().font_weight(FontWeight::SEMIBOLD).text_color(rgb(t.text)).child(title))
            .children(body)
            .child(div().flex().flex_row().flex_wrap().gap_1().children(buttons))
            .child(div().text_xs().text_color(rgb(t.dim)).child(hint))
            .children((more_waiting > 0).then(|| div().text_xs().text_color(rgb(t.dim)).child(format!("{more_waiting} more waiting"))))
            .into_any_element()
    }

    /// The saved conversations of this project: click one to read it and carry on.
    pub(crate) fn render_claude_history(&self, sessions: &[SessionInfo], cx: &mut Context<Self>) -> Div {
        let t = self.theme.clone();
        let now = std::time::SystemTime::now();
        let rows = sessions.iter().enumerate().map(|(i, s)| {
            let session = s.clone();
            div()
                .id(("claude-session", i))
                .flex()
                .flex_col()
                .w_full()
                .px_2()
                .py_1()
                .rounded_sm()
                .cursor_pointer()
                .hover(|s| s.bg(rgb(t.tab_active)))
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.claude_resume(&session);
                    cx.notify();
                }))
                .child(div().text_sm().text_color(rgb(t.text)).overflow_hidden().whitespace_nowrap().text_ellipsis().child(s.title.clone()))
                .child(div().text_xs().text_color(rgb(t.dim)).child(claude::ago(s.modified, now)))
        });
        div().flex().flex_col().gap_0p5().children(rows)
    }
}
