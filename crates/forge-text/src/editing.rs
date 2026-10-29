//! VS Code's editing commands as pure operations on a [`Buffer`].
//!
//! Every command is one undoable action, works on all cursors, and returns the new
//! selections. Commands that are not applicable return the selections unchanged.

use crate::buffer::Buffer;
use crate::selection::{Selection, SelectionSet};
use crate::words::{is_word_char, next_word_end, prev_word_start, word_range_at};

/// How indentation is written.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct IndentStyle {
    pub tab_size: usize,
    pub use_spaces: bool,
}

impl Default for IndentStyle {
    fn default() -> Self {
        Self { tab_size: 4, use_spaces: true }
    }
}

impl IndentStyle {
    pub fn unit(&self) -> String {
        if self.use_spaces { " ".repeat(self.tab_size) } else { "\t".to_owned() }
    }
}

/// Bracket and quote pairs used for auto-closing.
pub const PAIRS: &[(char, char)] = &[('(', ')'), ('[', ']'), ('{', '}'), ('"', '"'), ('\'', '\''), ('`', '`')];

fn closing_for(open: char) -> Option<char> {
    PAIRS.iter().find(|(o, _)| *o == open).map(|(_, c)| *c)
}

fn is_quote(c: char) -> bool {
    matches!(c, '"' | '\'' | '`')
}

/// Map an offset through sorted replacements. An insertion exactly at `pos` moves it
/// only when `after` is set; a position inside a replaced range collapses to its start.
pub fn map_through(reps: &[(usize, usize, String)], pos: usize, after: bool) -> usize {
    let mut delta: isize = 0;
    for (s, e, text) in reps {
        let before_pos = *e <= pos && !(*s == *e && *s == pos && !after);
        if before_pos {
            delta += text.chars().count() as isize - (*e - *s) as isize;
        } else if *s < pos && pos < *e {
            return (*s as isize + delta) as usize;
        } else {
            break;
        }
    }
    (pos as isize + delta) as usize
}

impl Buffer {
    /// Merged, inclusive line ranges touched by the selections. A non-empty selection
    /// ending at column 0 does not include that last line.
    pub fn touched_blocks(&self, selections: &SelectionSet) -> Vec<(usize, usize)> {
        let mut blocks: Vec<(usize, usize)> = Vec::new();
        for s in selections.as_slice() {
            let (first, _) = self.offset_to_point(s.start());
            let (mut last, end_col) = self.offset_to_point(s.end());
            if !s.is_empty() && end_col == 0 && last > first {
                last -= 1;
            }
            match blocks.last_mut() {
                Some(block) if first <= block.1 + 1 => block.1 = block.1.max(last),
                _ => blocks.push((first, last)),
            }
        }
        blocks
    }

    fn line_text(&self, line: usize) -> String {
        self.line(line)
    }

    fn leading_whitespace(&self, line: usize) -> String {
        self.line(line).chars().take_while(|c| *c == ' ' || *c == '\t').collect()
    }

    // ---- movement ----

    /// Ctrl+Left / Ctrl+Right.
    pub fn move_word(&self, selections: &SelectionSet, forward: bool, extend: bool) -> SelectionSet {
        selections.map(|s| {
            let head = if !extend && !s.is_empty() {
                if forward { s.end() } else { s.start() }
            } else if forward {
                next_word_end(self.rope(), s.head)
            } else {
                prev_word_start(self.rope(), s.head)
            };
            Selection { anchor: if extend { s.anchor } else { head }, head }
        })
    }

    /// Home: jump to the first non-blank char, or to column 0 if already there.
    pub fn smart_home(&self, selections: &SelectionSet, extend: bool) -> SelectionSet {
        selections.map(|s| {
            let (line, col) = self.offset_to_point(s.head);
            let indent = self.leading_whitespace(line).chars().count();
            let target = if col == indent || indent == self.line_len(line) && col == 0 && indent != 0 { 0 } else { indent };
            let head = self.line_start(line) + target;
            Selection { anchor: if extend { s.anchor } else { head }, head }
        })
    }

    /// Ctrl+Home / Ctrl+End.
    pub fn move_to_doc_edge(&self, selections: &SelectionSet, end: bool, extend: bool) -> SelectionSet {
        let head = if end { self.len_chars() } else { 0 };
        let anchor = selections.primary().anchor;
        SelectionSet::single(Selection { anchor: if extend { anchor } else { head }, head })
    }

    /// Vertical movement that keeps a "goal" column across short lines, like VS Code.
    /// `goal` is updated when it is unset and cleared by the caller on other commands.
    pub fn move_vertical_with_goal(
        &self,
        selections: &SelectionSet,
        delta: isize,
        extend: bool,
        goal: &mut Option<usize>,
    ) -> SelectionSet {
        let wanted = *goal.get_or_insert_with(|| self.offset_to_point(selections.primary().head).1);
        selections.map(|s| {
            let (line, col) = self.offset_to_point(s.head);
            let target = (line as isize + delta).clamp(0, self.len_lines() as isize - 1) as usize;
            // Moving past the first/last line goes to the buffer edge, as in VS Code.
            let head = if line as isize + delta < 0 {
                0
            } else if line as isize + delta >= self.len_lines() as isize {
                self.len_chars()
            } else {
                self.point_to_offset(target, if selections.as_slice().len() == 1 { wanted } else { col })
            };
            Selection { anchor: if extend { s.anchor } else { head }, head }
        })
    }

    // ---- selection commands ----

    /// Ctrl+L: select whole lines; repeating extends downward.
    pub fn select_line(&self, selections: &SelectionSet) -> SelectionSet {
        selections.map(|s| {
            let (first, _) = self.offset_to_point(s.start());
            let (last, _) = self.offset_to_point(s.end());
            // One line past the selection's last line: extends downward when repeated.
            let last = last + 1;
            let end = if last >= self.len_lines() { self.len_chars() } else { self.line_start(last) };
            Selection { anchor: self.line_start(first), head: end }
        })
    }

