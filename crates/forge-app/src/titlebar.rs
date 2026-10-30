//! Colours the window's title bar to match the theme instead of the Windows accent colour. This needs
//! Windows 11; earlier versions ignore the request and keep their own title bar.

use forge_core::theme::Theme;

/// What the title bar was last painted for, so it is only set again when the theme changes.
pub type Key = (u32, u32, u32);

pub fn key(theme: &Theme) -> Key {
    (theme.sidebar, theme.text, theme.border)
}

/// The bar's colour, its text and its outline from `theme`. Dark themes also get the dark window buttons.
#[cfg(windows)]
pub fn apply(window: &gpui::Window, theme: &Theme) {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};

    #[link(name = "dwmapi")]
    extern "system" {
        fn DwmSetWindowAttribute(hwnd: isize, attribute: u32, value: *const core::ffi::c_void, size: u32) -> i32;
    }
    const DARK_MODE: u32 = 20;
    const BORDER_COLOR: u32 = 34;
    const CAPTION_COLOR: u32 = 35;
    const TEXT_COLOR: u32 = 36;

    let Ok(handle) = HasWindowHandle::window_handle(window) else { return };
    let RawWindowHandle::Win32(win) = handle.as_raw() else { return };
    let hwnd = win.hwnd.get();
    // COLORREF is 0x00BBGGRR.
    let colorref = |rgb: u32| ((rgb & 0xFF) << 16) | (rgb & 0xFF00) | ((rgb >> 16) & 0xFF);
    let luminance = |rgb: u32| ((rgb >> 16) & 0xFF) * 299 + ((rgb >> 8) & 0xFF) * 587 + (rgb & 0xFF) * 114;
    let dark: u32 = (luminance(theme.sidebar) < 128_000) as u32;
    let set = |attribute: u32, value: u32| unsafe {
        DwmSetWindowAttribute(hwnd, attribute, &value as *const u32 as *const core::ffi::c_void, 4);
    };
    set(DARK_MODE, dark);
    set(CAPTION_COLOR, colorref(theme.sidebar));
    set(TEXT_COLOR, colorref(theme.text));
    set(BORDER_COLOR, colorref(theme.border));
}

#[cfg(not(windows))]
pub fn apply(_window: &gpui::Window, _theme: &Theme) {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_key_changes_with_the_theme() {
        assert_ne!(key(&Theme::dark()), key(&Theme::light()));
    }
}
