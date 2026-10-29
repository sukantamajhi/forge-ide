//! The minimap: a narrow strip beside the editor that draws the whole file as small bars, marks
//! the part on screen, and scrolls the editor when clicked or dragged.

use gpui::{canvas, div, fill, prelude::*, px, rgb, rgba, AnyElement, Bounds, Context, MouseButton, MouseDownEvent, MouseMoveEvent, Pixels, Point, ScrollStrategy, Size};

use crate::workspace::Workspace;

pub(crate) const MINIMAP_WIDTH: f32 = 90.0;
/// Files with more lines than this are not drawn: the strip is rebuilt on every edit.
const MAX_LINES: usize = 100_000;
/// Width of one character in the minimap, and the most characters drawn per line.
const CHAR_PX: f32 = 1.2;
const MAX_CHARS: usize = 70;
/// Rows are never taller than this; a longer file is squeezed to fit.
const MAX_ROW_PX: f32 = 2.0;

/// Indent and length (in characters) of each line, for drawing its bar.
pub(crate) type LineStats = Vec<(u8, u8)>;

pub(crate) fn line_stats(lines: impl Iterator<Item = String>) -> LineStats {
    lines
        .map(|text| {
            let trimmed = text.trim_end();
            let indent: usize = trimmed.chars().take_while(|c| c.is_whitespace()).map(|c| if c == '\t' { 4 } else { 1 }).sum();
            let len = trimmed.chars().count().saturating_sub(trimmed.chars().take_while(|c| c.is_whitespace()).count());
            (indent.min(40) as u8, len.min(MAX_CHARS) as u8)
        })
        .collect()
}

/// Pixel height of one row and how many rows each drawn bar stands for, for `rows` rows in `height` px.
pub(crate) fn row_scale(rows: usize, height: f32) -> (f32, usize) {
    if rows == 0 || height <= 0.0 {
        return (MAX_ROW_PX, 1);
    }
    let per_row = MAX_ROW_PX.min(height / rows as f32);
    let step = if per_row >= 1.0 { 1 } else { (1.0 / per_row).ceil() as usize };
    (per_row, step)
}

impl Workspace {
    /// Number of displayed rows in the editor of group `g` (wrapped rows count separately).
    pub(crate) fn display_rows(&self, g: usize) -> usize {
        let Some(view) = self.groups.get(g).and_then(|gr| gr.views.get(gr.active)) else { return 0 };
        let total = self.docs[view.doc].buffer.len_lines();
        match &view.wrap {
            Some(map) => map.row_count(),
            None => view.folds.visible_lines(total).map_or(total, |v| v.len()),
        }
    }

    pub fn ensure_minimap_stats(&mut self, g: usize, vi: usize) {
        let view = &mut self.groups[g].views[vi];
        let doc = &self.docs[view.doc];
        let total = doc.buffer.len_lines();
        if !self.settings.minimap || total > MAX_LINES {
            view.minimap = None;
            return;
        }
        if view.minimap.as_ref().is_some_and(|(revision, _)| *revision == doc.revision) {
            return;
        }
        let stats = line_stats((0..total).map(|l| doc.buffer.line(l)));
        view.minimap = Some((doc.revision, stats));
    }

    /// Scroll group `g`'s editor so the row under window y-coordinate `y` is centred.
    pub fn minimap_jump(&mut self, g: usize, y: Pixels) {
        let rows = self.display_rows(g);
        let Some(view) = self.groups.get(g).and_then(|gr| gr.views.get(gr.active)) else { return };
        let (top, height) = {
            let state = view.scroll.0.borrow();
            (state.base_handle.bounds().top(), f32::from(state.base_handle.bounds().size.height))
        };
        let (per_row, _) = row_scale(rows, height);
        let row = (f32::from(y - top) / per_row).max(0.0) as usize;
        view.scroll.scroll_to_item_strict(row.min(rows.saturating_sub(1)), ScrollStrategy::Center);
    }

