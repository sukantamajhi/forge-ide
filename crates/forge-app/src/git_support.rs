//! Source control: runs git on background threads and keeps the editor's view of the
//! repository (status, HEAD contents, change gutters, diff tabs) up to date.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use forge_core::commands::Command;
use forge_core::fileops;
use forge_core::git::{self, relative_path, Branch};
use forge_core::gutter::line_changes;
use forge_syntax::Syntax;
use forge_text::{Buffer, Selection, SelectionSet};
use gpui::Context;

use crate::workspace::{Doc, Focus, GitMsg, GitState, OverlayKind, SidebarView, View, Workspace};

/// How often status is re-read to notice changes made outside the editor.
const AUTO_REFRESH: Duration = Duration::from_secs(4);

/// Present-tense title shown in the status bar while an operation runs.
fn job_title(label: &str) -> &'static str {
    match label {
        "pull" => "Pulling…",
        "sync" => "Synchronizing…",
        "push" => "Pushing…",
        "fetch" => "Fetching…",
        "commit" => "Committing…",
        "checkout" | "branch" => "Switching branch…",
        "stash" | "stash-pop" => "Stashing…",
        "init" => "Initializing…",
        _ => "Working…",
    }
}

impl Workspace {
    pub fn init_git(&mut self) {
        self.git = None;
        if !git::is_available() {
            return;
        }
        if let Some(root) = git::discover(self.tree.root()) {
            self.git = Some(GitState {
                root,
                status: Default::default(),
                refreshing: false,
                refresh_wanted: true,
                last_refresh: Instant::now(),
                head: HashMap::new(),
                head_pending: HashSet::new(),
                epoch: 0,
                decor: HashMap::new(),
                job: None,
            });
        }
    }

    pub fn git_request_refresh(&mut self) {
        if let Some(g) = self.git.as_mut() {
            g.refresh_wanted = true;
        }
    }

    fn start_status(&mut self) {
        let Some(g) = self.git.as_mut() else { return };
        if g.refreshing {
            return;
        }
        g.refreshing = true;
        g.refresh_wanted = false;
        g.last_refresh = Instant::now();
        let (root, tx) = (g.root.clone(), self.git_tx.clone());
        std::thread::spawn(move || {
            let _ = tx.send(GitMsg::Status(git::status(&root)));
        });
    }

    /// Run a mutating git operation off the UI thread; its result arrives as [`GitMsg::Done`].
    pub fn git_job(&mut self, label: &str, f: impl FnOnce(&Path) -> Result<String, String> + Send + 'static) {
        let Some(g) = self.git.as_mut() else {
            self.set_status("This folder is not a git repository");
            return;
        };
        if let Some(running) = &g.job {
            let running = running.clone();
            self.set_status(format!("Waiting for the running operation: {running}"));
            return;
        }
        g.job = Some(job_title(label).to_owned());
        let (root, tx, label) = (g.root.clone(), self.git_tx.clone(), label.to_owned());
        std::thread::spawn(move || {
            let result = f(&root);
            let _ = tx.send(GitMsg::Done { label, result });
        });
    }

