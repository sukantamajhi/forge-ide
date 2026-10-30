//! Keyboard and mouse input, and the widgets that take it: the Quick Open / palette
//! overlays, the Find widget, project search, the explorer and the terminal.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, TryRecvError};
use std::sync::Arc;
use std::time::Instant;

use forge_core::commands::{chord, Command, Resolution};
use forge_core::fileops;
use forge_core::git;
use forge_core::fuzzy::rank;
use forge_core::input::{LineInput, Outcome};
use forge_core::project_search::{self, Hit};
use forge_core::quick::{file_entries, palette_entries, parse_go_to_line};
use forge_core::terminal::{key_bytes, Terminal};
use forge_text::columns::col_at_cell;
use forge_text::search::Finder;
use forge_text::words::word_range_at;
use forge_text::{Selection, SelectionSet};
use gpui::{Context, KeyDownEvent, Keystroke, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, Pixels, Point};

use crate::completion;
use crate::workspace::{
    Focus, Item, ItemAction, Overlay, OverlayKind, PanelView, SidebarView, Workspace, GLYPH_WIDTH, GUTTER_WIDTH,
    LINE_NUMBER_WIDTH, TEXT_PAD,
};

pub(crate) enum SearchMsg {
    Hits(Vec<Hit>),
    Done { capped: bool },
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum DragMode {
    Char,
    Word,
    Line,
}

/// A mouse drag in progress: the selection grows from `anchor` (a range, for word/line).
#[derive(Clone, Copy)]
pub(crate) struct Drag {
    pub group: usize,
    pub anchor: (usize, usize),
    pub mode: DragMode,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Gutter {
    Glyph,
    LineNumber,
    Fold,
}

/// Commands that still work while the terminal has the keyboard.
fn terminal_passthrough(command: Command) -> bool {
    use Command::*;
    matches!(
        command,
        ToggleTerminal | NewTerminal | TogglePanel | CommandPalette | QuickOpen | ToggleSidebar | NextTab | PreviousTab
            | FocusFirstGroup | FocusSecondGroup | FindInFiles | ShowProblems | FocusExplorer | OpenKeybindings | ChangeTheme
            | ZoomIn | ZoomOut
    )
}

impl Workspace {
    // ---- keyboard ----

    pub fn on_key_down(&mut self, event: &KeyDownEvent, cx: &mut Context<Self>) {
        let k = &event.keystroke;
        let m = k.modifiers;
        self.mouse_moved = Instant::now();
        self.hover = None;
        self.hover_request = None;
        let pressed = chord(m.control, m.shift, m.alt, m.platform, &k.key);

        // Any key dismisses a context menu; Escape is only that.
        if self.context_menu.take().is_some() {
            cx.notify();
            if k.key == "escape" {
                return;
            }
        }
        if self.overlay.is_some() && self.overlay_key(k, &pressed, cx) {
            cx.notify();
            return;
        }
        if self.rebase_page.is_some() && self.overlay.is_none() && self.rebase_key(k) {
            cx.notify();
            return;
        }
        if self.conflict_page.is_some() && self.overlay.is_none() && k.key == "escape" {
            self.conflict_page = None;
            cx.notify();
            return;
        }
        if self.image_page.is_some() && self.overlay.is_none() && k.key == "escape" {
            self.image_page = None;
            cx.notify();
            return;
        }
        if self.settings_open && self.overlay.is_none() && self.settings_key(k) {
            cx.notify();
            return;
        }
        let handled = match self.focus {
            Focus::Find => self.find_key(k, &pressed, cx),
            Focus::Search => self.search_key(k, &pressed, cx),
            Focus::Scm => self.scm_key(k, &pressed, cx),
            Focus::Extensions => self.ext_key(k, &pressed, cx),
            Focus::Terminal => self.terminal_key(k, &pressed, cx),
            Focus::Explorer => self.explorer_key(k, &pressed, cx),
            Focus::Editor => self.editor_popup_key(k, cx),
        };
        if handled {
            cx.notify();
            return;
        }

        match self.keymap.resolve(&self.pending_chord, &pressed) {
            Resolution::Run { command, extend } => {
                // A finished chord no longer needs its "waiting for second key" message.
                if !std::mem::take(&mut self.pending_chord).is_empty() {
                    self.set_status("");
                }
                self.run_command(command, extend, cx);
            }
            Resolution::Pending(prefix) => {
                self.set_status(format!("({prefix}) was pressed. Waiting for the second key of the chord..."));
                self.pending_chord = prefix;
            }
            Resolution::NoMatch => {
                let had_prefix = !std::mem::take(&mut self.pending_chord).is_empty();
                if had_prefix {
                    self.set_status(format!("The key combination ({pressed}) is not a command"));
                } else if self.focus == Focus::Editor {
                    // AltGr arrives as Ctrl+Alt and produces real characters.
                    let produces_text = !m.platform && (!m.control || m.alt);
                    // Space (like Enter and Tab) is a named key with no character on Windows.
                    let typed = if k.key == "space" && !m.control && !m.alt { Some(" ") } else { k.key_char.as_deref() };
                    if let (Some(text), true) = (typed, produces_text) {
                        if !text.is_empty() && !text.chars().any(|c| c.is_control()) {
                            self.type_text(text, cx);
                        }
                    }
                }
            }
        }
        cx.notify();
    }

    /// Keys owned by the completion popup, signature help and Escape in the editor.
    fn editor_popup_key(&mut self, k: &Keystroke, cx: &mut Context<Self>) -> bool {
        let m = k.modifiers;
        let plain = !m.control && !m.platform && !m.alt;
        if let Some(state) = self.completion.as_mut() {
            match k.key.as_str() {
                "down" | "up" if plain => {
                    state.move_selection(if k.key == "up" { -1 } else { 1 });
                    return true;
                }
                "escape" => {
                    self.completion = None;
                    return true;
                }
                "enter" | "tab" if plain && !m.shift => {
                    self.accept_completion(cx);
                    return true;
                }
                _ => {}
            }
        }
        if k.key == "escape" && plain {
            if self.snippet_session.take().is_some() {
                return true;
            }
            if self.signature.take().is_some() || self.hover.take().is_some() {
                return true;
            }
            if self.find.open {
                self.find.open = false;
                return true;
            }
            if let Some((g, vi)) = self.active_ix() {
                let view = &mut self.groups[g].views[vi];
                let primary = view.selections.primary();
                // Escape collapses extra cursors first, then the selection.
                if view.selections.as_slice().len() > 1 {
                    view.selections = SelectionSet::single(primary);
                    return true;
                }
                if !primary.is_empty() {
                    view.selections = SelectionSet::single(Selection::caret(primary.head));
                    return true;
                }
            }
        }
        false
    }

    fn accept_completion(&mut self, cx: &mut Context<Self>) {
        let Some(state) = self.completion.take() else { return };
        let Some(item) = state.selected_item().cloned() else { return };
        self.mutate(cx, move |b, s| completion::accept(b, s, &item));
        self.last_typing = None;
    }

    // ---- mouse ----

    /// Which editor group, line and column a window position falls on.
    pub fn point_at(&self, pos: Point<Pixels>) -> Option<(usize, usize, usize)> {
        let line_h = self.line_height();
        for (g, group) in self.groups.iter().enumerate() {
            let Some(view) = group.views.get(group.active) else { continue };
            let (bounds, offset) = {
                let state = view.scroll.0.borrow();
                (state.base_handle.bounds(), state.base_handle.offset())
            };
            if !bounds.contains(&pos) {
                continue;
            }
            let doc = &self.docs[view.doc];
            let row = ((pos.y - bounds.top() - offset.y) / px_f(line_h)).floor().max(0.0) as usize;
            // The line under the pointer, and (when wrapped) which cells of it this row shows.
            let (line, start_cell, end_cell) = match &view.wrap {
                Some(map) => {
                    let info = map.row(row.min(map.row_count().saturating_sub(1)))?;
                    (info.line, info.start, info.end)
                }
                None => {
                    let line = match view.folds.visible_lines(doc.buffer.len_lines()) {
                        Some(visible) => *visible.get(row).or(visible.last())?,
                        None => row,
                    };
                    (line, 0, None)
                }
            };
            let line = line.min(doc.buffer.len_lines() - 1);
            let mut cell_x = (pos.x - bounds.left() - px_f(GUTTER_WIDTH + TEXT_PAD)) / self.char_width + start_cell as f32;
            if let Some(end) = end_cell {
                cell_x = cell_x.min(end as f32 - 0.51);
            }
            let col = if cell_x <= 0.0 { 0 } else { col_at_cell(&doc.buffer.line(line), cell_x) };
            return Some((g, line, col));
        }
        None
    }

    fn gutter_zone(&self, pos: Point<Pixels>, g: usize) -> Option<Gutter> {
        let view = self.groups.get(g)?.views.get(self.groups[g].active)?;
        let left = view.scroll.0.borrow().base_handle.bounds().left();
        let x = f32::from(pos.x - left);
        if (0.0..GLYPH_WIDTH).contains(&x) {
            Some(Gutter::Glyph)
        } else if (GLYPH_WIDTH..GLYPH_WIDTH + LINE_NUMBER_WIDTH).contains(&x) {
            Some(Gutter::LineNumber)
        } else if (GLYPH_WIDTH + LINE_NUMBER_WIDTH..GUTTER_WIDTH).contains(&x) {
            Some(Gutter::Fold)
        } else {
            None
        }
    }

