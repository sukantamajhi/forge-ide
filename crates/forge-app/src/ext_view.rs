//! The Extensions view in the sidebar: a search box over Open VSX, the installed extensions,
//! and Install / Uninstall buttons.

use forge_core::input::Outcome;
use forge_ext_host::openvsx::Listing;
use gpui::{div, list, prelude::*, px, rems, rgb, AnyElement, Context, FontWeight, Keystroke, MouseButton, MouseDownEvent, Window};

use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, TryRecvError};

use forge_ext_host::openvsx::Details;
use forge_ext_host::Extension;

use crate::workspace::{Doc, ExtDetail, Focus, ItemAction, Readme, View, Workspace};

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
                    .cursor_pointer()
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.open_ext_detail_installed(i);
                        cx.notify();
                    }))
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
                            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
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
                    .cursor_text()
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
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.start_ext_install(target.clone());
                    cx.notify();
                }))
                .child(crate::icons::glyph(crate::codicons::CLOUD_DOWNLOAD, 0.875, t.button_text))
                .child("Install")
                .into_any_element()
        };
        let open = listing.clone();
        div()
            .id(("ext-result", i))
            .cursor_pointer()
            .on_click(cx.listener(move |this, _, _, cx| {
                this.open_ext_detail_listing(open.clone());
                cx.notify();
            }))
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

/// A URL written as a string or as `{ "url": ... }` in a package.json field.
fn url_of(value: Option<&serde_json::Value>) -> Option<String> {
    let value = value?;
    value.as_str().or_else(|| value.get("url").and_then(|u| u.as_str())).filter(|u| !u.is_empty()).map(|u| u.trim_start_matches("git+").to_owned())
}

