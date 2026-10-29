//! Resolving merge conflicts: a page listing each conflict with the current and incoming sides
//! next to each other, and buttons to keep one side or both. A resolved file is staged from here.

use forge_core::conflict::{self, Choice, Conflict};
use forge_core::git;
use gpui::{div, prelude::*, px, rems, rgb, rgba, AnyElement, Context, FontWeight, SharedString};

use crate::workspace::Workspace;

pub(crate) struct ConflictPage {
    pub rel: String,
    pub text: String,
}

/// Lines of a side shown before it is cut off, so one huge conflict does not fill the page.
const SHOWN_LINES: usize = 30;

impl Workspace {
    pub fn open_conflict_page(&mut self, rel: String) {
        let Some(root) = self.git.as_ref().map(|g| g.root.clone()) else { return };
        match std::fs::read_to_string(root.join(&rel)) {
            Ok(text) => self.conflict_page = Some(ConflictPage { rel, text }),
            Err(error) => self.set_status(format!("Cannot read {rel}: {error}")),
        }
    }

    /// Replace conflict `index` (or all of them, with `None`) by the chosen side and save the file.
    fn conflict_accept(&mut self, index: Option<usize>, choice: Choice) {
        let (Some(page), Some(root)) = (self.conflict_page.as_ref(), self.git.as_ref().map(|g| g.root.clone())) else { return };
        let next = match index {
            Some(i) => conflict::resolve(&page.text, i, choice),
            None => Some(conflict::resolve_all(&page.text, choice)),
        };
        let Some(next) = next else { return };
        let rel = page.rel.clone();
        if let Err(error) = std::fs::write(root.join(&rel), &next) {
            self.set_status(format!("Cannot save {rel}: {error}"));
            return;
        }
        if let Some(page) = self.conflict_page.as_mut() {
            page.text = next;
        }
        // An open tab of this file must show the resolved text.
        self.reload_clean_docs();
        self.git_request_refresh();
    }

