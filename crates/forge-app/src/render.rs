//! Drawing. Everything here reads state and builds elements; input lives elsewhere.

use std::ops::Range;

use forge_core::commands::Command;
use forge_core::git::Change;
use forge_core::gutter::{change_at, ChangeKind};
use forge_core::input::LineInput;
use forge_core::theme::Theme;
use forge_lsp::{Severity, SignatureHelp};
use forge_syntax::Span;
use forge_text::brackets::matching_bracket;
use forge_text::columns::{display_width, expand_tabs_mapped, TAB_WIDTH};
use forge_text::search::Match;
use gpui::{
    div, font, prelude::*, px, relative, rems, rgb, rgba, uniform_list, AnyElement, App, Context, Div, FontWeight, HighlightStyle, MouseButton, MouseDownEvent, MouseMoveEvent,
    Pixels, StyledText, Window,
};

use crate::resize::{equal_shares, Edge, Resize};
use crate::workspace::{
    Focus, OverlayKind, PanelView, SearchRow, SidebarView, Workspace, BREADCRUMB_HEIGHT, FOLD_WIDTH, FONT, GUTTER_WIDTH,
    GLYPH_WIDTH, LINE_NUMBER_WIDTH, TAB_BAR_HEIGHT, TEXT_PAD,
};

/// Horizontal padding of the Git view, and of the text inside the commit message box.
const SCM_PAD: f32 = 10.0;
const INPUT_PAD: f32 = 6.0;

/// Per-frame values every visible line needs.
struct LineCtx {
    char_w: Pixels,
    line_h: f32,
    spans: Vec<Vec<Span>>,
    first_line: usize,
    bracket: Option<(usize, usize)>,
    matches: Vec<Match>,
    current_match: Option<Match>,
    focused: bool,
    breakpoints: std::collections::BTreeSet<usize>,
    debug_line: Option<usize>,
}

/// The small coloured file-type badge shown before a file's name.
fn file_badge(name: &str) -> Div {
    let (label, color) = crate::file_icons::badge(name);
    div().w(rems(1.5)).flex_none().flex().justify_center().text_size(rems(0.6875)).font_weight(FontWeight::BOLD).text_color(rgb(color)).child(label)
}

/// A file's icon: the extension icon theme's image when there is one, else the letter badge.
fn file_icon(icons: Option<&forge_core::icon_theme::IconTheme>, name: &str) -> Div {
    match icons.and_then(|t| t.file_icon(name, None)) {
        Some(path) => icon_image(path),
        None => file_badge(name),
    }
}

fn icon_image(path: &std::path::Path) -> Div {
    div().w(rems(1.5)).flex_none().flex().justify_center().items_center().child(gpui::img(path.to_path_buf()).w(rems(1.125)).h(rems(1.125)).flex_none())
}

fn px_f(v: f32) -> Pixels {
    px(v)
}

impl Workspace {
    pub fn measure_font(&mut self, cx: &App) {
        let system = cx.text_system();
        let id = system.resolve_font(&font(FONT));
        if let Ok(size) = system.advance(id, px(self.settings.font_size), 'm') {
            self.char_width = size.width;
        }
    }

    pub(crate) fn render_input(&self, input: &LineInput, placeholder: &str, focused: bool) -> Div {
        let t = &self.theme;
        let text = input.text();
        let split = text.char_indices().nth(input.cursor()).map_or(text.len(), |(b, _)| b);
        let (before, after) = text.split_at(split);
        let row = div().flex().flex_row().items_center().h(rems(1.375)).px_2().bg(rgb(t.bg)).border_1().border_color(rgb(if focused { t.info } else { t.border })).rounded_sm().overflow_hidden().whitespace_nowrap();
        if text.is_empty() {
            return row.child(div().w(px(1.0)).h(rems(0.875)).bg(rgb(if focused { t.caret } else { t.bg }))).child(div().text_color(rgb(t.dim)).child(placeholder.to_owned()));
        }
        let selected = input.is_all_selected();
        row.child(div().when(selected, |d| d.bg(rgb(t.selection))).child(before.to_owned()))
            .child(div().w(px(1.0)).h(rems(0.875)).bg(rgb(if focused && !selected { t.caret } else { t.bg })))
            .child(div().when(selected, |d| d.bg(rgb(t.selection))).child(after.to_owned()))
    }

    // ---- tabs and groups ----

