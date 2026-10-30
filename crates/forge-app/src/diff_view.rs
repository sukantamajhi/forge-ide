//! The side-by-side diff of a file from the Git view: removed lines on the left, added lines on
//! the right, with buttons to stage (or unstage) a whole hunk or just the lines you click.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use forge_core::git;
use forge_core::patch::{self, LineKind, Patch, SideRow};
use gpui::{div, prelude::*, px, rems, rgb, rgba, uniform_list, AnyElement, Context, FontWeight, SharedString};

use crate::workspace::{Doc, GitMsg, Workspace};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DiffState {
    Staged,
    WorkingTree,
    /// An untracked file: shown as all added, and staged as a whole file.
    New,
}

pub(crate) struct DiffTarget {
    pub rel: String,
    pub state: DiffState,
}

#[derive(Clone, Copy)]
pub(crate) enum DiffItem {
    Hunk(usize),
    Row(usize),
}

pub(crate) struct DiffCache {
    pub path: PathBuf,
    pub revision: u64,
    pub patch: Option<Patch>,
    pub rows: Vec<SideRow>,
    pub items: Vec<DiffItem>,
}

/// `git:diff:Staged:src/main.rs` and its siblings, the names diff tabs are opened under.
pub(crate) fn parse_target(path: &Path) -> Option<DiffTarget> {
    let text = path.to_string_lossy();
    let rest = text.strip_prefix("git:diff:")?;
    let (state, rel) = rest.split_once(':')?;
    let state = match state {
        "Staged" => DiffState::Staged,
        "Working Tree" => DiffState::WorkingTree,
        "New" => DiffState::New,
        _ => return None,
    };
    Some(DiffTarget { rel: rel.to_owned(), state })
}

impl Workspace {
    /// The active tab of group `g`, when it is a file diff that is drawn side by side.
    pub(crate) fn diff_target_in(&self, g: usize) -> Option<(usize, DiffTarget)> {
        let group = self.groups.get(g)?;
        let doc_ix = group.views.get(group.active)?.doc;
        let doc = &self.docs[doc_ix];
        if !doc.diff {
            return None;
        }
        Some((doc_ix, parse_target(&doc.path)?))
    }

    pub(crate) fn active_diff_target(&self) -> Option<(usize, DiffTarget)> {
        self.diff_target_in(self.focused_group)
    }

    fn ensure_diff_cache(&self, doc_ix: usize) {
        let doc = &self.docs[doc_ix];
        let mut cache = self.diff_cache.borrow_mut();
        if cache.as_ref().is_some_and(|c| c.path == doc.path && c.revision == doc.revision) {
            return;
        }
        let patch = patch::parse(&doc.buffer.text());
        let rows = patch.as_ref().map(Patch::side_by_side).unwrap_or_default();
        let mut items = Vec::with_capacity(rows.len() + 8);
        let mut last_hunk = None;
        for (i, row) in rows.iter().enumerate() {
            if last_hunk != Some(row.hunk) {
                items.push(DiffItem::Hunk(row.hunk));
                last_hunk = Some(row.hunk);
            }
            items.push(DiffItem::Row(i));
        }
        *cache = Some(DiffCache { path: doc.path.clone(), revision: doc.revision, patch, rows, items });
    }

    pub fn toggle_diff_layout(&mut self) {
        let next = if self.settings.diff_side_by_side { "false" } else { "true" };
        self.set_setting("diffEditor.renderSideBySide", next);
        self.set_status(if self.settings.diff_side_by_side { "Diffs are shown side by side" } else { "Diffs are shown inline" });
    }

    fn diff_toggle_line(&mut self, hunk: usize, line: usize) {
        if !self.diff_selected.remove(&(hunk, line)) {
            self.diff_selected.insert((hunk, line));
        }
    }

    /// Stage (or, in a staged diff, unstage) hunk `hunk`, or only the lines chosen in it.
    pub fn diff_apply(&mut self, hunk: usize, only_selected: bool) {
        let Some((_, target)) = self.active_diff_target() else { return };
        let patch = self.diff_cache.borrow().as_ref().and_then(|c| c.patch.clone());
        let Some(patch) = patch else { return };
        if target.state == DiffState::New {
            self.stage_new_file_lines(&target.rel, &patch, only_selected);
            return;
        }
        let staged = target.state == DiffState::Staged;
        let text = if only_selected {
            let lines: BTreeSet<usize> = self.diff_selected.iter().filter(|(h, _)| *h == hunk).map(|(_, l)| *l).collect();
            patch.lines_patch(hunk, &lines, staged)
        } else {
            patch.hunk_patch(hunk)
        };
        let Some(text) = text else {
            self.set_status("Click some changed lines first");
            return;
        };
        let what = match (staged, only_selected) {
            (false, false) => "Staged the hunk",
            (false, true) => "Staged the selected lines",
            (true, false) => "Unstaged the hunk",
            (true, true) => "Unstaged the selected lines",
        };
        self.git_job("hunk", move |root| git::apply_patch(root, &text, true, staged).map(|_| what.to_owned()));
    }

