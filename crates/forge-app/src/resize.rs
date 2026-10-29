//! Resizing the panels by dragging the dividers between them: the sidebar, the bottom panel, the
//! Markdown preview, split editor groups and the Debug panel's columns.

use gpui::{div, prelude::*, px, rgba, Context, MouseButton, MouseDownEvent, Pixels, Point, Stateful};

use crate::workspace::Workspace;

/// A divider that can be dragged. `Groups(i)` is the one between editor group `i` and `i + 1`;
/// `Debug(i)` the one between column `i` and `i + 1` of the Debug panel.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Resize {
    Sidebar,
    Panel,
    Preview,
    Groups(usize),
    Debug(usize),
}

/// Which side of its container a handle sits on.
#[derive(Clone, Copy)]
pub(crate) enum Edge {
    Left,
    Right,
    Top,
}

/// A drag in progress.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Drag {
    pub which: Resize,
    /// Where the pointer was when the drag began (x, or y for the bottom panel).
    pub start: f32,
    /// The value being changed, as it was then.
    pub value: f32,
    /// For a pair of neighbours sharing a width, the neighbour's value.
    pub other: f32,
}

const MIN_SIDEBAR: f32 = 140.0;
const MAX_SIDEBAR: f32 = 900.0;
const MIN_PANEL: f32 = 80.0;
const MIN_PREVIEW: f32 = 160.0;
/// The least share of the width a split group or a debug column may take.
const MIN_SHARE: f32 = 0.12;

/// `value + delta`, kept so that it and its neighbour both stay above `MIN_SHARE` of their total.
fn shares(value: f32, other: f32, delta: f32) -> (f32, f32) {
    let total = value + other;
    let first = (value + delta).clamp(MIN_SHARE.min(total / 2.0), (total - MIN_SHARE).max(total / 2.0));
    (first, total - first)
}

/// Equal shares for `count` things.
pub(crate) fn equal_shares(count: usize) -> Vec<f32> {
    vec![1.0 / count.max(1) as f32; count]
}

impl Workspace {
    /// The width the sidebar is drawn at: its chosen width, scaled with the zoom.
    pub fn sidebar_width(&self) -> f32 {
        self.sidebar_base * (self.settings.font_size / 14.0).clamp(0.7, 3.0)
    }

    /// Remember the panel sizes for the next session.
    pub(crate) fn save_ui_state(&mut self) {
        let state = forge_core::ui_state::UiState { sidebar: self.sidebar_base, panel_height: self.panel_height, preview_width: self.preview_px, debug_columns: self.debug_cols };
        if let Err(error) = state.save() {
            self.set_status(error);
        }
    }

    /// A drag ended: keep what it produced.
    pub(crate) fn end_resize(&mut self) {
        if self.resize.take().is_some() {
            self.save_ui_state();
        }
    }

    /// The width available to editors, the width the shares of groups and columns are taken of.
    fn area_width(&self) -> f32 {
        let sidebar = if self.sidebar_visible { self.sidebar_width() } else { 0.0 };
        (self.viewport.0 - sidebar).max(200.0)
    }

