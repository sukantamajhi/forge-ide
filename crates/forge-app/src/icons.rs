//! Icons and tooltips. The icons are glyphs of VS Code's Codicons font (CC BY 4.0), embedded in the
//! program and registered at startup; a tooltip is the small label that appears when the pointer
//! rests on an icon button, naming it and giving its shortcut.

use std::borrow::Cow;

use gpui::{div, prelude::*, rems, rgb, AnyView, App, Context, Div, SharedString, Window};

/// The font file, compiled in so there is nothing to install or lose.
static CODICON_FONT: &[u8] = include_bytes!("../../../assets/fonts/codicon.ttf");

/// The font family name of Codicons.
pub const FAMILY: &str = "codicon";

/// Make the icon font available to the text system. Call once at startup.
pub fn register_font(cx: &mut App) {
    if let Err(error) = cx.text_system().add_fonts(vec![Cow::Borrowed(CODICON_FONT)]) {
        eprintln!("[icons] could not load the icon font: {error}");
    }
}

/// One icon, `size` rems tall, in `color`, centred in a box of its own size so it lines up.
pub fn glyph(icon: char, size: f32, color: u32) -> Div {
    div().flex().flex_none().items_center().justify_center().w(rems(size)).h(rems(size)).font_family(FAMILY).text_size(rems(size)).line_height(rems(size)).text_color(rgb(color)).child(icon.to_string())
}

/// The colours a tooltip is drawn with.
#[derive(Clone, Copy)]
pub struct TipColors {
    pub background: u32,
    pub text: u32,
    pub border: u32,
}

struct Tip {
    text: SharedString,
    colors: TipColors,
}

impl Render for Tip {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .px_2()
            .py_1()
            .rounded_sm()
            .bg(rgb(self.colors.background))
            .border_1()
            .border_color(rgb(self.colors.border))
            .text_color(rgb(self.colors.text))
            .text_xs()
            .child(self.text.clone())
    }
}

/// The builder `.tooltip(...)` wants, for a label such as "Source Control (Ctrl+Shift+G)".
pub fn tip(text: impl Into<SharedString>, colors: TipColors) -> impl Fn(&mut Window, &mut App) -> AnyView + 'static {
    let text = text.into();
    move |_, cx| cx.new(|_| Tip { text: text.clone(), colors }).into()
}

/// "Name (Shortcut)", or just "Name" when the action has no shortcut.
pub fn titled(name: &str, shortcut: Option<String>) -> String {
    match shortcut {
        Some(keys) => format!("{name} ({keys})"),
        None => name.to_owned(),
    }
}

impl crate::workspace::Workspace {
    pub fn tip_colors(&self) -> TipColors {
        TipColors { background: self.theme.popup, text: self.theme.text, border: self.theme.border }
    }

    /// A tooltip for an icon button: its name, and the shortcut of `command` if it has one.
    pub fn tooltip(&self, name: &str, command: Option<forge_core::commands::Command>) -> impl Fn(&mut Window, &mut App) -> AnyView + 'static {
        tip(titled(name, command.and_then(|c| self.keymap.shortcut_label(c))), self.tip_colors())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_tooltip_names_the_action_and_its_shortcut_when_it_has_one() {
        assert_eq!(titled("Source Control", Some("Ctrl+Shift+G".into())), "Source Control (Ctrl+Shift+G)");
        assert_eq!(titled("Refresh", None), "Refresh");
    }

    #[test]
    fn the_font_is_embedded_and_looks_like_a_truetype_file() {
        assert!(CODICON_FONT.len() > 50_000);
        assert_eq!(&CODICON_FONT[..4], &[0x00, 0x01, 0x00, 0x00], "TrueType header");
    }
}
