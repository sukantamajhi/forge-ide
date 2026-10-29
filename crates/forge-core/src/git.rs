//! Source control through the `git` command line, so behavior (config, hooks, credential
//! helpers) matches the user's own git. Nothing here prompts: `GIT_TERMINAL_PROMPT=0`
//! makes a command that needs credentials fail with a message instead of hanging.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Modified,
    Added,
    Deleted,
    Renamed,
    Copied,
    TypeChange,
    Unmerged,
}

impl Kind {
    pub fn letter(self) -> char {
        match self {
            Kind::Modified => 'M',
            Kind::Added => 'A',
            Kind::Deleted => 'D',
            Kind::Renamed => 'R',
            Kind::Copied => 'C',
            Kind::TypeChange => 'T',
            Kind::Unmerged => '!',
        }
    }

    fn from_status_char(c: char) -> Option<Kind> {
        Some(match c {
            'M' => Kind::Modified,
            'A' => Kind::Added,
            'D' => Kind::Deleted,
            'R' => Kind::Renamed,
            'C' => Kind::Copied,
            'T' => Kind::TypeChange,
            'U' => Kind::Unmerged,
            _ => return None,
        })
    }
}

/// One changed path. `index` is the staged state, `worktree` the unstaged one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Change {
    /// Repository-relative, `/` separated.
    pub path: String,
    pub orig_path: Option<String>,
    pub index: Option<Kind>,
    pub worktree: Option<Kind>,
    pub untracked: bool,
    pub conflicted: bool,
}

impl Change {
    pub fn is_staged(&self) -> bool {
        self.index.is_some() && !self.conflicted
    }

