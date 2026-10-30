//! Explorer file operations. Deleting moves to the Recycle Bin / Trash and never removes
//! permanently, and nothing here overwrites an existing path.

use std::path::{Path, PathBuf};

/// Why a name cannot be used for a new file or folder.
pub fn validate_name(name: &str) -> Result<(), String> {
    let name = name.trim();
    if name.is_empty() {
        return Err("A name must be provided.".to_owned());
    }
    if name == "." || name == ".." {
        return Err(format!("'{name}' is not a valid name."));
    }
    if let Some(bad) = name.chars().find(|c| matches!(c, '<' | '>' | ':' | '"' | '|' | '?' | '*' | '/' | '\\') || c.is_control()) {
        return Err(format!("The name contains the character '{bad}', which is not allowed."));
    }
    if name.ends_with('.') || name.ends_with(' ') {
        return Err("A name cannot end with a dot or a space.".to_owned());
    }
    Ok(())
}

/// Split `a/b/c.rs` into its folders and final name, validating each part.
fn split_relative(name: &str) -> Result<Vec<String>, String> {
    let parts: Vec<String> = name.trim().split(['/', '\\']).map(|p| p.trim().to_owned()).collect();
    if parts.iter().any(|p| p.is_empty()) {
        return Err("A name must be provided.".to_owned());
    }
    for part in &parts {
        validate_name(part)?;
    }
    Ok(parts)
}

