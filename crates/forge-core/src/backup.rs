//! Hot exit: what is in files that have unsaved changes (and in untitled files), kept beside `session.json` so
//! that closing Forge, or a crash, loses nothing. One file per folder, rewritten as the edits change, removed
//! when nothing is unsaved.

use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use serde_json::{json, Value};

use crate::settings::config_dir;

/// A document bigger than this is not backed up (the backup is rewritten as you type).
pub const MAX_BYTES: usize = 8_000_000;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Backup {
    /// The file the text belongs to; `None` for an untitled document.
    pub path: Option<PathBuf>,
    /// The tab's name for an untitled document (`Untitled-2`); the file name otherwise.
    pub name: String,
    pub text: String,
    /// Caret as a char offset.
    pub caret: usize,
    /// The file's modification time (milliseconds since the epoch) when the text was saved away: if the file
    /// has another time later, it changed on disk in between.
    pub mtime: Option<u64>,
    /// The language a user picked for the document (a VS Code language id), if they did.
    pub language: Option<String>,
}

/// A folder as a file name: a hash of its path with slashes unified and, on Windows, case folded.
fn file_name_for(root: &Path) -> String {
    let text = root.to_string_lossy().replace('\\', "/");
    let text = text.trim_end_matches('/');
    let text = if cfg!(windows) { text.to_lowercase() } else { text.to_owned() };
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in text.bytes() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0100_0000_01b3);
    }
    format!("{hash:016x}.json")
}

pub fn dir() -> PathBuf {
    config_dir().join("backups")
}

pub fn mtime_ms(path: &Path) -> Option<u64> {
    let modified = std::fs::metadata(path).ok()?.modified().ok()?;
    modified.duration_since(UNIX_EPOCH).ok().map(|d| d.as_millis() as u64)
}

pub fn load(root: &Path) -> Vec<Backup> {
    load_from(&dir(), root)
}

pub fn store(root: &Path, backups: &[Backup]) -> std::io::Result<()> {
    store_in(&dir(), root, backups)
}

pub fn load_from(dir: &Path, root: &Path) -> Vec<Backup> {
    let Ok(text) = std::fs::read_to_string(dir.join(file_name_for(root))) else { return Vec::new() };
    parse(&text)
}

pub fn store_in(dir: &Path, root: &Path, backups: &[Backup]) -> std::io::Result<()> {
    let file = dir.join(file_name_for(root));
    if backups.is_empty() {
        return match std::fs::remove_file(&file) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e),
            _ => Ok(()),
        };
    }
    std::fs::create_dir_all(dir)?;
    // Write beside the target and rename, so a crash mid-write leaves the previous backup intact.
    let temp = file.with_extension("tmp");
    std::fs::write(&temp, serialize(backups))?;
    std::fs::rename(&temp, &file)
}

fn serialize(backups: &[Backup]) -> String {
    let items: Vec<Value> = backups
        .iter()
        .map(|b| {
            json!({
                "path": b.path.as_ref().map(|p| p.to_string_lossy().into_owned()),
                "name": b.name,
                "text": b.text,
                "caret": b.caret,
                "mtime": b.mtime,
                "language": b.language,
            })
        })
        .collect();
    json!({ "backups": items }).to_string()
}

fn parse(text: &str) -> Vec<Backup> {
    let Ok(Value::Object(map)) = serde_json::from_str::<Value>(text) else { return Vec::new() };
    map.get("backups")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|item| {
            Some(Backup {
                path: item.get("path").and_then(Value::as_str).map(PathBuf::from),
                name: item.get("name").and_then(Value::as_str)?.to_owned(),
                text: item.get("text").and_then(Value::as_str)?.to_owned(),
                caret: item.get("caret").and_then(Value::as_u64).unwrap_or(0) as usize,
                mtime: item.get("mtime").and_then(Value::as_u64),
                language: item.get("language").and_then(Value::as_str).map(str::to_owned),
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("forge-backup-test-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn backups_round_trip_and_the_file_goes_when_nothing_is_unsaved() {
        let dir = temp("round");
        let root = Path::new("/work/app");
        let items = vec![
            Backup { path: Some(PathBuf::from("/work/app/src/a.rs")), name: "a.rs".into(), text: "fn a() {\n  \"quoted\" \u{e9}\n}".into(), caret: 5, mtime: Some(1234), language: None },
            Backup { path: None, name: "Untitled-2".into(), text: "notes".into(), caret: 0, mtime: None, language: Some("markdown".into()) },
        ];
        store_in(&dir, root, &items).unwrap();
        assert_eq!(load_from(&dir, root), items);
        // Another folder has its own file.
        assert!(load_from(&dir, Path::new("/work/other")).is_empty());
        store_in(&dir, root, &[]).unwrap();
        assert!(load_from(&dir, root).is_empty());
        assert!(!dir.join(file_name_for(root)).exists());
        store_in(&dir, root, &[]).unwrap();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_damaged_file_means_no_backups() {
        let dir = temp("damaged");
        std::fs::create_dir_all(&dir).unwrap();
        let root = Path::new("/x");
        std::fs::write(dir.join(file_name_for(root)), "{ not json").unwrap();
        assert!(load_from(&dir, root).is_empty());
        std::fs::write(dir.join(file_name_for(root)), r#"{"backups":[{"name":"n"},{"name":"ok","text":"t"}]}"#).unwrap();
        let got = load_from(&dir, root);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].name, "ok");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_modification_time_of_a_file_is_read() {
        let dir = temp("mtime");
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("f.txt");
        std::fs::write(&file, "x").unwrap();
        assert!(mtime_ms(&file).is_some_and(|t| t > 1_000_000_000_000));
        assert_eq!(mtime_ms(&dir.join("missing")), None);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
