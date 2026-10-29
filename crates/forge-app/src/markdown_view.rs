//! The Markdown preview: blocks from `forge_core::markdown` drawn beside the editor, with
//! clickable links and scrolling that follows the editor (and the editor follows it).

use std::cell::Cell;
use std::ops::Range;
use std::path::{Path, PathBuf};

use forge_core::markdown::{Block, Span};
use forge_core::theme::Theme;
use gpui::{div, point, prelude::*, px, rems, rgb, AnyElement, Context, Entity, FontStyle, FontWeight, HighlightStyle, InteractiveText, StrikethroughStyle, StyledText, UnderlineStyle};

use crate::resize::{Edge, Resize};
use crate::workspace::Workspace;

/// What drawing a block needs: colours, who to tell about a clicked link, and a counter for element ids.
struct Ctx<'a> {
    t: &'a Theme,
    workspace: Entity<Workspace>,
    /// The folder of the document, for links to neighbouring files.
    base: PathBuf,
    next_id: Cell<usize>,
}

/// One paragraph's worth of spans as wrapping text with styled runs; links in it can be clicked.
fn text(ctx: &Ctx, spans: &[Span], bold_all: bool) -> AnyElement {
    let t = ctx.t;
    let mut content = String::new();
    let mut runs: Vec<(Range<usize>, HighlightStyle)> = Vec::new();
    let mut link_ranges: Vec<Range<usize>> = Vec::new();
    let mut link_urls: Vec<String> = Vec::new();
    for span in spans {
        if span.text.is_empty() {
            continue;
        }
        let start = content.len();
        content.push_str(&span.text);
        let mut style = HighlightStyle::default();
        let mut styled = false;
        if span.bold || bold_all {
            style.font_weight = Some(FontWeight::BOLD);
            styled = true;
        }
        if span.italic {
            style.font_style = Some(FontStyle::Italic);
            styled = true;
        }
        if span.strike {
            style.strikethrough = Some(StrikethroughStyle { thickness: px(1.0), color: Some(rgb(t.dim).into()) });
            styled = true;
        }
        if span.code {
            style.background_color = Some(rgb(t.border).into());
            style.color = Some(rgb(t.accent).into());
            styled = true;
        }
        if let Some(url) = &span.link {
            style.color = Some(rgb(t.info).into());
            style.underline = Some(UnderlineStyle { thickness: px(1.0), color: Some(rgb(t.info).into()), wavy: false });
            styled = true;
            link_ranges.push(start..content.len());
            link_urls.push(url.clone());
        }
        if styled {
            runs.push((start..content.len(), style));
        }
    }
    let styled = StyledText::new(content).with_highlights(runs);
    if link_ranges.is_empty() {
        return styled.into_any_element();
    }
    let id = ctx.next_id.get();
    ctx.next_id.set(id + 1);
    let (workspace, base) = (ctx.workspace.clone(), ctx.base.clone());
    InteractiveText::new(("markdown-text", id), styled)
        .on_click(link_ranges, move |ix, _window, app| {
            let url = link_urls[ix].clone();
            let base = base.clone();
            workspace.update(app, |ws, cx| ws.open_markdown_link(&url, &base, cx));
        })
        .into_any_element()
}

