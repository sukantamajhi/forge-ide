//! "Language Servers: Manage and Install...": list every server, install the missing ones.

use std::sync::mpsc;

use forge_core::servers::{self, Install, ServerSpec};

use crate::workspace::{Item, ItemAction, Workspace};

fn installable(spec: &ServerSpec) -> bool {
    matches!(spec.install, Install::Npm(_) | Install::Download { .. })
}

impl Workspace {
    /// One row per built-in server, preceded by an "install everything missing" row.
    pub fn server_items(&self, query: &str) -> Vec<Item> {
        let specs = servers::all_specs();
        let missing = specs.iter().filter(|s| installable(s) && !servers::is_installed(s)).count();
        let mut items = Vec::new();
        if query.trim().is_empty() {
            items.push(Item {
                label: "Install all missing servers".to_owned(),
                detail: if missing == 0 { "everything available is installed".to_owned() } else { format!("{missing} to install") },
                positions: Vec::new(),
                action: ItemAction::ServersInstallAll,
            });
        }
        for (i, spec) in specs.iter().enumerate() {
            if !query.trim().is_empty() && !spec.title.to_lowercase().contains(&query.to_lowercase()) {
                continue;
            }
            let detail = match (servers::resolve(spec), &spec.install) {
                (Ok(path), _) => format!("installed: {}", path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()),
                (Err(_), Install::Npm(_)) => "not installed - npm".to_owned(),
                (Err(_), Install::Download { .. }) => "not installed - download".to_owned(),
                (Err(_), Install::Manual(how)) => format!("manual: {how}"),
            };
            items.push(Item { label: spec.title.clone(), detail, positions: Vec::new(), action: ItemAction::Server(i) });
        }
        items
    }

    pub fn perform_server_item(&mut self, action: ItemAction) {
        let specs = servers::all_specs();
        match action {
            ItemAction::ServersInstallAll => {
                let todo: Vec<ServerSpec> = specs.into_iter().filter(|s| installable(s) && !servers::is_installed(s)).collect();
                if todo.is_empty() {
                    self.set_status("Every installable language server is already installed");
                } else {
                    self.install_servers(todo);
                }
            }
            ItemAction::Server(i) => {
                let Some(spec) = specs.get(i).cloned() else { return };
                match servers::resolve(&spec) {
                    Ok(path) => self.set_status(format!("{} is installed at {}", spec.title, path.display())),
                    Err(reason) if !installable(&spec) => self.set_status(reason),
                    Err(_) => self.install_servers(vec![spec]),
                }
            }
            _ => {}
        }
    }

    /// Install servers on a background thread. npm packages are installed in one command.
    pub fn install_servers(&mut self, specs: Vec<ServerSpec>) {
        if self.install_rx.is_some() {
            self.set_status("A language server install is already running");
            return;
        }
        let mut packages: Vec<&'static str> = Vec::new();
        let mut downloads: Vec<(&'static str, &'static str)> = Vec::new();
        for spec in &specs {
            match &spec.install {
                Install::Npm(p) => {
                    for package in p.iter().copied() {
                        if !packages.contains(&package) {
                            packages.push(package);
                        }
                    }
                }
                Install::Download { url, file } => downloads.push((url, file)),
                Install::Manual(_) => {}
            }
        }
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let mut done = Vec::new();
            if !packages.is_empty() {
                match servers::install_npm(&packages) {
                    Ok(m) => done.push(m),
                    Err(e) => {
                        let _ = tx.send(Err(format!("npm: {e}")));
                        return;
                    }
                }
            }
            for (url, file) in downloads {
                match servers::install_download(url, file) {
                    Ok(m) => done.push(m),
                    Err(e) => {
                        let _ = tx.send(Err(format!("{file}: {e}")));
                        return;
                    }
                }
            }
            let _ = tx.send(Ok(done.join("; ")));
        });
        self.install_rx = Some(rx);
        self.lsp_progress = Some("Installing language servers...".to_owned());
        self.set_status("Installing language servers in the background...");
    }

    /// Download and install a debug adapter on a background thread (only when the user asked):
    /// CodeLLDB for Rust if `rust`, otherwise js-debug for JavaScript.
    pub fn install_debug_adapter(&mut self, rust: bool) {
        if self.install_rx.is_some() {
            self.set_status("An install is already running");
            return;
        }
        let (tx, rx) = mpsc::channel();
        let dir = forge_core::settings::config_dir().join("servers");
        std::thread::spawn(move || {
            let _ = std::fs::create_dir_all(&dir);
            let _ = tx.send(if rust { forge_core::debug_install::install_codelldb(&dir) } else { forge_core::debug_install::install_js_debug(&dir) });
        });
        self.install_rx = Some(rx);
        self.set_status(if rust { "Downloading CodeLLDB from github.com/vadimcn/codelldb (about 50 MB)..." } else { "Downloading the JavaScript debugger from github.com/microsoft/vscode-js-debug..." });
    }

    /// Pick up a finished install and start servers that were waiting for it.
    pub fn poll_install(&mut self) -> bool {
        let Some(rx) = &self.install_rx else { return false };
        let result = match rx.try_recv() {
            Ok(result) => result,
            Err(mpsc::TryRecvError::Empty) => return false,
            Err(mpsc::TryRecvError::Disconnected) => Err("the installer stopped unexpectedly".to_owned()),
        };
        self.install_rx = None;
        self.lsp_progress = None;
        match result {
            Ok(message) => {
                self.set_status(format!("{message}. Starting servers..."));
                self.lsp.forget_unavailable();
                for ix in 0..self.docs.len() {
                    if !self.docs[ix].lsp_open {
                        self.attach_lsp(ix);
                    }
                }
            }
            Err(error) => self.set_status(format!("Install failed: {error}")),
        }
        true
    }
}
