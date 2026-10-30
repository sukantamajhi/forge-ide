//! The menu bar under the window's title bar (File, Edit, Selection, View, Go, Run, Terminal, Help) and the
//! Explorer's New File / New Folder / Refresh / Collapse buttons. The drop-downs are the same menus the
//! right-click ones use, and every entry is a command with its shortcut.

use forge_core::commands::Command;
use gpui::{div, prelude::*, px, rgb, Context, IntoElement, MouseButton, MouseDownEvent, MouseMoveEvent, Point, Window, WindowControlArea};

use crate::context_menu::{ContextMenu, MenuAction, MenuEntry};
use crate::workspace::Workspace;

/// The title bar: logo, menus, the window title and the window buttons, all in one row.
pub const MENU_BAR_HEIGHT: f32 = 32.0;
const TITLES: [&str; 8] = ["File", "Edit", "Selection", "View", "Go", "Run", "Terminal", "Help"];

/// Each title has a fixed width so the drop-down can sit exactly under it.
fn title_width(ix: usize) -> f32 {
    TITLES[ix].len() as f32 * 7.5 + 20.0
}

/// Stands for the "..." menu in `ContextMenu::bar`.
const MORE: usize = usize::MAX;
const ICON_W: f32 = 30.0;
const ELLIPSIS_W: f32 = 36.0;
/// The three window buttons.
const WINDOW_BUTTONS_W: f32 = 138.0;

/// What fits in a title bar of a given width: how many menus (the rest sit behind "..."), the search box
/// with Go Back / Go Forward, the layout toggles, Settings.
#[derive(Debug, PartialEq, Eq)]
pub struct TitleLayout {
    pub menus: usize,
    pub center: bool,
    pub layout_icons: bool,
    pub gear: bool,
}

