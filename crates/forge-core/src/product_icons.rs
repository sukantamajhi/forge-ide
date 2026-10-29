//! VS Code product icon themes (`productIconThemes`): they replace the editor's own icons
//! (the Codicons) with characters from a font the theme ships.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::icon_theme::normalize;

/// A font file a theme ships.
#[derive(Clone, Debug, PartialEq)]
pub struct ProductFont {
    pub id: String,
    pub path: PathBuf,
}

#[derive(Clone, Debug, Default)]
pub struct ProductIcons {
    pub name: String,
    pub fonts: Vec<ProductFont>,
    /// Codicon id (`files`, `chevron-down`) to the font id and character that replace it.
    icons: HashMap<String, (String, char)>,
}

/// `"\\e001"` (a backslash and hex digits, as theme files write it) or a literal character.
fn font_character(text: &str) -> Option<char> {
    match text.strip_prefix('\\') {
        Some(hex) => u32::from_str_radix(hex, 16).ok().and_then(char::from_u32),
        None => {
            let mut chars = text.chars();
            chars.next().filter(|_| chars.next().is_none())
        }
    }
}

/// How well Forge can read a font file: lower is better, `None` is unusable here.
fn font_rank(path: &Path) -> Option<u8> {
    match path.extension()?.to_str()?.to_ascii_lowercase().as_str() {
        "ttf" | "otf" => Some(0),
        "woff" => Some(1),
        _ => None,
    }
}

impl ProductIcons {
    /// `json` is the theme file's text, found in `dir`. Font paths must stay inside `root`.
    pub fn parse(json: &str, name: &str, dir: &Path, root: &Path) -> Result<Self, String> {
        let Value::Object(map) = crate::jsonc::parse(json)? else { return Err("a product icon theme is a JSON object".to_owned()) };
        let mut fonts = Vec::new();
        let mut skipped_woff2 = false;
        for font in map.get("fonts").and_then(Value::as_array).into_iter().flatten() {
            let id = font.get("id").and_then(Value::as_str).unwrap_or_default().to_owned();
            let best = font
                .get("src")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(|s| s.get("path").and_then(Value::as_str))
                .map(|p| normalize(dir, p))
                .filter(|p| p.starts_with(root))
                .filter_map(|p| {
                    let rank = font_rank(&p);
                    skipped_woff2 |= rank.is_none() && p.extension().is_some_and(|e| e.eq_ignore_ascii_case("woff2"));
                    rank.map(|r| (r, p))
                })
                .min_by_key(|(r, _)| *r);
            if let Some((_, path)) = best {
                fonts.push(ProductFont { id, path });
            }
        }
        if fonts.is_empty() {
            return Err(if skipped_woff2 {
                "this theme only ships WOFF2 fonts, which Forge cannot read (it needs TTF, OTF or WOFF)".to_owned()
            } else {
                "this theme ships no font Forge can read".to_owned()
            });
        }
        let first = fonts[0].id.clone();
        let mut icons = HashMap::new();
        for (id, def) in map.get("iconDefinitions").and_then(Value::as_object).into_iter().flatten() {
            let Some(c) = def.get("fontCharacter").and_then(Value::as_str).and_then(font_character) else { continue };
            let font = def.get("fontId").and_then(Value::as_str).unwrap_or(&first).to_owned();
            if fonts.iter().any(|f| f.id == font) {
                icons.insert(id.clone(), (font, c));
            }
        }
        Ok(Self { name: name.to_owned(), fonts, icons })
    }

    pub fn load(path: &Path, name: &str, root: &Path) -> Result<Self, String> {
        let text = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
        Self::parse(&text, name, path.parent().unwrap_or(root), root)
    }

    /// The replacement for the first of `ids` the theme defines: its font id and character.
    pub fn icon(&self, ids: &[&str]) -> Option<(&str, char)> {
        ids.iter().find_map(|id| self.icons.get(*id)).map(|(font, c)| (font.as_str(), *c))
    }

    pub fn len(&self) -> usize {
        self.icons.len()
    }

    pub fn is_empty(&self) -> bool {
        self.icons.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const THEME: &str = r#"{
        "fonts": [
            { "id": "main", "src": [ { "path": "./f/a.woff2", "format": "woff2" }, { "path": "./f/a.woff", "format": "woff" }, { "path": "./f/a.ttf", "format": "truetype" } ] },
            { "id": "extra", "src": [ { "path": "../../outside.ttf" } ] }
        ],
        "iconDefinitions": {
            "files": { "fontCharacter": "\\e001" },
            "search": { "fontCharacter": "\\E002", "fontId": "main" },
            "bad": { "fontCharacter": "\\e003", "fontId": "extra" },
            "nochar": {}
        }
    }"#;

    #[test]
    fn picks_the_best_readable_font_and_maps_characters() {
        let t = ProductIcons::parse(THEME, "T", Path::new("/ext/dist"), Path::new("/ext")).unwrap();
        assert_eq!(t.fonts.len(), 1, "the font that leaves the extension folder is dropped");
        assert!(t.fonts[0].path.ends_with("a.ttf"), "ttf beats woff and woff2");
        assert_eq!(t.icon(&["files"]), Some(("main", '\u{e001}')));
        assert_eq!(t.icon(&["nope", "search"]), Some(("main", '\u{e002}')), "the first defined id wins");
        assert_eq!(t.icon(&["bad"]), None);
        assert_eq!(t.len(), 2);
    }

    #[test]
    fn woff2_only_themes_say_why_they_cannot_load() {
        let json = r#"{ "fonts": [ { "id": "m", "src": [ { "path": "./a.woff2" } ] } ], "iconDefinitions": {} }"#;
        let err = ProductIcons::parse(json, "T", Path::new("/e"), Path::new("/e")).unwrap_err();
        assert!(err.contains("WOFF2"), "{err}");
    }
}