    fn render_tabs(&self, g: usize, cx: &mut Context<Self>) -> impl IntoElement {
        let t = &self.theme;
        let group = &self.groups[g];
        let focused_group = g == self.focused_group;
        let tabs = group.views.iter().enumerate().map(|(vi, view)| {
            let doc = &self.docs[view.doc];
            let active = group.active == vi;
            let problems = doc.diagnostics.iter().filter(|d| d.severity == Severity::Error).count();
            div()
                .id(("tab", g * 1000 + vi))
                .flex()
                .items_center()
                .gap_2()
                .px_3()
                .h(px(TAB_BAR_HEIGHT))
                .cursor_pointer()
                .border_r_1()
                .border_color(rgb(t.border))
                .border_t_2()
                .border_color(rgb(if active && focused_group { t.info } else if active { t.border } else { t.sidebar }))
                .bg(rgb(if active { t.bg } else { t.sidebar }))
                .text_color(rgb(if problems > 0 { t.error } else if active { t.text } else { t.dim }))
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.focused_group = g;
                    this.groups[g].active = vi;
                    this.focus = Focus::Editor;
                    this.completion = None;
                    cx.notify();
                }))
                .child(if doc.ext_page.is_some() { crate::icons::glyph(crate::codicons::EXTENSIONS, 1.0, t.dim) } else { file_icon(self.icon_theme.as_ref(), &doc.name()) })
                .child(doc.name())
                .child(
                    div()
                        .id(("close", g * 1000 + vi))
                        .w(rems(1.375))
                        .h(rems(1.375))
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded_sm()
                        .cursor_pointer()
                        .hover(|s| s.bg(rgb(t.border)))
                        .tooltip(self.tooltip(if doc.dirty { "Unsaved changes: close" } else { "Close" }, Some(Command::CloseTab)))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            cx.stop_propagation();
                            this.close_view(g, vi, cx);
                        }))
                        .child(crate::icons::glyph(if doc.dirty { crate::codicons::CIRCLE_FILLED } else { crate::codicons::CLOSE }, 0.875, if doc.dirty { t.accent } else { t.dim })),
                )
        })
        .collect::<Vec<_>>();
        // Icon buttons for the file: its Markdown preview, a split, and a menu of everything else.
        let button = |id: &'static str, icon: gpui::Div, name: &'static str, command: Option<Command>, active: bool, cx: &mut Context<Self>, press: Box<dyn Fn(&mut Workspace, &gpui::MouseDownEvent, &mut Context<Workspace>)>| {
            div()
                .id((id, g))
                .tooltip(self.tooltip(name, command))
                .w(rems(1.875))
                .h(rems(1.875))
                .flex()
                .flex_none()
                .items_center()
                .justify_center()
                .rounded_sm()
                .cursor_pointer()
                .when(active, |d| d.bg(rgb(t.border)))
                .hover(|s| s.bg(rgb(t.border)))
                .on_mouse_down(MouseButton::Left, cx.listener(move |this, ev: &MouseDownEvent, _, cx| {
                    cx.stop_propagation();
                    this.focused_group = g;
                    press(this, ev, cx);
                    cx.notify();
                }))
                .child(icon)
        };
        let has_views = !group.views.is_empty();
        let markdown = has_views && self.is_markdown(g);
        let preview_on = self.preview_open && g == self.focused_group;
        let actions = div()
            .flex()
            .flex_row()
            .flex_none()
            .items_center()
            .gap_1()
            .px_2()
            .children((has_views && markdown).then(|| button("tab-preview", crate::icons::glyph(crate::codicons::OPEN_PREVIEW, 1.125, if preview_on { t.text } else { t.dim }), "Open Preview to the Side", Some(Command::MarkdownPreview), preview_on, cx, Box::new(|w, _, _| w.toggle_markdown_preview()))))
            .children(has_views.then(|| button("tab-split", crate::icons::glyph(crate::codicons::SPLIT_HORIZONTAL, 1.125, t.dim), "Split Editor", Some(Command::SplitEditor), false, cx, Box::new(|w, _, cx| w.run_command(Command::SplitEditor, false, cx)))))
            .children(has_views.then(|| button("tab-more", crate::icons::glyph(crate::codicons::ELLIPSIS, 1.125, t.dim), "More Actions...", None, false, cx, Box::new(move |w, ev, cx| w.open_tab_menu(g, ev.position, cx)))));
        div()
            .flex()
            .flex_row()
            .flex_none()
            .items_center()
            .h(px(TAB_BAR_HEIGHT))
            .bg(rgb(t.sidebar))
            .child(div().flex().flex_row().flex_1().min_w_0().h_full().overflow_hidden().children(tabs))
            .child(actions)
    }

    fn render_breadcrumbs(&self, g: usize) -> impl IntoElement {
        let t = &self.theme;
        let crumbs = self.groups[g].views.get(self.groups[g].active).map(|v| {
            let doc = &self.docs[v.doc];
            if doc.readonly {
                return doc.name();
            }
            let path = &doc.path;
            path.strip_prefix(self.root()).unwrap_or(path).components().map(|c| c.as_os_str().to_string_lossy().into_owned()).collect::<Vec<_>>().join("  ›  ")
        });
        div().flex().flex_row().items_center().flex_none().h(px(BREADCRUMB_HEIGHT)).px_3().text_xs().text_color(rgb(t.dim)).bg(rgb(t.bg)).child(crumbs.unwrap_or_default())
    }

    fn render_group(&mut self, g: usize, cx: &mut Context<Self>) -> Div {
        let t = self.theme.clone();
        let has_views = !self.groups[g].views.is_empty();
        let ext_page = self.groups[g].views.get(self.groups[g].active).map(|v| v.doc).filter(|&d| self.docs[d].ext_page.is_some());
        let diff = if self.settings.diff_side_by_side && has_views { self.diff_target_in(g) } else { None };
        let body: AnyElement = if let Some(page) = ext_page.and_then(|d| self.render_ext_page(d, cx)) {
            page
        } else if let Some((doc_ix, target)) = diff {
            self.render_diff_view(doc_ix, &target, cx)
        } else if has_views {
            self.render_editor(g, cx).into_any_element()
        } else {
            self.render_welcome().into_any_element()
        };
        let mut column = div().relative().flex().flex_col().min_w_0().h_full().bg(rgb(t.bg)).border_color(rgb(t.border));
        // Split groups take their dragged shares of the width; a single group takes all of it.
        column = match (self.groups.len() > 1).then(|| self.group_fractions.get(g).copied()).flatten() {
            Some(share) => column.flex_none().w(relative(share)),
            None => column.flex_1(),
        };
        if g > 0 {
            column = column.border_l_1();
        }
        column
            .when(g + 1 < self.groups.len(), |d| d.child(self.resize_handle(("resize-group", g), Resize::Groups(g), Edge::Right, cx)))
            .child(self.render_tabs(g, cx))
            .child(self.render_breadcrumbs(g))
            .child(div().relative().flex_1().min_h_0().child(body).children((g == self.focused_group && self.find.open && has_views).then(|| self.render_find_widget(cx))))
    }

    fn render_welcome(&self) -> impl IntoElement {
        let t = &self.theme;
        let rows: Vec<(String, Option<String>)> = [
            (forge_core::commands::Command::QuickOpen, "Go to File"),
            (forge_core::commands::Command::CommandPalette, "Show All Commands"),
            (forge_core::commands::Command::FindInFiles, "Find in Files"),
            (forge_core::commands::Command::ToggleTerminal, "Toggle Terminal"),
            (forge_core::commands::Command::OpenFolder, "Open Folder"),
        ]
        .into_iter()
        .map(|(c, label)| (label.to_owned(), self.keymap.shortcut_label(c)))
        .collect();
        div()
            .size_full()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap_2()
            .text_color(rgb(t.dim))
            .child(div().text_2xl().text_color(rgb(t.text)).child("Forge"))
            .children(rows.into_iter().map(|(label, key)| div().flex().flex_row().gap_4().child(div().w(rems(10.625)).child(label)).child(div().text_color(rgb(t.text)).child(key.unwrap_or_default()))))
    }

    // ---- the editor ----

    fn render_editor(&mut self, g: usize, cx: &mut Context<Self>) -> impl IntoElement {
        let vi = self.groups[g].active;
        self.ensure_folds(g, vi, false);
        self.ensure_wrap(g, vi);
        self.ensure_minimap_stats(g, vi);
        if self.preview_open && g == self.focused_group && self.is_markdown(g) {
            self.ensure_markdown(g);
        }
        let doc_ix = self.groups[g].views[vi].doc;
        self.ensure_gutter(doc_ix);
        let view = &self.groups[g].views[vi];
        let doc = &self.docs[view.doc];
        let total = doc.buffer.len_lines();
        let visible = view.folds.visible_lines(total);
        let count = match &view.wrap {
            Some(map) => map.row_count(),
            None => visible.as_ref().map_or(total, Vec::len),
        };
        let scroll = view.scroll.clone();
        let focused = g == self.focused_group;
        let minimap = self.render_minimap(g, count, cx);
        let preview = self.render_markdown_preview(g, cx);
        div().size_full().flex().flex_row().child(
            div().flex_1().min_w_0().h_full().child(
                uniform_list(
                    ("editor", g),
                    count,
                    cx.processor(move |this, range: Range<usize>, _window, _cx| this.render_lines(g, range)),
                )
                .track_scroll(scroll)
                .size_full()
                .when(!focused, |d| d.opacity(0.92)),
            ),
        )
        .children(preview)
        .children(minimap)
    }

    fn render_lines(&self, g: usize, range: Range<usize>) -> Vec<Div> {
        let Some(view) = self.groups.get(g).and_then(|gr| gr.views.get(gr.active)) else { return Vec::new() };
        let doc = &self.docs[view.doc];
        let total = doc.buffer.len_lines();
        let visible = view.folds.visible_lines(total);
        let rows: Vec<crate::wrap::Row> = match (&view.wrap, &visible) {
            (Some(map), _) => range.filter_map(|i| map.row(i)).collect(),
            (None, Some(v)) => range.filter_map(|i| v.get(i).copied()).map(|line| crate::wrap::Row { line, k: 0, start: 0, end: None }).collect(),
            (None, None) => range.filter(|i| *i < total).map(|line| crate::wrap::Row { line, k: 0, start: 0, end: None }).collect(),
        };
        let (Some(first), Some(last)) = (rows.first().map(|r| r.line), rows.last().map(|r| r.line)) else { return Vec::new() };
        let focused = g == self.focused_group;
        let spans = match (doc.syntax.as_ref(), doc.tm.as_ref()) {
            (Some(s), _) => s.highlight_lines(doc.buffer.rope(), first..last + 1),
            (None, Some(t)) => t.highlight_lines(doc.buffer.rope(), first..last + 1),
            (None, None) => Vec::new(),
        };
        let caret = view.selections.primary().head;
        let bracket = if focused { matching_bracket(doc.buffer.rope(), caret) } else { None };
        let (matches, current_match) = if focused && self.find.open && self.find_revision.0 == view.doc {
            let line_lo = doc.buffer.line_start(first);
            let line_hi = doc.buffer.line_end(last);
            let lo = self.find.matches.partition_point(|m| m.end <= line_lo);
            let visible_matches: Vec<Match> = self.find.matches[lo..].iter().take_while(|m| m.start <= line_hi).copied().collect();
            (visible_matches, self.find.current_match())
        } else {
            (Vec::new(), None)
        };
        let (breakpoints, debug_line) = self.debug_marks(&doc.path);
        let ctx = LineCtx { char_w: self.char_width, line_h: self.line_height(), spans, first_line: first, bracket, matches, current_match, focused, breakpoints, debug_line };
        rows.into_iter().map(|row| self.render_line(view, row.line, &ctx, row)).collect()
    }

    fn render_line(&self, view: &crate::workspace::View, line: usize, ctx: &LineCtx, row: crate::wrap::Row) -> Div {
        let t = &self.theme;
        let doc = &self.docs[view.doc];
        let text = doc.buffer.line(line);
        let (shown, byte_of_col) = expand_tabs_mapped(&text);
        let line_start = doc.buffer.line_start(line);
        let line_end = doc.buffer.line_end(line);
        let char_w = ctx.char_w;
        let line_h = px(ctx.line_h);
        // A wrapped row shows the whole line shifted left by its start cell; the text area clips it.
        let x_of = |col: usize| char_w * (display_width(&text, col) as f32 - row.start as f32) + px(TEXT_PAD);

        let runs: Vec<(Range<usize>, HighlightStyle)> = ctx
            .spans
            .get(line - ctx.first_line)
            .map(|spans| {
                spans
                    .iter()
                    .filter(|s| s.end_col < byte_of_col.len())
                    .map(|s| (byte_of_col[s.start_col]..byte_of_col[s.end_col], HighlightStyle { color: Some(rgb(t.syntax_color(s.class)).into()), ..Default::default() }))
                    .collect()
            })
            .unwrap_or_default();
        // Generated diffs are colored by their line prefix instead of by syntax.
        let diff_style = if doc.diff { diff_line_style(t, &text) } else { None };
        let runs = match diff_style {
            Some((color, _)) if !shown.is_empty() => vec![(0..shown.len(), HighlightStyle { color: Some(rgb(color).into()), ..Default::default() })],
            _ if doc.diff => Vec::new(),
            _ => runs,
        };

        let mut layers: Vec<Div> = Vec::new();
        let rect = |from: usize, cells: usize, color: u32| div().absolute().top_0().left(x_of(from)).w(char_w * cells as f32).h(line_h).bg(rgb(color));

        // Find matches.
        for m in ctx.matches.iter().filter(|m| m.start <= line_end && m.end > line_start) {
            let from = m.start.max(line_start) - line_start;
            let to = m.end.min(line_end) - line_start;
            let cells = (display_width(&text, to) - display_width(&text, from)).max(1);
            let current = ctx.current_match == Some(*m);
            layers.push(rect(from, cells, if current { t.find_current } else { t.find_match }));
        }
        // Matching brackets.
        if let Some((a, b)) = ctx.bracket {
            for pos in [a, b] {
                if (line_start..line_end).contains(&pos) {
                    layers.push(rect(pos - line_start, 1, t.bracket_match));
                }
            }
        }
        // Selections and carets.
        for sel in view.selections.as_slice() {
            if !sel.is_empty() && sel.start() <= line_end && sel.end() > line_start {
                let from = sel.start().max(line_start) - line_start;
                let to = sel.end().min(line_end) - line_start;
                // A selection running past the line end also covers its newline cell.
                let extra = usize::from(sel.end() > line_end);
                layers.push(rect(from, display_width(&text, to) - display_width(&text, from) + extra, t.selection));
            }
            let head_cell = display_width(&text, sel.head.saturating_sub(line_start));
            let in_row = head_cell >= row.start && row.end.is_none_or(|e| head_cell < e);
            if ctx.focused && in_row && doc.buffer.offset_to_point(sel.head).0 == line {
                layers.push(div().absolute().top_0().left(x_of(sel.head - line_start)).w(px(2.0)).h(line_h).bg(rgb(t.caret)));
            }
        }
        // Diagnostics underlines.
        let mut worst: Option<Severity> = None;
        for d in doc.diagnostics_on(line) {
            worst = Some(match worst {
                Some(w) if (w as u8) <= (d.severity as u8) => w,
                _ => d.severity,
            });
            let from = if d.range.start.line == line { doc.buffer.offset_from_utf16(line, d.range.start.character) - line_start } else { 0 };
            let to = if d.range.end.line == line { doc.buffer.offset_from_utf16(line, d.range.end.character) - line_start } else { line_end - line_start };
            let cells = (display_width(&text, to.max(from + 1)) - display_width(&text, from)).max(1);
            let color = match d.severity {
                Severity::Error => t.error,
                Severity::Warning => t.warning,
                _ => t.info,
            };
            layers.push(div().absolute().bottom_0().left(x_of(from)).w(char_w * cells as f32).h(px(2.0)).bg(rgb(color)));
        }
        // Indent guides.
        let lead_cells = display_width(&text, text.chars().take_while(|c| *c == ' ' || *c == '\t').count());
        if !text.trim().is_empty() && row.k == 0 {
            for k in 1..(lead_cells / TAB_WIDTH.max(1)).max(1) {
                if k * TAB_WIDTH < lead_cells {
                    layers.push(div().absolute().top_0().left(char_w * (k * TAB_WIDTH) as f32 + px(TEXT_PAD)).w(px(1.0)).h(line_h).bg(rgb(t.border)));
                }
            }
        }
        // Folded region marker.
        let collapsed = view.folds.collapsed_end(line).is_some();
        if collapsed && row.end.is_none() {
            let end_x = x_of(text.chars().count()) + char_w;
            layers.push(div().absolute().top(px(1.0)).left(end_x).h(line_h - px(2.0)).px_1().rounded_sm().bg(rgb(t.border)).text_color(rgb(t.dim)).child("…"));
        }

        let caret_line = ctx.focused && view.selections.primary().is_empty() && doc.buffer.offset_to_point(view.selections.primary().head).0 == line;
        let number_color = match worst {
            Some(Severity::Error) => t.error,
            Some(Severity::Warning) => t.warning,
            _ if caret_line => t.text,
            _ => t.dim,
        };
        let chevron = view.folds.has_region_at(line).then_some(if collapsed { crate::codicons::CHEVRON_RIGHT } else { crate::codicons::CHEVRON_DOWN });

        let change = change_at(&doc.gutter, line);
        let is_debug_line = ctx.debug_line == Some(line);
        let has_breakpoint = ctx.breakpoints.contains(&line);
        div()
            .relative()
            .flex()
            .flex_row()
            .w_full()
            .h(line_h)
            .when(caret_line, |d| d.bg(rgb(t.line_highlight)))
            .when(is_debug_line, |d| d.bg(crate::debug_support::tint(t.warning, 0x33)))
            .when_some(diff_style.and_then(|(c, tint)| tint.then_some(c)), |d, c| d.bg(rgba((c << 8) | 0x26)))
            .children(change.map(|kind| gutter_marker(t, kind, line_h)))
            .child(
                // The margin: a red dot for a breakpoint, an arrow where a paused program stopped.
                div()
                    .w(px(GLYPH_WIDTH))
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_color(rgb(t.warning))
                    .text_size(rems(0.75))
                    .child(if row.k != 0 {
                        div()
                    } else if is_debug_line {
                        div().child("▶")
                    } else if has_breakpoint {
                        div().w(px(10.0)).h(px(10.0)).rounded_full().bg(rgb(0xe51400))
                    } else {
                        div()
                    }),
            )
            .child(
                div()
                    .w(px(LINE_NUMBER_WIDTH))
                    .flex_none()
                    .pr_2()
                    .flex()
                    .justify_end()
                    .text_color(rgb(number_color))
                    .child(if row.k == 0 { (line + 1).to_string() } else { String::new() }),
            )
            .child(div().w(px(FOLD_WIDTH)).flex_none().flex().items_center().justify_center().children(chevron.filter(|_| row.k == 0).map(|icon| crate::icons::glyph(icon, 0.875, t.dim))))
            .child(
                div()
                    .relative()
                    .flex_1()
                    .h(line_h)
                    .overflow_hidden()
                    .child(
                        // A row that wrapped early is narrower than the editor: clip it to its own cells so
                        // the rest of the line does not show through beside it.
                        div()
                            .absolute()
                            .top_0()
                            .left_0()
                            .h(line_h)
                            .when_some(row.end, |d, end| d.w(char_w * (end - row.start) as f32 + px(TEXT_PAD)))
                            .when(row.end.is_none(), |d| d.w_full())
                            .overflow_hidden()
                            .children(layers)
                            .child(div().absolute().top_0().left(px(TEXT_PAD) - char_w * row.start as f32).whitespace_nowrap().child(StyledText::new(shown).with_highlights(runs))),
                    ),
            )
    }

    // ---- find widget ----

    fn render_find_widget(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = &self.theme;
        let f = &self.find;
        let toggle = |icon: char, tip: &'static str, on: bool, which: u8, cx: &mut Context<Self>| {
            div()
                .id(("find-toggle", which as usize))
                .min_w(rems(1.75))
                .h(rems(1.625))
                .flex()
                .items_center()
                .justify_center()
                .px_1()
                .rounded_sm()
                .cursor_pointer()
                .bg(rgb(if on { t.selection } else { t.popup }))
                .tooltip(self.tooltip(tip, None))
                .on_click(cx.listener(move |this, _, _, cx| {
                    match which {
                        0 => this.find.options.case_sensitive = !this.find.options.case_sensitive,
                        1 => this.find.options.whole_word = !this.find.options.whole_word,
                        _ => this.find.options.regex = !this.find.options.regex,
                    }
                    this.refresh_find();
                    cx.notify();
                }))
                .child(crate::icons::glyph(icon, 1.0, if on { t.text } else { t.dim }))
        };
        let step = |icon: char, forward: bool, cx: &mut Context<Self>| {
            div()
                .id(("find-step", usize::from(forward)))
                .w(rems(1.75))
                .h(rems(1.625))
                .flex()
                .items_center()
                .justify_center()
                .rounded_sm()
                .cursor_pointer()
                .hover(|s| s.bg(rgb(t.border)))
                .tooltip(self.tooltip(if forward { "Next Match" } else { "Previous Match" }, Some(if forward { Command::FindNext } else { Command::FindPrevious })))
                .on_click(cx.listener(move |this, _, _, cx| this.find_next(forward, cx)))
                .child(crate::icons::glyph(icon, 1.0, t.text))
        };
        let status = f.status_label();
        let error = f.error.is_some() || (status == "No results");
        let mut widget = div()
            .absolute()
            .top(rems(0.25))
            .right(rems(1.5))
            .w(rems(27.5))
            .flex()
            .flex_col()
            .gap_1()
            .p_1()
            .bg(rgb(t.popup))
            .border_1()
            .border_color(rgb(t.border))
            .rounded_md()
            .shadow_lg()
            .occlude()
            .on_mouse_down(MouseButton::Left, cx.listener(|this, _: &MouseDownEvent, _, cx| {
                this.focus = Focus::Find;
                cx.stop_propagation();
            }))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_1()
                    .child(div().flex_1().min_w_0().child(self.render_input(&f.query, "Find", self.focus == Focus::Find && !f.focus_replace)))
                    .child(toggle(crate::codicons::CASE_SENSITIVE, "Match Case (Alt+C)", f.options.case_sensitive, 0, cx))
                    .child(toggle(crate::codicons::WHOLE_WORD, "Match Whole Word (Alt+W)", f.options.whole_word, 1, cx))
                    .child(toggle(crate::codicons::REGEX, "Use Regular Expression (Alt+R)", f.options.regex, 2, cx))
                    .child(div().w(rems(4.375)).text_xs().text_color(rgb(if error { t.error } else { t.dim })).child(status))
                    .child(step(crate::codicons::ARROW_UP, false, cx))
                    .child(step(crate::codicons::ARROW_DOWN, true, cx))
                    .child(
                        div()
                            .id("find-close")
                            .w(rems(1.75))
                            .h(rems(1.625))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded_sm()
                            .cursor_pointer()
                            .hover(|s| s.bg(rgb(t.border)))
                            .tooltip(self.tooltip("Close (Escape)", None))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.find.open = false;
                                this.focus = Focus::Editor;
                                cx.notify();
                            }))
                            .child(crate::icons::glyph(crate::codicons::CLOSE, 1.0, t.dim)),
                    ),
            );
        if f.replace_visible {
            widget = widget.child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_1()
                    .child(div().flex_1().min_w_0().child(self.render_input(&f.replacement, "Replace", self.focus == Focus::Find && f.focus_replace)))
                    .child(div().id("replace-one").px_1().cursor_pointer().text_xs().text_color(rgb(t.text)).on_click(cx.listener(|this, _, _, cx| {
                        this.find.focus_replace = true;
                        this.replace_current(cx);
                    })).child("Replace"))
                    .child(div().id("replace-all").px_1().cursor_pointer().text_xs().text_color(rgb(t.text)).on_click(cx.listener(|this, _, _, cx| this.replace_all(cx))).child("All")),
            );
        }
        widget
    }

    // ---- sidebar ----

    fn render_sidebar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = &self.theme;
        let changes = self.git.as_ref().map_or(0, |g| g.status.changes.len());
        let tab = |icon: char, name: &'static str, command: Command, view: SidebarView, badge: usize, cx: &mut Context<Self>| {
            let active = self.sidebar_view == view;
            div()
                .id(("sidebar-tab", view as usize))
                .px_4()
                .h(rems(2.5))
                .flex()
                .flex_row()
                .items_center()
                .gap(rems(0.25))
                .cursor_pointer()
                .border_b_1()
                .border_color(rgb(if active { t.info } else { t.sidebar }))
                .hover(|s| s.bg(rgb(t.tab_active)))
                .tooltip(self.tooltip(name, Some(command)))
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.sidebar_view = view;
                    this.focus = match view {
                        SidebarView::Search => Focus::Search,
                        SidebarView::Git => Focus::Scm,
                        SidebarView::Extensions => Focus::Extensions,
                        SidebarView::Explorer => Focus::Explorer,
                    };
                    if view == SidebarView::Extensions {
                        this.open_extensions_view();
                    }
                    cx.notify();
                }))
                .child(crate::icons::glyph(icon, 1.375, if active { t.text } else { t.dim }))
                .children((badge > 0).then(|| div().min_w(rems(1.125)).h(rems(1.125)).px_1().rounded_full().flex().items_center().justify_center().text_xs().bg(rgb(t.button)).text_color(rgb(t.button_text)).child(badge.to_string())))
        };
        let body: AnyElement = match self.sidebar_view {
            SidebarView::Explorer => self.render_explorer(cx).into_any_element(),
            SidebarView::Search => self.render_search(cx).into_any_element(),
            SidebarView::Git => self.render_scm(cx),
            SidebarView::Extensions => self.render_extensions(cx).into_any_element(),
        };
        div()
            .flex()
            .flex_col()
            .flex_none()
            .w(px(self.sidebar_width()))
            .h_full()
            .relative()
            .bg(rgb(t.sidebar))
            .border_r_1()
            .border_color(rgb(t.border))
            .child(self.resize_handle(("resize", 0), Resize::Sidebar, Edge::Right, cx))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .child(tab(crate::codicons::FILES, "Explorer", Command::FocusExplorer, SidebarView::Explorer, 0, cx))
                    .child(tab(crate::codicons::SEARCH, "Search", Command::FindInFiles, SidebarView::Search, 0, cx))
                    .child(tab(crate::codicons::SOURCE_CONTROL, "Source Control", Command::ShowGit, SidebarView::Git, changes, cx))
                    .child(tab(crate::codicons::EXTENSIONS, "Extensions", Command::ShowExtensions, SidebarView::Extensions, 0, cx)),
            )
            .child(body)
    }

    fn render_explorer(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = &self.theme;
        let root_name = self.root().file_name().map(|n| n.to_string_lossy().to_uppercase()).unwrap_or_else(|| self.root().display().to_string());
        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .child(div().px_3().py_1().text_xs().text_color(rgb(t.dim)).child(root_name))
            .child(
                div().flex_1().min_h_0().child(
                    uniform_list(
                        "tree",
                        self.tree.entries.len(),
                        cx.processor(|this, range: Range<usize>, _window, cx| {
                            let t = this.theme.clone();
                            let line_h = this.line_height().max(26.0);
                            range
                                .map(|ix| {
                                    let entry = this.tree.entries[ix].clone();
                                    let selected = this.explorer_selected == Some(ix);
                                    let open = this.active_doc().is_some_and(|d| crate::workspace::same_path(&d.path, &entry.path));
                                    let (path, is_dir) = (entry.path.clone(), entry.is_dir);
                                    let decor = this.git.as_ref().and_then(|g| g.decor.get(&entry.path)).copied();
                                    let name_color = match decor {
                                        Some('U' | 'A') => t.git_added,
                                        Some('D') => t.git_deleted,
                                        Some('!') => t.error,
                                        Some(_) => t.git_modified,
                                        None => t.text,
                                    };
                                    let badge = decor.filter(|c| *c != '•').map(|c| c.to_string()).unwrap_or_default();
                                    div()
                                        .id(("entry", ix))
                                        .w_full()
                                        .h(px(line_h))
                                        .pl(px(8.0 + entry.depth as f32 * 14.0))
                                        .cursor_pointer()
                                        .whitespace_nowrap()
                                        .when(selected, |d| d.bg(rgb(if this.focus == Focus::Explorer { t.selection } else { t.tab_active })))
                                        .when(open && !selected, |d| d.bg(rgb(t.tab_active)))
                                        .hover(|s| s.bg(rgb(t.tab_active)))
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            this.focus = Focus::Explorer;
                                            this.explorer_selected = Some(ix);
                                            if is_dir {
                                                this.tree.toggle(&path);
                                            } else {
                                                this.open_file(&path, None, cx);
                                            }
                                            cx.notify();
                                        }))
                                        .on_mouse_down(MouseButton::Right, cx.listener(move |this, ev: &MouseDownEvent, _, cx| {
                                            // The workspace-level right click is for the editor only.
                                            cx.stop_propagation();
                                            this.focus = Focus::Explorer;
                                            this.explorer_selected = Some(ix);
                                            this.open_explorer_menu(ix, ev.position, cx);
                                        }))
                                        .flex()
                                        .flex_row()
                                        .justify_between()
                                        .pr_2()
                                        .child(
                                            div()
                                                .flex()
                                                .flex_row()
                                                .items_center()
                                                .gap(rems(0.25))
                                                .text_color(rgb(name_color))
                                                .child(if entry.is_dir {
                                                    div().w(rems(1.5)).flex_none().flex().justify_center().child(crate::icons::glyph(if entry.expanded { crate::codicons::CHEVRON_DOWN } else { crate::codicons::CHEVRON_RIGHT }, 1.0, t.dim))
                                                } else {
                                                    file_icon(this.icon_theme.as_ref(), &entry.name)
                                                })
                                                .children(entry.is_dir.then(|| this.icon_theme.as_ref().and_then(|i| i.folder_icon(&entry.name, entry.expanded)).map(icon_image)).flatten())
                                                .child(entry.name.clone()),
                                        )
                                        .child(div().text_xs().text_color(rgb(name_color)).child(badge))
                                })
                                .collect()
                        }),
                    )
                    .track_scroll(self.explorer_scroll.clone())
                    .size_full(),
                ),
            )
    }

    fn render_search(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = &self.theme;
        let s = &self.search;
        let toggle = |icon: char, tip: &'static str, on: bool, which: usize, cx: &mut Context<Self>| {
            div()
                .id(("search-toggle", which))
                .min_w(rems(1.75))
                .h(rems(1.625))
                .flex()
                .items_center()
                .justify_center()
                .px_1()
                .rounded_sm()
                .cursor_pointer()
                .bg(rgb(if on { t.selection } else { t.sidebar }))
                .tooltip(self.tooltip(tip, None))
                .on_click(cx.listener(move |this, _, _, cx| {
                    match which {
                        0 => this.search.options.case_sensitive = !this.search.options.case_sensitive,
                        1 => this.search.options.whole_word = !this.search.options.whole_word,
                        _ => this.search.options.regex = !this.search.options.regex,
                    }
                    this.start_search();
                    cx.notify();
                }))
                .child(crate::icons::glyph(icon, 1.0, if on { t.text } else { t.dim }))
        };
        let files = s.hits.iter().map(|h| &h.path).collect::<std::collections::BTreeSet<_>>().len();
        let status = if let Some(e) = &s.error {
            e.clone()
        } else if s.running {
            format!("Searching... {} results", s.hits.len())
        } else if s.query.text().is_empty() {
            "Type a query and press Enter".to_owned()
        } else if s.hits.is_empty() {
            "No results found".to_owned()
        } else {
            format!("{} results in {files} files{}", s.hits.len(), if s.capped { " (limit reached)" } else { "" })
        };
        let rows = s.rows();
        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .gap_1()
            .p_2()
            .child(
                div()
                    .flex()
                    .flex_row()
                    .gap_1()
                    .items_center()
                    .on_mouse_down(MouseButton::Left, cx.listener(|this, _: &MouseDownEvent, _, cx| {
                        this.focus = Focus::Search;
                        cx.notify();
                    }))
                    .child(
                        div()
                            .id("search-replace-toggle")
                            .w(rems(1.5))
                            .h(rems(1.625))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded_sm()
                            .cursor_pointer()
                            .text_color(rgb(t.dim))
                            .hover(|s| s.bg(rgb(t.tab_active)))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.search.replace_open = !this.search.replace_open;
                                this.search.replace_focused = this.search.replace_open;
                                this.focus = Focus::Search;
                                cx.notify();
                            }))
                            .tooltip(self.tooltip("Toggle Replace", Some(Command::ReplaceInFiles)))
                            .child(crate::icons::glyph(if s.replace_open { crate::codicons::CHEVRON_DOWN } else { crate::codicons::CHEVRON_RIGHT }, 1.0, t.dim)),
                    )
                    .child(div().flex_1().min_w_0().on_mouse_down(MouseButton::Left, cx.listener(|this, _: &MouseDownEvent, _, cx| {
                        this.search.replace_focused = false;
                        this.focus = Focus::Search;
                        cx.stop_propagation();
                        cx.notify();
                    })).child(self.render_input(&s.query, "Search", self.focus == Focus::Search && !s.replace_focused)))
                    .child(toggle(crate::codicons::CASE_SENSITIVE, "Match Case (Alt+C)", s.options.case_sensitive, 0, cx))
                    .child(toggle(crate::codicons::WHOLE_WORD, "Match Whole Word (Alt+W)", s.options.whole_word, 1, cx))
                    .child(toggle(crate::codicons::REGEX, "Use Regular Expression (Alt+R)", s.options.regex, 2, cx)),
            )
            .children(s.replace_open.then(|| {
                div()
                    .flex()
                    .flex_row()
                    .gap_1()
                    .items_center()
                    .pl(rems(1.75))
                    .child(div().flex_1().min_w_0().on_mouse_down(MouseButton::Left, cx.listener(|this, _: &MouseDownEvent, _, cx| {
                        this.search.replace_focused = true;
                        this.focus = Focus::Search;
                        cx.stop_propagation();
                        cx.notify();
                    })).child(self.render_input(&s.replace, "Replace", self.focus == Focus::Search && s.replace_focused)))
                    .child(
                        div()
                            .id("search-replace-all")
                            .h(rems(1.625))
                            .px_2()
                            .flex()
                            .items_center()
                            .rounded_sm()
                            .text_sm()
                            .when(!s.hits.is_empty(), |d| d.cursor_pointer().hover(|s| s.opacity(0.85)))
                            .bg(rgb(if s.hits.is_empty() { t.tab_active } else { t.button }))
                            .text_color(rgb(if s.hits.is_empty() { t.dim } else { t.button_text }))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.prompt_replace_all();
                                cx.notify();
                            }))
                            .child("Replace All"),
                    )
            }))
            .child(div().text_xs().text_color(rgb(if s.error.is_some() { t.error } else { t.dim })).child(status))
            .child(
                div().flex_1().min_h_0().child(
                    uniform_list(
                        "search-results",
                        rows.len(),
                        cx.processor(move |this, range: Range<usize>, _window, cx| {
                            let t = this.theme.clone();
                            range
                                .map(|ix| {
                                    let row = div().id(("hit", ix)).w_full().h(rems(1.625)).px_2().flex().flex_row().items_center().gap_1().overflow_hidden().cursor_pointer();
                                    match rows[ix] {
                                        SearchRow::File { first, count } => {
                                            let path = &this.search.hits[first].path;
                                            let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                                            let rel = path.strip_prefix(this.root()).unwrap_or(path);
                                            let dir = rel.parent().map(|p| p.to_string_lossy().replace('\\', "/")).unwrap_or_default();
                                            row.hover(|s| s.bg(rgb(t.tab_active)))
                                                .on_click(cx.listener(move |this, _, _, cx| this.open_hit(first, cx)))
                                                .child(file_icon(this.icon_theme.as_ref(), &name))
                                                .child(div().flex_none().whitespace_nowrap().font_weight(FontWeight::MEDIUM).child(name))
                                                .child(div().flex_1().min_w_0().overflow_hidden().whitespace_nowrap().text_xs().text_color(rgb(t.dim)).child(dir))
                                                .child(
                                                    div()
                                                        .flex_none()
                                                        .min_w(rems(1.25))
                                                        .px_1()
                                                        .rounded_full()
                                                        .bg(rgb(t.tab_active))
                                                        .text_xs()
                                                        .text_center()
                                                        .text_color(rgb(t.text))
                                                        .child(count.to_string()),
                                                )
                                        }
                                        SearchRow::Line { first, end } => {
                                            let selected = (first..end).contains(&this.search.selected);
                                            let hit = &this.search.hits[first];
                                            let chars: Vec<char> = hit.preview.chars().collect();
                                            // Skip the indentation; if the match sits far right, start a little before it.
                                            let mut skip = chars.iter().take_while(|c| c.is_whitespace()).count();
                                            let cut = hit.col > skip + 16;
                                            if cut {
                                                skip = hit.col - 16;
                                            }
                                            let mut text = String::new();
                                            if cut {
                                                text.push('…');
                                            }
                                            let lead = usize::from(cut);
                                            text.extend(&chars[skip.min(chars.len())..]);
                                            let bytes: Vec<usize> = text.char_indices().map(|(b, _)| b).chain([text.len()]).collect();
                                            let shown = bytes.len() - 1;
                                            let marks: Vec<(Range<usize>, HighlightStyle)> = this.search.hits[first..end]
                                                .iter()
                                                .filter(|h| h.col >= skip)
                                                .filter_map(|h| {
                                                    let s = h.col - skip + lead;
                                                    (s < shown).then(|| {
                                                        let e = (s + h.len).min(shown);
                                                        (bytes[s]..bytes[e], HighlightStyle { background_color: Some(rgb(t.find_current).into()), ..Default::default() })
                                                    })
                                                })
                                                .collect();
                                            row.pl(rems(2.25))
                                                .when(selected, |d| d.bg(rgb(t.selection)))
                                                .hover(|s| s.bg(rgb(t.tab_active)))
                                                .on_click(cx.listener(move |this, _, _, cx| this.open_hit(first, cx)))
                                                .child(div().flex_1().min_w_0().overflow_hidden().whitespace_nowrap().text_sm().child(StyledText::new(text).with_highlights(marks)))
                                        }
                                    }
                                })
                                .collect()
                        }),
                    )
                    .track_scroll(s.scroll.clone())
                    .size_full(),
                ),
            )
    }

    // ---- source control ----

    /// Characters that fit on one row of the commit message box.
    pub fn scm_wrap_cols(&self) -> usize {
        let inner = self.sidebar_width() - 1.0 - 2.0 * SCM_PAD - 2.0 * (1.0 + INPUT_PAD);
        ((inner / f32::from(self.char_width).max(1.0)).floor() as usize).max(8)
    }

    /// The commit message box: wrapped rows, caret, selection, placeholder, mouse selection.
    fn render_message_box(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = self.theme.clone();
        let focused = self.focus == Focus::Scm;
        let area = &self.scm_message;
        let rows = area.rows();
        let chars: Vec<char> = area.text().chars().collect();
        let (caret_row, _) = area.cursor_row_col();
        let (selection, cursor) = (area.selection(), area.cursor());
        let line_h = self.line_height();
        let char_w = f32::from(self.char_width).max(1.0);
        let text_left = SCM_PAD + 1.0 + INPUT_PAD;
        let placeholder = "Message (Ctrl+Enter to commit)";
        let column_at = move |x: Pixels| (((f32::from(x) - text_left) / char_w).round().max(0.0)) as usize;

        let row_elements = rows.iter().enumerate().map(|(ri, r)| {
            let caret = |on: bool| div().w(px(1.0)).h(px(line_h - 4.0)).flex_none().bg(rgb(if on { t.caret } else { t.bg }));
            let mut row = div()
                .id(("scm-input-row", ri))
                .flex()
                .flex_row()
                .items_center()
                .h(px(line_h))
                .w_full()
                .whitespace_nowrap()
                .cursor_text()
                .on_mouse_down(MouseButton::Left, cx.listener(move |this, ev: &MouseDownEvent, _, cx| {
                    cx.stop_propagation();
                    this.focus = Focus::Scm;
                    let offset = this.scm_message.offset_at(ri, column_at(ev.position.x));
                    match ev.click_count {
                        1 => this.scm_message.set_cursor(offset, ev.modifiers.shift),
                        2 => this.scm_message.select_word_at(offset),
                        _ => this.scm_message.select_all(),
                    }
                    this.scm_dragging = ev.click_count == 1;
                    cx.notify();
                }))
                .on_mouse_move(cx.listener(move |this, ev: &MouseMoveEvent, _, cx| {
                    if !this.scm_dragging {
                        return;
                    }
                    if ev.pressed_button != Some(MouseButton::Left) {
                        this.scm_dragging = false;
                        return;
                    }
                    let offset = this.scm_message.offset_at(ri, column_at(ev.position.x));
                    this.scm_message.set_cursor(offset, true);
                    cx.notify();
                }));
            if area.is_empty() {
                if ri == 0 {
                    row = row.child(caret(focused)).child(div().text_color(rgb(t.dim)).child(placeholder));
                }
                return row;
            }
            // Cut the row at the selection edges and the caret, and draw each piece.
            let mut cuts = vec![r.start, r.end];
            if let Some((a, b)) = selection {
                cuts.extend([a.clamp(r.start, r.end), b.clamp(r.start, r.end)]);
            }
            let has_caret = focused && caret_row == ri;
            if has_caret {
                cuts.push(cursor);
            }
            cuts.sort_unstable();
            cuts.dedup();
            let selected = |from: usize, to: usize| selection.is_some_and(|(a, b)| a <= from && to <= b);
            let mut caret_drawn = false;
            for (i, &at) in cuts.iter().enumerate() {
                if has_caret && at == cursor && !caret_drawn {
                    row = row.child(caret(true));
                    caret_drawn = true;
                }
                let Some(&next) = cuts.get(i + 1) else { continue };
                let piece: String = chars[at..next].iter().collect();
                row = row.child(div().when(selected(at, next), |d| d.bg(rgb(t.selection))).child(piece));
            }
            // A selected empty row (a blank line inside the selection) still shows as a small block.
            if r.is_empty() && selection.is_some_and(|(a, b)| a <= r.start && r.start < b) {
                row = row.child(div().w(px(char_w * 0.6)).h(px(line_h - 4.0)).bg(rgb(t.selection)));
            }
            row
        });

        div()
            .id("scm-input")
            .w_full()
            .px(px(INPUT_PAD))
            .py(rems(0.25))
            .bg(rgb(t.bg))
            .border_1()
            .border_color(rgb(if focused { t.info } else { t.border }))
            .rounded_sm()
            .overflow_hidden()
            .flex()
            .flex_col()
            .on_mouse_down(MouseButton::Left, cx.listener(|this, _: &MouseDownEvent, _, cx| {
                cx.stop_propagation();
                this.focus = Focus::Scm;
                cx.notify();
            }))
            .children(row_elements)
    }

    fn render_scm(&self, cx: &mut Context<Self>) -> AnyElement {
        let t = self.theme.clone();
        let Some(git) = self.git.as_ref() else {
            return div()
                .p_3()
                .flex()
                .flex_col()
                .gap_2()
                .text_color(rgb(t.dim))
                .child("The folder currently open doesn't have a git repository.")
                .child(
                    div()
                        .id("git-init")
                        .px_2()
                        .py_1()
                        .rounded_sm()
                        .cursor_pointer()
                        .bg(rgb(t.button))
                        .text_color(rgb(t.button_text))
                        .on_click(cx.listener(|this, _, _, cx| this.run_command(Command::GitInit, false, cx)))
                        .child("Initialize Repository"),
                )
                .into_any_element();
        };
        #[derive(Clone)]
        enum Row {
            Header { staged: bool, count: usize, open: bool },
            File(Change, bool),
        }
        let staged: Vec<Change> = git.status.staged().cloned().collect();
        let unstaged: Vec<Change> = git.status.unstaged().cloned().collect();
        let mut rows: Vec<Row> = Vec::new();
        for (is_staged, list, open) in [(true, &staged, self.scm_staged_open), (false, &unstaged, self.scm_changes_open)] {
            if list.is_empty() {
                continue;
            }
            rows.push(Row::Header { staged: is_staged, count: list.len(), open });
            if open {
                rows.extend(list.iter().cloned().map(|c| Row::File(c, is_staged)));
            }
        }
        let branch = git.status.branch.clone().unwrap_or_else(|| format!("detached at {}", git.status.head));
        let sync = if git.status.upstream.is_some() { format!("  ↓{} ↑{}", git.status.behind, git.status.ahead) } else { String::new() };
        // An operation (commit, push, publish, sync...) running in the background: its name and how long it has run.
        let working = git.job.clone().map(|name| (name, git.job_started.map_or(0, |at| at.elapsed().as_millis() as u64)));
        let busy = working.is_some();
        let can_commit = !staged.is_empty() && !busy;
        let nothing = staged.is_empty() && unstaged.is_empty();
        // With nothing staged, the primary button is what comes after committing: publish a branch
        // that has no upstream yet, or sync one that is ahead of or behind it.
        let (ahead, behind) = (git.status.ahead, git.status.behind);
        let publish = git.status.branch.is_some() && git.status.upstream.is_none() && git.status.has_remote && !git.status.head.is_empty();
        let sync_label = if publish {
            Some((crate::codicons::CLOUD_UPLOAD, "Publish Branch".to_owned()))
        } else if git.status.upstream.is_some() && (ahead > 0 || behind > 0) {
            Some((crate::codicons::SYNC, format!("Sync Changes  ↓{behind} ↑{ahead}")))
        } else {
            None
        };
        let sync_label = sync_label.filter(|_| !can_commit && !busy);

        // A small square icon button for the repository row.
        let icon = |id: &'static str, glyph: char, name: &'static str, enabled: bool, cx: &mut Context<Self>, command: Command| {
            div()
                .id(id)
                .w(rems(1.875))
                .h(rems(1.875))
                .flex()
                .items_center()
                .justify_center()
                .rounded_sm()
                .tooltip(self.tooltip(name, Some(command)))
                .when(enabled, |d| d.cursor_pointer().hover(|s| s.bg(rgb(t.tab_active))))
                .on_click(cx.listener(move |this, _, _, cx| {
                    if enabled {
                        this.run_command(command, false, cx);
                    }
                }))
                .child(crate::icons::glyph(glyph, 1.125, if enabled { t.text } else { t.dim }))
        };

        let sync_active = sync_label.is_some();
        let commit_button = div()
            .relative()
            .flex()
            .flex_row()
            .w_full()
            .h(rems(2.125))
            .rounded_sm()
            .overflow_hidden()
            .bg(rgb(t.button))
            .text_color(rgb(t.button_text))
            .when(!can_commit && sync_label.is_none() && !busy, |d| d.opacity(0.5))
            .when(busy, |d| d.opacity(0.9))
            // While work runs, a bar slides along the bottom edge of the button.
            .children(working.as_ref().map(|(_, ms)| {
                let along = (*ms % 1400) as f32 / 1400.0;
                div().absolute().bottom_0().left(relative(along * 0.75)).w(relative(0.25)).h(px(3.0)).rounded_full().bg(rgba(0xffffffdd))
            }))
            .child(
                div()
                    .id("git-commit")
                    .flex_1()
                    .flex()
                    .items_center()
                    .justify_center()
                    .when((can_commit || sync_label.is_some()) && !busy, |d| d.cursor_pointer().hover(|s| s.opacity(0.85)))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        // Nothing starts while another operation is still running.
                        if this.git.as_ref().is_some_and(|g| g.job.is_some()) {
                            return;
                        }
                        let command = if publish { Command::GitPush } else if sync_active { Command::GitSync } else { Command::GitCommit };
                        this.run_command(command, false, cx);
                    }))
                    .child(if let Some((name, ms)) = working.clone() {
                        // "Committing" with dots that count up, and the icon pulsing: it is plainly doing something.
                        let dots = ".".repeat(((ms / 350) % 4) as usize);
                        let verb = name.trim_end_matches('…').to_owned();
                        let pulse = if (ms / 400) % 2 == 0 { 1.0 } else { 0.45 };
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap(rems(0.5))
                            .child(div().opacity(pulse).child(crate::icons::glyph(crate::codicons::SYNC, 1.0, t.button_text)))
                            .child(div().min_w(rems(9.0)).child(format!("{verb}{dots}")))
                    } else {
                        let (icon, label) = sync_label.clone().unwrap_or((crate::codicons::CHECK, "Commit".to_owned()));
                        div().flex().flex_row().items_center().gap(rems(0.5)).child(crate::icons::glyph(icon, 1.0, t.button_text)).child(label)
                    }),
            )
            .child(
                div()
                    .id("git-commit-more")
                    .w(rems(2.375))
                    .text_size(rems(1.0))
                    .flex()
                    .items_center()
                    .justify_center()
                    .cursor_pointer()
                    .border_l_1()
                    .border_color(rgba(0xffffff55))
                    .hover(|s| s.opacity(0.85))
                    .tooltip(self.tooltip("More Commit Actions...", None))
                    .on_mouse_down(MouseButton::Left, cx.listener(|this, ev: &MouseDownEvent, _, cx| {
                        cx.stop_propagation();
                        this.open_scm_menu(ev.position, cx);
                    }))
                    .child(crate::icons::glyph(crate::codicons::CHEVRON_DOWN, 1.0, t.button_text)),
            );

        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .justify_between()
                    .h(rems(2.375))
                    .px(px(SCM_PAD))
                    .text_sm()
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap(rems(0.375))
                            .text_color(rgb(t.text))
                            .child(crate::icons::glyph(crate::codicons::GIT_BRANCH, 1.0, t.dim))
                            .child(format!("{branch}{sync}")),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .child(icon("git-ai", crate::codicons::SPARKLE, if self.ai_pending { "Writing a commit message..." } else { "Write Commit Message with AI" }, can_commit && !self.ai_pending, cx, Command::GitGenerateMessage))
                            .child(icon("git-commit-icon", crate::codicons::CHECK, "Commit", can_commit, cx, Command::GitCommit))
                            .child(icon("git-refresh", crate::codicons::REFRESH, "Refresh", true, cx, Command::GitRefresh))
                            .child(
                                div()
                                    .id("git-more")
                                    .w(rems(1.875))
                                    .h(rems(1.875))
                                    .text_size(rems(1.125))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .rounded_sm()
                                    .cursor_pointer()
                                    .hover(|s| s.bg(rgb(t.tab_active)))
                                    .tooltip(self.tooltip("More Actions...", None))
                                    .on_mouse_down(MouseButton::Left, cx.listener(|this, ev: &MouseDownEvent, _, cx| {
                                        cx.stop_propagation();
                                        this.open_scm_menu(ev.position, cx);
                                    }))
                                    .child(crate::icons::glyph(crate::codicons::ELLIPSIS, 1.125, t.text)),
                            ),
                    ),
            )
            .children(self.render_git_state_banner(cx))
            .child(div().flex().flex_col().gap(rems(0.5)).px(px(SCM_PAD)).pb(rems(0.625)).child(self.render_message_box(cx)).child(commit_button).children(busy.then(|| {
                div()
                    .id("git-cancel")
                    .w(rems(6.0))
                    .justify_center()
                    .px_3()
                    .h(rems(1.5))
                    .flex()
                    .items_center()
                    .rounded_sm()
                    .cursor_pointer()
                    .text_xs()
                    .bg(rgb(t.tab_active))
                    .text_color(rgb(t.text))
                    .hover(|s| s.bg(rgb(t.border)))
                    .tooltip(self.tooltip("Cancel the running git operation", Some(Command::GitCancel)))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.git_cancel();
                        cx.notify();
                    }))
                    .child("Cancel")
            })))
            .children(nothing.then(|| div().px(px(SCM_PAD)).pt_1().text_xs().text_color(rgb(t.dim)).child("No changes detected.")))
            .child(
                div().flex_1().min_h_0().child(
                    uniform_list(
                        "scm",
                        rows.len(),
                        cx.processor(move |this, range: Range<usize>, _window, cx| {
                            let t = this.theme.clone();
                            range
                                .filter_map(|i| rows.get(i).cloned().map(|r| (i, r)))
                                .map(|(i, row)| match row {
                                    Row::Header { staged, count, open } => {
                                        let (label, glyph, tip, command) = if staged {
                                            ("STAGED CHANGES", crate::codicons::REMOVE, "Unstage All Changes", Command::GitUnstageAll)
                                        } else {
                                            ("CHANGES", crate::codicons::ADD, "Stage All Changes", Command::GitStageAll)
                                        };
                                        let group = format!("scm-head-{}", usize::from(staged));
                                        div()
                                            .id(("scm-head", usize::from(staged)))
                                            .group(group.clone())
                                            .w_full()
                                            .flex()
                                            .flex_row()
                                            .items_center()
                                            .gap(rems(0.375))
                                            .h(rems(1.75))
                                            .px(rems(0.375))
                                            .cursor_pointer()
                                            .text_sm()
                                            .font_weight(FontWeight::BOLD)
                                            .text_color(rgb(t.text))
                                            .hover(|s| s.bg(rgb(t.tab_active)))
                                            .on_click(cx.listener(move |this, _, _, cx| {
                                                if staged {
                                                    this.scm_staged_open = !this.scm_staged_open;
                                                } else {
                                                    this.scm_changes_open = !this.scm_changes_open;
                                                }
                                                cx.notify();
                                            }))
                                            .child(crate::icons::glyph(if open { crate::codicons::CHEVRON_DOWN } else { crate::codicons::CHEVRON_RIGHT }, 1.0, t.dim))
                                            .child(div().flex_1().whitespace_nowrap().child(label))
                                            .child(
                                                div()
                                                    .id(("scm-bulk", usize::from(staged)))
                                                    .invisible()
                                                    .group_hover(group, |s| s.visible())
                                                    .w(rems(1.625))
                                                    .h(rems(1.5))
                                                    .flex()
                                                    .items_center()
                                                    .justify_center()
                                                    .rounded_sm()
                                                    .hover(|s| s.bg(rgb(t.border)))
                                                    .cursor_pointer()
                                                    .tooltip(this.tooltip(tip, Some(command)))
                                                    .on_click(cx.listener(move |this, _, _, cx| {
                                                        cx.stop_propagation();
                                                        this.run_command(command, false, cx);
                                                    }))
                                                    .child(crate::icons::glyph(glyph, 1.0, t.text)),
                                            )
                                            .child(
                                                div()
                                                    .min_w(rems(1.375))
                                                    .px_1()
                                                    .rounded_full()
                                                    .flex()
                                                    .justify_center()
                                                    .bg(rgb(t.border))
                                                    .text_color(rgb(t.text))
                                                    .font_weight(FontWeight::NORMAL)
                                                    .child(count.to_string()),
                                            )
                                            .into_any_element()
                                    }
                                    Row::File(change, is_staged) => {
                                        let letter = if is_staged { change.index.map_or('?', |k| k.letter()) } else { change.letter() };
                                        let color = match letter {
                                            'U' | 'A' => t.git_added,
                                            'D' => t.git_deleted,
                                            '!' => t.error,
                                            _ => t.git_modified,
                                        };
                                        let (name, dir) = match change.path.rsplit_once('/') {
                                            Some((d, n)) => (n.to_owned(), d.to_owned()),
                                            None => (change.path.clone(), String::new()),
                                        };
                                        let untracked = change.untracked;
                                        let conflicted = change.conflicted;
                                        let (open_rel, stage_rel, discard_rel) = (change.path.clone(), change.path.clone(), change.path.clone());
                                        let group = format!("scm-file-{i}");
                                        let action = |id: usize, glyph: char, tip: &'static str, cx: &mut Context<Workspace>, f: Box<dyn Fn(&mut Workspace) + 'static>| {
                                            div()
                                                .id(("scm-action", i * 4 + id))
                                                .w(rems(1.625))
                                                .h(rems(1.5))
                                                .tooltip(this.tooltip(tip, None))
                                                .flex()
                                                .items_center()
                                                .justify_center()
                                                .rounded_sm()
                                                .cursor_pointer()
                                                .hover(|s| s.bg(rgb(t.border)))
                                                .on_click(cx.listener(move |this, _, _, cx| {
                                                    cx.stop_propagation();
                                                    f(this);
                                                    cx.notify();
                                                }))
                                                .child(crate::icons::glyph(glyph, 1.0, t.text))
                                        };
                                        let mut actions = div().flex().flex_row().flex_none().invisible().group_hover(group.clone(), |s| s.visible());
                                        if is_staged {
                                            let rel = stage_rel.clone();
                                            actions = actions.child(action(1, crate::codicons::REMOVE, "Unstage Changes", cx, Box::new(move |w| w.scm_unstage(rel.clone()))));
                                        } else {
                                            let rel = discard_rel.clone();
                                            actions = actions.child(action(0, crate::codicons::DISCARD, "Discard Changes", cx, Box::new(move |w| w.scm_confirm_discard(rel.clone(), untracked))));
                                            let rel = stage_rel.clone();
                                            actions = actions.child(action(1, crate::codicons::ADD, "Stage Changes", cx, Box::new(move |w| w.scm_stage(rel.clone()))));
                                        }
                                        div()
                                            .id(("scm-file", i))
                                            .group(group)
                                            .w_full()
                                            .flex()
                                            .flex_row()
                                            .items_center()
                                            .gap(rems(0.375))
                                            .h(rems(1.75))
                                            .pl(rems(1.75))
                                            .pr(rems(0.5))
                                            .cursor_pointer()
                                            .whitespace_nowrap()
                                            .hover(|s| s.bg(rgb(t.tab_active)))
                                            .on_click(cx.listener(move |this, _, _, cx| {
                                                if conflicted {
                                                    this.open_conflict_page(open_rel.clone());
                                                } else {
                                                    this.git_open_diff(open_rel.clone(), is_staged, untracked);
                                                }
                                                cx.notify();
                                            }))
                                            .child(
                                                div()
                                                    .flex_1()
                                                    .min_w_0()
                                                    .overflow_hidden()
                                                    .flex()
                                                    .flex_row()
                                                    .gap(rems(0.375))
                                                    .items_center()
                                                    .child(file_icon(this.icon_theme.as_ref(), &name))
                                                    .child(div().text_color(rgb(color)).child(name))
                                                    .child(div().text_sm().text_color(rgb(t.dim)).child(dir)),
                                            )
                                            .child(actions)
                                            .child(div().w(rems(1.0)).flex_none().flex().justify_center().text_sm().text_color(rgb(color)).child(letter.to_string()))
                                            .into_any_element()
                                    }
                                })
                                .collect::<Vec<AnyElement>>()
                        }),
                    )
                    .track_scroll(self.scm_scroll.clone())
                    .size_full(),
                ),
            )
            .into_any_element()
    }

    /// While a merge or rebase waits (usually for conflicts): what is going on and how to go on.
    fn render_git_state_banner(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        use forge_core::git::RepoState;
        let git = self.git.as_ref()?;
        let label = git.status.state.label()?;
        let t = &self.theme;
        let conflicts = git.status.changes.iter().filter(|c| c.conflicted).count();
        let (continue_cmd, skip_cmd, abort_cmd) = match git.status.state {
            RepoState::Rebase { .. } => (Command::GitRebaseContinue, Some(Command::GitRebaseSkip), Command::GitRebaseAbort),
            _ => (Command::GitMergeContinue, None, Command::GitMergeAbort),
        };
        let busy = git.job.is_some();
        let button = |id: &'static str, text: &str, command: Command, primary: bool, cx: &mut Context<Self>| {
            div()
                .id(id)
                .px_2()
                .h(rems(1.5))
                .flex()
                .items_center()
                .rounded_sm()
                .text_xs()
                .when(!busy, |d| d.cursor_pointer().hover(|s| s.opacity(0.85)))
                .when(busy, |d| d.opacity(0.5))
                .bg(rgb(if primary { t.button } else { t.tab_active }))
                .text_color(rgb(if primary { t.button_text } else { t.text }))
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.run_command(command, false, cx);
                    cx.notify();
                }))
                .child(text.to_owned())
        };
        let hint = if conflicts > 0 {
            format!("{conflicts} file{} with conflicts: click one to resolve it.", if conflicts == 1 { "" } else { "s" })
        } else {
            "No conflicts left. Continue when you are ready.".to_owned()
        };
        Some(
            div()
                .mx(px(SCM_PAD))
                .mb(rems(0.5))
                .p_2()
                .flex()
                .flex_col()
                .gap_1()
                .rounded_sm()
                .border_1()
                .border_color(rgb(t.warning))
                .bg(rgba((t.warning << 8) | 0x1a))
                .child(div().text_sm().font_weight(FontWeight::BOLD).child(label))
                .child(div().text_xs().text_color(rgb(t.dim)).child(hint))
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .flex_wrap()
                        .gap_1()
                        .child(button("git-state-continue", "Continue", continue_cmd, conflicts == 0, cx))
                        .children(skip_cmd.map(|c| button("git-state-skip", "Skip", c, false, cx)))
                        .child(button("git-state-abort", "Abort", abort_cmd, false, cx)),
                )
                .into_any_element(),
        )
    }

    // ---- bottom panel ----

    fn render_panel(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = &self.theme;
        let problem_count: usize = self.docs.iter().map(|d| d.diagnostics.iter().filter(|x| x.severity != Severity::Hint).count()).sum();
        let tab = |label: String, view: PanelView, cx: &mut Context<Self>| {
            let active = self.panel_view == view;
            div()
                .id(("panel-tab", view as usize))
                .px_3()
                .cursor_pointer()
                .text_xs()
                .text_color(rgb(if active { t.text } else { t.dim }))
                .border_b_1()
                .border_color(rgb(if active { t.info } else { t.sidebar }))
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.panel_view = view;
                    if view == PanelView::Terminal {
                        if this.terminals.is_empty() {
                            this.new_terminal();
                        }
                        this.focus = Focus::Terminal;
                    }
                    cx.notify();
                }))
                .child(label)
        };
        let body: AnyElement = match self.panel_view {
            PanelView::Terminal => self.render_terminal(cx).into_any_element(),
            PanelView::Problems => self.render_problems(cx).into_any_element(),
            PanelView::References => self.render_references(cx).into_any_element(),
            PanelView::Debug => self.render_debug_panel(cx),
        };
        div()
            .flex()
            .flex_col()
            .flex_none()
            .h(px(self.panel_height))
            .relative()
            .border_t_1()
            .border_color(rgb(t.border))
            .bg(rgb(t.bg))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .h(rems(2.0))
                    .bg(rgb(t.sidebar))
                    .child(tab(format!("PROBLEMS {problem_count}"), PanelView::Problems, cx))
                    .child(tab("TERMINAL".to_owned(), PanelView::Terminal, cx))
                    .child(tab(format!("REFERENCES {}", self.references.len()), PanelView::References, cx))
                    .child(tab("DEBUG".to_owned(), PanelView::Debug, cx))
                    .child(div().flex_1())
                    .children(self.render_terminal_tabs(cx))
                    .child(
                        div()
                            .id("panel-close")
                            .w(rems(2.25))
                            .h(rems(2.0))
                            .flex()
                            .items_center()
                            .justify_center()
                            .cursor_pointer()
                            .hover(|s| s.bg(rgb(t.border)))
                            .tooltip(self.tooltip("Close Panel", Some(Command::TogglePanel)))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.panel_visible = false;
                                if this.focus == Focus::Terminal {
                                    this.focus = Focus::Editor;
                                }
                                cx.notify();
                            }))
                            .child(crate::icons::glyph(crate::codicons::CLOSE, 1.125, t.dim)),
                    ),
            )
            .child(div().flex_1().min_h_0().child(body))
            .child(self.resize_handle(("resize", 1), Resize::Panel, Edge::Top, cx))
    }

    /// Beside the panel tabs while Terminal is showing: one chip per terminal, and a button for a new one.
    fn render_terminal_tabs(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if self.panel_view != PanelView::Terminal {
            return None;
        }
        let t = &self.theme;
        let chips = self.terminals.iter().enumerate().map(|(i, terminal)| {
            let active = i == self.active_terminal;
            div()
                .id(("terminal-chip", i))
                .flex()
                .flex_row()
                .items_center()
                .gap_1()
                .h(rems(1.5))
                .pl_2()
                .pr_1()
                .rounded_sm()
                .cursor_pointer()
                .text_xs()
                .text_color(rgb(if active { t.text } else { t.dim }))
                .bg(rgb(if active { t.tab_active } else { t.sidebar }))
                .hover(|s| s.bg(rgb(t.tab_active)))
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.active_terminal = i;
                    this.focus = Focus::Terminal;
                    cx.notify();
                }))
                .child(format!("{}: {}", i + 1, terminal.title))
                .child(
                    div()
                        .id(("terminal-kill", i))
                        .w(rems(1.125))
                        .h(rems(1.125))
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded_sm()
                        .hover(|s| s.bg(rgb(t.border)))
                        .tooltip(self.tooltip("Kill Terminal", None))
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.kill_terminal(i);
                            cx.notify();
                        }))
                        .child(crate::icons::glyph(crate::codicons::CLOSE, 0.75, t.dim)),
                )
        });
        Some(
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap_1()
                .pr_1()
                .children(chips)
                .child(
                    div()
                        .id("terminal-new")
                        .w(rems(1.75))
                        .h(rems(1.5))
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded_sm()
                        .cursor_pointer()
                        .hover(|s| s.bg(rgb(t.border)))
                        .tooltip(self.tooltip("New Terminal", Some(Command::NewTerminal)))
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.new_terminal();
                            cx.notify();
                        }))
                        .child(crate::icons::glyph(crate::codicons::ADD, 1.0, t.text)),
                )
                .into_any_element(),
        )
    }

    fn render_terminal(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = &self.theme;
        let Some(terminal) = self.terminals.get(self.active_terminal) else {
            return div().p_3().text_color(rgb(t.dim)).child("No terminal is running. Press Ctrl+` to start one.").into_any_element();
        };
        let (rows, _) = terminal.size();
        let (scrolled, history) = terminal.scroll_position();
        // The caret belongs to the live screen: hide it while looking at older output.
        let (cur_row, cur_col, cur_visible) = terminal.cursor();
        let cur_visible = cur_visible && scrolled == 0;
        let line_h = px(self.line_height());
        let focused = self.focus == Focus::Terminal;
        let mut lines: Vec<Div> = Vec::with_capacity(rows as usize);
        for row in 0..rows {
            let runs = terminal.row_runs(row);
            let mut text = String::new();
            let mut styles: Vec<(Range<usize>, HighlightStyle)> = Vec::new();
            for run in &runs {
                let start = text.len();
                text.push_str(&run.text);
                if run.fg.is_some() || run.bg.is_some() || run.bold || run.underline {
                    styles.push((
                        start..text.len(),
                        HighlightStyle {
                            color: run.fg.map(|c| rgb(c).into()),
                            background_color: run.bg.map(|c| rgb(c).into()),
                            font_weight: run.bold.then_some(FontWeight::BOLD),
                            underline: run.underline.then(|| gpui::UnderlineStyle { thickness: px(1.0), ..Default::default() }),
                            ..Default::default()
                        },
                    ));
                }
            }
            let mut line = div().relative().h(line_h).whitespace_nowrap().child(StyledText::new(text).with_highlights(styles));
            if focused && cur_visible && row == cur_row {
                line = line.child(div().absolute().top_0().left(self.char_width * f32::from(cur_col)).w(self.char_width).h(line_h).bg(rgb(t.caret)).opacity(0.6));
            }
            lines.push(line);
        }
        // A thin scroll bar on the right while there is history to scroll through.
        let bar = (history > 0).then(|| {
            let total = (rows as usize + history) as f32;
            let thumb = (rows as f32 / total).clamp(0.05, 1.0);
            let top = (history - scrolled) as f32 / history as f32 * (1.0 - thumb);
            div().absolute().top_0().right_0().bottom_0().w(px(6.0)).child(div().absolute().right(px(1.0)).w(px(4.0)).rounded_full().bg(rgb(if scrolled > 0 { t.dim } else { t.border })).top(relative(top)).h(relative(thumb)))
        });
        div()
            .id("terminal-view")
            .relative()
            .size_full()
            .px_2()
            .py_1()
            .overflow_hidden()
            // Clicking the terminal gives it the keyboard, like clicking an editor does.
            .on_mouse_down(MouseButton::Left, cx.listener(|this, _: &MouseDownEvent, _, cx| {
                this.focus = Focus::Terminal;
                cx.notify();
            }))
            .on_scroll_wheel(cx.listener(|this, ev: &gpui::ScrollWheelEvent, _, cx| {
                let line = px(this.line_height());
                let lines = f32::from(ev.delta.pixel_delta(line).y) / f32::from(line);
                this.scroll_terminal(lines);
                cx.notify();
            }))
            .children(lines)
            .children(bar)
            .into_any_element()
    }

    fn render_problems(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = &self.theme;
        let mut rows: Vec<(std::path::PathBuf, usize, usize, Severity, String)> = Vec::new();
        for doc in &self.docs {
            for d in doc.diagnostics.iter().filter(|d| d.severity != Severity::Hint) {
                rows.push((doc.path.clone(), d.range.start.line, d.range.start.character, d.severity, d.message.lines().next().unwrap_or_default().to_owned()));
            }
        }
        rows.sort_by(|a, b| (a.3 as u8, &a.0, a.1).cmp(&(b.3 as u8, &b.0, b.1)));
        if rows.is_empty() {
            return div().p_3().text_color(rgb(t.dim)).child("No problems have been detected in the open files.").into_any_element();
        }
        let count = rows.len();
        let selected = self.problem_selected;
        uniform_list(
            "problems",
            count,
            cx.processor(move |this, range: Range<usize>, _window, cx| {
                let t = this.theme.clone();
                range
                    .filter_map(|ix| rows.get(ix).cloned().map(|r| (ix, r)))
                    .map(|(ix, (path, line, col, severity, message))| {
                        let color = match severity {
                            Severity::Error => t.error,
                            Severity::Warning => t.warning,
                            _ => t.info,
                        };
                        let rel = path.strip_prefix(this.root()).unwrap_or(&path).to_string_lossy().replace('\\', "/");
                        div()
                            .id(("problem", ix))
                            .w_full()
                            .flex()
                            .flex_row()
                            .gap_2()
                            .px_2()
                            .h(rems(1.375))
                            .items_center()
                            .cursor_pointer()
                            .whitespace_nowrap()
                            .when(selected == ix, |d| d.bg(rgb(t.selection)))
                            .hover(|s| s.bg(rgb(t.tab_active)))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.problem_selected = ix;
                                this.open_file(&path, Some((line, col)), cx);
                            }))
                            .child(div().text_color(rgb(color)).child(if severity == Severity::Error { "✖" } else { "⚠" }))
                            .child(message)
                            .child(div().text_color(rgb(t.dim)).text_xs().child(format!("{rel} [{}:{}]", line + 1, col + 1)))
                    })
                    .collect()
            }),
        )
        .track_scroll(self.problems_scroll.clone())
        .size_full()
        .into_any_element()
    }

    fn render_references(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = &self.theme;
        if self.references.is_empty() {
            return div().p_3().text_color(rgb(t.dim)).child("No references. Put the cursor on a symbol and press Shift+F12.").into_any_element();
        }
        uniform_list(
            "references",
            self.references.len(),
            cx.processor(|this, range: Range<usize>, _window, cx| {
                let t = this.theme.clone();
                range
                    .map(|ix| {
                        let entry = &this.references[ix];
                        let loc = entry.location.clone();
                        let rel = loc.path.strip_prefix(this.root()).unwrap_or(&loc.path).to_string_lossy().replace('\\', "/");
                        div()
                            .id(("reference", ix))
                            .w_full()
                            .flex()
                            .flex_row()
                            .gap_3()
                            .px_2()
                            .h(rems(1.375))
                            .items_center()
                            .cursor_pointer()
                            .whitespace_nowrap()
                            .when(this.reference_selected == ix, |d| d.bg(rgb(t.selection)))
                            .hover(|s| s.bg(rgb(t.tab_active)))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.reference_selected = ix;
                                this.open_file(&loc.path, Some((loc.range.start.line, loc.range.start.character)), cx);
                            }))
                            .child(entry.preview.clone())
                            .child(div().text_color(rgb(t.dim)).text_xs().child(format!("{rel}:{}", entry.location.range.start.line + 1)))
                    })
                    .collect()
            }),
        )
        .track_scroll(self.references_scroll.clone())
        .size_full()
        .into_any_element()
    }

    // ---- popups ----

    /// Window position of the bottom-left of `(line, col)` in the focused editor, if visible.
    fn caret_position(&self, line: usize, col: usize) -> Option<(Pixels, Pixels)> {
        let view = self.active_view()?;
        let doc = &self.docs[view.doc];
        let (bounds, offset) = {
            let state = view.scroll.0.borrow();
            (state.base_handle.bounds(), state.base_handle.offset())
        };
        if bounds.size.height <= px(0.0) {
            return None;
        }
        let text = doc.buffer.line(line);
        let cell = display_width(&text, col);
        let wrapped = view.wrap.as_ref().and_then(|w| w.row_of(line, cell).and_then(|r| w.row(r).map(|info| (r, info.start))));
        let (row, start_cell) = wrapped.unwrap_or_else(|| (view.folds.visible_lines(doc.buffer.len_lines()).and_then(|v| v.iter().position(|l| *l == line)).unwrap_or(line), 0));
        let x = bounds.left() + px_f(GUTTER_WIDTH + TEXT_PAD) + self.char_width * (cell - start_cell) as f32;
        let y = bounds.top() + offset.y + px_f(self.line_height()) * (row + 1) as f32;
        (y > bounds.top() - px(1.0) && y < bounds.bottom() + px_f(self.line_height())).then_some((x, y))
    }

    fn render_completion(&self) -> Option<Div> {
        let t = &self.theme;
        let state = self.completion.as_ref()?;
        let view = self.active_view()?;
        let doc = self.active_doc()?;
        let (line, col) = doc.buffer.offset_to_point(view.selections.primary().head);
        let (x, y) = self.caret_position(line, col)?;
        let rows = state.visible_window().map(|ix| {
            let item = &state.items[ix];
            div()
                .flex()
                .flex_row()
                .gap_3()
                .px_2()
                .h(rems(1.375))
                .items_center()
                .whitespace_nowrap()
                .bg(rgb(if ix == state.selected { t.selection } else { t.popup }))
                .child(item.label.clone())
                .children(item.detail.clone().map(|d| div().text_color(rgb(t.dim)).text_xs().child(d)))
        });
        Some(div().absolute().left(x).top(y).min_w(rems(17.5)).max_w(rems(35.0)).py_1().bg(rgb(t.popup)).border_1().border_color(rgb(t.border)).rounded_md().shadow_lg().occlude().children(rows))
    }

    fn render_hover(&self) -> Option<Div> {
        let t = &self.theme;
        let hover = self.hover.as_ref()?;
        let doc = self.active_doc()?;
        let (x, y) = self.caret_position(hover.line, hover.col)?;
        let mut sections: Vec<Div> = doc
            .diagnostics_on(hover.line)
            .map(|d| {
                let color = if d.severity == Severity::Error { t.error } else { t.warning };
                div().text_color(rgb(color)).child(d.message.clone())
            })
            .collect();
        if !hover.text.is_empty() {
            let text: String = hover.text.lines().take(14).collect::<Vec<_>>().join("\n");
            sections.push(div().child(text));
        }
        if sections.is_empty() {
            return None;
        }
        Some(div().absolute().left(x).top(y + px(4.0)).max_w(rems(35.0)).flex().flex_col().gap_2().p_2().bg(rgb(t.popup)).border_1().border_color(rgb(t.border)).rounded_md().shadow_lg().text_sm().occlude().children(sections))
    }

    fn render_signature(&self, help: &SignatureHelp) -> Option<Div> {
        let t = &self.theme;
        let view = self.active_view()?;
        let doc = self.active_doc()?;
        let (line, col) = doc.buffer.offset_to_point(view.selections.primary().head);
        let (x, y) = self.caret_position(line, col)?;
        // Sit above the caret line, like VS Code's parameter hints.
        let top = y - px_f(self.line_height() * 2.0 + 8.0);
        let label = help.label.clone();
        let highlights = help.active.map(|(s, e)| {
            let byte = |c: usize| label.char_indices().nth(c).map_or(label.len(), |(b, _)| b);
            vec![(byte(s)..byte(e), HighlightStyle { color: Some(rgb(t.info).into()), font_weight: Some(FontWeight::BOLD), ..Default::default() })]
        });
        let text = StyledText::new(label).with_highlights(highlights.unwrap_or_default());
        Some(div().absolute().left(x).top(top).max_w(rems(40.0)).px_2().py_1().bg(rgb(t.popup)).border_1().border_color(rgb(t.border)).rounded_md().shadow_lg().whitespace_nowrap().child(text))
    }

    fn render_overlay(&self, cx: &mut Context<Self>) -> Option<Div> {
        let t = &self.theme;
        let overlay = self.overlay.as_ref()?;
        let show_input = !matches!(overlay.kind, OverlayKind::ConfirmDelete(_) | OverlayKind::ConfirmDiscard { .. } | OverlayKind::ConfirmReplace { .. });
        let start = overlay.selected.saturating_sub(11);
        let end = (start + 12).min(overlay.items.len());
        let rows = (start..end).map(|ix| {
            let item = &overlay.items[ix];
            let selected = ix == overlay.selected;
            let label_style: Vec<(Range<usize>, HighlightStyle)> = item
                .positions
                .iter()
                .filter_map(|p| item.label.char_indices().nth(*p).map(|(b, c)| (b..b + c.len_utf8(), HighlightStyle { color: Some(rgb(t.info).into()), font_weight: Some(FontWeight::BOLD), ..Default::default() })))
                .collect();
            div()
                .id(("overlay-item", ix))
                .flex()
                .flex_row()
                .items_center()
                .justify_between()
                .gap_3()
                .px_2()
                .h(rems(1.5))
                .cursor_pointer()
                .whitespace_nowrap()
                .bg(rgb(if selected { t.selection } else { t.popup }))
                .on_mouse_down(MouseButton::Left, cx.listener(move |this, _: &MouseDownEvent, _, cx| {
                    cx.stop_propagation();
                    if let Some(o) = this.overlay.as_mut() {
                        o.selected = ix;
                    }
                    this.accept_overlay(cx);
                }))
                .child(StyledText::new(item.label.clone()).with_highlights(label_style))
                .child(div().text_color(rgb(t.dim)).text_xs().child(item.detail.clone()))
        });
        Some(
            div()
                .absolute()
                .top(rems(3.0))
                .left_0()
                .right_0()
                .flex()
                .justify_center()
                .child(
                    div()
                        .w(rems(40.0))
                        .flex()
                        .flex_col()
                        .gap_1()
                        .p_1()
                        .bg(rgb(t.popup))
                        .border_1()
                        .border_color(rgb(t.border))
                        .rounded_md()
                        .shadow_lg()
                        .occlude()
                        .on_mouse_down(MouseButton::Left, cx.listener(|_, _: &MouseDownEvent, _, cx| cx.stop_propagation()))
                        .when(show_input, |d| {
                            if overlay.kind == OverlayKind::AiKey || matches!(&overlay.kind, OverlayKind::GitCredential(p) if forge_core::askpass::is_secret(p)) {
                                // The key is never drawn: one dot per character.
                                let dots = LineInput::new(&"•".repeat(overlay.input.text().chars().count()));
                                d.child(self.render_input(&dots, &overlay.placeholder, true))
                            } else {
                                d.child(self.render_input(&overlay.input, &overlay.placeholder, true))
                            }
                        })
                        .children(overlay.note.clone().map(|n| div().px_2().py_1().text_xs().text_color(rgb(t.dim)).child(n)))
                        .children(rows),
                ),
        )
    }

    // ---- status bar ----

    /// One status bar item an extension contributed; it runs its command when clicked.
    fn render_ext_status(&self, id: &str, item: &crate::host_support::ExtStatus, cx: &mut Context<Self>) -> impl IntoElement {
        let command = item.command.clone();
        let mut element = div().id(gpui::SharedString::from(format!("ext-status-{id}"))).flex_none().whitespace_nowrap().text_color(rgb(self.theme.text)).child(item.text.clone());
        if let Some(command) = command {
            element = element.cursor_pointer().on_click(cx.listener(move |this, _, _, cx| this.run_ext_command(&command, cx)));
        }
        element
    }

    fn render_status(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = &self.theme;
        let doc = self.active_doc();
        let view = self.active_view();
        let (message, color) = doc
            .zip(view)
            .and_then(|(doc, view)| {
                let (line, _) = doc.buffer.offset_to_point(view.selections.primary().head);
                doc.diagnostics_on(line).min_by_key(|d| d.severity as u8).map(|d| {
                    (d.message.lines().next().unwrap_or_default().to_owned(), if d.severity == Severity::Error { t.error } else { t.warning })
                })
            })
            .unwrap_or_else(|| {
                // A result the user just caused is green or red for ten seconds, then fades back.
                let recent = self.status_at.elapsed().as_secs() < 10;
                let color = match (self.status_kind, recent) {
                    (crate::workspace::StatusKind::Success, true) => t.git_added,
                    (crate::workspace::StatusKind::Error, true) => t.error,
                    _ => t.dim,
                };
                (self.status.to_string(), color)
            });
        let result_icon = (self.status_at.elapsed().as_secs() < 10)
            .then(|| match self.status_kind {
                crate::workspace::StatusKind::Success => Some(crate::codicons::CHECK),
                crate::workspace::StatusKind::Error => Some(crate::codicons::ERROR),
                crate::workspace::StatusKind::Info => None,
            })
            .flatten();
        let (errors, warnings) = self.docs.iter().fold((0, 0), |(e, w), d| {
            (e + d.diagnostics.iter().filter(|x| x.severity == Severity::Error).count(), w + d.diagnostics.iter().filter(|x| x.severity == Severity::Warning).count())
        });
        let mut parts: Vec<String> = Vec::new();
        if let Some(p) = &self.lsp_progress {
            parts.push(p.clone());
        }
        if let (Some(doc), Some(view)) = (doc, view) {
            let (line, col) = doc.buffer.offset_to_point(view.selections.primary().head);
            let selected: usize = view.selections.as_slice().iter().map(|s| s.end() - s.start()).sum();
            let cursors = view.selections.as_slice().len();
            let mut pos = format!("Ln {}, Col {}", line + 1, col + 1);
            if selected > 0 {
                pos.push_str(&format!(" ({selected} selected)"));
            }
            if cursors > 1 {
                pos.push_str(&format!(" [{cursors} cursors]"));
            }
            parts.push(pos);
            parts.push(if self.settings.insert_spaces { format!("Spaces: {}", self.settings.tab_size) } else { "Tab Size: ".to_owned() + &self.settings.tab_size.to_string() });
            parts.push("UTF-8".to_owned());
            parts.push(if doc.buffer.eol() == "\r\n" { "CRLF" } else { "LF" }.to_owned());
            parts.push(doc.syntax.as_ref().map(|s| s.language().name().to_owned()).or_else(|| doc.ext_lang.as_ref().map(|l| l.name.clone())).unwrap_or_else(|| "Plain Text".to_owned()));
        }
        div()
            .flex()
            .flex_row()
            .flex_none()
            .justify_between()
            .items_center()
            .gap_4()
            .px_3()
            .h(rems(1.5))
            .text_xs()
            .bg(rgb(t.sidebar))
            .border_t_1()
            .border_color(rgb(t.border))
            .text_color(rgb(t.dim))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .gap_3()
                    .min_w_0()
                    .overflow_hidden()
                    .children(self.git.as_ref().map(|g| {
                        let dirty = if g.status.changes.is_empty() { "" } else { "*" };
                        let branch = g.status.branch.clone().unwrap_or_else(|| format!("({})", g.status.head));
                        let sync = if g.status.ahead + g.status.behind > 0 { format!(" ↓{} ↑{}", g.status.behind, g.status.ahead) } else { String::new() };
                        let busy = g
                            .job
                            .as_ref()
                            .map(|j| {
                                let ms = g.job_started.map_or(0, |at| at.elapsed().as_millis() as u64);
                                format!("  {}{}", j.trim_end_matches('…'), ".".repeat(((ms / 350) % 4) as usize))
                            })
                            .unwrap_or_default();
                        div()
                            .id("status-branch")
                            .flex_none()
                            .cursor_pointer()
                            .text_color(rgb(t.text))
                            .on_click(cx.listener(|this, _, _, cx| this.run_command(Command::GitCheckout, false, cx)))
                            .tooltip(self.tooltip("Checkout Branch...", Some(Command::GitCheckout)))
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap(rems(0.25))
                            .child(crate::icons::glyph(crate::codicons::GIT_BRANCH, 0.875, t.text))
                            .child(format!("{branch}{dirty}{sync}{busy}"))
                    }))
                    .children(self.git.as_ref().filter(|g| g.job.is_some()).map(|_| {
                        div()
                            .id("status-git-cancel")
                            .flex_none()
                            .cursor_pointer()
                            .px_1()
                            .rounded_sm()
                            .text_color(rgb(t.text))
                            .hover(|s| s.bg(rgb(t.tab_active)))
                            .tooltip(self.tooltip("Cancel the running git operation", Some(Command::GitCancel)))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.git_cancel();
                                cx.notify();
                            }))
                            .child("Cancel")
                    }))
                    .child(
                        div()
                            .id("status-problems")
                            .flex_none()
                            .cursor_pointer()
                            .text_color(rgb(if errors > 0 { t.error } else { t.dim }))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.panel_view = PanelView::Problems;
                                this.panel_visible = true;
                                cx.notify();
                            }))
                            .child(format!("✖ {errors}  ⚠ {warnings}")),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap(rems(0.25))
                            .min_w_0()
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_color(rgb(color))
                            .children(result_icon.map(|icon| crate::icons::glyph(icon, 0.875, color)))
                            .child(message),
                    )
                    .children(self.ext_status_items(1).into_iter().map(|(id, item)| self.render_ext_status(id, item, cx))),
            )
            .child(
                div()
                    .flex()
                    .flex_row()
                    .flex_none()
                    .gap_4()
                    .whitespace_nowrap()
                    .children(self.ext_status_items(2).into_iter().map(|(id, item)| self.render_ext_status(id, item, cx)))
                    .child(parts.join("    ")),
            )
    }
}

