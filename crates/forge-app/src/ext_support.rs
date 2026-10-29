//! Extensions in the app: install from Open VSX or a `.vsix`, list and remove them, and
//! use the color themes they contribute.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::mpsc::{self, TryRecvError};

use forge_core::ext_lang::{parse_language_configuration, ExtLanguage};
use forge_core::settings::config_dir;
use forge_core::theme::{Theme, THEME_NAMES};
use forge_core::vscode_theme;
use forge_ext_host::openvsx::{self, Listing};
use forge_ext_host::{self as ext, Extension};
use forge_syntax::{Language, TextMateRegistry};

use crate::workspace::{Item, ItemAction, Workspace};

/// What a background extension job reports back.
pub(crate) enum ExtEvent {
    Found(Result<Vec<Listing>, String>),
    /// `Ok` carries the message to show.
    Installed(Result<String, String>),
}

pub fn extensions_dir() -> PathBuf {
    config_dir().join("extensions")
}

pub fn load_extensions() -> Vec<Extension> {
    ext::discover(&extensions_dir()).map(|(found, _)| found).unwrap_or_default()
}

/// A built-in theme, or one contributed by an installed extension. Unknown names give the default.
pub fn resolve_theme(extensions: &[Extension], name: &str) -> Theme {
    if !THEME_NAMES.contains(&name) {
        for extension in extensions {
            for theme in &extension.manifest.contributes.themes {
                if extension.localize(&theme.label) != name {
                    continue;
                }
                let Some(path) = extension.resolve(&theme.path) else { continue };
                if let Ok(loaded) = vscode_theme::load_theme_file(&path, name, theme.ui_theme.as_deref()) {
                    return loaded;
                }
            }
        }
    }
    Theme::by_name(name)
}

/// Read the languages and TextMate grammars the installed extensions contribute. Languages
/// Forge bundles keep their tree-sitter grammar. The strings say what could not be used.
pub fn build_ext_languages(extensions: &[Extension]) -> (Vec<ExtLanguage>, TextMateRegistry, Vec<String>) {
    let registry = TextMateRegistry::new();
    let mut warnings = Vec::new();
    // Which registered grammar colors each language id.
    let mut grammar_of: HashMap<String, String> = HashMap::new();
    for extension in extensions {
        for grammar in &extension.manifest.contributes.grammars {
            if grammar.scope_name.is_empty() {
                continue;
            }
            let Some(path) = extension.resolve(&grammar.path) else {
                warnings.push(format!("{}: grammar path {:?} leaves the extension folder", extension.id(), grammar.path));
                continue;
            };
            if !path.extension().is_some_and(|e| e.eq_ignore_ascii_case("json")) {
                warnings.push(format!("{}: grammar {} is not JSON (plist and YAML grammars are not read yet)", extension.id(), grammar.path));
                continue;
            }
            match std::fs::read(&path).map_err(|e| e.to_string()).and_then(|bytes| registry.add_grammar(&grammar.scope_name, &bytes)) {
                Ok(()) => {
                    if let Some(language) = &grammar.language {
                        grammar_of.entry(language.clone()).or_insert_with(|| grammar.scope_name.clone());
                    }
                }
                Err(error) => warnings.push(format!("{}: grammar {}: {error}", extension.id(), grammar.path)),
            }
        }
    }
    let mut languages = Vec::new();
    for extension in extensions {
        for language in &extension.manifest.contributes.languages {
            if language.id.is_empty() || Language::from_name(&language.id).is_some() {
                continue;
            }
            let config = language
                .configuration
                .as_deref()
                .and_then(|c| extension.resolve(c))
                .and_then(|p| std::fs::read_to_string(p).ok())
                .and_then(|text| parse_language_configuration(&text).ok())
                .unwrap_or_default();
            languages.push(ExtLanguage {
                id: language.id.clone(),
                name: language.aliases.first().cloned().unwrap_or_else(|| language.id.clone()),
                extensions: language.extensions.clone(),
                filenames: language.filenames.clone(),
                patterns: language.filename_patterns.clone(),
                config,
                grammar: grammar_of.get(&language.id).cloned(),
            });
        }
    }
    (languages, registry, warnings)
}