    /// Right click in the editor text: place the caret (unless inside a selection) and open the menu.
    pub fn on_right_mouse_down(&mut self, ev: &MouseDownEvent, cx: &mut Context<Self>) {
        self.context_menu = None;
        if self.overlay.is_some() {
            return;
        }
        let Some((g, line, col)) = self.point_at(ev.position) else { return };
        if self.gutter_zone(ev.position, g).is_some() {
            return;
        }
        self.focused_group = g;
        self.focus = Focus::Editor;
        self.completion = None;
        self.hover = None;
        self.signature = None;
        let Some(vi) = self.groups.get(g).map(|gr| gr.active) else { return };
        let view = &mut self.groups[g].views[vi];
        let offset = self.docs[view.doc].buffer.point_to_offset(line, col);
        let inside = view.selections.as_slice().iter().any(|s| !s.is_empty() && s.start() <= offset && offset <= s.end());
        if !inside {
            view.selections = SelectionSet::single(Selection::caret(offset));
        }
        self.open_editor_menu(ev.position, cx);
    }

    pub fn on_mouse_down(&mut self, ev: &MouseDownEvent, cx: &mut Context<Self>) {
        // A click anywhere else only closes an open context menu.
        if self.context_menu.take().is_some() {
            cx.notify();
            return;
        }
        if ev.button != MouseButton::Left || self.overlay.is_some() {
            if self.overlay.is_some() {
                self.overlay = None;
                cx.notify();
            }
            return;
        }
        let Some((g, line, col)) = self.point_at(ev.position) else { return };
        self.focused_group = g;
        self.focus = Focus::Editor;
        self.completion = None;
        self.hover = None;
        self.signature = None;
        let Some(vi) = self.groups.get(g).map(|gr| gr.active) else { return };

        if let Some(zone) = self.gutter_zone(ev.position, g) {
            if zone == Gutter::Glyph {
                let path = self.docs[self.groups[g].views[vi].doc].path.clone();
                self.toggle_breakpoint(path, line);
                cx.notify();
                return;
            }
            self.ensure_folds(g, vi, true);
            let view = &mut self.groups[g].views[vi];
            let doc = &self.docs[view.doc];
            match zone {
                Gutter::Glyph => {}
                Gutter::Fold => {
                    view.folds.toggle_at(line);
                }
                Gutter::LineNumber => {
                    let start = doc.buffer.line_start(line);
                    let end = if line + 1 < doc.buffer.len_lines() { doc.buffer.line_start(line + 1) } else { doc.buffer.len_chars() };
                    let anchor = if ev.modifiers.shift { view.selections.primary().anchor } else { start };
                    view.selections = SelectionSet::single(Selection { anchor, head: end });
                    self.drag = Some(Drag { group: g, anchor: (start, end), mode: DragMode::Line });
                }
            }
            cx.notify();
            return;
        }

        let view = &mut self.groups[g].views[vi];
        let doc = &self.docs[view.doc];
        let offset = doc.buffer.point_to_offset(line, col);
        view.goal = None;
        view.expand.clear();
        let before = view.selections.clone();
        match ev.click_count {
            2 => {
                let (s, e) = word_range_at(doc.buffer.rope(), offset).unwrap_or((offset, offset));
                view.selections = SelectionSet::single(Selection { anchor: s, head: e });
                self.drag = Some(Drag { group: g, anchor: (s, e), mode: DragMode::Word });
            }
            n if n >= 3 => {
                let start = doc.buffer.line_start(line);
                let end = if line + 1 < doc.buffer.len_lines() { doc.buffer.line_start(line + 1) } else { doc.buffer.len_chars() };
                view.selections = SelectionSet::single(Selection { anchor: start, head: end });
                self.drag = Some(Drag { group: g, anchor: (start, end), mode: DragMode::Line });
            }
            _ => {
                if ev.modifiers.shift {
                    let anchor = view.selections.primary().anchor;
                    view.selections = SelectionSet::single(Selection { anchor, head: offset });
                } else if ev.modifiers.alt {
                    let mut all = view.selections.as_slice().to_vec();
                    all.push(Selection::caret(offset));
                    view.selections = SelectionSet::new(all);
                } else {
                    view.selections = SelectionSet::single(Selection::caret(offset));
                }
                self.drag = Some(Drag { group: g, anchor: (offset, offset), mode: DragMode::Char });
            }
        }
        if view.selections != before {
            view.history.push(before);
        }
        if ev.modifiers.control {
            eprintln!("[click] ctrl+click at line {line} col {col}, click_count {}", ev.click_count);
            if ev.click_count == 1 {
                self.run_command(Command::GoToDefinition, false, cx);
            } else {
                self.set_status("Ctrl+click needs a single click (it counted as a repeat click)");
            }
        }
        cx.notify();
    }

    pub fn on_mouse_move(&mut self, ev: &MouseMoveEvent, cx: &mut Context<Self>) {
        // A divider is being dragged: everything else waits.
        if self.resize.is_some() {
            if ev.pressed_button == Some(MouseButton::Left) {
                self.apply_resize(ev.position);
            } else {
                self.end_resize();
            }
            cx.notify();
            return;
        }
        let moved_far = self.mouse.is_none_or(|p| (p.x - ev.position.x).abs() > px_f(2.0) || (p.y - ev.position.y).abs() > px_f(2.0));
        self.mouse = Some(ev.position);
        if moved_far {
            self.mouse_moved = Instant::now();
            if self.hover.take().is_some() {
                cx.notify();
            }
            self.hover_request = None;
        }
        let Some(drag) = self.drag else { return };
        if ev.pressed_button != Some(MouseButton::Left) {
            self.drag = None;
            return;
        }
        let Some((g, line, col)) = self.point_at(ev.position) else { return };
        if g != drag.group {
            return;
        }
        let vi = self.groups[g].active;
        let view = &mut self.groups[g].views[vi];
        let doc = &self.docs[view.doc];
        let offset = doc.buffer.point_to_offset(line, col);
        let selection = match drag.mode {
            DragMode::Char => Selection { anchor: drag.anchor.0, head: offset },
            DragMode::Word => {
                let (ws, we) = word_range_at(doc.buffer.rope(), offset).unwrap_or((offset, offset));
                if offset < drag.anchor.0 { Selection { anchor: drag.anchor.1, head: ws } } else { Selection { anchor: drag.anchor.0, head: we } }
            }
            DragMode::Line => {
                let start = doc.buffer.line_start(line);
                let end = if line + 1 < doc.buffer.len_lines() { doc.buffer.line_start(line + 1) } else { doc.buffer.len_chars() };
                if start < drag.anchor.0 { Selection { anchor: drag.anchor.1, head: start } } else { Selection { anchor: drag.anchor.0, head: end } }
            }
        };
        if view.selections.as_slice().len() == 1 && view.selections.primary() != selection {
            view.selections = SelectionSet::single(selection);
            cx.notify();
        } else if view.selections.as_slice().len() > 1 {
            // Alt+drag keeps the cursors already placed and extends the newest one.
            let mut all = view.selections.as_slice().to_vec();
            if let Some(last) = all.last_mut() {
                *last = selection;
            }
            view.selections = SelectionSet::new(all);
            cx.notify();
        }
    }

    pub fn on_mouse_up(&mut self, _: &MouseUpEvent, _: &mut Context<Self>) {
        self.drag = None;
        self.scm_dragging = false;
        self.minimap_dragging = false;
        self.end_resize();
    }

    // ---- overlays ----

    pub fn open_overlay(&mut self, kind: OverlayKind, initial: &str) {
        let placeholder = match &kind {
            OverlayKind::Palette => "Type the name of a command to run",
            OverlayKind::QuickOpen => "Search files by name (append : to go to line, > for commands, @ for symbols)",
            OverlayKind::GoToLine => "Type a line number, or line:column",
            OverlayKind::Symbols => "Go to symbol in editor",
            OverlayKind::OpenRecent => "Select a folder to open",
            OverlayKind::CompareWith => "Select the file to compare the active file with",
            OverlayKind::Theme => "Select a color theme",
            OverlayKind::IconTheme => "Select a file icon theme",
            OverlayKind::ProductIconTheme => "Select a product icon theme",
            OverlayKind::CodeActions => "Select a code action to apply",
            OverlayKind::Definitions => "Select a definition to go to",
            OverlayKind::AiKey => match self.settings.ai_provider {
                forge_core::ai::Provider::OpenAi => "Paste your OpenAI API key (sk-...)",
                forge_core::ai::Provider::Gemini => "Paste your Gemini API key (AIza...)",
            },
            OverlayKind::Rename => "New name for the symbol",
            OverlayKind::NewEntry { folder: false, .. } => "File name (a/b/c.txt creates folders)",
            OverlayKind::NewEntry { folder: true, .. } => "Folder name",
            OverlayKind::RenameEntry(_) => "New name",
            OverlayKind::ConfirmDelete(_) | OverlayKind::ConfirmDiscard { .. } | OverlayKind::ConfirmDiscardHunk { .. } | OverlayKind::ConfirmReplace { .. } => "",
            OverlayKind::GitCheckout => "Select a branch to switch to",
            OverlayKind::GitRebase => "Select the branch to rebase the current branch onto",
            OverlayKind::GitMerge => "Select a branch to merge into the current branch",
            OverlayKind::GitRebaseFrom => "Select the commit to rebase after (the ones above it can be edited)",
            OverlayKind::GitCherryPick => "Select a commit from any branch to apply here",
            OverlayKind::GitRevert => "Select a commit to undo with a new commit",
            OverlayKind::GitSubmodules => "Submodules (Enter opens one in a new window)",
            OverlayKind::GitWorktrees => "Worktrees (Enter opens one in a new window)",
            OverlayKind::GitWorktreeAdd => "Select a branch to check out in a new worktree",
            OverlayKind::GitWorktreeRemove => "Select a worktree to remove",
            OverlayKind::GitCredential(prompt) => prompt.trim(),
            OverlayKind::GitCreateBranch => "New branch name",
            OverlayKind::GitHistory => "Select a commit to view its changes",
            OverlayKind::LanguageServers => "Filter language servers (Enter installs the selected one)",
            OverlayKind::ExtSearch => "Search Open VSX, or type publisher.name",
            OverlayKind::ExtVsix => "Path of a .vsix file",
            OverlayKind::ExtResults => "Filter the results (Enter installs the selected extension)",
            OverlayKind::ExtInstalled => "Filter installed extensions (Enter uninstalls the selected one)",
            OverlayKind::Snippets => "Select a snippet to insert",
        }
        .to_owned();
        if kind == OverlayKind::QuickOpen {
            self.ensure_file_index();
        }
        self.overlay = Some(Overlay { kind, input: LineInput::new(initial), items: Vec::new(), selected: 0, placeholder, note: None });
        self.completion = None;
        self.refresh_overlay();
    }

