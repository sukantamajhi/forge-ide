//! Source control through the `git` command line, so behavior (config, hooks, credential
//! helpers) matches the user's own git. There is no terminal to prompt on
//! (`GIT_TERMINAL_PROMPT=0`); a command that needs a login asks through [`crate::askpass`],
//! which shows the question in the editor. A running operation can be cancelled.

use std::cell::RefCell;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

/// Lets another thread stop the git process a background job is running.
#[derive(Default)]
pub struct JobControl {
    pid: AtomicU32,
    cancelled: AtomicBool,
}

impl JobControl {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::SeqCst)
    }

    /// Stop the running git process and everything it started (ssh, credential helpers).
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::SeqCst);
        let pid = self.pid.load(Ordering::SeqCst);
        if pid != 0 {
            kill_tree(pid);
        }
    }
}

fn kill_tree(pid: u32) {
    let mut cmd = if cfg!(windows) {
        let mut c = Command::new("taskkill");
        c.args(["/PID", &pid.to_string(), "/T", "/F"]);
        c
    } else {
        let mut c = Command::new("kill");
        c.args(["-TERM", &pid.to_string()]);
        c
    };
    cmd.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000);
    }
    let _ = cmd.status();
}

thread_local! {
    /// The job the current thread works for, so `run` can register its process with it.
    static JOB: RefCell<Option<Arc<JobControl>>> = const { RefCell::new(None) };
}

/// Run `f` as `job`: git processes it starts can be cancelled through `job`.
pub fn with_job<T>(job: &Arc<JobControl>, f: impl FnOnce() -> T) -> T {
    JOB.with(|slot| *slot.borrow_mut() = Some(Arc::clone(job)));
    let result = f();
    JOB.with(|slot| *slot.borrow_mut() = None);
    result
}

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
    /// A merge, rebase or similar that stopped and waits to be continued or aborted.
    pub state: RepoState,
}

/// An operation git has paused, usually for conflicts.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum RepoState {
    #[default]
    Clean,
    Merge,
    Rebase { step: usize, total: usize },
    CherryPick,
    Revert,
}

impl RepoState {
    pub fn label(&self) -> Option<String> {
        match self {
            RepoState::Clean => None,
            RepoState::Merge => Some("Merge in progress".to_owned()),
            RepoState::Rebase { step, total } if *total > 0 => Some(format!("Rebase in progress ({step}/{total})")),
            RepoState::Rebase { .. } => Some("Rebase in progress".to_owned()),
            RepoState::CherryPick => Some("Cherry-pick in progress".to_owned()),
            RepoState::Revert => Some("Revert in progress".to_owned()),
        }
    }
}

/// What the repository is in the middle of, from the marker files git keeps in its directory.
pub fn repo_state(root: &Path) -> RepoState {
    let Ok(dir) = run(root, &["rev-parse", "--git-dir"]) else { return RepoState::Clean };
    let dir = root.join(dir.trim());
    let number = |file: &str| std::fs::read_to_string(dir.join(file)).ok().and_then(|t| t.trim().parse::<usize>().ok()).unwrap_or(0);
    if dir.join("rebase-merge").is_dir() {
        return RepoState::Rebase { step: number("rebase-merge/msgnum"), total: number("rebase-merge/end") };
    }
    if dir.join("rebase-apply").is_dir() {
        return RepoState::Rebase { step: number("rebase-apply/next"), total: number("rebase-apply/last") };
    }
    if dir.join("MERGE_HEAD").exists() {
        RepoState::Merge
    } else if dir.join("CHERRY_PICK_HEAD").exists() {
        RepoState::CherryPick
    } else if dir.join("REVERT_HEAD").exists() {
        RepoState::Revert
    } else {
        RepoState::Clean
    }
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

/// A unified diff of two texts (empty when they are equal), made by `git diff --no-index` on temporary copies.
/// `a_name` and `b_name` appear in the headers. Works outside any repository.
pub fn diff_texts(a: &str, b: &str, a_name: &str, b_name: &str) -> String {
    if a == b {
        return String::new();
    }
    let dir = std::env::temp_dir().join(format!("forge-compare-{}-{}", std::process::id(), std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos())));
    if std::fs::create_dir_all(&dir).is_err() {
        return String::new();
    }
    let (left, right) = (dir.join("a"), dir.join("b"));
    let text = if std::fs::write(&left, a).is_ok() && std::fs::write(&right, b).is_ok() {
        let mut cmd = Command::new("git");
        cmd.current_dir(&dir).args(["-c", "core.quotepath=off", "diff", "--no-index", "--no-color", "--no-ext-diff", "--", "a", "b"]).stdin(Stdio::null());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            cmd.creation_flags(0x0800_0000);
        }
        // Exit code 1 means "they differ", which is the normal case here.
        cmd.output().map(|o| String::from_utf8_lossy(&o.stdout).into_owned()).unwrap_or_default()
    } else {
        String::new()
    };
    let _ = std::fs::remove_dir_all(&dir);
    text.replace("a/a", &format!("a/{a_name}")).replace("b/b", &format!("b/{b_name}")).replace("diff --git a/a b/b", &format!("diff --git a/{a_name} b/{b_name}"))
}

fn command(root: &Path) -> Command {
    let mut cmd = Command::new("git");
    cmd.arg("-C").arg(root).args(["-c", "core.quotepath=off"]);
    cmd.env("GIT_TERMINAL_PROMPT", "0").env("GIT_EDITOR", "true").env("LC_ALL", "C");
    cmd.envs(crate::askpass::env_vars());
    // Signing commits asks for the passphrase in the editor (see `gpg_wrap`).
    if let Some(program) = crate::askpass::gpg_program() {
        cmd.arg("-c").arg(format!("gpg.program={program}"));
    }
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
    run_with_input(command(root).args(args), None)
}

