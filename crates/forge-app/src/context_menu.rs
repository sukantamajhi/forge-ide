//! Right-click menus for the editor and the explorer: what goes in them, running a choice, drawing.

use std::path::{Path, PathBuf};

use forge_core::commands::Command;
use gpui::{div, prelude::*, px, rgb, AnyElement, ClipboardItem, Context, Div, MouseButton, MouseDownEvent, Pixels, Point, Size};

use crate::workspace::{Focus, OverlayKind, SidebarView, Workspace};

const MENU_WIDTH: f32 = 300.0;
const ROW_HEIGHT: f32 = 30.0;
const SEPARATOR_HEIGHT: f32 = 11.0;
/// Vertical padding inside the menu frame, top and bottom together.
const FRAME_PADDING: f32 = 8.0;

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum MenuAction {
    Command(Command),
    OpenEntry,
    NewFile,
    NewFolder,
    RenameEntry,
    DeleteEntry,
    CopyPath { relative: bool },
    RevealEntry,
    OpenInTerminal,
    OpenToSide,
    SelectForCompare,
    CompareWithSelected,
    FileHistory,
    CopyRemoteUrl,
    FindInFolder,
    CutEntry,
    CopyEntry,
    PasteEntry,
    CloseOthers,
    CloseAll,
    /// A choice from the menu of one particular tab: group `g`, tab `vi`.
    Tab { op: TabOp, g: usize, vi: usize },
}

/// What a tab's right-click menu can do to that tab.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TabOp {
    Close,
    CloseOthers,
    CloseRight,
    CloseSaved,
    CloseAll,
    CopyPath,
    CopyRelativePath,
    Pin,
    Unpin,
    SplitRight,
    RevealInOs,
    RevealInExplorer,
}

pub(crate) enum MenuEntry {
    Separator,
    Item { label: String, shortcut: String, action: MenuAction, enabled: bool },
}

pub(crate) struct ContextMenu {
    /// Window coordinates of the click.
    pub position: Point<Pixels>,
    pub entries: Vec<MenuEntry>,
    /// The explorer entry the menu was opened on, and whether it is a folder.
    pub target: Option<(PathBuf, bool)>,
    /// The menu-bar title (File, Edit...) this menu hangs from, if it came from the bar.
    pub bar: Option<usize>,
}

impl Workspace {
    pub(crate) fn menu_item(&self, label: &str, action: MenuAction, shortcut: Option<String>, enabled: bool) -> MenuEntry {
        MenuEntry::Item { label: label.to_owned(), shortcut: shortcut.unwrap_or_default(), action, enabled }
    }

    pub(crate) fn command_item(&self, label: &str, command: Command, enabled: bool) -> MenuEntry {
        self.menu_item(label, MenuAction::Command(command), self.keymap.shortcut_label(command), enabled)
    }

    /// The editor's menu at `position`. The caret moves to the click unless it landed inside a selection.
    pub fn open_editor_menu(&mut self, position: Point<Pixels>, cx: &mut Context<Self>) {
        let has_server = self.lsp_target().is_some();
        let (has_selection, writable) = match (self.active_view(), self.active_doc()) {
            (Some(view), Some(doc)) => (view.selections.as_slice().iter().any(|s| !s.is_empty()), !doc.readonly),
            _ => (false, false),
        };
        let entries = vec![
            self.command_item("Go to Definition", Command::GoToDefinition, has_server),
            self.command_item("Go to References", Command::FindReferences, has_server),
            self.command_item("Rename Symbol", Command::Rename, has_server && writable),
            self.command_item("Quick Fix...", Command::QuickFix, has_server && writable),
            MenuEntry::Separator,
            self.command_item("Cut", Command::Cut, has_selection && writable),
            self.command_item("Copy", Command::Copy, has_selection),
            self.command_item("Paste", Command::Paste, writable),
            MenuEntry::Separator,
            self.command_item("Select All", Command::SelectAll, true),
            self.command_item("Format Document", Command::FormatDocument, has_server && writable),
            MenuEntry::Separator,
            self.command_item("Command Palette...", Command::CommandPalette, true),
        ];
        self.context_menu = Some(ContextMenu { position, entries, target: None, bar: None });
        cx.notify();
    }