    /// Ctrl+D: select the word under the caret, then add the next occurrence of the
    /// selected text (wrapping around), skipping ones already selected.
    pub fn add_next_occurrence(&self, selections: &SelectionSet) -> SelectionSet {
        let primary = selections.as_slice().last().copied().unwrap_or(selections.primary());
        if primary.is_empty() {
            return match word_range_at(self.rope(), primary.head) {
                Some((s, e)) => {
                    let mut all: Vec<Selection> = selections.as_slice().iter().copied().filter(|x| !x.is_empty()).collect();
                    all.push(Selection { anchor: s, head: e });
                    SelectionSet::new(all)
                }
                None => selections.clone(),
            };
        }
        let needle = self.rope().slice(primary.start()..primary.end()).to_string();
        let text = self.text();
        let needle_chars = needle.chars().count();
        let taken = |at: usize| selections.as_slice().iter().any(|s| s.start() < at + needle_chars && at < s.end());
        for at in occurrences_from(&text, &needle, primary.end()).chain(occurrences_from(&text, &needle, 0)) {
            if !taken(at) {
                let mut all = selections.as_slice().to_vec();
                all.push(Selection { anchor: at, head: at + needle_chars });
                return SelectionSet::new(all);
            }
        }
        selections.clone()
    }

    /// Ctrl+Shift+L: select every occurrence of the selected text (or the word at the caret).
    pub fn select_all_occurrences(&self, selections: &SelectionSet) -> SelectionSet {
        let primary = selections.primary();
        let (start, end) = if primary.is_empty() {
            match word_range_at(self.rope(), primary.head) {
                Some(range) => range,
                None => return selections.clone(),
            }
        } else {
            (primary.start(), primary.end())
        };
        let needle = self.rope().slice(start..end).to_string();
        let n = needle.chars().count();
        let text = self.text();
        let all: Vec<Selection> =
            occurrences_from(&text, &needle, 0).map(|at| Selection { anchor: at, head: at + n }).collect();
        if all.is_empty() { selections.clone() } else { SelectionSet::new(all) }
    }

    // ---- deleting ----

    /// Ctrl+Backspace / Ctrl+Delete.
    pub fn delete_word(&mut self, selections: &SelectionSet, forward: bool) -> SelectionSet {
        let ranges: Vec<(usize, usize)> = selections
            .as_slice()
            .iter()
            .map(|s| {
                if !s.is_empty() {
                    (s.start(), s.end())
                } else if forward {
                    (s.head, next_word_end(self.rope(), s.head))
                } else {
                    (prev_word_start(self.rope(), s.head), s.head)
                }
            })
            .collect();
        let reps: Vec<(usize, usize, String)> = merge_ranges(ranges).into_iter().map(|(s, e)| (s, e, String::new())).collect();
        self.edit_many(selections, &reps, |_, starts| carets_at(starts))
    }

    /// Ctrl+Shift+K: delete the lines the cursors are on.
    pub fn delete_lines(&mut self, selections: &SelectionSet) -> SelectionSet {
        let blocks = self.touched_blocks(selections);
        let last_line = self.len_lines() - 1;
        let cols: Vec<usize> = blocks.iter().map(|(a, _)| self.offset_to_point(selections.primary().head).1.min(self.line_len(*a))).collect();
        let reps: Vec<(usize, usize, String)> = blocks
            .iter()
            .map(|&(a, b)| {
                if b < last_line {
                    (self.line_start(a), self.line_start(b + 1), String::new())
                } else if a > 0 {
                    // Deleting through the end: take the previous line's terminator instead.
                    (self.line_end(a - 1), self.len_chars(), String::new())
                } else {
                    (0, self.len_chars(), String::new())
                }
            })
            .collect();
        let line_after: Vec<usize> = blocks.iter().map(|(a, _)| *a).collect();
        self.edit_many(selections, &reps, move |buf, _| {
            SelectionSet::new(
                line_after
                    .iter()
                    .zip(&cols)
                    .map(|(line, col)| Selection::caret(buf.point_to_offset((*line).min(buf.len_lines() - 1), *col)))
                    .collect(),
            )
        })
    }

    // ---- line operations ----

    /// Ctrl+Enter / Ctrl+Shift+Enter: open a new line below or above, keeping indentation.
    pub fn insert_line(&mut self, selections: &SelectionSet, above: bool) -> SelectionSet {
        let eol = self.eol();
        let blocks = self.touched_blocks(selections);
        let reps: Vec<(usize, usize, String)> = blocks
            .iter()
            .map(|&(a, b)| {
                let indent = self.leading_whitespace(if above { a } else { b });
                if above {
                    let at = self.line_start(a);
                    (at, at, format!("{indent}{eol}"))
                } else {
                    let at = self.line_end(b);
                    (at, at, format!("{eol}{indent}"))
                }
            })
            .collect();
        let carets: Vec<usize> = reps
            .iter()
            .map(|(_, _, t)| if above { t.chars().count() - eol.chars().count() } else { t.chars().count() })
            .collect();
        self.edit_many(selections, &reps, move |_, starts| {
            SelectionSet::new(starts.iter().zip(&carets).map(|(s, c)| Selection::caret(s + c)).collect())
        })
    }

