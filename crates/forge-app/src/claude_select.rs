//! Selecting text in the Claude chat with the mouse. Every piece of text the chat draws (an answer's paragraphs and code
//! lines, what the user asked, tool output) is registered with its layout as it is drawn; a mouse press, drag or double
//! click is mapped to a place in one of them, the selected part is painted, and Ctrl+C copies it.

use std::cell::RefCell;
use std::ops::Range;

use gpui::{div, prelude::*, rgb, AnyElement, FontWeight, HighlightStyle, MouseDownEvent, MouseMoveEvent, Pixels, Point, StyledText, TextLayout};

use crate::workspace::{Focus, Workspace};

/// One drawn piece of text and where it was laid out.
pub(crate) struct Registered {
    pub layout: TextLayout,
    pub text: String,
}

/// A place in the chat: which registered text (in the order they are drawn), and a byte offset in it.
pub(crate) type Pos = (usize, usize);
/// The anchor where the selection started and the head where it is now.
pub(crate) type Sel = (Pos, Pos);

/// What drawing a piece of text needs to take part in selection.
pub(crate) struct Selectable<'a> {
    pub texts: &'a RefCell<Vec<Registered>>,
    pub sel: Option<Sel>,
    pub color: u32,
}

impl Selectable<'_> {
    /// Register a drawn text: its highlight runs get the selected part painted, and it takes the next place in the order.
    pub(crate) fn register(&self, text: &str, runs: Vec<(Range<usize>, HighlightStyle)>) -> StyledText {
        let index = self.texts.borrow().len();
        let runs = match selected_range(self.sel, index, text.len()) {
            Some(range) => {
                let color = self.color;
                paint_background(runs, range, HighlightStyle::default(), move |s| s.background_color = Some(rgb(color).into()))
            }
            None => runs,
        };
        let styled = StyledText::new(text.to_owned()).with_highlights(runs);
        self.texts.borrow_mut().push(Registered { layout: styled.layout().clone(), text: text.to_owned() });
        styled
    }
}

/// How far to scroll (positive is down) for a pointer at `y` when the chat spans `top..bottom`: nothing inside it, faster the
/// further outside.
pub(crate) fn autoscroll_step(y: f32, top: f32, bottom: f32) -> f32 {
    if y < top {
        -((top - y) * 0.5).clamp(4.0, 40.0)
    } else if y > bottom {
        ((y - bottom) * 0.5).clamp(4.0, 40.0)
    } else {
        0.0
    }
}

fn ordered(sel: Sel) -> (Pos, Pos) {
    if sel.0 <= sel.1 { sel } else { (sel.1, sel.0) }
}

/// The part of the text at `index` (of `len` bytes) that the selection covers.
pub(crate) fn selected_range(sel: Option<Sel>, index: usize, len: usize) -> Option<Range<usize>> {
    let (lo, hi) = ordered(sel?);
    if index < lo.0 || index > hi.0 {
        return None;
    }
    let start = if index == lo.0 { lo.1 } else { 0 };
    let end = if index == hi.0 { hi.1 } else { len };
    let (start, end) = (start.min(len), end.min(len));
    (start < end).then_some(start..end)
}

/// The selected text across `texts`, one line per piece.
pub(crate) fn selected_text(texts: &[&str], sel: Sel) -> String {
    let (lo, hi) = ordered(sel);
    let mut out: Vec<&str> = Vec::new();
    for index in lo.0..=hi.0.min(texts.len().saturating_sub(1)) {
        let Some(text) = texts.get(index) else { break };
        if let Some(range) = selected_range(Some(sel), index, text.len()) {
            if let Some(piece) = text.get(range) {
                out.push(piece);
            }
        }
    }
    out.join("\n")
}

/// The word around byte `at`: letters, digits and underscores. Empty when `at` is not in one.
pub(crate) fn word_range(text: &str, at: usize) -> Range<usize> {
    let at = at.min(text.len());
    let is_word = |c: char| c.is_alphanumeric() || c == '_';
    let start = text[..at].char_indices().rev().take_while(|(_, c)| is_word(*c)).last().map_or(at, |(i, _)| i);
    let end = at + text[at..].char_indices().take_while(|(_, c)| is_word(*c)).last().map_or(0, |(i, c)| i + c.len_utf8());
    start..end
}