    /// The Git view's "more actions" menu, opened from the Commit button's arrow or the ... icon.
    pub fn open_scm_menu(&mut self, position: Point<Pixels>, cx: &mut Context<Self>) {
        let staged = self.git.as_ref().is_some_and(|g| g.status.staged().next().is_some());
        let entries = vec![
            self.command_item("Commit", Command::GitCommit, staged),
            self.command_item("Commit All", Command::GitCommitAll, true),
            self.command_item("Write Commit Message with AI", Command::GitGenerateMessage, staged && !self.ai_pending),
            MenuEntry::Separator,
            self.command_item("Pull", Command::GitPull, true),
            self.command_item("Push", Command::GitPush, true),
            self.command_item("Sync Changes", Command::GitSync, true),
            self.command_item("Fetch", Command::GitFetch, true),
            MenuEntry::Separator,
            self.command_item("Stage All Changes", Command::GitStageAll, true),
            self.command_item("Unstage All Changes", Command::GitUnstageAll, staged),
            MenuEntry::Separator,
            self.command_item("Checkout to...", Command::GitCheckout, true),
            self.command_item("Create Branch...", Command::GitCreateBranch, true),
            self.command_item("Stash", Command::GitStash, true),
            self.command_item("Pop Latest Stash", Command::GitStashPop, true),
            self.command_item("View History", Command::GitLog, true),
        ];
        self.context_menu = Some(ContextMenu { position, entries, target: None, bar: None });
        cx.notify();
    }

    /// The menu behind a file's "more actions" button in the tab bar of group `g`.
    pub fn open_tab_menu(&mut self, g: usize, position: Point<Pixels>, cx: &mut Context<Self>) {
        let Some(doc) = self.groups.get(g).and_then(|gr| gr.views.get(gr.active)).map(|v| &self.docs[v.doc]) else { return };
        // A generated document (a diff, a commit) has no file to copy the path of.
        let on_disk = !doc.readonly && doc.path.is_file();
        let target = on_disk.then(|| (doc.path.clone(), false));
        let git = self.git.is_some() && on_disk;
        let entries = vec![
            self.menu_item("Copy Path", MenuAction::CopyPath { relative: false }, None, on_disk),
            self.menu_item("Copy Relative Path", MenuAction::CopyPath { relative: true }, None, on_disk),
            self.menu_item("Reveal in File Explorer", MenuAction::RevealEntry, None, on_disk),
            MenuEntry::Separator,
            self.command_item("Close", Command::CloseTab, true),
            self.menu_item("Close Others", MenuAction::CloseOthers, None, self.groups[g].views.len() > 1),
            self.menu_item("Close All", MenuAction::CloseAll, None, true),
            MenuEntry::Separator,
            self.command_item("Toggle Minimap", Command::ToggleMinimap, true),
            self.command_item("Go to Line...", Command::GoToLine, true),
            self.command_item("Command Palette...", Command::CommandPalette, true),
            MenuEntry::Separator,
            self.command_item("Git: Stage This File", Command::GitStageFile, git),
            self.command_item("Git: Open Changes", Command::GitOpenChanges, git),
            self.command_item("Git: Show Blame for Line", Command::GitBlame, git),
            self.command_item("Git: View File History", Command::GitFileHistory, git),
        ];
        self.context_menu = Some(ContextMenu { position, entries, target, bar: None });
        cx.notify();
    }

