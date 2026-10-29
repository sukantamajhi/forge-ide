//! Which language server serves which language, where Forge keeps the ones it installs,
//! and how to install them.
//!
//! One server can serve several languages (VS Code runs a single TypeScript server for
//! `.js`, `.jsx`, `.ts` and `.tsx`), so servers are identified by a `key`, not by language.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use forge_syntax::Language;
use serde_json::{json, Value};

use crate::settings::config_dir;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Install {
    /// npm packages installed into Forge's own folder (nothing global is touched).
    Npm(&'static [&'static str]),
    /// A single file fetched into Forge's `bin` folder.
    Download { url: &'static str, file: &'static str },
    /// Needs a toolchain Forge does not manage; the text says how.
    Manual(&'static str),
}

/// A resolved choice of server for a language.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ServerSpec {
    /// Shared by every language this server handles.
    pub key: String,
    pub title: String,
    pub command: String,
    /// `{managed}` expands to Forge's server folder.
    pub args: Vec<String>,
    pub install: Install,
    /// Set by `languageServers` in settings.json.
    pub custom: bool,
}

struct Def {
    key: &'static str,
    title: &'static str,
    command: &'static str,
    args: &'static [&'static str],
    install: Install,
    languages: &'static [Language],
}

const LEMMINX_JAR: &str = "https://repo.eclipse.org/content/repositories/lemminx-releases/org/eclipse/lemminx/org.eclipse.lemminx/0.31.2/org.eclipse.lemminx-0.31.2-uber.jar";

fn defs() -> Vec<Def> {
    use Language::*;
    vec![
        Def { key: "typescript", title: "TypeScript / JavaScript (typescript-language-server)", command: "typescript-language-server", args: &["--stdio"], install: Install::Npm(&["typescript-language-server", "typescript@5"]), languages: &[JavaScript, JavaScriptReact, TypeScript, TypeScriptReact] },
        Def { key: "html", title: "HTML (vscode-html-language-server)", command: "vscode-html-language-server", args: &["--stdio"], install: Install::Npm(&["vscode-langservers-extracted"]), languages: &[Html] },
        Def { key: "css", title: "CSS, SCSS, Less (vscode-css-language-server)", command: "vscode-css-language-server", args: &["--stdio"], install: Install::Npm(&["vscode-langservers-extracted"]), languages: &[Css] },
        Def { key: "json", title: "JSON (vscode-json-language-server)", command: "vscode-json-language-server", args: &["--stdio"], install: Install::Npm(&["vscode-langservers-extracted"]), languages: &[Json] },
        Def { key: "yaml", title: "YAML (yaml-language-server)", command: "yaml-language-server", args: &["--stdio"], install: Install::Npm(&["yaml-language-server"]), languages: &[Yaml] },
        Def { key: "bash", title: "Shell scripts (bash-language-server)", command: "bash-language-server", args: &["start"], install: Install::Npm(&["bash-language-server"]), languages: &[Shell] },
        Def { key: "python", title: "Python (pyright)", command: "pyright-langserver", args: &["--stdio"], install: Install::Npm(&["pyright"]), languages: &[Python] },
        Def { key: "dockerfile", title: "Dockerfile (docker-langserver)", command: "docker-langserver", args: &["--stdio"], install: Install::Npm(&["dockerfile-language-server-nodejs"]), languages: &[Dockerfile] },
        Def { key: "php", title: "PHP (intelephense)", command: "intelephense", args: &["--stdio"], install: Install::Npm(&["intelephense"]), languages: &[Php] },
        Def { key: "xml", title: "XML (LemMinX, needs Java)", command: "java", args: &["-jar", "{managed}/bin/lemminx.jar"], install: Install::Download { url: LEMMINX_JAR, file: "lemminx.jar" }, languages: &[Xml] },
        Def { key: "markdown", title: "Markdown (marksman)", command: "marksman", args: &["server"], install: Install::Download { url: "https://github.com/artempyanykh/marksman/releases/latest/download/marksman.exe", file: "marksman.exe" }, languages: &[Markdown] },
        Def { key: "rust", title: "Rust (rust-analyzer)", command: "rust-analyzer", args: &[], install: Install::Manual("rustup component add rust-analyzer"), languages: &[Rust] },
        Def { key: "go", title: "Go (gopls)", command: "gopls", args: &[], install: Install::Manual("go install golang.org/x/tools/gopls@latest"), languages: &[Go] },
        Def { key: "clangd", title: "C and C++ (clangd)", command: "clangd", args: &[], install: Install::Manual("install LLVM (winget install LLVM.LLVM) so clangd is on PATH"), languages: &[C, Cpp] },
        Def { key: "java", title: "Java (Eclipse JDT LS)", command: "jdtls", args: &[], install: Install::Manual("install Eclipse JDT LS and put the jdtls launcher on PATH"), languages: &[Java] },
        Def { key: "csharp", title: "C# (csharp-ls)", command: "csharp-ls", args: &[], install: Install::Manual("dotnet tool install --global csharp-ls"), languages: &[CSharp] },
        Def { key: "ruby", title: "Ruby (ruby-lsp)", command: "ruby-lsp", args: &[], install: Install::Manual("gem install ruby-lsp"), languages: &[Ruby] },
        Def { key: "lua", title: "Lua (lua-language-server)", command: "lua-language-server", args: &[], install: Install::Manual("install lua-language-server and put it on PATH"), languages: &[Lua] },
        Def { key: "toml", title: "TOML (taplo)", command: "taplo", args: &["lsp", "stdio"], install: Install::Manual("cargo install taplo-cli --features lsp"), languages: &[Toml] },
        Def { key: "sql", title: "SQL (sqls)", command: "sqls", args: &[], install: Install::Manual("go install github.com/sqls-server/sqls@latest"), languages: &[Sql] },
    ]
}

