use std::collections::HashSet;
use std::path::{Path, PathBuf};

/// Directories never shown: huge, generated, or VCS internals.
const HIDDEN: &[&str] = &[".git", "target", "node_modules"];

/// Folder to open for a file: the nearest ancestor holding `.git`, else the nearest one
/// holding `Cargo.toml`, else the file's own directory. Language servers need the
/// project root, not just the directory the file happens to sit in.
pub fn project_root(file: &Path) -> PathBuf {
    let start = file.parent().unwrap_or(file);
    let ancestor_with = |marker: &str| start.ancestors().find(|dir| dir.join(marker).exists()).map(Path::to_path_buf);
    // The nearest repository, else the nearest project file of any ecosystem. Language servers
    // resolve imports from this folder, so a Python file opened on its own must not be rooted in
    // its own package directory.
    ancestor_with(".git")
        .or_else(|| ["Cargo.toml", "pyproject.toml", "setup.py", "package.json", "go.mod", "pom.xml"].iter().find_map(|m| ancestor_with(m)))
        .or_else(|| python_package_parent(start))
        .unwrap_or_else(|| start.to_path_buf())
}

/// For a folder inside a Python package (`__init__.py`), the folder above the outermost package.
fn python_package_parent(start: &Path) -> Option<PathBuf> {
    let outermost = start.ancestors().take_while(|dir| dir.join("__init__.py").exists()).last()?;
    outermost.parent().map(Path::to_path_buf)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TreeEntry {
    pub depth: usize,
    pub path: PathBuf,
    pub name: String,
    pub is_dir: bool,
    pub expanded: bool,
}

/// Flattened, lazily expanded view of a directory tree.
pub struct FileTree {
    root: PathBuf,
    /// Folders added to the workspace next to `root`. With any, every folder is listed as a top-level row of its own.
    extra: Vec<PathBuf>,
    expanded: HashSet<PathBuf>,
    pub entries: Vec<TreeEntry>,
}

impl FileTree {
    pub fn new(root: PathBuf) -> Self {
        let mut tree = Self { root, extra: Vec::new(), expanded: HashSet::new(), entries: Vec::new() };
        tree.refresh();
        tree
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Every folder of the workspace: the first one, then the added ones.
    pub fn roots(&self) -> Vec<PathBuf> {
        std::iter::once(self.root.clone()).chain(self.extra.iter().cloned()).collect()
    }

    /// Whether `path` is one of the workspace folders shown as a row of its own (never with a single folder).
    pub fn is_root_row(&self, path: &Path) -> bool {
        !self.extra.is_empty() && (path == self.root || self.extra.iter().any(|r| r == path))
    }

    /// Add a folder to the workspace. False if it is already in it (or inside one: it is listed there already).
    pub fn add_root(&mut self, path: PathBuf) -> bool {
        if self.roots().iter().any(|r| path.starts_with(r)) {
            return false;
        }
        if self.extra.is_empty() {
            // The first folder becomes a row too, so it starts open like the new one.
            self.expanded.insert(self.root.clone());
        }
        self.expanded.insert(path.clone());
        self.extra.push(path);
        self.refresh();
        true
    }

    pub fn toggle(&mut self, path: &Path) {
        if !self.expanded.remove(path) {
            self.expanded.insert(path.to_path_buf());
        }
        self.refresh();
    }

    /// Fold every open folder back up.
    pub fn collapse_all(&mut self) {
        self.expanded.clear();
        self.refresh();
    }

    /// Expand every folder above `path` so it is listed.
    pub fn reveal(&mut self, path: &Path) {
        let roots = self.roots();
        for ancestor in path.ancestors().skip(1) {
            if roots.iter().any(|r| ancestor.starts_with(r) && (ancestor != r.as_path() || !self.extra.is_empty())) {
                self.expanded.insert(ancestor.to_path_buf());
            }
        }
        self.refresh();
    }

    pub fn index_of(&self, path: &Path) -> Option<usize> {
        self.entries.iter().position(|e| e.path == path)
    }

    pub fn refresh(&mut self) {
        let mut entries = Vec::new();
        if self.extra.is_empty() {
            self.walk(&self.root, 0, &mut entries);
        } else {
            for root in self.roots() {
                let expanded = self.expanded.contains(&root);
                let name = root.file_name().map_or_else(|| root.display().to_string(), |n| n.to_string_lossy().into_owned());
                entries.push(TreeEntry { depth: 0, path: root.clone(), name, is_dir: true, expanded });
                if expanded {
                    self.walk(&root, 1, &mut entries);
                }
            }
        }
        self.entries = entries;
    }

    fn walk(&self, dir: &Path, depth: usize, out: &mut Vec<TreeEntry>) {
        // Unreadable directories show as empty rather than failing the whole tree.
        let Ok(read) = std::fs::read_dir(dir) else { return };
        let mut children: Vec<(String, PathBuf, bool)> = read
            .filter_map(Result::ok)
            .filter_map(|e| {
                let name = e.file_name().to_string_lossy().into_owned();
                let is_dir = e.file_type().ok()?.is_dir();
                (!(is_dir && HIDDEN.contains(&name.as_str()))).then(|| (name, e.path(), is_dir))
            })
            .collect();
        children.sort_by(|a, b| b.2.cmp(&a.2).then_with(|| a.0.to_lowercase().cmp(&b.0.to_lowercase())));

        for (name, path, is_dir) in children {
            let expanded = is_dir && self.expanded.contains(&path);
            out.push(TreeEntry { depth, path: path.clone(), name, is_dir, expanded });
            if expanded {
                self.walk(&path, depth + 1, out);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("forge-tree-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn added_folders_are_rows_of_their_own() {
        let (a, b) = (scratch("m1"), scratch("m2"));
        std::fs::create_dir_all(a.join("src")).unwrap();
        std::fs::write(b.join("x.txt"), "").unwrap();
        let mut tree = FileTree::new(a.clone());
        assert!(tree.entries.iter().all(|e| e.depth == 0), "one folder: its children are the top level");
        assert!(!tree.is_root_row(&a));
        assert!(tree.add_root(b.clone()));
        assert!(!tree.add_root(b.join("inner")), "a folder inside one that is open is not added again");
        assert!(tree.is_root_row(&a) && tree.is_root_row(&b));
        let top: Vec<&Path> = tree.entries.iter().filter(|e| e.depth == 0).map(|e| e.path.as_path()).collect();
        assert_eq!(top, [a.as_path(), b.as_path()]);
        assert!(tree.entries.iter().any(|e| e.depth == 1 && e.name == "x.txt"));
        tree.toggle(&b);
        assert!(tree.entries.iter().all(|e| e.name != "x.txt"), "a folded folder hides its files");
        tree.reveal(&a.join("src").join("lib.rs"));
        assert!(tree.entries.iter().any(|e| e.path == a.join("src") && e.expanded));
        let _ = std::fs::remove_dir_all(&a);
        let _ = std::fs::remove_dir_all(&b);
    }

    #[test]
    fn dirs_first_hidden_skipped_and_lazy_expand() {
        let root = scratch("a");
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::create_dir_all(root.join("target")).unwrap();
        std::fs::create_dir_all(root.join(".git")).unwrap();
        std::fs::write(root.join("Zed.txt"), "").unwrap();
        std::fs::write(root.join("apple.txt"), "").unwrap();
        std::fs::write(root.join("src").join("main.rs"), "").unwrap();

        let mut tree = FileTree::new(root.clone());
        let names: Vec<&str> = tree.entries.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, ["src", "apple.txt", "Zed.txt"]);

        tree.toggle(&root.join("src"));
        assert_eq!(tree.entries[1].name, "main.rs");
        assert_eq!(tree.entries[1].depth, 1);
        assert!(tree.entries[0].expanded);

        tree.toggle(&root.join("src"));
        assert_eq!(tree.entries.len(), 3);
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn collapse_all_folds_every_open_folder() {
        let root = scratch("collapse");
        std::fs::create_dir_all(root.join("a/b")).unwrap();
        std::fs::write(root.join("a/b/f.txt"), "").unwrap();
        let mut tree = FileTree::new(root.clone());
        tree.reveal(&root.join("a/b/f.txt"));
        assert!(tree.entries.len() > 1);
        tree.collapse_all();
        assert_eq!(tree.entries.len(), 1);
        assert!(!tree.entries[0].expanded);
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn reveal_expands_ancestors_so_the_file_is_listed() {
        let root = scratch("reveal");
        std::fs::create_dir_all(root.join("a/b")).unwrap();
        std::fs::write(root.join("a/b/f.txt"), "").unwrap();
        let mut tree = FileTree::new(root.clone());
        assert!(tree.index_of(&root.join("a/b/f.txt")).is_none());
        tree.reveal(&root.join("a/b/f.txt"));
        assert_eq!(tree.index_of(&root.join("a/b/f.txt")), Some(2));
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn project_root_prefers_git_then_cargo_then_parent() {
        let root = scratch("root");
        std::fs::create_dir_all(root.join("crates/a/src")).unwrap();
        std::fs::write(root.join("crates/a/Cargo.toml"), "").unwrap();
        let file = root.join("crates/a/src/lib.rs");
        assert_eq!(project_root(&file), root.join("crates/a"));

        std::fs::create_dir_all(root.join(".git")).unwrap();
        assert_eq!(project_root(&file), root);

        let loose = scratch("loose").join("x.txt");
        assert_eq!(project_root(&loose), loose.parent().unwrap());
        std::fs::remove_dir_all(&root).unwrap();
        std::fs::remove_dir_all(loose.parent().unwrap()).unwrap();
    }

    #[test]
    fn project_root_knows_other_ecosystems_and_python_packages() {
        let root = scratch("eco");
        std::fs::create_dir_all(root.join("web/src")).unwrap();
        std::fs::write(root.join("web/package.json"), "{}").unwrap();
        assert_eq!(project_root(&root.join("web/src/app.ts")), root.join("web"));

        std::fs::create_dir_all(root.join("svc/pkg/sub")).unwrap();
        std::fs::write(root.join("svc/pkg/__init__.py"), "").unwrap();
        std::fs::write(root.join("svc/pkg/sub/__init__.py"), "").unwrap();
        assert_eq!(project_root(&root.join("svc/pkg/sub/mod.py")), root.join("svc"), "the folder above the outermost package");

        std::fs::write(root.join("svc/pyproject.toml"), "").unwrap();
        assert_eq!(project_root(&root.join("svc/pkg/sub/mod.py")), root.join("svc"));
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn missing_root_yields_empty_tree() {
        let tree = FileTree::new(PathBuf::from("Z:/definitely/not/here"));
        assert!(tree.entries.is_empty());
    }
}