    /// The menu of tab `vi` in group `g`, opened by right-clicking it.
    pub fn open_tab_context_menu(&mut self, g: usize, vi: usize, position: Point<Pixels>, cx: &mut Context<Self>) {
        let Some(group) = self.groups.get(g) else { return };
        let Some(view) = group.views.get(vi) else { return };
        let doc = &self.docs[view.doc];
        // A generated document (a diff, a commit, an extension page) has no file to copy the path of.
        let on_disk = !doc.readonly && doc.path.is_file();
        let pinned = view.pinned;
        let count = group.views.len();
        let can_close = |i: usize| !group.views[i].pinned;
        let others = (0..count).any(|i| i != vi && can_close(i));
        let right = ((vi + 1)..count).any(can_close);
        let saved = (0..count).any(|i| can_close(i) && !self.docs[group.views[i].doc].dirty);
        let any = (0..count).any(can_close);
        let tab = |op: TabOp| MenuAction::Tab { op, g, vi };
        let entries = vec![
            self.menu_item("Close", tab(TabOp::Close), self.keymap.shortcut_label(Command::CloseTab), !pinned),
            self.menu_item("Close Others", tab(TabOp::CloseOthers), None, others),
            self.menu_item("Close to the Right", tab(TabOp::CloseRight), None, right),
            self.menu_item("Close Saved", tab(TabOp::CloseSaved), self.keymap.shortcut_label(Command::CloseSavedTabs), saved),
            self.menu_item("Close All", tab(TabOp::CloseAll), self.keymap.shortcut_label(Command::CloseAllTabs), any),
            MenuEntry::Separator,
            self.menu_item("Copy Path", tab(TabOp::CopyPath), self.keymap.shortcut_label(Command::CopyFilePath), on_disk),
            self.menu_item("Copy Relative Path", tab(TabOp::CopyRelativePath), self.keymap.shortcut_label(Command::CopyRelativeFilePath), on_disk),
            MenuEntry::Separator,
            if pinned {
                self.menu_item("Unpin", tab(TabOp::Unpin), self.keymap.shortcut_label(Command::UnpinTab), true)
            } else {
                self.menu_item("Pin", tab(TabOp::Pin), self.keymap.shortcut_label(Command::PinTab), true)
            },
            MenuEntry::Separator,
            self.menu_item("Split Right", tab(TabOp::SplitRight), self.keymap.shortcut_label(Command::SplitEditor), true),
            MenuEntry::Separator,
            self.menu_item("Reveal in File Explorer", tab(TabOp::RevealInOs), self.keymap.shortcut_label(Command::RevealActiveFile), on_disk),
            self.menu_item("Reveal in Explorer View", tab(TabOp::RevealInExplorer), self.keymap.shortcut_label(Command::ShowActiveFileInExplorer), on_disk && doc.path.starts_with(self.root())),
        ];
        let target = on_disk.then(|| (doc.path.clone(), false));
        self.context_menu = Some(ContextMenu { position, entries, target, bar: None });
        cx.notify();
    }

    /// Close the tabs of group `g` that `wanted` picks. Pinned tabs and files with unsaved changes stay, and
    /// the tab that was in front stays in front.
    fn close_tabs_where(&mut self, g: usize, wanted: impl Fn(usize) -> bool, cx: &mut Context<Self>) {
        let Some(group) = self.groups.get(g) else { return };
        let front = group.views.get(group.active).map(|v| v.doc);
        let (mut closing, mut kept) = (Vec::new(), 0);
        for (vi, view) in group.views.iter().enumerate() {
            if !wanted(vi) || view.pinned {
                continue;
            }
            // Unsaved work is never discarded by a bulk close; another tab on the same file keeps it safe.
            if self.docs[view.doc].dirty && self.views_of(view.doc).count() <= 1 {
                kept += 1;
            } else {
                closing.push(vi);
            }
        }
        for vi in closing.into_iter().rev() {
            if g >= self.groups.len() {
                break;
            }
            self.close_view(g, vi, cx);
        }
        if let (Some(group), Some(doc)) = (self.groups.get_mut(g), front) {
            if let Some(at) = group.views.iter().position(|v| v.doc == doc) {
                group.active = at;
            }
        }
        self.report_kept(kept);
    }