    fn git_background<T: Send + 'static>(&self, make: impl FnOnce(&Path) -> T + Send + 'static, wrap: impl FnOnce(T) -> GitMsg + Send + 'static) {
        let Some(g) = self.git.as_ref() else { return };
        let (root, tx) = (g.root.clone(), self.git_tx.clone());
        std::thread::spawn(move || {
            let _ = tx.send(wrap(make(&root)));
        });
    }

    /// Drain results and refresh on a timer. True if anything changed.
    pub fn git_tick(&mut self, cx: &mut Context<Self>) -> bool {
        let mut changed = false;
        while let Ok(msg) = self.git_rx.try_recv() {
            changed = true;
            self.handle_git_msg(msg, cx);
        }
        let due = self.git.as_ref().is_some_and(|g| !g.refreshing && (g.refresh_wanted || g.last_refresh.elapsed() > AUTO_REFRESH));
        if due {
            self.start_status();
        }
        changed
    }

    fn handle_git_msg(&mut self, msg: GitMsg, cx: &mut Context<Self>) {
        match msg {
            GitMsg::Status(Ok(status)) => self.apply_status(status),
            GitMsg::Status(Err(_)) => {
                if let Some(g) = self.git.as_mut() {
                    g.refreshing = false;
                }
            }
            GitMsg::Head(path, content) => {
                if let Some(g) = self.git.as_mut() {
                    g.head_pending.remove(&path);
                    g.head.insert(path, content);
                    g.epoch += 1;
                }
            }
            GitMsg::Done { label, result } => {
                if let Some(g) = self.git.as_mut() {
                    g.job = None;
                    g.head.clear();
                    g.head_pending.clear();
                    g.epoch += 1;
                    g.refresh_wanted = true;
                }
                match result {
                    Ok(message) => {
                        if label == "commit" {
                            self.scm_message.set_text("");
                        }
                        if label == "init" {
                            self.init_git();
                        }
                        self.set_status(if message.is_empty() { "Done".to_owned() } else { message });
                    }
                    Err(error) => self.set_status(format!("Git: {error}")),
                }
                // Checkout, pull, stash and discard rewrite files under the open tabs.
                self.reload_clean_docs();
                self.tree.refresh();
                self.file_index = None;
            }
            GitMsg::Diff { key, title, result } => match result {
                Ok(text) => self.open_virtual(&key, &title, &text, true, cx),
                Err(error) => self.set_status(format!("Git: {error}")),
            },
            GitMsg::AiMessage(result) => {
                self.ai_pending = false;
                match result {
                    Ok(message) => {
                        self.scm_message.set_text(&message);
                        self.sidebar_visible = true;
                        self.sidebar_view = SidebarView::Git;
                        self.focus = Focus::Scm;
                        self.set_status("Generated a commit message. Edit it if needed, then press Enter to commit.");
                    }
                    Err(error) => {
                        // A rejected key is forgotten so the next try asks for a new one.
                        if error.contains("API key") {
                            self.ai_key = None;
                            let _ = forge_core::ai::forget_saved_key();
                        }
                        self.set_status(format!("AI commit message: {error}"));
                    }
                }
            }
            GitMsg::Branches(Ok(list)) => {
                self.branches = list;
                self.refresh_overlay();
            }
            GitMsg::History(Ok(list)) => {
                self.history = list;
                self.refresh_overlay();
            }
            GitMsg::Branches(Err(e)) | GitMsg::History(Err(e)) => {
                self.overlay = None;
                self.set_status(format!("Git: {e}"));
            }
            GitMsg::Blame(Ok(b)) => self.set_status(if b.uncommitted {
                "Not committed yet".to_owned()
            } else {
                format!("{}, {} • {} ({})", b.author, b.when, b.summary, b.hash)
            }),
            GitMsg::Blame(Err(e)) => self.set_status(format!("Git: {e}")),
        }
    }

    fn apply_status(&mut self, status: git::Status) {
        let Some(g) = self.git.as_mut() else { return };
        g.refreshing = false;
        if g.status.head != status.head || g.status.branch != status.branch {
            g.head.clear();
            g.head_pending.clear();
        }
        if g.status != status {
            g.epoch += 1;
        }
        g.decor.clear();
        for change in &status.changes {
            let abs = g.root.join(&change.path);
            for ancestor in abs.ancestors().skip(1) {
                if ancestor.starts_with(&g.root) && ancestor != g.root {
                    g.decor.entry(ancestor.to_path_buf()).or_insert('•');
                } else {
                    break;
                }
            }
            g.decor.insert(abs, change.letter());
        }
        g.status = status;
    }

    /// Recompute a document's change gutter if its text or the repository state changed.
    pub fn ensure_gutter(&mut self, doc_ix: usize) {
        let Some(git) = self.git.as_mut() else { return };
        let doc = &mut self.docs[doc_ix];
        if doc.readonly {
            return;
        }
        let key = (doc.revision, git.epoch);
        if doc.gutter_key == key {
            return;
        }
        let Some(rel) = relative_path(&git.root, &doc.path) else {
            doc.gutter.clear();
            doc.gutter_key = key;
            return;
        };
        match git.head.get(&doc.path) {
            None => {
                if git.head_pending.insert(doc.path.clone()) {
                    let (root, tx, path) = (git.root.clone(), self.git_tx.clone(), doc.path.clone());
                    std::thread::spawn(move || {
                        let content = git::show_head(&root, &rel).ok().flatten();
                        let _ = tx.send(GitMsg::Head(path, content));
                    });
                }
            }
            Some(head) => {
                let text = doc.buffer.text();
                let in_status = git.status.changes.iter().any(|c| c.path == rel);
                doc.gutter = match head {
                    Some(old) => line_changes(old, &text),
                    // A new file shows as entirely added; an ignored one shows nothing.
                    None if in_status => line_changes("", &text),
                    None => Vec::new(),
                };
                doc.gutter_key = key;
            }
        }
    }

    /// Re-read open, unmodified files whose content changed on disk.
    pub fn reload_clean_docs(&mut self) {
        for ix in 0..self.docs.len() {
            let doc = &self.docs[ix];
            if doc.dirty || doc.readonly {
                continue;
            }
            let Ok(bytes) = std::fs::read(&doc.path) else { continue };
            let Some((text, bom)) = crate::workspace::decode_utf8(bytes) else { continue };
            if text == doc.buffer.text() {
                // Only the byte-order mark may have changed.
                self.docs[ix].bom = bom;
                continue;
            }
            let (language, was_open, path) = (doc.language, doc.lsp_open, doc.path.clone());
            let doc = &mut self.docs[ix];
            doc.buffer = Buffer::new(&text);
            doc.bom = bom;
            // A fresh tree: the old one describes text that no longer exists.
            doc.syntax = language.and_then(Syntax::new).map(|mut s| {
                s.parse(doc.buffer.rope(), &[]);
                s
            });
            doc.revision += 1;
            if let Some(tm) = doc.tm.as_mut() {
                tm.reset();
            }
            doc.set_lsp_diagnostics(Vec::new());
            doc.lsp_open = false;
            if let (Some(language), true) = (language, was_open) {
                self.lsp.did_close(language, &path);
                self.attach_lsp(ix);
            }
            let len = self.docs[ix].buffer.len_chars();
            for (g, vi) in self.views_of(ix).collect::<Vec<_>>() {
                let view = &mut self.groups[g].views[vi];
                let head = view.selections.primary().head.min(len);
                view.selections = SelectionSet::single(Selection::caret(head));
                view.folds_revision = u64::MAX;
            }
        }
    }

    /// Show generated text (a diff or commit) in a read-only tab, replacing an earlier one with the same key.
    pub fn open_virtual(&mut self, key: &str, title: &str, text: &str, diff: bool, cx: &mut Context<Self>) {
        let path = PathBuf::from(format!("git:{key}"));
        let doc = Doc::new_virtual(path.clone(), title, text, diff);
        let ix = match self.doc_index(&path) {
            Some(ix) => {
                self.docs[ix] = doc;
                for (g, vi) in self.views_of(ix).collect::<Vec<_>>() {
                    let view = &mut self.groups[g].views[vi];
                    view.selections = SelectionSet::single(Selection::caret(0));
                    view.folds_revision = u64::MAX;
                }
                ix
            }
            None => {
                self.docs.push(doc);
                self.docs.len() - 1
            }
        };
        let group = self.focused_group;
        match self.groups[group].views.iter().position(|v| v.doc == ix) {
            Some(vi) => self.groups[group].active = vi,
            None => {
                self.groups[group].views.push(View::new(ix));
                self.groups[group].active = self.groups[group].views.len() - 1;
            }
        }
        self.focus = Focus::Editor;
        cx.notify();
    }

    pub fn git_open_diff(&mut self, rel: String, staged: bool, untracked: bool) {
        let name = rel.rsplit('/').next().unwrap_or(&rel).to_owned();
        let state = if staged { "Staged" } else if untracked { "New" } else { "Working Tree" };
        let key = format!("diff:{state}:{rel}");
        let title = format!("{name} ({state})");
        self.git_background(move |root| git::diff_text(root, &rel, staged, untracked), move |result| GitMsg::Diff { key, title, result });
    }

    pub fn active_rel(&self) -> Option<String> {
        let doc = self.active_doc()?;
        if doc.readonly {
            return None;
        }
        relative_path(&self.git.as_ref()?.root, &doc.path)
    }

    pub fn scm_stage(&mut self, rel: String) {
        self.git_job("stage", move |r| git::stage(r, &[rel]).map(|_| "Staged".to_owned()));
    }

    pub fn scm_unstage(&mut self, rel: String) {
        self.git_job("unstage", move |r| git::unstage(r, &[rel]).map(|_| "Unstaged".to_owned()));
    }

    pub fn scm_confirm_discard(&mut self, rel: String, untracked: bool) {
        self.open_overlay(OverlayKind::ConfirmDiscard { rel, untracked }, "");
    }

    pub fn finish_discard(&mut self, rel: &str, untracked: bool) {
        let Some(root) = self.git.as_ref().map(|g| g.root.clone()) else { return };
        if untracked {
            // Untracked files cannot be restored by git, so they go to the Recycle Bin.
            match fileops::move_to_trash(&root.join(rel)) {
                Ok(()) => {
                    self.git_request_refresh();
                    self.tree.refresh();
                    self.set_status(format!("Moved {rel} to the Recycle Bin"));
                }
                Err(e) => self.set_status(e),
            }
        } else {
            let rel = rel.to_owned();
            self.git_job("discard", move |r| git::discard(r, &[rel]).map(|_| "Discarded changes".to_owned()));
        }
    }

    pub fn git_commit(&mut self, all: bool) {
        let message = self.scm_message.text().trim().to_owned();
        if message.is_empty() {
            self.sidebar_visible = true;
            self.sidebar_view = SidebarView::Git;
            self.focus = Focus::Scm;
            self.set_status("Enter a commit message, then press Enter");
            return;
        }
        let Some(g) = self.git.as_ref() else { return };
        if !all && g.status.staged().next().is_none() {
            self.set_status("There are no staged changes to commit. Stage changes first, or use Git: Commit All.");
            return;
        }
        self.git_job("commit", move |r| git::commit(r, &message, all));
    }

    /// Ask the model for a commit subject describing the staged changes. Only the staged diff
    /// (minus lock files and likely secrets) and the last few commit subjects are sent.
    pub fn git_generate_message(&mut self) {
        self.sidebar_visible = true;
        self.sidebar_view = SidebarView::Git;
        if self.ai_pending {
            self.set_status("Already generating a commit message");
            return;
        }
        if self.git.as_ref().is_none_or(|g| g.status.staged().next().is_none()) {
            self.set_status("There are no staged changes to describe. Stage changes first.");
            return;
        }
        // An environment variable wins, then the key typed this session, then the one saved
        // (encrypted) by an earlier session. Only when there is none is the user asked.
        if self.ai_key.is_none() {
            self.ai_key = forge_core::ai::load_saved_key();
        }
        let Some(key) = forge_core::ai::api_key().or_else(|| self.ai_key.clone()) else {
            self.open_overlay(OverlayKind::AiKey, "");
            return;
        };
        let model = self.settings.ai_model.clone();
        self.ai_pending = true;
        self.set_status(format!("Generating a commit message with {model}: the staged diff is sent to the OpenAI API..."));
        self.git_background(
            move |root| {
                let diff = git::staged_diff(root)?;
                if diff.trim().is_empty() {
                    return Err("only lock files or secret-like files are staged, so there is nothing to describe".to_owned());
                }
                let recent: Vec<String> = git::log(root, None, 10).map(|log| log.into_iter().map(|c| c.subject).collect()).unwrap_or_default();
                forge_core::ai::generate_commit_message(&key, &model, &diff, &recent)
            },
            GitMsg::AiMessage,
        );
    }

    /// Drop the key from memory and delete the saved copy.
    pub fn forget_ai_key(&mut self) {
        self.ai_key = None;
        self.set_status(match forge_core::ai::forget_saved_key() {
            Ok(()) => "The saved OpenAI API key was removed".to_owned(),
            Err(error) => error,
        });
    }

    pub fn git_request_branches(&mut self) {
        self.git_background(git::branches, GitMsg::Branches);
    }

    pub fn git_request_history(&mut self, rel: Option<String>) {
        self.history_path = rel.clone();
        self.git_background(move |root| git::log(root, rel.as_deref(), 200), GitMsg::History);
    }

    pub fn git_checkout_branch(&mut self, branch: Branch) {
        self.git_job("checkout", move |r| git::checkout(r, &branch).map(|_| format!("Switched to {}", branch.name)));
    }

    pub fn git_show_commit(&mut self, index: usize) {
        let Some(commit) = self.history.get(index).cloned() else { return };
        let rel = self.history_path.clone();
        let short: String = commit.hash.chars().take(8).collect();
        let key = format!("commit:{short}:{}", rel.clone().unwrap_or_default());
        let title = format!("{short} {}", commit.subject.chars().take(40).collect::<String>());
        self.git_background(move |root| git::show_commit(root, &commit.hash, rel.as_deref()), move |result| GitMsg::Diff { key, title, result });
    }

    pub fn run_git_command(&mut self, command: Command, cx: &mut Context<Self>) {
        use Command::*;
        if self.git.is_none() && !matches!(command, GitInit | ShowGit) {
            self.set_status("This folder is not a git repository. Run Git: Initialize Repository.");
            return;
        }
        match command {
            ShowGit => {
                self.sidebar_visible = true;
                self.sidebar_view = SidebarView::Git;
                self.focus = Focus::Scm;
            }
            GitRefresh => {
                self.git_request_refresh();
                self.set_status("Refreshing git status");
            }
            GitInit => {
                if self.git.is_some() {
                    self.set_status("This folder is already a git repository");
                } else if !git::is_available() {
                    self.set_status("git was not found on PATH");
                } else {
                    // `git_job` needs an existing repository, so initialise directly.
                    let (root, tx) = (self.root().to_path_buf(), self.git_tx.clone());
                    std::thread::spawn(move || {
                        let result = git::init(&root).map(|_| "Initialized an empty repository".to_owned());
                        let _ = tx.send(GitMsg::Done { label: "init".to_owned(), result });
                    });
                }
            }
            GitStageAll => self.git_job("stage", |r| git::stage(r, &[".".to_owned()]).map(|_| "Staged all changes".to_owned())),
            GitUnstageAll => self.git_job("unstage", |r| git::unstage(r, &[".".to_owned()]).map(|_| "Unstaged all changes".to_owned())),
            GitStageFile | GitUnstageFile | GitDiscardFile | GitOpenChanges | GitFileHistory | GitBlame => {
                let Some(rel) = self.active_rel() else {
                    self.set_status("The current editor is not a file in this repository");
                    return;
                };
                match command {
                    GitStageFile => self.scm_stage(rel),
                    GitUnstageFile => self.scm_unstage(rel),
                    GitDiscardFile => {
                        let untracked = self.git.as_ref().is_some_and(|g| g.status.changes.iter().any(|c| c.path == rel && c.untracked));
                        self.scm_confirm_discard(rel, untracked);
                    }
                    GitOpenChanges => {
                        let (staged_only, untracked) = self
                            .git
                            .as_ref()
                            .and_then(|g| g.status.changes.iter().find(|c| c.path == rel))
                            .map_or((false, false), |c| (c.is_staged() && c.worktree.is_none(), c.untracked));
                        self.git_open_diff(rel, staged_only, untracked);
                    }
                    GitFileHistory => {
                        self.history.clear();
                        self.open_overlay(OverlayKind::GitHistory, "");
                        self.git_request_history(Some(rel));
                    }
                    _ => {
                        let line = self.active_view().zip(self.active_doc()).map_or(1, |(v, d)| d.buffer.offset_to_point(v.selections.primary().head).0 + 1);
                        self.git_background(move |root| git::blame_line(root, &rel, line), GitMsg::Blame);
                    }
                }
            }
            GitCommit => self.git_commit(false),
            GitCommitAll => self.git_commit(true),
            GitGenerateMessage => self.git_generate_message(),
            GitCheckout => {
                self.branches.clear();
                self.open_overlay(OverlayKind::GitCheckout, "");
                self.git_request_branches();
            }
            GitCreateBranch => self.open_overlay(OverlayKind::GitCreateBranch, ""),
            GitPull => self.git_job("pull", git::pull),
            GitPush => self.git_job("push", git::push),
            GitSync => self.git_job("sync", git::sync),
            GitFetch => self.git_job("fetch", git::fetch),
            GitStash => self.git_job("stash", git::stash),
            GitStashPop => self.git_job("stash-pop", git::stash_pop),
            GitLog => {
                self.history.clear();
                self.open_overlay(OverlayKind::GitHistory, "");
                self.git_request_history(None);
            }
            _ => {}
        }
        cx.notify();
    }
}
