//! The mouse in the terminal panel: selecting text (drag, double-click a word, triple-click a line), copying it,
//! and clicking links (web addresses, and file paths with an optional `:line:col`) in the output.

use std::path::PathBuf;

use forge_core::terminal::{find_links, word_span, Link, LinkKind};
use gpui::{ClipboardItem, Context, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, Pixels, Point};

use crate::workspace::{Focus, PanelView, Workspace};

/// Space between the terminal view's edge and its first cell (`px_2`, `py_1`).
const PAD_X: f32 = 8.0;
const PAD_Y: f32 = 4.0;

/// A selection in screen cells `(row, col)` of one terminal.
#[derive(Clone, Debug)]
pub struct TermSel {
    pub terminal: usize,
    pub anchor: (u16, u16),
    pub head: (u16, u16),
    pub dragging: bool,
    /// The pointer left the cell it went down on: a drag, not a click.
    pub moved: bool,
    /// Clicks in a row: 1 characters, 2 words, 3 lines.
    pub mode: usize,
}

/// A link the pointer is over, with the file it resolved to when it is a path.
#[derive(Clone, Debug)]
pub struct TermLink {
    pub terminal: usize,
    pub row: u16,
    pub link: Link,
    pub path: Option<PathBuf>,
}

impl Workspace {
    fn terminal_showing(&self) -> bool {
        self.panel_visible && self.panel_view == PanelView::Terminal && self.active_terminal < self.terminals.len()
    }

    /// The `(row, col)` cell under a window position. With `clamp` a position outside the view lands on the
    /// nearest cell (while dragging); without it, outside is `None`.
    pub fn term_cell(&self, pos: Point<Pixels>, clamp: bool) -> Option<(u16, u16)> {
        if !self.terminal_showing() {
            return None;
        }
        let bounds = self.term_bounds.get();
        if !clamp && !bounds.contains(&pos) {
            return None;
        }
        let (rows, cols) = self.terminals[self.active_terminal].size();
        let col = (f32::from(pos.x - bounds.left()) - PAD_X) / f32::from(self.char_width).max(1.0);
        let row = (f32::from(pos.y - bounds.top()) - PAD_Y) / self.line_height();
        Some((row.floor().clamp(0.0, rows.saturating_sub(1) as f32) as u16, col.floor().clamp(0.0, cols.saturating_sub(1) as f32) as u16))
    }

    /// The selected cells in reading order, or `None` for no selection (a bare click selects nothing).
    pub fn term_selection_range(&self) -> Option<((u16, u16), (u16, u16))> {
        let sel = self.term_sel.as_ref().filter(|s| s.terminal == self.active_terminal)?;
        if !sel.moved && sel.mode == 1 {
            return None;
        }
        Some(if sel.anchor <= sel.head { (sel.anchor, sel.head) } else { (sel.head, sel.anchor) })
    }

    /// The link at a cell, if there is one. A path counts only when the file exists.
    fn term_link_at(&self, (row, col): (u16, u16)) -> Option<TermLink> {
        let terminal = self.terminals.get(self.active_terminal)?;
        let cells = terminal.row_chars(row);
        let link = find_links(&cells).into_iter().find(|l| (l.start..l.end).contains(&(col as usize)))?;
        let path = match link.kind {
            LinkKind::Url => None,
            LinkKind::File { .. } => Some(self.resolve_terminal_path(&link.text)?),
        };
        Some(TermLink { terminal: self.active_terminal, row, link, path })
    }

    fn resolve_terminal_path(&self, text: &str) -> Option<PathBuf> {
        let given = PathBuf::from(text);
        let full = if given.is_absolute() { given } else { self.root().join(text.trim_start_matches("./").trim_start_matches(".\\")) };
        full.is_file().then_some(full)
    }

