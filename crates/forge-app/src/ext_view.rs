//! The Extensions view in the sidebar: a search box over Open VSX, the installed extensions,
//! and Install / Uninstall buttons.

use forge_core::input::Outcome;
use forge_ext_host::openvsx::Listing;
use gpui::{div, prelude::*, rems, rgb, AnyElement, Context, FontWeight, Keystroke, MouseButton, MouseDownEvent};

use crate::workspace::{Focus, ItemAction, Workspace};

/// 1234 -> "1.2K", 2_500_000 -> "2.5M".
fn count(n: u64) -> String {
    match n {
        0..=999 => n.to_string(),
        1_000..=999_999 => format!("{:.1}K", n as f64 / 1_000.0),
        _ => format!("{:.1}M", n as f64 / 1_000_000.0),
    }
}

impl Workspace {
    pub(crate) fn ext_key(&mut self, k: &Keystroke, pressed: &str, cx: &mut Context<Self>) -> bool {
        let ctrl = k.modifiers.control || k.modifiers.platform;
        match k.key.as_str() {
            "escape" => {
                self.focus = Focus::Editor;
                return true;
            }
            "enter" => {
                self.start_panel_search();
                return true;
            }
            _ => {}
        }
        if pressed == "ctrl+v" {
            if let Some(text) = cx.read_from_clipboard().and_then(|i| i.text()) {
                self.ext_panel.query.insert(&text);
            }
            return true;
        }
        matches!(self.ext_panel.query.apply(&k.key, ctrl, k.key_char.as_deref()), Outcome::Edited | Outcome::Moved)
    }

    /// The square with an extension's first letter, standing in for its icon.
    fn ext_tile(&self, name: &str) -> gpui::Div {
        let t = &self.theme;
        let letter = name.chars().find(|c| c.is_alphanumeric()).map(|c| c.to_uppercase().to_string()).unwrap_or_else(|| "?".to_owned());
        div()
            .flex_none()
            .w(rems(2.5))
            .h(rems(2.5))
            .flex()
            .items_center()
            .justify_center()
            .rounded_md()
            .bg(rgb(t.tab_active))
            .text_color(rgb(t.info))
            .text_size(rems(1.25))
            .font_weight(FontWeight::BOLD)
            .child(letter)
    }

    fn ext_heading(&self, text: String) -> AnyElement {
        div().px_3().pt_2().pb_1().text_xs().font_weight(FontWeight::BOLD).text_color(rgb(self.theme.dim)).child(text).into_any_element()
    }

