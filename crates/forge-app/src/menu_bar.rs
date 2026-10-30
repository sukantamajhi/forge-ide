//! The menu bar under the window's title bar (File, Edit, Selection, View, Go, Run, Terminal, Help) and the
//! Explorer's New File / New Folder / Refresh / Collapse buttons. The drop-downs are the same menus the
//! right-click ones use, and every entry is a command with its shortcut.

use forge_core::commands::Command;
use gpui::{div, prelude::*, px, rgb, Context, IntoElement, MouseButton, MouseDownEvent, MouseMoveEvent, Point};

use crate::context_menu::{ContextMenu, MenuEntry};
use crate::workspace::Workspace;

pub const MENU_BAR_HEIGHT: f32 = 28.0;
const TITLES: [&str; 8] = ["File", "Edit", "Selection", "View", "Go", "Run", "Terminal", "Help"];

/// Each title has a fixed width so the drop-down can sit exactly under it.
fn title_width(ix: usize) -> f32 {
    TITLES[ix].len() as f32 * 8.0 + 24.0
}

fn title_left(ix: usize) -> f32 {
    4.0 + (0..ix).map(title_width).sum::<f32>()
}

impl Workspace {
    fn bar_menu(&self, ix: usize) -> Vec<MenuEntry> {
        use Command::*;
        let sep = || MenuEntry::Separator;
        let c = |label: &str, command: Command| self.command_item(label, command, true);
        match ix {
            0 => vec![
                c("New File...", NewFile),
                c("New Folder...", NewFolder),
                c("New Window", NewWindow),
                sep(),
                c("Open Folder...", OpenFolder),
                c("Open Recent...", OpenRecent),
                sep(),
                c("Save", Save),
                c("Save All", SaveAll),
                c("Toggle Auto Save", ToggleAutoSave),
                sep(),
                c("Compare Active File With...", CompareActiveFileWith),
                sep(),
                c("Close Editor", CloseTab),
                c("Close All Editors", CloseAllTabs),
                c("Reopen Closed Editor", ReopenClosedTab),
                sep(),
                c("Settings", OpenSettings),
                c("Keyboard Shortcuts", OpenKeybindings),
                c("Color Theme", ChangeTheme),
                sep(),
                c("Exit", Quit),
            ],
            1 => vec![
                c("Undo", Undo),
                c("Redo", Redo),
                sep(),
                c("Cut", Cut),
                c("Copy", Copy),
                c("Paste", Paste),
                sep(),
                c("Find", Find),
                c("Replace", Replace),
                c("Find in Files", FindInFiles),
                c("Replace in Files", ReplaceInFiles),
                sep(),
                c("Toggle Line Comment", ToggleLineComment),
                c("Toggle Block Comment", ToggleBlockComment),
                sep(),
                c("Format Document", FormatDocument),
                c("Format Selection", FormatSelection),
                c("Organize Imports", OrganizeImports),
            ],
            2 => vec![
                c("Select All", SelectAll),
                c("Expand Selection", ExpandSelection),
                c("Shrink Selection", ShrinkSelection),
                sep(),
                c("Copy Line Up", CopyLinesUp),
                c("Copy Line Down", CopyLinesDown),
                c("Move Line Up", MoveLinesUp),
                c("Move Line Down", MoveLinesDown),
                sep(),
                c("Add Cursor Above", AddCursorAbove),
                c("Add Cursor Below", AddCursorBelow),
                c("Add Next Occurrence", AddSelectionToNextMatch),
                c("Select All Occurrences", SelectAllOccurrences),
            ],
            3 => vec![
                c("Command Palette...", CommandPalette),
                sep(),
                c("Explorer", FocusExplorer),
                c("Search", FindInFiles),
                c("Source Control", ShowGit),
                c("Extensions", ShowExtensions),
                sep(),
                c("Problems", ShowProblems),
                c("Terminal", ToggleTerminal),
                c("Toggle Panel", TogglePanel),
                c("Toggle Primary Side Bar", ToggleSidebar),
                sep(),
                c("Split Editor", SplitEditor),
                c("Word Wrap", ToggleWordWrap),
                c("Minimap", ToggleMinimap),
                c("Zen Mode", ZenMode),
                sep(),
                c("Zoom In", ZoomIn),
                c("Zoom Out", ZoomOut),
                c("Reset Zoom", ZoomReset),
            ],
            4 => vec![
                c("Back", NavigateBack),
                c("Forward", NavigateForward),
                sep(),
                c("Go to File...", QuickOpen),
                c("Go to Symbol in Editor...", GoToSymbol),
                c("Go to Definition", GoToDefinition),
                c("Go to References", FindReferences),
                c("Go to Line/Column...", GoToLine),
                c("Go to Bracket", GoToBracket),
                sep(),
                c("Next Problem", NextProblem),
                c("Previous Problem", PreviousProblem),
            ],
            5 => vec![
                c("Start Debugging", DebugStart),
                c("Stop Debugging", DebugStop),
                sep(),
                c("Step Over", DebugStepOver),
                c("Step Into", DebugStepInto),
                c("Step Out", DebugStepOut),
                sep(),
                c("Toggle Breakpoint", ToggleBreakpoint),
                c("Remove All Breakpoints", ClearBreakpoints),
            ],
            6 => vec![
                c("New Terminal", NewTerminal),
                c("Toggle Terminal", ToggleTerminal),
                sep(),
                c("Next Terminal", TerminalNext),
                c("Previous Terminal", TerminalPrevious),
                c("Kill Terminal", TerminalKill),
            ],
            _ => vec![
                c("Show All Commands", CommandPalette),
                c("Keyboard Shortcuts (JSON)", OpenKeybindings),
                sep(),
                c("Language Servers...", LanguageServers),
                c("Install Extension from Open VSX...", ExtensionsInstall),
                sep(),
                c("About Forge", ShowAbout),
            ],
        }
    }