    /// Alt+Up / Alt+Down.
    pub fn move_lines(&mut self, selections: &SelectionSet, up: bool) -> SelectionSet {
        let blocks = self.touched_blocks(selections);
        let last_line = self.len_lines() - 1;
        let eol = self.eol();
        let mut reps = Vec::new();
        let mut moved: Vec<(usize, usize)> = Vec::new();
        for &(a, b) in &blocks {
            if (up && a == 0) || (!up && b >= last_line) {
                continue;
            }
            // The neighbour is moved across the block; both sides are rewritten as one region.
            let (first, last) = if up { (a - 1, b) } else { (a, b + 1) };
            let mut lines: Vec<String> = (first..=last).map(|l| self.line_text(l)).collect();
            if up {
                let prev = lines.remove(0);
                lines.push(prev);
            } else {
                let next = lines.pop().unwrap_or_default();
                lines.insert(0, next);
            }
            reps.push((self.line_start(first), self.line_end(last), lines.join(eol)));
            moved.push((a, b));
        }
        if reps.is_empty() {
            return selections.clone();
        }
        let points: Vec<((usize, usize), (usize, usize))> = selections
            .as_slice()
            .iter()
            .map(|s| (self.offset_to_point(s.anchor), self.offset_to_point(s.head)))
            .collect();
        let delta: isize = if up { -1 } else { 1 };
        self.edit_many(selections, &reps, move |buf, _| {
            SelectionSet::new(
                points
                    .iter()
                    .map(|((al, ac), (hl, hc))| {
                        let shift = |line: usize| {
                            if moved.iter().any(|(a, b)| *a <= line && line <= *b) { (line as isize + delta) as usize } else { line }
                        };
                        Selection {
                            anchor: buf.point_to_offset(shift(*al), *ac),
                            head: buf.point_to_offset(shift(*hl), *hc),
                        }
                    })
                    .collect(),
            )
        })
    }

    /// Shift+Alt+Up / Shift+Alt+Down: duplicate the lines above or below. After copying
    /// up the cursor stays on the upper copy; after copying down it moves to the lower one.
    pub fn copy_lines(&mut self, selections: &SelectionSet, up: bool) -> SelectionSet {
        let eol = self.eol();
        let blocks = self.touched_blocks(selections);
        let reps: Vec<(usize, usize, String)> = blocks
            .iter()
            .map(|&(a, b)| {
                let body = (a..=b).map(|l| self.line_text(l)).collect::<Vec<_>>().join(eol);
                if up {
                    let at = self.line_start(a);
                    (at, at, format!("{body}{eol}"))
                } else {
                    let at = self.line_end(b);
                    (at, at, format!("{eol}{body}"))
                }
            })
            .collect();
        let points: Vec<((usize, usize), (usize, usize))> = selections
            .as_slice()
            .iter()
            .map(|s| (self.offset_to_point(s.anchor), self.offset_to_point(s.head)))
            .collect();
        self.edit_many(selections, &reps, move |buf, _| {
            let shifted = |line: usize| {
                let mut out = line;
                for &(a, b) in &blocks {
                    let size = b - a + 1;
                    if b < line || (!up && a <= line && line <= b) {
                        out += size;
                    }
                }
                out
            };
            SelectionSet::new(
                points
                    .iter()
                    .map(|((al, ac), (hl, hc))| Selection {
                        anchor: buf.point_to_offset(shifted(*al), *ac),
                        head: buf.point_to_offset(shifted(*hl), *hc),
                    })
                    .collect(),
            )
        })
    }

    /// Join each block's lines (a single caret joins with the next line).
    pub fn join_lines(&mut self, selections: &SelectionSet) -> SelectionSet {
        let blocks = self.touched_blocks(selections);
        let mut reps = Vec::new();
        for &(a, b) in &blocks {
            let end_line = if b > a { b } else { a + 1 };
            if end_line >= self.len_lines() {
                continue;
            }
            let mut joined = self.line_text(a);
            for l in a + 1..=end_line {
                let next = self.line_text(l);
                let trimmed = next.trim_start();
                if !trimmed.is_empty() && !joined.is_empty() && !joined.ends_with(' ') {
                    joined.push(' ');
                }
                joined.push_str(trimmed);
            }
            reps.push((self.line_start(a), self.line_end(end_line), joined));
        }
        if reps.is_empty() {
            return selections.clone();
        }
        let first_len: Vec<usize> = blocks.iter().map(|(a, _)| self.line_len(*a)).collect();
        self.edit_many(selections, &reps, move |_, starts| {
            SelectionSet::new(starts.iter().zip(&first_len).map(|(s, n)| Selection::caret(s + n)).collect())
        })
    }

    /// Ctrl+]: indent every touched line.
    pub fn indent_lines(&mut self, selections: &SelectionSet, style: IndentStyle) -> SelectionSet {
        let unit = style.unit();
        let reps: Vec<(usize, usize, String)> = self
            .touched_blocks(selections)
            .into_iter()
            .flat_map(|(a, b)| a..=b)
            .filter(|l| self.line_len(*l) > 0)
            .map(|l| (self.line_start(l), self.line_start(l), unit.clone()))
            .collect();
        self.remap_selections_through(selections, reps)
    }

    /// Ctrl+[ / Shift+Tab: remove one indent level from every touched line.
    pub fn outdent_lines(&mut self, selections: &SelectionSet, style: IndentStyle) -> SelectionSet {
        let reps: Vec<(usize, usize, String)> = self
            .touched_blocks(selections)
            .into_iter()
            .flat_map(|(a, b)| a..=b)
            .filter_map(|l| {
                let line = self.line_text(l);
                let remove = if line.starts_with('\t') {
                    1
                } else {
                    line.chars().take(style.tab_size).take_while(|c| *c == ' ').count()
                };
                (remove > 0).then(|| (self.line_start(l), self.line_start(l) + remove, String::new()))
            })
            .collect();
        self.remap_selections_through(selections, reps)
    }

    /// Apply `reps` and carry every selection endpoint across them.
    fn remap_selections_through(&mut self, selections: &SelectionSet, reps: Vec<(usize, usize, String)>) -> SelectionSet {
        if reps.is_empty() {
            return selections.clone();
        }
        let old = selections.as_slice().to_vec();
        let mapped = reps.clone();
        self.edit_many(selections, &reps, move |_, _| {
            SelectionSet::new(
                old.iter()
                    .map(|s| Selection {
                        anchor: map_through(&mapped, s.anchor, true),
                        head: map_through(&mapped, s.head, true),
                    })
                    .collect(),
            )
        })
    }

