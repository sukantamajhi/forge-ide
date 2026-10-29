//! VS Code file icon themes (the JSON files extensions contribute under `iconThemes`):
//! which image stands for a file or folder. Only image icons (SVG, PNG) are used; themes
//! built on icon fonts are skipped.

use std::collections::HashMap;
use std::path::{Component, Path, PathBuf};

use serde_json::Value;

#[derive(Clone, Debug, Default)]
pub struct IconTheme {
    pub name: String,
    /// Icon definition id to image file.
    images: HashMap<String, PathBuf>,
    file: Option<String>,
    folder: Option<String>,
    folder_expanded: Option<String>,
    file_names: HashMap<String, String>,
    file_extensions: HashMap<String, String>,
    language_ids: HashMap<String, String>,
    folder_names: HashMap<String, String>,
    folder_names_expanded: HashMap<String, String>,
}

/// `base` joined with `relative`, with `.` and `..` resolved by text (the file may not exist yet).
pub(crate) fn normalize(base: &Path, relative: &str) -> PathBuf {
    let mut out = PathBuf::new();
    for part in base.join(relative).components() {
        match part {
            Component::ParentDir => {
                out.pop();
            }
            Component::CurDir => {}
            other => out.push(other.as_os_str()),
        }
    }
    out
}

fn string_map(value: Option<&Value>, lowercase: bool) -> HashMap<String, String> {
    value
        .and_then(Value::as_object)
        .map(|m| {
            m.iter()
                .filter_map(|(k, v)| {
                    v.as_str().map(|v| {
                        (
                            if lowercase {
                                k.to_lowercase()
                            } else {
                                k.clone()
                            },
                            v.to_owned(),
                        )
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

impl IconTheme {
    /// `json` is the theme file's text, found in `dir`. Icon paths are relative to `dir` and
    /// must stay inside `root` (the extension's folder).
    pub fn parse(json: &str, name: &str, dir: &Path, root: &Path) -> Result<Self, String> {
        let Value::Object(map) = crate::jsonc::parse(json)? else {
            return Err("an icon theme is a JSON object".to_owned());
        };
        let mut images = HashMap::new();
        if let Some(defs) = map.get("iconDefinitions").and_then(Value::as_object) {
            for (id, def) in defs {
                let Some(path) = def.get("iconPath").and_then(Value::as_str) else {
                    continue;
                };
                let full = normalize(dir, path);
                let image = full.extension().is_some_and(|e| {
                    ["svg", "png", "jpg", "jpeg", "gif", "webp"]
                        .iter()
                        .any(|x| e.eq_ignore_ascii_case(x))
                });
                if image && full.starts_with(root) {
                    images.insert(id.clone(), full);
                }
            }
        }
        if images.is_empty() {
            return Err(
                "this icon theme has no image icons (icon fonts are not supported)".to_owned(),
            );
        }
        let text = |key: &str| map.get(key).and_then(Value::as_str).map(str::to_owned);
        Ok(Self {
            name: name.to_owned(),
            images,
            file: text("file"),
            folder: text("folder"),
            folder_expanded: text("folderExpanded"),
            file_names: string_map(map.get("fileNames"), true),
            file_extensions: string_map(map.get("fileExtensions"), true),
            language_ids: string_map(map.get("languageIds"), false),
            folder_names: string_map(map.get("folderNames"), true),
            folder_names_expanded: string_map(map.get("folderNamesExpanded"), true),
        })
    }

    pub fn load(path: &Path, name: &str, root: &Path) -> Result<Self, String> {
        let text = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
        Self::parse(&text, name, path.parent().unwrap_or(root), root)
    }

    fn image(&self, id: Option<&String>) -> Option<&Path> {
        id.and_then(|id| self.images.get(id)).map(PathBuf::as_path)
    }

    /// The icon for a file: its exact name, then its longest known extension (`d.ts` before `ts`),
    /// then its language, then the theme's plain file icon.
    pub fn file_icon(&self, name: &str, language: Option<&str>) -> Option<&Path> {
        let lower = name.to_lowercase();
        if let Some(found) = self.image(self.file_names.get(&lower)) {
            return Some(found);
        }
        for (at, _) in lower.match_indices('.') {
            if let Some(found) = self.image(self.file_extensions.get(&lower[at + 1..])) {
                return Some(found);
            }
        }
        if let Some(found) = language.and_then(|l| self.image(self.language_ids.get(l))) {
            return Some(found);
        }
        self.image(self.file.as_ref())
    }

    pub fn folder_icon(&self, name: &str, open: bool) -> Option<&Path> {
        let lower = name.to_lowercase();
        let (named, plain) = if open {
            (&self.folder_names_expanded, &self.folder_expanded)
        } else {
            (&self.folder_names, &self.folder)
        };
        self.image(named.get(&lower))
            .or_else(|| self.image(plain.as_ref()))
            .or_else(|| self.image(self.folder.as_ref()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const THEME: &str = r#"{
        // comments are fine
        "iconDefinitions": {
            "_file": { "iconPath": "./icons/file.svg" },
            "_rs": { "iconPath": "./../icons/rust.svg" },
            "_ts": { "iconPath": "./icons/ts.svg" },
            "_dts": { "iconPath": "./icons/dts.svg" },
            "_dir": { "iconPath": "./icons/dir.svg" },
            "_src": { "iconPath": "./icons/src.svg" },
            "_font": { "fontCharacter": "\\E001" },
            "_escape": { "iconPath": "../../../outside.svg" }
        },
        "file": "_file", "folder": "_dir", "folderExpanded": "_dir",
        "fileExtensions": { "rs": "_rs", "ts": "_ts", "d.ts": "_dts" },
        "fileNames": { "Cargo.toml": "_src" },
        "folderNames": { "src": "_src" },
        "languageIds": { "python": "_ts" }
    }"#;

    fn theme() -> IconTheme {
        IconTheme::parse(THEME, "Test", Path::new("/ext/dist"), Path::new("/ext")).unwrap()
    }

    #[test]
    fn names_then_longest_extension_then_language_then_plain_file() {
        let t = theme();
        assert!(t
            .file_icon("cargo.toml", None)
            .unwrap()
            .ends_with("src.svg"));
        assert!(
            t.file_icon("main.RS", None).unwrap().ends_with("rust.svg"),
            "paths may climb out of dist/ but stay in the extension"
        );
        assert!(t.file_icon("index.ts", None).unwrap().ends_with("ts.svg"));
        assert!(t
            .file_icon("types.d.ts", None)
            .unwrap()
            .ends_with("dts.svg"));
        assert!(t
            .file_icon("x.unknown", Some("python"))
            .unwrap()
            .ends_with("ts.svg"));
        assert!(t
            .file_icon("x.unknown", None)
            .unwrap()
            .ends_with("file.svg"));
    }

    #[test]
    fn folders_use_names_then_the_plain_folder_icon() {
        let t = theme();
        assert!(t.folder_icon("src", false).unwrap().ends_with("src.svg"));
        assert!(t.folder_icon("docs", false).unwrap().ends_with("dir.svg"));
        assert!(t.folder_icon("docs", true).unwrap().ends_with("dir.svg"));
    }

    #[test]
    fn icons_outside_the_extension_and_font_icons_are_dropped() {
        let t = theme();
        assert!(!t.images.contains_key("_escape"));
        assert!(!t.images.contains_key("_font"));
        assert!(IconTheme::parse(
            r#"{"iconDefinitions":{"a":{"fontCharacter":"x"}}}"#,
            "F",
            Path::new("/e"),
            Path::new("/e")
        )
        .is_err());
    }
}