impl TitleLayout {
    pub fn for_width(width: f32) -> Self {
        let center = width >= 860.0;
        let layout_icons = width >= 700.0;
        let gear = width >= 520.0;
        let tools = if layout_icons { 2.0 * ICON_W } else { 0.0 } + if gear { ICON_W } else { 0.0 } + 8.0;
        let reserved = 4.0 + LOGO_SLOT + WINDOW_BUTTONS_W + tools + if center { 300.0 } else { 0.0 };
        let space = (width - reserved).max(0.0);
        let total: f32 = (0..TITLES.len()).map(title_width).sum();
        // Narrower than this, no title is worth showing: one hamburger button holds them all.
        let menus = if width < 560.0 {
            0
        } else if total <= space {
            TITLES.len()
        } else {
            let (mut used, mut n) = (ELLIPSIS_W, 0);
            while n < TITLES.len() && used + title_width(n) <= space {
                used += title_width(n);
                n += 1;
            }
            n
        };
        Self { menus, center, layout_icons, gear }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_wide_bar_shows_everything_and_a_narrow_one_folds_the_menus() {
        let wide = TitleLayout::for_width(1600.0);
        assert_eq!((wide.menus, wide.center, wide.layout_icons, wide.gear), (TITLES.len(), true, true, true));
        let medium = TitleLayout::for_width(1000.0);
        assert!(medium.menus < TITLES.len() && medium.menus > 0 && medium.center);
        let narrow = TitleLayout::for_width(480.0);
        assert!(!narrow.center && !narrow.layout_icons && !narrow.gear && narrow.menus == 0);
        // The menus that are shown never run into the window buttons.
        for w in (400..2000).step_by(25) {
            let l = TitleLayout::for_width(w as f32);
            let shown: f32 = (0..l.menus).map(title_width).sum::<f32>() + if l.menus < TITLES.len() { ELLIPSIS_W } else { 0.0 };
            let others = 4.0 + LOGO_SLOT + WINDOW_BUTTONS_W;
            assert!(shown + others <= w as f32, "{w}: {shown}");
        }
    }
}

/// Room at the left of the bar for the logo.
const LOGO_SLOT: f32 = 34.0;

fn title_left(ix: usize) -> f32 {
    4.0 + LOGO_SLOT + (0..ix).map(title_width).sum::<f32>()
}

/// The Forge mark (lines of code on a dark tile), `size` pixels square. It matches `assets/build_icons.py`.
pub fn logo_mark(size: f32) -> gpui::Div {
    let k = size / 256.0;
    let bar = |x: f32, y: f32, w: f32, h: f32, color: u32| div().absolute().left(px(x * k)).top(px(y * k)).w(px(w * k)).h(px(h * k)).rounded_full().bg(rgb(color));
    div()
        .relative()
        .flex_none()
        .w(px(size))
        .h(px(size))
        .rounded(px(58.0 * k))
        .bg(rgb(0x181A21))
        .child(bar(56.0, 57.0, 84.0, 22.0, 0xF2F2F2))
        .child(bar(86.0, 97.0, 116.0, 22.0, 0x8A90A0))
        .child(bar(86.0, 137.0, 62.0, 22.0, 0xFF8A1F))
        .child(bar(56.0, 177.0, 100.0, 22.0, 0xF2F2F2))
        .child(bar(162.0, 131.0, 12.0, 34.0, 0xFF8A1F))
}

impl Workspace {
    fn bar_menu(&self, ix: usize) -> Vec<MenuEntry> {
        use Command::*;
        let sep = || MenuEntry::Separator;
        let c = |label: &str, command: Command| self.command_item(label, command, true);
        match ix {
            0 => vec![
                c("New Text File", NewUntitledFile),
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
                c("Configure User Snippets", ConfigureSnippets),
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
                c("Markdown Preview", MarkdownPreviewFull),
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
        let position = Point { x: px(title_left(ix)), y: px(MENU_BAR_HEIGHT) };
        self.open_bar_menu_at(ix, position, cx);
    }

    /// Menu `ix` of the bar hanging from `position` (a hidden one opens where the "..." menu was).
    pub(crate) fn open_bar_menu_at(&mut self, ix: usize, position: Point<gpui::Pixels>, cx: &mut Context<Self>) {
        let entries = self.bar_menu(ix);
        self.context_menu = Some(ContextMenu { position, entries, target: None, bar: Some(ix) });
        cx.notify();
    }

    /// The "..." menu: the titles that did not fit, each opening its own menu.
    fn open_hidden_menus(&mut self, first: usize, cx: &mut Context<Self>) {
        let entries = (first..TITLES.len()).map(|i| self.menu_item(TITLES[i], MenuAction::BarMenu(i), None, true)).collect();
        let position = Point { x: px(title_left(first)), y: px(MENU_BAR_HEIGHT) };
        self.context_menu = Some(ContextMenu { position, entries, target: None, bar: Some(MORE) });
        cx.notify();
    }

    /// The "..." at the end of the activity icons: the panels and views that have no icon of their own.
    pub fn open_more_views_menu(&mut self, position: Point<gpui::Pixels>, cx: &mut Context<Self>) {
        use Command::*;
        let c = |label: &str, command: Command| self.command_item(label, command, true);
        let entries = vec![
            c("Problems", ShowProblems),
            c("Terminal", ToggleTerminal),
            c("Outline", ShowOutline),
            MenuEntry::Separator,
            c("Git Graph", ShowGitGraph),
            c("File Timeline", ShowGitTimeline),
        ];
        self.context_menu = Some(ContextMenu { position, entries, target: None, bar: None });
        cx.notify();
    }

    pub fn render_menu_bar(&self, window: &Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = &self.theme;
        let lay = TitleLayout::for_width(f32::from(window.viewport_size().width));
        let open = self.context_menu.as_ref().and_then(|m| m.bar);
        let hidden = lay.menus < TITLES.len();
        let hover_bg = t.border;
        let (dim, text) = (t.dim, t.text);
        // Titles that fit; the rest go behind a "..." button.
        let titles: Vec<_> = (0..lay.menus)
            .map(|ix| {
                div()
                    .id(("menu-bar", ix))
                    .w(px(title_width(ix)))
                    .h(px(MENU_BAR_HEIGHT - 8.0))
                    .flex()
                    .flex_none()
                    .items_center()
                    .justify_center()
                    .rounded_sm()
                    .cursor_pointer()
                    .text_color(rgb(if open == Some(ix) { text } else { dim }))
                    .when(open == Some(ix), |d| d.bg(rgb(hover_bg)))
                    .hover(move |s| s.bg(rgb(hover_bg)).text_color(rgb(text)))
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
            })
            .collect();
        let more = hidden.then(|| {
            let first_hidden = lay.menus;
            div()
                .id("menu-bar-more")
                .w(px(ELLIPSIS_W - 6.0))
                .h(px(MENU_BAR_HEIGHT - 8.0))
                .flex()
                .flex_none()
                .items_center()
                .justify_center()
                .rounded_sm()
                .cursor_pointer()
                .when(open == Some(MORE), |d| d.bg(rgb(hover_bg)))
                .hover(move |s| s.bg(rgb(hover_bg)))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _: &MouseDownEvent, _, cx| {
                        cx.stop_propagation();
                        if this.context_menu.as_ref().and_then(|m| m.bar) == Some(MORE) {
                            this.context_menu = None;
                            cx.notify();
                        } else {
                            this.open_hidden_menus(first_hidden, cx);
                        }
                    }),
                )
                .on_mouse_move(cx.listener(move |this, _: &MouseMoveEvent, _, cx| {
                    if this.context_menu.as_ref().and_then(|m| m.bar).is_some_and(|open| open != MORE) {
                        this.open_hidden_menus(first_hidden, cx);
                    }
                }))
                // With none of the titles fitting, the button is the hamburger; with some, it is the "...".
                .child(crate::icons::glyph(if first_hidden == 0 { crate::codicons::MENU } else { crate::codicons::ELLIPSIS }, 1.0, t.dim))
        });
        div()
            .flex()
            .flex_row()
            .flex_none()
            .items_center()
            .h(px(MENU_BAR_HEIGHT))
            // The root element moves keyboard focus on a mouse press and so marks the press as handled, which
            // would stop Windows from dragging the window by this bar or pressing its buttons.
            .occlude()
            .pl(px(4.0))
            .bg(rgb(t.sidebar))
            .border_b_1()
            .border_color(rgb(t.border))
            .text_size(px(13.0))
            .font_family(self.ui_font.clone())
            .child(div().w(px(LOGO_SLOT)).h_full().flex().flex_none().items_center().justify_center().child(logo_mark(18.0)))
            .children(titles)
            .children(more)
            .child(self.title_centre(&lay, cx))
            .child(self.title_tools(&lay, cx))
            .child(self.window_buttons(window))
    }