    /// Tab: indent selected lines, or insert up to the next tab stop at each caret.
    pub fn insert_tab(&mut self, selections: &SelectionSet, style: IndentStyle) -> SelectionSet {
        let multi_line = selections.as_slice().iter().any(|s| {
            !s.is_empty() && self.offset_to_point(s.start()).0 != self.offset_to_point(s.end()).0
        });
        if multi_line {
            return self.indent_lines(selections, style);
        }
        let reps: Vec<(usize, usize, String)> = selections
            .as_slice()
            .iter()
            .map(|s| {
                let text = if style.use_spaces {
                    let col = self.offset_to_point(s.start()).1;
                    " ".repeat(style.tab_size - col % style.tab_size)
                } else {
                    "\t".to_owned()
                };
                (s.start(), s.end(), text)
            })
            .collect();
        let lens: Vec<usize> = reps.iter().map(|(_, _, t)| t.chars().count()).collect();
        self.edit_many(selections, &reps, move |_, starts| {
            SelectionSet::new(starts.iter().zip(&lens).map(|(s, n)| Selection::caret(s + n)).collect::<Vec<_>>())
        })
    }

    /// Ctrl+/: toggle `token` comments on the touched, non-blank lines.
    pub fn toggle_line_comment(&mut self, selections: &SelectionSet, token: &str) -> SelectionSet {
        let lines: Vec<usize> = self
            .touched_blocks(selections)
            .into_iter()
            .flat_map(|(a, b)| a..=b)
            .filter(|l| !self.line_text(*l).trim().is_empty())
            .collect();
        if lines.is_empty() {
            return selections.clone();
        }
        let all_commented = lines.iter().all(|l| self.line_text(*l).trim_start().starts_with(token));
        let reps: Vec<(usize, usize, String)> = if all_commented {
            lines
                .iter()
                .map(|l| {
                    let line = self.line_text(*l);
                    let indent = line.chars().take_while(|c| c.is_whitespace()).count();
                    let after_token = line.chars().nth(indent + token.chars().count());
                    let len = token.chars().count() + usize::from(after_token == Some(' '));
                    (self.line_start(*l) + indent, self.line_start(*l) + indent + len, String::new())
                })
                .collect()
        } else {
            // Comment at the shallowest indent so the block stays aligned.
            let col = lines
                .iter()
                .map(|l| self.line_text(*l).chars().take_while(|c| c.is_whitespace()).count())
                .min()
                .unwrap_or(0);
            lines
                .iter()
                .map(|l| (self.line_start(*l) + col, self.line_start(*l) + col, format!("{token} ")))
                .collect()
        };
        self.remap_selections_through(selections, reps)
    }

    /// Shift+Alt+A: wrap each selection (or the caret's line content) in a block comment,
    /// or unwrap it when it is already wrapped.
    pub fn toggle_block_comment(&mut self, selections: &SelectionSet, open: &str, close: &str) -> SelectionSet {
        let reps: Vec<(usize, usize, String)> = selections
            .as_slice()
            .iter()
            .map(|s| {
                let (start, end) = if s.is_empty() {
                    let (line, _) = self.offset_to_point(s.head);
                    let text = self.line_text(line);
                    let lead = text.chars().take_while(|c| c.is_whitespace()).count();
                    (self.line_start(line) + lead, self.line_end(line))
                } else {
                    (s.start(), s.end())
                };
                let inner = self.rope().slice(start..end).to_string();
                let text = if inner.starts_with(open) && inner.ends_with(close) && inner.len() >= open.len() + close.len() {
                    inner[open.len()..inner.len() - close.len()].trim_matches(' ').to_owned()
                } else {
                    format!("{open} {inner} {close}")
                };
                (start, end, text)
            })
            .collect();
        let reps = merge_reps(reps);
        let ends: Vec<usize> = reps.iter().map(|(_, _, t)| t.chars().count()).collect();
        self.edit_many(selections, &reps, move |_, starts| {
            SelectionSet::new(
                starts.iter().zip(&ends).map(|(s, n)| Selection { anchor: *s, head: s + n }).collect(),
            )
        })
    }

    // ---- typing ----

    /// Enter: split the line, carrying indentation. After an opening bracket the new line
    /// is indented one level, and a pair like `{|}` opens onto its own line.
    pub fn newline_with_indent(&mut self, selections: &SelectionSet, style: IndentStyle, indent_after_colon: bool) -> SelectionSet {
        let eol = self.eol();
        let unit = style.unit();
        let mut carets = Vec::new();
        let reps: Vec<(usize, usize, String)> = selections
            .as_slice()
            .iter()
            .map(|s| {
                let (line, col) = self.offset_to_point(s.start());
                let indent = self.leading_whitespace(line);
                let before: String = self.line_text(line).chars().take(col).collect();
                let trimmed = before.trim_end();
                let opens = trimmed.ends_with(['{', '[', '(']) || (indent_after_colon && trimmed.ends_with(':'));
                let next = self.rope().get_char(s.end()).filter(|c| *c != '\n' && *c != '\r');
                let between = trimmed.chars().last().zip(next).is_some_and(|(o, c)| closing_for(o) == Some(c) && !is_quote(o));
                let (text, caret) = if between {
                    let inner = format!("{eol}{indent}{unit}");
                    let caret = inner.chars().count();
                    (format!("{inner}{eol}{indent}"), caret)
                } else if opens {
                    let t = format!("{eol}{indent}{unit}");
                    let n = t.chars().count();
                    (t, n)
                } else {
                    let t = format!("{eol}{indent}");
                    let n = t.chars().count();
                    (t, n)
                };
                carets.push(caret);
                (s.start(), s.end(), text)
            })
            .collect();
        self.edit_many(selections, &reps, move |_, starts| {
            SelectionSet::new(starts.iter().zip(&carets).map(|(s, c)| Selection::caret(s + c)).collect())
        })
    }