/// Create an empty file at `name` (which may include subfolders) under `dir`.
pub fn create_file(dir: &Path, name: &str) -> Result<PathBuf, String> {
    let parts = split_relative(name)?;
    let path = parts.iter().fold(dir.to_path_buf(), |p, part| p.join(part));
    if path.exists() {
        return Err(format!("A file or folder '{}' already exists at this location.", path.display()));
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    std::fs::OpenOptions::new().write(true).create_new(true).open(&path).map_err(|e| e.to_string())?;
    Ok(path)
}

/// A name for `name` inside `dir` that is not taken: `name`, then `name copy`, `name copy 2`...
fn free_name(dir: &Path, name: &std::ffi::OsStr) -> PathBuf {
    let first = dir.join(name);
    if !first.exists() {
        return first;
    }
    let as_path = Path::new(name);
    let stem = as_path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let ext = as_path.extension().map(|e| format!(".{}", e.to_string_lossy())).unwrap_or_default();
    (1..)
        .map(|n| dir.join(if n == 1 { format!("{stem} copy{ext}") } else { format!("{stem} copy {n}{ext}") }))
        .find(|p| !p.exists())
        .expect("an unused name exists")
}

fn copy_tree(from: &Path, to: &Path) -> std::io::Result<()> {
    if from.is_dir() {
        std::fs::create_dir_all(to)?;
        for entry in std::fs::read_dir(from)? {
            let entry = entry?;
            copy_tree(&entry.path(), &to.join(entry.file_name()))?;
        }
        Ok(())
    } else {
        std::fs::copy(from, to).map(|_| ())
    }
}

/// Copy a file or folder into `dir` (under a free name when one is taken). Returns the new path.
pub fn copy_into(from: &Path, dir: &Path) -> Result<PathBuf, String> {
    let name = from.file_name().ok_or("Nothing to copy")?;
    if from.is_dir() && dir.starts_with(from) {
        return Err("Cannot copy a folder into itself.".to_owned());
    }
    let to = free_name(dir, name);
    copy_tree(from, &to).map_err(|e| e.to_string())?;
    Ok(to)
}

/// Move a file or folder into `dir`. Refuses to replace anything or to move a folder into itself.
pub fn move_into(from: &Path, dir: &Path) -> Result<PathBuf, String> {
    let name = from.file_name().ok_or("Nothing to move")?;
    if from.is_dir() && dir.starts_with(from) {
        return Err("Cannot move a folder into itself.".to_owned());
    }
    let to = dir.join(name);
    if to == from {
        return Ok(to);
    }
    if to.exists() {
        return Err(format!("A file or folder '{}' already exists at this location.", to.display()));
    }
    if std::fs::rename(from, &to).is_ok() {
        return Ok(to);
    }
    // Across drives a rename fails: copy, then remove the original.
    copy_tree(from, &to).map_err(|e| e.to_string())?;
    if from.is_dir() { std::fs::remove_dir_all(from) } else { std::fs::remove_file(from) }.map_err(|e| e.to_string())?;
    Ok(to)
}

pub fn create_dir(dir: &Path, name: &str) -> Result<PathBuf, String> {
    let parts = split_relative(name)?;
    let path = parts.iter().fold(dir.to_path_buf(), |p, part| p.join(part));
    if path.exists() {
        return Err(format!("A file or folder '{}' already exists at this location.", path.display()));
    }
    std::fs::create_dir_all(&path).map_err(|e| e.to_string())?;
    Ok(path)
}

/// Rename `path` to `new_name` within the same folder. Refuses to replace anything.
pub fn rename(path: &Path, new_name: &str) -> Result<PathBuf, String> {
    let new_name = new_name.trim();
    validate_name(new_name)?;
    let target = path.with_file_name(new_name);
    if target == path {
        return Ok(target);
    }
    // On a case-insensitive filesystem a case-only rename names the same entry; allow it.
    let case_only = target.to_string_lossy().eq_ignore_ascii_case(&path.to_string_lossy());
    if target.exists() && !case_only {
        return Err(format!("A file or folder '{new_name}' already exists at this location."));
    }
    std::fs::rename(path, &target).map_err(|e| e.to_string())?;
    Ok(target)
}

/// Move to the Recycle Bin / Trash. Fails rather than deleting permanently.
pub fn move_to_trash(path: &Path) -> Result<(), String> {
    trash::delete(path).map_err(|e| format!("Could not move to the Recycle Bin: {e}"))
}

#[cfg(test)]
mod copy_tests {
    use super::*;

    #[test]
    fn copy_and_move_into_folders() {
        let root = std::env::temp_dir().join(format!("forge-fileops-copy-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("src/inner")).unwrap();
        std::fs::create_dir_all(root.join("dest")).unwrap();
        std::fs::write(root.join("src/inner/a.txt"), "hi").unwrap();
        std::fs::write(root.join("note.txt"), "n").unwrap();

        let copied = copy_into(&root.join("src"), &root.join("dest")).unwrap();
        assert_eq!(std::fs::read_to_string(copied.join("inner/a.txt")).unwrap(), "hi");
        // Copying again picks a free name; the first copy is untouched.
        let again = copy_into(&root.join("src"), &root.join("dest")).unwrap();
        assert_eq!(again.file_name().unwrap(), "src copy");
        let file_copy = copy_into(&root.join("note.txt"), &root).unwrap();
        assert_eq!(file_copy.file_name().unwrap(), "note copy.txt");

        assert!(copy_into(&root.join("src"), &root.join("src/inner")).is_err());
        assert!(move_into(&root.join("src"), &root.join("src/inner")).is_err());
        let moved = move_into(&root.join("note.txt"), &root.join("dest")).unwrap();
        assert!(moved.is_file() && !root.join("note.txt").exists());
        std::fs::write(root.join("note.txt"), "again").unwrap();
        assert!(move_into(&root.join("note.txt"), &root.join("dest")).is_err(), "never replaces");
        std::fs::remove_dir_all(&root).unwrap();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("forge-fileops-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn names_are_validated() {
        assert!(validate_name("main.rs").is_ok());
        assert!(validate_name(".gitignore").is_ok());
        for bad in ["", "  ", "..", "a?b", "a:b", "a*", "name.", "tab\\there"] {
            assert!(validate_name(bad).is_err(), "{bad:?} should be rejected");
        }
    }

    #[test]
    fn create_file_makes_parents_and_never_overwrites() {
        let dir = scratch("create");
        let made = create_file(&dir, "a/b/c.txt").unwrap();
        assert!(made.is_file() && made.ends_with("c.txt"));
        std::fs::write(&made, "keep").unwrap();
        assert!(create_file(&dir, "a/b/c.txt").unwrap_err().contains("already exists"));
        assert_eq!(std::fs::read_to_string(&made).unwrap(), "keep");
        assert!(create_file(&dir, "a//b").is_err());
        assert!(create_file(&dir, "bad|name").is_err());
        assert!(create_dir(&dir, "x/y").unwrap().is_dir());
        assert!(create_dir(&dir, "x/y").is_err());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn rename_moves_within_the_folder_and_refuses_to_replace() {
        let dir = scratch("rename");
        std::fs::write(dir.join("one.txt"), "1").unwrap();
        std::fs::write(dir.join("two.txt"), "2").unwrap();
        assert!(rename(&dir.join("one.txt"), "two.txt").unwrap_err().contains("already exists"));
        assert_eq!(std::fs::read_to_string(dir.join("two.txt")).unwrap(), "2");
        let moved = rename(&dir.join("one.txt"), "three.txt").unwrap();
        assert_eq!(std::fs::read_to_string(moved).unwrap(), "1");
        assert!(rename(&dir.join("three.txt"), "a/b").is_err(), "a rename cannot move between folders");
        assert!(dir.join("three.txt").exists());
        assert_eq!(rename(&dir.join("three.txt"), "three.txt").unwrap(), dir.join("three.txt"));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