fn short(text: &str, max: usize) -> String {
    let flat: String = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() <= max {
        flat
    } else {
        format!("{}...", flat.chars().take(max).collect::<String>().trim_end())
    }
}

impl Workspace {
    /// Give `doc` an extension's language when Forge has no built-in one for its file name.
    pub fn attach_ext_language(&self, doc: &mut crate::workspace::Doc) {
        if doc.language.is_some() || doc.readonly {
            return;
        }
        let name = doc.path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        if let Some(language) = forge_core::ext_lang::detect(&self.ext_langs, &name) {
            doc.use_ext_language(language, &self.tm);
        }
    }

    /// Reload the languages of the installed extensions and re-detect open files.
    pub fn refresh_ext_languages(&mut self) {
        let (languages, registry, _) = build_ext_languages(&self.extensions);
        self.ext_langs = languages;
        self.tm = registry;
        for ix in 0..self.docs.len() {
            let mut doc = std::mem::replace(&mut self.docs[ix], crate::workspace::Doc::new_virtual(PathBuf::new(), "", "", false));
            doc.ext_lang = None;
            doc.tm = None;
            self.attach_ext_language(&mut doc);
            self.docs[ix] = doc;
        }
    }

    /// Built-in themes followed by those of installed extensions.
    pub fn theme_items(&self, query: &str) -> Vec<Item> {
        let query = query.to_lowercase();
        let wanted = |name: &str| query.is_empty() || name.to_lowercase().contains(&query);
        let mut items = Vec::new();
        let mut add = |name: String, source: String| {
            let detail = if name == self.theme.name { "current".to_owned() } else { source };
            items.push(Item { label: name.clone(), detail, positions: Vec::new(), action: ItemAction::Theme(name) });
        };
        for name in THEME_NAMES.iter().filter(|n| wanted(n)) {
            add((*name).to_owned(), String::new());
        }
        for extension in &self.extensions {
            for theme in &extension.manifest.contributes.themes {
                let label = extension.localize(&theme.label);
                if wanted(&label) {
                    add(label, extension.display_name());
                }
            }
        }
        items
    }

    pub fn ext_result_items(&self, query: &str) -> Vec<Item> {
        let query = query.to_lowercase();
        self.ext_listings
            .iter()
            .enumerate()
            .filter(|(_, l)| query.is_empty() || l.display_name.to_lowercase().contains(&query) || l.id().to_lowercase().contains(&query))
            .map(|(i, l)| Item {
                label: format!("{}  ({})", l.display_name, l.id()),
                detail: format!("v{} - {}", l.version, short(&l.description, 48)),
                positions: Vec::new(),
                action: ItemAction::ExtListing(i),
            })
            .collect()
    }

    pub fn ext_installed_items(&self, query: &str) -> Vec<Item> {
        let query = query.to_lowercase();
        self.extensions
            .iter()
            .enumerate()
            .filter(|(_, e)| query.is_empty() || e.display_name().to_lowercase().contains(&query) || e.id().to_lowercase().contains(&query))
            .map(|(i, e)| Item {
                label: format!("{} {}", e.display_name(), e.manifest.version),
                detail: format!("{} - Enter uninstalls", e.supported_summary()),
                positions: Vec::new(),
                action: ItemAction::ExtInstalled(i),
            })
            .collect()
    }

