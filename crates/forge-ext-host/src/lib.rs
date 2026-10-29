//! VS Code extension support, step one: reading extension manifests.
//!
//! Running an extension needs a Node process that implements the `vscode` API and
//! talks to Forge over IPC. That host does not exist yet; this crate only discovers
//! extensions and decides when each one would activate.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

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
    pub grammars: Vec<serde_json::Value>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct LanguageContribution {
    pub id: String,
    #[serde(default)]
    pub extensions: Vec<String>,
    #[serde(default)]
    pub filenames: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct CommandContribution {
    pub command: String,
    #[serde(default)]
    pub title: Option<String>,
}

#[derive(Debug, Clone)]
pub struct Extension {
    pub root: PathBuf,
    pub manifest: Manifest,
}

impl Extension {
    pub fn id(&self) -> String {
        match &self.manifest.publisher {
            Some(p) => format!("{p}.{}", self.manifest.name),
            None => self.manifest.name.clone(),
        }
    }

    /// Declarative-only extensions (themes, grammars, snippets) need no host process.
    pub fn needs_node_host(&self) -> bool {
        self.manifest.main.is_some()
    }

    pub fn activates_on_startup(&self) -> bool {
        self.manifest.activation_events.iter().any(|e| e == "*" || e == "onStartupFinished")
    }
}

pub fn parse_manifest(json: &str) -> Result<Manifest> {
    serde_json::from_str(json).context("invalid extension package.json")
}

/// Scan `dir` for subdirectories containing a readable `package.json` manifest.
/// Unreadable or malformed extensions are skipped and reported in the second value.
pub fn discover(dir: &Path) -> Result<(Vec<Extension>, BTreeMap<PathBuf, String>)> {
    let mut found = Vec::new();
    let mut skipped = BTreeMap::new();
    for entry in std::fs::read_dir(dir).with_context(|| format!("reading {}", dir.display()))? {
        let root = entry?.path();
        let manifest_path = root.join("package.json");
        if !manifest_path.is_file() {
            continue;
        }
        match std::fs::read_to_string(&manifest_path)
            .map_err(anyhow::Error::from)
            .and_then(|text| parse_manifest(&text))
        {
            Ok(manifest) => found.push(Extension { root, manifest }),
            Err(err) => {
                skipped.insert(root, format!("{err:#}"));
            }
        }
    }
    found.sort_by_key(Extension::id);
    Ok((found, skipped))
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
            "languages": [{"id": "rust", "extensions": [".rs"]}],
            "commands": [{"command": "sample.run", "title": "Run"}]
        }
    }"#;

    #[test]
    fn parses_manifest_fields() {
        let m = parse_manifest(SAMPLE).unwrap();
        assert_eq!(m.main.as_deref(), Some("./out/extension.js"));
        assert_eq!(m.contributes.languages[0].id, "rust");
        assert_eq!(m.contributes.commands[0].command, "sample.run");
    }

    #[test]
    fn declarative_extension_needs_no_host() {
        let m = parse_manifest(r#"{"name":"theme","contributes":{"grammars":[{}]}}"#).unwrap();
        let ext = Extension { root: PathBuf::new(), manifest: m };
        assert!(!ext.needs_node_host());
        assert!(!ext.activates_on_startup());
        assert_eq!(ext.id(), "theme");
    }

    #[test]
    fn startup_activation_detected() {
        let m = parse_manifest(r#"{"name":"a","main":"x.js","activationEvents":["*"]}"#).unwrap();
        let ext = Extension { root: PathBuf::new(), manifest: m };
        assert!(ext.needs_node_host() && ext.activates_on_startup());
    }

    #[test]
    fn discover_skips_malformed_manifests() {
        let dir = std::env::temp_dir().join(format!("forge-ext-test-{}", std::process::id()));
        let good = dir.join("good");
        let bad = dir.join("bad");
        std::fs::create_dir_all(&good).unwrap();
        std::fs::create_dir_all(&bad).unwrap();
        std::fs::write(good.join("package.json"), SAMPLE).unwrap();
        std::fs::write(bad.join("package.json"), "{ not json").unwrap();

        let (found, skipped) = discover(&dir).unwrap();
        std::fs::remove_dir_all(&dir).unwrap();

        assert_eq!(found.len(), 1);
        assert_eq!(found[0].id(), "example.rust-lang-sample");
        assert!(skipped.contains_key(&bad));
    }
}