    /// The letter shown in lists: unstaged state first, then staged; `U` for untracked.
    pub fn letter(&self) -> char {
        if self.conflicted {
            '!'
        } else if self.untracked {
            'U'
        } else {
            self.worktree.or(self.index).map_or('?', Kind::letter)
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Status {
    /// `None` when HEAD is detached.
    pub branch: Option<String>,
    pub head: String,
    pub upstream: Option<String>,
    pub ahead: usize,
    pub behind: usize,
    /// The repository has at least one remote, so a branch can be published.
    pub has_remote: bool,
    pub changes: Vec<Change>,
}

impl Status {
    pub fn staged(&self) -> impl Iterator<Item = &Change> {
        self.changes.iter().filter(|c| c.is_staged())
    }

    /// Unstaged, untracked and conflicted entries (VS Code's "Changes" group).
    pub fn unstaged(&self) -> impl Iterator<Item = &Change> {
        self.changes.iter().filter(|c| c.worktree.is_some() || c.untracked || c.conflicted)
    }
}

/// Parse `git status --porcelain=v2 -z --branch`.
pub fn parse_status(output: &str) -> Status {
    let mut status = Status::default();
    let mut records = output.split('\0').filter(|r| !r.is_empty());
    while let Some(record) = records.next() {
        if let Some(rest) = record.strip_prefix("# ") {
            let (key, value) = rest.split_once(' ').unwrap_or((rest, ""));
            match key {
                "branch.oid" => status.head = value.chars().take(8).collect(),
                "branch.head" => status.branch = (value != "(detached)").then(|| value.to_owned()),
                "branch.upstream" => status.upstream = Some(value.to_owned()),
                "branch.ab" => {
                    let mut parts = value.split(' ');
                    status.ahead = parts.next().and_then(|a| a.trim_start_matches('+').parse().ok()).unwrap_or(0);
                    status.behind = parts.next().and_then(|b| b.trim_start_matches('-').parse().ok()).unwrap_or(0);
                }
                _ => {}
            }
            continue;
        }
        let mut chars = record.chars();
        let Some(tag) = chars.next() else { continue };
        let xy = |s: &str| -> (Option<Kind>, Option<Kind>) {
            let mut c = s.chars();
            (c.next().and_then(Kind::from_status_char), c.next().and_then(Kind::from_status_char))
        };
        match tag {
            '1' => {
                let f: Vec<&str> = record.splitn(9, ' ').collect();
                if f.len() == 9 {
                    let (index, worktree) = xy(f[1]);
                    status.changes.push(Change { path: f[8].to_owned(), orig_path: None, index, worktree, untracked: false, conflicted: false });
                }
            }
            '2' => {
                let f: Vec<&str> = record.splitn(10, ' ').collect();
                // With -z the original path is the next NUL-separated record.
                let orig = records.next().map(str::to_owned);
                if f.len() == 10 {
                    let (index, worktree) = xy(f[1]);
                    status.changes.push(Change { path: f[9].to_owned(), orig_path: orig, index, worktree, untracked: false, conflicted: false });
                }
            }
            'u' => {
                let f: Vec<&str> = record.splitn(11, ' ').collect();
                if f.len() == 11 {
                    status.changes.push(Change { path: f[10].to_owned(), orig_path: None, index: None, worktree: Some(Kind::Unmerged), untracked: false, conflicted: true });
                }
            }
            '?' => {
                let path = record[1..].trim_start().to_owned();
                status.changes.push(Change { path, orig_path: None, index: None, worktree: None, untracked: true, conflicted: false });
            }
            _ => {}
        }
    }
    status.changes.sort_by(|a, b| a.path.cmp(&b.path));
    status
}

// ---- running git ----

fn command(root: &Path) -> Command {
    let mut cmd = Command::new("git");
    cmd.arg("-C").arg(root).args(["-c", "core.quotepath=off"]);
    cmd.env("GIT_TERMINAL_PROMPT", "0").env("GIT_EDITOR", "true").env("LC_ALL", "C");
    cmd.stdin(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // No console window flashing for every git call.
        cmd.creation_flags(0x0800_0000);
    }
    cmd
}

fn run(root: &Path, args: &[&str]) -> Result<String, String> {
    let output = command(root).args(args).output().map_err(|e| format!("could not run git: {e}"))?;
    if output.status.success() {
        return Ok(String::from_utf8_lossy(&output.stdout).into_owned());
    }
    Err(error_message(&String::from_utf8_lossy(&output.stderr), &String::from_utf8_lossy(&output.stdout)))
}

/// One readable line from a failed command: git's own `fatal:`/`error:` line if present
/// (hints follow it), else the last line of whatever it printed.
fn error_message(stderr: &str, stdout: &str) -> String {
    let text = if stderr.trim().is_empty() { stdout } else { stderr };
    let line = text
        .lines()
        .find(|l| l.starts_with("fatal:") || l.starts_with("error:"))
        .or_else(|| text.lines().rfind(|l| !l.trim().is_empty()))
        .unwrap_or("git failed");
    line.trim_start_matches("fatal: ").trim_start_matches("error: ").trim().to_owned()
}

pub fn is_available() -> bool {
    // Same setup as every other git call: with no console of its own, a program that starts git
    // without a null stdin and the no-window flag waits for seconds before git answers.
    let mut cmd = Command::new("git");
    cmd.arg("--version").stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000);
    }
    cmd.status().is_ok_and(|s| s.success())
}

/// The repository root containing `start`, if any.
pub fn discover(start: &Path) -> Option<PathBuf> {
    let dir = if start.is_dir() { start } else { start.parent()? };
    let out = run(dir, &["rev-parse", "--show-toplevel"]).ok()?;
    let top = out.trim();
    (!top.is_empty()).then(|| PathBuf::from(top))
}

pub fn init(root: &Path) -> Result<String, String> {
    run(root, &["init"])
}

pub fn status(root: &Path) -> Result<Status, String> {
    let out = run(root, &["-c", "core.fsmonitor=false", "status", "--porcelain=v2", "-z", "--branch", "--untracked-files=all"])?;
    let mut status = parse_status(&out);
    status.has_remote = run(root, &["remote"]).is_ok_and(|r| !r.trim().is_empty());
    status.changes.truncate(5000);
    Ok(status)
}

/// A file's content at HEAD. `Ok(None)` for a file HEAD does not have (new, or no commits
/// yet) and for binary content.
pub fn show_head(root: &Path, rel: &str) -> Result<Option<String>, String> {
    let output = command(root).args(["show", &format!("HEAD:{rel}")]).output().map_err(|e| e.to_string())?;
    if !output.status.success() {
        return Ok(None);
    }
    Ok(String::from_utf8(output.stdout).ok())
}

fn paths_args<'a>(base: &[&'a str], paths: &'a [String]) -> Vec<&'a str> {
    let mut args: Vec<&str> = base.to_vec();
    args.push("--");
    args.extend(paths.iter().map(String::as_str));
    args
}

