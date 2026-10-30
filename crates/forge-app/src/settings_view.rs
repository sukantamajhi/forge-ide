//! The Settings page: a panel over the editor with a control for each common setting. Every
//! change is written straight into `settings.json` (keeping its comments) and applied live.

use forge_core::commands::Command;
use forge_core::input::{LineInput, Outcome};
use forge_core::settings::{config_dir, set_value, Settings};
use forge_core::theme::{Theme, THEME_NAMES};
use gpui::{div, prelude::*, rems, rgb, AnyElement, Context, Div, FontWeight, Keystroke, MouseButton, MouseDownEvent};

use crate::workspace::{Focus, Workspace};

/// A row: the setting's name and what it does on the left, its control on the right.
fn row(t: &Theme, title: &str, description: &str, control: AnyElement) -> AnyElement {
    div()
        .flex()
        .flex_row()
        .items_center()
        .justify_between()
        .gap_4()
        .py(rems(0.75))
        .border_b_1()
        .border_color(rgb(t.border))
        .child(
            div()
                .flex()
                .flex_col()
                .gap(rems(0.125))
                .min_w_0()
                .child(div().font_weight(FontWeight::MEDIUM).child(title.to_owned()))
                .child(div().text_xs().text_color(rgb(t.dim)).child(description.to_owned())),
        )
        .child(div().flex_none().child(control))
        .into_any_element()
}

fn heading(t: &Theme, text: &str) -> Div {
    div().pt(rems(1.5)).pb(rems(0.25)).text_xs().font_weight(FontWeight::BOLD).text_color(rgb(t.info)).child(text.to_uppercase())
}

impl Workspace {
    /// Open the Settings page.
    pub fn open_settings_page(&mut self) {
        self.settings_model = LineInput::new(self.settings.active_ai_model());
        self.settings_model_focus = false;
        self.settings_open = true;
    }

    /// Write one setting into `settings.json` and apply it now.
    pub fn set_setting(&mut self, key: &str, json_value: &str) {
        let path = config_dir().join("settings.json");
        let current = std::fs::read_to_string(&path).unwrap_or_else(|_| Settings::template());
        let updated = set_value(&current, key, json_value);
        let written = std::fs::create_dir_all(config_dir()).and_then(|()| std::fs::write(&path, updated));
        if let Err(error) = written {
            self.set_status(format!("Cannot save settings: {error}"));
            return;
        }
        self.reload_config_if_needed(&path);
        self.reload_clean_docs();
    }

    fn step_setting(&mut self, key: &str, current: f64, delta: f64, min: f64, max: f64) {
        let next = (current + delta).clamp(min, max);
        self.set_setting(key, &format!("{next}"));
    }

    /// Keys while the Settings page is open. True when the key was used (or must not reach the editor).
    pub fn settings_key(&mut self, k: &Keystroke) -> bool {
        let ctrl = k.modifiers.control || k.modifiers.platform;
        if k.key == "escape" {
            if self.settings_model_focus {
                self.settings_model_focus = false;
                self.settings_model = LineInput::new(self.settings.active_ai_model());
            } else {
                self.settings_open = false;
            }
            return true;
        }
        if self.settings_model_focus {
            match self.settings_model.apply(&k.key, ctrl, k.key_char.as_deref()) {
                Outcome::Submit => {
                    let model = self.settings_model.text().trim().to_owned();
                    if !model.is_empty() {
                        let json = serde_json::to_string(&model).unwrap_or_default();
                        self.set_setting(self.settings.active_ai_model_key(), &json);
                    }
                    self.settings_model_focus = false;
                    true
                }
                Outcome::Ignored => false,
                _ => true,
            }
        } else {
            // Shortcuts still work; plain typing must not land in the editor behind the page.
            !ctrl && !k.modifiers.alt
        }
    }