    /// Search Open VSX on a background thread. A `publisher.name` that matches nothing is looked up directly.
    pub fn start_ext_search(&mut self, query: String) {
        if self.ext_rx.is_some() {
            self.set_status("An extension job is already running");
            return;
        }
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let mut found = openvsx::search(&query, 25);
            // An exact `publisher.name` goes first, whether or not the search ranked it.
            if query.contains('.') && !query.contains(' ') {
                if let Ok(exact) = openvsx::lookup(&query) {
                    let mut list = found.unwrap_or_default();
                    list.retain(|l| l.id() != exact.id());
                    list.insert(0, exact);
                    found = Ok(list);
                }
            }
            let _ = tx.send(ExtEvent::Found(found));
        });
        self.ext_rx = Some(rx);
        self.set_status("Searching Open VSX...");
    }

    pub fn start_ext_install(&mut self, listing: Listing) {
        if self.ext_rx.is_some() {
            self.set_status("An extension job is already running");
            return;
        }
        let (tx, rx) = mpsc::channel();
        let label = listing.id();
        let file_name = format!("forge-{}-{}.vsix", std::process::id(), label.replace(['/', '\\', ':'], "_"));
        std::thread::spawn(move || {
            let file = std::env::temp_dir().join(file_name);
            let result = openvsx::download(&listing, &file).and_then(|()| install_file(&file));
            let _ = std::fs::remove_file(&file);
            let _ = tx.send(ExtEvent::Installed(result));
        });
        self.ext_rx = Some(rx);
        self.set_status(format!("Installing {label} from Open VSX..."));
    }

    pub fn start_vsix_install(&mut self, path: String) {
        if self.ext_rx.is_some() {
            self.set_status("An extension job is already running");
            return;
        }
        let file = PathBuf::from(path.trim().trim_matches('"'));
        if !file.is_file() {
            self.set_status(format!("{} is not a file", file.display()));
            return;
        }
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(ExtEvent::Installed(install_file(&file)));
        });
        self.ext_rx = Some(rx);
        self.set_status("Installing extension...");
    }

    /// Pick up a finished search or install.
    pub fn poll_ext(&mut self) -> bool {
        let Some(rx) = &self.ext_rx else { return false };
        let event = match rx.try_recv() {
            Ok(event) => event,
            Err(TryRecvError::Empty) => return false,
            Err(TryRecvError::Disconnected) => ExtEvent::Installed(Err("the extension job stopped unexpectedly".to_owned())),
        };
        self.ext_rx = None;
        match event {
            ExtEvent::Found(Ok(listings)) if listings.is_empty() => self.set_status("No extensions found on Open VSX"),
            ExtEvent::Found(Ok(listings)) => {
                self.ext_listings = listings;
                self.set_status("Choose an extension to install");
                self.open_overlay(crate::workspace::OverlayKind::ExtResults, "");
            }
            ExtEvent::Found(Err(error)) => self.set_status(format!("Open VSX search failed: {error}")),
            ExtEvent::Installed(Ok(message)) => {
                self.extensions = load_extensions();
                self.refresh_ext_languages();
                self.snippets = crate::snippet_support::load_snippets(&self.extensions);
                self.theme = resolve_theme(&self.extensions, &self.settings.theme);
                self.restart_host();
                self.set_status(message);
            }
            ExtEvent::Installed(Err(error)) => self.set_status(format!("Extension install failed: {error}")),
        }
        true
    }

    pub fn perform_ext_item(&mut self, action: ItemAction) {
        match action {
            ItemAction::ExtListing(i) => {
                if let Some(listing) = self.ext_listings.get(i).cloned() {
                    self.start_ext_install(listing);
                }
            }
            ItemAction::ExtInstalled(i) => {
                let Some(extension) = self.extensions.get(i) else { return };
                let id = extension.id();
                match ext::uninstall(&extensions_dir(), &id) {
                    Ok(_) => {
                        self.extensions = load_extensions();
                        self.refresh_ext_languages();
                        self.snippets = crate::snippet_support::load_snippets(&self.extensions);
                        self.theme = resolve_theme(&self.extensions, &self.settings.theme);
                        self.restart_host();
                        self.set_status(format!("Uninstalled {id}"));
                    }
                    Err(error) => self.set_status(format!("Could not uninstall {id}: {error:#}")),
                }
            }
            _ => {}
        }
    }
}

/// Install a `.vsix` and describe what came of it.
fn install_file(file: &std::path::Path) -> Result<String, String> {
    let installed = ext::vsix::install_vsix(file, &extensions_dir()).map_err(|e| format!("{e:#}"))?;
    let themes = installed.manifest.contributes.themes.len();
    let hint = if themes > 0 { " Use \"Preferences: Color Theme\" to pick one." } else { "" };
    Ok(format!("Installed {} {} ({}).{hint}", installed.id(), installed.manifest.version, installed.supported_summary()))
}