impl Render for Workspace {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.measure_font(cx);
        if std::mem::take(&mut self.product_dirty) {
            let loaded = crate::ext_support::resolve_product_icons(&self.extensions, &self.settings.product_icon_theme)
                .and_then(|theme| crate::product_icons::apply(theme.as_ref(), cx));
            if let Err(error) = loaded {
                self.set_status(format!("Product icon theme: {error}"));
            }
        }
        self.sync_terminal_size(window.viewport_size().width);
        let cols = self.scm_wrap_cols();
        self.scm_message.set_wrap(cols);
        // One rem is 16px at the default zoom. Zooming changes the font size, and every size written in
        // rems (buttons, icons, rows) follows it.
        window.set_rem_size(px(16.0 * self.settings.font_size / 14.0));
        let t = self.theme.clone();

        let mut centre = div().flex().flex_col().flex_1().min_w_0().h_full();
        let group_count = self.groups.len();
        if self.group_fractions.len() != group_count {
            self.group_fractions = equal_shares(group_count);
        }
        self.viewport = (f32::from(window.viewport_size().width), f32::from(window.viewport_size().height));
        let mut groups = div().flex().flex_row().flex_1().min_h_0();
        for g in 0..group_count {
            groups = groups.child(self.render_group(g, cx));
        }
        centre = centre.child(groups);
        if self.panel_visible {
            centre = centre.child(self.render_panel(cx));
        }