    /// A thin strip on the edge of a panel that starts a resize when pressed.
    pub(crate) fn resize_handle(&self, id: (&'static str, usize), which: Resize, edge: Edge, cx: &mut Context<Self>) -> Stateful<gpui::Div> {
        let t = &self.theme;
        let active = self.resize.is_some_and(|d| d.which == which);
        let hint = rgba((t.info << 8) | 0x77);
        let handle = div().id(id).absolute();
        let handle = match edge {
            Edge::Left => handle.top_0().bottom_0().left_0().w(px(5.0)).cursor_col_resize(),
            Edge::Right => handle.top_0().bottom_0().right_0().w(px(5.0)).cursor_col_resize(),
            Edge::Top => handle.left_0().right_0().top_0().h(px(5.0)).cursor_row_resize(),
        };
        handle
            .when(active, |d| d.bg(hint))
            .hover(|s| s.bg(hint))
            .on_mouse_down(MouseButton::Left, cx.listener(move |this, ev: &MouseDownEvent, _, cx| {
                cx.stop_propagation();
                this.begin_resize(which, ev.position);
                cx.notify();
            }))
    }

    fn begin_resize(&mut self, which: Resize, at: Point<Pixels>) {
        let (x, y) = (f32::from(at.x), f32::from(at.y));
        let drag = match which {
            Resize::Sidebar => Drag { which, start: x, value: self.sidebar_width(), other: 0.0 },
            Resize::Panel => Drag { which, start: y, value: self.panel_height, other: 0.0 },
            Resize::Preview => Drag { which, start: x, value: self.preview_px.unwrap_or(self.area_width() / 2.0), other: 0.0 },
            Resize::Groups(i) => Drag { which, start: x, value: self.group_fractions.get(i).copied().unwrap_or(0.5), other: self.group_fractions.get(i + 1).copied().unwrap_or(0.5) },
            Resize::Debug(i) => Drag { which, start: x, value: self.debug_cols[i.min(2)], other: self.debug_cols[(i + 1).min(2)] },
        };
        self.resize = Some(drag);
    }

    /// Follow the pointer during a drag.
    pub fn apply_resize(&mut self, to: Point<Pixels>) {
        let Some(drag) = self.resize else { return };
        let (x, y) = (f32::from(to.x), f32::from(to.y));
        match drag.which {
            Resize::Sidebar => {
                let zoom = (self.settings.font_size / 14.0).clamp(0.7, 3.0);
                let max = (self.viewport.0 - 200.0).clamp(MIN_SIDEBAR, MAX_SIDEBAR);
                let width = (drag.value + (x - drag.start)).clamp(MIN_SIDEBAR, max);
                self.sidebar_base = width / zoom;
            }
            Resize::Panel => {
                let max = (self.viewport.1 - 160.0).max(MIN_PANEL);
                self.panel_height = (drag.value - (y - drag.start)).clamp(MIN_PANEL, max);
            }
            Resize::Preview => {
                let max = (self.area_width() - MIN_PREVIEW).max(MIN_PREVIEW);
                self.preview_px = Some((drag.value - (x - drag.start)).clamp(MIN_PREVIEW, max));
            }
            Resize::Groups(i) => {
                let (first, second) = shares(drag.value, drag.other, (x - drag.start) / self.area_width());
                if let Some(slot) = self.group_fractions.get_mut(i) {
                    *slot = first;
                }
                if let Some(slot) = self.group_fractions.get_mut(i + 1) {
                    *slot = second;
                }
            }
            Resize::Debug(i) => {
                let (first, second) = shares(drag.value, drag.other, (x - drag.start) / self.area_width());
                self.debug_cols[i.min(2)] = first;
                self.debug_cols[(i + 1).min(2)] = second;
            }
        }
    }

    /// Replay hook (`FORGE_REPLAY="resize:sidebar:120"`): run a drag of `delta` pixels through the
    /// same code the mouse uses. `which` is sidebar, panel, preview, group0 or debug0.
    pub(crate) fn replay_resize(&mut self, token: &str) {
        let mut parts = token.split(':').skip(1);
        let (Some(name), Some(delta)) = (parts.next(), parts.next().and_then(|d| d.parse::<f32>().ok())) else { return };
        let which = match name {
            "sidebar" => Resize::Sidebar,
            "panel" => Resize::Panel,
            "preview" => Resize::Preview,
            "group0" => Resize::Groups(0),
            "debug0" => Resize::Debug(0),
            "debug1" => Resize::Debug(1),
            _ => return,
        };
        let from = gpui::point(px(1000.0), px(1000.0));
        self.begin_resize(which, from);
        let to = if which == Resize::Panel { gpui::point(px(1000.0), px(1000.0 + delta)) } else { gpui::point(px(1000.0 + delta), px(1000.0)) };
        self.apply_resize(to);
        self.end_resize();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dragging_a_divider_moves_width_between_the_neighbours_and_keeps_the_total() {
        let (a, b) = shares(0.5, 0.5, 0.2);
        assert!((a - 0.7).abs() < 1e-6 && (b - 0.3).abs() < 1e-6);
        let (a, b) = shares(0.3, 0.2, -0.1);
        assert!((a + b - 0.5).abs() < 1e-6);
    }

    #[test]
    fn neither_neighbour_can_be_squeezed_out() {
        let (a, b) = shares(0.5, 0.5, 5.0);
        assert!((b - MIN_SHARE).abs() < 1e-6 && (a - (1.0 - MIN_SHARE)).abs() < 1e-6);
        let (a, b) = shares(0.5, 0.5, -5.0);
        assert!((a - MIN_SHARE).abs() < 1e-6 && (b - (1.0 - MIN_SHARE)).abs() < 1e-6);
    }

    #[test]
    fn equal_shares_add_up_to_one() {
        assert_eq!(equal_shares(0), Vec::<f32>::new());
        assert!((equal_shares(3).iter().sum::<f32>() - 1.0).abs() < 1e-6);
    }
}
