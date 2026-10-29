//! Loading VS Code color themes (the JSON files extensions contribute) as Forge [`Theme`]s.
//!
//! A VS Code theme has `colors` (workbench colors such as `editor.background`) and
//! `tokenColors` (TextMate scope rules). Forge's UI uses a smaller set of colors and a
//! fixed set of token classes, so the loader maps one onto the other: each token class asks
//! for a few TextMate scopes in order of preference and takes the color of the most specific
//! rule that matches. Anything a theme leaves out keeps the built-in theme's color, chosen
//! by the theme's kind (light, dark, high contrast).

use std::collections::BTreeMap;
use std::path::{Component, Path, PathBuf};

use forge_syntax::HighlightClass;
use serde_json::Value;

use crate::jsonc;
use crate::theme::Theme;

/// How deep `include` may chain, which also stops include cycles.
const MAX_INCLUDE_DEPTH: usize = 6;

struct Rule {
    scope: String,
    foreground: Option<u32>,
}

#[derive(Default)]
struct Parts {
    colors: BTreeMap<String, String>,
    rules: Vec<Rule>,
    /// `type` from the theme file: `dark`, `light`, `hc`...
    kind: Option<String>,
}

/// Read a theme file (following its `include`s) and build a [`Theme`] named `label`.
/// `ui_theme` is the manifest's `uiTheme` (`vs`, `vs-dark`, `hc-black`), which picks the
/// built-in theme used for anything the file does not set.
pub fn load_theme_file(path: &Path, label: &str, ui_theme: Option<&str>) -> Result<Theme, String> {
    let mut parts = Parts::default();
    collect(path, 0, &mut parts)?;
    Ok(build(label, ui_theme, parts))
}

/// Like [`load_theme_file`] for theme text that has no `include`s.
pub fn theme_from_json(json: &str, label: &str, ui_theme: Option<&str>) -> Result<Theme, String> {
    let mut parts = Parts::default();
    merge(&jsonc::parse(json)?, None, 0, &mut parts)?;
    Ok(build(label, ui_theme, parts))
}

fn collect(path: &Path, depth: usize, parts: &mut Parts) -> Result<(), String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let value = jsonc::parse(&text).map_err(|e| format!("{}: {e}", path.display()))?;
    merge(&value, path.parent(), depth, parts)
}

/// Fold one theme file into `parts`: an included file comes first, so the includer overrides it.
fn merge(value: &Value, dir: Option<&Path>, depth: usize, parts: &mut Parts) -> Result<(), String> {
    if let Some(include) = value.get("include").and_then(Value::as_str) {
        if depth >= MAX_INCLUDE_DEPTH {
            return Err("theme includes are nested too deeply".to_owned());
        }
        let relative = Path::new(include);
        // An include stays inside the folder of the file that names it.
        let confined = relative.components().all(|c| matches!(c, Component::Normal(_) | Component::CurDir));
        match (confined, dir) {
            (true, Some(dir)) => collect(&dir.join(relative), depth + 1, parts)?,
            _ => return Err(format!("cannot include {include:?}")),
        }
    }
    if let Some(kind) = value.get("type").and_then(Value::as_str) {
        parts.kind = Some(kind.to_owned());
    }
    if let Some(colors) = value.get("colors").and_then(Value::as_object) {
        for (key, color) in colors {
            if let Some(color) = color.as_str() {
                parts.colors.insert(key.clone(), color.to_owned());
            }
        }
    }
    // Older themes name a .tmTheme file here instead of listing rules; those are not read.
    if let Some(rules) = value.get("tokenColors").and_then(Value::as_array) {
        for rule in rules {
            let foreground = rule.get("settings").and_then(|s| s.get("foreground")).and_then(Value::as_str).and_then(parse_color).map(|(rgb, _)| rgb);
            let scopes: Vec<String> = match rule.get("scope") {
                Some(Value::String(s)) => s.split(',').map(|p| p.trim().to_owned()).collect(),
                Some(Value::Array(a)) => a.iter().filter_map(Value::as_str).flat_map(|s| s.split(',')).map(|p| p.trim().to_owned()).collect(),
                // A rule without a scope sets the default text color.
                _ => vec![String::new()],
            };
            for scope in scopes {
                parts.rules.push(Rule { scope, foreground });
            }
        }
    }
    Ok(())
}

