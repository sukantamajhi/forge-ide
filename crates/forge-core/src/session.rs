//! What to bring back when Forge is opened again: for each folder, the files that were open in the
//! first editor group (in tab order), which one was active, and where the caret was in each; and the
//! last folder opened. Kept in `session.json` beside `settings.json`. A missing or damaged file just
//! means starting empty.

use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use crate::settings::config_dir;

/// The folders remembered; the least recently used are forgotten beyond this.
const MAX_FOLDERS: usize = 30;
/// The files remembered per folder.
const MAX_FILES: usize = 60;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SessionFile {
    pub path: PathBuf,
    /// Caret line (0-based) and UTF-16 column, the units language servers use.
    pub line: usize,
    pub col: usize,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Session {
    pub files: Vec<SessionFile>,
    /// Index into `files` of the tab that was active.
    pub active: usize,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SessionStore {
    pub last_folder: Option<PathBuf>,
    /// Most recently used first.
    folders: Vec<(String, Session)>,
    /// Each folder's own spelling (the key above is lower-cased on Windows), for showing in "Open Recent".
    spelled: Vec<(String, PathBuf)>,
}

/// A folder as a comparison key: slashes unified, no trailing slash, case folded on Windows.
fn key(folder: &Path) -> String {
    let text = folder.to_string_lossy().replace('\\', "/");
    let text = text.trim_end_matches('/');
    if cfg!(windows) { text.to_lowercase() } else { text.to_owned() }
}

pub fn path() -> PathBuf {
    config_dir().join("session.json")
}

impl SessionStore {
    pub fn parse(text: &str) -> Self {
        let mut store = Self::default();
        let Ok(Value::Object(map)) = serde_json::from_str::<Value>(text) else { return store };
        store.last_folder = map.get("lastFolder").and_then(Value::as_str).filter(|p| !p.is_empty()).map(PathBuf::from);
        for entry in map.get("folders").and_then(Value::as_array).into_iter().flatten().take(MAX_FOLDERS) {
            let Some(folder) = entry.get("folder").and_then(Value::as_str) else { continue };
            let files: Vec<SessionFile> = entry
                .get("files")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .take(MAX_FILES)
                .filter_map(|f| {
                    Some(SessionFile {
                        path: PathBuf::from(f.get("path")?.as_str()?),
                        line: f.get("line").and_then(Value::as_u64).unwrap_or(0) as usize,
                        col: f.get("col").and_then(Value::as_u64).unwrap_or(0) as usize,
                    })
                })
                .collect();
            let active = entry.get("active").and_then(Value::as_u64).unwrap_or(0) as usize;
            let k = key(Path::new(folder));
            if let Some(path) = entry.get("path").and_then(Value::as_str) {
                store.spelled.push((k.clone(), PathBuf::from(path)));
            }
            store.folders.push((k, Session { active: active.min(files.len().saturating_sub(1)), files }));
            // Keep the folder's own spelling for display and for `last_folder` lookups.
        }
        store
    }

    pub fn to_json(&self) -> String {
        let folders: Vec<Value> = self
            .folders
            .iter()
            .map(|(folder, session)| {
                json!({
                    "folder": folder,
                    "path": self.spelled.iter().find(|(k, _)| k == folder).map(|(_, p)| p),
                    "active": session.active,
                    "files": session.files.iter().map(|f| json!({ "path": f.path, "line": f.line, "col": f.col })).collect::<Vec<_>>(),
                })
            })
            .collect();
        let mut value = json!({ "folders": folders });
        if let Some(last) = &self.last_folder {
            value["lastFolder"] = json!(last);
        }
        serde_json::to_string_pretty(&value).unwrap_or_default() + "\n"
    }

    /// What was open in `folder` last time.
    pub fn session(&self, folder: &Path) -> Option<&Session> {
        let wanted = key(folder);
        self.folders.iter().find(|(k, _)| *k == wanted).map(|(_, s)| s)
    }

    /// Record `session` for `folder`, make it the most recent, and make it the folder to reopen.
    pub fn remember(&mut self, folder: &Path, mut session: Session) {
        session.files.truncate(MAX_FILES);
        session.active = session.active.min(session.files.len().saturating_sub(1));
        let wanted = key(folder);
        self.folders.retain(|(k, _)| *k != wanted);
        self.spelled.retain(|(k, _)| *k != wanted);
        self.spelled.insert(0, (wanted.clone(), folder.to_path_buf()));
        self.folders.insert(0, (wanted, session));
        self.folders.truncate(MAX_FOLDERS);
        self.spelled.truncate(MAX_FOLDERS);
        self.last_folder = Some(folder.to_path_buf());
    }

    /// Folders that were open before, most recent first, spelled as they were opened. Only ones that still exist.
    pub fn recent_folders(&self, limit: usize) -> Vec<PathBuf> {
        self.folders
            .iter()
            .map(|(k, _)| self.spelled.iter().find(|(s, _)| s == k).map_or_else(|| PathBuf::from(k), |(_, p)| p.clone()))
            .filter(|p| p.is_dir())
            .take(limit)
            .collect()
    }

    pub fn load() -> Self {
        std::fs::read_to_string(path()).map(|t| Self::parse(&t)).unwrap_or_default()
    }

    pub fn save(&self) -> Result<(), String> {
        std::fs::create_dir_all(config_dir()).and_then(|()| std::fs::write(path(), self.to_json())).map_err(|e| format!("could not save the session: {e}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(name: &str, line: usize, col: usize) -> SessionFile {
        SessionFile { path: PathBuf::from(name), line, col }
    }

    #[test]
    fn a_session_round_trips_with_its_order_active_tab_and_carets() {
        let mut store = SessionStore::default();
        let session = Session { files: vec![file("C:/p/a.rs", 3, 7), file("C:/p/b.rs", 0, 0), file("C:/p/c.md", 41, 2)], active: 1 };
        store.remember(Path::new("C:/p"), session.clone());
        let back = SessionStore::parse(&store.to_json());
        assert_eq!(back.session(Path::new("C:/p")), Some(&session));
        assert_eq!(back.last_folder, Some(PathBuf::from("C:/p")));
    }

    #[test]
    fn folders_are_matched_regardless_of_slashes_trailing_slash_and_windows_case() {
        let mut store = SessionStore::default();
        store.remember(Path::new("C:\\Work\\App\\"), Session { files: vec![file("x", 0, 0)], active: 0 });
        if cfg!(windows) {
            assert!(store.session(Path::new("c:/work/app")).is_some());
        }
        assert!(store.session(Path::new("C:/Other")).is_none());
    }

    #[test]
    fn the_most_recent_folder_comes_first_and_old_ones_are_forgotten() {
        let mut store = SessionStore::default();
        for i in 0..40 {
            store.remember(Path::new(&format!("/f{i}")), Session::default());
        }
        assert_eq!(store.folders.len(), MAX_FOLDERS);
        assert_eq!(store.last_folder, Some(PathBuf::from("/f39")));
        assert!(store.session(Path::new("/f0")).is_none() && store.session(Path::new("/f39")).is_some());
        // Remembering an existing folder again moves it to the front instead of duplicating it.
        store.remember(Path::new("/f20"), Session::default());
        assert_eq!(store.folders.iter().filter(|(k, _)| k.ends_with("f20")).count(), 1);
        assert!(store.folders[0].0.ends_with("f20"));
    }

    #[test]
    fn damaged_or_odd_data_gives_an_empty_store_and_a_bad_active_index_is_clamped() {
        assert_eq!(SessionStore::parse(""), SessionStore::default());
        assert_eq!(SessionStore::parse("[1]"), SessionStore::default());
        let store = SessionStore::parse(r#"{ "folders": [ { "folder": "/p", "active": 9, "files": [ { "path": "/p/a" }, { "nope": 1 } ] } ] }"#);
        let session = store.session(Path::new("/p")).unwrap();
        assert_eq!((session.files.len(), session.active), (1, 0));
        assert_eq!(session.files[0], file("/p/a", 0, 0));
    }
}
