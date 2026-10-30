//! Images open in a viewer instead of failing as "not valid UTF-8".

use std::path::Path;

use gpui::{div, img, prelude::*, rems, rgb, Context, IntoElement};

use crate::workspace::{Focus, Workspace};

pub fn is_image(path: &Path) -> bool {
    path.extension().and_then(|e| e.to_str()).is_some_and(|e| matches!(e.to_ascii_lowercase().as_str(), "png" | "jpg" | "jpeg" | "gif" | "bmp" | "webp" | "ico"))
}

fn size_text(bytes: u64) -> String {
    if bytes >= 1 << 20 {
        format!("{:.1} MiB", bytes as f64 / f64::from(1u32 << 20))
    } else if bytes >= 1 << 10 {
        format!("{:.1} KiB", bytes as f64 / 1024.0)
    } else {
        format!("{bytes} bytes")
    }
}

impl Workspace {
    pub fn open_image(&mut self, path: &Path, cx: &mut Context<Self>) {
        if !path.is_file() {
            self.set_status(format!("Cannot open: {} does not exist", path.display()));
            return;
        }
        self.image_page = Some(path.to_path_buf());
        self.focus = Focus::Editor;
        self.set_status(format!("Opened {}", path.display()));
        cx.notify();
    }

    pub fn render_image_page(&self, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        let path = self.image_page.clone()?;
        let t = self.theme.clone();
        let left = if self.sidebar_visible { self.sidebar_width() } else { 0.0 };
        let size = std::fs::metadata(&path).map(|m| size_text(m.len())).unwrap_or_default();
        let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        Some(
            crate::scroll::overlay_page("image-page", left, t.bg, t.text)
                .p_6()
                .flex()
                .flex_col()
                .items_start()
                .justify_start()
                .gap_3()
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap_3()
                        .child(div().text_size(rems(1.25)).child(name))
                        .child(div().text_xs().text_color(rgb(t.dim)).child(size))
                        .child(
                            div()
                                .id("image-close")
                                .px_2()
                                .rounded_sm()
                                .cursor_pointer()
                                .border_1()
                                .border_color(rgb(t.border))
                                .hover(|s| s.bg(rgb(t.tab_active)))
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.image_page = None;
                                    cx.notify();
                                }))
                                .child("Close"),
                        ),
                )
                .child(img(path).max_w_full().rounded_sm().border_1().border_color(rgb(t.border))),
        )
    }
}