    pub fn render_minimap(&self, g: usize, rows: usize, cx: &mut Context<Self>) -> Option<AnyElement> {
        let view = self.groups.get(g).and_then(|gr| gr.views.get(gr.active))?;
        let (_, stats) = view.minimap.as_ref()?;
        let (height, offset_y, width) = {
            let state = view.scroll.0.borrow();
            let b = state.base_handle.bounds();
            (f32::from(b.size.height), f32::from(state.base_handle.offset().y), f32::from(b.size.width))
        };
        // Too narrow to be worth the room, or not laid out yet.
        if height <= 0.0 || width < 420.0 {
            return None;
        }
        let t = &self.theme;
        let (per_row, step) = row_scale(rows, height);
        let line_of = |row: usize| -> usize {
            match &view.wrap {
                Some(map) => map.row(row).map_or(0, |r| r.line),
                None => view.folds.visible_lines(stats.len()).and_then(|v| v.get(row).copied()).unwrap_or(row),
            }
        };
        let bar_h = (per_row * step as f32 * 0.72).max(1.0);
        let mut bars: Vec<(f32, f32, f32, f32)> = Vec::new();
        for row in (0..rows).step_by(step) {
            let Some(&(indent, len)) = stats.get(line_of(row)) else { continue };
            if len == 0 || bars.len() >= 4000 {
                continue;
            }
            bars.push((6.0 + f32::from(indent) * CHAR_PX, row as f32 * per_row, f32::from(len) * CHAR_PX, bar_h));
        }
        // The part of the file on screen, and the caret's row.
        let line_h = self.line_height();
        let first_visible = (-offset_y / line_h).max(0.0);
        let slider = (first_visible * per_row, ((height / line_h) * per_row).min(height));
        let doc = &self.docs[view.doc];
        let (caret_line, caret_col) = doc.buffer.offset_to_point(view.selections.primary().head);
        let caret_row = view
            .wrap
            .as_ref()
            .and_then(|w| w.row_of(caret_line, forge_text::columns::display_width(&doc.buffer.line(caret_line), caret_col)))
            .or_else(|| view.folds.visible_lines(stats.len()).and_then(|v| v.iter().position(|l| *l == caret_line)))
            .unwrap_or(caret_line);
        let caret_y = caret_row as f32 * per_row;

        let (bar_color, slider_color, caret_color) = ((t.dim << 8) | 0x88, (t.text << 8) | 0x22, (t.caret << 8) | 0xcc);
        Some(
            div()
                .id(("minimap", g))
                .relative()
                .w(px(MINIMAP_WIDTH))
                .h_full()
                .flex_none()
                .bg(rgb(t.bg))
                .border_l_1()
                .border_color(rgb(t.border))
                .cursor_pointer()
                .on_mouse_down(MouseButton::Left, cx.listener(move |this, ev: &MouseDownEvent, _, cx| {
                    cx.stop_propagation();
                    this.minimap_dragging = true;
                    this.minimap_jump(g, ev.position.y);
                    cx.notify();
                }))
                .on_mouse_move(cx.listener(move |this, ev: &MouseMoveEvent, _, cx| {
                    if !this.minimap_dragging {
                        return;
                    }
                    if ev.pressed_button != Some(MouseButton::Left) {
                        this.minimap_dragging = false;
                        return;
                    }
                    this.minimap_jump(g, ev.position.y);
                    cx.notify();
                }))
                .child(
                    canvas(
                        |_, _, _| (),
                        move |bounds: Bounds<Pixels>, _, window, _| {
                            let at = |x: f32, y: f32, w: f32, h: f32| Bounds { origin: bounds.origin + Point { x: px(x), y: px(y) }, size: Size { width: px(w), height: px(h) } };
                            for (x, y, w, h) in &bars {
                                window.paint_quad(fill(at(*x, *y, *w, *h), rgba(bar_color)));
                            }
                            window.paint_quad(fill(at(0.0, slider.0, MINIMAP_WIDTH, slider.1.max(6.0)), rgba(slider_color)));
                            window.paint_quad(fill(at(0.0, caret_y, MINIMAP_WIDTH, 1.5), rgba(caret_color)));
                        },
                    )
                    .size_full(),
                )
                .into_any_element(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stats_record_indent_and_trimmed_length() {
        let stats = line_stats(["fn main() {", "    let x = 1;  ", "", "\tx", &"y".repeat(500)].into_iter().map(str::to_owned));
        assert_eq!(stats[0], (0, 11));
        assert_eq!(stats[1], (4, 10));
        assert_eq!(stats[2], (0, 0));
        assert_eq!(stats[3], (4, 1), "a tab counts as four columns");
        assert_eq!(stats[4], (0, MAX_CHARS as u8), "very long lines are capped");
    }

    #[test]
    fn short_files_use_full_rows_and_long_ones_are_squeezed_and_sampled() {
        assert_eq!(row_scale(100, 800.0), (2.0, 1));
        let (per_row, step) = row_scale(4000, 800.0);
        assert!((per_row - 0.2).abs() < 1e-6 && step == 5, "{per_row} {step}");
        assert_eq!(row_scale(0, 800.0), (2.0, 1));
    }
}
