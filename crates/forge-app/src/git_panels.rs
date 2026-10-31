//! Two bottom-panel views over git history: the **commit graph** of every branch (lanes and dots,
//! like `git log --graph`), and the **timeline** of the file in the editor (its commits).

use std::ops::Range;

use forge_core::git::{self, Commit, GraphCommit};
use forge_core::graph::{self, GraphRow};
use gpui::{div, prelude::*, px, rgb, uniform_list, AnyElement, Context, Div, FontWeight, MouseButton, SharedString};

use crate::workspace::{GitMsg, PanelView, Workspace};

/// Width of one lane column, and the most lanes drawn (further ones are cut off).
const LANE_W: f32 = 14.0;
const MAX_LANES: usize = 12;

impl Workspace {
    pub fn open_graph_panel(&mut self) {
        if self.git.is_none() {
            self.set_status("This folder is not a git repository");
            return;
        }
        self.panel_visible = true;
        self.panel_view = PanelView::Graph;
        self.request_graph();
    }

    pub fn request_graph(&mut self) {
        if self.graph_pending || self.git.is_none() {
            return;
        }
        self.graph_pending = true;
        self.git_background(|root| git::graph_log(root, 400), GitMsg::Graph);
    }

    pub(crate) fn set_graph(&mut self, result: Result<Vec<GraphCommit>, String>) {
        self.graph_pending = false;
        match result {
            Ok(list) => {
                self.graph_rows = graph::layout(&list);
                self.graph = list;
            }
            Err(e) => self.set_status(format!("Git: {e}")),
        }
    }

    pub fn open_timeline_panel(&mut self) {
        if self.git.is_none() {
            self.set_status("This folder is not a git repository");
            return;
        }
        self.panel_visible = true;
        self.panel_view = PanelView::Timeline;
        self.request_timeline();
    }

    pub fn request_timeline(&mut self) {
        let Some(rel) = self.active_rel() else {
            self.set_status("Open a file in the repository to see its timeline");
            return;
        };
        if self.timeline_pending {
            return;
        }
        self.timeline_pending = true;
        let wanted = rel.clone();
        self.git_background(move |root| git::log(root, Some(&rel), 200), move |result| GitMsg::Timeline { rel: wanted, result });
    }

    pub(crate) fn set_timeline(&mut self, rel: String, result: Result<Vec<Commit>, String>) {
        self.timeline_pending = false;
        match result {
            Ok(list) => self.timeline = Some((rel, list)),
            Err(e) => self.set_status(format!("Git: {e}")),
        }
    }

    /// After an operation changed history, show the new state; and let the timeline follow the active file.
    pub fn refresh_git_panels(&mut self) {
        if self.panel_visible && self.panel_view == PanelView::Graph {
            self.request_graph();
        }
        if self.panel_visible && self.panel_view == PanelView::Timeline {
            self.timeline = None;
        }
    }

    /// Called every tick: the timeline shows whichever file is in front.
    pub fn follow_git_panels(&mut self) {
        if !(self.panel_visible && self.panel_view == PanelView::Timeline) || self.timeline_pending {
            return;
        }
        let wanted = self.active_rel();
        if wanted.is_some() && wanted.as_deref() != self.timeline.as_ref().map(|(rel, _)| rel.as_str()) {
            self.request_timeline();
        }
    }

    /// Open a commit (or, with `rel`, what it did to one file) in a diff tab.
    pub fn git_show_hash(&mut self, hash: String, subject: String, rel: Option<String>) {
        let short: String = hash.chars().take(8).collect();
        let key = format!("commit:{short}:{}", rel.clone().unwrap_or_default());
        let title = format!("{short} {}", subject.chars().take(40).collect::<String>());
        self.git_background(move |root| git::show_commit(root, &hash, rel.as_deref()), move |result| GitMsg::Diff { key, title, result });
    }

    fn lane_color(&self, color: usize) -> u32 {
        let t = &self.theme;
        [t.info, t.git_added, t.warning, t.accent, t.error, t.git_modified, t.text, t.dim][color % 8]
    }