    /// Pin or unpin tab `vi`: pinned tabs are kept together at the left.
    fn set_pinned(&mut self, g: usize, vi: usize, pinned: bool) {
        let Some(group) = self.groups.get_mut(g) else { return };
        if vi >= group.views.len() || group.views[vi].pinned == pinned {
            return;
        }
        let front = group.views[group.active].doc;
        let was_front = group.active == vi;
        let mut view = group.views.remove(vi);
        view.pinned = pinned;
        // Pinning joins the end of the pinned tabs; unpinning becomes the first unpinned tab.
        let at = group.views.iter().filter(|v| v.pinned).count();
        group.views.insert(at, view);
        group.active = if was_front { at } else { group.views.iter().position(|v| v.doc == front).unwrap_or(0) };
    }

    /// Do `op` to tab `vi` of group `g`.
    pub(crate) fn tab_op(&mut self, op: TabOp, g: usize, vi: usize, cx: &mut Context<Self>) {
        let Some(view) = self.groups.get(g).and_then(|gr| gr.views.get(vi)) else { return };
        let path = self.docs[view.doc].path.clone();
        match op {
            TabOp::Close => self.close_view(g, vi, cx),
            TabOp::CloseOthers => self.close_tabs_where(g, |i| i != vi, cx),
            TabOp::CloseRight => self.close_tabs_where(g, |i| i > vi, cx),
            TabOp::CloseSaved => {
                let saved: Vec<bool> = self.groups[g].views.iter().map(|v| !self.docs[v.doc].dirty).collect();
                self.close_tabs_where(g, |i| saved.get(i).copied().unwrap_or(false), cx);
            }
            TabOp::CloseAll => self.close_tabs_where(g, |_| true, cx),
            TabOp::CopyPath | TabOp::CopyRelativePath => {
                let relative = op == TabOp::CopyRelativePath;
                let text = if relative { self.display_path(&path) } else { path.display().to_string() };
                cx.write_to_clipboard(ClipboardItem::new_string(text));
                self.set_status(if relative { "Copied the relative path" } else { "Copied the path" });
            }
            TabOp::Pin => self.set_pinned(g, vi, true),
            TabOp::Unpin => self.set_pinned(g, vi, false),
            TabOp::SplitRight => {
                self.focused_group = g;
                self.groups[g].active = vi;
                self.run_command(Command::SplitEditor, false, cx);
            }
            TabOp::RevealInOs => match reveal_in_file_manager(&path) {
                Ok(()) => self.set_status(format!("Revealed {}", path.display())),
                Err(err) => self.set_status(format!("Cannot reveal {}: {err}", path.display())),
            },
            TabOp::RevealInExplorer => {
                self.sidebar_visible = true;
                self.sidebar_view = SidebarView::Explorer;
                self.tree.reveal(&path);
                if let Some(ix) = self.tree.entries.iter().position(|e| e.path == path) {
                    self.explorer_selected = Some(ix);
                    self.explorer_scroll.scroll_to_item(ix, gpui::ScrollStrategy::Center);
                }
                self.focus = Focus::Explorer;
            }
        }
    }

    /// The palette and keyboard versions of the tab operations act on the front tab of the focused group.
    pub(crate) fn run_tab_command(&mut self, command: Command, cx: &mut Context<Self>) {
        let g = self.focused_group;
        let Some(vi) = self.groups.get(g).filter(|gr| !gr.views.is_empty()).map(|gr| gr.active) else {
            self.set_status("No editor is open");
            return;
        };
        let op = match command {
            Command::CloseOtherTabs => TabOp::CloseOthers,
            Command::CloseTabsToRight => TabOp::CloseRight,
            Command::CloseSavedTabs => TabOp::CloseSaved,
            Command::CloseAllTabs => TabOp::CloseAll,
            Command::PinTab => TabOp::Pin,
            Command::UnpinTab => TabOp::Unpin,
            Command::CopyFilePath => TabOp::CopyPath,
            Command::CopyRelativeFilePath => TabOp::CopyRelativePath,
            Command::RevealActiveFile => TabOp::RevealInOs,
            _ => TabOp::RevealInExplorer,
        };
        let on_disk = self.docs[self.groups[g].views[vi].doc].path.is_file();
        if matches!(op, TabOp::CopyPath | TabOp::CopyRelativePath | TabOp::RevealInOs | TabOp::RevealInExplorer) && !on_disk {
            self.set_status("The current editor is not a file on disk");
            return;
        }
        self.tab_op(op, g, vi, cx);
    }

