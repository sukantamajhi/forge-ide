//! Right-click menus for the editor and the explorer: what goes in them, running a choice, drawing.

use std::path::{Path, PathBuf};

use forge_core::commands::Command;
use gpui::{div, prelude::*, px, rgb, AnyElement, ClipboardItem, Context, Div, MouseButton, MouseDownEvent, Pixels, Point, Size};

use crate::workspace::{Focus, OverlayKind, Workspace};

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
    CloseOthers,
    CloseAll,
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
}

impl Workspace {
    fn menu_item(&self, label: &str, action: MenuAction, shortcut: Option<String>, enabled: bool) -> MenuEntry {
        MenuEntry::Item { label: label.to_owned(), shortcut: shortcut.unwrap_or_default(), action, enabled }
    }

    fn command_item(&self, label: &str, command: Command, enabled: bool) -> MenuEntry {
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
        self.context_menu = Some(ContextMenu { position, entries, target: None });
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
        self.context_menu = Some(ContextMenu { position, entries, target: None });
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
        self.context_menu = Some(ContextMenu { position, entries, target });
        cx.notify();
    }

    /// Close every tab of the focused group but the active one. Files with unsaved changes stay.
    fn close_other_tabs(&mut self, cx: &mut Context<Self>) {
        let g = self.focused_group;
        let Some(active) = self.groups.get(g).map(|gr| gr.active) else { return };
        let (closing, kept) = self.closable_tabs(g, Some(active));
        for vi in closing.into_iter().rev() {
            self.close_view(g, vi, cx);
        }
        self.report_kept(kept);
    }

    /// Close every tab of the focused group. Files with unsaved changes stay.
    fn close_all_tabs(&mut self, cx: &mut Context<Self>) {
        let g = self.focused_group;
        let (closing, kept) = self.closable_tabs(g, None);
        for vi in closing.into_iter().rev() {
            if g >= self.groups.len() {
                break;
            }
            self.close_view(g, vi, cx);
        }
        self.report_kept(kept);
    }

    /// The tabs of group `g` that can be closed without losing work (all but `except`), and how many were kept for that reason.
    fn closable_tabs(&self, g: usize, except: Option<usize>) -> (Vec<usize>, usize) {
        let Some(group) = self.groups.get(g) else { return (Vec::new(), 0) };
        let (mut closing, mut kept) = (Vec::new(), 0);
        for (vi, view) in group.views.iter().enumerate() {
            if Some(vi) == except {
                continue;
            }
            // Unsaved work is never discarded by a bulk close; another tab on the same file keeps it safe.
            if self.docs[view.doc].dirty && self.views_of(view.doc).count() <= 1 {
                kept += 1;
            } else {
                closing.push(vi);
            }
        }
        (closing, kept)
    }

    fn report_kept(&mut self, kept: usize) {
        if kept > 0 {
            self.set_status(format!("{kept} tab{} with unsaved changes {} kept open", if kept == 1 { "" } else { "s" }, if kept == 1 { "was" } else { "were" }));
        }
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
        let entries = vec![
            self.menu_item(open_label, MenuAction::OpenEntry, None, true),
            MenuEntry::Separator,
            self.menu_item("New File...", MenuAction::NewFile, None, true),
            self.menu_item("New Folder...", MenuAction::NewFolder, None, true),
            MenuEntry::Separator,
            self.menu_item("Rename...", MenuAction::RenameEntry, Some("F2".to_owned()), true),
            self.menu_item("Delete", MenuAction::DeleteEntry, Some("Delete".to_owned()), true),
            MenuEntry::Separator,
            self.menu_item("Copy Path", MenuAction::CopyPath { relative: false }, None, true),
            self.menu_item("Copy Relative Path", MenuAction::CopyPath { relative: true }, None, true),
            self.menu_item("Reveal in File Explorer", MenuAction::RevealEntry, None, true),
        ];
        self.context_menu = Some(ContextMenu { position, entries, target: Some((path, is_dir)) });
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
            MenuAction::DeleteEntry => {
                if let Some((path, _)) = target {
                    self.open_overlay(OverlayKind::ConfirmDelete(path), "");
                }
            }
            MenuAction::CopyPath { relative } => {
                if let Some((path, _)) = target {
                    let text = if relative { self.display_path(&path) } else { path.display().to_string() };
                    cx.write_to_clipboard(ClipboardItem::new_string(text));
                    self.set_status(if relative { "Copied the relative path" } else { "Copied the path" });
                }
            }
            MenuAction::CloseOthers => self.close_other_tabs(cx),
            MenuAction::CloseAll => self.close_all_tabs(cx),
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
