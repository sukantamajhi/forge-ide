//! File listing for Quick Open and text search across the project.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use forge_text::search::Finder;
use ignore::WalkBuilder;

/// Directories skipped even when no `.gitignore` names them.
const SKIPPED_DIRS: &[&str] = &[".git", "target", "node_modules"];
/// Files larger than this are not searched.
const MAX_SEARCH_BYTES: u64 = 2 * 1024 * 1024;
pub const MAX_HITS: usize = 5_000;

fn walker(root: &Path) -> ignore::Walk {
    WalkBuilder::new(root)
        .hidden(false)
        .require_git(false)
        .filter_entry(|entry| {
            let is_dir = entry.file_type().is_some_and(|t| t.is_dir());
            !(is_dir && entry.file_name().to_str().is_some_and(|n| SKIPPED_DIRS.contains(&n)))
        })
        .build()
}

/// Regular files under `root`, relative-sorted, honoring `.gitignore`; at most `max`.
pub fn list_files(root: &Path, max: usize) -> Vec<PathBuf> {
    let mut files: Vec<PathBuf> = walker(root)
        .filter_map(Result::ok)
        .filter(|e| e.file_type().is_some_and(|t| t.is_file()))
        .map(|e| e.into_path())
        .take(max)
        .collect();
    files.sort();
    files
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Hit {
    pub path: PathBuf,
    pub line: usize,
    /// Char column of the match within the line.
    pub col: usize,
    pub len: usize,
    pub preview: String,
}

/// Read a file as UTF-8 text, refusing binaries and very large files.
pub fn read_text_file(path: &Path) -> Option<String> {
    if std::fs::metadata(path).ok()?.len() > MAX_SEARCH_BYTES {
        return None;
    }
    let bytes = std::fs::read(path).ok()?;
    if bytes.iter().take(8000).any(|b| *b == 0) {
        return None;
    }
    String::from_utf8(bytes).ok()
}

/// What a replace across files did.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct ReplaceReport {
    pub replaced: usize,
    pub files: usize,
    /// Files left alone because the caller said so (open with unsaved edits).
    pub skipped: Vec<PathBuf>,
    pub failed: Vec<(PathBuf, String)>,
}

/// `text` with ascending, non-overlapping char-range `edits` applied.
pub fn apply_replacements(text: &str, edits: &[(usize, usize, String)]) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    let mut at = 0;
    for (start, end, replacement) in edits {
        out.extend(&chars[at..*start]);
        out.push_str(replacement);
        at = *end;
    }
    out.extend(&chars[at..]);
    out
}

/// Replace every match of `finder` with `replacement` in `files`. Files for which `skip` is true are
/// reported, not touched. In regex mode `$1` and `${name}` expand to capture groups.
pub fn replace_in_files(finder: &Finder, replacement: &str, files: &[PathBuf], skip: &dyn Fn(&Path) -> bool) -> ReplaceReport {
    let mut report = ReplaceReport::default();
    for path in files {
        if skip(path) {
            report.skipped.push(path.clone());
            continue;
        }
        let Some(text) = read_text_file(path) else { continue };
        let edits = finder.replacements(&text, replacement);
        if edits.is_empty() {
            continue;
        }
        match std::fs::write(path, apply_replacements(&text, &edits)) {
            Ok(()) => {
                report.replaced += edits.len();
                report.files += 1;
            }
            Err(e) => report.failed.push((path.clone(), e.to_string())),
        }
    }
    report
}

/// Matches of `finder` in one text, as hits for `path`.
pub fn hits_in_text(path: &Path, text: &str, finder: &Finder) -> Vec<Hit> {
    let matches = finder.find_all(text);
    if matches.is_empty() {
        return Vec::new();
    }
    let mut line_starts = vec![0usize];
    for (i, c) in text.chars().enumerate() {
        if c == '\n' {
            line_starts.push(i + 1);
        }
    }
    let lines: Vec<&str> = text.split('\n').collect();
    matches
        .into_iter()
        .map(|m| {
            let line = line_starts.partition_point(|s| *s <= m.start) - 1;
            let preview: String = lines[line].trim_end_matches('\r').chars().take(300).collect();
            Hit { path: path.to_path_buf(), line, col: m.start - line_starts[line], len: m.end - m.start, preview }
        })
        .collect()
}

