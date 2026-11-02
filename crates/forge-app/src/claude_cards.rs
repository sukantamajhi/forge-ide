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
                }
                cx.notify();
            }))
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
            .children(detail.map(|d| div().pl_4().text_xs().text_color(rgb(t.error)).child(d)))
            .children(open_rows.then(|| self.render_diff_rows(&rows)))
            .into_any_element()
    }

    /// What Claude is waiting on: a yes or no, a plan to approve, or a question to answer.
    pub(crate) fn render_claude_permission(&self, request: &PermissionRequest, cx: &mut Context<Self>) -> AnyElement {
        let t = self.theme.clone();
        let req = request.clone();
        let file = request.input["file_path"].as_str().map(|p| claude::display_path(p, self.root()));
        let more_waiting = self.claude.convo.pending.len().saturating_sub(1);

        let (title, body, buttons): (String, Option<AnyElement>, Vec<AnyElement>) = match request.tool_name.as_str() {
            "ExitPlanMode" => {
                let plan = request.input["plan"].as_str().unwrap_or_default();
                let blocks: Vec<AnyElement> = markdown::parse(plan).iter().enumerate().map(|(i, b)| self.markdown_block(b, self.root().to_path_buf(), 900_000 + i * 40, cx)).collect();
                let (a, b, c) = (req.clone(), req.clone(), req.clone());
                (
                    "Claude has a plan".to_owned(),
                    Some(div().flex().flex_col().gap_1().max_h(px(260.0)).overflow_hidden().text_sm().children(blocks).into_any_element()),
                    vec![
                        self.claude_button("claude-plan-auto", "Approve, edit automatically", true, cx, move |this| this.claude_approve_plan(&a, true)).into_any_element(),
                        self.claude_button("claude-plan-ask", "Approve, ask before edits", false, cx, move |this| this.claude_approve_plan(&b, false)).into_any_element(),
                        self.claude_button("claude-plan-no", "Keep planning", false, cx, move |this| this.claude_deny(&c, "The user wants to keep planning. Revise the plan.")).into_any_element(),
                    ],
                )
            }
            "AskUserQuestion" => {
                let questions = request.input["questions"].as_array().cloned().unwrap_or_default();
                let any_multi = questions.iter().any(|q| q["multiSelect"].as_bool().unwrap_or(false));
                let mut col = div().flex().flex_col().gap_2();
                for (qi, q) in questions.iter().enumerate() {
                    let text = q["question"].as_str().unwrap_or_default().to_owned();
                    let multi = q["multiSelect"].as_bool().unwrap_or(false);
                    let picked = self.claude.answers.get(&text).cloned().unwrap_or_default();
                    let mut options = div().flex().flex_row().flex_wrap().gap_1();
                    for (oi, o) in q["options"].as_array().into_iter().flatten().enumerate() {
                        let label = o["label"].as_str().unwrap_or_default().to_owned();
                        let (r, qt, lb) = (req.clone(), text.clone(), label.clone());
                        options = options.child(self.claude_button(("claude-option", qi * 100 + oi), label.clone(), picked.contains(&label), cx, move |this| this.claude_pick(&r, &qt, &lb, multi)));
                    }
                    col = col.child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .child(div().text_sm().font_weight(FontWeight::SEMIBOLD).child(text))
                            .children((multi).then(|| div().text_xs().text_color(rgb(t.dim)).child("Pick any number")))
                            .child(options),
                    );
                }
                let (send, skip) = (req.clone(), req.clone());
                let mut buttons = Vec::new();
                if any_multi {
                    buttons.push(self.claude_button("claude-answers", "Send answers", true, cx, move |this| this.claude_send_answers(&send)).into_any_element());
                }
                buttons.push(self.claude_button("claude-skip", "Skip", false, cx, move |this| this.claude_deny(&skip, "The user chose not to answer.")).into_any_element());
                ("Claude has a question".to_owned(), Some(col.into_any_element()), buttons)
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
                // What the program offers to remember along with a yes.
                let remember_label = request.suggestions.first().map(|s| match s["type"].as_str() {
                    Some("setMode") if s["mode"] == "acceptEdits" => "Allow all edits this session".to_owned(),
                    Some("setMode") => "Allow, and don't ask again".to_owned(),
                    _ => "Always allow this".to_owned(),
                });
                let (allow, always, deny) = (req.clone(), req.clone(), req.clone());
                let mut buttons = vec![self.claude_button("claude-allow", "Allow", true, cx, move |this| this.claude_allow(&allow, false)).into_any_element()];
                if let Some(label) = remember_label {
                    buttons.push(self.claude_button("claude-always", label, false, cx, move |this| this.claude_allow(&always, true)).into_any_element());
                }
                buttons.push(self.claude_button("claude-deny", "Deny", false, cx, move |this| this.claude_deny(&deny, "The user denied this action.")).into_any_element());
                (title, body, buttons)
            }
        };

        div()
            .flex()
            .flex_col()
            .gap_2()
            .w_full()
            .p_2()
            .rounded_md()
            .border_1()
            .border_color(rgb(t.warning))
            .bg(rgb(t.sidebar))
            .child(div().text_sm().font_weight(FontWeight::SEMIBOLD).text_color(rgb(t.text)).child(title))
            .children(body)
            .child(div().flex().flex_row().flex_wrap().gap_1().children(buttons))
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
