//! VS Code extension support, phase one: declarative extensions.
//!
//! This crate reads extension manifests, installs and removes extensions (from a `.vsix`
//! file or from Open VSX), and exposes what they contribute: color themes, languages and
//! their configuration, TextMate grammars, and snippets. None of that needs a running
//! extension. Running an extension's own code (its `main` entry point) needs a Node host
//! that does not exist yet; see `docs/EXTENSIONS_PLAN.md`.

pub mod host;
pub mod openvsx;
pub mod vsix;

use std::collections::BTreeMap;
use std::path::{Component, Path, PathBuf};

use anyhow::{Context, Result};
use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Manifest {
    pub name: String,
    #[serde(default)]
    pub publisher: Option<String>,
    #[serde(default)]
    pub version: String,
    #[serde(default)]
    pub display_name: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    /// Entry point of the Node extension host code, relative to the extension root.
    #[serde(default)]
    pub main: Option<String>,
    /// Entry point for a web extension host, which Forge does not support.
    #[serde(default)]
    pub browser: Option<String>,
    #[serde(default)]
    pub activation_events: Vec<String>,
    #[serde(default)]
    pub contributes: Contributes,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct Contributes {
    #[serde(default)]
    pub languages: Vec<LanguageContribution>,
    #[serde(default)]
    pub commands: Vec<CommandContribution>,
    #[serde(default)]
    pub grammars: Vec<GrammarContribution>,
    #[serde(default)]
    pub themes: Vec<ThemeContribution>,
    #[serde(default, rename = "iconThemes")]
    pub icon_themes: Vec<IconThemeContribution>,
    #[serde(default, rename = "productIconThemes")]
    pub product_icon_themes: Vec<IconThemeContribution>,
    #[serde(default)]
    pub snippets: Vec<SnippetContribution>,
    /// `contributes.configuration`: one section object, or an array of them.
    #[serde(default)]
    pub configuration: serde_json::Value,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LanguageContribution {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub aliases: Vec<String>,
    #[serde(default)]
    pub extensions: Vec<String>,
    #[serde(default)]
    pub filenames: Vec<String>,
    /// Glob patterns such as `*.env` or `Dockerfile.*`.
    #[serde(default)]
    pub filename_patterns: Vec<String>,
    /// Regex tested against the first line (shebangs).
    #[serde(default)]
    pub first_line: Option<String>,
    /// Path of the language's `language-configuration.json`.
    #[serde(default)]
    pub configuration: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct CommandContribution {
    pub command: String,
    #[serde(default)]
    pub title: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GrammarContribution {
    /// The language id this grammar colors. Absent for injection grammars.
    #[serde(default)]
    pub language: Option<String>,
    #[serde(default)]
    pub scope_name: String,
    #[serde(default)]
    pub path: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThemeContribution {
    #[serde(default)]
    pub label: String,
    /// `vs`, `vs-dark` or `hc-black`; decides the fallback colors.
    #[serde(default)]
    pub ui_theme: Option<String>,
    #[serde(default)]
    pub path: String,
}

/// A file icon theme: a JSON file that maps file names and extensions to icon images.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct IconThemeContribution {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub label: String,
    #[serde(default)]
    pub path: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct SnippetContribution {
    #[serde(default)]
    pub language: Option<String>,
    #[serde(default)]
    pub path: String,
}

#[derive(Debug, Clone)]
pub struct Extension {
    pub root: PathBuf,
    pub manifest: Manifest,
    /// `package.nls.json`: replacements for `%key%` placeholders in the manifest.
    pub nls: BTreeMap<String, String>,
    /// The whole `package.json`, which extension code can read as `packageJSON`.
    pub manifest_json: serde_json::Value,
}

impl Extension {
    pub fn new(root: PathBuf, manifest: Manifest) -> Self {
        Self { root, manifest, nls: BTreeMap::new(), manifest_json: serde_json::Value::Null }
    }

    pub fn id(&self) -> String {
        match &self.manifest.publisher {
            Some(p) => format!("{p}.{}", self.manifest.name),
            None => self.manifest.name.clone(),
        }
    }

    pub fn display_name(&self) -> String {
        match &self.manifest.display_name {
            Some(name) if !name.trim().is_empty() => self.localize(name),
            _ => self.manifest.name.clone(),
        }
    }

    /// Resolve a `%key%` placeholder through `package.nls.json`; other text is unchanged.
    pub fn localize(&self, text: &str) -> String {
        match text.strip_prefix('%').and_then(|t| t.strip_suffix('%')) {
            Some(key) => self.nls.get(key).cloned().unwrap_or_else(|| text.to_owned()),
            None => text.to_owned(),
        }
    }

    /// A manifest path made absolute. `None` if it would escape the extension's folder.
    pub fn resolve(&self, relative: &str) -> Option<PathBuf> {
        let path = Path::new(relative);
        let confined = path.components().all(|c| matches!(c, Component::Normal(_) | Component::CurDir));
        confined.then(|| self.root.join(path))
    }

    /// Declarative-only extensions (themes, grammars, snippets) need no host process.
    pub fn needs_node_host(&self) -> bool {
        self.manifest.main.is_some()
    }

    pub fn activates_on_startup(&self) -> bool {
        self.manifest.activation_events.iter().any(|e| e == "*" || e == "onStartupFinished")
    }

    /// What Forge can use from this extension today, for the "installed" listing.
    pub fn supported_summary(&self) -> String {
        let c = &self.manifest.contributes;
        let mut parts = Vec::new();
        let mut add = |n: usize, what: &str| {
            if n > 0 {
                parts.push(format!("{n} {what}{}", if n == 1 { "" } else { "s" }));
            }
        };
        add(c.themes.len(), "theme");
        add(c.icon_themes.len(), "icon theme");
        add(c.product_icon_themes.len(), "product icon theme");
        add(c.grammars.len(), "grammar");
        add(c.languages.len(), "language");
        add(c.snippets.len(), "snippet file");
        if self.needs_node_host() {
            parts.push("code not run (no extension host yet)".to_owned());
        }
        if parts.is_empty() {
            "nothing Forge supports yet".to_owned()
        } else {
            parts.join(", ")
        }
    }
}

pub fn parse_manifest(json: &str) -> Result<Manifest> {
    // Some packagers write a byte-order mark, which JSON parsers reject.
    serde_json::from_str(json.trim_start_matches('\u{feff}')).context("invalid extension package.json")
}

/// Values of `package.nls.json`, which are plain strings or `{ "message": ... }`.
fn parse_nls(json: &str) -> BTreeMap<String, String> {
    let Ok(serde_json::Value::Object(map)) = serde_json::from_str(json.trim_start_matches('\u{feff}')) else { return BTreeMap::new() };
    map.into_iter()
        .filter_map(|(key, value)| {
            let text = match &value {
                serde_json::Value::String(s) => Some(s.clone()),
                serde_json::Value::Object(o) => o.get("message").and_then(|m| m.as_str()).map(str::to_owned),
                _ => None,
            }?;
            Some((key, text))
        })
        .collect()
}

/// Read one extension from its folder (a `package.json` at the root).
pub fn load(root: &Path) -> Result<Extension> {
    let text = std::fs::read_to_string(root.join("package.json")).with_context(|| format!("reading {}", root.join("package.json").display()))?;
    let manifest = parse_manifest(&text)?;
    let manifest_json = serde_json::from_str(text.trim_start_matches('\u{feff}')).unwrap_or_default();
    let nls = std::fs::read_to_string(root.join("package.nls.json")).map(|t| parse_nls(&t)).unwrap_or_default();
    Ok(Extension { root: root.to_path_buf(), manifest, nls, manifest_json })
}

/// Scan `dir` for subdirectories containing a readable `package.json` manifest.
/// Unreadable or malformed extensions are skipped and reported in the second value.
pub fn discover(dir: &Path) -> Result<(Vec<Extension>, BTreeMap<PathBuf, String>)> {
    let mut found = Vec::new();
    let mut skipped = BTreeMap::new();
    for entry in std::fs::read_dir(dir).with_context(|| format!("reading {}", dir.display()))? {
        let root = entry?.path();
        // Folders still being unpacked by an install are not extensions yet.
        let staging = root.file_name().is_some_and(|n| n.to_string_lossy().starts_with(".installing-"));
        if staging || !root.join("package.json").is_file() {
            continue;
        }
        match load(&root) {
            Ok(extension) => found.push(extension),
            Err(err) => {
                skipped.insert(root, format!("{err:#}"));
            }
        }
    }
    found.sort_by_key(Extension::id);
    Ok((found, skipped))
}

/// Remove every installed version of `id`. Returns how many folders were removed.
pub fn uninstall(dir: &Path, id: &str) -> Result<usize> {
    let (found, _) = discover(dir)?;
    let mut removed = 0;
    for extension in found.into_iter().filter(|e| e.id().eq_ignore_ascii_case(id)) {
        std::fs::remove_dir_all(&extension.root).with_context(|| format!("removing {}", extension.root.display()))?;
        removed += 1;
    }
    Ok(removed)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"{
        "name": "rust-lang-sample",
        "publisher": "example",
        "version": "1.2.3",
        "main": "./out/extension.js",
        "activationEvents": ["onLanguage:rust"],
        "contributes": {
            "languages": [{"id": "rust", "extensions": [".rs"], "configuration": "./language-configuration.json"}],
            "commands": [{"command": "sample.run", "title": "Run"}],
            "themes": [{"label": "%theme%", "uiTheme": "vs-dark", "path": "./themes/a.json"}],
            "grammars": [{"language": "rust", "scopeName": "source.rust", "path": "./syntaxes/rust.tmLanguage.json"}],
            "snippets": [{"language": "rust", "path": "./snippets.json"}]
        }
    }"#;

    #[test]
    fn parses_manifest_fields() {
        let m = parse_manifest(SAMPLE).unwrap();
        assert_eq!(m.main.as_deref(), Some("./out/extension.js"));
        assert_eq!(m.contributes.languages[0].id, "rust");
        assert_eq!(m.contributes.languages[0].configuration.as_deref(), Some("./language-configuration.json"));
        assert_eq!(m.contributes.commands[0].command, "sample.run");
        assert_eq!(m.contributes.themes[0].ui_theme.as_deref(), Some("vs-dark"));
        assert_eq!(m.contributes.grammars[0].scope_name, "source.rust");
        assert_eq!(m.contributes.snippets[0].path, "./snippets.json");
    }

    #[test]
    fn a_byte_order_mark_does_not_break_the_manifest() {
        assert!(parse_manifest("\u{feff}{\"name\":\"a\"}").is_ok());
    }

    #[test]
    fn declarative_extension_needs_no_host() {
        let m = parse_manifest(r#"{"name":"theme","contributes":{"grammars":[{}]}}"#).unwrap();
        let ext = Extension::new(PathBuf::new(), m);
        assert!(!ext.needs_node_host());
        assert!(!ext.activates_on_startup());
        assert_eq!(ext.id(), "theme");
    }

    #[test]
    fn startup_activation_detected() {
        let m = parse_manifest(r#"{"name":"a","main":"x.js","activationEvents":["*"]}"#).unwrap();
        let ext = Extension::new(PathBuf::new(), m);
        assert!(ext.needs_node_host() && ext.activates_on_startup());
    }

    #[test]
    fn placeholders_resolve_through_the_nls_file() {
        let mut ext = Extension::new(PathBuf::new(), parse_manifest(SAMPLE).unwrap());
        ext.nls = parse_nls(r#"{ "theme": "Night Owl", "other": { "message": "Hi", "comment": ["x"] } }"#);
        assert_eq!(ext.localize("%theme%"), "Night Owl");
        assert_eq!(ext.localize("%other%"), "Hi");
        assert_eq!(ext.localize("%missing%"), "%missing%");
        assert_eq!(ext.localize("Plain"), "Plain");
    }

    #[test]
    fn manifest_paths_cannot_leave_the_extension_folder() {
        let ext = Extension::new(PathBuf::from("C:/exts/a"), parse_manifest(SAMPLE).unwrap());
        assert!(ext.resolve("./themes/a.json").is_some());
        assert!(ext.resolve("themes/a.json").is_some());
        assert!(ext.resolve("../b/secret.json").is_none());
        assert!(ext.resolve("themes/../../b").is_none());
        assert!(ext.resolve("/etc/passwd").is_none());
        assert!(ext.resolve("C:/Windows/win.ini").is_none());
    }

    #[test]
    fn discover_skips_malformed_manifests_and_uninstall_removes_by_id() {
        let dir = std::env::temp_dir().join(format!("forge-ext-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let good = dir.join("good");
        let bad = dir.join("bad");
        std::fs::create_dir_all(&good).unwrap();
        std::fs::create_dir_all(&bad).unwrap();
        std::fs::write(good.join("package.json"), SAMPLE).unwrap();
        std::fs::write(bad.join("package.json"), "{ not json").unwrap();

        let (found, skipped) = discover(&dir).unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].id(), "example.rust-lang-sample");
        assert!(skipped.contains_key(&bad));

        assert_eq!(uninstall(&dir, "other.thing").unwrap(), 0);
        assert!(good.exists());
        assert_eq!(uninstall(&dir, "Example.Rust-Lang-Sample").unwrap(), 1);
        assert!(!good.exists());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