    /// Type `text` with auto-closing pairs, overtyping of closers and surround-selection.
    pub fn type_text(&mut self, selections: &SelectionSet, text: &str, auto_close: bool) -> SelectionSet {
        let mut chars = text.chars();
        let (Some(c), None) = (chars.next(), chars.next()) else {
            return self.insert(selections, text);
        };
        if !auto_close {
            return self.insert(selections, text);
        }
        let is_closer = PAIRS.iter().any(|(_, close)| *close == c);
        let mut carets_after: Vec<usize> = Vec::new();
        let reps: Vec<(usize, usize, String)> = selections
            .as_slice()
            .iter()
            .map(|s| {
                let next = self.rope().get_char(s.end());
                let prev = (s.start() > 0).then(|| self.rope().char(s.start() - 1));
                if s.is_empty() && is_closer && next == Some(c) {
                    // Overtype the closer that is already there.
                    carets_after.push(1);
                    return (s.start(), s.end() + 1, c.to_string());
                }
                if let Some(close) = closing_for(c) {
                    if !s.is_empty() {
                        let inner = self.rope().slice(s.start()..s.end()).to_string();
                        carets_after.push(usize::MAX); // marker: keep the wrapped text selected
                        return (s.start(), s.end(), format!("{c}{inner}{close}"));
                    }
                    let next_ok = next.is_none_or(|n| n.is_whitespace() || matches!(n, ')' | ']' | '}' | ',' | ';'));
                    let quote_ok = !is_quote(c) || (!prev.is_some_and(|p| is_word_char(p) || p == c) && next != Some(c));
                    if next_ok && quote_ok {
                        carets_after.push(1);
                        return (s.start(), s.end(), format!("{c}{close}"));
                    }
                }
                carets_after.push(c.len_utf8().min(1));
                (s.start(), s.end(), c.to_string())
            })
            .collect();
        let lens: Vec<usize> = reps.iter().map(|(_, _, t)| t.chars().count()).collect();
        self.edit_many(selections, &reps, move |_, starts| {
            SelectionSet::new(
                starts
                    .iter()
                    .zip(&carets_after)
                    .zip(&lens)
                    .map(|((s, c), n)| {
                        if *c == usize::MAX {
                            Selection { anchor: s + 1, head: s + n - 1 }
                        } else {
                            Selection::caret(s + c)
                        }
                    })
                    .collect(),
            )
        })
    }

    /// Backspace that also removes the closer of an empty pair like `(|)`.
    pub fn backspace_with_pairs(&mut self, selections: &SelectionSet) -> SelectionSet {
        let in_empty_pair = |s: &Selection| {
            s.is_empty()
                && s.head > 0
                && self.rope().get_char(s.head).zip(Some(self.rope().char(s.head - 1))).is_some_and(|(next, prev)| closing_for(prev) == Some(next))
        };
        if !selections.as_slice().iter().any(&in_empty_pair) {
            return self.delete_backward(selections);
        }
        let reps: Vec<(usize, usize, String)> = selections
            .as_slice()
            .iter()
            .map(|s| if in_empty_pair(s) { (s.head - 1, s.head + 1, String::new()) } else if s.is_empty() { (s.head.saturating_sub(1), s.head, String::new()) } else { (s.start(), s.end(), String::new()) })
            .collect();
        let reps = merge_reps(reps);
        self.edit_many(selections, &reps, |_, starts| carets_at(starts))
    }

    /// Backspace as VS Code does it: in leading whitespace delete back to the previous tab
    /// stop; between an empty pair `(|)` delete both halves; otherwise one char.
    pub fn backspace(&mut self, selections: &SelectionSet, style: IndentStyle, pairs: bool) -> SelectionSet {
        let tab = style.tab_size.max(1);
        let reps: Vec<(usize, usize, String)> = selections
            .as_slice()
            .iter()
            .map(|s| {
                if !s.is_empty() {
                    return (s.start(), s.end(), String::new());
                }
                let (line, col) = self.offset_to_point(s.head);
                let before: String = self.line_text(line).chars().take(col).collect();
                let in_indent = col > 0 && style.use_spaces && before.chars().all(|c| c == ' ');
                if in_indent {
                    let remove = if col % tab == 0 { tab } else { col % tab };
                    return (s.head - remove.min(col), s.head, String::new());
                }
                let prev = (s.head > 0).then(|| self.rope().char(s.head - 1));
                let next = self.rope().get_char(s.head);
                if pairs && prev.zip(next).is_some_and(|(p, n)| closing_for(p) == Some(n)) {
                    return (s.head - 1, s.head + 1, String::new());
                }
                (s.head.saturating_sub(1), s.head, String::new())
            })
            .collect();
        let reps = merge_reps(reps);
        self.edit_many(selections, &reps, |_, starts| carets_at(starts))
    }

    // ---- text transforms ----

    /// Sort the touched lines (or the whole buffer when nothing spans several lines).
    pub fn sort_lines(&mut self, selections: &SelectionSet, descending: bool) -> SelectionSet {
        let blocks = self.touched_blocks(selections);
        let blocks: Vec<(usize, usize)> = if blocks.iter().all(|(a, b)| a == b) { vec![(0, self.len_lines() - 1)] } else { blocks };
        let eol = self.eol();
        let reps: Vec<(usize, usize, String)> = blocks
            .iter()
            .map(|&(a, b)| {
                let mut lines: Vec<String> = (a..=b).map(|l| self.line_text(l)).collect();
                lines.sort();
                if descending {
                    lines.reverse();
                }
                (self.line_start(a), self.line_end(b), lines.join(eol))
            })
            .collect();
        let old = selections.clone();
        self.edit_many(selections, &reps, move |_, _| old)
    }

    /// Apply `f` to each selected range's text (the word at a bare caret).
    pub fn transform_selection(&mut self, selections: &SelectionSet, f: impl Fn(&str) -> String) -> SelectionSet {
        let reps: Vec<(usize, usize, String)> = selections
            .as_slice()
            .iter()
            .filter_map(|s| {
                let (start, end) = if s.is_empty() { word_range_at(self.rope(), s.head)? } else { (s.start(), s.end()) };
                Some((start, end, f(&self.rope().slice(start..end).to_string())))
            })
            .collect();
        if reps.is_empty() {
            return selections.clone();
        }
        let mapped = reps.clone();
        let old = selections.as_slice().to_vec();
        self.edit_many(selections, &reps, move |_, _| {
            SelectionSet::new(old.iter().map(|s| Selection { anchor: map_through(&mapped, s.anchor, false), head: map_through(&mapped, s.head, true) }).collect())
        })
    }

