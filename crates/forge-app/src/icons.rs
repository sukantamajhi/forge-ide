//! Small icons drawn from shapes, so they look like icons and scale with the zoom. Each fits a
//! 1rem square and takes the colour it should be drawn in.

use gpui::{div, prelude::*, px, rems, rgb, Div};

/// Two panes side by side: "split the editor".
pub fn split(color: u32) -> Div {
    div()
        .relative()
        .w(rems(1.0))
        .h(rems(0.8125))
        .rounded_sm()
        .border_1()
        .border_color(rgb(color))
        .child(div().absolute().top_0().bottom_0().left(rems(0.4375)).w(px(1.5)).bg(rgb(color)))
}

/// A page beside a magnifier: "open the preview to the side".
pub fn preview(color: u32) -> Div {
    div()
        .relative()
        .w(rems(1.0))
        .h(rems(0.8125))
        .rounded_sm()
        .border_1()
        .border_color(rgb(color))
        .child(div().absolute().top_0().bottom_0().left(rems(0.3125)).w(px(1.5)).bg(rgb(color)))
        .child(div().absolute().top(rems(0.1875)).left(rems(0.5625)).w(rems(0.25)).h(rems(0.25)).rounded_full().border_1().border_color(rgb(color)))
}

/// Three dots: "more actions".
pub fn more(color: u32) -> Div {
    let dot = || div().w(px(3.0)).h(px(3.0)).rounded_full().bg(rgb(color));
    div().flex().flex_row().items_center().gap(px(2.5)).child(dot()).child(dot()).child(dot())
}