/// Run a prepared git command, optionally feeding `input` to it. The process is registered with the
/// current thread's [`JobControl`] so it can be cancelled.
/// How long to keep reading a finished process's output when something else still holds its pipe open.
const PIPE_GRACE: std::time::Duration = std::time::Duration::from_millis(1500);

/// Wait for `child` to exit and return what it printed. Unlike `wait_with_output` this does not wait for
/// the output pipes to close: a program git starts (gpg-agent, a credential cache, an ssh master) can
/// keep running with the pipes inherited, and would make every such command hang after it had finished.
/// Output is read on threads, so a large output cannot fill a pipe and stall the process.
pub(crate) fn wait_and_collect(mut child: std::process::Child) -> std::io::Result<std::process::Output> {
    use std::io::Read;
    use std::sync::Mutex;
    fn drain(pipe: Option<impl Read + Send + 'static>) -> (Arc<Mutex<Vec<u8>>>, Arc<AtomicBool>) {
        let buffer = Arc::new(Mutex::new(Vec::new()));
        let done = Arc::new(AtomicBool::new(false));
        let (b, d) = (Arc::clone(&buffer), Arc::clone(&done));
        std::thread::spawn(move || {
            if let Some(mut pipe) = pipe {
                let mut chunk = [0u8; 8192];
                while let Ok(n) = pipe.read(&mut chunk) {
                    if n == 0 {
                        break;
                    }
                    if let Ok(mut buf) = b.lock() {
                        buf.extend_from_slice(&chunk[..n]);
                    }
                }
            }
            d.store(true, Ordering::SeqCst);
        });
        (buffer, done)
    }
    let (stdout, stdout_done) = drain(child.stdout.take());
    let (stderr, stderr_done) = drain(child.stderr.take());
    let status = child.wait()?;
    // The pipes normally close with the process; give leftovers a moment, then take what has arrived.
    let deadline = std::time::Instant::now() + PIPE_GRACE;
    while !(stdout_done.load(Ordering::SeqCst) && stderr_done.load(Ordering::SeqCst)) && std::time::Instant::now() < deadline {
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    let take = |buffer: &Arc<Mutex<Vec<u8>>>| buffer.lock().map(|b| b.clone()).unwrap_or_default();
    Ok(std::process::Output { status, stdout: take(&stdout), stderr: take(&stderr) })
}

fn run_with_input(cmd: &mut Command, input: Option<&str>) -> Result<String, String> {
    cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
    if input.is_some() {
        cmd.stdin(Stdio::piped());
    }
    let mut child = cmd.spawn().map_err(|e| format!("could not run git: {e}"))?;
    let job = JOB.with(|slot| slot.borrow().clone());
    if let Some(job) = &job {
        job.pid.store(child.id(), Ordering::SeqCst);
        // Cancelled between starting the job and starting the process.
        if job.is_cancelled() {
            kill_tree(child.id());
        }
    }
    if let (Some(text), Some(mut stdin)) = (input, child.stdin.take()) {
        let _ = stdin.write_all(text.as_bytes());
    }
    let output = wait_and_collect(child);
    if let Some(job) = &job {
        job.pid.store(0, Ordering::SeqCst);
        if job.is_cancelled() {
            return Err("cancelled".to_owned());
        }
    }
    let output = output.map_err(|e| format!("could not run git: {e}"))?;
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
    status.state = repo_state(root);
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

/// Apply a patch to the index (`cached`) or the working tree, forwards or `reverse`d. This is how one
/// hunk or some lines are staged (forwards, to the index), unstaged (reversed, to the index) or
/// discarded (reversed, to the working tree). `--recount` lets git fix the line counts of a patch
/// that was cut down to chosen lines.
pub fn apply_patch(root: &Path, patch: &str, cached: bool, reverse: bool) -> Result<String, String> {
    let mut cmd = command(root);
    cmd.args(["apply", "--recount", "--whitespace=nowarn"]);
    if cached {
        cmd.arg("--cached");
    }
    if reverse {
        cmd.arg("--reverse");
    }
    cmd.arg("-");
    run_with_input(&mut cmd, Some(patch))
}

/// Mark a conflicted file resolved: stage its current content.
pub fn mark_resolved(root: &Path, rel: &str) -> Result<String, String> {
    run(root, &["add", "--", rel])
}

/// When an operation stops with conflicts, say what to do instead of showing git's raw failure.
fn stopped(root: &Path, result: Result<String, String>, done: String) -> Result<String, String> {
    match result {
        Ok(_) => Ok(done),
        Err(e) if e == "cancelled" => Err(e),
        Err(e) if repo_state(root) != RepoState::Clean => Err(format!("stopped with conflicts. Resolve them and stage the files, then Continue ({e})")),
        Err(e) => Err(e),
    }
}

pub fn rebase(root: &Path, onto: &str) -> Result<String, String> {
    let result = run(root, &["rebase", onto]);
    stopped(root, result, format!("Rebased onto {onto}"))
}

pub fn rebase_continue(root: &Path) -> Result<String, String> {
    let result = run(root, &["rebase", "--continue"]);
    stopped(root, result, "Rebase finished".to_owned())
}

pub fn rebase_skip(root: &Path) -> Result<String, String> {
    let result = run(root, &["rebase", "--skip"]);
    stopped(root, result, "Rebase finished".to_owned())
}

pub fn rebase_abort(root: &Path) -> Result<String, String> {
    run(root, &["rebase", "--abort"]).map(|_| "Rebase aborted".to_owned())
}

pub fn merge(root: &Path, branch: &str) -> Result<String, String> {
    let result = run(root, &["merge", "--no-edit", branch]);
    stopped(root, result, format!("Merged {branch}"))
}

pub fn merge_abort(root: &Path) -> Result<String, String> {
    run(root, &["merge", "--abort"]).map(|_| "Merge aborted".to_owned())
}

/// Finish a merge whose conflicts are all staged.
pub fn merge_continue(root: &Path) -> Result<String, String> {
    run(root, &["commit", "--no-edit"]).map(|out| out.lines().next().unwrap_or("Merge committed").to_owned())
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

/// Whether git tracks `rel` (it has an index entry).
pub fn is_tracked(root: &Path, rel: &str) -> bool {
    run(root, &["ls-files", "--error-unmatch", "--", rel]).is_ok()
}

// ---- cherry-pick and revert ----

fn short_hash(hash: &str) -> String {
    hash.chars().take(8).collect()
}

pub fn cherry_pick(root: &Path, hash: &str) -> Result<String, String> {
    let result = run(root, &["cherry-pick", hash]);
    stopped(root, result, format!("Cherry-picked {}", short_hash(hash)))
}

pub fn cherry_pick_continue(root: &Path) -> Result<String, String> {
    let result = run(root, &["cherry-pick", "--continue"]);
    stopped(root, result, "Cherry-pick finished".to_owned())
}

pub fn cherry_pick_skip(root: &Path) -> Result<String, String> {
    let result = run(root, &["cherry-pick", "--skip"]);
    stopped(root, result, "Cherry-pick finished".to_owned())
}

pub fn cherry_pick_abort(root: &Path) -> Result<String, String> {
    run(root, &["cherry-pick", "--abort"]).map(|_| "Cherry-pick aborted".to_owned())
}

/// Undo a commit by committing its opposite.
pub fn revert(root: &Path, hash: &str) -> Result<String, String> {
    let result = run(root, &["revert", "--no-edit", hash]);
    stopped(root, result, format!("Reverted {}", short_hash(hash)))
}

pub fn revert_continue(root: &Path) -> Result<String, String> {
    let result = run(root, &["revert", "--continue"]);
    stopped(root, result, "Revert finished".to_owned())
}

pub fn revert_skip(root: &Path) -> Result<String, String> {
    let result = run(root, &["revert", "--skip"]);
    stopped(root, result, "Revert finished".to_owned())
}

pub fn revert_abort(root: &Path) -> Result<String, String> {
    run(root, &["revert", "--abort"]).map(|_| "Revert aborted".to_owned())
}

/// Commits from every branch, newest first, to pick one from.
pub fn log_all(root: &Path, limit: usize) -> Result<Vec<Commit>, String> {
    let n = format!("-{limit}");
    let out = run(root, &["log", "--all", "--date-order", &n, "--pretty=format:%H\u{1f}%an\u{1f}%ar\u{1f}%s"]).or_else(|e| if e.contains("does not have any commits") { Ok(String::new()) } else { Err(e) })?;
    Ok(out
        .lines()
        .filter_map(|line| {
            let mut f = line.split('\u{1f}');
            Some(Commit { hash: f.next()?.to_owned(), author: f.next()?.to_owned(), when: f.next()?.to_owned(), subject: f.next()?.to_owned() })
        })
        .collect())
}

// ---- interactive rebase ----

/// What to do with one commit in an interactive rebase.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TodoAction {
    Pick,
    /// Keep the changes, use this message.
    Reword(String),
    /// Fold into the commit above, joining the messages.
    Squash,
    /// Fold into the commit above, dropping this message.
    Fixup,
    Drop,
}

impl TodoAction {
    pub fn label(&self) -> &'static str {
        match self {
            TodoAction::Pick => "pick",
            TodoAction::Reword(_) => "reword",
            TodoAction::Squash => "squash",
            TodoAction::Fixup => "fixup",
            TodoAction::Drop => "drop",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TodoItem {
    pub hash: String,
    pub subject: String,
    pub action: TodoAction,
}

/// The commits an interactive rebase onto `base` would replay, oldest first, all set to pick.
pub fn rebase_commits(root: &Path, base: &str) -> Result<Vec<TodoItem>, String> {
    let range = format!("{base}..HEAD");
    let out = run(root, &["log", "--reverse", "--no-merges", "--pretty=format:%H\u{1f}%s", &range])?;
    Ok(out
        .lines()
        .filter_map(|line| {
            let (hash, subject) = line.split_once('\u{1f}')?;
            Some(TodoItem { hash: hash.to_owned(), subject: subject.to_owned(), action: TodoAction::Pick })
        })
        .collect())
}

/// The todo list git is given, or why the choices cannot be done (a squash with nothing above it).
pub fn rebase_todo(items: &[TodoItem]) -> Result<String, String> {
    let mut text = String::new();
    let mut have_commit = false;
    for item in items {
        match item.action {
            TodoAction::Drop => {}
            TodoAction::Squash | TodoAction::Fixup if !have_commit => {
                return Err(format!("\"{}\" cannot be {}ed into a commit above it: it is the first one kept", item.subject, item.action.label()));
            }
            _ => have_commit = true,
        }
        text.push_str(&format!("{} {} {}\n", item.action.label(), item.hash, item.subject.replace('\n', " ")));
    }
    if !have_commit {
        return Err("every commit is dropped, so there is nothing to rebase".to_owned());
    }
    Ok(text)
}

/// Rebase the current branch onto `base`, applying `items` (their order and actions). No editor opens:
/// git's sequence editor copies our todo list in, and its commit-message editor hands out the new
/// messages in the order git asks for them.
pub fn interactive_rebase(root: &Path, base: &str, items: &[TodoItem]) -> Result<String, String> {
    let todo = rebase_todo(items)?;
    let dir = std::env::temp_dir().join(format!(
        "forge-rebase-{}-{}",
        std::process::id(),
        SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_nanos())
    ));
    std::fs::create_dir_all(&dir).map_err(|e| format!("could not prepare the rebase: {e}"))?;
    let result = (|| {
        std::fs::write(dir.join("todo.txt"), &todo).map_err(|e| e.to_string())?;
        // One file per time git opens the message editor: a reword gets its text, a squash keeps the joined message.
        let mut queue = 0;
        for item in items {
            let contents = match &item.action {
                TodoAction::Reword(message) => message.trim_end().to_owned() + "\n",
                TodoAction::Squash => "@keep".to_owned(),
                _ => continue,
            };
            queue += 1;
            std::fs::write(dir.join(format!("msg-{queue:03}")), contents).map_err(|e| e.to_string())?;
        }
        let script = "#!/bin/sh\ndir=\"$(dirname \"$0\")\"\nnext=$(ls \"$dir\"/msg-* 2>/dev/null | head -n 1)\nif [ -n \"$next\" ]; then\n  if [ \"$(head -c 5 \"$next\")\" != \"@keep\" ]; then cat \"$next\" > \"$1\"; fi\n  rm -f \"$next\"\nfi\n";
        std::fs::write(dir.join("edit.sh"), script).map_err(|e| e.to_string())?;
        let slash = |p: std::path::PathBuf| p.to_string_lossy().replace('\\', "/");
        let mut cmd = command(root);
        cmd.env("GIT_SEQUENCE_EDITOR", format!("cp \"{}\"", slash(dir.join("todo.txt"))));
        cmd.env("GIT_EDITOR", format!("sh \"{}\"", slash(dir.join("edit.sh"))));
        cmd.args(["rebase", "-i", base]);
        run_with_input(&mut cmd, None)
    })();
    let outcome = stopped(root, result, format!("Rebased {} commit{}", items.iter().filter(|i| i.action != TodoAction::Drop).count(), if items.len() == 1 { "" } else { "s" }));
    // Leave the files while the rebase waits for conflicts: later steps may still run the editor.
    if repo_state(root) == RepoState::Clean {
        let _ = std::fs::remove_dir_all(&dir);
    }
    outcome
}

// ---- merge conflicts: the common ancestor ----

/// For each conflict of `rel` as git's own three-way merge reports them, the lines of the common ancestor.
/// Read from the index (stages 1, 2 and 3) and merged in memory with `git merge-file --diff3`, so the
/// working file is not touched. An error when there is no common ancestor (both sides added the file).
pub fn conflict_bases(root: &Path, rel: &str) -> Result<Vec<Option<Vec<String>>>, String> {
    let stage = |n: u8| run(root, &["show", &format!(":{n}:{rel}")]);
    let (base, ours, theirs) = (stage(1).map_err(|_| "there is no common ancestor (both sides added this file)".to_owned())?, stage(2)?, stage(3)?);
    let dir = std::env::temp_dir().join(format!("forge-base-{}-{}", std::process::id(), SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_nanos())));
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let result = (|| {
        for (name, text) in [("base", &base), ("ours", &ours), ("theirs", &theirs)] {
            std::fs::write(dir.join(name), text).map_err(|e| e.to_string())?;
        }
        // The exit status is the number of conflicts, so only the output matters.
        let output = command(root)
            .args(["merge-file", "-p", "--diff3", "-L", "ours", "-L", "base", "-L", "theirs"])
            .arg(dir.join("ours"))
            .arg(dir.join("base"))
            .arg(dir.join("theirs"))
            .output()
            .map_err(|e| format!("could not run git: {e}"))?;
        Ok(crate::conflict::find(&String::from_utf8_lossy(&output.stdout)).into_iter().map(|c| c.base).collect())
    })();
    let _ = std::fs::remove_dir_all(&dir);
    result
}

// ---- staging lines of a new file ----

/// Put `content` in the index as the file `rel`, without touching the working file. This is how some
/// lines of an untracked file are staged: only they go in, and the rest stays as a working change.
pub fn stage_new_file(root: &Path, rel: &str, content: &str) -> Result<String, String> {
    let mut hash = command(root);
    hash.args(["hash-object", "-w", "--stdin"]);
    let blob = run_with_input(&mut hash, Some(content))?.trim().to_owned();
    run(root, &["update-index", "--add", "--cacheinfo", &format!("100644,{blob},{rel}")])
}

// ---- submodules ----

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Submodule {
    pub path: String,
    pub hash: String,
    /// `' '` up to date, `'-'` not initialised, `'+'` at a different commit than the repository records, `'U'` conflicts.
    pub state: char,
    pub describe: String,
}

impl Submodule {
    pub fn state_label(&self) -> &'static str {
        match self.state {
            '-' => "not initialised",
            '+' => "at another commit than recorded",
            'U' => "merge conflicts",
            _ => "up to date",
        }
    }
}

pub fn parse_submodules(output: &str) -> Vec<Submodule> {
    output
        .lines()
        .filter_map(|line| {
            let state = line.chars().next()?;
            let rest = line.get(1..)?;
            let (hash, tail) = rest.split_once(' ')?;
            let (path, describe) = match tail.split_once(" (") {
                Some((path, d)) => (path, d.trim_end_matches(')')),
                None => (tail, ""),
            };
            Some(Submodule { path: path.to_owned(), hash: hash.to_owned(), state, describe: describe.to_owned() })
        })
        .collect()
}

pub fn submodules(root: &Path) -> Result<Vec<Submodule>, String> {
    Ok(parse_submodules(&run(root, &["submodule", "status", "--recursive"])?))
}

/// Fetch and check out every submodule at the commit this repository records, initialising new ones.
pub fn submodule_update(root: &Path) -> Result<String, String> {
    run(root, &["submodule", "update", "--init", "--recursive"]).map(|_| "Submodules are up to date".to_owned())
}

// ---- worktrees ----

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Worktree {
    pub path: String,
    pub head: String,
    /// `None` when detached or bare.
    pub branch: Option<String>,
    pub bare: bool,
    pub locked: bool,
    /// The folder is gone; `git worktree prune` forgets it.
    pub prunable: bool,
}

pub fn parse_worktrees(output: &str) -> Vec<Worktree> {
    let mut list = Vec::new();
    for block in output.split("\n\n") {
        let mut tree = Worktree::default();
        for line in block.lines() {
            let (key, value) = line.split_once(' ').unwrap_or((line, ""));
            match key {
                "worktree" => tree.path = value.to_owned(),
                "HEAD" => tree.head = value.chars().take(8).collect(),
                "branch" => tree.branch = Some(value.trim_start_matches("refs/heads/").to_owned()),
                "bare" => tree.bare = true,
                "locked" => tree.locked = true,
                "prunable" => tree.prunable = true,
                _ => {}
            }
        }
        if !tree.path.is_empty() {
            list.push(tree);
        }
    }
    list
}

pub fn worktrees(root: &Path) -> Result<Vec<Worktree>, String> {
    Ok(parse_worktrees(&run(root, &["worktree", "list", "--porcelain"])?))
}

/// Check `branch` out in a new folder `path`. `create` makes the branch (from HEAD) first.
pub fn worktree_add(root: &Path, path: &str, branch: &str, create: bool) -> Result<String, String> {
    let result = if create { run(root, &["worktree", "add", "-b", branch, path]) } else { run(root, &["worktree", "add", path, branch]) };
    result.map(|_| format!("Added a worktree for {branch} in {path}"))
}

pub fn worktree_remove(root: &Path, path: &str) -> Result<String, String> {
    run(root, &["worktree", "remove", path]).map(|_| format!("Removed the worktree {path}"))
}

// ---- the commit graph ----

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GraphCommit {
    pub hash: String,
    pub parents: Vec<String>,
    pub author: String,
    pub when: String,
    /// Branches and tags pointing here (`HEAD -> main`, `origin/main`, `tag: v1`).
    pub refs: Vec<String>,
    pub subject: String,
}

/// Every branch's history, newest first, with the parents the graph needs.
pub fn graph_log(root: &Path, limit: usize) -> Result<Vec<GraphCommit>, String> {
    let n = format!("-{limit}");
    let out = run(root, &["log", "--all", "--date-order", &n, "--pretty=format:%H\u{1f}%P\u{1f}%an\u{1f}%ar\u{1f}%D\u{1f}%s"]).or_else(|e| if e.contains("does not have any commits") { Ok(String::new()) } else { Err(e) })?;
    Ok(out
        .lines()
        .filter_map(|line| {
            let mut f = line.split('\u{1f}');
            let hash = f.next()?.to_owned();
            let parents = f.next()?.split_whitespace().map(str::to_owned).collect();
            let (author, when) = (f.next()?.to_owned(), f.next()?.to_owned());
            let refs = f.next()?.split(", ").filter(|r| !r.is_empty()).map(str::to_owned).collect();
            Some(GraphCommit { hash, parents, author, when, refs, subject: f.next()?.to_owned() })
        })
        .collect())
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

    fn numbered(n: usize) -> String {
        (1..=n).map(|i| format!("line {i}\n")).collect()
    }

    fn index_text(dir: &Path) -> String {
        run(dir, &["show", ":f.txt"]).unwrap()
    }

    #[test]
    fn one_hunk_is_staged_and_unstaged_through_a_patch() {
        let Some(dir) = repo("hunks") else { return };
        write(&dir, "f.txt", &numbered(14));
        commit_all(&dir, "base");
        write(&dir, "f.txt", &numbered(14).replace("line 2\n", "LINE 2\n").replace("line 13\n", "LINE 13\n"));
        let patch = crate::patch::parse(&diff_text(&dir, "f.txt", false, false).unwrap()).unwrap();
        assert_eq!(patch.hunks.len(), 2, "two edits far apart are two hunks");
        apply_patch(&dir, &patch.hunk_patch(0).unwrap(), true, false).unwrap();
        let staged = diff_text(&dir, "f.txt", true, false).unwrap();
        assert!(staged.contains("+LINE 2") && !staged.contains("LINE 13"), "{staged}");
        let unstaged = diff_text(&dir, "f.txt", false, false).unwrap();
        assert!(unstaged.contains("+LINE 13") && !unstaged.contains("+LINE 2\n"), "{unstaged}");
        // Unstage it again by applying the staged hunk backwards to the index.
        let staged_patch = crate::patch::parse(&staged).unwrap();
        apply_patch(&dir, &staged_patch.hunk_patch(0).unwrap(), true, true).unwrap();
        assert_eq!(diff_text(&dir, "f.txt", true, false).unwrap(), "No changes.\n");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn hunks_of_a_file_with_windows_line_endings_stage_and_discard() {
        let Some(dir) = repo("crlf") else { return };
        let crlf = |text: String| text.replace('\n', "\r\n");
        write(&dir, "f.txt", &crlf(numbered(14)));
        commit_all(&dir, "base");
        write(&dir, "f.txt", &crlf(numbered(14).replace("line 2\n", "LINE 2\n").replace("line 13\n", "LINE 13\n")));
        let patch = crate::patch::parse(&diff_text(&dir, "f.txt", false, false).unwrap()).unwrap();
        assert_eq!(patch.hunks.len(), 2);
        apply_patch(&dir, &patch.hunk_patch(0).unwrap(), true, false).unwrap();
        let staged = run(&dir, &["show", ":f.txt"]).unwrap();
        assert!(staged.contains("LINE 2\r\n") && staged.contains("line 13\r\n"), "the index keeps the file's line endings: {staged:?}");
        // The second hunk is thrown away from the working file.
        apply_patch(&dir, &patch.hunk_patch(1).unwrap(), false, true).unwrap();
        let file = std::fs::read_to_string(dir.join("f.txt")).unwrap();
        assert!(file.contains("line 13\r\n") && file.contains("LINE 2\r\n"), "{file:?}");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn chosen_lines_are_staged_and_unstaged_leaving_the_rest_alone() {
        let Some(dir) = repo("lines") else { return };
        write(&dir, "f.txt", &numbered(10));
        commit_all(&dir, "base");
        write(&dir, "f.txt", &numbered(10).replace("line 5\n", "FIVE\n").replace("line 6\n", "SIX\n"));
        let patch = crate::patch::parse(&diff_text(&dir, "f.txt", false, false).unwrap()).unwrap();
        let find = |kind: crate::patch::LineKind, text: &str| patch.hunks[0].lines.iter().position(|l| l.kind == kind && l.text == text).unwrap();
        use crate::patch::LineKind::{Add, Del};
        // Stage only the 5 -> FIVE edit.
        let chosen = std::collections::BTreeSet::from([find(Del, "line 5"), find(Add, "FIVE")]);
        apply_patch(&dir, &patch.lines_patch(0, &chosen, false).unwrap(), true, false).unwrap();
        let index = index_text(&dir);
        assert!(index.contains("FIVE\n") && index.contains("line 6\n") && !index.contains("SIX"), "{index}");
        // Stage the rest, then unstage only the 6 -> SIX edit.
        stage(&dir, &["f.txt".to_owned()]).unwrap();
        let staged = crate::patch::parse(&diff_text(&dir, "f.txt", true, false).unwrap()).unwrap();
        let find = |kind: crate::patch::LineKind, text: &str| staged.hunks[0].lines.iter().position(|l| l.kind == kind && l.text == text).unwrap();
        let chosen = std::collections::BTreeSet::from([find(Del, "line 6"), find(Add, "SIX")]);
        apply_patch(&dir, &staged.lines_patch(0, &chosen, true).unwrap(), true, true).unwrap();
        let index = index_text(&dir);
        assert!(index.contains("FIVE\n") && index.contains("line 6\n") && !index.contains("SIX"), "{index}");
        assert!(std::fs::read_to_string(dir.join("f.txt")).unwrap().contains("SIX"), "the file itself is untouched");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_rebase_that_stops_for_conflicts_is_resolved_and_continued() {
        let Some(dir) = repo("rebase") else { return };
        git_ok(&dir, &["branch", "-M", "main"]);
        write(&dir, "a.txt", "base\n");
        commit_all(&dir, "base");
        git_ok(&dir, &["switch", "-q", "-c", "feature"]);
        write(&dir, "a.txt", "feature\n");
        commit_all(&dir, "feature change");
        git_ok(&dir, &["switch", "-q", "main"]);
        write(&dir, "a.txt", "main\n");
        commit_all(&dir, "main change");
        git_ok(&dir, &["switch", "-q", "feature"]);

        let error = rebase(&dir, "main").unwrap_err();
        assert!(error.contains("conflicts"), "{error}");
        assert!(matches!(repo_state(&dir), RepoState::Rebase { .. }), "{:?}", repo_state(&dir));
        let state = status(&dir).unwrap();
        assert!(state.changes.iter().any(|c| c.path == "a.txt" && c.conflicted));
        assert!(state.state.label().unwrap().starts_with("Rebase in progress"));

        let text = std::fs::read_to_string(dir.join("a.txt")).unwrap();
        assert_eq!(crate::conflict::find(&text).len(), 1);
        std::fs::write(dir.join("a.txt"), crate::conflict::resolve_all(&text, crate::conflict::Choice::Both)).unwrap();
        mark_resolved(&dir, "a.txt").unwrap();
        assert_eq!(rebase_continue(&dir).unwrap(), "Rebase finished");
        assert_eq!(repo_state(&dir), RepoState::Clean);
        assert_eq!(std::fs::read_to_string(dir.join("a.txt")).unwrap(), "main\nfeature\n");

        // Abort puts everything back.
        git_ok(&dir, &["reset", "-q", "--hard", "HEAD~1"]);
        git_ok(&dir, &["switch", "-q", "-c", "again", "main"]);
        write(&dir, "a.txt", "again\n");
        commit_all(&dir, "again change");
        git_ok(&dir, &["reset", "-q", "--hard", "HEAD"]);
        let error = merge(&dir, "feature");
        assert!(error.is_ok() || error.unwrap_err().contains("conflicts"));
        if repo_state(&dir) == RepoState::Merge {
            assert_eq!(merge_abort(&dir).unwrap(), "Merge aborted");
        }
        assert_eq!(repo_state(&dir), RepoState::Clean);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_leftover_daemon_holding_the_output_pipe_does_not_hang_a_finished_command() {
        let Some(dir) = repo("pipe") else { return };
        // The alias starts a background process that keeps git's output pipe open for a while, then git exits.
        let started = std::time::Instant::now();
        let result = run(&dir, &["-c", "alias.leave=!sleep 20 & echo started", "leave"]);
        assert_eq!(result.unwrap().trim(), "started");
        assert!(started.elapsed() < std::time::Duration::from_secs(10), "returned after {:?}, not after the 20 s the background process lives", started.elapsed());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_running_operation_can_be_cancelled() {
        let Some(dir) = repo("cancel") else { return };
        let job = JobControl::new();
        let (worker_job, worker_dir) = (Arc::clone(&job), dir.clone());
        let started = std::time::Instant::now();
        let worker = std::thread::spawn(move || with_job(&worker_job, || run(&worker_dir, &["-c", "alias.slow=!sleep 60", "slow"])));
        std::thread::sleep(std::time::Duration::from_millis(1500));
        job.cancel();
        assert_eq!(worker.join().unwrap(), Err("cancelled".to_owned()));
        assert!(started.elapsed() < std::time::Duration::from_secs(30), "the process was stopped, not waited for");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    fn subjects(dir: &Path) -> Vec<String> {
        log(dir, None, 50).unwrap().into_iter().map(|c| c.subject).collect()
    }

    #[test]
    fn a_commit_from_another_branch_is_cherry_picked_and_then_reverted() {
        let Some(dir) = repo("pick") else { return };
        write(&dir, "a.txt", "base\n");
        commit_all(&dir, "base");
        git_ok(&dir, &["switch", "-q", "-c", "feature"]);
        write(&dir, "b.txt", "feature file\n");
        commit_all(&dir, "add b");
        git_ok(&dir, &["switch", "-q", "main"]);
        let wanted = log_all(&dir, 10).unwrap().into_iter().find(|c| c.subject == "add b").expect("the commit is listed from the other branch");
        assert_eq!(cherry_pick(&dir, &wanted.hash).unwrap(), format!("Cherry-picked {}", short_hash(&wanted.hash)));
        assert!(dir.join("b.txt").is_file());
        assert_eq!(subjects(&dir)[0], "add b");
        // Reverting the new commit takes the file away again.
        let tip = log(&dir, None, 1).unwrap().remove(0);
        revert(&dir, &tip.hash).unwrap();
        assert!(!dir.join("b.txt").exists());
        assert!(subjects(&dir)[0].starts_with("Revert \"add b\""));
        // A cherry-pick that conflicts stops and can be aborted.
        git_ok(&dir, &["switch", "-q", "feature"]);
        write(&dir, "a.txt", "feature edit\n");
        commit_all(&dir, "edit a on feature");
        git_ok(&dir, &["switch", "-q", "main"]);
        write(&dir, "a.txt", "main edit\n");
        commit_all(&dir, "edit a on main");
        let conflicting = log_all(&dir, 10).unwrap().into_iter().find(|c| c.subject == "edit a on feature").unwrap();
        let error = cherry_pick(&dir, &conflicting.hash).unwrap_err();
        assert!(error.contains("conflicts"), "{error}");
        assert_eq!(repo_state(&dir), RepoState::CherryPick);
        assert_eq!(cherry_pick_abort(&dir).unwrap(), "Cherry-pick aborted");
        assert_eq!(repo_state(&dir), RepoState::Clean);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn an_interactive_rebase_reorders_rewords_squashes_and_drops() {
        let Some(dir) = repo("rebase-i") else { return };
        write(&dir, "base.txt", "base\n");
        commit_all(&dir, "base");
        for (file, subject) in [("one.txt", "add one"), ("two.txt", "add two"), ("three.txt", "add three"), ("four.txt", "add four"), ("temp.txt", "temporary")] {
            write(&dir, file, &format!("{file}\n"));
            commit_all(&dir, subject);
        }
        let base = log(&dir, None, 10).unwrap().last().unwrap().hash.clone();
        let mut items = rebase_commits(&dir, &base).unwrap();
        assert_eq!(items.iter().map(|i| i.subject.as_str()).collect::<Vec<_>>(), ["add one", "add two", "add three", "add four", "temporary"], "oldest first");
        // one, three, two (reworded), four folded into two, temporary dropped.
        let pick = |i: usize| items[i].clone();
        let (one, two, three, four, temp) = (pick(0), pick(1), pick(2), pick(3), pick(4));
        items = vec![
            one,
            three,
            TodoItem { action: TodoAction::Reword("add two, renamed".into()), ..two },
            TodoItem { action: TodoAction::Fixup, ..four },
            TodoItem { action: TodoAction::Drop, ..temp },
        ];
        assert_eq!(interactive_rebase(&dir, &base, &items).unwrap(), "Rebased 4 commits");
        assert_eq!(subjects(&dir), ["add two, renamed", "add three", "add one", "base"]);
        for kept in ["one.txt", "two.txt", "three.txt", "four.txt"] {
            assert!(dir.join(kept).is_file(), "{kept} is still in the tree (four was folded into two)");
        }
        assert!(!dir.join("temp.txt").exists(), "the dropped commit's file is gone");
        assert_eq!(repo_state(&dir), RepoState::Clean);

        // A squash keeps both messages in the joined one; the order of messages does not get mixed up with a reword.
        let base = log(&dir, None, 10).unwrap().last().unwrap().hash.clone();
        let mut items = rebase_commits(&dir, &base).unwrap();
        items[1].action = TodoAction::Squash; // "add three" into "add one"
        items[2].action = TodoAction::Reword("final subject".into());
        interactive_rebase(&dir, &base, &items).unwrap();
        assert_eq!(subjects(&dir), ["final subject", "add one", "base"]);
        let squashed = run(&dir, &["log", "-1", "--format=%B", "HEAD~1"]).unwrap();
        assert!(squashed.contains("add one") && squashed.contains("add three"), "{squashed}");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn rebase_todo_refuses_choices_git_could_not_do() {
        let item = |action: TodoAction| TodoItem { hash: "abc".into(), subject: "s".into(), action };
        assert!(rebase_todo(&[item(TodoAction::Squash)]).unwrap_err().contains("first one kept"));
        assert!(rebase_todo(&[item(TodoAction::Drop), item(TodoAction::Fixup)]).is_err(), "a fixup after only a drop has nothing above it");
        assert!(rebase_todo(&[item(TodoAction::Drop)]).unwrap_err().contains("nothing to rebase"));
        let todo = rebase_todo(&[item(TodoAction::Pick), item(TodoAction::Reword("m".into())), item(TodoAction::Squash), item(TodoAction::Drop)]).unwrap();
        assert_eq!(todo, "pick abc s\nreword abc s\nsquash abc s\ndrop abc s\n");
    }

    #[test]
    fn the_common_ancestor_of_a_conflict_is_read_without_touching_the_file() {
        let Some(dir) = repo("bases") else { return };
        write(&dir, "a.txt", "one\ntwo\nthree\n");
        commit_all(&dir, "base");
        git_ok(&dir, &["switch", "-q", "-c", "feature"]);
        write(&dir, "a.txt", "one\ntwo FEATURE\nthree\n");
        commit_all(&dir, "feature");
        git_ok(&dir, &["switch", "-q", "main"]);
        write(&dir, "a.txt", "one\ntwo MAIN\nthree\n");
        commit_all(&dir, "main");
        assert!(merge(&dir, "feature").unwrap_err().contains("conflicts"));
        let before = std::fs::read_to_string(dir.join("a.txt")).unwrap();
        let bases = conflict_bases(&dir, "a.txt").unwrap();
        assert_eq!(bases, vec![Some(vec!["two\n".to_owned()])], "the line as it was before either side changed it");
        assert_eq!(std::fs::read_to_string(dir.join("a.txt")).unwrap(), before);
        assert_eq!(crate::conflict::find(&before).len(), bases.len(), "the file's conflicts line up with the ancestors");
        assert_eq!(merge_abort(&dir).unwrap(), "Merge aborted");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn some_lines_of_a_new_file_are_staged_and_the_rest_stays_a_working_change() {
        let Some(dir) = repo("newfile") else { return };
        write(&dir, "seed.txt", "x\n");
        commit_all(&dir, "seed");
        write(&dir, "new.txt", "one\ntwo\nthree\nfour\n");
        let patch = crate::patch::parse(&diff_text(&dir, "new.txt", false, true).unwrap()).unwrap();
        let chosen = std::collections::BTreeSet::from([0usize, 2]);
        let content = patch.new_file_text(&chosen, false).unwrap();
        assert_eq!(content, "one\nthree\n");
        stage_new_file(&dir, "new.txt", &content).unwrap();
        assert_eq!(run(&dir, &["show", ":new.txt"]).unwrap(), "one\nthree\n", "only the chosen lines are in the index");
        assert_eq!(std::fs::read_to_string(dir.join("new.txt")).unwrap(), "one\ntwo\nthree\nfour\n", "the file itself is untouched");
        let s = status(&dir).unwrap();
        let c = s.changes.iter().find(|c| c.path == "new.txt").unwrap();
        assert!(c.index == Some(Kind::Added) && c.worktree == Some(Kind::Modified), "{c:?}");
        let rest = crate::patch::parse(&diff_text(&dir, "new.txt", false, false).unwrap()).unwrap();
        assert_eq!(rest.hunks[0].lines.iter().filter(|l| l.kind == crate::patch::LineKind::Add).map(|l| l.text.as_str()).collect::<Vec<_>>(), ["two", "four"]);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn submodule_and_worktree_listings_parse() {
        let subs = parse_submodules(" 1a2b3c4d5e6f7a8b9c0d1e2f3a4b5c6d7e8f9a0b libs/one (v1.2-3-gabc)\n-ffffffffffffffffffffffffffffffffffffffff libs/two\n+0123456789012345678901234567890123456789 libs/three (heads/main)\n");
        assert_eq!(subs.len(), 3);
        assert_eq!((subs[0].path.as_str(), subs[0].state, subs[0].describe.as_str()), ("libs/one", ' ', "v1.2-3-gabc"));
        assert_eq!((subs[1].path.as_str(), subs[1].state_label()), ("libs/two", "not initialised"));
        assert_eq!(subs[2].state_label(), "at another commit than recorded");
        let trees = parse_worktrees("worktree C:/repo\nHEAD 0123456789abcdef\nbranch refs/heads/main\n\nworktree C:/repo-feature\nHEAD fedcba9876543210\nbranch refs/heads/feature\nlocked busy\n\nworktree C:/repo-detached\nHEAD 1111111111111111\ndetached\nprunable gitdir file points to non-existent location\n");
        assert_eq!(trees.len(), 3);
        assert_eq!((trees[0].branch.as_deref(), trees[0].head.as_str()), (Some("main"), "01234567"));
        assert!(trees[1].locked && trees[1].branch.as_deref() == Some("feature"));
        assert!(trees[2].branch.is_none() && trees[2].prunable);
    }

    #[test]
    fn a_worktree_is_added_listed_and_removed() {
        let Some(dir) = repo("worktree") else { return };
        write(&dir, "a.txt", "a\n");
        commit_all(&dir, "base");
        let other = std::env::temp_dir().join(format!("forge-git-worktree-other-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&other);
        let other_text = other.to_string_lossy().replace('\\', "/");
        worktree_add(&dir, &other_text, "side", true).unwrap();
        let trees = worktrees(&dir).unwrap();
        assert_eq!(trees.len(), 2, "{trees:?}");
        assert!(trees.iter().any(|t| t.branch.as_deref() == Some("side") && t.path.replace('\\', "/").ends_with(other_text.rsplit('/').next().unwrap())));
        assert!(other.join("a.txt").is_file());
        worktree_remove(&dir, &other_text).unwrap();
        assert_eq!(worktrees(&dir).unwrap().len(), 1);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn the_graph_log_lists_parents_and_refs() {
        let Some(dir) = repo("graphlog") else { return };
        write(&dir, "a.txt", "a\n");
        commit_all(&dir, "base");
        git_ok(&dir, &["switch", "-q", "-c", "feature"]);
        write(&dir, "b.txt", "b\n");
        commit_all(&dir, "on feature");
        git_ok(&dir, &["switch", "-q", "main"]);
        write(&dir, "c.txt", "c\n");
        commit_all(&dir, "on main");
        git_ok(&dir, &["merge", "-q", "--no-ff", "-m", "merge feature", "feature"]);
        let commits = graph_log(&dir, 20).unwrap();
        assert_eq!(commits.len(), 4);
        let merge = &commits[0];
        assert_eq!(merge.subject, "merge feature");
        assert_eq!(merge.parents.len(), 2);
        assert!(merge.refs.iter().any(|r| r.contains("main")), "{:?}", merge.refs);
        let rows = crate::graph::layout(&commits);
        assert_eq!(rows[0].diverge.len(), 1, "the merge opens a lane for its second parent");
        assert!(rows.iter().map(|r| r.width()).max().unwrap() >= 2);
        std::fs::remove_dir_all(&dir).unwrap();
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