    fn report_kept(&mut self, kept: usize) {
        if kept > 0 {
            self.set_status(format!("{kept} tab{} with unsaved changes {} kept open", if kept == 1 { "" } else { "s" }, if kept == 1 { "was" } else { "were" }));
        }
    }

    /// The menu for the empty part of the Explorer: it acts on the project folder.
    pub fn open_explorer_root_menu(&mut self, position: Point<Pixels>, cx: &mut Context<Self>) {
        let root = self.root().to_path_buf();
        let entries = vec![
            self.menu_item("New File...", MenuAction::NewFile, None, true),
            self.menu_item("New Folder...", MenuAction::NewFolder, None, true),
            self.menu_item("Reveal in File Explorer", MenuAction::RevealEntry, None, true),
            self.menu_item("Open in Integrated Terminal", MenuAction::OpenInTerminal, None, true),
            MenuEntry::Separator,
            self.menu_item("Find in Folder...", MenuAction::FindInFolder, None, true),
            MenuEntry::Separator,
            self.menu_item("Paste", MenuAction::PasteEntry, Some("Ctrl+V".to_owned()), self.explorer_clipboard.is_some()),
            MenuEntry::Separator,
            self.menu_item("Copy Path", MenuAction::CopyPath { relative: false }, None, true),
            self.menu_item("Copy Relative Path", MenuAction::CopyPath { relative: true }, None, true),
        ];
        // Nothing is selected, so New File / New Folder go in the project folder.
        self.explorer_selected = None;
        self.context_menu = Some(ContextMenu { position, entries, target: Some((root, true)), bar: None });
        cx.notify();
    }

    /// The explorer's menu for row `ix`.
    pub fn open_explorer_menu(&mut self, ix: usize, position: Point<Pixels>, cx: &mut Context<Self>) {
        let Some(entry) = self.tree.entries.get(ix) else { return };
        let (path, is_dir, expanded) = (entry.path.clone(), entry.is_dir, entry.expanded);
        let open_label = match (is_dir, expanded) {
            (false, _) => "Open",
            (true, false) => "Expand",
            (true, true) => "Collapse",
        };
        let mut entries = vec![self.menu_item(open_label, MenuAction::OpenEntry, None, true)];
        if !is_dir {
            entries.push(self.menu_item("Open to the Side", MenuAction::OpenToSide, None, true));
        }
        entries.extend([
            MenuEntry::Separator,
            self.menu_item("New File...", MenuAction::NewFile, None, true),
            self.menu_item("New Folder...", MenuAction::NewFolder, None, true),
            self.menu_item("Reveal in File Explorer", MenuAction::RevealEntry, None, true),
        ]);
        if !is_dir {
            entries.push(self.menu_item("Open in Integrated Terminal", MenuAction::OpenInTerminal, None, true));
            entries.push(MenuEntry::Separator);
            entries.push(self.menu_item("Select for Compare", MenuAction::SelectForCompare, None, true));
            if self.compare_base.as_ref().is_some_and(|b| !crate::workspace::same_path(b, &path)) {
                entries.push(self.menu_item("Compare with Selected", MenuAction::CompareWithSelected, None, true));
            }
            if self.git.is_some() {
                entries.push(MenuEntry::Separator);
                entries.push(self.menu_item("File History", MenuAction::FileHistory, None, true));
                entries.push(self.menu_item("Copy Remote File URL", MenuAction::CopyRemoteUrl, None, true));
            }
        }
        if is_dir {
            entries.push(self.menu_item("Open in Integrated Terminal", MenuAction::OpenInTerminal, None, true));
            entries.push(MenuEntry::Separator);
            entries.push(self.menu_item("Find in Folder...", MenuAction::FindInFolder, None, true));
        }
        entries.extend([
            MenuEntry::Separator,
            self.menu_item("Cut", MenuAction::CutEntry, Some("Ctrl+X".to_owned()), true),
            self.menu_item("Copy", MenuAction::CopyEntry, Some("Ctrl+C".to_owned()), true),
            self.menu_item("Paste", MenuAction::PasteEntry, Some("Ctrl+V".to_owned()), self.explorer_clipboard.is_some()),
            MenuEntry::Separator,
            self.menu_item("Copy Path", MenuAction::CopyPath { relative: false }, None, true),
            self.menu_item("Copy Relative Path", MenuAction::CopyPath { relative: true }, None, true),
            MenuEntry::Separator,
            self.menu_item("Rename...", MenuAction::RenameEntry, Some("F2".to_owned()), true),
            self.menu_item("Delete", MenuAction::DeleteEntry, Some("Delete".to_owned()), true),
        ]);
        self.context_menu = Some(ContextMenu { position, entries, target: Some((path, is_dir)), bar: None });
        cx.notify();
    }

