//! Interactive rebase: a page listing the commits above the chosen base, oldest first, where each
//! can be kept, reworded, squashed or fixed up into the one above it, dropped, or moved. Starting
//! hands the list to git (see `forge_core::git::interactive_rebase`).

use forge_core::git::{self, TodoAction, TodoItem};
use forge_core::input::{LineInput, Outcome};
use gpui::{div, prelude::*, rems, rgb, AnyElement, Context, FontWeight, Keystroke, MouseButton, MouseDownEvent, SharedString};

use crate::workspace::{Focus, Workspace};

pub(crate) struct RebasePage {
    pub base: String,
    /// What the base commit is called, for the heading.
    pub label: String,
    pub items: Vec<TodoItem>,
    /// The message boxes of rewords, one per item, and which one has the keyboard.
    pub messages: Vec<LineInput>,
    pub focus: Option<usize>,
}

const ACTIONS: [&str; 5] = ["pick", "reword", "squash", "fixup", "drop"];

fn action_from(label: &str, message: &str) -> TodoAction {
    match label {
        "reword" => TodoAction::Reword(message.to_owned()),
        "squash" => TodoAction::Squash,
        "fixup" => TodoAction::Fixup,
        "drop" => TodoAction::Drop,
        _ => TodoAction::Pick,
    }
}

impl Workspace {
    pub(crate) fn open_rebase_page(&mut self, base: String, label: String, items: Vec<TodoItem>) {
        let messages = items.iter().map(|i| LineInput::new(&i.subject)).collect();
        self.rebase_page = Some(RebasePage { base, label, items, messages, focus: None });
    }

    fn rebase_set_action(&mut self, i: usize, label: &str) {
        let Some(page) = self.rebase_page.as_mut() else { return };
        let Some(item) = page.items.get_mut(i) else { return };
        item.action = action_from(label, page.messages[i].text());
        page.focus = (label == "reword").then_some(i);
    }

    fn rebase_move(&mut self, i: usize, delta: isize) {
        let Some(page) = self.rebase_page.as_mut() else { return };
        let j = i as isize + delta;
        if j < 0 || j as usize >= page.items.len() {
            return;
        }
        let j = j as usize;
        page.items.swap(i, j);
        page.messages.swap(i, j);
        page.focus = None;
    }

    /// Hand the choices to git. The page stays open if they cannot be done (a squash with nothing above it).
    fn start_interactive_rebase(&mut self) {
        let Some(page) = self.rebase_page.as_ref() else { return };
        let items: Vec<TodoItem> = page
            .items
            .iter()
            .enumerate()
            .map(|(i, item)| match &item.action {
                TodoAction::Reword(_) => TodoItem { action: TodoAction::Reword(page.messages[i].text().to_owned()), ..item.clone() },
                _ => item.clone(),
            })
            .collect();
        if let Err(problem) = git::rebase_todo(&items) {
            self.set_status(problem);
            return;
        }
        let base = page.base.clone();
        self.rebase_page = None;
        self.git_job("rebase", move |root| git::interactive_rebase(root, &base, &items));
    }

    /// Keys while the page is open. True when the key was used (or must not reach the editor behind).
    pub(crate) fn rebase_key(&mut self, k: &Keystroke) -> bool {
        let ctrl = k.modifiers.control || k.modifiers.platform;
        let Some(page) = self.rebase_page.as_mut() else { return false };
        if k.key == "escape" {
            match page.focus.take() {
                Some(_) => {}
                None => self.rebase_page = None,
            }
            return true;
        }
        if let Some(i) = page.focus {
            return match page.messages[i].apply(&k.key, ctrl, k.key_char.as_deref()) {
                Outcome::Submit => {
                    page.focus = None;
                    true
                }
                Outcome::Ignored => false,
                _ => true,
            };
        }
        // Shortcuts still work; plain typing must not land in the editor behind the page.
        !ctrl && !k.modifiers.alt
    }