    /// Start the language servers for the languages found in the project's files, most common first.
    fn autostart_servers(&mut self, files: &[PathBuf]) {
        let mut counts: std::collections::HashMap<forge_syntax::Language, usize> = std::collections::HashMap::new();
        for path in files {
            if let Some(language) = forge_syntax::Language::from_path(path) {
                *counts.entry(language).or_default() += 1;
            }
        }
        let mut ranked: Vec<_> = counts.into_iter().collect();
        ranked.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.name().cmp(b.0.name())));
        // Only languages that are a real part of the project: a few helper scripts in a Rust
        // project should not launch a JavaScript server.
        let top = ranked.first().map_or(0, |(_, n)| *n);
        let languages: Vec<_> = ranked.into_iter().enumerate().filter(|(i, (_, n))| *i == 0 || (*n >= 3 && *n * 10 >= top)).map(|(_, (language, _))| language).collect();
        let started = self.lsp.autostart(&languages, 4);
        eprintln!("[lsp] autostart: {started:?}");
        if !started.is_empty() {
            self.set_status(format!("Starting language servers: {}", started.join(", ")));
        }
    }

    pub fn ensure_file_index(&mut self) {
        if self.file_index.is_some() || self.file_index_rx.is_some() {
            return;
        }
        let (tx, rx) = mpsc::channel();
        let root = self.root().to_path_buf();
        std::thread::spawn(move || {
            let _ = tx.send(project_search::list_files(&root, 50_000));
        });
        self.file_index_rx = Some(rx);
    }

