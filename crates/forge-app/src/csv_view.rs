//! The full-page preview of a CSV or TSV file: a table with a header row, row numbers and aligned columns.

use std::rc::Rc;

use forge_core::csv::{detect_delimiter, is_numeric, parse, Table};
use gpui::{div, prelude::*, px, rgb, rgba, uniform_list, AnyElement, Context, FontWeight};

use crate::workspace::Workspace;

const ROW_NUMBER_WIDTH: f32 = 56.0;
const CELL_PADDING: f32 = 20.0;

/// A parsed table with the pixel width of each column.
pub(crate) struct CsvCache {
    pub doc: usize,
    pub revision: u64,
    pub table: Rc<Table>,
    pub widths: Rc<Vec<f32>>,
    pub total_width: f32,
}

impl Workspace {
    /// Whether the active document in group `g` is a CSV or TSV file.
    pub(crate) fn is_csv(&self, g: usize) -> bool {
        let Some(view) = self.groups.get(g).and_then(|gr| gr.views.get(gr.active)) else { return false };
        self.docs[view.doc].path.extension().is_some_and(|e| matches!(e.to_string_lossy().to_ascii_lowercase().as_str(), "csv" | "tsv"))
    }

    /// Parse the active CSV again if it changed.
    pub fn ensure_csv(&mut self, g: usize) {
        let Some(view) = self.groups.get(g).and_then(|gr| gr.views.get(gr.active)) else { return };
        let (doc_ix, revision) = (view.doc, self.docs[view.doc].revision);
        if self.csv_cache.as_ref().is_some_and(|c| c.doc == doc_ix && c.revision == revision) {
            return;
        }
        let doc = &self.docs[doc_ix];
        let tsv = doc.path.extension().is_some_and(|e| e.eq_ignore_ascii_case("tsv"));
        let text = doc.buffer.text();
        let table = parse(&text, detect_delimiter(&text, tsv));
        let char_w = f32::from(self.char_width);
        let widths: Vec<f32> = table.column_widths(4, 40).into_iter().map(|w| w as f32 * char_w + CELL_PADDING).collect();
        let total_width = ROW_NUMBER_WIDTH + widths.iter().sum::<f32>();
        self.csv_cache = Some(CsvCache { doc: doc_ix, revision, table: Rc::new(table), widths: Rc::new(widths), total_width });
    }

    /// Sideways scrolling of the table: a horizontal wheel or swipe, or Shift with the wheel.
    fn csv_hscroll(&mut self, ev: &gpui::ScrollWheelEvent, cx: &mut Context<Self>) {
        let line_h = px(self.line_height());
        let delta = ev.delta.pixel_delta(line_h);
        let mut dx = f32::from(delta.x);
        let handle = self.csv_scroll.0.borrow().base_handle.clone();
        if ev.modifiers.shift && dx == 0.0 {
            dx = f32::from(delta.y);
            let mut offset = handle.offset();
            offset.y -= delta.y;
            handle.set_offset(offset);
        }
        let Some(cache) = &self.csv_cache else { return };
        if dx == 0.0 {
            return;
        }
        let viewport = f32::from(self.csv_scroll.0.borrow().base_handle.bounds().size.width);
        let max = (cache.total_width - viewport).max(0.0);
        let after = (self.csv_scroll_x - dx).clamp(0.0, max);
        if after != self.csv_scroll_x {
            self.csv_scroll_x = after;
            cx.notify();
        }
    }