    pub fn render_settings(&self, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        if !self.settings_open {
            return None;
        }
        let t = self.theme.clone();
        let s = self.settings.clone();
        let left = if self.sidebar_visible { self.sidebar_width() } else { 0.0 };

        let button = |id: &'static str, label: &'static str, cx: &mut Context<Self>, f: Box<dyn Fn(&mut Workspace, &mut Context<Workspace>)>| {
            div()
                .id(id)
                .h(rems(1.75))
                .min_w(rems(1.75))
                .px_2()
                .flex()
                .items_center()
                .justify_center()
                .rounded_sm()
                .cursor_pointer()
                .border_1()
                .border_color(rgb(t.border))
                .hover(|s| s.bg(rgb(t.tab_active)))
                .on_click(cx.listener(move |this, _, _, cx| {
                    f(this, cx);
                    cx.notify();
                }))
                .child(label)
        };
        let stepper = |key: &'static str, value: f64, step: f64, min: f64, max: f64, id: usize, cx: &mut Context<Self>| {
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap_2()
                .child(button(["dec-a", "dec-b"][id], "−", cx, Box::new(move |w, _| w.step_setting(key, value, -step, min, max))))
                .child(div().min_w(rems(2.5)).flex().justify_center().child(format!("{value}")))
                .child(button(["inc-a", "inc-b"][id], "+", cx, Box::new(move |w, _| w.step_setting(key, value, step, min, max))))
                .into_any_element()
        };
        let toggle = |id: usize, on: bool, on_json: &'static str, off_json: &'static str, key: &'static str, cx: &mut Context<Self>| {
            div()
                .id(("setting-toggle", id))
                .w(rems(2.5))
                .h(rems(1.375))
                .rounded_full()
                .cursor_pointer()
                .flex()
                .items_center()
                .px(rems(0.125))
                .when(on, |d| d.justify_end())
                .bg(rgb(if on { t.button } else { t.border }))
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.set_setting(key, if on { off_json } else { on_json });
                    cx.notify();
                }))
                .child(div().w(rems(1.125)).h(rems(1.125)).rounded_full().bg(rgb(if on { t.button_text } else { t.dim })))
                .into_any_element()
        };

        let theme_chips = div().flex().flex_row().gap_2().children(THEME_NAMES.iter().enumerate().map(|(i, name)| {
            let selected = s.theme == *name;
            let name = (*name).to_owned();
            div()
                .id(("theme-chip", i))
                .px_3()
                .h(rems(1.75))
                .flex()
                .items_center()
                .rounded_sm()
                .cursor_pointer()
                .border_1()
                .border_color(rgb(if selected { t.info } else { t.border }))
                .bg(rgb(if selected { t.selection } else { t.bg }))
                .on_click(cx.listener(move |this, _, _, cx| {
                    let json = serde_json::to_string(&name).unwrap_or_default();
                    this.set_setting("workbench.colorTheme", &json);
                    cx.notify();
                }))
                .child(THEME_NAMES[i])
        }));

        let model_focused = self.settings_model_focus;
        let model_input = div()
            .w(rems(14.0))
            .on_mouse_down(MouseButton::Left, cx.listener(|this, _: &MouseDownEvent, _, cx| {
                this.settings_model_focus = true;
                cx.stop_propagation();
                cx.notify();
            }))
            .child(self.render_input(&self.settings_model, s.ai_provider.default_model(), model_focused))
            .into_any_element();
        let provider_chips = div().flex().flex_row().gap_2().children(forge_core::ai::Provider::ALL.into_iter().enumerate().map(|(i, provider)| {
            let selected = s.ai_provider == provider;
            div()
                .id(("ai-provider", i))
                .px_3()
                .h(rems(1.75))
                .flex()
                .items_center()
                .rounded_sm()
                .cursor_pointer()
                .border_1()
                .border_color(rgb(if selected { t.info } else { t.border }))
                .bg(rgb(if selected { t.selection } else { t.bg }))
                .on_click(cx.listener(move |this, _, _, cx| {
                    let json = serde_json::to_string(provider.id()).unwrap_or_default();
                    this.set_setting("ai.provider", &json);
                    // The model box shows the chosen provider's model.
                    this.settings_model = LineInput::new(this.settings.active_ai_model());
                    this.settings_model_focus = false;
                    cx.notify();
                }))
                .child(provider.label())
        }));

        let auto_closing = s.auto_closing_brackets;
        let content = div()
            .flex()
            .flex_col()
            .w_full()
            .max_w(rems(46.0))
            .px(rems(2.0))
            .pt(rems(1.5))
            // The page's scroll range is measured from this box, and wrapped descriptions paint taller than they
            // measure, so without this room the last rows stay out of reach.
            .pb(rems(8.0))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .justify_between()
                    .child(div().text_size(rems(1.5)).font_weight(FontWeight::BOLD).child("Settings"))
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .gap_2()
                            .child(button("settings-json", "Open settings.json", cx, Box::new(|w, cx| {
                                w.settings_open = false;
                                w.run_command(Command::OpenSettingsJson, false, cx);
                            })))
                            .child(button("settings-close", "×", cx, Box::new(|w, _| w.settings_open = false))),
                    ),
            )
            .child(div().text_xs().text_color(rgb(t.dim)).pt(rems(0.25)).child("Changes are saved to settings.json and apply immediately."))
            .child(heading(&t, "Editor"))
            .child(row(&t, "Font Size", "Editor text size in pixels (8 to 40). Ctrl+= and Ctrl+- zoom temporarily.", stepper("editor.fontSize", f64::from(s.font_size), 1.0, 8.0, 40.0, 0, cx)))
            .child(row(&t, "Tab Size", "Columns per tab stop (1 to 16).", stepper("editor.tabSize", s.tab_size as f64, 1.0, 1.0, 16.0, 1, cx)))
            .child(row(&t, "Insert Spaces", "Pressing Tab inserts spaces instead of a tab character.", toggle(0, s.insert_spaces, "true", "false", "editor.insertSpaces", cx)))
            .child(row(&t, "Auto Closing Brackets", "Type a closing bracket or quote for you.", toggle(1, auto_closing, "\"always\"", "\"never\"", "editor.autoClosingBrackets", cx)))
            .child(row(&t, "Trim Trailing Whitespace", "Remove spaces at the end of lines when saving.", toggle(2, s.trim_trailing_whitespace, "true", "false", "files.trimTrailingWhitespace", cx)))
            .child(row(&t, "Format On Save", "Run the language server's formatter when saving.", toggle(3, s.format_on_save, "true", "false", "editor.formatOnSave", cx)))
            .child(row(&t, "Word Wrap", "Wrap long lines at the editor's width. Alt+Z toggles it for this session.", toggle(5, s.word_wrap, "\"on\"", "\"off\"", "editor.wordWrap", cx)))
            .child(row(&t, "Minimap", "Show a small overview of the file beside the editor. Click or drag it to scroll.", toggle(6, s.minimap, "true", "false", "editor.minimap.enabled", cx)))
            .child(heading(&t, "Workbench"))
            .child(row(&t, "Color Theme", "Extension themes are chosen in the command palette (Color Theme).", theme_chips.into_any_element()))
            .child(row(&t, "File Icon Theme", "Install one from Extensions (try \"Material Icon Theme\"), then pick it in the command palette (File Icon Theme).", div().child(s.icon_theme.clone()).into_any_element()))
            .child(heading(&t, "Language Servers"))
            .child(row(&t, "Start Automatically", "Start the servers for your project's languages when a folder opens.", toggle(4, s.lsp_autostart, "true", "false", "lsp.autoStart", cx)))
            .child(heading(&t, "AI Commit Messages"))
            .child(row(&t, "Provider", "Who writes commit messages. Gemini is called through its OpenAI-compatible endpoint, so both use the same request.", provider_chips.into_any_element()))
            .child(row(&t, "Model", &format!("The {} model that writes commit messages. Press Enter to save.", s.ai_provider.label()), model_input))
            .child(row(&t, &format!("{} API Key", s.ai_provider.label()), &format!("Saved once, encrypted for your Windows account. Get one at {}.", s.ai_provider.key_page()), div().flex().flex_row().gap_2()
                .child(button("settings-key-set", "Enter key...", cx, Box::new(|w, cx| {
                    w.settings_open = false;
                    w.run_command(Command::AiSetKey, false, cx);
                })))
                .child(button("settings-key-forget", "Forget key", cx, Box::new(|w, _| w.forget_ai_key())))
                .into_any_element()));

        Some(
            crate::scroll::overlay_page("settings-page", left, t.bg, t.text)
                .on_mouse_down(MouseButton::Left, cx.listener(|this, _: &MouseDownEvent, _, cx| {
                    this.settings_model_focus = false;
                    this.focus = Focus::Editor;
                    cx.stop_propagation();
                    cx.notify();
                }))
                .child(content),
        )
    }
}