    /// The lane lines and dot of one graph row, drawn with thin boxes.
    fn graph_cell(&self, row: &GraphRow, height: f32, lanes: usize) -> Div {
        let mid = height / 2.0;
        let x = |lane: usize| lane as f32 * LANE_W + LANE_W / 2.0;
        let line = |left: f32, top: f32, w: f32, h: f32, color: usize| div().absolute().left(px(left)).top(px(top)).w(px(w)).h(px(h)).bg(rgb(self.lane_color(color)));
        let mut cell = div().relative().flex_none().w(px(lanes as f32 * LANE_W + 6.0)).h(px(height));
        for lane in 0..lanes.min(row.top.len().max(row.bottom.len())) {
            if let Some(Some(color)) = row.top.get(lane) {
                cell = cell.child(line(x(lane) - 1.0, 0.0, 2.0, mid + 1.0, *color));
            }
            if let Some(Some(color)) = row.bottom.get(lane) {
                cell = cell.child(line(x(lane) - 1.0, mid, 2.0, height - mid, *color));
            }
        }
        // Lines that join or leave at this row run sideways at the middle.
        let sideways = |from: usize, color: usize| {
            let (a, b) = (from.min(row.node), from.max(row.node));
            line(x(a), mid - 1.0, x(b) - x(a), 2.0, color)
        };
        for &lane in row.converge.iter().filter(|l| **l < lanes) {
            let color = row.top.get(lane).copied().flatten().unwrap_or(row.node_color);
            cell = cell.child(sideways(lane, color));
        }
        for &lane in row.diverge.iter().filter(|l| **l < lanes) {
            let color = row.bottom.get(lane).copied().flatten().unwrap_or(row.node_color);
            cell = cell.child(sideways(lane, color));
        }
        if row.node < lanes {
            let t = &self.theme;
            cell = cell.child(div().absolute().left(px(x(row.node) - 4.5)).top(px(mid - 4.5)).w(px(9.0)).h(px(9.0)).rounded_full().bg(rgb(self.lane_color(row.node_color))).border_1().border_color(rgb(t.bg)));
        }
        cell
    }

    fn ref_chip(&self, name: &str) -> Div {
        let t = &self.theme;
        let (label, color) = if let Some(tag) = name.strip_prefix("tag: ") {
            (tag.to_owned(), t.warning)
        } else if let Some(branch) = name.strip_prefix("HEAD -> ") {
            (branch.to_owned(), t.info)
        } else if name.contains('/') {
            (name.to_owned(), t.dim)
        } else {
            (name.to_owned(), t.git_added)
        };
        div().flex_none().px_1().rounded_sm().border_1().border_color(rgb(color)).text_color(rgb(color)).text_xs().child(label)
    }