    pub(crate) fn render_extensions(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = &self.theme;
        let panel = &self.ext_panel;
        let query = panel.query.text().trim().to_lowercase();
        let installed_ids: Vec<String> = self.extensions.iter().map(|e| e.id().to_lowercase()).collect();

        let status = if let Some(error) = &panel.error {
            format!("Open VSX: {error}")
        } else if panel.waiting {
            "Searching Open VSX...".to_owned()
        } else if panel.searched && panel.results.is_empty() {
            "No extensions found".to_owned()
        } else if panel.shown_query.is_empty() {
            "Popular on Open VSX. Type a name and press Enter to search.".to_owned()
        } else {
            format!("{} results for \"{}\"", panel.results.len(), panel.shown_query)
        };

        let mut list = div().id("ext-list").flex().flex_col().flex_1().min_h_0().overflow_y_scroll().pb_2();

        // Installed extensions, narrowed by what is typed.
        let installed: Vec<usize> = (0..self.extensions.len())
            .filter(|&i| {
                let e = &self.extensions[i];
                query.is_empty() || e.display_name().to_lowercase().contains(&query) || e.id().to_lowercase().contains(&query)
            })
            .collect();
        list = list.child(self.ext_heading(format!("INSTALLED  {}", installed.len())));
        if installed.is_empty() {
            list = list.child(div().px_3().py_1().text_xs().text_color(rgb(t.dim)).child("Nothing installed yet."));
        }
        for i in installed {
            let e = &self.extensions[i];
            let name = e.display_name();
            let summary = e.supported_summary();
            list = list.child(
                div()
                    .id(("ext-installed", i))
                    .flex()
                    .flex_row()
                    .gap_2()
                    .px_3()
                    .py_1()
                    .items_center()
                    .hover(|s| s.bg(rgb(t.tab_active)))
                    .child(self.ext_tile(&name))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .flex_1()
                            .min_w_0()
                            .child(div().flex().flex_row().gap_1().items_baseline().child(div().font_weight(FontWeight::MEDIUM).overflow_hidden().whitespace_nowrap().text_ellipsis().child(name)).child(div().flex_none().text_xs().text_color(rgb(t.dim)).child(e.manifest.version.clone())))
                            .child(div().text_xs().text_color(rgb(t.dim)).overflow_hidden().whitespace_nowrap().text_ellipsis().child(summary))
                            .child(div().text_xs().text_color(rgb(t.dim)).overflow_hidden().whitespace_nowrap().text_ellipsis().child(e.id())),
                    )
                    .child(
                        div()
                            .id(("ext-uninstall", i))
                            .flex_none()
                            .w(rems(1.75))
                            .h(rems(1.75))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded_sm()
                            .cursor_pointer()
                            .hover(|s| s.bg(rgb(t.selection)))
                            .tooltip(self.tooltip("Uninstall", None))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.perform_ext_item(ItemAction::ExtInstalled(i));
                                cx.notify();
                            }))
                            .child(crate::icons::glyph(crate::codicons::TRASH, 1.0, t.dim)),
                    ),
            );
        }

        // What Open VSX offers.
        list = list.child(self.ext_heading("OPEN VSX".to_owned()));
        for (i, listing) in panel.results.iter().enumerate() {
            let id = listing.id();
            let has = installed_ids.contains(&id.to_lowercase());
            let busy = self.ext_installing.as_deref() == Some(id.as_str());
            list = list.child(self.ext_result_row(i, listing, has, busy, cx));
        }
        if panel.waiting && panel.results.is_empty() {
            list = list.child(div().px_3().py_1().text_xs().text_color(rgb(t.dim)).child("Loading..."));
        }

        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .gap_1()
            .pt_2()
            .child(
                div()
                    .px_2()
                    .on_mouse_down(MouseButton::Left, cx.listener(|this, _: &MouseDownEvent, _, cx| {
                        this.focus = Focus::Extensions;
                        cx.notify();
                    }))
                    .child(self.render_input(&panel.query, "Search Extensions in Open VSX", self.focus == Focus::Extensions)),
            )
            .child(div().px_3().text_xs().text_color(rgb(if panel.error.is_some() { t.error } else { t.dim })).child(status))
            .child(list)
    }

    fn ext_result_row(&self, i: usize, listing: &Listing, installed: bool, busy: bool, cx: &mut Context<Self>) -> AnyElement {
        let t = &self.theme;
        let action: AnyElement = if installed {
            div().flex_none().text_xs().text_color(rgb(t.dim)).child("Installed").into_any_element()
        } else if busy {
            div().flex_none().text_xs().text_color(rgb(t.dim)).child("Installing...").into_any_element()
        } else {
            let target = listing.clone();
            div()
                .id(("ext-install", i))
                .flex_none()
                .flex()
                .flex_row()
                .items_center()
                .gap_1()
                .px_2()
                .h(rems(1.5))
                .rounded_sm()
                .cursor_pointer()
                .text_xs()
                .bg(rgb(t.button))
                .text_color(rgb(t.button_text))
                .hover(|s| s.opacity(0.85))
                .tooltip(self.tooltip(&format!("Install {}", listing.id()), None))
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.start_ext_install(target.clone());
                    cx.notify();
                }))
                .child(crate::icons::glyph(crate::codicons::CLOUD_DOWNLOAD, 0.875, t.button_text))
                .child("Install")
                .into_any_element()
        };
        div()
            .id(("ext-result", i))
            .flex()
            .flex_row()
            .gap_2()
            .px_3()
            .py_1()
            .hover(|s| s.bg(rgb(t.tab_active)))
            .child(self.ext_tile(&listing.display_name))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w_0()
                    .child(div().flex().flex_row().gap_1().items_baseline().child(div().font_weight(FontWeight::MEDIUM).overflow_hidden().whitespace_nowrap().text_ellipsis().child(listing.display_name.clone())).child(div().flex_none().text_xs().text_color(rgb(t.dim)).child(listing.version.clone())))
                    .child(div().text_xs().text_color(rgb(t.dim)).overflow_hidden().whitespace_nowrap().text_ellipsis().child(listing.description.clone()))
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .justify_between()
                            .gap_2()
                            .pt(rems(0.125))
                            .child(div().min_w_0().text_xs().text_color(rgb(t.dim)).overflow_hidden().whitespace_nowrap().text_ellipsis().child(format!("{}  ·  {}", listing.namespace, count(listing.downloads))))
                            .child(action),
                    ),
            )
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::count;

    #[test]
    fn download_counts_are_short() {
        assert_eq!(count(7), "7");
        assert_eq!(count(1_234), "1.2K");
        assert_eq!(count(2_500_000), "2.5M");
    }
}