fn block(ctx: &Ctx, b: &Block) -> AnyElement {
    let t = ctx.t;
    match b {
        Block::Heading { level, spans } => {
            let size = match level {
                1 => 1.875,
                2 => 1.5,
                3 => 1.25,
                _ => 1.0,
            };
            div()
                .w_full()
                .pt(rems(if *level <= 2 { 1.0 } else { 0.75 }))
                .pb(rems(0.375))
                .when(*level <= 2, |d| d.border_b_1().border_color(rgb(t.border)))
                .text_size(rems(size))
                .text_color(rgb(t.text))
                .child(text(ctx, spans, true))
                .into_any_element()
        }
        Block::Paragraph(spans) => div().w_full().py(rems(0.375)).text_color(rgb(t.text)).child(text(ctx, spans, false)).into_any_element(),
        Block::Code { lang, text: code } => div()
            .w_full()
            .my(rems(0.5))
            .p_3()
            .rounded_md()
            .bg(rgb(t.sidebar))
            .border_1()
            .border_color(rgb(t.border))
            .flex()
            .flex_col()
            .overflow_hidden()
            .text_sm()
            .when(!lang.is_empty(), |d| d.child(div().pb_1().text_xs().text_color(rgb(t.dim)).child(lang.clone())))
            .children(code.split('\n').map(|line| div().whitespace_nowrap().min_h(rems(1.25)).text_color(rgb(t.text)).child(line.to_owned())))
            .into_any_element(),
        Block::Quote(blocks) => div()
            .w_full()
            .my(rems(0.375))
            .flex()
            .flex_row()
            .gap_3()
            .child(div().w(px(3.0)).flex_none().rounded_sm().bg(rgb(t.border)))
            .child(div().flex_1().min_w_0().text_color(rgb(t.dim)).children(blocks.iter().map(|b| block(ctx, b))))
            .into_any_element(),
        Block::List { start, items } => div()
            .w_full()
            .py(rems(0.25))
            .flex()
            .flex_col()
            .children(items.iter().enumerate().map(|(i, item)| {
                let marker = match (item.checked, start) {
                    (Some(true), _) => "☑".to_owned(),
                    (Some(false), _) => "☐".to_owned(),
                    (None, Some(first)) => format!("{}.", first + i as u64),
                    (None, None) => "•".to_owned(),
                };
                div()
                    .flex()
                    .flex_row()
                    .gap_2()
                    .pl(rems(0.75))
                    .child(div().w(rems(1.5)).flex_none().text_color(rgb(t.dim)).child(marker))
                    .child(div().flex_1().min_w_0().children(item.blocks.iter().map(|b| match b {
                        // A list item's own text sits close to its neighbours.
                        Block::Paragraph(spans) => div().w_full().py(rems(0.125)).text_color(rgb(t.text)).child(text(ctx, spans, false)).into_any_element(),
                        other => block(ctx, other),
                    })))
            }))
            .into_any_element(),
        Block::Rule => div().w_full().my(rems(0.75)).h(px(1.0)).bg(rgb(t.border)).into_any_element(),
        Block::Table { header, rows } => {
            let cell = |spans: &Vec<Span>, head: bool| {
                div().flex_1().min_w_0().px_2().py_1().border_1().border_color(rgb(t.border)).when(head, |d| d.bg(rgb(t.sidebar))).child(text(ctx, spans, head))
            };
            div()
                .w_full()
                .my(rems(0.5))
                .flex()
                .flex_col()
                .child(div().flex().flex_row().children(header.iter().map(|c| cell(c, true))))
                .children(rows.iter().map(|row| div().flex().flex_row().children(row.iter().map(|c| cell(c, false)))))
                .into_any_element()
        }
    }
}

/// Where a link inside a Markdown file points, decided without touching the disk.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum LinkTarget {
    /// A web or mail address, opened by the system.
    External(String),
    /// A file in the project, relative to the document's folder.
    File(PathBuf),
    /// A `#heading` link within the same document.
    Anchor,
    /// Any other scheme (`file:`, `javascript:`, ...): never opened.
    Refused,
}

pub(crate) fn classify_link(url: &str, base: &Path) -> LinkTarget {
    let lower = url.to_ascii_lowercase();
    if lower.starts_with("http://") || lower.starts_with("https://") || lower.starts_with("mailto:") {
        return LinkTarget::External(url.to_owned());
    }
    if url.starts_with('#') {
        return LinkTarget::Anchor;
    }
    // Anything with a scheme (`name:`) that is not a Windows drive letter is not a project file.
    if let Some(colon) = url.find(':') {
        if colon > 1 && url[..colon].chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.')) {
            return LinkTarget::Refused;
        }
    }
    let path = url.split(['#', '?']).next().unwrap_or("").replace("%20", " ");
    if path.is_empty() {
        return LinkTarget::Anchor;
    }
    LinkTarget::File(base.join(path))
}