    /// The table as the whole page of group `g`.
    pub fn render_csv_full(&self, g: usize, cx: &mut Context<Self>) -> Option<AnyElement> {
        let cache = self.csv_cache.as_ref()?;
        let t = self.theme.clone();
        let table = cache.table.clone();
        let widths = cache.widths.clone();
        let total = cache.total_width;
        let line_h = self.line_height() + 6.0;
        let scroll_x = self.csv_scroll_x.min((total - 200.0).max(0.0));
        let name = self.active_doc().map(|d| d.name()).unwrap_or_default();
        let delimiter = match table.delimiter {
            '\t' => "tab".to_owned(),
            ',' => "comma".to_owned(),
            ';' => "semicolon".to_owned(),
            '|' => "pipe".to_owned(),
            other => other.to_string(),
        };
        let data_rows = table.rows.len().saturating_sub(1);
        let summary = format!(
            "{} data row{}, {} column{}, {}-separated{}",
            data_rows,
            if data_rows == 1 { "" } else { "s" },
            table.columns,
            if table.columns == 1 { "" } else { "s" },
            delimiter,
            if table.truncated { format!(" (only the first {} rows are shown)", forge_core::csv::MAX_ROWS) } else { String::new() }
        );
        let toolbar = div()
            .flex_none()
            .h(gpui::rems(2.0))
            .px_3()
            .flex()
            .flex_row()
            .items_center()
            .gap_3()
            .bg(rgb(t.sidebar))
            .border_b_1()
            .border_color(rgb(t.border))
            .child(div().font_weight(FontWeight::MEDIUM).child(name))
            .child(div().text_xs().text_color(rgb(t.dim)).child(summary));
        if table.rows.is_empty() {
            return Some(div().size_full().flex().flex_col().bg(rgb(t.bg)).child(toolbar).child(div().p_4().text_color(rgb(t.dim)).child("This file is empty.")).into_any_element());
        }
        let cell = move |text: &str, width: f32, right: bool, bold: bool, color: u32| {
            div()
                .flex_none()
                .w(px(width))
                .h_full()
                .px(px(CELL_PADDING / 2.0))
                .flex()
                .items_center()
                .when(right, |d| d.justify_end())
                .overflow_hidden()
                .whitespace_nowrap()
                .text_color(rgb(color))
                .when(bold, |d| d.font_weight(FontWeight::BOLD))
                .child(text.lines().next().unwrap_or("").to_owned())
        };
        let header = {
            let mut row = div().relative().left(px(-scroll_x)).flex_none().w(px(total)).h(px(line_h)).flex().flex_row().items_center().bg(rgb(t.sidebar)).border_b_1().border_color(rgb(t.border));
            row = row.child(div().flex_none().w(px(ROW_NUMBER_WIDTH)).pr_2().flex().justify_end().text_xs().text_color(rgb(t.dim)).child("#"));
            for (c, text) in table.rows[0].iter().enumerate() {
                row = row.child(cell(text, widths[c], false, true, t.text));
            }
            row
        };
        let body = {
            let table = table.clone();
            let widths = widths.clone();
            let theme = t.clone();
            crate::scroll::restrict_to_axis(uniform_list(
                ("csv-rows", g),
                data_rows,
                cx.processor(move |_, range: std::ops::Range<usize>, _window, _cx| {
                    range
                        .map(|i| {
                            let cells = &table.rows[i + 1];
                            let mut row = div()
                                .relative()
                                .left(px(-scroll_x))
                                .flex_none()
                                .w(px(total))
                                .h(px(line_h))
                                .flex()
                                .flex_row()
                                .items_center()
                                .when(i % 2 == 1, |d| d.bg(rgba((theme.sidebar << 8) | 0x99)))
                                .border_b_1()
                                .border_color(rgba((theme.border << 8) | 0x55));
                            row = row.child(div().flex_none().w(px(ROW_NUMBER_WIDTH)).pr_2().flex().justify_end().text_xs().text_color(rgb(theme.dim)).child((i + 1).to_string()));
                            for (c, text) in cells.iter().enumerate() {
                                row = row.child(cell(text, widths[c], is_numeric(text), false, theme.text));
                            }
                            // The list places its items itself, so the sideways shift lives on the row inside a clipping item.
                            div().w_full().h(px(line_h)).overflow_hidden().child(row)
                        })
                        .collect()
                }),
            )
            .track_scroll(self.csv_scroll.clone()))
            .size_full()
        };
        Some(
            div()
                .size_full()
                .flex()
                .flex_col()
                .bg(rgb(t.bg))
                .child(toolbar)
                .child(
                    div()
                        .flex_1()
                        .min_h_0()
                        .flex()
                        .flex_col()
                        .overflow_hidden()
                        .on_scroll_wheel(cx.listener(|this, ev: &gpui::ScrollWheelEvent, _, cx| this.csv_hscroll(ev, cx)))
                        .child(div().flex_none().w_full().overflow_hidden().child(header))
                        .child(div().flex_1().min_h_0().overflow_hidden().child(body)),
                )
                .into_any_element(),
        )
    }
}
