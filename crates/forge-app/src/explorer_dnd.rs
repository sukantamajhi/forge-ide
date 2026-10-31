//! Dragging files and folders in the Explorer: what is dragged and the chip drawn under the pointer.

use std::path::PathBuf;

use gpui::{div, prelude::*, rgb, Context, IntoElement, Render, Window};

/// A file or folder being dragged by its Explorer row.
#[derive(Clone, Debug)]
pub struct DraggedEntry {
    pub path: PathBuf,
}

/// An editor tab being dragged by its title: its group and its place in the group's strip.
#[derive(Clone, Debug)]
pub struct DraggedTab {
    pub group: usize,
    pub index: usize,
}

/// The label that follows the pointer while dragging.
pub struct DragChip {
    pub name: String,
    pub bg: u32,
    pub fg: u32,
    pub border: u32,
}

impl Render for DragChip {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().px_2().py_1().rounded_sm().bg(rgb(self.bg)).text_color(rgb(self.fg)).border_1().border_color(rgb(self.border)).opacity(0.92).child(self.name.clone())
    }
}