    /// Open `path` in the second editor group, making it if there is only one.
    pub fn open_to_side(&mut self, path: &Path, cx: &mut Context<Self>) {
        if self.groups.len() < 2 {
            self.groups.push(crate::workspace::Group { views: Vec::new(), active: 0 });
        }
        self.focused_group = 1;
        self.open_file(path, None, cx);
    }

    /// Ctrl+C / Ctrl+X on a file or folder in the Explorer.
    pub fn explorer_copy(&mut self, path: Option<PathBuf>, cut: bool) {
        let Some(path) = path else { return };
        self.set_status(format!("{} {}", if cut { "Cut" } else { "Copied" }, path.display()));
        self.explorer_clipboard = Some((path, cut));
    }

    /// Paste into `target` when it is a folder, else next to it (or into the project root).
    pub fn explorer_paste(&mut self, target: Option<(PathBuf, bool)>, cx: &mut Context<Self>) {
        let Some((from, cut)) = self.explorer_clipboard.clone() else { return };
        let dir = match target {
            Some((path, true)) => path,
            Some((path, false)) => path.parent().map(Path::to_path_buf).unwrap_or_else(|| self.root().to_path_buf()),
            None => self.root().to_path_buf(),
        };
        if !from.exists() {
            self.explorer_clipboard = None;
            self.set_status("What was cut or copied no longer exists");
            return;
        }
        let result = if cut { forge_core::fileops::move_into(&from, &dir) } else { forge_core::fileops::copy_into(&from, &dir) };
        match result {
            Ok(new_path) => {
                if cut {
                    self.explorer_clipboard = None;
                    // Open tabs under the moved path follow it.
                    for doc in &mut self.docs {
                        if let Ok(rest) = doc.path.strip_prefix(&from) {
                            doc.path = new_path.join(rest);
                        }
                    }
                }
                self.tree.reveal(&new_path);
                self.file_index = None;
                self.git_request_refresh();
                self.set_status(format!("{} {}", if cut { "Moved to" } else { "Pasted as" }, new_path.display()));
            }
            Err(message) => self.set_status(message),
        }
        cx.notify();
    }

    /// Run the enabled item at `ix` of the open menu, closing the menu.
    fn run_menu_item(&mut self, ix: usize, cx: &mut Context<Self>) {
        let Some(menu) = self.context_menu.take() else { return };
        if let Some(MenuEntry::Item { action, enabled: true, .. }) = menu.entries.get(ix) {
            self.run_menu_action(action.clone(), menu.target, cx);
        }
        cx.notify();
    }