/// The README file of an installed extension, parsed.
fn local_readme(root: &Path) -> Readme {
    let Ok(entries) = std::fs::read_dir(root) else { return Readme::Missing("This extension has no README.".to_owned()) };
    let file = entries.flatten().map(|e| e.path()).find(|p| {
        let name = p.file_name().map(|n| n.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
        p.is_file() && name.starts_with("readme") && matches!(p.extension().and_then(|e| e.to_str()).unwrap_or("md").to_ascii_lowercase().as_str(), "md" | "markdown" | "txt")
    });
    match file.and_then(|f| std::fs::read_to_string(f).ok()) {
        Some(text) => Readme::Ready(forge_core::markdown::parse(&text)),
        None => Readme::Missing("This extension has no README.".to_owned()),
    }
}

fn local_info(extension: &Extension) -> Details {
    let json = &extension.manifest_json;
    Details {
        license: json.get("license").and_then(|l| l.as_str().or_else(|| l.get("type").and_then(|t| t.as_str()))).map(str::to_owned),
        repository: url_of(json.get("repository")),
        homepage: url_of(json.get("homepage")),
        categories: json.get("categories").and_then(|c| c.as_array()).map(|a| a.iter().filter_map(|c| c.as_str().map(str::to_owned)).collect()).unwrap_or_default(),
        ..Details::default()
    }
}

impl Workspace {
    /// Show an extension's page in a tab of the focused group, or switch to its tab if it is open.
    fn open_ext_page(&mut self, detail: ExtDetail) {
        let doc = Doc::new_ext_page(detail);
        let ix = match self.doc_index(&doc.path) {
            Some(ix) => {
                self.docs[ix] = doc;
                ix
            }
            None => {
                self.docs.push(doc);
                self.docs.len() - 1
            }
        };
        let group = self.focused_group;
        match self.groups[group].views.iter().position(|v| v.doc == ix) {
            Some(vi) => self.groups[group].active = vi,
            None => {
                self.groups[group].views.push(View::new(ix));
                self.groups[group].active = self.groups[group].views.len() - 1;
            }
        }
        self.focus = Focus::Editor;
    }

    /// Close the tab of an extension's page, wherever it is open.
    fn close_ext_page(&mut self, id: &str, cx: &mut Context<Self>) {
        let path = PathBuf::from(format!("ext:{}", id.to_lowercase()));
        while let Some((g, vi)) = self.doc_index(&path).and_then(|ix| self.views_of(ix).next()) {
            self.close_view(g, vi, cx);
        }
    }

    /// Open the page of an installed extension: its README comes from its own folder.
    pub(crate) fn open_ext_detail_installed(&mut self, i: usize) {
        let Some(e) = self.extensions.get(i) else { return };
        let id = e.id();
        let (publisher, _) = id.split_once('.').unwrap_or(("", &id));
        let detail = ExtDetail {
            name: e.display_name(),
            publisher: publisher.to_owned(),
            version: e.manifest.version.clone(),
            description: e.manifest.description.clone().map(|d| e.localize(&d)).unwrap_or_default(),
            summary: Some(e.supported_summary()),
            listing: None,
            info: local_info(e),
            readme: local_readme(&e.root),
            base: e.root.clone(),
            list: ExtDetail::blank_list(),
            id,
        };
        self.open_ext_page(detail);
    }

    /// Open the page of an Open VSX search result; its README is fetched in the background.
    pub(crate) fn open_ext_detail_listing(&mut self, listing: forge_ext_host::openvsx::Listing) {
        let id = listing.id();
        let (tx, rx) = mpsc::channel();
        let wanted = id.clone();
        std::thread::spawn(move || {
            let _ = tx.send((wanted.clone(), forge_ext_host::openvsx::details(&wanted)));
        });
        self.ext_detail_rx.push(rx);
        self.open_ext_page(ExtDetail {
            name: listing.display_name.clone(),
            publisher: listing.namespace.clone(),
            version: listing.version.clone(),
            description: listing.description.clone(),
            summary: None,
            info: Details { downloads: listing.downloads, ..Details::default() },
            readme: Readme::Loading,
            base: PathBuf::new(),
            list: ExtDetail::blank_list(),
            listing: Some(listing),
            id,
        });
    }

    /// Fill the open pages whose details have arrived.
    pub(crate) fn poll_ext_detail(&mut self) -> bool {
        let mut changed = false;
        for rx in std::mem::take(&mut self.ext_detail_rx) {
            match rx.try_recv() {
                Ok((id, result)) => {
                    // The tab may have been closed while the fetch ran.
                    if let Some(detail) = self.docs.iter_mut().filter_map(|d| d.ext_page.as_mut()).find(|d| d.id == id) {
                        match result {
                            Ok(mut info) => {
                                detail.readme = match info.readme.take() {
                                    Some(text) => Readme::Ready(forge_core::markdown::parse(&text)),
                                    None => Readme::Missing("This extension has no README.".to_owned()),
                                };
                                detail.info = info;
                            }
                            Err(error) => detail.readme = Readme::Missing(format!("Could not load details from Open VSX: {error}")),
                        }
                        detail.sync_list();
                        changed = true;
                    }
                }
                Err(TryRecvError::Empty) => self.ext_detail_rx.push(rx),
                Err(TryRecvError::Disconnected) => {}
            }
        }
        changed
    }

    /// The body of an extension's tab, in place of the editor. Its rows are laid out lazily, so a long
    /// README costs only what is on screen.
    pub(crate) fn render_ext_page(&self, doc_ix: usize, cx: &mut Context<Self>) -> Option<AnyElement> {
        let d = self.docs.get(doc_ix)?.ext_page.as_deref()?;
        let (id, state) = (d.id.clone(), d.list.clone());
        let row = cx.processor(move |this: &mut Workspace, ix: usize, _: &mut Window, cx: &mut Context<Workspace>| this.ext_page_row(&id, ix, cx));
        Some(div().size_full().bg(rgb(self.theme.bg)).text_color(rgb(self.theme.text)).child(list(state, row).size_full()).into_any_element())
    }

    /// Row `ix` of the extension page `id`, centred in a column.
    fn ext_page_row(&self, id: &str, ix: usize, cx: &mut Context<Self>) -> AnyElement {
        let Some(d) = self.docs.iter().find_map(|doc| doc.ext_page.as_deref().filter(|p| p.id == id)) else { return div().into_any_element() };
        let t = &self.theme;
        let inner: AnyElement = if ix == 0 {
            self.ext_header(d, cx)
        } else if ix + 1 >= d.item_count() {
            div().h(rems(1.5)).into_any_element()
        } else {
            match &d.readme {
                Readme::Loading => div().py_4().text_color(rgb(t.dim)).child("Loading README from Open VSX...").into_any_element(),
                Readme::Missing(message) => div().py_4().text_color(rgb(t.dim)).child(message.clone()).into_any_element(),
                Readme::Ready(blocks) => match blocks.get(ix - 1) {
                    Some(b) => self.markdown_block(b, d.base.clone(), ix * 10_000, cx),
                    None => div().into_any_element(),
                },
            }
        };
        div().w_full().flex().justify_center().child(div().w_full().max_w(rems(52.0)).px(rems(2.0)).child(inner)).into_any_element()
    }

    /// The top of the page: icon, name, facts, and the Install or Uninstall button.
    fn ext_header(&self, d: &ExtDetail, cx: &mut Context<Self>) -> AnyElement {
        let t = self.theme.clone();
        let installed_ix = self.extensions.iter().position(|e| e.id().eq_ignore_ascii_case(&d.id));
        let busy = self.ext_installing.as_deref().is_some_and(|i| i.eq_ignore_ascii_case(&d.id));

        let button = |id: &'static str, label: &str, primary: bool| {
            div()
                .id(id)
                .flex_none()
                .h(rems(1.875))
                .px_4()
                .flex()
                .items_center()
                .justify_center()
                .rounded_sm()
                .cursor_pointer()
                .bg(rgb(if primary { t.button } else { t.tab_active }))
                .text_color(rgb(if primary { t.button_text } else { t.text }))
                .hover(|s| s.opacity(0.85))
                .child(label.to_owned())
        };
        let action: AnyElement = match (installed_ix, &d.listing) {
            (Some(ix), listing) => {
                let keep_open = listing.is_some();
                let id = d.id.clone();
                button("ext-detail-uninstall", "Uninstall", false)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.perform_ext_item(ItemAction::ExtInstalled(ix));
                        // An extension that is gone has nothing left to show, unless Open VSX still lists it.
                        if !keep_open {
                            this.close_ext_page(&id, cx);
                        }
                        cx.notify();
                    }))
                    .into_any_element()
            }
            (None, Some(_)) if busy => div().flex_none().text_color(rgb(t.dim)).child("Installing...").into_any_element(),
            (None, Some(listing)) => {
                let listing = listing.clone();
                button("ext-detail-install", "Install", true)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.start_ext_install(listing.clone());
                        cx.notify();
                    }))
                    .into_any_element()
            }
            (None, None) => div().into_any_element(),
        };

        let link = |id: &'static str, label: &str, url: &str, cx: &mut Context<Self>| -> AnyElement {
            let url = url.to_owned();
            div()
                .id(id)
                .flex_none()
                .cursor_pointer()
                .text_color(rgb(t.info))
                .hover(|s| s.underline())
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.open_markdown_link(&url, Path::new(""), cx);
                }))
                .child(label.to_owned())
                .into_any_element()
        };
        let mut facts: Vec<AnyElement> = Vec::new();
        if d.info.downloads > 0 {
            facts.push(div().child(format!("{} downloads", count(d.info.downloads))).into_any_element());
        }
        if let Some(rating) = d.info.rating.filter(|_| d.info.rating_count > 0) {
            facts.push(div().child(format!("★ {rating:.1} ({})", d.info.rating_count)).into_any_element());
        }
        if let Some(license) = &d.info.license {
            facts.push(div().child(format!("License: {license}")).into_any_element());
        }
        if let Some(url) = &d.info.repository {
            facts.push(link("ext-detail-repo", "Repository", url, cx));
        }
        if let Some(url) = d.info.homepage.as_ref().filter(|h| Some(*h) != d.info.repository.as_ref()) {
            facts.push(link("ext-detail-home", "Homepage", url, cx));
        }

        div()
            .flex()
            .flex_col()
            .w_full()
            .pt(rems(1.5))
            .gap_3()
            .child(
                div()
                    .flex()
                    .flex_row()
                    .gap_4()
                    .child(self.ext_tile(&d.name).w(rems(5.0)).h(rems(5.0)).text_size(rems(2.5)))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .flex_1()
                            .min_w_0()
                            .gap_1()
                            .child(div().flex().flex_row().items_baseline().gap_2().child(div().text_size(rems(1.625)).font_weight(FontWeight::BOLD).child(d.name.clone())).child(div().text_color(rgb(t.dim)).child(format!("v{}", d.version))))
                            .child(div().text_color(rgb(t.dim)).child(format!("{}  ·  {}", d.publisher, d.id)))
                            .child(div().child(d.description.clone()))
                            .child(div().flex().flex_row().flex_wrap().gap_x_4().text_xs().text_color(rgb(t.dim)).children(facts))
                            .children(d.summary.as_ref().map(|s| div().text_xs().text_color(rgb(t.dim)).child(format!("Forge uses: {s}"))))
                            .children((!d.info.categories.is_empty()).then(|| {
                                div().flex().flex_row().flex_wrap().gap_1().pt_1().children(d.info.categories.iter().map(|c| div().px_2().py(rems(0.125)).rounded_full().bg(rgb(t.tab_active)).text_xs().child(c.clone())))
                            }))
                            .child(div().flex().flex_row().items_center().gap_2().pt_2().child(action)),
                    ),
            )
            .child(div().w_full().h(px(1.0)).bg(rgb(t.border)))
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::{count, url_of};

    #[test]
    fn repository_urls_come_from_strings_or_objects() {
        let object = serde_json::json!({ "type": "git", "url": "git+https://github.com/a/b.git" });
        assert_eq!(url_of(Some(&object)).as_deref(), Some("https://github.com/a/b.git"));
        assert_eq!(url_of(Some(&serde_json::json!("https://x.dev"))).as_deref(), Some("https://x.dev"));
        assert_eq!(url_of(None), None);
    }

    #[test]
    fn download_counts_are_short() {
        assert_eq!(count(7), "7");
        assert_eq!(count(1_234), "1.2K");
        assert_eq!(count(2_500_000), "2.5M");
    }
}