    /// Stage some (or all) lines of an untracked file: only they go into the index; the file is untouched.
    fn stage_new_file_lines(&mut self, rel: &str, patch: &Patch, only_selected: bool) {
        let lines: BTreeSet<usize> = if only_selected {
            self.diff_selected.iter().filter(|(h, _)| *h == 0).map(|(_, l)| *l).collect()
        } else {
            patch.hunks.first().map(|h| h.change_lines()).unwrap_or_default()
        };
        let crlf = self.git.as_ref().and_then(|g| std::fs::read(g.root.join(rel)).ok()).is_some_and(|b| b.windows(2).any(|w| w == b"\r\n"));
        let Some(content) = patch.new_file_text(&lines, crlf) else {
            self.set_status("Click some lines first");
            return;
        };
        let (rel, what) = (rel.to_owned(), if only_selected { "Staged the selected lines" } else { "Staged the file" });
        self.git_job("hunk", move |root| git::stage_new_file(root, &rel, &content).map(|_| what.to_owned()));
    }

    /// Ask before throwing away hunk `hunk`, or the lines chosen in it, from the working file.
    pub fn diff_discard(&mut self, hunk: usize, only_selected: bool) {
        let Some((_, target)) = self.active_diff_target() else { return };
        if target.state != DiffState::WorkingTree {
            return;
        }
        let patch = self.diff_cache.borrow().as_ref().and_then(|c| c.patch.clone());
        let Some(patch) = patch else { return };
        let text = if only_selected {
            let lines: BTreeSet<usize> = self.diff_selected.iter().filter(|(h, _)| *h == hunk).map(|(_, l)| *l).collect();
            patch.lines_patch(hunk, &lines, true)
        } else {
            patch.hunk_patch(hunk)
        };
        let Some(patch_text) = text else {
            self.set_status("Click some changed lines first");
            return;
        };
        let what = if only_selected { "the selected lines" } else { "this hunk" }.to_owned();
        self.open_overlay(crate::workspace::OverlayKind::ConfirmDiscardHunk { rel: target.rel, patch: patch_text, what }, "");
    }

    /// Close every tab showing `path` (a generated document, so nothing is lost).
    fn close_tabs_of(&mut self, path: &Path, cx: &mut Context<Self>) {
        for _ in 0..8 {
            let found = self.groups.iter().enumerate().find_map(|(g, gr)| gr.views.iter().position(|v| self.docs[v.doc].path == path).map(|vi| (g, vi)));
            match found {
                Some((g, vi)) => self.close_view(g, vi, cx),
                None => break,
            }
        }
    }

    /// After staging or unstaging, bring every open diff up to date.
    pub fn refresh_open_diffs(&mut self, cx: &mut Context<Self>) {
        let open: Vec<(String, String, DiffTarget)> = self
            .docs
            .iter()
            .filter(|d| d.diff)
            .filter_map(|d| Some((d.path.to_string_lossy().trim_start_matches("git:").to_owned(), d.name(), parse_target(&d.path)?)))
            .collect();
        for (key, title, target) in open {
            // A new file that has just had lines staged is tracked now: show what is left as an ordinary change.
            if target.state == DiffState::New {
                if let Some(root) = self.git.as_ref().map(|g| g.root.clone()) {
                    if git::is_tracked(&root, &target.rel) {
                        self.close_tabs_of(&PathBuf::from(format!("git:{key}")), cx);
                        self.git_open_diff(target.rel.clone(), false, false);
                        continue;
                    }
                }
            }
            let (staged, untracked) = (target.state == DiffState::Staged, target.state == DiffState::New);
            let rel = target.rel;
            self.git_background(move |root| git::diff_text(root, &rel, staged, untracked), move |result| GitMsg::DiffRefresh { key, title, result });
        }
    }

