//! Remembering what was open: saved as tabs and carets change, restored when the folder is opened again.

use std::path::Path;
use std::time::Instant;

use forge_core::session::{Session, SessionFile, SessionStore};
use gpui::Context;

use crate::workspace::{same_path, Workspace};

impl Workspace {
    /// What is open in the first editor group, as it would be saved. Generated documents (diffs,
    /// commits) and files that are gone are left out.
    pub fn session_snapshot(&self) -> Session {
        let Some(group) = self.groups.first() else { return Session::default() };
        let mut files = Vec::new();
        let mut active = 0;
        for (vi, view) in group.views.iter().enumerate() {
            let doc = &self.docs[view.doc];
            if doc.readonly || !doc.path.is_file() {
                continue;
            }
            if vi == group.active {
                active = files.len();
            }
            let (line, col) = doc.buffer.utf16_point(view.selections.primary().head);
            files.push(SessionFile { path: doc.path.clone(), line, col });
        }
        Session { files, active }
    }

    /// Write the session to disk if it differs from what was last written. `force` skips the
    /// once-a-second throttle (used when the window closes or the folder changes).
    pub fn save_session(&mut self, force: bool) {
        if !self.session_ready || (!force && self.session_checked.elapsed().as_millis() < 1000) {
            return;
        }
        self.session_checked = Instant::now();
        let snapshot = self.session_snapshot();
        if self.session_saved.as_ref() == Some(&snapshot) {
            return;
        }
        let mut store = SessionStore::load();
        store.remember(self.root(), snapshot.clone());
        match store.save() {
            Ok(()) => self.session_saved = Some(snapshot),
            Err(error) => eprintln!("[session] {error}"),
        }
    }

    /// Open the files that were open in this folder last time, with their carets, and make the
    /// saved tab active. `also` (a file named on the command line) is opened last, so it is the active one.
    pub fn restore_session(&mut self, also: Option<&Path>, cx: &mut Context<Self>) {
        let store = SessionStore::load();
        let root = self.root().to_path_buf();
        if let Some(session) = store.session(&root).cloned() {
            let mut opened = 0;
            for file in session.files.iter().filter(|f| f.path.is_file()) {
                self.open_file(&file.path, Some((file.line, file.col)), cx);
                opened += 1;
            }
            let saved_active = session.files.get(session.active).map(|f| f.path.clone());
            if let Some(path) = saved_active {
                if let Some(vi) = self.groups[0].views.iter().position(|v| same_path(&self.docs[v.doc].path, &path)) {
                    self.groups[0].active = vi;
                    self.reveal_caret(0, vi, true);
                }
            }
            if opened > 0 {
                self.set_status(format!("Restored {opened} open file{}", if opened == 1 { "" } else { "s" }));
            }
        }
        if let Some(file) = also {
            self.open_file(file, None, cx);
        }
        self.session_saved = Some(self.session_snapshot());
        self.session_ready = true;
    }
}

#[cfg(test)]
mod tests {
    use forge_core::session::SessionStore;

    #[test]
    fn nothing_is_restored_from_an_empty_store() {
        assert!(SessionStore::default().session(std::path::Path::new("/nowhere")).is_none());
    }
}