fn spec_of(def: &Def) -> ServerSpec {
    ServerSpec {
        key: def.key.to_owned(),
        title: def.title.to_owned(),
        command: def.command.to_owned(),
        args: def.args.iter().map(|a| (*a).to_owned()).collect(),
        install: def.install.clone(),
        custom: false,
    }
}

/// The server for `language`. `overrides` (from settings, by VS Code language id) win over
/// the built-in choice and are never installed by Forge.
pub fn spec_for(language: Language, overrides: &BTreeMap<String, (String, Vec<String>)>) -> Option<ServerSpec> {
    if let Some((command, args)) = overrides.get(language.language_id()) {
        return Some(ServerSpec {
            key: format!("custom:{}", language.language_id()),
            title: format!("{} (custom: {command})", language.name()),
            command: command.clone(),
            args: args.clone(),
            install: Install::Manual("configured in settings.json"),
            custom: true,
        });
    }
    defs().iter().find(|d| d.languages.contains(&language)).map(spec_of)
}

/// Every built-in server, once each.
pub fn all_specs() -> Vec<ServerSpec> {
    defs().iter().map(spec_of).collect()
}

/// Languages a built-in server key handles.
pub fn languages_of(key: &str) -> Vec<Language> {
    defs().iter().find(|d| d.key == key).map(|d| d.languages.to_vec()).unwrap_or_default()
}

// ---- where things live ----

pub fn managed_dir() -> PathBuf {
    config_dir().join("servers")
}

/// Where `npm install --prefix` puts executables.
pub fn managed_npm_bin() -> PathBuf {
    managed_dir().join("node_modules").join(".bin")
}

pub fn managed_bin() -> PathBuf {
    managed_dir().join("bin")
}

/// Find `name` in `dirs`, trying Windows' executable suffixes.
pub fn which_in(name: &str, dirs: &[PathBuf]) -> Option<PathBuf> {
    // On Windows the bare name is often a shell script (npm, tsc) that cannot be executed,
    // so the runnable suffixes win.
    let suffixes: &[&str] = if cfg!(windows) { &[".exe", ".cmd", ".bat", ""] } else { &[""] };
    dirs.iter().find_map(|dir| suffixes.iter().map(|s| dir.join(format!("{name}{s}"))).find(|c| c.is_file()))
}

/// Search Forge's folders first, then `PATH`.
pub fn which(name: &str) -> Option<PathBuf> {
    let path = Path::new(name);
    if path.is_absolute() {
        return path.is_file().then(|| path.to_path_buf());
    }
    let mut dirs = vec![managed_npm_bin(), managed_bin()];
    if let Some(p) = std::env::var_os("PATH") {
        dirs.extend(std::env::split_paths(&p));
    }
    which_in(name, &dirs)
}

