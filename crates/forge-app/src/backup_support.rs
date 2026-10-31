//! Hot exit: unsaved edits and untitled files are kept on disk as they change and brought back the next time the
//! folder is opened, so closing the window (or a crash) loses nothing and nothing asks to discard.

use std::time::{Duration, Instant};

use forge_core::backup::{self, Backup};
use forge_syntax::Language;
use gpui::Context;

use crate::workspace::{LangChoice, Workspace};

/// The backups are rewritten at most this often.
const EVERY: Duration = Duration::from_secs(1);

impl Workspace {
    /// What is unsaved right now: files with changes, and untitled files with text.
    pub fn backup_snapshot(&self) -> Vec<Backup> {
        let caret = |doc_ix: usize| self.groups.iter().flat_map(|g| g.views.iter()).find(|v| v.doc == doc_ix).map_or(0, |v| v.selections.primary().head);
        self.docs
            .iter()
            .enumerate()
            .filter(|(_, d)| !d.readonly && !d.diff && d.ext_page.is_none() && d.buffer.len_chars() <= backup::MAX_BYTES && (if d.untitled { d.buffer.len_chars() > 0 } else { d.dirty }))
            .map(|(ix, d)| Backup {
                path: (!d.untitled).then(|| d.path.clone()),
                name: d.name(),
                text: d.buffer.text(),
                caret: caret(ix),
                mtime: if d.untitled { None } else { backup::mtime_ms(&d.path) },
                language: d.lang_manual.then(|| d.language.map_or("plaintext", |l| l.vscode_id()).to_owned()),
            })
            .collect()
    }

    /// Write the backups if what is unsaved changed since they were last written. `force` skips the throttle.
    pub fn save_backups(&mut self, force: bool) {
        // Until the last session is restored, the files on disk are what is waiting to be restored.
        if !self.session_ready || (!force && self.backup_checked.elapsed() < EVERY) {
            return;
        }
        self.backup_checked = Instant::now();
        let signature: Vec<(String, u64, usize)> = self
            .docs
            .iter()
            .filter(|d| !d.readonly && !d.diff && (if d.untitled { d.buffer.len_chars() > 0 } else { d.dirty }))
            .map(|d| (d.path.to_string_lossy().into_owned(), d.revision, d.buffer.len_chars()))
            .collect();
        if self.backup_signature.as_ref() == Some(&signature) {
            return;
        }
        let snapshot = self.backup_snapshot();
        match backup::store(self.root(), &snapshot) {
            Ok(()) => self.backup_signature = Some(signature),
            Err(error) => eprintln!("[backup] {error}"),
        }
    }

    /// Bring back what was unsaved when Forge last closed in this folder.
    pub fn restore_backups(&mut self, cx: &mut Context<Self>) {
        let backups = backup::load(self.root());
        let mut restored = 0;
        let mut recovered = Vec::new();
        for item in backups {
            let into_file = item.path.as_ref().filter(|p| p.is_file() && backup::mtime_ms(p) == item.mtime);
            match (&item.path, into_file) {
                (Some(_), Some(path)) => {
                    let path = path.clone();
                    self.open_file(&path, None, cx);
                    self.fill_active(&item.text, item.caret, cx);
                }
                (Some(path), None) => {
                    // The file changed or went away since: the text goes into an untitled tab instead of over it.
                    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                    self.new_untitled(cx);
                    self.fill_active(&item.text, item.caret, cx);
                    recovered.push(name);
                }
                (None, _) => {
                    self.new_untitled_named(&item.name, cx);
                    self.fill_active(&item.text, item.caret, cx);
                }
            }
            if let Some(choice) = item.language.as_deref().map(|id| Language::ALL.iter().find(|l| l.vscode_id() == id).map_or(LangChoice::PlainText, |l| LangChoice::Lang(*l))) {
                if let Some(ix) = self.active_doc_index() {
                    self.choose_language(ix, choice, cx);
                }
            }
            restored += 1;
        }
        if !recovered.is_empty() {
            self.set_status(format!("{} changed on disk: its unsaved edits were recovered into an untitled tab", recovered.join(", ")));
        } else if restored > 0 {
            self.set_status(format!("Restored {restored} unsaved file{}", if restored == 1 { "" } else { "s" }));
        }
    }

    /// Replace everything in the active document with `text` as an edit (so language servers, highlighting and
    /// undo all see it), and put the caret back.
    fn fill_active(&mut self, text: &str, caret: usize, cx: &mut Context<Self>) {
        let text = text.to_owned();
        self.mutate(cx, move |b, s| {
            let end = b.len_chars();
            b.replace_range(s, 0, end, &text)
        });
        let len = self.active_doc().map_or(0, |d| d.buffer.len_chars());
        self.place_caret_offset(caret.min(len));
    }

    pub fn active_doc_index(&self) -> Option<usize> {
        self.active_view().map(|v| v.doc)
    }
}