/// `#rgb`, `#rgba`, `#rrggbb` or `#rrggbbaa` as `(0xRRGGBB, alpha 0..=255)`.
pub fn parse_color(text: &str) -> Option<(u32, u8)> {
    let hex = text.trim().strip_prefix('#')?;
    if !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let digit = |i: usize| u32::from_str_radix(&hex[i..=i], 16).ok();
    let pair = |i: usize| u32::from_str_radix(&hex[i..i + 2], 16).ok();
    match hex.len() {
        3 => Some((((digit(0)? * 17) << 16) | ((digit(1)? * 17) << 8) | (digit(2)? * 17), 255)),
        4 => Some((((digit(0)? * 17) << 16) | ((digit(1)? * 17) << 8) | (digit(2)? * 17), (digit(3)? * 17) as u8)),
        6 => Some(((pair(0)? << 16) | (pair(2)? << 8) | pair(4)?, 255)),
        8 => Some(((pair(0)? << 16) | (pair(2)? << 8) | pair(4)?, pair(6)? as u8)),
        _ => None,
    }
}

/// `color` drawn at `alpha` over `under`.
fn blend(color: u32, alpha: u8, under: u32) -> u32 {
    if alpha == 255 {
        return color;
    }
    let mix = |shift: u32| {
        let (c, u) = ((color >> shift) & 0xff, (under >> shift) & 0xff);
        (c * alpha as u32 + u * (255 - alpha as u32) + 127) / 255
    };
    (mix(16) << 16) | (mix(8) << 8) | mix(0)
}

fn luminance(rgb: u32) -> u32 {
    (((rgb >> 16) & 0xff) * 299 + ((rgb >> 8) & 0xff) * 587 + (rgb & 0xff) * 114) / 1000
}

/// The best color for the first of `keys` the theme defines, over `under`.
fn pick(colors: &BTreeMap<String, String>, keys: &[&str], under: u32) -> Option<u32> {
    keys.iter().find_map(|key| colors.get(*key).and_then(|c| parse_color(c)).map(|(rgb, alpha)| blend(rgb, alpha, under)))
}

/// Scopes each token class looks up, most preferred first.
fn class_scopes(class: HighlightClass) -> &'static [&'static str] {
    use HighlightClass::*;
    match class {
        Keyword => &["keyword.control", "keyword", "storage.type", "storage"],
        Function => &["entity.name.function", "support.function", "meta.function-call", "entity.name"],
        Type => &["entity.name.type", "entity.name.class", "support.class", "support.type"],
        String => &["string", "string.quoted"],
        Escape => &["constant.character.escape", "string"],
        Number => &["constant.numeric", "constant"],
        Constant => &["constant.language", "support.constant", "constant"],
        Comment => &["comment"],
        Property => &["variable.other.property", "support.type.property-name", "meta.object-literal.key", "variable.other.object.property"],
        Attribute => &["entity.other.attribute-name"],
        Operator => &["keyword.operator", "keyword"],
        Punctuation => &["punctuation", "meta.brace"],
        Variable => &["variable", "variable.other"],
        Tag => &["entity.name.tag", "meta.tag"],
    }
}

/// The color of the most specific rule matching `scope`: a rule scope `a.b` matches
/// `a.b` and `a.b.c`. Later rules win ties. Rules with descendant selectors (`a b`) or
/// exclusions are not understood and are skipped.
fn color_for_scope(rules: &[Rule], scope: &str) -> Option<u32> {
    let mut best: Option<(usize, u32)> = None;
    for rule in rules {
        let Some(color) = rule.foreground else { continue };
        let target = rule.scope.as_str();
        if target.is_empty() || target.contains([' ', '-', '|', '(']) {
            continue;
        }
        let matches = scope == target || (scope.starts_with(target) && scope.as_bytes().get(target.len()) == Some(&b'.'));
        if matches && best.is_none_or(|(len, _)| target.len() >= len) {
            best = Some((target.len(), color));
        }
    }
    best.map(|(_, color)| color)
}