    fn run_menu_action(&mut self, action: MenuAction, target: Option<(PathBuf, bool)>, cx: &mut Context<Self>) {
        match action {
            MenuAction::Command(command) => {
                self.focus = Focus::Editor;
                self.run_command(command, false, cx);
            }
            MenuAction::OpenEntry => {
                if let Some((path, is_dir)) = target {
                    if is_dir {
                        self.tree.toggle(&path);
                    } else {
                        self.open_file(&path, None, cx);
                    }
                }
            }
            MenuAction::NewFile => self.prompt_new_entry(false),
            MenuAction::NewFolder => self.prompt_new_entry(true),
            MenuAction::RenameEntry => self.prompt_rename_entry(),
            MenuAction::OpenInTerminal => {
                if let Some((path, is_dir)) = target {
                    let dir = if is_dir { Some(path) } else { path.parent().map(Path::to_path_buf) };
                    if let Some(dir) = dir {
                        self.new_terminal_in(&dir);
                    }
                }
            }
            MenuAction::OpenToSide => {
                if let Some((path, false)) = target {
                    self.open_to_side(&path, cx);
                }
            }
            MenuAction::SelectForCompare => {
                if let Some((path, false)) = target {
                    self.set_status(format!("Selected {} for compare: right-click another file and choose Compare with Selected", path.display()));
                    self.compare_base = Some(path);
                }
            }
            MenuAction::CompareWithSelected => {
                if let Some((path, false)) = target {
                    self.compare_with(&path, cx);
                }
            }
            MenuAction::FileHistory => {
                if let Some((path, false)) = target {
                    match self.git.as_ref().and_then(|g| forge_core::git::relative_path(&g.root, &path)) {
                        Some(rel) => {
                            self.history.clear();
                            self.open_overlay(OverlayKind::GitHistory, "");
                            self.git_request_history(Some(rel));
                        }
                        None => self.set_status("This file is not inside the repository"),
                    }
                }
            }
            MenuAction::CopyRemoteUrl => {
                if let Some((path, false)) = target {
                    let found = self.git.as_ref().and_then(|g| forge_core::git::relative_path(&g.root, &path).map(|rel| (g.root.clone(), rel)));
                    match found {
                        Some((root, rel)) => match forge_core::git::remote_file_url(&root, &rel) {
                            Ok(url) => {
                                cx.write_to_clipboard(ClipboardItem::new_string(url.clone()));
                                self.set_status(format!("Copied {url}"));
                            }
                            Err(message) => self.set_status(message),
                        },
                        None => self.set_status("This file is not inside the repository"),
                    }
                }
            }
            MenuAction::FindInFolder => {
                if let Some((path, true)) = target {
                    self.search.scope = Some(path);
                    self.open_search_panel();
                    self.start_search();
                }
            }
            MenuAction::CutEntry => self.explorer_copy(target.map(|t| t.0), true),
            MenuAction::CopyEntry => self.explorer_copy(target.map(|t| t.0), false),
            MenuAction::PasteEntry => self.explorer_paste(target, cx),
            MenuAction::DeleteEntry => {
                if let Some((path, _)) = target {
                    self.open_overlay(OverlayKind::ConfirmDelete(path), "");
                }
            }
            MenuAction::CopyPath { relative } => {
                if let Some((path, _)) = target {
                    let text = if relative { Some(self.display_path(&path)).filter(|t| !t.is_empty()).unwrap_or_else(|| ".".to_owned()) } else { path.display().to_string() };
                    cx.write_to_clipboard(ClipboardItem::new_string(text));
                    self.set_status(if relative { "Copied the relative path" } else { "Copied the path" });
                }
            }
            MenuAction::CloseOthers | MenuAction::CloseAll => {
                let g = self.focused_group;
                let vi = self.groups.get(g).map_or(0, |gr| gr.active);
                self.tab_op(if action == MenuAction::CloseAll { TabOp::CloseAll } else { TabOp::CloseOthers }, g, vi, cx);
            }
            MenuAction::Tab { op, g, vi } => self.tab_op(op, g, vi, cx),
            MenuAction::RevealEntry => {
                if let Some((path, _)) = target {
                    match reveal_in_file_manager(&path) {
                        Ok(()) => self.set_status(format!("Revealed {}", path.display())),
                        Err(err) => self.set_status(format!("Cannot reveal {}: {err}", path.display())),
                    }
                }
            }
        }
    }