impl Workspace {
    /// Markdown blocks drawn like the preview, for pages other than an editor tab.
    pub(crate) fn markdown_blocks(&self, blocks: &[Block], base: PathBuf, cx: &mut Context<Self>) -> AnyElement {
        let ctx = Ctx { t: &self.theme, workspace: cx.entity(), base, next_id: Cell::new(0) };
        div().w_full().flex().flex_col().children(blocks.iter().map(|b| block(&ctx, b))).into_any_element()
    }

    /// Whether the active document in group `g` is Markdown.
    pub(crate) fn is_markdown(&self, g: usize) -> bool {
        let Some(view) = self.groups.get(g).and_then(|gr| gr.views.get(gr.active)) else { return false };
        self.docs[view.doc].path.extension().is_some_and(|e| matches!(e.to_string_lossy().to_ascii_lowercase().as_str(), "md" | "markdown" | "mdx"))
    }

    /// Parse the active Markdown document again if it changed.
    pub fn ensure_markdown(&mut self, g: usize) {
        let Some(view) = self.groups.get(g).and_then(|gr| gr.views.get(gr.active)) else { return };
        let (doc_ix, revision) = (view.doc, self.docs[view.doc].revision);
        if self.md_cache.as_ref().is_some_and(|(d, r, _)| *d == doc_ix && *r == revision) {
            return;
        }
        let blocks = forge_core::markdown::parse(&self.docs[doc_ix].buffer.text());
        self.md_cache = Some((doc_ix, revision, blocks));
    }

    /// The preview pane for group `g`, when it is showing a Markdown file.
    pub fn render_markdown_preview(&self, g: usize, cx: &mut Context<Self>) -> Option<AnyElement> {
        if !self.preview_open || g != self.focused_group || !self.is_markdown(g) {
            return None;
        }
        let (_, _, blocks) = self.md_cache.as_ref()?;
        let doc = self.active_doc()?;
        let ctx = Ctx { t: &self.theme, workspace: cx.entity(), base: doc.path.parent().map(Path::to_path_buf).unwrap_or_default(), next_id: Cell::new(0) };
        let t = &self.theme;
        let title = doc.path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        Some(
            div()
                .relative()
                .when_some(self.preview_px, |d, width| d.flex_none().w(px(width)))
                .when(self.preview_px.is_none(), |d| d.flex_1())
                .min_w_0()
                .h_full()
                .flex()
                .flex_col()
                .child(self.resize_handle(("resize", 2), Resize::Preview, Edge::Left, cx))
                .border_l_1()
                .border_color(rgb(t.border))
                .bg(rgb(t.bg))
                .child(div().flex_none().h(rems(1.75)).px_3().flex().items_center().text_xs().text_color(rgb(t.dim)).child(format!("Preview: {title}")))
                .child(
                    div()
                        .id(("markdown-preview", g))
                        .flex_1()
                        .min_h_0()
                        .track_scroll(&self.preview_scroll)
                        .overflow_y_scroll()
                        .px(rems(1.5))
                        .pb(rems(2.0))
                        .child(div().flex().flex_col().children(blocks.iter().map(|b| block(&ctx, b)))),
                )
                .into_any_element(),
        )
    }

