// Release builds are a GUI program: launching from Explorer ("Open with Forge") must not flash a console window.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod command_impl;
mod completion;
mod context_menu;
mod debug_support;
mod git_support;
mod input_handling;
mod lsp;
mod markdown_view;
mod minimap;
mod render;
mod resize;
mod scroll;
mod host_support;
mod codicons;
mod icons;
mod markup_support;
mod menu_bar;
mod image_view;
mod prof;
mod product_icons;
mod snippet_support;
mod ext_support;
mod ext_view;
mod diff_view;
mod explorer_dnd;
mod conflict_view;
mod csv_view;
mod rebase_view;
mod git_panels;
mod file_icons;
mod server_support;
mod session_support;
mod settings_view;
mod terminal_mouse;
mod titlebar;
mod tree;
mod wrap;
mod workspace;

use std::path::PathBuf;

use gpui::{
    prelude::*, px, size, App, Application, Bounds, Focusable, TitlebarOptions, WindowBounds,
    WindowOptions,
};

use workspace::Workspace;

fn main() {
    prof::mark("process started");
    // Git runs this program to ask for a password or passphrase (see `forge_core::askpass`).
    let args: Vec<String> = std::env::args().collect();
    if let Some(code) = forge_core::askpass::run_as_helper(&args) {
        std::process::exit(code);
    }
    // Git runs this program in place of gpg when it signs a commit.
    if let Some(code) = forge_core::gpg_wrap::run_as_gpg(&args) {
        std::process::exit(code);
    }
    let target = std::env::args()
        .nth(1)
        .map(PathBuf::from)
        // With no argument, reopen the folder that was open last time (like VS Code), if it still exists.
        .or_else(|| forge_core::session::SessionStore::load().last_folder.filter(|p| p.is_dir()))
        .or_else(|| std::env::current_dir().ok())
        .unwrap_or_else(|| PathBuf::from("."));
    // Language servers need absolute paths (they become `file://` URIs).
    let target = std::path::absolute(&target).unwrap_or(target);
    // `forge some/file.rs` opens the file inside its project folder.
    let (root, initial_file) = if target.is_file() {
        (tree::project_root(&target), Some(target.clone()))
    } else {
        (target, None)
    };

    prof::mark("folder chosen");
    let application = Application::new();
    prof::mark("GPUI created");
    application.run(move |cx: &mut App| {
        prof::mark("application running");
        icons::register_font(cx);
        prof::mark("icon font registered");
        let bounds = Bounds::centered(None, size(px(1200.0), px(800.0)), cx);
        let options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            titlebar: Some(TitlebarOptions { title: Some("Forge".into()), appears_transparent: true, ..Default::default() }),
            ..Default::default()
        };
        cx.open_window(options, move |window, cx| {
            prof::mark("window opened");
            let workspace = cx.new(|cx| Workspace::new(root.clone(), initial_file.as_deref(), cx));
            prof::mark("workspace built");
            window.focus(&workspace.focus_handle(cx));
            workspace
        })
        .expect("failed to open the main window");
        cx.activate(true);
    });
}