    pub(crate) fn render_rebase_page(&self, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        let page = self.rebase_page.as_ref()?;
        let t = self.theme.clone();
        let left = if self.sidebar_visible { self.sidebar_width() } else { 0.0 };
        let count = page.items.len();

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

        let rows: Vec<AnyElement> = page
            .items
            .iter()
            .enumerate()
            .map(|(i, item)| {
                let current = item.action.label();
                let dropped = current == "drop";
                let chips: Vec<AnyElement> = ACTIONS.iter().map(|label| {
                    let chosen = *label == current;
                    let label = *label;
                    div()
                        .id(SharedString::from(format!("rebase-action-{i}-{label}")))
                        .px_2()
                        .h(rems(1.5))
                        .flex()
                        .items_center()
                        .rounded_sm()
                        .cursor_pointer()
                        .text_xs()
                        .when(label == "drop" && chosen, |d| d.text_color(rgb(t.error)))
                        .bg(rgb(if chosen { t.selection } else { t.tab_active }))
                        .when(!chosen, |d| d.text_color(rgb(t.dim)).hover(|s| s.bg(rgb(t.border))))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.rebase_set_action(i, label);
                            cx.notify();
                        }))
                        .child(label)
                        .into_any_element()
                }).collect();
                let arrow = |id: SharedString, glyph: &str, delta: isize, enabled: bool, cx: &mut Context<Self>| {
                    div()
                        .id(id)
                        .w(rems(1.5))
                        .h(rems(1.5))
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded_sm()
                        .text_color(rgb(if enabled { t.text } else { t.border }))
                        .when(enabled, |d| d.cursor_pointer().hover(|s| s.bg(rgb(t.border))))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if enabled {
                                this.rebase_move(i, delta);
                                cx.notify();
                            }
                        }))
                        .child(glyph.to_owned())
                };
                let subject: AnyElement = if matches!(item.action, TodoAction::Reword(_)) {
                    let focused = page.focus == Some(i);
                    div()
                        .flex_1()
                        .min_w_0()
                        .on_mouse_down(MouseButton::Left, cx.listener(move |this, _: &MouseDownEvent, _, cx| {
                            if let Some(p) = this.rebase_page.as_mut() {
                                p.focus = Some(i);
                            }
                            this.focus = Focus::Editor;
                            cx.stop_propagation();
                            cx.notify();
                        }))
                        .child(self.render_input(&page.messages[i], "New message", focused))
                        .into_any_element()
                } else {
                    div().flex_1().min_w_0().overflow_hidden().whitespace_nowrap().text_ellipsis().when(dropped, |d| d.text_color(rgb(t.dim)).line_through()).child(item.subject.clone()).into_any_element()
                };
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .py_1()
                    .border_b_1()
                    .border_color(rgb(t.border))
                    .child(arrow(format!("rebase-up-{i}").into(), "▲", -1, i > 0, cx))
                    .child(arrow(format!("rebase-down-{i}").into(), "▼", 1, i + 1 < count, cx))
                    .child(div().flex().flex_row().gap_1().flex_none().children(chips))
                    .child(div().flex_none().w(rems(4.5)).text_xs().text_color(rgb(t.dim)).child(item.hash.chars().take(8).collect::<String>()))
                    .child(subject)
                    .into_any_element()
            })
            .collect();

        Some(
            crate::scroll::overlay_page("rebase-page", left, t.bg, t.text)
                .on_mouse_down(MouseButton::Left, cx.listener(|this, _: &MouseDownEvent, _, cx| {
                    if let Some(p) = this.rebase_page.as_mut() {
                        p.focus = None;
                    }
                    cx.notify();
                }))
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .w_full()
                        .max_w(rems(64.0))
                        .px(rems(2.0))
                        .py(rems(1.5))
                        .gap_3()
                        .child(
                            div()
                                .flex()
                                .flex_row()
                                .items_center()
                                .gap_2()
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w(rems(14.0))
                                        .child(div().text_size(rems(1.5)).font_weight(FontWeight::BOLD).child("Interactive Rebase"))
                                        .child(div().text_color(rgb(t.dim)).child(format!("{count} commit{} above {} ({}); the oldest is at the top", if count == 1 { "" } else { "s" }, page.base.chars().take(8).collect::<String>(), page.label))),
                                )
                                .child(button("rebase-start".into(), "Start Rebase", true).on_click(cx.listener(|this, _, _, cx| {
                                    this.start_interactive_rebase();
                                    cx.notify();
                                })))
                                .child(button("rebase-cancel".into(), "Cancel", false).on_click(cx.listener(|this, _, _, cx| {
                                    this.rebase_page = None;
                                    cx.notify();
                                }))),
                        )
                        .child(div().text_xs().text_color(rgb(t.dim)).child("pick keeps a commit; reword changes its message; squash and fixup fold it into the commit above (squash joins the messages, fixup drops this one); drop removes it. Use the arrows to reorder."))
                        .child(div().flex().flex_col().children(rows)),
                ),
        )
    }
}
