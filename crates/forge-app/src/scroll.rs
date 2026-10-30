//! Shared scroll containers. GPUI sizes a scroll area's range from the bounds of its *direct* children
//! only, so the container has to let those children be as tall as their content. Build scrollable
//! pages with these instead of repeating `overflow_y_scroll()` by hand.

use gpui::{div, prelude::*, px, rems, rgb, Div, ElementId, Stateful};

/// Make a scrolling element move only along the axis the wheel turns. Without this a list that scrolls vertically also
/// treats a sideways swipe as a vertical one and drifts down the page while the text moves sideways.
pub fn restrict_to_axis<T: gpui::Styled>(mut element: T) -> T {
    element.style().restrict_scroll_to_axis = Some(true);
    element
}

/// A vertically scrolling page holding one centred content column. The caller gives it its size. The
/// column keeps its natural height, so the page scrolls when the column is taller than the page.
pub fn scroll_page(id: impl Into<ElementId>) -> Stateful<Div> {
    div()
        .id(id)
        .overflow_y_scroll()
        .flex()
        .justify_center()
        // A flex row stretches its child to the page's height by default. Then the content never
        // measures taller than the page and there is nothing to scroll.
        .items_start()
}

/// A scrolling page drawn over the editor area (Settings): it covers everything right of the sidebar
/// down to the status bar and swallows mouse input.
pub fn overlay_page(id: impl Into<ElementId>, left: f32, bg: u32, text: u32) -> Stateful<Div> {
    scroll_page(id).absolute().top(px(crate::menu_bar::MENU_BAR_HEIGHT)).left(px(left)).right_0().bottom(rems(1.5)).bg(rgb(bg)).text_color(rgb(text)).occlude()
}