fn build(label: &str, ui_theme: Option<&str>, parts: Parts) -> Theme {
    let colors = &parts.colors;
    let declared_bg = pick(colors, &["editor.background"], 0);
    let kind = match (ui_theme, parts.kind.as_deref()) {
        (Some("vs"), _) | (_, Some("light")) => "light",
        (Some("hc-black" | "hc-light"), _) | (_, Some("hc" | "hc-black" | "hc-light")) => "hc",
        (Some("vs-dark"), _) | (_, Some("dark")) => "dark",
        _ => match declared_bg {
            Some(bg) if luminance(bg) > 140 => "light",
            _ => "dark",
        },
    };
    let mut theme = match kind {
        "light" => Theme::light(),
        "hc" => Theme::high_contrast(),
        _ => Theme::dark(),
    };
    theme.name = label.to_owned();

    theme.bg = declared_bg.unwrap_or(theme.bg);
    let bg = theme.bg;
    let set = |field: &mut u32, keys: &[&str], under: u32| {
        if let Some(color) = pick(colors, keys, under) {
            *field = color;
        }
    };
    set(&mut theme.sidebar, &["sideBar.background", "activityBar.background"], bg);
    set(&mut theme.tab_active, &["tab.activeBackground", "editorGroupHeader.tabsBackground"], bg);
    set(&mut theme.border, &["sideBar.border", "panel.border", "tab.border", "editorGroupHeader.tabsBorder", "editorGroup.border"], bg);
    set(&mut theme.text, &["editor.foreground", "foreground"], bg);
    set(&mut theme.dim, &["editorLineNumber.foreground", "descriptionForeground", "sideBar.foreground"], bg);
    set(&mut theme.caret, &["editorCursor.foreground"], bg);
    set(&mut theme.selection, &["editor.selectionBackground", "selection.background"], bg);
    set(&mut theme.accent, &["activityBar.activeBorder", "focusBorder", "button.background", "textLink.foreground"], bg);
    set(&mut theme.error, &["editorError.foreground", "errorForeground"], bg);
    set(&mut theme.warning, &["editorWarning.foreground"], bg);
    set(&mut theme.info, &["editorInfo.foreground"], bg);
    set(&mut theme.find_match, &["editor.findMatchHighlightBackground"], bg);
    set(&mut theme.find_current, &["editor.findMatchBackground"], bg);
    set(&mut theme.bracket_match, &["editorBracketMatch.background"], bg);
    set(&mut theme.line_highlight, &["editor.lineHighlightBackground"], bg);
    set(&mut theme.popup, &["editorWidget.background", "editorSuggestWidget.background"], bg);
    set(&mut theme.git_added, &["gitDecoration.addedResourceForeground", "editorGutter.addedBackground"], bg);
    set(&mut theme.git_modified, &["gitDecoration.modifiedResourceForeground", "editorGutter.modifiedBackground"], bg);
    set(&mut theme.git_deleted, &["gitDecoration.deletedResourceForeground", "editorGutter.deletedBackground"], bg);

    // Plain text and punctuation fall back to the default text color rather than the
    // built-in theme's, which may belong to a different kind of theme.
    let default_text = color_for_scope(&parts.rules.iter().map(|r| Rule { scope: if r.scope.is_empty() { "*".to_owned() } else { r.scope.clone() }, foreground: r.foreground }).collect::<Vec<_>>(), "*").unwrap_or(theme.text);
    for class in ALL_CLASSES {
        let found = class_scopes(*class).iter().find_map(|scope| color_for_scope(&parts.rules, scope));
        let color = match (found, class) {
            (Some(color), _) => color,
            (None, HighlightClass::Variable | HighlightClass::Punctuation) => default_text,
            (None, _) => theme.syntax_color(*class),
        };
        theme.set_syntax_color(*class, color);
    }
    theme
}