pub fn stage(root: &Path, paths: &[String]) -> Result<String, String> {
    run(root, &paths_args(&["add", "-A"], paths))
}

pub fn unstage(root: &Path, paths: &[String]) -> Result<String, String> {
    // Before the first commit there is no HEAD to restore from.
    run(root, &paths_args(&["restore", "--staged"], paths)).or_else(|_| run(root, &paths_args(&["rm", "--cached", "-r", "-q"], paths)))
}

/// Throw away unstaged changes to tracked files (untracked files are handled by the caller).
pub fn discard(root: &Path, paths: &[String]) -> Result<String, String> {
    run(root, &paths_args(&["restore"], paths))
}

pub fn commit(root: &Path, message: &str, all: bool) -> Result<String, String> {
    if message.trim().is_empty() {
        return Err("Please provide a commit message".to_owned());
    }
    let mut args = vec!["commit", "-m", message];
    if all {
        args.insert(1, "-a");
    }
    run(root, &args).map(|out| out.lines().next().unwrap_or("Committed").to_owned())
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Branch {
    pub name: String,
    pub current: bool,
    pub remote: bool,
}

pub fn branches(root: &Path) -> Result<Vec<Branch>, String> {
    let out = run(root, &["branch", "--all", "--format=%(HEAD)\u{1f}%(refname)"])?;
    let mut list: Vec<Branch> = out
        .lines()
        .filter_map(|line| {
            let (head, name) = line.split_once('\u{1f}')?;
            if let Some(local) = name.strip_prefix("refs/heads/") {
                Some(Branch { name: local.to_owned(), current: head == "*", remote: false })
            } else {
                let remote = name.strip_prefix("refs/remotes/")?;
                (!remote.ends_with("/HEAD")).then(|| Branch { name: remote.to_owned(), current: false, remote: true })
            }
        })
        .collect();
    list.sort_by(|a, b| (a.remote, &a.name).cmp(&(b.remote, &b.name)));
    Ok(list)
}

pub fn checkout(root: &Path, branch: &Branch) -> Result<String, String> {
    if branch.remote {
        run(root, &["switch", "--track", &branch.name])
    } else {
        run(root, &["switch", &branch.name])
    }
}

pub fn create_branch(root: &Path, name: &str) -> Result<String, String> {
    let name = name.trim();
    if name.is_empty() || name.contains(char::is_whitespace) {
        return Err("A branch name cannot be empty or contain spaces".to_owned());
    }
    run(root, &["switch", "-c", name])
}

pub fn fetch(root: &Path) -> Result<String, String> {
    run(root, &["fetch", "--all", "--prune"]).map(|_| "Fetched".to_owned())
}

pub fn pull(root: &Path) -> Result<String, String> {
    run(root, &["pull", "--no-edit"]).map(|out| out.lines().last().unwrap_or("Pulled").trim().to_owned())
}

/// Pull what others pushed, then push what is local: "Sync Changes". Pulling is skipped for a
/// branch with no upstream (nothing to pull yet), which makes this a first publish.
pub fn sync(root: &Path) -> Result<String, String> {
    let has_upstream = run(root, &["rev-parse", "--abbrev-ref", "--symbolic-full-name", "@{u}"]).is_ok();
    if has_upstream {
        run(root, &["pull", "--no-edit"])?;
    }
    push(root).map(|_| "Synchronized with the remote".to_owned())
}

pub fn push(root: &Path) -> Result<String, String> {
    match run(root, &["push"]) {
        Err(e) if e.contains("no upstream branch") || e.contains("--set-upstream") => {
            run(root, &["push", "--set-upstream", "origin", "HEAD"]).map(|_| "Pushed (upstream set)".to_owned())
        }
        other => other.map(|_| "Pushed".to_owned()),
    }
}

pub fn stash(root: &Path) -> Result<String, String> {
    run(root, &["stash", "push", "--include-untracked"]).map(|out| out.lines().next().unwrap_or("Stashed").to_owned())
}

pub fn stash_pop(root: &Path) -> Result<String, String> {
    run(root, &["stash", "pop"]).map(|_| "Applied the latest stash".to_owned())
}

/// A unified diff for one path: unstaged (`staged = false`, against the index) or staged
/// (against HEAD). Untracked files are shown as entirely added.
pub fn diff_text(root: &Path, rel: &str, staged: bool, untracked: bool) -> Result<String, String> {
    if untracked {
        let text = std::fs::read(root.join(rel)).map_err(|e| e.to_string())?;
        let Ok(text) = String::from_utf8(text) else { return Ok(format!("Binary file {rel} (new)\n")) };
        let body: String = text.lines().map(|l| format!("+{l}\n")).collect();
        return Ok(format!("diff --git a/{rel} b/{rel}\nnew file\n--- /dev/null\n+++ b/{rel}\n@@ -0,0 +1,{} @@\n{body}", text.lines().count()));
    }
    let mut args = vec!["diff", "--no-color"];
    if staged {
        args.push("--cached");
    }
    args.extend(["--", rel]);
    let out = run(root, &args)?;
    Ok(if out.is_empty() { "No changes.\n".to_owned() } else { out })
}

/// The staged changes as one diff, for describing them to a model. Lock files and files that
/// commonly hold secrets (`.env*`, keys, certificates) are left out so they are never sent.
pub fn staged_diff(root: &Path) -> Result<String, String> {
    run(
        root,
        &[
            "diff", "--cached", "--no-color", "--no-ext-diff", "--", ".", ":(exclude,glob)**/*.lock", ":(exclude,glob)**/package-lock.json",
            ":(exclude,glob)**/.env*", ":(exclude,glob)**/*.pem", ":(exclude,glob)**/*.key",
        ],
    )
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Commit {
    pub hash: String,
    pub author: String,
    pub when: String,
    pub subject: String,
}

pub fn log(root: &Path, rel: Option<&str>, limit: usize) -> Result<Vec<Commit>, String> {
    let n = format!("-{limit}");
    let mut args = vec!["log", &n, "--pretty=format:%H\u{1f}%an\u{1f}%ar\u{1f}%s"];
    if let Some(rel) = rel {
        args.extend(["--follow", "--", rel]);
    }
    let out = run(root, &args).or_else(|e| if e.contains("does not have any commits") { Ok(String::new()) } else { Err(e) })?;
    Ok(out
        .lines()
        .filter_map(|line| {
            let mut f = line.split('\u{1f}');
            Some(Commit { hash: f.next()?.to_owned(), author: f.next()?.to_owned(), when: f.next()?.to_owned(), subject: f.next()?.to_owned() })
        })
        .collect())
}

pub fn show_commit(root: &Path, hash: &str, rel: Option<&str>) -> Result<String, String> {
    let mut args = vec!["show", "--no-color", hash];
    if let Some(rel) = rel {
        args.extend(["--", rel]);
    }
    run(root, &args)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Blame {
    pub author: String,
    pub when: String,
    pub summary: String,
    pub hash: String,
    pub uncommitted: bool,
}

/// Blame one (1-based) line.
pub fn blame_line(root: &Path, rel: &str, line: usize) -> Result<Blame, String> {
    let range = format!("{line},{line}");
    let out = run(root, &["blame", "-L", &range, "--porcelain", "--", rel])?;
    Ok(parse_blame(&out, now_secs()))
}

fn now_secs() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64)
}

pub fn parse_blame(porcelain: &str, now: i64) -> Blame {
    let hash = porcelain.split_whitespace().next().unwrap_or_default().to_owned();
    let field = |name: &str| porcelain.lines().find_map(|l| l.strip_prefix(name).map(|v| v.trim().to_owned())).unwrap_or_default();
    let time: i64 = field("author-time ").parse().unwrap_or(0);
    let uncommitted = hash.chars().all(|c| c == '0');
    Blame { author: field("author "), when: relative_time(now - time), summary: field("summary "), hash: hash.chars().take(8).collect(), uncommitted }
}

/// "3 days ago" style text for an age in seconds.
pub fn relative_time(seconds: i64) -> String {
    let s = seconds.max(0);
    let (n, unit) = match s {
        0..=59 => return "just now".to_owned(),
        60..=3599 => (s / 60, "minute"),
        3600..=86_399 => (s / 3600, "hour"),
        86_400..=2_591_999 => (s / 86_400, "day"),
        2_592_000..=31_535_999 => (s / 2_592_000, "month"),
        _ => (s / 31_536_000, "year"),
    };
    format!("{n} {unit}{} ago", if n == 1 { "" } else { "s" })
}

/// `abs` relative to `root` with `/` separators, comparing case-insensitively on Windows.
pub fn relative_path(root: &Path, abs: &Path) -> Option<String> {
    let norm = |p: &Path| p.to_string_lossy().replace('\\', "/");
    let (r, a) = (norm(root), norm(abs));
    let r = r.trim_end_matches('/');
    let matches = if cfg!(windows) { a.len() > r.len() && a[..r.len()].eq_ignore_ascii_case(r) } else { a.starts_with(r) && a.len() > r.len() };
    (matches && a.as_bytes()[r.len()] == b'/').then(|| a[r.len() + 1..].to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn nul(parts: &[&str]) -> String {
        parts.iter().map(|p| format!("{p}\0")).collect()
    }

    #[test]
    fn parses_branch_headers_and_every_record_type() {
        let out = nul(&[
            "# branch.oid 0123456789abcdef",
            "# branch.head main",
            "# branch.upstream origin/main",
            "# branch.ab +2 -1",
            "1 .M N... 100644 100644 100644 aaa bbb src/lib.rs",
            "1 A. N... 000000 100644 100644 000 ccc new file.txt",
            "1 MM N... 100644 100644 100644 aaa ddd both.rs",
            "1 D. N... 100644 000000 000000 eee 000 gone.rs",
            "2 R. N... 100644 100644 100644 fff ggg R100 renamed to.rs",
            "from.rs",
            "u UU N... 100644 100644 100644 100644 h1 h2 h3 conflict.rs",
            "? untracked dir/x.txt",
        ]);
        let s = parse_status(&out);
        assert_eq!((s.branch.as_deref(), s.head.as_str(), s.upstream.as_deref()), (Some("main"), "01234567", Some("origin/main")));
        assert_eq!((s.ahead, s.behind), (2, 1));
        let by = |p: &str| s.changes.iter().find(|c| c.path == p).unwrap_or_else(|| panic!("missing {p}: {:?}", s.changes));
        assert_eq!((by("src/lib.rs").index, by("src/lib.rs").worktree), (None, Some(Kind::Modified)));
        assert_eq!(by("new file.txt").index, Some(Kind::Added), "paths may contain spaces");
        assert_eq!((by("both.rs").index, by("both.rs").worktree), (Some(Kind::Modified), Some(Kind::Modified)));
        assert_eq!(by("gone.rs").index, Some(Kind::Deleted));
        assert_eq!((by("renamed to.rs").index, by("renamed to.rs").orig_path.as_deref()), (Some(Kind::Renamed), Some("from.rs")));
        assert!(by("conflict.rs").conflicted && by("conflict.rs").letter() == '!');
        assert!(by("untracked dir/x.txt").untracked && by("untracked dir/x.txt").letter() == 'U');
        // Staged and unstaged views overlap for a file changed in both.
        assert!(s.staged().any(|c| c.path == "both.rs") && s.unstaged().any(|c| c.path == "both.rs"));
        assert!(!s.unstaged().any(|c| c.path == "new file.txt"));
    }

    #[test]
    fn detached_head_and_empty_output() {
        let s = parse_status(&nul(&["# branch.oid abcdef12", "# branch.head (detached)"]));
        assert_eq!(s.branch, None);
        assert!(s.changes.is_empty());
        assert_eq!(parse_status(""), Status::default());
    }

    #[test]
    fn error_messages_prefer_git_fatal_lines() {
        assert_eq!(error_message("fatal: not a git repository
hint: something
", ""), "not a git repository");
        assert_eq!(error_message("", "error: pathspec 'x' did not match
"), "pathspec 'x' did not match");
        assert_eq!(error_message("plain failure

", ""), "plain failure");
        assert_eq!(error_message("", ""), "git failed");
    }

    #[test]
    fn relative_paths_and_times() {
        let root = Path::new(if cfg!(windows) { r"C:\Proj" } else { "/proj" });
        let file = root.join("src").join("a.rs");
        assert_eq!(relative_path(root, &file).as_deref(), Some("src/a.rs"));
        assert_eq!(relative_path(root, root), None);
        assert_eq!(relative_path(root, Path::new(if cfg!(windows) { r"C:\Projects\x" } else { "/projects/x" })), None, "a sibling with the same prefix is not inside");
        assert_eq!(relative_time(30), "just now");
        assert_eq!(relative_time(3 * 86_400), "3 days ago");
        assert_eq!(relative_time(3600), "1 hour ago");
    }

    #[test]
    fn blame_porcelain_parses_author_age_and_summary() {
        let porcelain = "abcdef0123456789abcdef0123456789abcdef01 1 1 1\nauthor Ada Lovelace\nauthor-time 1000\nsummary Fix the thing\nfilename x.rs\n\tcode\n";
        let b = parse_blame(porcelain, 1000 + 2 * 86_400);
        assert_eq!((b.author.as_str(), b.when.as_str(), b.summary.as_str(), b.hash.as_str()), ("Ada Lovelace", "2 days ago", "Fix the thing", "abcdef01"));
        assert!(!b.uncommitted);
        assert!(parse_blame("0000000000000000000000000000000000000000 1 1 1\nauthor Not Committed Yet\n", 0).uncommitted);
    }

    // ---- real repositories ----

    fn git_ok(dir: &Path, args: &[&str]) {
        let out = Command::new("git").arg("-C").arg(dir).args(args).output().expect("git runs");
        assert!(out.status.success(), "git {args:?} failed: {}", String::from_utf8_lossy(&out.stderr));
    }

    fn repo(tag: &str) -> Option<PathBuf> {
        if !is_available() {
            eprintln!("skipped: git is not installed");
            return None;
        }
        let dir = std::env::temp_dir().join(format!("forge-git-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        git_ok(&dir, &["init", "-q", "-b", "main"]);
        for (k, v) in [("user.name", "Forge Test"), ("user.email", "test@example.com"), ("commit.gpgsign", "false"), ("core.autocrlf", "false")] {
            git_ok(&dir, &["config", k, v]);
        }
        Some(dir)
    }

    fn write(dir: &Path, name: &str, text: &str) {
        if let Some(parent) = dir.join(name).parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(dir.join(name), text).unwrap();
    }

    fn commit_all(dir: &Path, msg: &str) {
        git_ok(dir, &["add", "-A"]);
        git_ok(dir, &["commit", "-q", "-m", msg]);
    }

    fn by_path<'a>(s: &'a Status, p: &str) -> &'a Change {
        s.changes.iter().find(|c| c.path == p).unwrap_or_else(|| panic!("no {p} in {:?}", s.changes))
    }

    #[test]
    fn a_new_branch_publishes_and_then_syncs_with_the_remote() {
        let Some(dir) = repo("publish") else { return };
        write(&dir, "a.txt", "one
");
        commit_all(&dir, "first");
        assert!(!status(&dir).unwrap().has_remote, "no remote yet, so nothing to publish to");
        let bare = std::env::temp_dir().join(format!("forge-git-publish-bare-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&bare);
        git_ok(&std::env::temp_dir(), &["init", "-q", "--bare", "-b", "main", bare.to_str().unwrap()]);
        git_ok(&dir, &["remote", "add", "origin", bare.to_str().unwrap()]);
        let before = status(&dir).unwrap();
        assert!(before.has_remote && before.upstream.is_none(), "{before:?}");
        // First sync publishes: there is no upstream to pull from yet.
        sync(&dir).unwrap();
        let published = status(&dir).unwrap();
        assert_eq!(published.upstream.as_deref(), Some("origin/main"), "{published:?}");
        // A later local commit is "ahead", and sync pushes it.
        write(&dir, "a.txt", "two
");
        commit_all(&dir, "second");
        assert_eq!(status(&dir).unwrap().ahead, 1);
        sync(&dir).unwrap();
        assert_eq!(status(&dir).unwrap().ahead, 0);
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&bare);
    }

    #[test]
    fn staged_diff_leaves_out_lock_files_and_secret_like_files() {
        let Some(dir) = repo("aidiff") else { return };
        write(&dir, "src/lib.rs", "pub fn kept() {}\n");
        write(&dir, "Cargo.lock", "LOCK_CONTENT\n");
        write(&dir, "web/package-lock.json", "LOCK_JSON\n");
        write(&dir, ".env", "SECRET_TOKEN=abc\n");
        write(&dir, "config/.env.local", "SECRET_LOCAL=abc\n");
        write(&dir, "certs/server.pem", "PEM_CONTENT\n");
        git_ok(&dir, &["add", "-A"]);
        let diff = staged_diff(&dir).unwrap();
        assert!(diff.contains("pub fn kept"), "{diff}");
        for hidden in ["LOCK_CONTENT", "LOCK_JSON", "SECRET_TOKEN", "SECRET_LOCAL", "PEM_CONTENT"] {
            assert!(!diff.contains(hidden), "{hidden} must not be sent:\n{diff}");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn status_stage_unstage_commit_round_trip_on_a_real_repo() {
        let Some(dir) = repo("flow") else { return };
        assert_eq!(discover(&dir).map(|p| p.to_string_lossy().to_lowercase().replace('\\', "/")), Some(dir.to_string_lossy().to_lowercase().replace('\\', "/")));
        write(&dir, "a.txt", "one\n");
        write(&dir, "sub/b.txt", "two\n");
        let s = status(&dir).unwrap();
        assert_eq!(s.branch.as_deref(), Some("main"));
        assert!(by_path(&s, "a.txt").untracked && by_path(&s, "sub/b.txt").untracked, "untracked files are listed individually");

        stage(&dir, &["a.txt".to_owned()]).unwrap();
        let s = status(&dir).unwrap();
        assert_eq!(by_path(&s, "a.txt").index, Some(Kind::Added));
        unstage(&dir, &["a.txt".to_owned()]).unwrap(); // works even before the first commit
        assert!(by_path(&status(&dir).unwrap(), "a.txt").untracked);

        stage(&dir, &["a.txt".to_owned(), "sub/b.txt".to_owned()]).unwrap();
        assert_eq!(commit(&dir, "", false).unwrap_err(), "Please provide a commit message");
        commit(&dir, "first commit", false).unwrap();
        assert!(status(&dir).unwrap().changes.is_empty());
        assert_eq!(log(&dir, None, 10).unwrap()[0].subject, "first commit");

        // Modify, stage part, delete another.
        write(&dir, "a.txt", "one\nmore\n");
        std::fs::remove_file(dir.join("sub/b.txt")).unwrap();
        let s = status(&dir).unwrap();
        assert_eq!(by_path(&s, "a.txt").worktree, Some(Kind::Modified));
        assert_eq!(by_path(&s, "sub/b.txt").worktree, Some(Kind::Deleted));
        stage(&dir, &["sub/b.txt".to_owned()]).unwrap();
        assert_eq!(by_path(&status(&dir).unwrap(), "sub/b.txt").index, Some(Kind::Deleted));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn head_content_diff_discard_and_blame() {
        let Some(dir) = repo("content") else { return };
        write(&dir, "f.txt", "alpha\nbeta\n");
        assert_eq!(show_head(&dir, "f.txt").unwrap(), None, "no commits yet");
        commit_all(&dir, "init");
        assert_eq!(show_head(&dir, "f.txt").unwrap().as_deref(), Some("alpha\nbeta\n"));
        assert_eq!(show_head(&dir, "nope.txt").unwrap(), None, "a file HEAD does not have");

        write(&dir, "f.txt", "alpha\nBETA\n");
        let d = diff_text(&dir, "f.txt", false, false).unwrap();
        assert!(d.contains("-beta") && d.contains("+BETA"), "{d}");
        assert_eq!(diff_text(&dir, "f.txt", true, false).unwrap(), "No changes.\n");
        stage(&dir, &["f.txt".to_owned()]).unwrap();
        assert!(diff_text(&dir, "f.txt", true, false).unwrap().contains("+BETA"));

        write(&dir, "new.txt", "x\ny\n");
        let nd = diff_text(&dir, "new.txt", false, true).unwrap();
        assert!(nd.contains("+x") && nd.contains("@@ -0,0 +1,2 @@"), "{nd}");

        // Blame: a committed line is attributed; an edited one is not committed yet.
        let committed = blame_line(&dir, "f.txt", 1).unwrap();
        assert_eq!((committed.author.as_str(), committed.summary.as_str(), committed.uncommitted), ("Forge Test", "init", false));
        assert!(blame_line(&dir, "f.txt", 2).unwrap().uncommitted);

        // Discard restores unstaged edits only.
        write(&dir, "f.txt", "scribble\n");
        discard(&dir, &["f.txt".to_owned()]).unwrap();
        assert_eq!(std::fs::read_to_string(dir.join("f.txt")).unwrap(), "alpha\nBETA\n", "back to the staged version");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn branches_checkout_create_and_stash() {
        let Some(dir) = repo("branches") else { return };
        write(&dir, "f.txt", "base\n");
        commit_all(&dir, "base");
        create_branch(&dir, "feature").unwrap();
        assert!(create_branch(&dir, "bad name").is_err());
        assert!(create_branch(&dir, "feature").is_err(), "already exists");
        let list = branches(&dir).unwrap();
        assert!(list.iter().any(|b| b.name == "feature" && b.current));
        assert!(list.iter().any(|b| b.name == "main" && !b.current));
        checkout(&dir, &Branch { name: "main".into(), current: false, remote: false }).unwrap();
        assert_eq!(status(&dir).unwrap().branch.as_deref(), Some("main"));

        write(&dir, "f.txt", "dirty\n");
        write(&dir, "u.txt", "untracked\n");
        stash(&dir).unwrap();
        let s = status(&dir).unwrap();
        assert!(s.changes.is_empty(), "stash includes untracked files: {:?}", s.changes);
        assert_eq!(std::fs::read_to_string(dir.join("f.txt")).unwrap(), "base\n");
        stash_pop(&dir).unwrap();
        assert_eq!(std::fs::read_to_string(dir.join("f.txt")).unwrap(), "dirty\n");
        assert!(dir.join("u.txt").exists());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn push_pull_fetch_and_ahead_behind_against_a_local_remote() {
        let Some(a) = repo("remote-a") else { return };
        let bare = std::env::temp_dir().join(format!("forge-git-bare-{}", std::process::id()));
        let b = std::env::temp_dir().join(format!("forge-git-remote-b-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&bare);
        let _ = std::fs::remove_dir_all(&b);
        git_ok(&std::env::temp_dir(), &["init", "-q", "--bare", "-b", "main", bare.to_str().unwrap()]);
        write(&a, "f.txt", "v1\n");
        commit_all(&a, "v1");
        git_ok(&a, &["remote", "add", "origin", bare.to_str().unwrap()]);

        // No upstream yet: push sets it.
        assert_eq!(push(&a).unwrap(), "Pushed (upstream set)");
        let s = status(&a).unwrap();
        assert_eq!((s.upstream.as_deref(), s.ahead, s.behind), (Some("origin/main"), 0, 0));

        // A second clone commits and pushes; the first is now behind after a fetch.
        git_ok(&std::env::temp_dir(), &["clone", "-q", bare.to_str().unwrap(), b.to_str().unwrap()]);
        for (k, v) in [("user.name", "Other"), ("user.email", "o@example.com"), ("commit.gpgsign", "false")] {
            git_ok(&b, &["config", k, v]);
        }
        write(&b, "f.txt", "v2\n");
        commit_all(&b, "v2 from b");
        push(&b).unwrap();
        fetch(&a).unwrap();
        let s = status(&a).unwrap();
        assert_eq!((s.ahead, s.behind), (0, 1));
        pull(&a).unwrap();
        assert_eq!(std::fs::read_to_string(a.join("f.txt")).unwrap(), "v2\n");

        // Local commit puts the first clone ahead; history for the file lists both commits.
        write(&a, "g.txt", "g\n");
        commit_all(&a, "local");
        assert_eq!(status(&a).unwrap().ahead, 1);
        let history = log(&a, Some("f.txt"), 10).unwrap();
        assert_eq!(history.iter().map(|c| c.subject.as_str()).collect::<Vec<_>>(), ["v2 from b", "v1"]);
        assert!(show_commit(&a, &history[0].hash, Some("f.txt")).unwrap().contains("+v2"));
        for d in [&a, &b, &bare] {
            let _ = std::fs::remove_dir_all(d);
        }
    }

    #[test]
    fn errors_are_short_readable_messages() {
        let Some(dir) = repo("errors") else { return };
        let e = checkout(&dir, &Branch { name: "nope".into(), current: false, remote: false }).unwrap_err();
        assert!(!e.is_empty() && !e.contains('\n'), "{e:?}");
        assert!(pull(&dir).is_err());
        assert!(discover(&std::env::temp_dir().join("definitely-not-a-repo-xyz")).is_none());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