    /// Put fresh text in an open diff tab without switching to it.
    pub fn replace_virtual(&mut self, key: &str, title: &str, text: &str) {
        let path = PathBuf::from(format!("git:{key}"));
        let Some(ix) = self.doc_index(&path) else { return };
        self.docs[ix] = Doc::new_virtual(path, title, text, true);
        *self.diff_cache.borrow_mut() = None;
        self.diff_selected.clear();
    }

    pub(crate) fn render_diff_view(&self, doc_ix: usize, target: &DiffTarget, cx: &mut Context<Self>) -> AnyElement {
        self.ensure_diff_cache(doc_ix);
        let t = self.theme.clone();
        let stageable = true;
        let (count, has_patch) = {
            let cache = self.diff_cache.borrow();
            let cache = cache.as_ref();
            (cache.map_or(0, |c| c.items.len()), cache.is_some_and(|c| c.patch.is_some()))
        };
        let state = match target.state {
            DiffState::Staged => "Staged Changes",
            DiffState::WorkingTree => "Changes",
            DiffState::New => "New file",
        };
        let hint = match target.state {
            DiffState::Staged => "Click lines to pick them, then Unstage Selected. Unstage Hunk takes the whole block.",
            DiffState::WorkingTree => "Click lines to pick them, then Stage Selected. Stage Hunk takes the whole block.",
            DiffState::New => "Click lines to pick them, then Stage Selected, or Stage All for the whole file.",
        };
        let toolbar = div()
            .flex_none()
            .h(rems(2.0))
            .px_3()
            .flex()
            .flex_row()
            .items_center()
            .gap_3()
            .bg(rgb(t.sidebar))
            .border_b_1()
            .border_color(rgb(t.border))
            .child(div().font_weight(FontWeight::MEDIUM).child(target.rel.clone()))
            .child(div().text_xs().text_color(rgb(t.dim)).child(state))
            .child(div().flex_1().min_w_0().overflow_hidden().whitespace_nowrap().text_xs().text_color(rgb(t.dim)).child(hint))
            .child(
                div()
                    .id("diff-inline")
                    .flex_none()
                    .px_2()
                    .h(rems(1.5))
                    .flex()
                    .items_center()
                    .rounded_sm()
                    .cursor_pointer()
                    .text_xs()
                    .bg(rgb(t.tab_active))
                    .hover(|s| s.bg(rgb(t.border)))
                    .tooltip(self.tooltip("Show the diff as one column", Some(forge_core::commands::Command::ToggleDiffLayout)))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.toggle_diff_layout();
                        cx.notify();
                    }))
                    .child("Inline"),
            );
        let body: AnyElement = if count == 0 || !has_patch {
            div().p_4().text_color(rgb(t.dim)).child(self.docs[doc_ix].buffer.text().lines().next().unwrap_or("No changes.").to_owned()).into_any_element()
        } else {
            let line_h = self.line_height();
            let state = target.state;
            uniform_list(
                "diff-rows",
                count,
                cx.processor(move |this, range: std::ops::Range<usize>, _window, cx| {
                    range.map(|ix| this.diff_item(ix, state, stageable, line_h, cx)).collect()
                }),
            )
            .track_scroll(self.diff_scroll.clone())
            .size_full()
            .into_any_element()
        };
        div().size_full().flex().flex_col().child(toolbar).child(div().flex_1().min_h_0().child(body)).into_any_element()
    }

    fn diff_item(&self, ix: usize, state: DiffState, stageable: bool, line_h: f32, cx: &mut Context<Self>) -> AnyElement {
        let t = self.theme.clone();
        let cache = self.diff_cache.borrow();
        let Some(cache) = cache.as_ref() else { return div().into_any_element() };
        let row_box = || div().w_full().h(px(line_h)).flex().flex_row().items_center();
        match cache.items.get(ix).copied() {
            Some(DiffItem::Hunk(h)) => {
                let header = cache.patch.as_ref().and_then(|p| p.hunks.get(h)).map(|hunk| hunk.header.clone()).unwrap_or_default();
                let selected = self.diff_selected.iter().filter(|(sh, _)| *sh == h).count();
                let staged = state == DiffState::Staged;
                let chip = |id: SharedString, label: String, primary: bool, only: bool, discard: bool, cx: &mut Context<Self>| {
                    div()
                        .id(id)
                        .flex_none()
                        .px_2()
                        .h(px(line_h - 4.0))
                        .flex()
                        .items_center()
                        .rounded_sm()
                        .cursor_pointer()
                        .text_xs()
                        .bg(rgb(if primary { t.button } else { t.tab_active }))
                        .text_color(rgb(if primary { t.button_text } else { t.text }))
                        .hover(|s| s.opacity(0.85))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if discard {
                                this.diff_discard(h, only);
                            } else {
                                this.diff_apply(h, only);
                            }
                            cx.notify();
                        }))
                        .child(label)
                };
                row_box()
                    .bg(rgb(t.sidebar))
                    .px_2()
                    .gap_2()
                    .child(div().flex_1().min_w_0().overflow_hidden().whitespace_nowrap().text_xs().text_color(rgb(t.dim)).child(header))
                    // Throwing changes away: only from the working tree, and always asked about first.
                    .when(state == DiffState::WorkingTree && selected > 0, |d| d.child(chip(format!("diff-discard-lines-{h}").into(), format!("Discard Selected ({selected})"), false, true, true, cx)))
                    .when(state == DiffState::WorkingTree, |d| d.child(chip(format!("diff-discard-{h}").into(), "Discard Hunk".to_owned(), false, false, true, cx)))
                    .when(stageable && selected > 0, |d| {
                        d.child(chip(
                            format!("diff-lines-{h}").into(),
                            format!("{} Selected ({selected})", if staged { "Unstage" } else { "Stage" }),
                            true,
                            true,
                            false,
                            cx,
                        ))
                    })
                    .when(stageable, |d| {
                        let label = if state == DiffState::New { "Stage All".to_owned() } else { format!("{} Hunk", if staged { "Unstage" } else { "Stage" }) };
                        d.child(chip(format!("diff-hunk-{h}").into(), label, selected == 0, false, false, cx))
                    })
                    .into_any_element()
            }
            Some(DiffItem::Row(r)) => {
                let Some(row) = cache.rows.get(r) else { return div().into_any_element() };
                let hunk = row.hunk;
                let cell = |side: usize, cell: &Option<patch::Cell>, cx: &mut Context<Self>| {
                    let base = div().id(("diff-cell", r * 2 + side)).flex_1().min_w_0().h_full().flex().flex_row().items_center().overflow_hidden();
                    let Some(c) = cell else { return base.bg(rgba((t.sidebar << 8) | 0x80)) };
                    let (tint, solid) = match c.kind {
                        LineKind::Del => (Some(t.git_deleted), 0x2e),
                        LineKind::Add => (Some(t.git_added), 0x2e),
                        LineKind::Context => (None, 0),
                    };
                    let chosen = self.diff_selected.contains(&(hunk, c.line));
                    let line = c.line;
                    base.when_some(tint, |d, color| d.bg(rgba((color << 8) | if chosen { 0x80 } else { solid })))
                        .when(chosen, |d| d.border_l_2().border_color(rgb(t.info)))
                        .when(stageable && c.kind != LineKind::Context, |d| {
                            d.cursor_pointer().on_click(cx.listener(move |this, _, _, cx| {
                                this.diff_toggle_line(hunk, line);
                                cx.notify();
                            }))
                        })
                        .child(div().flex_none().w(rems(3.25)).pr_2().flex().justify_end().text_xs().text_color(rgb(t.dim)).child(c.number.to_string()))
                        .child(div().flex_1().min_w_0().overflow_hidden().whitespace_nowrap().child(c.text.replace('\t', "    ").replace('\r', "")))
                };
                row_box().child(cell(0, &row.left, cx)).child(div().flex_none().w(px(1.0)).h_full().bg(rgb(t.border))).child(cell(1, &row.right, cx)).into_any_element()
            }
            None => div().into_any_element(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn diff_tabs_name_their_file_and_state() {
        let t = parse_target(Path::new("git:diff:Staged:src/main.rs")).unwrap();
        assert_eq!((t.rel.as_str(), t.state), ("src/main.rs", DiffState::Staged));
        let t = parse_target(Path::new("git:diff:Working Tree:a b/c:d.txt")).unwrap();
        assert_eq!((t.rel.as_str(), t.state), ("a b/c:d.txt", DiffState::WorkingTree));
        assert_eq!(parse_target(Path::new("git:diff:New:x")).unwrap().state, DiffState::New);
        assert!(parse_target(Path::new("git:commit:abc")).is_none());
        assert!(parse_target(Path::new("src/main.rs")).is_none());
    }
}
