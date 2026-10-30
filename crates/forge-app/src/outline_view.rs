//! The Outline section at the bottom of the Explorer: the symbols of the open file as a tree, following the caret.
//! They come from the language server when there is one, else from a quick scan of the text.

use std::time::{Duration, Instant};

use forge_core::outline::{scan, OutlineItem};
use forge_lsp::document_symbol_params;
use gpui::{div, prelude::*, px, rems, rgb, uniform_list, AnyElement, Context, MouseButton, MouseDownEvent, ScrollStrategy};
use std::ops::Range;

use crate::lsp::RequestTag;
use crate::workspace::{Focus, SidebarView, Workspace};

/// How long the file has to stay unchanged before its outline is rebuilt.
const SETTLE: Duration = Duration::from_millis(350);
const HEADER_HEIGHT: f32 = 26.0;
const BODY_HEIGHT: f32 = 240.0;
const ROW_HEIGHT: f32 = 22.0;

impl Workspace {
    /// Rebuild the outline when the file in front, or its text, changed and has settled. True if the list changed.
    pub fn refresh_outline(&mut self) -> bool {
        let current = self.active_doc().filter(|d| !d.diff && d.ext_page.is_none()).map(|d| (d.path.clone(), d.revision));
        let Some(key) = current else {
            let had = !self.outline_items.is_empty();
            self.outline_items.clear();
            self.outline_key = None;
            self.outline_seen = None;
            return had;
        };
        if self.outline_key.as_ref() == Some(&key) || !self.outline_open {
            return false;
        }
        if self.outline_seen.as_ref() != Some(&key) {
            // A different file shows its own list at once; the same file waits for typing to pause.
            let other_file = self.outline_seen.as_ref().is_none_or(|(p, _)| p != &key.0);
            self.outline_seen = Some(key.clone());
            self.outline_changed = Instant::now();
            if !other_file {
                return false;
            }
        } else if self.outline_changed.elapsed() < SETTLE {
            return false;
        }
        self.outline_key = Some(key.clone());
        self.outline_items = self.active_doc().map(|d| scan(&key.0, &d.buffer.text())).unwrap_or_default();
        self.outline_followed.set(None);
        // A language server knows better (it sees methods, fields, ...): ask, and swap its answer in.
        if let Some((language, uri, _)) = self.lsp_target() {
            self.outline_requested = Some(key.0);
            self.lsp.request(language, "textDocument/documentSymbol", document_symbol_params(&uri), RequestTag::Outline);
        }
        true
    }

    /// Open the Outline section and show the Explorer (the Outline command).
    pub fn reveal_outline(&mut self) {
        self.sidebar_visible = true;
        self.sidebar_view = SidebarView::Explorer;
        self.outline_open = true;
        self.outline_key = None;
    }

    /// The entry the caret is in: the last one that starts on or before its line.
    fn outline_active(&self) -> Option<usize> {
        let (doc, view) = self.active_doc().zip(self.active_view())?;
        let (line, _) = doc.buffer.utf16_point(view.selections.primary().head);
        self.outline_items.iter().rposition(|item| item.line <= line)
    }

    pub fn render_outline(&self, cx: &mut Context<Self>) -> AnyElement {
        let t = self.theme.clone();
        let open = self.outline_open;
        let header = div()
            .id("outline-header")
            .flex()
            .flex_row()
            .flex_none()
            .items_center()
            .gap_1()
            .h(px(HEADER_HEIGHT))
            .pl_2()
            .pr_2()
            .border_t_1()
            .border_color(rgb(t.border))
            .cursor_pointer()
            .text_xs()
            .text_color(rgb(t.dim))
            .hover(|s| s.bg(rgb(t.tab_active)))
            .on_click(cx.listener(|this, _, _, cx| {
                this.outline_open = !this.outline_open;
                this.outline_key = None;
                cx.notify();
            }))
            .child(crate::icons::glyph(if open { crate::codicons::CHEVRON_DOWN } else { crate::codicons::CHEVRON_RIGHT }, 0.875, t.dim))
            .child("OUTLINE");
        let mut section = div().flex().flex_col().flex_none().child(header);
        if !open {
            return section.into_any_element();
        }
        if self.outline_items.is_empty() {
            let note = if self.active_doc().is_some() { "No symbols found in the active editor." } else { "No editor is open." };
            section = section.child(div().h(px(BODY_HEIGHT)).px_3().py_2().text_xs().text_color(rgb(t.dim)).child(note));
            return section.into_any_element();
        }
        // Scroll to follow the caret, but only when it moves to another entry.
        let active = self.outline_active();
        if active != self.outline_followed.get() {
            self.outline_followed.set(active);
            if let Some(ix) = active {
                self.outline_scroll.scroll_to_item(ix, ScrollStrategy::Center);
            }
        }
        let count = self.outline_items.len();
        let list = uniform_list(
            "outline",
            count,
            cx.processor(move |this, range: Range<usize>, _window, cx| {
                let t = this.theme.clone();
                let active = this.outline_active();
                range
                    .filter_map(|ix| this.outline_items.get(ix).cloned().map(|item| (ix, item)))
                    .map(|(ix, item): (usize, OutlineItem)| {
                        let (line, col) = (item.line, item.col);
                        div()
                            .id(("outline-row", ix))
                            .w_full()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap_2()
                            .h(px(ROW_HEIGHT))
                            .pl(px(10.0 + item.depth as f32 * 12.0))
                            .pr_2()
                            .cursor_pointer()
                            .whitespace_nowrap()
                            .text_sm()
                            .text_color(rgb(t.text))
                            .when(active == Some(ix), |d| d.bg(rgb(t.tab_active)))
                            .hover(|s| s.bg(rgb(t.tab_active)))
                            .on_mouse_down(MouseButton::Left, cx.listener(move |this, _: &MouseDownEvent, _, cx| {
                                cx.stop_propagation();
                                this.place_caret_utf16(line, col);
                                this.focus = Focus::Editor;
                                cx.notify();
                            }))
                            .child(div().w(rems(2.5)).flex_none().text_xs().text_color(rgb(t.info)).child(item.kind))
                            .child(div().min_w_0().overflow_hidden().child(item.name))
                    })
                    .collect::<Vec<_>>()
            }),
        )
        .track_scroll(self.outline_scroll.clone())
        .h(px(BODY_HEIGHT));
        section.child(list).into_any_element()
    }
}