    /// Remove trailing spaces and tabs on every line.
    pub fn trim_trailing_whitespace(&mut self, selections: &SelectionSet) -> SelectionSet {
        let reps: Vec<(usize, usize, String)> = (0..self.len_lines())
            .filter_map(|l| {
                let text = self.line_text(l);
                let trimmed = text.trim_end_matches([' ', '\t']).chars().count();
                let len = text.chars().count();
                (trimmed < len).then(|| (self.line_start(l) + trimmed, self.line_start(l) + len, String::new()))
            })
            .collect();
        self.remap_selections_through(selections, reps)
    }

    /// Swap the two chars around each caret (or the last two at a line end).
    pub fn transpose(&mut self, selections: &SelectionSet) -> SelectionSet {
        let reps: Vec<(usize, usize, String)> = selections
            .as_slice()
            .iter()
            .filter_map(|s| {
                let (line, col) = self.offset_to_point(s.head);
                if !s.is_empty() || col == 0 || self.line_len(line) < 2 {
                    return None;
                }
                let at = if col >= self.line_len(line) { s.head - 1 } else { s.head };
                let (a, b) = (self.rope().char(at - 1), self.rope().char(at));
                Some((at - 1, at + 1, format!("{b}{a}")))
            })
            .collect();
        if reps.is_empty() {
            return selections.clone();
        }
        let ends: Vec<usize> = reps.iter().map(|(_, e, _)| *e).collect();
        self.edit_many(selections, &reps, move |_, _| carets_at(&ends))
    }
}

fn carets_at(offsets: &[usize]) -> SelectionSet {
    SelectionSet::new(offsets.iter().map(|o| Selection::caret(*o)).collect())
}

/// Merge overlapping or touching ranges so replacements stay disjoint.
fn merge_ranges(mut ranges: Vec<(usize, usize)>) -> Vec<(usize, usize)> {
    ranges.sort_unstable();
    let mut merged: Vec<(usize, usize)> = Vec::new();
    for (s, e) in ranges {
        match merged.last_mut() {
            Some(last) if s <= last.1 => last.1 = last.1.max(e),
            _ => merged.push((s, e)),
        }
    }
    merged
}

fn merge_reps(mut reps: Vec<(usize, usize, String)>) -> Vec<(usize, usize, String)> {
    reps.sort_by_key(|r| (r.0, r.1));
    reps.dedup_by(|b, a| b.0 < a.1 || (b.0 == a.0 && b.1 == a.1));
    reps
}