/// The program to run for `spec`, or why it cannot run yet.
pub fn resolve(spec: &ServerSpec) -> Result<PathBuf, String> {
    if let Install::Download { file, .. } = &spec.install {
        if !spec.custom && !managed_bin().join(file).is_file() {
            return Err(format!("{} is not installed. Run \"Language Servers: Manage and Install...\" to download it.", spec.title));
        }
    }
    which(&spec.command).ok_or_else(|| match &spec.install {
        Install::Npm(_) => format!("{} is not installed. Run \"Language Servers: Manage and Install...\" to install it.", spec.title),
        Install::Download { .. } => format!("{} needs `{}` on PATH.", spec.title, spec.command),
        Install::Manual(how) => format!("{} is not installed: {how}.", spec.title),
    })
}

pub fn is_installed(spec: &ServerSpec) -> bool {
    resolve(spec).is_ok()
}

/// Arguments with `{managed}` expanded.
pub fn expanded_args(spec: &ServerSpec) -> Vec<String> {
    let dir = managed_dir().to_string_lossy().into_owned();
    spec.args.iter().map(|a| a.replace("{managed}", &dir)).collect()
}

/// `initializationOptions` a server needs. The TypeScript server must be told where the
/// `typescript` package it should drive lives.
pub fn init_options(spec: &ServerSpec) -> Option<Value> {
    if spec.key != "typescript" {
        return None;
    }
    let tsserver = managed_dir().join("node_modules").join("typescript").join("lib").join("tsserver.js");
    tsserver.is_file().then(|| json!({ "tsserver": { "path": tsserver.to_string_lossy() } }))
}

// ---- installing ----

fn quiet(cmd: &mut Command) -> &mut Command {
    cmd.stdin(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000);
    }
    cmd
}

/// Install packages into Forge's folder. Blocks; run it on a background thread.
pub fn install_npm(packages: &[&str]) -> Result<String, String> {
    let npm = which("npm").ok_or("npm was not found on PATH (install Node.js)")?;
    std::fs::create_dir_all(managed_dir()).map_err(|e| e.to_string())?;
    let mut cmd = Command::new(npm);
    cmd.args(["install", "--prefix"]).arg(managed_dir()).args(["--no-audit", "--no-fund", "--loglevel=error"]).args(packages);
    let output = quiet(&mut cmd).output().map_err(|e| format!("could not run npm: {e}"))?;
    if output.status.success() {
        Ok(format!("Installed {}", packages.join(", ")))
    } else {
        let err = String::from_utf8_lossy(&output.stderr);
        Err(err.lines().rfind(|l| !l.trim().is_empty()).unwrap_or("npm failed").trim().to_owned())
    }
}

/// Download `url` into Forge's `bin` folder as `file`. Blocks; run it on a background thread.
pub fn install_download(url: &str, file: &str) -> Result<String, String> {
    std::fs::create_dir_all(managed_bin()).map_err(|e| e.to_string())?;
    let dest = managed_bin().join(file);
    let partial = managed_bin().join(format!("{file}.part"));
    let mut cmd = Command::new("curl");
    cmd.args(["-L", "-f", "-sS", "-o"]).arg(&partial).arg(url);
    let output = quiet(&mut cmd).output().map_err(|e| format!("could not run curl: {e}"))?;
    if !output.status.success() {
        let _ = std::fs::remove_file(&partial);
        let err = String::from_utf8_lossy(&output.stderr);
        return Err(err.lines().next().unwrap_or("download failed").trim().to_owned());
    }
    // Move into place only once complete, so a failed download never leaves a broken file.
    std::fs::rename(&partial, &dest).map_err(|e| e.to_string())?;
    Ok(format!("Downloaded {file}"))
}