    /// Keep the editor and the preview at the same relative position: whichever moved since the
    /// last frame leads, and the other follows.
    /// Returns true if it moved either pane.
    pub fn sync_preview_scroll(&mut self, g: usize, vi: usize) -> bool {
        let (editor_max, editor_y) = {
            let state = self.groups[g].views[vi].scroll.0.borrow();
            // A uniform list reports how far it can scroll through its last measured sizes.
            let max = state.last_item_size.map_or(0.0, |size| f32::from(size.contents.height - size.item.height));
            (max, f32::from(state.base_handle.offset().y))
        };
        let (preview_max, preview_y) = (f32::from(self.preview_scroll.max_offset().height), f32::from(self.preview_scroll.offset().y));
        if editor_max <= 1.0 || preview_max <= 1.0 {
            return false;
        }
        let fraction = |offset: f32, max: f32| (-offset / max).clamp(0.0, 1.0);
        let (mut editor_at, mut preview_at) = (fraction(editor_y, editor_max), fraction(preview_y, preview_max));
        let (last_editor, last_preview) = self.sync_last;
        let moved = |now: f32, before: f32| (now - before).abs() > 0.002;
        if moved(editor_at, last_editor) {
            self.preview_scroll.set_offset(point(px(0.0), px(-editor_at * preview_max)));
            preview_at = editor_at;
        } else if moved(preview_at, last_preview) {
            self.groups[g].views[vi].scroll.0.borrow().base_handle.set_offset(point(px(0.0), px(-preview_at * editor_max)));
            editor_at = preview_at;
        }
        let changed = moved(editor_at, last_editor) || moved(preview_at, last_preview);
        self.sync_last = (editor_at, preview_at);
        changed
    }

    /// Open a link clicked in the preview: web and mail links in the system's apps, project files in Forge.
    pub fn open_markdown_link(&mut self, url: &str, base: &Path, cx: &mut Context<Self>) {
        match classify_link(url, base) {
            LinkTarget::External(address) => {
                cx.open_url(&address);
                self.set_status(format!("Opened {address}"));
            }
            LinkTarget::File(path) => {
                if path.is_file() {
                    self.open_file(&path, None, cx);
                } else {
                    self.set_status(format!("The linked file was not found: {}", path.display()));
                }
            }
            LinkTarget::Anchor => self.set_status("Links to a heading in the same page are not supported yet"),
            LinkTarget::Refused => self.set_status("Forge opens web, mail and project-file links only"),
        }
        cx.notify();
    }

    pub fn toggle_markdown_preview(&mut self) {
        if self.preview_open {
            self.preview_open = false;
            self.set_status("Markdown preview closed");
        } else if self.is_markdown(self.focused_group) {
            self.preview_open = true;
            self.sync_last = (0.0, 0.0);
            self.set_status("Markdown preview opened beside the editor");
        } else {
            self.set_status("Open a Markdown file (.md) to preview it");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn web_and_mail_links_open_outside_and_odd_schemes_are_refused() {
        let base = Path::new("C:/proj/docs");
        assert_eq!(classify_link("https://example.com/a?b=1#c", base), LinkTarget::External("https://example.com/a?b=1#c".into()));
        assert_eq!(classify_link("HTTP://EXAMPLE.COM", base), LinkTarget::External("HTTP://EXAMPLE.COM".into()));
        assert_eq!(classify_link("mailto:a@b.c", base), LinkTarget::External("mailto:a@b.c".into()));
        for bad in ["javascript:alert(1)", "file:///C:/Windows/System32/cmd.exe", "ms-settings:privacy", "data:text/html,x", "vscode://x"] {
            assert_eq!(classify_link(bad, base), LinkTarget::Refused, "{bad}");
        }
    }

    #[test]
    fn project_links_resolve_next_to_the_document() {
        let base = Path::new("C:/proj/docs");
        assert_eq!(classify_link("ARCH.md#intro", base), LinkTarget::File(base.join("ARCH.md")));
        assert_eq!(classify_link("sub/my%20file.md", base), LinkTarget::File(base.join("sub/my file.md")));
        assert_eq!(classify_link("../README.md", base), LinkTarget::File(base.join("../README.md")));
        assert_eq!(classify_link("#section", base), LinkTarget::Anchor);
        assert_eq!(classify_link("?x=1", base), LinkTarget::Anchor);
        // A Windows drive path is a file, not a scheme.
        assert_eq!(classify_link("C:/data/x.md", base), LinkTarget::File(base.join("C:/data/x.md")));
    }
}