/// Search every file under `root`, calling `on_batch` with each file's hits. Stops at
/// [`MAX_HITS`] or when `cancel` is set. Returns whether the hit limit was reached.
pub fn search(root: &Path, finder: &Finder, cancel: &AtomicBool, mut on_batch: impl FnMut(Vec<Hit>)) -> bool {
    let mut total = 0;
    for entry in walker(root).filter_map(Result::ok) {
        if cancel.load(Ordering::Relaxed) {
            return false;
        }
        if !entry.file_type().is_some_and(|t| t.is_file()) {
            continue;
        }
        let Some(text) = read_text_file(entry.path()) else { continue };
        let mut hits = hits_in_text(entry.path(), &text, finder);
        if hits.is_empty() {
            continue;
        }
        hits.truncate(MAX_HITS - total);
        total += hits.len();
        on_batch(hits);
        if total >= MAX_HITS {
            return true;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use forge_text::search::SearchOptions;

    fn project(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("forge-ps-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("src")).unwrap();
        std::fs::create_dir_all(dir.join("target/debug")).unwrap();
        std::fs::create_dir_all(dir.join("generated")).unwrap();
        std::fs::write(dir.join(".gitignore"), "generated/\n").unwrap();
        std::fs::write(dir.join("src/a.rs"), "fn a() {}\nlet needle = 1;\n  needle again\n").unwrap();
        std::fs::write(dir.join("src/b.txt"), "no match here\r\nNEEDLE\r\n").unwrap();
        std::fs::write(dir.join("target/debug/x.rs"), "needle in target").unwrap();
        std::fs::write(dir.join("generated/g.rs"), "needle in ignored dir").unwrap();
        std::fs::write(dir.join("blob.bin"), b"needle\0binary").unwrap();
        std::fs::write(dir.join(".hidden"), "needle in a dotfile").unwrap();
        dir
    }

    fn finder(q: &str) -> Finder {
        Finder::new(q, SearchOptions::default()).unwrap()
    }

    #[test]
    fn lists_files_honoring_gitignore_and_skipping_build_dirs() {
        let root = project("list");
        let names: Vec<String> = list_files(&root, 100)
            .iter()
            .map(|p| p.strip_prefix(&root).unwrap().to_string_lossy().replace('\\', "/"))
            .collect();
        assert!(names.contains(&"src/a.rs".to_owned()) && names.contains(&".hidden".to_owned()), "{names:?}");
        assert!(!names.iter().any(|n| n.starts_with("target/") || n.starts_with("generated/") || n.starts_with(".git/")), "{names:?}");
        assert_eq!(list_files(&root, 2).len(), 2);
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn finds_hits_with_line_column_and_preview_skipping_binaries_and_ignored() {
        let root = project("search");
        let mut hits = Vec::new();
        let capped = search(&root, &finder("needle"), &AtomicBool::new(false), |batch| hits.extend(batch));
        assert!(!capped);
        hits.sort_by(|a, b| (&a.path, a.line).cmp(&(&b.path, b.line)));
        let summary: Vec<(String, usize, usize)> = hits
            .iter()
            .map(|h| (h.path.file_name().unwrap().to_string_lossy().into_owned(), h.line, h.col))
            .collect();
        assert_eq!(
            summary,
            vec![
                (".hidden".to_owned(), 0, 0),
                ("a.rs".to_owned(), 1, 4),
                ("a.rs".to_owned(), 2, 2),
                ("b.txt".to_owned(), 1, 0),
            ]
        );
        assert_eq!(hits[1].preview, "let needle = 1;");
        assert_eq!(hits[3].preview, "NEEDLE", "CRLF is trimmed from the preview");
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn cancellation_stops_the_walk() {
        let root = project("cancel");
        let mut batches = 0;
        search(&root, &finder("needle"), &AtomicBool::new(true), |_| batches += 1);
        assert_eq!(batches, 0);
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn multiple_matches_on_one_line_and_unicode_columns() {
        let hits = hits_in_text(Path::new("x"), "é aa aa\nbb", &finder("aa"));
        assert_eq!(hits.iter().map(|h| (h.line, h.col)).collect::<Vec<_>>(), vec![(0, 2), (0, 5)]);
        assert_eq!(hits[0].len, 2);
    }

    #[test]
    fn replace_in_files_rewrites_matches_skips_and_expands_captures() {
        use forge_text::search::SearchOptions;
        let dir = std::env::temp_dir().join(format!("forge-replace-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let (a, b, c) = (dir.join("a.txt"), dir.join("b.txt"), dir.join("c.txt"));
        std::fs::write(&a, "foo bar\r\nfoo\n").unwrap();
        std::fs::write(&b, "foo unsaved\n").unwrap();
        std::fs::write(&c, "nothing here\n").unwrap();

        let finder = Finder::new("foo", SearchOptions::default()).unwrap();
        let skip_b = |p: &Path| p.ends_with("b.txt");
        let report = replace_in_files(&finder, "baz", &[a.clone(), b.clone(), c.clone()], &skip_b);
        assert_eq!((report.replaced, report.files), (2, 1));
        assert_eq!(report.skipped, vec![b.clone()]);
        assert_eq!(std::fs::read_to_string(&a).unwrap(), "baz bar\r\nbaz\n", "line endings are preserved");
        assert_eq!(std::fs::read_to_string(&b).unwrap(), "foo unsaved\n");

        let regex = Finder::new(r"(\w+) bar", SearchOptions { regex: true, ..Default::default() }).unwrap();
        let report = replace_in_files(&regex, "bar $1", &[a.clone()], &|_| false);
        assert_eq!(report.replaced, 1);
        assert_eq!(std::fs::read_to_string(&a).unwrap(), "bar baz\r\nbaz\n");
        assert_eq!(apply_replacements("héllo wörld", &[(1, 2, "e".into()), (7, 8, "o".into())]), "hello world");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