    /// Rebuild the overlay's list for its current input.
    pub fn refresh_overlay(&mut self) {
        let Some(mut overlay) = self.overlay.take() else { return };
        let text = overlay.input.text().to_owned();
        overlay.note = None;
        overlay.items = match &overlay.kind {
            OverlayKind::Palette => self.palette_items(&text),
            OverlayKind::QuickOpen => {
                if let Some(rest) = text.strip_prefix('>') {
                    self.palette_items(rest)
                } else if let Some(rest) = text.strip_prefix('@') {
                    if self.symbols.is_empty() {
                        overlay.note = Some("Loading symbols (needs a running language server)...".to_owned());
                    }
                    self.symbol_items(rest)
                } else if text.starts_with(':') {
                    overlay.note = Some("Type a line number, then Enter".to_owned());
                    Vec::new()
                } else {
                    match &self.file_index {
                        Some(files) => file_entries(files, self.root(), &text, &self.recent_files, 60)
                            .into_iter()
                            .map(|e| {
                                let (dir, name) = match e.label.rfind('/') {
                                    Some(i) => (e.label[..i].to_owned(), e.label[i + 1..].to_owned()),
                                    None => (String::new(), e.label.clone()),
                                };
                                let name_offset = e.label.chars().count() - name.chars().count();
                                Item {
                                    label: name,
                                    detail: dir,
                                    positions: e.positions.into_iter().filter_map(|p| p.checked_sub(name_offset)).collect(),
                                    action: ItemAction::File(e.path),
                                }
                            })
                            .collect(),
                        None => {
                            overlay.note = Some("Indexing files...".to_owned());
                            Vec::new()
                        }
                    }
                }
            }
            OverlayKind::GoToLine => {
                let (total, current) = self.active_doc().zip(self.active_view()).map_or((0, 0), |(d, v)| (d.buffer.len_lines(), d.buffer.offset_to_point(v.selections.primary().head).0));
                overlay.note = Some(format!("Current line: {}. Type a line number between 1 and {total}.", current + 1));
                Vec::new()
            }
            OverlayKind::Symbols => {
                if self.symbols.is_empty() {
                    overlay.note = Some("Loading symbols...".to_owned());
                }
                self.symbol_items(&text)
            }
            OverlayKind::OpenRecent => {
                let folders = &self.recent_folders;
                if folders.is_empty() {
                    overlay.note = Some("No folders opened before".to_owned());
                }
                let labels: Vec<String> = folders.iter().map(|p| p.to_string_lossy().into_owned()).collect();
                rank(&text, &labels, 30)
                    .into_iter()
                    .map(|(i, m)| {
                        let name = folders[i].file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| labels[i].clone());
                        let skip = labels[i].chars().count().saturating_sub(name.chars().count());
                        Item { label: name, detail: labels[i].clone(), positions: m.positions.into_iter().filter_map(|p| p.checked_sub(skip)).collect(), action: ItemAction::OpenRecent(folders[i].clone()) }
                    })
                    .collect()
            }
            OverlayKind::CompareWith => match &self.file_index {
                Some(files) => file_entries(files, self.root(), &text, &self.recent_files, 60)
                    .into_iter()
                    .map(|e| Item { label: e.label.clone(), detail: String::new(), positions: e.positions, action: ItemAction::CompareWith(e.path) })
                    .collect(),
                None => {
                    overlay.note = Some("Indexing files...".to_owned());
                    Vec::new()
                }
            },
            OverlayKind::Theme => self.theme_items(&text),
            OverlayKind::IconTheme => self.icon_theme_items(&text),
            OverlayKind::ProductIconTheme => self.product_icon_theme_items(&text),
            OverlayKind::CodeActions => {
                if self.code_actions.is_empty() {
                    overlay.note = Some("Looking for code actions...".to_owned());
                }
                let titles: Vec<&str> = self.code_actions.iter().map(|a| a.title.as_str()).collect();
                rank(&text, &titles, 50)
                    .into_iter()
                    .map(|(i, m)| {
                        let a = &self.code_actions[i];
                        let detail = match (&a.kind, a.edit.is_empty(), &a.command) {
                            (_, false, _) if a.preferred => "preferred".to_owned(),
                            (_, true, Some(_)) => "server command (not supported)".to_owned(),
                            (Some(k), _, _) => k.clone(),
                            _ => String::new(),
                        };
                        Item { label: a.title.clone(), detail, positions: m.positions, action: ItemAction::CodeAction(i) }
                    })
                    .collect()
            }
            OverlayKind::Definitions => {
                let labels: Vec<String> = self.definitions.iter().map(|d| if d.preview.is_empty() { self.display_path(&d.location.path) } else { d.preview.clone() }).collect();
                let refs: Vec<&str> = labels.iter().map(String::as_str).collect();
                rank(&text, &refs, 50)
                    .into_iter()
                    .map(|(i, m)| {
                        let d = &self.definitions[i];
                        let detail = format!("{}:{}", self.display_path(&d.location.path), d.location.range.start.line + 1);
                        Item { label: labels[i].clone(), detail, positions: m.positions, action: ItemAction::Definition(i) }
                    })
                    .collect()
            }
            OverlayKind::AiKey => {
                let provider = self.settings.ai_provider;
                overlay.note = Some(format!(
                    "{} key from {}. Used only to write commit messages. Saved once, encrypted for your Windows account, so Forge does not ask again. Enter to confirm.",
                    provider.label(),
                    provider.key_page()
                ));
                Vec::new()
            }
            OverlayKind::Rename => {
                overlay.note = Some("Enter to rename every reference, Escape to cancel".to_owned());
                Vec::new()
            }
            OverlayKind::NewEntry { dir, folder } => {
                overlay.note = Some(format!("Create {} in {}", if *folder { "a folder" } else { "a file" }, dir.display()));
                Vec::new()
            }
            OverlayKind::RenameEntry(path) => {
                overlay.note = Some(format!("Rename {}", path.display()));
                Vec::new()
            }
            OverlayKind::ConfirmDelete(path) => {
                overlay.note = Some(format!("Press Enter to move '{}' to the Recycle Bin, Escape to cancel", path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()));
                Vec::new()
            }
            OverlayKind::ConfirmReplace { matches, files } => {
                let with = self.search.replace.text();
                overlay.note = Some(format!(
                    "Replace {matches} matches in {files} files with '{}'? Files with unsaved changes are skipped. Enter to replace, Escape to cancel",
                    if with.is_empty() { "nothing (delete the matches)" } else { with }
                ));
                Vec::new()
            }
            OverlayKind::ConfirmDiscard { rel, untracked } => {
                overlay.note = Some(if *untracked {
                    format!("Press Enter to move the untracked file '{rel}' to the Recycle Bin, Escape to cancel")
                } else {
                    format!("Press Enter to discard ALL unstaged changes in '{rel}'. This cannot be undone. Escape to cancel")
                });
                Vec::new()
            }
            OverlayKind::ConfirmDiscardHunk { rel, what, .. } => {
                overlay.note = Some(format!("Press Enter to discard {what} in '{rel}'. This cannot be undone. Escape to cancel"));
                Vec::new()
            }
            OverlayKind::GitCreateBranch => {
                overlay.note = Some("Creates the branch from HEAD and switches to it".to_owned());
                Vec::new()
            }
            OverlayKind::GitCherryPick | OverlayKind::GitRevert | OverlayKind::GitRebaseFrom => {
                let kind = overlay.kind.clone();
                let commits = if kind == OverlayKind::GitCherryPick { &self.pick_commits } else { &self.history };
                overlay.note = Some(
                    if commits.is_empty() {
                        "Loading commits (or there are none yet)..."
                    } else if kind == OverlayKind::GitRebaseFrom {
                        "The commits above the one you pick can be reordered, reworded, squashed or dropped"
                    } else if kind == OverlayKind::GitRevert {
                        "Adds a commit that undoes the one you pick"
                    } else {
                        "Applies the commit you pick on top of the current branch"
                    }
                    .to_owned(),
                );
                let subjects: Vec<&str> = commits.iter().map(|c| c.subject.as_str()).collect();
                rank(&text, &subjects, 80)
                    .into_iter()
                    .map(|(i, m)| {
                        let c = &commits[i];
                        let short: String = c.hash.chars().take(8).collect();
                        let action = match kind {
                            OverlayKind::GitCherryPick => ItemAction::GitCherryPickCommit(i),
                            OverlayKind::GitRevert => ItemAction::GitRevertCommit(i),
                            _ => ItemAction::GitRebaseFrom(i),
                        };
                        Item { label: format!("{short}  {}", c.subject), detail: format!("{} • {}", c.author, c.when), positions: m.positions.into_iter().map(|p| p + 10).collect(), action }
                    })
                    .collect()
            }
            OverlayKind::GitSubmodules => {
                if self.submodules.is_empty() {
                    overlay.note = Some("Loading submodules (there are none if this repository has no .gitmodules)...".to_owned());
                }
                let root = self.git.as_ref().map(|g| g.root.clone()).unwrap_or_default();
                let paths: Vec<&str> = self.submodules.iter().map(|s| s.path.as_str()).collect();
                rank(&text, &paths, 60)
                    .into_iter()
                    .map(|(i, m)| {
                        let s = &self.submodules[i];
                        Item { label: s.path.clone(), detail: format!("{}  {}", s.state_label(), s.hash.chars().take(8).collect::<String>()), positions: m.positions, action: ItemAction::OpenFolder(root.join(&s.path)) }
                    })
                    .collect()
            }
            OverlayKind::GitWorktrees | OverlayKind::GitWorktreeRemove => {
                let removing = overlay.kind == OverlayKind::GitWorktreeRemove;
                if self.worktrees.is_empty() {
                    overlay.note = Some("Loading worktrees...".to_owned());
                } else if removing {
                    overlay.note = Some("Git refuses to remove a worktree with uncommitted changes".to_owned());
                }
                let paths: Vec<&str> = self.worktrees.iter().map(|w| w.path.as_str()).collect();
                rank(&text, &paths, 60)
                    .into_iter()
                    // The first entry is the main working tree, which cannot be removed.
                    .filter(|(i, _)| !removing || *i > 0)
                    .map(|(i, m)| {
                        let w = &self.worktrees[i];
                        let branch = w.branch.clone().unwrap_or_else(|| if w.bare { "bare".to_owned() } else { format!("detached at {}", w.head) });
                        let name = std::path::Path::new(&w.path).file_name().map_or_else(|| w.path.clone(), |n| n.to_string_lossy().into_owned());
                        let detail = format!("{branch}{}{}", if i == 0 { "  (main)" } else { "" }, if w.locked { "  locked" } else { "" });
                        let action = if removing { ItemAction::WorktreeRemove(i) } else { ItemAction::OpenFolder(std::path::PathBuf::from(&w.path)) };
                        // The folder's name is what tells worktrees apart; the full path is in the tooltip-less detail's tail.
                        let positions = m.positions.into_iter().filter_map(|p| p.checked_sub(w.path.chars().count() - name.chars().count())).collect();
                        Item { label: name, detail, positions, action }
                    })
                    .collect()
            }
            OverlayKind::GitWorktreeAdd => {
                if self.branches.is_empty() {
                    overlay.note = Some("Loading branches...".to_owned());
                } else {
                    overlay.note = Some("The worktree is created in a folder next to this one".to_owned());
                }
                let names: Vec<&str> = self.branches.iter().map(|b| b.name.as_str()).collect();
                rank(&text, &names, 60)
                    .into_iter()
                    // A branch can be checked out in one worktree at a time: skip the current one and remote ones.
                    .filter(|(i, _)| !self.branches[*i].current && !self.branches[*i].remote)
                    .map(|(i, m)| Item { label: self.branches[i].name.clone(), detail: String::new(), positions: m.positions, action: ItemAction::WorktreeAddBranch(i) })
                    .collect()
            }
            OverlayKind::LanguageServers => {
                overlay.note = Some("Servers install into Forge's own folder; nothing global is changed. Add your own under \"languageServers\" in settings.json.".to_owned());
                self.server_items(&text)
            }
            OverlayKind::ExtSearch => {
                overlay.note = Some("Press Enter to search Open VSX (open-vsx.org). Themes install and apply; grammars, snippets and language settings are read too. Extension code is not run yet.".to_owned());
                Vec::new()
            }
            OverlayKind::ExtVsix => {
                overlay.note = Some("Type or paste the full path of a .vsix file, then press Enter".to_owned());
                Vec::new()
            }
            OverlayKind::ExtResults => self.ext_result_items(&text),
            OverlayKind::Snippets => {
                let items = self.snippet_items(&text);
                if items.is_empty() {
                    overlay.note = Some("No snippets for this language. Add some in a snippets folder next to settings.json (<language>.json) or install an extension that has them.".to_owned());
                }
                items
            }
            OverlayKind::ExtInstalled => {
                if self.extensions.is_empty() {
                    overlay.note = Some("No extensions installed. Use \"Extensions: Install from Open VSX...\"".to_owned());
                }
                self.ext_installed_items(&text)
            }
            OverlayKind::GitCheckout | OverlayKind::GitRebase | OverlayKind::GitMerge => {
                if self.branches.is_empty() {
                    overlay.note = Some("Loading branches...".to_owned());
                }
                let kind = overlay.kind.clone();
                let names: Vec<&str> = self.branches.iter().map(|b| b.name.as_str()).collect();
                rank(&text, &names, 60)
                    .into_iter()
                    // Rebasing or merging a branch into itself means nothing.
                    .filter(|(i, _)| kind == OverlayKind::GitCheckout || !self.branches[*i].current)
                    .map(|(i, m)| {
                        let b = &self.branches[i];
                        let detail = if b.current { "current" } else if b.remote { "remote" } else { "" };
                        let action = match kind {
                            OverlayKind::GitRebase => ItemAction::GitRebaseOnto(i),
                            OverlayKind::GitMerge => ItemAction::GitMergeBranch(i),
                            _ => ItemAction::GitBranch(i),
                        };
                        Item { label: b.name.clone(), detail: detail.to_owned(), positions: m.positions, action }
                    })
                    .collect()
            }
            OverlayKind::GitCredential(prompt) => {
                overlay.note = Some(format!("{}  Sent only to git for the running command. Enter to confirm, Escape to cancel it.", prompt.trim()));
                Vec::new()
            }
            OverlayKind::GitHistory => {
                if self.history.is_empty() {
                    overlay.note = Some("Loading history (or this has no commits yet)...".to_owned());
                }
                let subjects: Vec<&str> = self.history.iter().map(|c| c.subject.as_str()).collect();
                rank(&text, &subjects, 80)
                    .into_iter()
                    .map(|(i, m)| {
                        let c = &self.history[i];
                        let short: String = c.hash.chars().take(8).collect();
                        Item { label: format!("{short}  {}", c.subject), detail: format!("{} • {}", c.author, c.when), positions: m.positions.into_iter().map(|p| p + 10).collect(), action: ItemAction::GitCommit(i) }
                    })
                    .collect()
            }
        };
        overlay.selected = overlay.selected.min(overlay.items.len().saturating_sub(1));
        self.overlay = Some(overlay);
    }

    fn palette_items(&self, query: &str) -> Vec<Item> {
        let mut items: Vec<Item> = palette_entries(query, &self.keymap, &self.recent_commands, 60)
            .into_iter()
            .map(|e| Item { label: e.command.title().to_owned(), detail: e.shortcut.unwrap_or_default(), positions: e.positions, action: ItemAction::Command(e.command) })
            .collect();
        // A typed query that appears verbatim in an extension command's title is a strong match,
        // so those lead; with nothing typed they follow the built-ins.
        let ext = self.ext_command_items(query);
        if query.is_empty() {
            items.extend(ext);
            items
        } else {
            ext.into_iter().chain(items).collect()
        }
    }

    fn symbol_items(&self, query: &str) -> Vec<Item> {
        let names: Vec<&str> = self.symbols.iter().map(|s| s.name.as_str()).collect();
        rank(query, &names, 60)
            .into_iter()
            .map(|(i, m)| {
                let s = &self.symbols[i];
                Item {
                    label: format!("{}{}", "  ".repeat(s.depth), s.name),
                    detail: format!("{} :{}", s.kind_label(), s.range.start.line + 1),
                    positions: m.positions.into_iter().map(|p| p + 2 * s.depth).collect(),
                    action: ItemAction::Symbol(s.range.start.line, s.range.start.character),
                }
            })
            .collect()
    }

    /// Returns true when the overlay consumed the key.
    fn overlay_key(&mut self, k: &Keystroke, pressed: &str, cx: &mut Context<Self>) -> bool {
        // The pickers can be switched between without closing first.
        if let Resolution::Run { command, .. } = self.keymap.resolve("", pressed) {
            if matches!(command, Command::CommandPalette | Command::QuickOpen | Command::GoToLine | Command::GoToSymbol) {
                self.overlay = None;
                return false;
            }
        }
        let Some(overlay) = self.overlay.as_mut() else { return false };
        let ctrl = k.modifiers.control || k.modifiers.platform;
        match (k.key.as_str(), ctrl) {
            ("escape", _) => {
                self.overlay = None;
                return true;
            }
            ("down", false) if !overlay.items.is_empty() => overlay.selected = (overlay.selected + 1) % overlay.items.len(),
            ("up", false) if !overlay.items.is_empty() => overlay.selected = (overlay.selected + overlay.items.len() - 1) % overlay.items.len(),
            ("pagedown", _) => overlay.selected = (overlay.selected + 8).min(overlay.items.len().saturating_sub(1)),
            ("pageup", _) => overlay.selected = overlay.selected.saturating_sub(8),
            ("enter", _) => {
                self.accept_overlay(cx);
                return true;
            }
            ("v", true) => {
                if let Some(text) = cx.read_from_clipboard().and_then(|i| i.text()) {
                    overlay.input.insert(&text);
                    overlay.selected = 0;
                    self.refresh_overlay();
                }
            }
            _ => {
                if matches!(overlay.kind, OverlayKind::ConfirmDelete(_) | OverlayKind::ConfirmDiscard { .. } | OverlayKind::ConfirmDiscardHunk { .. } | OverlayKind::ConfirmReplace { .. }) {
                    return true;
                }
                match overlay.input.apply(&k.key, ctrl, k.key_char.as_deref()) {
                    Outcome::Edited => {
                        overlay.selected = 0;
                        self.refresh_overlay();
                        // `@` in Quick Open asks the server for symbols once.
                        let wants_symbols = self.overlay.as_ref().is_some_and(|o| o.kind == OverlayKind::QuickOpen && o.input.text().starts_with('@'));
                        if wants_symbols && self.symbols.is_empty() {
                            self.run_command(Command::GoToSymbol, false, cx);
                            self.open_overlay(OverlayKind::QuickOpen, "@");
                        }
                    }
                    Outcome::Moved => {}
                    _ => {}
                }
            }
        }
        true
    }

    pub(crate) fn accept_overlay(&mut self, cx: &mut Context<Self>) {
        let Some(overlay) = self.overlay.take() else { return };
        let text = overlay.input.text().trim().to_owned();
        if let Some(item) = overlay.items.get(overlay.selected) {
            self.perform_item(item.action.clone(), cx);
            return;
        }
        match overlay.kind {
            OverlayKind::GoToLine | OverlayKind::QuickOpen => {
                let query = text.trim_start_matches(':');
                if overlay.kind == OverlayKind::QuickOpen && !text.starts_with(':') {
                    return;
                }
                let Some((view, doc)) = self.active_view().zip(self.active_doc()) else { return };
                let current = doc.buffer.offset_to_point(view.selections.primary().head).0;
                if let Some((line, col)) = parse_go_to_line(query, doc.buffer.len_lines(), current) {
                    let offset = doc.buffer.point_to_offset(line, col);
                    self.place_caret_offset(offset);
                }
            }
            OverlayKind::GitCredential(_) => {
                // The answer goes to git exactly as typed (a password may begin or end with a space).
                if let Some(reply) = self.askpass_pending.take() {
                    let _ = reply.send(Some(overlay.input.text().to_owned()));
                }
            }
            OverlayKind::AiKey => {
                let provider = self.settings.ai_provider;
                if forge_core::ai::is_valid_key(&text) {
                    let saved = forge_core::ai::save_key(provider, &text);
                    self.ai_key = Some(text);
                    if self.git.as_ref().is_some_and(|g| g.status.staged().next().is_some()) {
                        self.git_generate_message();
                    } else {
                        self.set_status(match &saved {
                            Ok(()) => format!("{} API key saved, encrypted for your Windows account. Stage changes, then write a message with AI.", provider.label()),
                            Err(error) => format!("The key is kept for this session only: {error}"),
                        });
                    }
                } else {
                    self.set_status(format!("That does not look like a {} API key: it needs at least 8 characters and no spaces, quotes or backslashes. Paste only the key.", provider.label()));
                }
            }
            OverlayKind::Rename => {
                if !text.is_empty() {
                    self.send_rename(&text);
                }
            }
            OverlayKind::NewEntry { dir, folder } => {
                let result = if folder { fileops::create_dir(&dir, &text) } else { fileops::create_file(&dir, &text) };
                match result {
                    Ok(path) => {
                        self.tree.reveal(&path);
                        self.file_index = None;
        self.git_request_refresh();
                        if !folder {
                            self.open_file(&path, None, cx);
                        } else {
                            self.set_status(format!("Created {}", path.display()));
                        }
                    }
                    Err(message) => self.set_status(message),
                }
            }
            OverlayKind::RenameEntry(path) => self.finish_rename_entry(&path, &text),
            OverlayKind::ConfirmDelete(path) => self.finish_delete_entry(&path, cx),
            OverlayKind::ConfirmDiscard { rel, untracked } => self.finish_discard(&rel, untracked),
            OverlayKind::ConfirmDiscardHunk { patch, what, .. } => {
                self.git_job("hunk", move |root| git::apply_patch(root, &patch, false, true).map(|_| format!("Discarded {what}")));
            }
            OverlayKind::ConfirmReplace { .. } => self.finish_replace_all(),
            OverlayKind::ExtSearch => {
                if !text.is_empty() {
                    self.start_ext_search(text);
                }
            }
            OverlayKind::ExtVsix => {
                if !text.is_empty() {
                    self.start_vsix_install(text);
                }
            }
            OverlayKind::GitCreateBranch => {
                let name = text.clone();
                self.git_job("branch", move |r| forge_core::git::create_branch(r, &name).map(|_| format!("Created and switched to {name}")));
            }
            _ => {}
        }
    }

    fn perform_item(&mut self, action: ItemAction, cx: &mut Context<Self>) {
        match action {
            ItemAction::Command(c) => self.run_command(c, false, cx),
            ItemAction::File(path) => self.open_file(&path, None, cx),
            ItemAction::Theme(name) => {
                self.theme = crate::ext_support::resolve_theme(&self.extensions, &name);
                self.settings.theme = name;
            }
            ItemAction::IconTheme(name) => {
                let json = serde_json::Value::String(name.clone()).to_string();
                self.set_setting("workbench.iconTheme", &json);
                if self.settings.icon_theme == name {
                    self.set_status(format!("File icon theme: {name}"));
                }
            }
            ItemAction::ProductIconTheme(name) => {
                let json = serde_json::Value::String(name.clone()).to_string();
                self.set_setting("workbench.productIconTheme", &json);
                if self.settings.product_icon_theme == name {
                    self.set_status(format!("Product icon theme: {name}"));
                }
            }
            ItemAction::CodeAction(i) => {
                let Some(action) = self.code_actions.get(i).cloned() else { return };
                if !action.edit.is_empty() {
                    self.apply_workspace_edit(action.edit, cx);
                } else if let Some(command) = action.command {
                    self.set_status(format!("The server command '{command}' cannot be run by Forge"));
                }
            }
            ItemAction::Definition(i) => {
                if let Some(location) = self.definitions.get(i).map(|d| d.location.clone()) {
                    self.open_file(&location.path, Some((location.range.start.line, location.range.start.character)), cx);
                }
            }
            ItemAction::Symbol(line, col) => self.place_caret_utf16(line, col),
            ItemAction::GitBranch(i) => {
                if let Some(branch) = self.branches.get(i).cloned() {
                    self.git_checkout_branch(branch);
                }
            }
            ItemAction::GitRebaseOnto(i) => {
                if let Some(name) = self.branches.get(i).map(|b| b.name.clone()) {
                    self.git_job("rebase", move |r| git::rebase(r, &name));
                }
            }
            ItemAction::GitMergeBranch(i) => {
                if let Some(name) = self.branches.get(i).map(|b| b.name.clone()) {
                    self.git_job("merge", move |r| git::merge(r, &name));
                }
            }
            ItemAction::GitRebaseFrom(i) => {
                if let Some(commit) = self.history.get(i).cloned() {
                    let (hash, label) = (commit.hash.clone(), commit.subject.clone());
                    let (base, shown) = (hash.clone(), label.clone());
                    self.git_background(move |root| git::rebase_commits(root, &hash), move |result| crate::workspace::GitMsg::RebasePlan { base, label: shown, result });
                    let _ = label;
                }
            }
            ItemAction::GitCherryPickCommit(i) => {
                if let Some(hash) = self.pick_commits.get(i).map(|c| c.hash.clone()) {
                    self.git_job("cherry-pick", move |r| git::cherry_pick(r, &hash));
                }
            }
            ItemAction::GitRevertCommit(i) => {
                if let Some(hash) = self.history.get(i).map(|c| c.hash.clone()) {
                    self.git_job("revert", move |r| git::revert(r, &hash));
                }
            }
            ItemAction::OpenFolder(path) => self.open_in_new_window(&path),
            ItemAction::OpenRecent(path) => {
                if self.docs.iter().any(|d| d.dirty) {
                    self.set_status("Save or close modified files before opening another folder");
                } else {
                    self.open_folder(path, cx);
                }
            }
            ItemAction::CompareWith(path) => self.compare_with(&path, cx),
            ItemAction::WorktreeAddBranch(i) => {
                if let (Some(name), Some(root)) = (self.branches.get(i).map(|b| b.name.clone()), self.git.as_ref().map(|g| g.root.clone())) {
                    let folder = root.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "repo".to_owned());
                    let safe: String = name.chars().map(|c| if c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.') { c } else { '-' }).collect();
                    let path = root.parent().unwrap_or(&root).join(format!("{folder}-{safe}")).to_string_lossy().replace('\\', "/");
                    self.git_job("worktree", move |r| git::worktree_add(r, &path, &name, false));
                }
            }
            ItemAction::WorktreeRemove(i) => {
                if let Some(path) = self.worktrees.get(i).map(|w| w.path.clone()) {
                    self.git_job("worktree", move |r| git::worktree_remove(r, &path));
                }
            }
            ItemAction::GitCommit(i) => self.git_show_commit(i),
            action @ (ItemAction::Server(_) | ItemAction::ServersInstallAll) => self.perform_server_item(action),
            action @ (ItemAction::ExtListing(_) | ItemAction::ExtInstalled(_)) => self.perform_ext_item(action),
            ItemAction::Snippet(i) => self.insert_snippet_entry(i, cx),
            ItemAction::ExtCommand(id) => self.run_ext_command(&id, cx),
        }
    }

    // ---- find widget ----

    pub fn open_find(&mut self, replace: bool) {
        let seed = self.active_view().zip(self.active_doc()).and_then(|(view, doc)| {
            let s = view.selections.primary();
            let text = if s.is_empty() {
                word_range_at(doc.buffer.rope(), s.head).map(|(a, b)| doc.buffer.rope().slice(a..b).to_string())?
            } else {
                doc.buffer.rope().slice(s.start()..s.end()).to_string()
            };
            (!text.contains('\n')).then_some(text)
        });
        self.find.open = true;
        if replace {
            self.find.replace_visible = true;
        }
        self.find.focus_replace = false;
        if let Some(seed) = seed.filter(|s| !s.is_empty()) {
            self.find.query = LineInput::new(&seed);
        } else {
            let existing = self.find.query.text().to_owned();
            self.find.query = LineInput::new(&existing);
        }
        self.focus = Focus::Find;
        self.refresh_find();
        self.select_current_find_match();
    }

    pub fn refresh_find(&mut self) {
        let Some((view, doc)) = self.active_view().zip(self.active_doc()) else { return };
        let (from, text, key) = (view.selections.primary().start(), doc.buffer.text(), (view.doc, doc.revision));
        self.find.refresh(&text, from);
        self.find_revision = key;
    }

    fn select_current_find_match(&mut self) {
        let Some(m) = self.find.current_match() else { return };
        let Some((g, vi)) = self.active_ix() else { return };
        self.groups[g].views[vi].selections = SelectionSet::single(Selection { anchor: m.start, head: m.end });
        self.reveal_caret(g, vi, true);
    }

    pub fn find_next(&mut self, forward: bool, cx: &mut Context<Self>) {
        if self.find.query.text().is_empty() {
            self.open_find(false);
            return;
        }
        self.find.open = true;
        self.refresh_find();
        let Some(view) = self.active_view() else { return };
        let sel = view.selections.primary();
        let from = if forward { sel.end() } else { sel.start() };
        self.find.step(forward, from);
        self.select_current_find_match();
        cx.notify();
    }

    fn find_key(&mut self, k: &Keystroke, pressed: &str, cx: &mut Context<Self>) -> bool {
        let m = k.modifiers;
        let ctrl = m.control || m.platform;
        match (k.key.as_str(), ctrl, m.alt) {
            ("escape", ..) => {
                self.find.open = false;
                self.focus = Focus::Editor;
                return true;
            }
            ("enter", true, true) => {
                self.replace_all(cx);
                return true;
            }
            ("enter", ..) => {
                if self.find.focus_replace && self.find.replace_visible {
                    self.replace_current(cx);
                } else {
                    let Some(view) = self.active_view() else { return true };
                    let sel = view.selections.primary();
                    let forward = !m.shift;
                    let from = if forward { sel.end() } else { sel.start() };
                    self.find.step(forward, from);
                    self.select_current_find_match();
                }
                return true;
            }
            ("tab", false, false) if self.find.replace_visible => {
                self.find.focus_replace = !self.find.focus_replace;
                return true;
            }
            ("c", false, true) => {
                self.find.options.case_sensitive = !self.find.options.case_sensitive;
                self.refresh_find();
                return true;
            }
            ("w", false, true) => {
                self.find.options.whole_word = !self.find.options.whole_word;
                self.refresh_find();
                return true;
            }
            ("r", false, true) => {
                self.find.options.regex = !self.find.options.regex;
                self.refresh_find();
                return true;
            }
            ("up" | "down" | "pageup" | "pagedown", false, false) => return true,
            _ => {}
        }
        let replace_field = self.find.focus_replace && self.find.replace_visible;
        if pressed == "ctrl+v" {
            if let Some(text) = cx.read_from_clipboard().and_then(|i| i.text()) {
                if replace_field {
                    self.find.replacement.insert(&text);
                } else {
                    self.find.query.insert(&text);
                    self.refresh_find();
                    self.select_current_find_match();
                }
            }
            return true;
        }
        let input = if replace_field { &mut self.find.replacement } else { &mut self.find.query };
        match input.apply(&k.key, ctrl, k.key_char.as_deref()) {
            Outcome::Edited => {
                if !replace_field {
                    self.refresh_find();
                    self.select_current_find_match();
                }
                true
            }
            Outcome::Moved => true,
            _ => false,
        }
    }

    pub(crate) fn replace_current(&mut self, cx: &mut Context<Self>) {
        let Some(doc) = self.active_doc() else { return };
        let text = doc.buffer.text();
        let Some((s, e, replacement)) = self.find.replace_current_edit(&text) else {
            self.find_next(true, cx);
            return;
        };
        let reps = vec![(s, e, replacement)];
        self.mutate(cx, move |b, sels| {
            let len = reps[0].2.chars().count();
            let start = reps[0].0;
            b.edit_many(sels, &reps, move |_, _| SelectionSet::single(Selection::caret(start + len)))
        });
        self.refresh_find();
        self.select_current_find_match();
    }

    pub(crate) fn replace_all(&mut self, cx: &mut Context<Self>) {
        let Some(doc) = self.active_doc() else { return };
        let reps = self.find.replace_all_edits(&doc.buffer.text());
        if reps.is_empty() {
            return;
        }
        let count = reps.len();
        self.mutate(cx, move |b, sels| {
            let first = reps[0].0;
            b.edit_many(sels, &reps, move |_, _| SelectionSet::single(Selection::caret(first)))
        });
        self.refresh_find();
        self.set_status(format!("Replaced {count} occurrences"));
    }

    // ---- project search ----

    pub fn open_search_panel(&mut self) {
        let seed = self.active_view().zip(self.active_doc()).and_then(|(view, doc)| {
            let s = view.selections.primary();
            (!s.is_empty()).then(|| doc.buffer.rope().slice(s.start()..s.end()).to_string()).filter(|t| !t.contains('\n'))
        });
        self.sidebar_visible = true;
        self.sidebar_view = SidebarView::Search;
        self.focus = Focus::Search;
        if let Some(seed) = seed {
            self.search.query = LineInput::new(&seed);
        }
    }

    pub fn start_search(&mut self) {
        self.search.cancel.store(true, Ordering::Relaxed);
        let cancel = Arc::new(AtomicBool::new(false));
        self.search.cancel = cancel.clone();
        self.search.hits.clear();
        self.search.selected = 0;
        self.search.capped = false;
        self.search.error = None;
        if self.search.query.text().is_empty() {
            self.search.running = false;
            return;
        }
        let finder = match Finder::new(self.search.query.text(), self.search.options) {
            Ok(f) => f,
            Err(message) => {
                self.search.error = Some(message);
                self.search.running = false;
                return;
            }
        };
        let (tx, rx) = mpsc::channel();
        let root = self.search.scope.clone().unwrap_or_else(|| self.root().to_path_buf());
        std::thread::spawn(move || {
            let tx_hits = tx.clone();
            let capped = project_search::search(&root, &finder, &cancel, move |batch| {
                let _ = tx_hits.send(SearchMsg::Hits(batch));
            });
            let _ = tx.send(SearchMsg::Done { capped });
        });
        self.search.rx = Some(rx);
        self.search.running = true;
    }

    /// Drain finished background work. True if anything changed.
    pub fn poll_background(&mut self) -> bool {
        let mut changed = false;
        if let Some(rx) = &self.search.rx {
            loop {
                match rx.try_recv() {
                    Ok(SearchMsg::Hits(batch)) => {
                        self.search.hits.extend(batch);
                        changed = true;
                    }
                    Ok(SearchMsg::Done { capped }) => {
                        self.search.running = false;
                        self.search.capped = capped;
                        changed = true;
                    }
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => {
                        self.search.running = false;
                        self.search.rx = None;
                        changed = true;
                        break;
                    }
                }
            }
        }
        if let Some(rx) = &self.file_index_rx {
            if let Ok(files) = rx.try_recv() {
                if std::mem::take(&mut self.autostart_pending) {
                    self.autostart_servers(&files);
                }
                self.file_index = Some(files);
                self.file_index_rx = None;
                self.refresh_overlay();
                changed = true;
            }
        }
        changed |= self.poll_install();
        changed |= self.poll_ext();
        changed |= self.poll_ext_detail();
        // A shell that exited closes its terminal.
        let before = self.terminals.len();
        self.terminals.retain_mut(|t| t.is_alive() || t.keep_when_dead);
        if self.terminals.len() != before {
            self.active_terminal = self.active_terminal.min(self.terminals.len().saturating_sub(1));
            if self.terminals.is_empty() {
                if self.focus == Focus::Terminal {
                    self.focus = Focus::Editor;
                }
                self.panel_visible = self.panel_view != PanelView::Terminal && self.panel_visible;
            }
            changed = true;
        }
        changed
    }

    pub fn open_hit(&mut self, index: usize, cx: &mut Context<Self>) {
        let Some(hit) = self.search.hits.get(index).cloned() else { return };
        self.search.selected = index;
        self.open_file(&hit.path, None, cx);
        let Some((g, vi)) = self.active_ix() else { return };
        let view = &mut self.groups[g].views[vi];
        let doc = &self.docs[view.doc];
        let start = doc.buffer.point_to_offset(hit.line, hit.col);
        view.selections = SelectionSet::single(Selection { anchor: start, head: (start + hit.len).min(doc.buffer.len_chars()) });
        self.reveal_caret(g, vi, true);
    }

    /// Ask to replace every search result with the replacement text.
    pub fn prompt_replace_all(&mut self) {
        let matches = self.search.hits.len();
        if matches == 0 || self.search.error.is_some() {
            self.set_status("Search first: there is nothing to replace");
            return;
        }
        let files = self.search.hits.iter().map(|h| &h.path).collect::<std::collections::BTreeSet<_>>().len();
        self.open_overlay(OverlayKind::ConfirmReplace { matches, files }, "");
    }

    fn finish_replace_all(&mut self) {
        let finder = match Finder::new(self.search.query.text(), self.search.options) {
            Ok(f) => f,
            Err(message) => {
                self.set_status(message);
                return;
            }
        };
        let mut files: Vec<PathBuf> = self.search.hits.iter().map(|h| h.path.clone()).collect();
        files.dedup();
        files.sort();
        files.dedup();
        // A file open with unsaved edits must not be overwritten underneath the editor.
        let dirty: Vec<PathBuf> = self.docs.iter().filter(|d| d.dirty).map(|d| d.path.clone()).collect();
        let skip = |path: &Path| dirty.iter().any(|d| crate::workspace::same_path(d, path));
        let report = project_search::replace_in_files(&finder, self.search.replace.text(), &files, &skip);
        self.reload_clean_docs();
        self.tree.refresh();
        self.start_search();
        let mut message = format!("Replaced {} occurrences in {} files", report.replaced, report.files);
        if !report.skipped.is_empty() {
            message.push_str(&format!(" ({} skipped: unsaved changes)", report.skipped.len()));
        }
        if let Some((path, error)) = report.failed.first() {
            message.push_str(&format!(". Could not write {}: {error}", path.display()));
        }
        self.set_status(message);
    }

    fn search_key(&mut self, k: &Keystroke, pressed: &str, cx: &mut Context<Self>) -> bool {
        let m = k.modifiers;
        let ctrl = m.control || m.platform;
        let in_replace = self.search.replace_open && self.search.replace_focused;
        match (k.key.as_str(), ctrl, m.alt) {
            ("escape", ..) => {
                self.focus = Focus::Editor;
                return true;
            }
            ("enter", true, true) => {
                self.prompt_replace_all();
                return true;
            }
            ("tab", false, false) if self.search.replace_open => {
                self.search.replace_focused = !self.search.replace_focused;
                return true;
            }
            ("enter", false, false) if in_replace => {
                self.prompt_replace_all();
                return true;
            }
            ("enter", false, false) => {
                self.start_search();
                return true;
            }
            ("enter", _, true) | ("f4", ..) => {
                let selected = self.search.selected;
                self.open_hit(selected, cx);
                return true;
            }
            ("down", false, false) | ("up", false, false) => {
                use crate::workspace::SearchRow;
                let rows = self.search.rows();
                let selected = self.search.selected;
                let current = rows.iter().position(|r| matches!(r, SearchRow::Line { first, end } if (*first..*end).contains(&selected)));
                let lines: Vec<usize> = (0..rows.len()).filter(|&i| matches!(rows[i], SearchRow::Line { .. })).collect();
                let target = match current {
                    Some(cur) => {
                        let at = lines.iter().position(|&i| i == cur).unwrap_or(0);
                        let at = if k.key == "down" { (at + 1).min(lines.len() - 1) } else { at.saturating_sub(1) };
                        lines.get(at).copied()
                    }
                    None => lines.first().copied(),
                };
                if let Some(row) = target {
                    if let SearchRow::Line { first, .. } = rows[row] {
                        self.search.selected = first;
                        self.search.scroll.scroll_to_item(row, gpui::ScrollStrategy::Center);
                    }
                }
                return true;
            }
            ("c", false, true) => {
                self.search.options.case_sensitive = !self.search.options.case_sensitive;
                self.start_search();
                return true;
            }
            ("w", false, true) => {
                self.search.options.whole_word = !self.search.options.whole_word;
                self.start_search();
                return true;
            }
            ("r", false, true) => {
                self.search.options.regex = !self.search.options.regex;
                self.start_search();
                return true;
            }
            _ => {}
        }
        let field = if in_replace { &mut self.search.replace } else { &mut self.search.query };
        if pressed == "ctrl+v" {
            if let Some(text) = cx.read_from_clipboard().and_then(|i| i.text()) {
                field.insert(&text);
            }
            return true;
        }
        matches!(field.apply(&k.key, ctrl, k.key_char.as_deref()), Outcome::Edited | Outcome::Moved)
    }

    // ---- source control ----

    fn scm_key(&mut self, k: &Keystroke, _pressed: &str, cx: &mut Context<Self>) -> bool {
        let ctrl = k.modifiers.control || k.modifiers.platform;
        match (k.key.as_str(), ctrl) {
            ("escape", _) => {
                self.focus = Focus::Editor;
                return true;
            }
            ("c" | "x", true) => {
                if let Some(text) = self.scm_message.selected_text() {
                    cx.write_to_clipboard(gpui::ClipboardItem::new_string(text));
                    if k.key == "x" {
                        self.scm_message.delete_selection();
                    }
                }
                return true;
            }
            ("v", true) => {
                if let Some(text) = cx.read_from_clipboard().and_then(|i| i.text()) {
                    self.scm_message.insert(&text);
                }
                return true;
            }
            _ => {}
        }
        // Enter adds a line, like a text box; Ctrl+Enter commits.
        match self.scm_message.apply(&k.key, ctrl, k.modifiers.shift, k.key_char.as_deref()) {
            Outcome::Submit => {
                self.git_commit(false);
                true
            }
            Outcome::Edited | Outcome::Moved => true,
            _ => false,
        }
    }

    // ---- explorer ----

    fn selected_entry_path(&self) -> Option<(PathBuf, bool)> {
        let e = self.tree.entries.get(self.explorer_selected?)?;
        Some((e.path.clone(), e.is_dir))
    }

    fn explorer_key(&mut self, k: &Keystroke, _pressed: &str, cx: &mut Context<Self>) -> bool {
        let m = k.modifiers;
        if (m.control || m.platform) && !m.alt && !m.shift && matches!(k.key.as_str(), "x" | "c" | "v") {
            let target = self.selected_entry_path();
            match k.key.as_str() {
                "v" => self.explorer_paste(target, cx),
                key => self.explorer_copy(target.map(|t| t.0), key == "x"),
            }
            return true;
        }
        if m.control && !m.alt && !m.shift && k.key == "enter" {
            if let Some((path, false)) = self.selected_entry_path() {
                self.open_to_side(&path, cx);
            }
            return true;
        }
        if m.control || m.alt || m.platform {
            return false;
        }
        let n = self.tree.entries.len();
        let sel = self.explorer_selected.unwrap_or(0);
        match k.key.as_str() {
            "down" if n > 0 => self.explorer_selected = Some((sel + 1).min(n - 1)),
            "up" if n > 0 => self.explorer_selected = Some(sel.saturating_sub(1)),
            "home" if n > 0 => self.explorer_selected = Some(0),
            "end" if n > 0 => self.explorer_selected = Some(n - 1),
            "right" | "left" => {
                let Some((path, is_dir)) = self.selected_entry_path() else { return true };
                let expanded = self.tree.entries.get(sel).is_some_and(|e| e.expanded);
                if is_dir && (k.key == "right") != expanded {
                    self.tree.toggle(&path);
                } else if k.key == "left" {
                    // Collapse, or jump to the parent folder.
                    if let Some(parent) = path.parent() {
                        self.explorer_selected = self.tree.index_of(parent).or(self.explorer_selected);
                    }
                }
            }
            "enter" => {
                let Some((path, is_dir)) = self.selected_entry_path() else { return true };
                if is_dir {
                    self.tree.toggle(&path);
                } else {
                    self.open_file(&path, None, cx);
                }
            }
            "delete" => {
                if let Some((path, _)) = self.selected_entry_path() {
                    self.open_overlay(OverlayKind::ConfirmDelete(path), "");
                }
            }
            "escape" => self.focus = Focus::Editor,
            "f2" => self.prompt_rename_entry(),
            _ => return false,
        }
        if let Some(i) = self.explorer_selected {
            self.explorer_scroll.scroll_to_item(i, gpui::ScrollStrategy::Center);
        }
        true
    }

    pub fn prompt_new_entry(&mut self, folder: bool) {
        let dir = match self.selected_entry_path() {
            Some((path, true)) => path,
            Some((path, false)) => path.parent().map(Path::to_path_buf).unwrap_or_else(|| self.root().to_path_buf()),
            None => self.root().to_path_buf(),
        };
        self.open_overlay(OverlayKind::NewEntry { dir, folder }, "");
    }

    pub fn prompt_rename_entry(&mut self) {
        if let Some((path, _)) = self.selected_entry_path() {
            let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            self.open_overlay(OverlayKind::RenameEntry(path), &name);
        }
    }

    fn finish_rename_entry(&mut self, path: &Path, new_name: &str) {
        match fileops::rename(path, new_name) {
            Ok(new_path) => {
                // Open documents under the renamed path follow it, and re-attach to their server.
                for ix in 0..self.docs.len() {
                    let doc = &mut self.docs[ix];
                    if let Ok(rest) = doc.path.strip_prefix(path) {
                        let (language, was_open, old) = (doc.language, doc.lsp_open, doc.path.clone());
                        doc.path = if rest.as_os_str().is_empty() { new_path.clone() } else { new_path.join(rest) };
                        doc.lsp_open = false;
                        if let (Some(language), true) = (language, was_open) {
                            self.lsp.did_close(language, &old);
                            self.attach_lsp(ix);
                        }
                    }
                }
                self.tree.refresh();
                self.explorer_selected = self.tree.index_of(&new_path);
                self.file_index = None;
        self.git_request_refresh();
                self.set_status(format!("Renamed to {}", new_path.display()));
            }
            Err(message) => self.set_status(message),
        }
    }

    fn finish_delete_entry(&mut self, path: &Path, cx: &mut Context<Self>) {
        if self.docs.iter().any(|d| d.dirty && d.path.starts_with(path)) {
            self.set_status("Save or close modified files before deleting them");
            return;
        }
        // Close tabs for the deleted files first.
        while let Some((g, vi)) = self
            .groups
            .iter()
            .enumerate()
            .find_map(|(g, gr)| gr.views.iter().position(|v| self.docs[v.doc].path.starts_with(path)).map(|vi| (g, vi)))
        {
            self.close_view(g, vi, cx);
        }
        match fileops::move_to_trash(path) {
            Ok(()) => {
                self.tree.refresh();
                self.explorer_selected = self.explorer_selected.map(|i| i.min(self.tree.entries.len().saturating_sub(1)));
                self.file_index = None;
        self.git_request_refresh();
                self.set_status(format!("Moved {} to the Recycle Bin", path.display()));
            }
            Err(message) => self.set_status(message),
        }
    }

    // ---- terminal ----

    pub fn toggle_terminal(&mut self) {
        if self.panel_visible && self.panel_view == PanelView::Terminal && self.focus == Focus::Terminal {
            self.panel_visible = false;
            self.focus = Focus::Editor;
            return;
        }
        self.panel_visible = true;
        self.panel_view = PanelView::Terminal;
        if self.terminals.is_empty() {
            self.new_terminal();
        } else {
            self.focus = Focus::Terminal;
        }
    }

    pub fn new_terminal(&mut self) {
        let root = self.root().to_path_buf();
        self.new_terminal_in(&root);
    }

    /// A new terminal whose shell starts in `cwd`.
    pub fn new_terminal_in(&mut self, cwd: &Path) {
        let (program, args) = Terminal::default_shell();
        match Terminal::spawn(&program, &args, cwd, 12, 100) {
            Ok(terminal) => {
                self.terminals.push(terminal);
                self.active_terminal = self.terminals.len() - 1;
                self.panel_visible = true;
                self.panel_view = PanelView::Terminal;
                self.focus = Focus::Terminal;
            }
            Err(e) => self.set_status(format!("Cannot start a terminal: {e:#}")),
        }
    }

    /// Show the terminal `delta` places along (wrapping round), and give it the keyboard.
    pub fn switch_terminal(&mut self, delta: isize) {
        let n = self.terminals.len() as isize;
        if n == 0 {
            self.new_terminal();
            return;
        }
        self.active_terminal = (self.active_terminal as isize + delta).rem_euclid(n) as usize;
        self.panel_visible = true;
        self.panel_view = PanelView::Terminal;
        self.focus = Focus::Terminal;
    }

    /// End terminal `index` and take its tab away.
    pub fn kill_terminal(&mut self, index: usize) {
        if index >= self.terminals.len() {
            return;
        }
        self.terminals[index].kill();
        self.terminals.remove(index);
        if self.active_terminal > index || self.active_terminal >= self.terminals.len() {
            self.active_terminal = self.active_terminal.saturating_sub(1).min(self.terminals.len().saturating_sub(1));
        }
        if self.terminals.is_empty() {
            if self.focus == Focus::Terminal {
                self.focus = Focus::Editor;
            }
            if self.panel_view == PanelView::Terminal {
                self.panel_visible = false;
            }
        } else if self.focus == Focus::Terminal || self.panel_view == PanelView::Terminal {
            self.focus = Focus::Terminal;
        }
    }

    /// The mouse wheel over the terminal. `lines` is positive for up (older output). A full-screen
    /// program such as a pager gets arrow keys instead, as in other terminals.
    pub fn scroll_terminal(&mut self, lines: f32) {
        let Some(terminal) = self.terminals.get_mut(self.active_terminal) else { return };
        let mut step = lines.round() as isize;
        if step == 0 && lines != 0.0 {
            step = lines.signum() as isize;
        }
        if step == 0 {
            return;
        }
        if terminal.alternate_screen() {
            let arrow: &[u8] = if step > 0 { b"\x1b[A" } else { b"\x1b[B" };
            for _ in 0..step.unsigned_abs().min(5) {
                terminal.send(arrow);
            }
        } else {
            terminal.scroll(step);
        }
    }

    fn terminal_key(&mut self, k: &Keystroke, pressed: &str, cx: &mut Context<Self>) -> bool {
        if let Resolution::Run { command, .. } = self.keymap.resolve("", pressed) {
            if terminal_passthrough(command) {
                return false;
            }
        }
        let m = k.modifiers;
        let Some(terminal) = self.terminals.get_mut(self.active_terminal) else { return false };
        // Shift+PageUp / PageDown / Home / End move through the history instead of reaching the shell.
        let page = (terminal.size().0.saturating_sub(1) as isize).max(1);
        match pressed {
            "shift+pageup" => terminal.scroll(page),
            "shift+pagedown" => terminal.scroll(-page),
            "ctrl+shift+home" | "shift+home" => terminal.scroll(isize::MAX / 2),
            "ctrl+shift+end" | "shift+end" => terminal.scroll_to_bottom(),
            "ctrl+v" | "ctrl+shift+v" => {
                if let Some(text) = cx.read_from_clipboard().and_then(|i| i.text()) {
                    terminal.scroll_to_bottom();
                    terminal.send(text.as_bytes());
                }
            }
            _ => {
                if let Some(bytes) = key_bytes(&k.key, m.control, m.alt, m.shift, k.key_char.as_deref()) {
                    // Typing brings the live prompt back, like any terminal.
                    terminal.scroll_to_bottom();
                    terminal.send(&bytes);
                }
            }
        }
        true
    }

    /// Match the terminal grid to the panel size.
    pub fn sync_terminal_size(&mut self, viewport_width: Pixels) {
        let sidebar = if self.sidebar_visible { self.sidebar_width() } else { 0.0 };
        let cols = (f32::from(viewport_width) - sidebar - 24.0) / f32::from(self.char_width).max(1.0);
        let rows = (self.panel_height - 34.0) / self.line_height();
        for terminal in &mut self.terminals {
            terminal.resize(rows.max(2.0) as u16, cols.max(10.0) as u16);
        }
    }
}

fn px_f(v: f32) -> Pixels {
    gpui::px(v)
}