    /// A square icon button for the title bar. It takes the pointer itself, so the drag area under it does not.
    fn title_button(&self, id: &'static str, glyph: char, color: u32, tip: &'static str, command: Command, cx: &mut Context<Self>) -> impl IntoElement {
        let t = &self.theme;
        let hover = t.border;
        div()
            .id(id)
            .occlude()
            .w(px(ICON_W))
            .h(px(MENU_BAR_HEIGHT - 8.0))
            .flex()
            .flex_none()
            .items_center()
            .justify_center()
            .rounded_sm()
            .cursor_pointer()
            .hover(move |s| s.bg(rgb(hover)))
            .tooltip(self.tooltip(tip, Some(command)))
            .on_click(cx.listener(move |this, _, _, cx| {
                this.run_command(command, false, cx);
                cx.notify();
            }))
            .child(crate::icons::glyph(glyph, 1.0, color))
    }

    /// The middle of the bar: it drags the window, and holds Go Back / Go Forward and the search box with
    /// the folder's name when there is room.
    fn title_centre(&self, lay: &TitleLayout, cx: &mut Context<Self>) -> impl IntoElement {
        let t = &self.theme;
        let folder = self.root().file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| self.root().display().to_string());
        let back_color = if self.nav_back.is_empty() { t.border } else { t.dim };
        let forward_color = if self.nav_forward.is_empty() { t.border } else { t.dim };
        let group = lay.center.then(|| {
            div()
                .absolute()
                .top_0()
                .left_0()
                .size_full()
                .flex()
                .flex_row()
                .items_center()
                .justify_center()
                .gap_1()
                .px_2()
                .child(self.title_button("nav-back", crate::codicons::ARROW_LEFT, back_color, "Go Back", Command::NavigateBack, cx))
                .child(self.title_button("nav-forward", crate::codicons::ARROW_RIGHT, forward_color, "Go Forward", Command::NavigateForward, cx))
                .child(
                    div()
                        .id("command-centre")
                        .occlude()
                        .flex_1()
                        .min_w(px(120.0))
                        .max_w(px(420.0))
                        .h(px(MENU_BAR_HEIGHT - 10.0))
                        .px_3()
                        .flex()
                        .flex_row()
                        .items_center()
                        .justify_center()
                        .gap_2()
                        .rounded_md()
                        .border_1()
                        .border_color(rgb(t.border))
                        .bg(rgb(t.bg))
                        .cursor_pointer()
                        .text_size(px(12.0))
                        .text_color(rgb(t.dim))
                        .hover(|s| s.border_color(rgb(t.dim)))
                        .tooltip(self.tooltip("Go to File", Some(Command::QuickOpen)))
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.run_command(Command::QuickOpen, false, cx);
                            cx.notify();
                        }))
                        .child(crate::icons::glyph(crate::codicons::SEARCH, 0.75, t.dim))
                        .child(div().min_w_0().overflow_hidden().whitespace_nowrap().child(folder)),
                )
        });
        div()
            .relative()
            .flex_1()
            .h_full()
            .min_w_0()
            .overflow_hidden()
            .child(div().window_control_area(WindowControlArea::Drag).absolute().top_0().left_0().size_full())
            .children(group)
    }

    /// Layout toggles and Settings, at the right before the window buttons, as far as the width allows.
    fn title_tools(&self, lay: &TitleLayout, cx: &mut Context<Self>) -> impl IntoElement {
        let t = &self.theme;
        let sidebar = if self.sidebar_visible { crate::codicons::LAYOUT_SIDEBAR_LEFT } else { crate::codicons::LAYOUT_SIDEBAR_LEFT_OFF };
        let sidebar_color = if self.sidebar_visible { t.text } else { t.dim };
        let panel_color = if self.panel_visible { t.text } else { t.dim };
        div()
            .flex()
            .flex_row()
            .flex_none()
            .items_center()
            .gap_0p5()
            .pr_2()
            .children(lay.layout_icons.then(|| self.title_button("tool-sidebar", sidebar, sidebar_color, "Toggle Primary Side Bar", Command::ToggleSidebar, cx)))
            .children(lay.layout_icons.then(|| self.title_button("tool-panel", crate::codicons::LAYOUT_PANEL, panel_color, "Toggle Panel", Command::TogglePanel, cx)))
            .children(lay.gear.then(|| self.title_button("tool-settings", crate::codicons::SETTINGS_GEAR, t.dim, "Settings", Command::OpenSettings, cx)))
    }

    /// Minimise, maximise (or restore) and close. Windows handles the clicks itself from the control areas.
    fn window_buttons(&self, window: &Window) -> impl IntoElement {
        let t = &self.theme;
        let maximized = window.is_maximized();
        let button = |area: WindowControlArea, glyph: char, hover: u32, hover_text: u32| {
            div()
                .window_control_area(area)
                .w(px(46.0))
                .h_full()
                .flex()
                .flex_none()
                .items_center()
                .justify_center()
                .font_family("Segoe MDL2 Assets")
                .text_size(px(10.0))
                .text_color(rgb(t.text))
                .hover(move |s| s.bg(rgb(hover)).text_color(rgb(hover_text)))
                .child(glyph.to_string())
        };
        div()
            .flex()
            .flex_row()
            .flex_none()
            .h_full()
            .child(button(WindowControlArea::Min, char::from_u32(0xE921).unwrap_or('-'), t.border, t.text))
            .child(button(WindowControlArea::Max, char::from_u32(if maximized { 0xE923 } else { 0xE922 }).unwrap_or('+'), t.border, t.text))
            .child(button(WindowControlArea::Close, char::from_u32(0xE8BB).unwrap_or('x'), 0xE81123, 0xFFFFFF))
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