    pub fn terminal_mouse_down(&mut self, ev: &MouseDownEvent, cx: &mut Context<Self>) {
        self.focus = Focus::Terminal;
        if ev.button == MouseButton::Right {
            if self.copy_terminal_selection(cx) {
                self.term_sel = None;
            } else if let Some(text) = cx.read_from_clipboard().and_then(|i| i.text()) {
                if let Some(terminal) = self.terminals.get_mut(self.active_terminal) {
                    terminal.scroll_to_bottom();
                    terminal.send(text.as_bytes());
                }
            }
            return;
        }
        let Some(cell) = self.term_cell(ev.position, false) else { return };
        let Some(terminal) = self.terminals.get(self.active_terminal) else { return };
        let cells = terminal.row_chars(cell.0);
        let (anchor, head, mode) = match ev.click_count {
            2 => match word_span(&cells, cell.1 as usize) {
                Some((s, e)) => ((cell.0, s as u16), (cell.0, e as u16), 2),
                None => (cell, cell, 1),
            },
            n if n >= 3 => {
                let last = cells.iter().rposition(|c| !c.is_whitespace()).unwrap_or(0);
                ((cell.0, 0), (cell.0, last as u16), 3)
            }
            _ => (cell, cell, 1),
        };
        self.term_sel = Some(TermSel { terminal: self.active_terminal, anchor, head, dragging: true, moved: mode != 1, mode });
    }

    /// Pointer movement anywhere in the window: extends a drag, or shows what link is under the pointer.
    /// True when something on screen changed.
    pub fn terminal_mouse_move(&mut self, ev: &MouseMoveEvent) -> bool {
        if let Some(sel) = self.term_sel.as_ref().filter(|s| s.dragging) {
            if ev.pressed_button != Some(MouseButton::Left) {
                if let Some(sel) = self.term_sel.as_mut() {
                    sel.dragging = false;
                }
                return false;
            }
            let Some(cell) = self.term_cell(ev.position, true) else { return false };
            if cell == sel.head {
                return false;
            }
            if let Some(sel) = self.term_sel.as_mut() {
                sel.head = cell;
                sel.moved = true;
            }
            return true;
        }
        let link = self.term_cell(ev.position, false).and_then(|cell| self.term_link_at(cell));
        let same = match (&self.term_link, &link) {
            (None, None) => true,
            (Some(a), Some(b)) => a.terminal == b.terminal && a.row == b.row && a.link == b.link,
            _ => false,
        };
        if !same {
            self.term_link = link;
        }
        !same
    }

    /// Button released: ends a drag, and a click that did not drag opens the link under it.
    pub fn terminal_mouse_up(&mut self, ev: &MouseUpEvent, cx: &mut Context<Self>) {
        let Some(sel) = self.term_sel.as_mut().filter(|s| s.dragging) else { return };
        sel.dragging = false;
        if sel.moved {
            return;
        }
        self.term_sel = None;
        if let Some(cell) = self.term_cell(ev.position, false) {
            if let Some(hit) = self.term_link_at(cell) {
                self.open_terminal_link(&hit, cx);
            }
        }
        cx.notify();
    }

    fn open_terminal_link(&mut self, hit: &TermLink, cx: &mut Context<Self>) {
        match (&hit.link.kind, &hit.path) {
            (LinkKind::Url, _) => cx.open_url(&hit.link.text),
            (LinkKind::File { line, col }, Some(path)) => {
                let at = line.map(|l| (l.saturating_sub(1), col.map_or(0, |c| c.saturating_sub(1))));
                self.open_file(path, at, cx);
                self.focus = Focus::Editor;
            }
            _ => {}
        }
    }

    /// Put the selected text on the clipboard. False when nothing is selected.
    pub fn copy_terminal_selection(&mut self, cx: &mut Context<Self>) -> bool {
        let Some((from, to)) = self.term_selection_range() else { return false };
        let Some(terminal) = self.terminals.get(self.active_terminal) else { return false };
        let text = terminal.text_between(from, to);
        if text.is_empty() {
            return false;
        }
        cx.write_to_clipboard(ClipboardItem::new_string(text));
        true
    }
}