        let mut main = div().flex().flex_row().flex_1().min_h_0();
        if self.sidebar_visible {
            main = main.child(self.render_sidebar(cx));
        }
        main = main.child(centre);

        let signature = self.signature.clone();
        div()
            .id("workspace")
            .key_context("Workspace")
            .track_focus(&self.focus_handle)
            .on_key_down(cx.listener(|this, ev, _w, cx| this.on_key_down(ev, cx)))
            .on_mouse_down(MouseButton::Left, cx.listener(|this, ev: &MouseDownEvent, _w, cx| this.on_mouse_down(ev, cx)))
            .on_mouse_down(MouseButton::Right, cx.listener(|this, ev: &MouseDownEvent, _w, cx| this.on_right_mouse_down(ev, cx)))
            .on_mouse_move(cx.listener(|this, ev, _w, cx| this.on_mouse_move(ev, cx)))
            .on_mouse_up(MouseButton::Left, cx.listener(|this, ev, _w, cx| this.on_mouse_up(ev, cx)))
            .on_mouse_up_out(MouseButton::Left, cx.listener(|this, ev, _w, cx| this.on_mouse_up(ev, cx)))
            .relative()
            .size_full()
            .flex()
            .flex_col()
            .bg(rgb(t.bg))
            .text_color(rgb(t.text))
            .font_family(FONT)
            .text_size(px(self.settings.font_size))
            .child(main)
            .child(self.render_status(cx))
            .children(self.render_completion())
            .children(self.render_hover())
            .children(signature.as_ref().and_then(|s| self.render_signature(s)))
            .children(self.render_settings(cx))
            .children(self.render_conflict_page(cx))
            .children(self.render_debug_toolbar(f32::from(window.viewport_size().width), cx))
            .children(self.render_overlay(cx))
            .children(self.render_context_menu(window.viewport_size(), cx))
    }
}

