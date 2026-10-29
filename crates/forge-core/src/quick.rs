//! What the Quick Open, command palette and Go to Line boxes list for a query.

use std::path::{Path, PathBuf};

use crate::commands::{Command, Keymap};
use crate::fuzzy::{fuzzy_match, rank};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PaletteEntry {
    pub command: Command,
    pub shortcut: Option<String>,
    pub positions: Vec<usize>,
}

/// Commands matching `query` (title or id), best first. Recently used ones lead an empty query.
pub fn palette_entries(query: &str, keymap: &Keymap, recent: &[Command], limit: usize) -> Vec<PaletteEntry> {
    let commands: Vec<Command> = Command::ALL.iter().copied().filter(|c| c.in_palette()).collect();
    let entry = |command: Command, positions: Vec<usize>| PaletteEntry { command, shortcut: keymap.shortcut_label(command), positions };
    if query.trim().is_empty() {
        let mut ordered: Vec<Command> = recent.iter().copied().filter(|c| commands.contains(c)).collect();
        let rest: Vec<Command> = commands.iter().copied().filter(|c| !ordered.contains(c)).collect();
        ordered.extend(rest);
        ordered.truncate(limit);
        return ordered.into_iter().map(|c| entry(c, Vec::new())).collect();
    }
    let titles: Vec<&str> = commands.iter().map(|c| c.title()).collect();
    let mut hits: Vec<(Command, i32, Vec<usize>)> = rank(query, &titles, usize::MAX)
        .into_iter()
        .map(|(i, m)| (commands[i], m.score, m.positions))
        .collect();
    // Matching the id (e.g. "toggleSidebar") is allowed too, ranked below title matches.
    for c in &commands {
        if !hits.iter().any(|(h, _, _)| h == c) {
            if let Some(m) = fuzzy_match(query, c.id()) {
                hits.push((*c, m.score - 20, Vec::new()));
            }
        }
    }
    hits.sort_by_key(|(_, score, _)| std::cmp::Reverse(*score));
    hits.into_iter().take(limit).map(|(c, _, p)| entry(c, p)).collect()
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileEntry {
    pub path: PathBuf,
    /// Path relative to the project root with `/` separators.
    pub label: String,
    pub positions: Vec<usize>,
}

/// Files matching `query`, best first. A match on the file name outranks one on the folders.
pub fn file_entries(files: &[PathBuf], root: &Path, query: &str, recent: &[PathBuf], limit: usize) -> Vec<FileEntry> {
    let label = |p: &Path| p.strip_prefix(root).unwrap_or(p).to_string_lossy().replace('\\', "/");
    let labels: Vec<String> = files.iter().map(|p| label(p)).collect();
    if query.trim().is_empty() {
        let mut order: Vec<usize> = recent.iter().filter_map(|r| files.iter().position(|f| f == r)).collect();
        order.dedup();
        let rest: Vec<usize> = (0..files.len()).filter(|i| !order.contains(i)).collect();
        order.extend(rest);
        return order
            .into_iter()
            .take(limit)
            .map(|i| FileEntry { path: files[i].clone(), label: labels[i].clone(), positions: Vec::new() })
            .collect();
    }
    let mut hits: Vec<(usize, i32, Vec<usize>)> = Vec::new();
    for (i, full) in labels.iter().enumerate() {
        let name_start = full.rfind('/').map_or(0, |s| s + 1);
        let name = &full[name_start..];
        let by_name = fuzzy_match(query, name).map(|m| {
            let offset = full[..name_start].chars().count();
            (m.score + 40, m.positions.into_iter().map(|p| p + offset).collect::<Vec<_>>())
        });
        let by_path = fuzzy_match(query, full).map(|m| (m.score, m.positions));
        let best = match (by_name, by_path) {
            (Some(a), Some(b)) => Some(if a.0 >= b.0 { a } else { b }),
            (a, b) => a.or(b),
        };
        if let Some((score, positions)) = best {
            hits.push((i, score, positions));
        }
    }
    hits.sort_by(|a, b| b.1.cmp(&a.1).then(labels[a.0].len().cmp(&labels[b.0].len())).then(a.0.cmp(&b.0)));
    hits.into_iter()
        .take(limit)
        .map(|(i, _, positions)| FileEntry { path: files[i].clone(), label: labels[i].clone(), positions })
        .collect()
}

/// Parse `"12"`, `"12:5"` or `":5"` into zero-based `(line, column)`; lines clamp to `line_count`.
pub fn parse_go_to_line(input: &str, line_count: usize, current_line: usize) -> Option<(usize, usize)> {
    let input = input.trim();
    let (line_part, col_part) = match input.split_once(':') {
        Some((l, c)) => (l.trim(), Some(c.trim())),
        None => (input, None),
    };
    let line = if line_part.is_empty() && col_part.is_some() {
        current_line
    } else {
        let n: usize = line_part.parse().ok()?;
        n.clamp(1, line_count.max(1)) - 1
    };
    let col = match col_part {
        Some(c) if !c.is_empty() => c.parse::<usize>().ok()?.max(1) - 1,
        _ => 0,
    };
    Some((line, col))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn palette_ranks_titles_then_ids_and_shows_shortcuts() {
        let km = Keymap::default();
        let hits = palette_entries("toggle term", &km, &[], 5);
        assert_eq!(hits[0].command, Command::ToggleTerminal);
        assert_eq!(hits[0].shortcut.as_deref(), Some("Ctrl+`"));
        assert!(!hits[0].positions.is_empty());
        // An id-only query still finds the command.
        let by_id = palette_entries("toggleWordWrap", &km, &[], 5);
        assert!(by_id.iter().any(|e| e.command == Command::ToggleWordWrap));
        // Raw key actions never appear.
        assert!(palette_entries("cursor", &km, &[], 50).iter().all(|e| !e.command.is_movement()));
    }

    #[test]
    fn empty_palette_query_lists_recent_commands_first() {
        let km = Keymap::default();
        let hits = palette_entries("", &km, &[Command::Rename, Command::Find], 3);
        assert_eq!(hits.iter().map(|e| e.command).collect::<Vec<_>>()[..2], [Command::Rename, Command::Find]);
        assert_eq!(hits.len(), 3);
    }

    fn files() -> (PathBuf, Vec<PathBuf>) {
        let root = PathBuf::from("/proj");
        let list = ["src/main.rs", "src/lib.rs", "docs/main_notes.md", "crates/forge-app/src/main.rs", "README.md"];
        (root.clone(), list.iter().map(|f| root.join(f)).collect())
    }

    #[test]
    fn files_rank_name_matches_above_folder_matches() {
        let (root, fs) = files();
        let hits = file_entries(&fs, &root, "main", &[], 10);
        assert_eq!(hits[0].label, "src/main.rs", "shorter path wins among equal name matches");
        assert!(hits.iter().any(|h| h.label == "docs/main_notes.md"));
        // Positions point into the label, at the file name.
        assert_eq!(hits[0].positions, vec![4, 5, 6, 7]);
        let by_folder = file_entries(&fs, &root, "docsmain", &[], 10);
        assert_eq!(by_folder[0].label, "docs/main_notes.md");
        assert!(file_entries(&fs, &root, "zzzz", &[], 10).is_empty());
    }

    #[test]
    fn empty_file_query_lists_recent_files_first() {
        let (root, fs) = files();
        let hits = file_entries(&fs, &root, "", &[root.join("README.md")], 3);
        assert_eq!(hits[0].label, "README.md");
        assert_eq!(hits.len(), 3);
    }

    #[test]
    fn go_to_line_accepts_line_column_forms_and_clamps() {
        assert_eq!(parse_go_to_line("12", 100, 0), Some((11, 0)));
        assert_eq!(parse_go_to_line("12:5", 100, 0), Some((11, 4)));
        assert_eq!(parse_go_to_line(":7", 100, 3), Some((3, 6)));
        assert_eq!(parse_go_to_line("999", 10, 0), Some((9, 0)), "clamps to the last line");
        assert_eq!(parse_go_to_line("0", 10, 0), Some((0, 0)));
        assert_eq!(parse_go_to_line("abc", 10, 0), None);
        assert_eq!(parse_go_to_line("", 10, 0), None);
    }
}