    /// The open menu, kept inside the window.
    pub fn render_context_menu(&self, viewport: Size<Pixels>, cx: &mut Context<Self>) -> Option<Div> {
        let menu = self.context_menu.as_ref()?;
        let t = &self.theme;
        let k = (self.settings.font_size / 14.0).max(0.5);
        let height = (FRAME_PADDING * k)
            + menu.entries.iter().map(|e| if matches!(e, MenuEntry::Separator) { SEPARATOR_HEIGHT * k } else { ROW_HEIGHT * k }).sum::<f32>();
        let x = f32::from(menu.position.x).min(f32::from(viewport.width) - MENU_WIDTH * k).max(0.0);
        let y = f32::from(menu.position.y).min(f32::from(viewport.height) - height).max(0.0);
        let rows: Vec<AnyElement> = menu
            .entries
            .iter()
            .enumerate()
            .map(|(ix, entry)| match entry {
                MenuEntry::Separator => div()
                    .h(px(SEPARATOR_HEIGHT * k))
                    .px_2()
                    .flex()
                    .items_center()
                    .child(div().h(px(1.0)).w_full().bg(rgb(t.border)))
                    .into_any_element(),
                MenuEntry::Item { label, shortcut, enabled, .. } => {
                    let enabled = *enabled;
                    div()
                        .id(("context-menu-item", ix))
                        .h(px(ROW_HEIGHT * k))
                        .px_3()
                        .flex()
                        .flex_row()
                        .items_center()
                        .justify_between()
                        .gap_4()
                        .whitespace_nowrap()
                        .text_color(rgb(if enabled { t.text } else { t.dim }))
                        .when(enabled, |d| d.cursor_pointer().hover(|s| s.bg(rgb(t.selection))))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _: &MouseDownEvent, _, cx| {
                                cx.stop_propagation();
                                if enabled {
                                    this.run_menu_item(ix, cx);
                                }
                            }),
                        )
                        .child(label.clone())
                        .child(div().text_xs().text_color(rgb(t.dim)).child(shortcut.clone()))
                        .into_any_element()
                }
            })
            .collect();
        Some(
            div()
                .absolute()
                .left(px(x))
                .top(px(y))
                .w(px(MENU_WIDTH * k))
                .py_1()
                .bg(rgb(t.popup))
                .border_1()
                .border_color(rgb(t.border))
                .rounded_md()
                .shadow_lg()
                .occlude()
                .on_mouse_down(MouseButton::Left, cx.listener(|_, _: &MouseDownEvent, _, cx| cx.stop_propagation()))
                .on_mouse_down(MouseButton::Right, cx.listener(|_, _: &MouseDownEvent, _, cx| cx.stop_propagation()))
                .children(rows),
        )
    }
}

fn reveal_in_file_manager(path: &Path) -> std::io::Result<()> {
    #[cfg(windows)]
    {
        let native = path.to_string_lossy().replace('/', "\\");
        std::process::Command::new("explorer.exe").arg(format!("/select,{native}")).spawn().map(|_| ())
    }
    #[cfg(not(windows))]
    {
        let dir = if path.is_dir() { path } else { path.parent().unwrap_or(path) };
        std::process::Command::new("xdg-open").arg(dir).spawn().map(|_| ())
    }
}