/// Color of a unified-diff line and whether it gets a tinted background.
fn diff_line_style(t: &Theme, text: &str) -> Option<(u32, bool)> {
    const HEADERS: [&str; 10] = ["+++", "---", "diff ", "index ", "new file", "deleted file", "similarity", "rename ", "commit ", "Author:"];
    if HEADERS.iter().any(|h| text.starts_with(h)) || text.starts_with("Date:") {
        return Some((t.dim, false));
    }
    match text.chars().next()? {
        '+' => Some((t.git_added, true)),
        '-' => Some((t.git_deleted, true)),
        '@' => Some((t.info, false)),
        _ => None,
    }
}

/// The colored bar in the gutter beside a changed line.
fn gutter_marker(t: &Theme, kind: ChangeKind, line_h: Pixels) -> Div {
    match kind {
        ChangeKind::Added => div().absolute().left(px(1.0)).top_0().w(rems(0.1875)).h(line_h).bg(rgb(t.git_added)),
        ChangeKind::Modified => div().absolute().left(px(1.0)).top_0().w(rems(0.1875)).h(line_h).bg(rgb(t.git_modified)),
        ChangeKind::Removed => div().absolute().left(px(0.0)).bottom_0().w(rems(0.5625)).h(px(2.0)).bg(rgb(t.git_deleted)),
    }
}