/// `runs` (sorted, not overlapping) with `range` painted as well: the runs are cut at its edges, the parts inside get
/// `set` applied, and the gaps inside it become runs of `base` with `set` applied.
pub(crate) fn paint_background<S: Clone>(runs: Vec<(Range<usize>, S)>, range: Range<usize>, base: S, set: impl Fn(&mut S)) -> Vec<(Range<usize>, S)> {
    let mut out = Vec::with_capacity(runs.len() + 2);
    let mut cursor = range.start;
    for (r, style) in runs {
        if r.start < range.start {
            out.push((r.start..r.end.min(range.start), style.clone()));
        }
        let (from, to) = (r.start.max(range.start), r.end.min(range.end));
        if from < to {
            if cursor < from {
                let mut gap = base.clone();
                set(&mut gap);
                out.push((cursor..from, gap));
            }
            let mut inside = style.clone();
            set(&mut inside);
            out.push((from..to, inside));
            cursor = to;
        }
        if r.end > range.end {
            out.push((r.start.max(range.end)..r.end, style));
        }
    }
    if cursor < range.end {
        let mut gap = base;
        set(&mut gap);
        out.push((cursor..range.end, gap));
    }
    out
}

impl Workspace {
    /// The chat's drawn texts take part in selection through this.
    pub(crate) fn claude_selectable(&self) -> Selectable<'_> {
        Selectable { texts: &self.claude.texts, sel: self.claude.sel, color: self.theme.selection }
    }

    /// Plain text in the chat (a question, a notice, tool output) that can be selected.
    pub(crate) fn claude_text(&self, text: &str) -> AnyElement {
        self.claude_selectable().register(text, Vec::new()).into_any_element()
    }

    /// A line of tool output or code in the chat's code font.
    pub(crate) fn claude_code_line(&self, line: &str) -> AnyElement {
        div().whitespace_nowrap().min_h(gpui::rems(1.0)).font_weight(FontWeight::NORMAL).child(self.claude_text(&line.replace('\t', "    "))).into_any_element()
    }

    /// The place under `pos`: inside one of the drawn texts, or with `nearest` the text closest to it (a drag that
    /// goes past the edge of the text it began in).
    fn claude_hit(&self, pos: Point<Pixels>, nearest: bool) -> Option<Pos> {
        let texts = self.claude.texts.borrow();
        let mut best: Option<(f32, usize)> = None;
        for (i, r) in texts.iter().enumerate() {
            let b = r.layout.bounds();
            if b.contains(&pos) {
                let at = r.layout.index_for_position(pos).unwrap_or_else(|near| near);
                return Some((i, at.min(r.text.len())));
            }
            if nearest {
                let (top, bottom) = (f32::from(b.top()), f32::from(b.bottom()));
                let y = f32::from(pos.y);
                let gap = if y < top { top - y } else if y > bottom { y - bottom } else { 0.0 };
                if best.is_none_or(|(g, _)| gap < g) {
                    best = Some((gap, i));
                }
            }
        }
        let (_, i) = best?;
        let r = &texts[i];
        let at = r.layout.index_for_position(pos).unwrap_or_else(|near| near);
        Some((i, at.min(r.text.len())))
    }

    /// A press in the chat: a click places the start of a selection, a double click takes a word, a triple click a whole piece.
    pub(crate) fn claude_select_down(&mut self, ev: &MouseDownEvent) {
        self.focus = Focus::Claude;
        let Some((block, at)) = self.claude_hit(ev.position, false) else {
            self.claude.sel = None;
            return;
        };
        let text = self.claude.texts.borrow().get(block).map(|r| r.text.clone()).unwrap_or_default();
        self.claude.sel = Some(match ev.click_count {
            1 if ev.modifiers.shift && self.claude.sel.is_some() => (self.claude.sel.map_or((block, at), |s| s.0), (block, at)),
            1 => ((block, at), (block, at)),
            2 => {
                let word = word_range(&text, at);
                ((block, word.start), (block, word.end))
            }
            _ => ((block, 0), (block, text.len())),
        });
        self.claude.sel_dragging = ev.click_count == 1;
    }

    /// The pointer moved with the button down: stretch the selection. True when it changed.
    pub(crate) fn claude_select_move(&mut self, ev: &MouseMoveEvent) -> bool {
        if !self.claude.sel_dragging {
            return false;
        }
        if ev.pressed_button != Some(gpui::MouseButton::Left) {
            self.claude.sel_dragging = false;
            return false;
        }
        let Some(head) = self.claude_hit(ev.position, true) else { return false };
        let Some((anchor, old)) = self.claude.sel else { return false };
        if old == head {
            return false;
        }
        self.claude.sel = Some((anchor, head));
        true
    }

    /// While a drag is held above or below the chat, scroll it that way and carry the selection along. Called on the timer,
    /// since a held pointer sends no events. True when something moved.
    pub(crate) fn claude_select_autoscroll(&mut self) -> bool {
        if !self.claude.sel_dragging {
            return false;
        }
        let (Some(pos), bounds) = (self.mouse, self.claude.scroll.bounds()) else { return false };
        // The frame after a scroll has laid the texts out again: the selection's head goes to where the pointer is now.
        if std::mem::take(&mut self.claude.drag_follow) {
            if let (Some(head), Some((anchor, _))) = (self.claude_hit(pos, true), self.claude.sel) {
                self.claude.sel = Some((anchor, head));
            }
        }
        let step = autoscroll_step(f32::from(pos.y), f32::from(bounds.top()), f32::from(bounds.bottom()));
        if step == 0.0 {
            return false;
        }
        let offset = self.claude.scroll.offset();
        let max = self.claude.scroll.max_offset().height;
        let y = (offset.y - Pixels::from(step)).clamp(-max, Pixels::ZERO);
        if y == offset.y {
            return false;
        }
        self.claude.scroll.set_offset(Point::new(offset.x, y));
        // The texts are laid out again on the next frame; until then the head follows the old layout, so it is moved then.
        self.claude.drag_follow = true;
        true
    }

    pub(crate) fn claude_select_up(&mut self) {
        self.claude.sel_dragging = false;
        // A click without a drag leaves nothing selected.
        if let Some((a, h)) = self.claude.sel {
            if a == h {
                self.claude.sel = None;
            }
        }
    }

    /// The selected text of the chat, if any.
    pub(crate) fn claude_selection_text(&self) -> Option<String> {
        let sel = self.claude.sel?;
        let texts = self.claude.texts.borrow();
        let all: Vec<&str> = texts.iter().map(|r| r.text.as_str()).collect();
        let text = selected_text(&all, sel);
        (!text.is_empty()).then_some(text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_selection_covers_the_end_of_its_first_text_the_middle_ones_and_the_start_of_its_last() {
        let sel = Some(((1, 3), (3, 2)));
        assert_eq!(selected_range(sel, 0, 10), None);
        assert_eq!(selected_range(sel, 1, 10), Some(3..10));
        assert_eq!(selected_range(sel, 2, 7), Some(0..7));
        assert_eq!(selected_range(sel, 3, 10), Some(0..2));
        assert_eq!(selected_range(sel, 4, 10), None);
        // Dragged backwards, it is the same selection.
        assert_eq!(selected_range(Some(((3, 2), (1, 3))), 1, 10), Some(3..10));
        // Inside one text, and an empty one.
        assert_eq!(selected_range(Some(((2, 4), (2, 1))), 2, 8), Some(1..4));
        assert_eq!(selected_range(Some(((2, 4), (2, 4))), 2, 8), None);
    }

    #[test]
    fn the_selected_text_is_cut_from_each_piece_and_joined_by_lines() {
        let texts = ["first paragraph", "second", "third one"];
        assert_eq!(selected_text(&texts, ((0, 6), (2, 5))), "paragraph\nsecond\nthird");
        assert_eq!(selected_text(&texts, ((1, 2), (1, 5))), "con");
        assert_eq!(selected_text(&texts, ((2, 9), (0, 0))), "first paragraph\nsecond\nthird one");
        assert_eq!(selected_text(&texts, ((1, 3), (1, 3))), "");
    }

    #[test]
    fn dragging_outside_the_chat_scrolls_toward_the_pointer() {
        assert_eq!(autoscroll_step(100.0, 50.0, 400.0), 0.0);
        assert!(autoscroll_step(20.0, 50.0, 400.0) < 0.0);
        assert!(autoscroll_step(450.0, 50.0, 400.0) > 0.0);
        assert!(autoscroll_step(2000.0, 50.0, 400.0) <= 40.0);
    }

    #[test]
    fn a_word_is_found_around_a_position() {
        assert_eq!(word_range("let total_count = 5;", 6), 4..15);
        assert_eq!(word_range("let total_count = 5;", 4), 4..15);
        assert_eq!(word_range("let total_count = 5;", 15), 4..15);
        assert_eq!(word_range("a  b", 2), 2..2);
        assert_eq!(word_range("naïve café", 9), 7..12);
    }

    #[test]
    fn painting_cuts_the_runs_and_fills_the_gaps() {
        // Runs 0..4 'a' and 6..10 'b'; paint 2..8.
        let out = paint_background(vec![(0..4, "a".to_owned()), (6..10, "b".to_owned())], 2..8, String::new(), |s| s.push('*'));
        assert_eq!(out, vec![(0..2, "a".to_owned()), (2..4, "a*".to_owned()), (4..6, "*".to_owned()), (6..8, "b*".to_owned()), (8..10, "b".to_owned())]);
        // No runs at all: the whole range is one painted run.
        assert_eq!(paint_background(Vec::<(Range<usize>, String)>::new(), 1..3, String::new(), |s| s.push('*')), vec![(1..3, "*".to_owned())]);
        // A run that spans the whole range is cut into three.
        assert_eq!(paint_background(vec![(0..10, 1)], 3..5, 0, |s| *s += 10), vec![(0..3, 1), (3..5, 11), (5..10, 1)]);
    }
}