    fn conflict_side(&self, title: String, lines: &[String], color: u32) -> AnyElement {
        let t = &self.theme;
        let shown: Vec<String> = lines.iter().take(SHOWN_LINES).map(|l| l.trim_end_matches(['\r', '\n']).replace('\t', "    ")).collect();
        div()
            .flex_1()
            .min_w_0()
            .flex()
            .flex_col()
            .border_1()
            .border_color(rgb(t.border))
            .rounded_sm()
            .overflow_hidden()
            .child(div().px_2().py_1().text_xs().bg(rgba((color << 8) | 0x55)).font_weight(FontWeight::MEDIUM).child(title))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .bg(rgba((color << 8) | 0x18))
                    .px_2()
                    .py_1()
                    .when(shown.is_empty(), |d| d.child(div().text_color(rgb(t.dim)).child("(nothing)")))
                    .children(shown.into_iter().map(|l| div().whitespace_nowrap().overflow_hidden().min_h(rems(1.25)).child(l)))
                    .when(lines.len() > SHOWN_LINES, |d| d.child(div().text_xs().text_color(rgb(t.dim)).child(format!("... {} more lines", lines.len() - SHOWN_LINES)))),
            )
            .into_any_element()
    }

    pub(crate) fn render_conflict_page(&self, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        let page = self.conflict_page.as_ref()?;
        let t = self.theme.clone();
        let left = if self.sidebar_visible { self.sidebar_width() } else { 0.0 };
        let conflicts: Vec<Conflict> = conflict::find(&page.text);
        let rel = page.rel.clone();

        let button = |id: SharedString, label: &str, primary: bool| {
            div()
                .id(id)
                .flex_none()
                .h(rems(1.75))
                .px_3()
                .flex()
                .items_center()
                .rounded_sm()
                .cursor_pointer()
                .text_sm()
                .bg(rgb(if primary { t.button } else { t.tab_active }))
                .text_color(rgb(if primary { t.button_text } else { t.text }))
                .hover(|s| s.opacity(0.85))
                .child(label.to_owned())
        };
        let choose = |id: SharedString, label: &str, index: Option<usize>, choice: Choice, primary: bool, cx: &mut Context<Self>| {
            button(id, label, primary).on_click(cx.listener(move |this, _, _, cx| {
                this.conflict_accept(index, choice);
                cx.notify();
            }))
        };

        let header = div()
            .flex()
            .flex_row()
            .flex_wrap()
            .items_center()
            .gap_2()
            .child(div().flex_1().min_w(rems(14.0)).child(div().text_size(rems(1.5)).font_weight(FontWeight::BOLD).child("Resolve conflicts")).child(div().text_color(rgb(t.dim)).child(if conflicts.is_empty() { format!("{rel}: no conflict markers are left") } else { format!("{rel}: {} conflict{} left", conflicts.len(), if conflicts.len() == 1 { "" } else { "s" }) })))
            .when(!conflicts.is_empty(), |d| {
                d.child(choose("conflict-all-current".into(), "Accept All Current", None, Choice::Current, false, cx))
                    .child(choose("conflict-all-incoming".into(), "Accept All Incoming", None, Choice::Incoming, false, cx))
            })
            .child(button("conflict-open".into(), "Open in Editor", false).on_click({
                let rel = rel.clone();
                cx.listener(move |this, _, _, cx| {
                    let path = this.git.as_ref().map(|g| g.root.join(&rel));
                    this.conflict_page = None;
                    if let Some(path) = path {
                        this.open_file(&path, None, cx);
                    }
                    cx.notify();
                })
            }))
            .child(button("conflict-close".into(), "Close", false).on_click(cx.listener(|this, _, _, cx| {
                this.conflict_page = None;
                cx.notify();
            })));

        let cards: Vec<AnyElement> = conflicts.iter().enumerate().map(|(i, c)| {
            div()
                .flex()
                .flex_col()
                .gap_2()
                .p_3()
                .rounded_md()
                .border_1()
                .border_color(rgb(t.border))
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap_2()
                        .child(div().flex_1().font_weight(FontWeight::MEDIUM).child(format!("Conflict {} of {}  (line {})", i + 1, conflicts.len(), c.start + 1)))
                        .child(choose(format!("conflict-current-{i}").into(), "Accept Current", Some(i), Choice::Current, false, cx))
                        .child(choose(format!("conflict-incoming-{i}").into(), "Accept Incoming", Some(i), Choice::Incoming, false, cx))
                        .child(choose(format!("conflict-both-{i}").into(), "Accept Both", Some(i), Choice::Both, false, cx)),
                )
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .gap_2()
                        .child(self.conflict_side(format!("Current Change  ({})", c.current_label), &c.current, t.git_added))
                        .child(self.conflict_side(format!("Incoming Change  ({})", c.incoming_label), &c.incoming, t.info)),
                )
                .into_any_element()
        }).collect();

        let footer = conflicts.is_empty().then(|| {
            let rel = rel.clone();
            div().flex().flex_row().items_center().gap_3().child(div().text_color(rgb(t.dim)).child("Stage the file to mark it resolved.")).child(button("conflict-stage".into(), "Stage File (Mark Resolved)", true).on_click(cx.listener(move |this, _, _, cx| {
                let rel = rel.clone();
                this.conflict_page = None;
                this.git_job("stage", move |r| git::mark_resolved(r, &rel).map(|_| "Marked as resolved".to_owned()));
                cx.notify();
            })))
        });

        Some(
            div()
                .id("conflict-page")
                .absolute()
                .top_0()
                .left(px(left))
                .right_0()
                .bottom(rems(1.5))
                .bg(rgb(t.bg))
                .text_color(rgb(t.text))
                .occlude()
                .overflow_y_scroll()
                .flex()
                .justify_center()
                .child(div().flex().flex_col().w_full().max_w(rems(70.0)).px(rems(2.0)).py(rems(1.5)).gap_3().child(header).children(cards).children(footer)),
        )
    }
}