    fn open_bar_menu(&mut self, ix: usize, cx: &mut Context<Self>) {
        let entries = self.bar_menu(ix);
        let position = Point { x: px(title_left(ix)), y: px(MENU_BAR_HEIGHT) };
        self.context_menu = Some(ContextMenu { position, entries, target: None, bar: Some(ix) });
        cx.notify();
    }

    pub fn render_menu_bar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = &self.theme;
        let open = self.context_menu.as_ref().and_then(|m| m.bar);
        div()
            .flex()
            .flex_row()
            .flex_none()
            .items_center()
            .h(px(MENU_BAR_HEIGHT))
            .pl(px(4.0))
            .bg(rgb(t.sidebar))
            .border_b_1()
            .border_color(rgb(t.border))
            .text_size(px(13.0))
            .children((0..TITLES.len()).map(|ix| {
                div()
                    .id(("menu-bar", ix))
                    .w(px(title_width(ix)))
                    .h(px(MENU_BAR_HEIGHT - 6.0))
                    .flex()
                    .flex_none()
                    .items_center()
                    .justify_center()
                    .rounded_sm()
                    .cursor_pointer()
                    .text_color(rgb(if open == Some(ix) { t.text } else { t.dim }))
                    .when(open == Some(ix), |d| d.bg(rgb(t.border)))
                    .hover(|s| s.bg(rgb(t.border)).text_color(rgb(t.text)))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _: &MouseDownEvent, _, cx| {
                            cx.stop_propagation();
                            if this.context_menu.as_ref().and_then(|m| m.bar) == Some(ix) {
                                this.context_menu = None;
                                cx.notify();
                            } else {
                                this.open_bar_menu(ix, cx);
                            }
                        }),
                    )
                    // With one menu open, moving onto another title opens that one, as in every menu bar.
                    .on_mouse_move(cx.listener(move |this, _: &MouseMoveEvent, _, cx| {
                        if this.context_menu.as_ref().and_then(|m| m.bar).is_some_and(|open| open != ix) {
                            this.open_bar_menu(ix, cx);
                        }
                    }))
                    .child(TITLES[ix])
            }))
    }

    /// New File, New Folder, Refresh and Collapse Folders, at the right of the Explorer's folder name.
    pub fn render_explorer_toolbar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = &self.theme;
        let button = |id: &'static str, icon: char, name: &'static str, command: Command, cx: &mut Context<Self>| {
            div()
                .id(id)
                .tooltip(self.tooltip(name, None))
                .w(gpui::rems(1.5))
                .h(gpui::rems(1.5))
                .flex()
                .flex_none()
                .items_center()
                .justify_center()
                .rounded_sm()
                .cursor_pointer()
                .hover(|s| s.bg(rgb(t.border)))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _: &MouseDownEvent, _, cx| {
                        cx.stop_propagation();
                        this.focus = crate::workspace::Focus::Explorer;
                        this.run_command(command, false, cx);
                        cx.notify();
                    }),
                )
                .child(crate::icons::glyph(icon, 1.0, t.dim))
        };
        div()
            .flex()
            .flex_row()
            .flex_none()
            .items_center()
            .gap_0p5()
            .child(button("explorer-new-file", crate::codicons::NEW_FILE, "New File...", Command::NewFile, cx))
            .child(button("explorer-new-folder", crate::codicons::NEW_FOLDER, "New Folder...", Command::NewFolder, cx))
            .child(button("explorer-refresh", crate::codicons::REFRESH, "Refresh Explorer", Command::RefreshExplorer, cx))
            .child(button("explorer-collapse", crate::codicons::COLLAPSE_ALL, "Collapse Folders in Explorer", Command::CollapseExplorer, cx))
    }
}
