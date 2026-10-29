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
mod host_support;
mod snippet_support;
mod ext_support;
mod file_icons;
mod server_support;
mod settings_view;
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
    let target = std::env::args()
        .nth(1)
        .map(PathBuf::from)
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

    Application::new().run(move |cx: &mut App| {
        let bounds = Bounds::centered(None, size(px(1200.0), px(800.0)), cx);
        let options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            titlebar: Some(TitlebarOptions { title: Some("Forge".into()), ..Default::default() }),
            ..Default::default()
        };
        cx.open_window(options, move |window, cx| {
            let workspace = cx.new(|cx| Workspace::new(root.clone(), initial_file.as_deref(), cx));
            window.focus(&workspace.focus_handle(cx));
            workspace
        })
        .expect("failed to open the main window");
        cx.activate(true);
    });
}