    pub(crate) fn render_graph(&self, cx: &mut Context<Self>) -> AnyElement {
        let t = &self.theme;
        if self.git.is_none() {
            return div().p_3().text_color(rgb(t.dim)).child("This folder is not a git repository.").into_any_element();
        }
        if self.graph.is_empty() {
            let message = if self.graph_pending { "Loading the commit graph..." } else { "No commits yet." };
            return div().p_3().text_color(rgb(t.dim)).child(message).into_any_element();
        }
        let lanes = self.graph_rows.iter().map(GraphRow::width).max().unwrap_or(1).min(MAX_LANES);
        let count = self.graph.len();
        let height = self.line_height();
        uniform_list(
            "git-graph",
            count,
            cx.processor(move |this, range: Range<usize>, _window, cx| {
                let t = this.theme.clone();
                range
                    .filter_map(|i| Some((i, this.graph.get(i)?.clone(), this.graph_rows.get(i)?.clone())))
                    .map(|(i, commit, row)| {
                        let (hash, subject) = (commit.hash.clone(), commit.subject.clone());
                        div()
                            .id(("graph-row", i))
                            .w_full()
                            .h(px(height))
                            .px_2()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap_2()
                            .cursor_pointer()
                            .hover(|s| s.bg(rgb(t.tab_active)))
                            .on_mouse_down(MouseButton::Right, {
                                let hash = hash.clone();
                                cx.listener(move |this, ev: &gpui::MouseDownEvent, _, cx| {
                                    cx.stop_propagation();
                                    this.open_graph_menu(ev.position, hash.clone(), cx);
                                })
                            })
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.git_show_hash(hash.clone(), subject.clone(), None);
                                cx.notify();
                            }))
                            .child(this.graph_cell(&row, height, lanes))
                            .children(commit.refs.iter().take(4).map(|r| this.ref_chip(r)))
                            .child(div().flex_1().min_w_0().overflow_hidden().whitespace_nowrap().text_ellipsis().child(commit.subject.clone()))
                            .child(div().flex_none().text_xs().text_color(rgb(t.dim)).child(commit.author.clone()))
                            .child(div().flex_none().w(px(110.0)).text_xs().text_color(rgb(t.dim)).child(commit.when.clone()))
                            .child(div().flex_none().w(px(64.0)).text_xs().text_color(rgb(t.dim)).child(commit.hash.chars().take(8).collect::<String>()))
                    })
                    .collect()
            }),
        )
        .track_scroll(self.graph_scroll.clone())
        .size_full()
        .into_any_element()
    }

    pub(crate) fn render_timeline(&self, cx: &mut Context<Self>) -> AnyElement {
        let t = &self.theme;
        if self.git.is_none() {
            return div().p_3().text_color(rgb(t.dim)).child("This folder is not a git repository.").into_any_element();
        }
        let Some((rel, commits)) = self.timeline.as_ref() else {
            let message = if self.timeline_pending { "Loading the history of this file..." } else { "Open a file to see its history here." };
            return div().p_3().text_color(rgb(t.dim)).child(message).into_any_element();
        };
        if commits.is_empty() {
            return div().p_3().text_color(rgb(t.dim)).child(format!("{rel} has no commits yet.")).into_any_element();
        }
        let (rel, count, height) = (rel.clone(), commits.len(), self.line_height());
        let header = div().px_3().py_1().text_xs().text_color(rgb(t.dim)).child(format!("Timeline: {rel} ({count} commit{})", if count == 1 { "" } else { "s" }));
        let list = uniform_list(
            "git-timeline",
            count,
            cx.processor(move |this, range: Range<usize>, _window, cx| {
                let t = this.theme.clone();
                let Some((rel, commits)) = this.timeline.clone() else { return Vec::new() };
                range
                    .filter_map(|i| commits.get(i).cloned().map(|c| (i, c)))
                    .map(|(i, commit)| {
                        let (hash, subject, rel) = (commit.hash.clone(), commit.subject.clone(), rel.clone());
                        div()
                            .id(SharedString::from(format!("timeline-row-{i}")))
                            .w_full()
                            .h(px(height))
                            .px_3()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap_3()
                            .cursor_pointer()
                            .hover(|s| s.bg(rgb(t.tab_active)))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.git_show_hash(hash.clone(), subject.clone(), Some(rel.clone()));
                                cx.notify();
                            }))
                            .child(div().flex_1().min_w_0().overflow_hidden().whitespace_nowrap().text_ellipsis().font_weight(FontWeight::MEDIUM).child(commit.subject.clone()))
                            .child(div().flex_none().text_xs().text_color(rgb(t.dim)).child(commit.author.clone()))
                            .child(div().flex_none().w(px(110.0)).text_xs().text_color(rgb(t.dim)).child(commit.when.clone()))
                            .child(div().flex_none().w(px(64.0)).text_xs().text_color(rgb(t.dim)).child(commit.hash.chars().take(8).collect::<String>()))
                    })
                    .collect()
            }),
        )
        .track_scroll(self.timeline_scroll.clone())
        .size_full();
        div().size_full().flex().flex_col().child(header).child(div().flex_1().min_h_0().child(list)).into_any_element()
    }
}