/// Char offsets of non-overlapping occurrences of `needle` in `text`, from char offset `from`.
fn occurrences_from<'a>(text: &'a str, needle: &'a str, from: usize) -> impl Iterator<Item = usize> + 'a {
    let byte_from = text.char_indices().nth(from).map_or(text.len(), |(b, _)| b);
    let base = from;
    let hay = &text[byte_from..];
    let mut chars_before = 0usize;
    let mut last_byte = 0usize;
    hay.match_indices(needle).map(move |(b, _)| {
        chars_before += hay[last_byte..b].chars().count();
        last_byte = b;
        base + chars_before
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sel(a: usize, h: usize) -> SelectionSet {
        SelectionSet::single(Selection { anchor: a, head: h })
    }
    fn caret(at: usize) -> SelectionSet {
        sel(at, at)
    }
    fn carets(offsets: &[usize]) -> SelectionSet {
        carets_at(offsets)
    }
    fn heads(s: &SelectionSet) -> Vec<usize> {
        s.as_slice().iter().map(|x| x.head).collect()
    }

    #[test]
    fn map_through_handles_insertions_and_collapsing() {
        let reps = vec![(2, 2, "ab".to_owned()), (5, 8, "x".to_owned())];
        assert_eq!(map_through(&reps, 2, true), 4);
        assert_eq!(map_through(&reps, 2, false), 2);
        assert_eq!(map_through(&reps, 6, true), 7, "inside a replaced range collapses to its (shifted) start");
        assert_eq!(map_through(&reps, 9, true), 9, "net change of the two replacements is zero");
    }

    #[test]
    fn word_movement_and_deletion() {
        let mut b = Buffer::new("foo bar baz");
        assert_eq!(heads(&b.move_word(&caret(0), true, false)), vec![3]);
        assert_eq!(heads(&b.move_word(&caret(11), false, false)), vec![8]);
        let after = b.delete_word(&caret(7), false); // delete "bar"
        assert_eq!(b.text(), "foo  baz");
        assert_eq!(heads(&after), vec![4]);
        b.delete_word(&caret(4), true); // delete the following spaces and word
        assert_eq!(b.text(), "foo ");
    }

    #[test]
    fn smart_home_toggles_between_indent_and_column_zero() {
        let b = Buffer::new("    code");
        assert_eq!(heads(&b.smart_home(&caret(8), false)), vec![4]);
        assert_eq!(heads(&b.smart_home(&caret(4), false)), vec![0]);
        assert_eq!(heads(&b.smart_home(&caret(0), false)), vec![4]);
    }

    #[test]
    fn vertical_goal_column_survives_short_lines() {
        let b = Buffer::new("abcdef\nx\nabcdef");
        let mut goal = None;
        let s = b.move_vertical_with_goal(&caret(4), 1, false, &mut goal);
        assert_eq!(s.primary().head, 8);
        let s = b.move_vertical_with_goal(&s, 1, false, &mut goal);
        assert_eq!(b.offset_to_point(s.primary().head), (2, 4), "returns to the remembered column");
        // Past the last line goes to the end of the buffer.
        let s = b.move_vertical_with_goal(&s, 1, false, &mut goal);
        assert_eq!(s.primary().head, b.len_chars());
    }

    #[test]
    fn select_line_extends_down_on_repeat() {
        let b = Buffer::new("a\nb\nc");
        let once = b.select_line(&caret(0));
        assert_eq!((once.primary().start(), once.primary().end()), (0, 2));
        let twice = b.select_line(&once);
        assert_eq!((twice.primary().start(), twice.primary().end()), (0, 4));
    }

    #[test]
    fn add_next_occurrence_selects_word_then_matches_and_wraps() {
        let b = Buffer::new("foo bar foo baz foo");
        let first = b.add_next_occurrence(&caret(1));
        assert_eq!((first.primary().start(), first.primary().end()), (0, 3));
        let second = b.add_next_occurrence(&first);
        let third = b.add_next_occurrence(&second);
        let ranges: Vec<(usize, usize)> = third.as_slice().iter().map(|s| (s.start(), s.end())).collect();
        assert_eq!(ranges, vec![(0, 3), (8, 11), (16, 19)]);
        // All taken: nothing more to add.
        assert_eq!(b.add_next_occurrence(&third), third);
        assert_eq!(b.select_all_occurrences(&caret(9)).as_slice().len(), 3);
    }

    #[test]
    fn delete_lines_including_the_last_line() {
        let mut b = Buffer::new("a\nb\nc");
        b.delete_lines(&caret(2));
        assert_eq!(b.text(), "a\nc");
        b.delete_lines(&caret(2));
        assert_eq!(b.text(), "a");
        b.delete_lines(&caret(0));
        assert_eq!(b.text(), "");
        b.undo().unwrap();
        assert_eq!(b.text(), "a");
    }

    #[test]
    fn move_lines_swaps_and_keeps_the_selection_on_the_moved_text() {
        let mut b = Buffer::new("one\ntwo\nthree");
        let after = b.move_lines(&caret(5), true);
        assert_eq!(b.text(), "two\none\nthree");
        assert_eq!(b.offset_to_point(after.primary().head), (0, 1));
        let after = b.move_lines(&after, false);
        assert_eq!(b.text(), "one\ntwo\nthree");
        assert_eq!(b.offset_to_point(after.primary().head), (1, 1));
        // Already at the edge: unchanged.
        let top = b.move_lines(&caret(0), true);
        assert_eq!(b.text(), "one\ntwo\nthree");
        assert_eq!(top, caret(0));
        b.move_lines(&caret(9), false);
        assert_eq!(b.text(), "one\ntwo\nthree");
    }

    #[test]
    fn move_block_of_lines_down() {
        let mut b = Buffer::new("a\nb\nc\nd");
        let after = b.move_lines(&sel(0, 3), false); // lines a and b
        assert_eq!(b.text(), "c\na\nb\nd");
        assert_eq!(b.offset_to_point(after.primary().head), (2, 1));
    }

    #[test]
    fn copy_lines_up_and_down() {
        let mut b = Buffer::new("a\nb");
        let after = b.copy_lines(&caret(0), false);
        assert_eq!(b.text(), "a\na\nb");
        assert_eq!(b.offset_to_point(after.primary().head).0, 1, "cursor lands on the new copy below");
        let mut b = Buffer::new("a\nb");
        let after = b.copy_lines(&caret(2), true);
        assert_eq!(b.text(), "a\nb\nb");
        assert_eq!(b.offset_to_point(after.primary().head).0, 1);
    }

    #[test]
    fn insert_line_above_and_below_keeps_indent() {
        let mut b = Buffer::new("  x\ny");
        let after = b.insert_line(&caret(3), false);
        assert_eq!(b.text(), "  x\n  \ny");
        assert_eq!(b.offset_to_point(after.primary().head), (1, 2));
        let mut b = Buffer::new("  x");
        let after = b.insert_line(&caret(1), true);
        assert_eq!(b.text(), "  \n  x");
        assert_eq!(b.offset_to_point(after.primary().head), (0, 2));
    }

    #[test]
    fn join_lines_collapses_the_gap() {
        let mut b = Buffer::new("a\n   b\nc");
        let after = b.join_lines(&caret(0));
        assert_eq!(b.text(), "a b\nc");
        assert_eq!(after.primary().head, 1);
        let mut b = Buffer::new("a\nb\nc\nd");
        b.join_lines(&sel(0, 5));
        assert_eq!(b.text(), "a b c\nd");
    }

    #[test]
    fn indent_and_outdent_lines() {
        let style = IndentStyle::default();
        let mut b = Buffer::new("a\n\nb");
        let after = b.indent_lines(&sel(0, 4), style);
        assert_eq!(b.text(), "    a\n\n    b");
        assert_eq!((after.primary().start(), after.primary().end()), (4, 12));
        b.outdent_lines(&after, style);
        assert_eq!(b.text(), "a\n\nb");
        let mut tabs = Buffer::new("\tx\n  y");
        tabs.outdent_lines(&sel(0, 6), style);
        assert_eq!(tabs.text(), "x\ny");
    }

    #[test]
    fn tab_inserts_to_the_next_stop_or_indents_a_multi_line_selection() {
        let style = IndentStyle::default();
        let mut b = Buffer::new("ab");
        let after = b.insert_tab(&caret(2), style);
        assert_eq!(b.text(), "ab  ");
        assert_eq!(after.primary().head, 4);
        let mut b = Buffer::new("a\nb");
        b.insert_tab(&sel(0, 3), style);
        assert_eq!(b.text(), "    a\n    b");
        let mut tab = Buffer::new("x");
        tab.insert_tab(&caret(0), IndentStyle { tab_size: 4, use_spaces: false });
        assert_eq!(tab.text(), "\tx");
    }

    #[test]
    fn line_comment_toggles_at_the_shallowest_indent_and_skips_blank_lines() {
        let mut b = Buffer::new("    a\n\n      b");
        b.toggle_line_comment(&sel(0, 12), "//");
        assert_eq!(b.text(), "    // a\n\n    //   b");
        b.toggle_line_comment(&sel(0, b.len_chars()), "//");
        assert_eq!(b.text(), "    a\n\n      b");
        // Mixed: comments everything rather than uncommenting.
        let mut mixed = Buffer::new("// a\nb");
        mixed.toggle_line_comment(&sel(0, 6), "//");
        assert_eq!(mixed.text(), "// // a\n// b");
    }

    #[test]
    fn block_comment_wraps_and_unwraps() {
        let mut b = Buffer::new("hello world");
        let after = b.toggle_block_comment(&sel(6, 11), "/*", "*/");
        assert_eq!(b.text(), "hello /* world */");
        b.toggle_block_comment(&after, "/*", "*/");
        assert_eq!(b.text(), "hello world");
        let mut line = Buffer::new("  code()");
        line.toggle_block_comment(&caret(3), "/*", "*/");
        assert_eq!(line.text(), "  /* code() */");
    }

    #[test]
    fn enter_keeps_indent_and_opens_bracket_pairs() {
        let style = IndentStyle::default();
        let mut b = Buffer::new("  fn f() {");
        let after = b.newline_with_indent(&caret(10), style, false);
        assert_eq!(b.text(), "  fn f() {\n      ");
        assert_eq!(b.offset_to_point(after.primary().head), (1, 6));

        let mut pair = Buffer::new("{}");
        let after = pair.newline_with_indent(&caret(1), style, false);
        assert_eq!(pair.text(), "{\n    \n}");
        assert_eq!(pair.offset_to_point(after.primary().head), (1, 4));

        let mut py = Buffer::new("if x:");
        py.newline_with_indent(&caret(5), style, true);
        assert_eq!(py.text(), "if x:\n    ");
        let mut plain = Buffer::new("  x");
        plain.newline_with_indent(&caret(1), style, false);
        assert_eq!(plain.text(), " \n   x");
    }

    #[test]
    fn typing_auto_closes_overtypes_and_surrounds() {
        let mut b = Buffer::new("");
        let after = b.type_text(&caret(0), "(", true);
        assert_eq!((b.text().as_str(), after.primary().head), ("()", 1));
        let after = b.type_text(&after, ")", true);
        assert_eq!((b.text().as_str(), after.primary().head), ("()", 2));

        // Before a word character: no auto-close.
        let mut w = Buffer::new("x");
        w.type_text(&caret(0), "(", true);
        assert_eq!(w.text(), "(x");

        // Quotes: not inside a word (apostrophes), but yes after a space.
        let mut q = Buffer::new("don");
        q.type_text(&caret(3), "'", true);
        assert_eq!(q.text(), "don'");
        let mut q2 = Buffer::new("a ");
        q2.type_text(&caret(2), "\"", true);
        assert_eq!(q2.text(), "a \"\"");

        // Surround a selection.
        let mut s = Buffer::new("word");
        let after = s.type_text(&sel(0, 4), "[", true);
        assert_eq!(s.text(), "[word]");
        assert_eq!((after.primary().start(), after.primary().end()), (1, 5));

        // Off: plain insert.
        let mut off = Buffer::new("");
        off.type_text(&caret(0), "(", false);
        assert_eq!(off.text(), "(");
    }

    #[test]
    fn backspace_removes_both_halves_of_an_empty_pair() {
        let mut b = Buffer::new("f()");
        let after = b.backspace_with_pairs(&caret(2));
        assert_eq!(b.text(), "f");
        assert_eq!(after.primary().head, 1);
        let mut plain = Buffer::new("ab");
        plain.backspace_with_pairs(&caret(2));
        assert_eq!(plain.text(), "a");
    }

    #[test]
    fn smart_backspace_deletes_to_the_previous_tab_stop_in_indentation() {
        let style = IndentStyle::default();
        let mut b = Buffer::new("      x");
        let after = b.backspace(&caret(6), style, true); // 6 spaces: back to column 4
        assert_eq!(b.text(), "    x");
        assert_eq!(after.primary().head, 4);
        b.backspace(&caret(4), style, true);
        assert_eq!(b.text(), "x", "a whole indent level");
        // Not in indentation: one char. With tabs-for-indent: one char too.
        let mut mid = Buffer::new("a    b");
        mid.backspace(&caret(5), style, true);
        assert_eq!(mid.text(), "a   b");
        let mut tabs = Buffer::new("		x");
        tabs.backspace(&caret(2), IndentStyle { tab_size: 4, use_spaces: false }, true);
        assert_eq!(tabs.text(), "	x");
        // Pairs: both halves go; with pairs off only one.
        let mut pair = Buffer::new("f()");
        pair.backspace(&caret(2), style, true);
        assert_eq!(pair.text(), "f");
        let mut no_pair = Buffer::new("f()");
        no_pair.backspace(&caret(2), style, false);
        assert_eq!(no_pair.text(), "f)");
    }

    #[test]
    fn multi_cursor_typing_and_undo() {
        let mut b = Buffer::new("a\nb");
        let after = b.type_text(&carets(&[0, 2]), "x", true);
        assert_eq!(b.text(), "xa\nxb");
        assert_eq!(heads(&after), vec![1, 4]);
        b.undo().unwrap();
        assert_eq!(b.text(), "a\nb");
    }

    #[test]
    fn transforms_sort_trim_case_and_transpose() {
        let mut b = Buffer::new("b\nc\na");
        b.sort_lines(&sel(0, 5), false);
        assert_eq!(b.text(), "a\nb\nc");
        b.sort_lines(&sel(0, 5), true);
        assert_eq!(b.text(), "c\nb\na");

        let mut t = Buffer::new("x  \ny\t\nz");
        t.trim_trailing_whitespace(&caret(0));
        assert_eq!(t.text(), "x\ny\nz");

        let mut u = Buffer::new("hello world");
        u.transform_selection(&caret(2), |s| s.to_uppercase());
        assert_eq!(u.text(), "HELLO world");
        u.transform_selection(&sel(6, 11), |s| s.to_uppercase());
        assert_eq!(u.text(), "HELLO WORLD");

        let mut tr = Buffer::new("abc");
        tr.transpose(&caret(1));
        assert_eq!(tr.text(), "bac");
        let mut end = Buffer::new("abc");
        end.transpose(&caret(3));
        assert_eq!(end.text(), "acb");
    }
}