const ALL_CLASSES: &[HighlightClass] = &[
    HighlightClass::Keyword,
    HighlightClass::Function,
    HighlightClass::Type,
    HighlightClass::String,
    HighlightClass::Escape,
    HighlightClass::Number,
    HighlightClass::Constant,
    HighlightClass::Comment,
    HighlightClass::Property,
    HighlightClass::Attribute,
    HighlightClass::Operator,
    HighlightClass::Punctuation,
    HighlightClass::Variable,
    HighlightClass::Tag,
];

/// Where `relative` (a manifest path) points inside `root`, refusing anything that escapes it.
pub fn confined_path(root: &Path, relative: &str) -> Option<PathBuf> {
    let path = Path::new(relative);
    path.components().all(|c| matches!(c, Component::Normal(_) | Component::CurDir)).then(|| root.join(path))
}

#[cfg(test)]
mod tests {
    use super::*;

    const DRACULA_LIKE: &str = r##"{
        // comments and trailing commas are normal in theme files
        "name": "Night",
        "colors": {
            "editor.background": "#282A36",
            "editor.foreground": "#F8F8F2",
            "sideBar.background": "#21222C",
            "editor.selectionBackground": "#FFFFFF80",
            "editorCursor.foreground": "#F8F8F0",
            "editorError.foreground": "#FF5555",
            "editorGroup.border": "#BD93F9",
            "sideBar.border": "#191A21",
        },
        "tokenColors": [
            { "scope": ["comment"], "settings": { "foreground": "#6272A4" } },
            { "scope": "string, string.quoted", "settings": { "foreground": "#F1FA8C" } },
            { "scope": ["keyword", "storage"], "settings": { "foreground": "#FF79C6" } },
            { "scope": ["keyword.operator"], "settings": { "foreground": "#FFB86C" } },
            { "scope": ["entity.name.function"], "settings": { "foreground": "#50FA7B" } },
            { "scope": ["constant.numeric"], "settings": { "foreground": "#BD93F9" } },
            { "scope": ["emphasis"], "settings": { "fontStyle": "italic" } },
            { "scope": "markup.heading entity.name", "settings": { "foreground": "#123456" } },
        ],
    }"##;

    #[test]
    fn colors_parse_in_every_css_form() {
        assert_eq!(parse_color("#fff"), Some((0xffffff, 255)));
        assert_eq!(parse_color("#1a2"), Some((0x11aa22, 255)));
        assert_eq!(parse_color("#1a2b"), Some((0x11aa22, 0xbb)));
        assert_eq!(parse_color("#282A36"), Some((0x282a36, 255)));
        assert_eq!(parse_color("#FFB86C80"), Some((0xffb86c, 0x80)));
        for bad in ["", "#", "#12", "#12345", "282a36", "#gg0000", "red", "#1234567"] {
            assert_eq!(parse_color(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn translucent_colors_blend_over_what_is_behind_them() {
        assert_eq!(blend(0xffffff, 255, 0x000000), 0xffffff);
        assert_eq!(blend(0xffffff, 0, 0x102030), 0x102030);
        assert_eq!(blend(0xffffff, 128, 0x000000), 0x808080);
    }

    #[test]
    fn a_theme_maps_workbench_colors_and_token_rules() {
        let theme = theme_from_json(DRACULA_LIKE, "Night", Some("vs-dark")).unwrap();
        assert_eq!(theme.name, "Night");
        assert_eq!((theme.bg, theme.sidebar, theme.text, theme.caret, theme.error), (0x282a36, 0x21222c, 0xf8f8f2, 0xf8f8f0, 0xff5555));
        // The selection is half-transparent white over the editor background.
        assert_eq!(theme.selection, blend(0xffffff, 0x80, 0x282a36));
        // The sidebar border beats the loud editor-group border for general dividers.
        assert_eq!(theme.border, 0x191a21);
        use HighlightClass::*;
        assert_eq!(theme.syntax_color(Comment), 0x6272a4);
        assert_eq!(theme.syntax_color(String), 0xf1fa8c);
        assert_eq!(theme.syntax_color(Function), 0x50fa7b);
        assert_eq!(theme.syntax_color(Number), 0xbd93f9);
        // `keyword.control` has no rule of its own, so the `keyword` rule applies.
        assert_eq!(theme.syntax_color(Keyword), 0xff79c6);
        // `keyword.operator` is more specific than `keyword`.
        assert_eq!(theme.syntax_color(Operator), 0xffb86c);
        // Descendant selectors are skipped, and unset classes keep the built-in color.
        assert_ne!(theme.syntax_color(Type), 0x123456);
        assert_eq!(theme.syntax_color(Type), Theme::dark().syntax_color(Type));
        // Variables and punctuation use the theme's own text color, not the built-in one.
        assert_eq!(theme.syntax_color(Variable), 0xf8f8f2);
    }

    #[test]
    fn the_kind_of_theme_picks_the_fallbacks() {
        let light_json = r##"{ "colors": { "editor.background": "#ffffff" } }"##;
        assert_eq!(theme_from_json(light_json, "x", None).unwrap().sidebar, Theme::light().sidebar, "guessed from the background");
        assert_eq!(theme_from_json(light_json, "x", Some("vs-dark")).unwrap().sidebar, Theme::dark().sidebar, "the manifest wins");
        assert_eq!(theme_from_json("{}", "x", Some("hc-black")).unwrap().border, Theme::high_contrast().border);
        assert_eq!(theme_from_json(r##"{ "type": "light" }"##, "x", None).unwrap().bg, Theme::light().bg);
        // An empty theme is the built-in one under another name.
        let empty = theme_from_json("{}", "Plain", None).unwrap();
        assert_eq!(empty.name, "Plain");
        assert_eq!(empty.bg, Theme::dark().bg);
    }

    #[test]
    fn scope_matching_respects_dot_boundaries_and_prefers_specific_rules() {
        let rule = |scope: &str, color| Rule { scope: scope.to_owned(), foreground: Some(color) };
        let rules = [rule("string", 1), rule("string.quoted", 2), rule("stringy", 3), rule("string.quoted", 4)];
        assert_eq!(color_for_scope(&rules, "string"), Some(1));
        assert_eq!(color_for_scope(&rules, "string.quoted.double"), Some(4), "most specific, later wins ties");
        assert_eq!(color_for_scope(&rules, "string.other"), Some(1));
        assert_eq!(color_for_scope(&rules, "stringify"), None, "not a prefix on a dot boundary");
        assert_eq!(color_for_scope(&[], "string"), None);
    }

    #[test]
    fn includes_are_merged_confined_and_bounded() {
        let dir = std::env::temp_dir().join(format!("forge-theme-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("base.json"), r##"{ "colors": { "editor.background": "#101010", "editor.foreground": "#eeeeee" }, "tokenColors": [{ "scope": "comment", "settings": { "foreground": "#111111" } }] }"##).unwrap();
        std::fs::write(dir.join("child.json"), r##"{ "include": "./base.json", "colors": { "editor.foreground": "#abcdef" }, "tokenColors": [{ "scope": "comment", "settings": { "foreground": "#222222" } }] }"##).unwrap();
        std::fs::write(dir.join("escape.json"), r##"{ "include": "../outside.json" }"##).unwrap();
        std::fs::write(dir.join("loop.json"), r##"{ "include": "./loop.json" }"##).unwrap();
        std::fs::write(dir.join("broken.json"), "{ nope").unwrap();

        let theme = load_theme_file(&dir.join("child.json"), "Child", None).unwrap();
        assert_eq!((theme.bg, theme.text), (0x101010, 0xabcdef), "the includer overrides, the include supplies the rest");
        assert_eq!(theme.syntax_color(HighlightClass::Comment), 0x222222);
        assert!(load_theme_file(&dir.join("escape.json"), "x", None).unwrap_err().contains("cannot include"));
        assert!(load_theme_file(&dir.join("loop.json"), "x", None).unwrap_err().contains("too deeply"));
        assert!(load_theme_file(&dir.join("broken.json"), "x", None).is_err());
        assert!(load_theme_file(&dir.join("missing.json"), "x", None).is_err());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn confined_paths_reject_escapes() {
        let root = Path::new("C:/ext");
        assert!(confined_path(root, "./themes/a.json").is_some());
        assert!(confined_path(root, "../a.json").is_none());
        assert!(confined_path(root, "/etc/passwd").is_none());
    }
}