/// Install a built-in server. Blocks.
pub fn install(spec: &ServerSpec) -> Result<String, String> {
    match &spec.install {
        Install::Npm(packages) => install_npm(packages),
        Install::Download { url, file } => install_download(url, file),
        Install::Manual(how) => Err(format!("{} must be installed by hand: {how}", spec.title)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn none() -> BTreeMap<String, (String, Vec<String>)> {
        BTreeMap::new()
    }

    #[test]
    fn the_reported_languages_each_have_a_server_and_one_server_serves_the_javascript_family() {
        let key = |l| spec_for(l, &none()).map(|s| s.key);
        for l in [Language::JavaScript, Language::JavaScriptReact, Language::TypeScript, Language::TypeScriptReact] {
            assert_eq!(key(l).as_deref(), Some("typescript"), "{l:?}");
        }
        assert_eq!(key(Language::Xml).as_deref(), Some("xml"));
        assert_eq!(key(Language::Html).as_deref(), Some("html"));
        assert_eq!(key(Language::Css).as_deref(), Some("css"));
        assert_eq!(key(Language::Json).as_deref(), Some("json"));
        assert_eq!(key(Language::Yaml).as_deref(), Some("yaml"));
        assert_eq!(key(Language::C).as_deref(), Some("clangd"));
        assert_eq!(key(Language::Cpp).as_deref(), Some("clangd"));
        // Formats with no server.
        for l in [Language::Env, Language::Ini, Language::Makefile, Language::Diff, Language::Ignore] {
            assert_eq!(key(l), None, "{l:?}");
        }
    }

    #[cfg(windows)]
    #[test]
    fn a_runnable_suffix_beats_the_bare_shell_script_of_the_same_name() {
        let dir = std::env::temp_dir().join(format!("forge-which-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("fakecli"), "#!/bin/sh").unwrap();
        std::fs::write(dir.join("fakecli.cmd"), "@echo off").unwrap();
        let found = which_in("fakecli", std::slice::from_ref(&dir));
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(found.and_then(|p| p.extension().map(|e| e.to_string_lossy().into_owned())).as_deref(), Some("cmd"));
    }

    #[test]
    fn keys_are_unique_and_every_server_has_a_command_and_install_route() {
        let specs = all_specs();
        let mut keys = std::collections::HashSet::new();
        for s in &specs {
            assert!(keys.insert(s.key.clone()), "duplicate key {}", s.key);
            assert!(!s.command.is_empty() && !s.title.is_empty());
            assert!(!languages_of(&s.key).is_empty());
        }
        assert!(specs.len() >= 18, "{} servers", specs.len());
        // The shared npm package installs three servers.
        let npm: Vec<_> = specs.iter().filter(|s| matches!(&s.install, Install::Npm(p) if p == &["vscode-langservers-extracted"])).collect();
        assert_eq!(npm.len(), 3);
    }

    #[test]
    fn settings_overrides_replace_the_builtin_choice_for_that_language_only() {
        let mut o = none();
        o.insert("python".to_owned(), ("my-pylsp".to_owned(), vec!["--flag".to_owned()]));
        let py = spec_for(Language::Python, &o).unwrap();
        assert_eq!((py.command.as_str(), py.args.clone(), py.custom, py.key.as_str()), ("my-pylsp", vec!["--flag".to_owned()], true, "custom:python"));
        assert_eq!(spec_for(Language::Go, &o).unwrap().key, "go");
        // A custom server is never considered installable by Forge.
        assert!(matches!(py.install, Install::Manual(_)));
    }

    #[test]
    fn which_in_finds_executables_with_windows_suffixes() {
        let dir = std::env::temp_dir().join(format!("forge-servers-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let name = if cfg!(windows) { "fake-server.cmd" } else { "fake-server" };
        std::fs::write(dir.join(name), "").unwrap();
        assert!(which_in("fake-server", std::slice::from_ref(&dir)).is_some());
        assert!(which_in("absent-server", std::slice::from_ref(&dir)).is_none());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn missing_servers_explain_how_to_get_them() {
        let spec = ServerSpec {
            key: "x".into(),
            title: "Thing (thing-ls)".into(),
            command: "definitely-not-installed-thing-ls".into(),
            args: vec![],
            install: Install::Manual("cargo install thing-ls"),
            custom: false,
        };
        assert_eq!(resolve(&spec).unwrap_err(), "Thing (thing-ls) is not installed: cargo install thing-ls.");
        let npm = ServerSpec { install: Install::Npm(&["thing"]), ..spec.clone() };
        assert!(resolve(&npm).unwrap_err().contains("Language Servers: Manage and Install"));
        assert!(!is_installed(&npm));
    }

    #[test]
    fn args_expand_the_managed_folder_and_typescript_gets_init_options_only_when_present() {
        let xml = spec_for(Language::Xml, &none()).unwrap();
        assert!(expanded_args(&xml)[1].ends_with("bin/lemminx.jar") || expanded_args(&xml)[1].ends_with("bin\\lemminx.jar"));
        assert!(!expanded_args(&xml)[1].contains("{managed}"));
        let rust = spec_for(Language::Rust, &none()).unwrap();
        assert_eq!(init_options(&rust), None);
    }
}
