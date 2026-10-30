//! The editor's state and the logic that keeps its parts consistent. Input handling is
//! in `input_handling.rs`, commands in `command_impl.rs`, drawing in `render.rs`.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::Arc;
use std::time::{Duration, Instant};

use forge_core::commands::{Command, Keymap};
use forge_core::find::FindState;
use forge_core::git::{Blame, Branch, Commit, Status};
use forge_core::gutter::LineChange;
use forge_core::folds::Folds;
use forge_core::input::LineInput;
use forge_core::project_search::Hit;
use forge_core::settings::{config_dir, Settings};
use forge_core::terminal::Terminal;
use forge_core::theme::Theme;
use forge_lsp::{
    parse_code_actions, parse_completion, parse_document_symbols, parse_hover, parse_locations,
    parse_signature_help, parse_text_edits, parse_workspace_edit, path_to_uri, text_document_position,
    CodeAction, CompletionItem, ContentChange, Diagnostic, Location, Position, Range as LspRange, SignatureHelp,
    Symbol, TextEditSpec,
};
use forge_syntax::{Language, Syntax};
use forge_text::{Buffer, IndentStyle, Selection, SelectionSet, TextEdit};
use gpui::{px, App, Context, FocusHandle, Focusable, Pixels, SharedString, UniformListScrollHandle};

use crate::completion::{filter_items, identifier_start, CompletionState};
use crate::lsp::{Event, LspManager, RequestTag};
use crate::tree::FileTree;

/// How often background work (language servers, terminals, searches) is drained.
pub(crate) const TICK: Duration = Duration::from_millis(50);
/// Files larger than this are refused instead of stalling the UI thread on load.
pub(crate) const MAX_FILE_BYTES: u64 = 64 * 1024 * 1024;
/// How long the pointer must rest on text before hover information is requested.
pub(crate) const HOVER_DELAY: Duration = Duration::from_millis(500);
pub(crate) const TAB_BAR_HEIGHT: f32 = 34.0;
pub(crate) const BREADCRUMB_HEIGHT: f32 = 22.0;
pub(crate) const LINE_NUMBER_WIDTH: f32 = 46.0;
pub(crate) const FOLD_WIDTH: f32 = 16.0;
/// The margin left of the line numbers: breakpoints and the current debug line.
pub(crate) const GLYPH_WIDTH: f32 = 18.0;
pub(crate) const GUTTER_WIDTH: f32 = GLYPH_WIDTH + LINE_NUMBER_WIDTH + FOLD_WIDTH;
pub(crate) const TEXT_PAD: f32 = 8.0;
pub(crate) const FONT: &str = "Consolas";

pub(crate) fn same_path(a: &Path, b: &Path) -> bool {
    if cfg!(windows) {
        let norm = |p: &Path| p.to_string_lossy().replace('/', "\\");
        norm(a).eq_ignore_ascii_case(&norm(b))
    } else {
        a == b
    }
}

// ---- documents and views ----

/// A file's text and everything derived from it, shared by every view of the file.
pub(crate) struct Doc {
    pub path: PathBuf,
    pub buffer: Buffer,
    pub dirty: bool,
    pub syntax: Option<Syntax>,
    pub language: Option<Language>,
    pub lsp_open: bool,
    pub lsp_version: i32,
    /// Language-server and extension diagnostics together, which is what everything reads.
    pub diagnostics: Vec<Diagnostic>,
    pub lsp_diagnostics: Vec<Diagnostic>,
    pub ext_diagnostics: Vec<Diagnostic>,
    /// Bumped on every edit; caches compare against it.
    pub revision: u64,
    /// Generated content (a diff, a commit) that cannot be edited or saved.
    pub readonly: bool,
    /// Tab title for generated documents.
    pub title: Option<String>,
    /// A new document that has no file yet (`Untitled-1`): Save asks where to put it.
    pub untitled: bool,
    /// The language was picked by the user, so it is not guessed again from the name or the contents.
    pub lang_manual: bool,
    /// Colors lines by their unified-diff prefix.
    pub diff: bool,
    /// Lines changed relative to HEAD, and the `(revision, git epoch)` they were computed for.
    pub gutter: Vec<LineChange>,
    pub gutter_key: (u64, u64),
    /// Bracket nesting depth at the start of each line, and the `revision` it was worked out for.
    pub bracket_depths: std::cell::RefCell<(u64, std::rc::Rc<Vec<i32>>)>,
    /// Width in cells of the longest line, and the `revision` it was measured for (the sideways scroll range).
    pub widest: std::cell::Cell<(u64, usize)>,
    /// Syntax colours of a stretch of lines (`revision`, first line, spans per line), so building a frame, which asks
    /// for the visible lines several times, and scrolling a few lines, colour them once.
    pub span_cache: std::cell::RefCell<(u64, usize, Vec<Vec<forge_syntax::Span>>)>,
    /// The file began with a UTF-8 byte-order mark. It is kept out of the text (language
    /// servers choke on it) and written back on save.
    pub bom: bool,
    /// Set for files of a language an extension contributes (no tree-sitter grammar).
    pub ext_lang: Option<forge_core::ext_lang::ExtLanguage>,
    /// That language's TextMate tokenizer, when its extension ships a grammar.
    pub tm: Option<forge_syntax::TextMateSyntax>,
    /// The syntax tree has not been built yet; [`Doc::ensure_parsed`] does it.
    pub parse_pending: bool,
    /// Set when the tab shows an extension's page instead of text.
    pub ext_page: Option<Box<ExtDetail>>,
}

/// Decode file contents, splitting a leading UTF-8 byte-order mark off the text.
pub(crate) fn decode_utf8(bytes: Vec<u8>) -> Option<(String, bool)> {
    let mut text = String::from_utf8(bytes).ok()?;
    let bom = text.starts_with('\u{FEFF}');
    if bom {
        text.drain(..'\u{FEFF}'.len_utf8());
    }
    Some((text, bom))
}

impl Doc {
    pub fn new(path: PathBuf, text: &str) -> Self {
        Self::with_parse(path, text, true)
    }

    /// A document whose syntax tree is built when it is first shown (see [`Self::ensure_parsed`]): the tabs restored
    /// at startup that are not in front do not hold up the first window.
    pub fn new_unparsed(path: PathBuf, text: &str) -> Self {
        Self::with_parse(path, text, false)
    }

    fn with_parse(path: PathBuf, text: &str, parse_now: bool) -> Self {
        let buffer = Buffer::new(text);
        let language = Language::from_path(&path);
        let syntax = language.and_then(Syntax::new).map(|mut syntax| {
            if parse_now {
                syntax.parse(buffer.rope(), &[]);
            }
            syntax
        });
        Self {
            parse_pending: !parse_now && syntax.is_some(),
            path,
            buffer,
            dirty: false,
            syntax,
            language,
            lsp_open: false,
            lsp_version: 1,
            diagnostics: Vec::new(),
            lsp_diagnostics: Vec::new(),
            ext_diagnostics: Vec::new(),
            revision: 0,
            readonly: false,
            title: None,
            untitled: false,
            lang_manual: false,
            diff: false,
            gutter: Vec::new(),
            gutter_key: (u64::MAX, 0),
            bracket_depths: std::cell::RefCell::new((u64::MAX, std::rc::Rc::new(Vec::new()))),
            widest: std::cell::Cell::new((u64::MAX, 0)),
            span_cache: std::cell::RefCell::new((u64::MAX, 0, Vec::new())),
            bom: false,
            ext_lang: None,
            tm: None,
            ext_page: None,
        }
    }

    /// Build the syntax tree now if it was put off.
    pub fn ensure_parsed(&mut self) {
        if std::mem::take(&mut self.parse_pending) {
            if let Some(syntax) = self.syntax.as_mut() {
                syntax.parse(self.buffer.rope(), &[]);
            }
        }
    }

    /// The coloured runs of lines `range`, from the cache when it holds them. When the text has not changed, a margin
    /// of lines around the request is coloured too, so scrolling a little does not start over; right after an edit
    /// only what is asked for is coloured, since that is what the next keystroke will throw away.
    pub fn highlight_cached(&self, range: std::ops::Range<usize>) -> Vec<Vec<forge_syntax::Span>> {
        const MARGIN: usize = 40;
        let mut cache = self.span_cache.borrow_mut();
        let (revision, first, lines) = &*cache;
        if *revision != self.revision || range.start < *first || range.end > *first + lines.len() {
            let total = self.buffer.len_lines();
            let margin = if *revision == self.revision { MARGIN } else { 0 };
            let wanted = range.start.saturating_sub(margin)..(range.end + margin).min(total);
            let spans = match (self.syntax.as_ref(), self.tm.as_ref()) {
                (Some(s), _) => s.highlight_lines(self.buffer.rope(), wanted.clone()),
                (None, Some(t)) => t.highlight_lines(self.buffer.rope(), wanted.clone()),
                (None, None) => Vec::new(),
            };
            *cache = (self.revision, wanted.start, spans);
        }
        let (_, first, lines) = &*cache;
        lines.iter().skip(range.start.saturating_sub(*first)).take(range.len()).cloned().collect()
    }

    /// Display width in cells of the longest line (tabs expanded), kept until the text changes.
    pub fn widest_line_cells(&self) -> usize {
        let (rev, cells) = self.widest.get();
        if rev == self.revision {
            return cells;
        }
        let rope = self.buffer.rope();
        let widest = rope
            .lines()
            .map(|line| {
                let text = line.to_string();
                let text = text.trim_end_matches(['\n', '\r']);
                forge_text::columns::display_width(text, text.chars().count())
            })
            .max()
            .unwrap_or(0);
        self.widest.set((self.revision, widest));
        widest
    }

    /// Not worth colouring: a page, not text.
    pub fn readonly_text_only(&self) -> bool {
        self.ext_page.is_some()
    }

    /// Bracket nesting at the start of every line, kept until the text changes. `None` for very large files.
    pub fn bracket_depths(&self) -> Option<std::rc::Rc<Vec<i32>>> {
        if self.buffer.len_chars() > 2_000_000 {
            return None;
        }
        let mut cache = self.bracket_depths.borrow_mut();
        if cache.0 != self.revision || cache.1.len() < self.buffer.len_lines() {
            let depths = forge_text::brackets::depths_at_line_starts(self.buffer.rope(), self.language.and_then(|l| l.line_comment()), self.language == Some(Language::Rust));
            *cache = (self.revision, std::rc::Rc::new(depths));
        }
        Some(cache.1.clone())
    }

    /// The tab for an extension's page. It has no text; the tab is named after the extension.
    pub fn new_ext_page(detail: ExtDetail) -> Self {
        let path = PathBuf::from(format!("ext:{}", detail.id.to_lowercase()));
        let mut doc = Self::new_virtual(path, &detail.name, "", false);
        detail.sync_list();
        doc.ext_page = Some(Box::new(detail));
        doc
    }

    /// Use an extension's language for this file: its name, comment tokens and grammar.
    pub fn use_ext_language(&mut self, lang: &forge_core::ext_lang::ExtLanguage, registry: &forge_syntax::TextMateRegistry) {
        self.tm = lang.grammar.as_deref().and_then(|g| registry.syntax(g));
        self.ext_lang = Some(lang.clone());
    }

    /// A read-only generated document (a diff or commit). Never gets syntax or a server.
    pub fn new_virtual(path: PathBuf, title: &str, text: &str, diff: bool) -> Self {
        let mut doc = Self::new(PathBuf::from("virtual"), text);
        doc.path = path;
        doc.language = None;
        doc.syntax = None;
        doc.ext_lang = None;
        doc.tm = None;
        doc.readonly = true;
        doc.title = Some(title.to_owned());
        doc.diff = diff;
        doc
    }

    /// Give the document another language: its syntax tree (with the languages embedded in it) is built again.
    pub fn set_language(&mut self, language: Option<Language>) {
        self.language = language;
        self.ext_lang = None;
        self.tm = None;
        self.syntax = language.and_then(Syntax::new);
        if let Some(syntax) = self.syntax.as_mut() {
            syntax.parse(self.buffer.rope(), &[]);
        }
        self.parse_pending = false;
        // Colours, bracket depths and the like are cached per revision.
        self.revision += 1;
    }

    /// An empty `Untitled-<n>` document. Its path is only a unique key, not a place on disk.
    pub fn new_untitled(n: usize) -> Self {
        let mut doc = Self::new(PathBuf::from(format!("untitled:Untitled-{n}")), "");
        doc.title = Some(format!("Untitled-{n}"));
        doc.untitled = true;
        doc
    }

    pub fn name(&self) -> String {
        self.title.clone().unwrap_or_else(|| self.path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default())
    }

    /// Feed pending edits to the syntax tree and return them. Edits are drained even
    /// without a grammar so the buffer's log cannot grow unbounded.
    pub fn sync_syntax(&mut self) -> Vec<TextEdit> {
        let edits = self.buffer.take_edits();
        if let (Some(syntax), false) = (self.syntax.as_mut(), edits.is_empty()) {
            syntax.parse(self.buffer.rope(), &edits);
        }
        if let (Some(tm), false) = (self.tm.as_mut(), edits.is_empty()) {
            tm.parse(self.buffer.rope(), &edits);
        }
        edits
    }

    pub fn set_lsp_diagnostics(&mut self, diagnostics: Vec<Diagnostic>) {
        self.lsp_diagnostics = diagnostics;
        self.rebuild_diagnostics();
    }

    pub fn rebuild_diagnostics(&mut self) {
        self.diagnostics = self.lsp_diagnostics.iter().chain(self.ext_diagnostics.iter()).cloned().collect();
    }

    pub fn diagnostics_on(&self, line: usize) -> impl Iterator<Item = &Diagnostic> {
        self.diagnostics.iter().filter(move |d| d.severity != forge_lsp::Severity::Hint && d.range.start.line <= line && line <= d.range.end.line)
    }
}

/// One open tab: a document plus this tab's own cursors, scroll and folds.
pub(crate) struct View {
    pub doc: usize,
    /// A pinned tab sits at the left, shows a pin instead of a close button, and survives "Close Others / All".
    pub pinned: bool,
    /// A replaceable tab (shown in italics): the next single-click open takes its place until it is edited or double-clicked.
    pub preview: bool,
    pub selections: SelectionSet,
    /// Column kept across vertical moves through short lines.
    pub goal: Option<usize>,
    /// Earlier selections for Cursor Undo (Ctrl+U).
    pub history: Vec<SelectionSet>,
    /// Selections before each Expand Selection, for Shrink.
    pub expand: Vec<SelectionSet>,
    pub scroll: UniformListScrollHandle,
    pub folds: Folds,
    pub folds_revision: u64,
    pub folds_lines: usize,
    /// Which displayed row shows which part of which line, while word wrap is on.
    pub wrap: Option<crate::wrap::WrapMap>,
    /// The minimap's per-line bars, with the document revision they were built for.
    pub minimap: Option<(u64, crate::minimap::LineStats)>,
    /// Lines `(first, last before, last after)` of the edits made since the minimap was built, so a keystroke
    /// updates only the lines it touched.
    pub minimap_pending: Vec<(usize, usize, usize)>,
    /// How far the text is scrolled sideways, in pixels (always 0 with word wrap on).
    pub scroll_x: f32,
}

impl View {
    pub fn new(doc: usize) -> Self {
        Self {
            doc,
            pinned: false,
            preview: false,
            selections: SelectionSet::single(Selection::caret(0)),
            goal: None,
            history: Vec::new(),
            expand: Vec::new(),
            scroll: UniformListScrollHandle::new(),
            folds: Folds::default(),
            folds_revision: u64::MAX,
            folds_lines: 0,
            wrap: None,
            minimap: None,
            minimap_pending: Vec::new(),
            scroll_x: 0.0,
        }
    }
}

pub(crate) struct Group {
    pub views: Vec<View>,
    pub active: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Focus {
    Editor,
    Explorer,
    Find,
    Search,
    Scm,
    Extensions,
    Terminal,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SidebarView {
    Explorer,
    Search,
    Git,
    Extensions,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PanelView {
    Terminal,
    Problems,
    References,
    Debug,
    Graph,
    Timeline,
}

// ---- overlays (palette, quick open, prompts) ----

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum OverlayKind {
    Palette,
    QuickOpen,
    GoToLine,
    Symbols,
    /// Folders opened before.
    OpenRecent,
    /// Pick the language whose snippets file to open (Configure User Snippets).
    SnippetFiles,
    /// Change the language mode of the open file (the status bar's language).
    Language,
    /// The second file of "Compare Active File With...".
    CompareWith,
    Theme,
    IconTheme,
    ProductIconTheme,
    CodeActions,
    /// Several definitions for one symbol (trait impls, overloads): pick where to go.
    Definitions,
    /// The user's OpenAI API key, typed here and kept in memory only.
    AiKey,
    Rename,
    NewEntry { dir: PathBuf, folder: bool },
    RenameEntry(PathBuf),
    ConfirmDelete(PathBuf),
    /// A file or folder was dragged onto another folder in the Explorer.
    ConfirmMove { from: PathBuf, dir: PathBuf },
    GitCheckout,
    GitRebase,
    GitMerge,
    /// Pick the commit an interactive rebase starts after.
    GitRebaseFrom,
    GitCherryPick,
    GitRevert,
    GitSubmodules,
    GitWorktrees,
    GitWorktreeAdd,
    GitWorktreeRemove,
    /// Press Enter to throw away a hunk or some lines of the working file: the patch that undoes them.
    ConfirmDiscardHunk { rel: String, patch: String, what: String },
    /// A question from git (a username, password or passphrase), to be answered for the running command.
    GitCredential(String),
    GitCreateBranch,
    GitHistory,
    ConfirmDiscard { rel: String, untracked: bool },
    /// Replace every search result: how many matches in how many files.
    ConfirmReplace { matches: usize, files: usize },
    LanguageServers,
    /// Search text for Open VSX.
    ExtSearch,
    /// Path of a .vsix file.
    ExtVsix,
    ExtResults,
    ExtInstalled,
    Snippets,
}

/// What the language picker can set a file to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum LangChoice {
    /// Work it out from the file name, else from the contents.
    Auto,
    PlainText,
    Lang(Language),
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum ItemAction {
    Command(Command),
    File(PathBuf),
    Theme(String),
    IconTheme(String),
    ProductIconTheme(String),
    CodeAction(usize),
    Definition(usize),
    Symbol(usize, usize),
    GitBranch(usize),
    GitRebaseOnto(usize),
    GitMergeBranch(usize),
    GitRebaseFrom(usize),
    GitCherryPickCommit(usize),
    GitRevertCommit(usize),
    /// Open this folder (a submodule or worktree) in a new Forge window.
    OpenFolder(std::path::PathBuf),
    /// Open this folder in this window (Open Recent).
    OpenRecent(std::path::PathBuf),
    CompareWith(std::path::PathBuf),
    /// Open (and create) the user's snippets file for this language id, or `global`.
    SnippetFile(String),
    SetLanguage(LangChoice),
    WorktreeAddBranch(usize),
    WorktreeRemove(usize),
    GitCommit(usize),
    Server(usize),
    ServersInstallAll,
    ExtListing(usize),
    ExtInstalled(usize),
    Snippet(usize),
    ExtCommand(String),
}

#[derive(Clone, Debug)]
pub(crate) struct Item {
    pub label: String,
    pub detail: String,
    pub positions: Vec<usize>,
    pub action: ItemAction,
}

pub(crate) struct Overlay {
    pub kind: OverlayKind,
    pub input: LineInput,
    pub items: Vec<Item>,
    pub selected: usize,
    pub placeholder: String,
    pub note: Option<String>,
}

pub(crate) struct SearchPanel {
    pub query: LineInput,
    /// The replacement text, and whether its row is shown and has the keyboard.
    pub replace: LineInput,
    pub replace_open: bool,
    pub replace_focused: bool,
    pub options: forge_text::search::SearchOptions,
    pub hits: Vec<Hit>,
    pub running: bool,
    pub capped: bool,
    pub error: Option<String>,
    pub rx: Option<Receiver<crate::input_handling::SearchMsg>>,
    pub cancel: Arc<AtomicBool>,
    pub scroll: UniformListScrollHandle,
    pub selected: usize,
    /// "Find in Folder": search only here instead of the whole project.
    pub scope: Option<PathBuf>,
}

/// The Extensions view: the search box and what Open VSX answered.
pub(crate) struct ExtPanel {
    pub query: LineInput,
    pub results: Vec<forge_ext_host::openvsx::Listing>,
    /// A search has finished (or failed) at least once, so opening the view does not search again.
    pub searched: bool,
    /// A search started from this view is running.
    pub waiting: bool,
    pub error: Option<String>,
    /// The query the current results answer.
    pub shown_query: String,
}

/// An extension's README, as it is loaded.
pub(crate) enum Readme {
    Loading,
    Ready(Vec<forge_core::markdown::Block>),
    Missing(String),
}

/// The page shown in a tab when an extension is clicked in the Extensions view.
pub(crate) struct ExtDetail {
    pub id: String,
    pub name: String,
    pub publisher: String,
    pub version: String,
    pub description: String,
    /// What Forge can use from it, for an installed extension.
    pub summary: Option<String>,
    /// Present when it came from Open VSX, so it can be installed from the page.
    pub listing: Option<forge_ext_host::openvsx::Listing>,
    pub info: forge_ext_host::openvsx::Details,
    pub readme: Readme,
    /// The folder links in the README are relative to (the extension's own, when installed).
    pub base: PathBuf,
    /// Lays out only the rows on screen: row 0 is the header, then one row per README block, then a spacer.
    pub list: gpui::ListState,
}

impl ExtDetail {
    pub fn blank_list() -> gpui::ListState {
        gpui::ListState::new(0, gpui::ListAlignment::Top, px(600.0))
    }

    pub fn item_count(&self) -> usize {
        2 + match &self.readme {
            Readme::Ready(blocks) => blocks.len(),
            _ => 1,
        }
    }

    /// Tell the list how many rows there are; call after the README changes.
    pub fn sync_list(&self) {
        self.list.reset(self.item_count());
    }
}

/// One row of the results list: a file header, or one line holding hits `first..end`
/// (several matches on a line share a row).
pub(crate) enum SearchRow {
    File { first: usize, count: usize },
    Line { first: usize, end: usize },
}

impl SearchPanel {
    /// The results grouped by file, then by line. Hits arrive one file at a time, so they are contiguous.
    pub fn rows(&self) -> Vec<SearchRow> {
        let hits = &self.hits;
        let mut rows = Vec::new();
        let mut i = 0;
        while i < hits.len() {
            let file_end = (i..hits.len()).find(|&j| hits[j].path != hits[i].path).unwrap_or(hits.len());
            rows.push(SearchRow::File { first: i, count: file_end - i });
            let mut j = i;
            while j < file_end {
                let end = (j..file_end).find(|&k| hits[k].line != hits[j].line).unwrap_or(file_end);
                rows.push(SearchRow::Line { first: j, end });
                j = end;
            }
            i = file_end;
        }
        rows
    }
}

pub(crate) struct RefEntry {
    pub location: Location,
    pub preview: String,
}

pub(crate) struct HoverInfo {
    pub text: String,
    pub line: usize,
    pub col: usize,
}

/// Results of git work done on background threads.
pub(crate) enum GitMsg {
    /// The repository found around a folder (none if it is not in one, or git is missing).
    Repo { folder: PathBuf, repo: Option<PathBuf> },
    Status(Result<Status, String>),
    Head(PathBuf, Option<String>),
    Done { label: String, result: Result<String, String> },
    Diff { key: String, title: String, result: Result<String, String> },
    /// New text for a diff tab that is already open (after staging).
    DiffRefresh { key: String, title: String, result: Result<String, String> },
    PickCommits(Result<Vec<Commit>, String>),
    Submodules(Result<Vec<forge_core::git::Submodule>, String>),
    Worktrees(Result<Vec<forge_core::git::Worktree>, String>),
    /// The commits an interactive rebase from `base` would replay.
    RebasePlan { base: String, label: String, result: Result<Vec<forge_core::git::TodoItem>, String> },
    /// The common-ancestor text of each conflict in `rel`.
    ConflictBases { rel: String, result: Result<Vec<Option<Vec<String>>>, String> },
    Graph(Result<Vec<forge_core::git::GraphCommit>, String>),
    Timeline { rel: String, result: Result<Vec<Commit>, String> },
    Branches(Result<Vec<Branch>, String>),
    History(Result<Vec<Commit>, String>),
    Blame(Result<Blame, String>),
    /// A commit subject written by the model, or why it could not be.
    AiMessage(Result<String, String>),
}

/// What a status message is about.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum StatusKind {
    Info,
    Success,
    Error,
}

pub(crate) struct GitState {
    /// Stops the running operation.
    pub cancel: Option<Arc<forge_core::git::JobControl>>,
    pub root: PathBuf,
    pub status: Status,
    /// A status request is in flight.
    pub refreshing: bool,
    pub refresh_wanted: bool,
    pub last_refresh: Instant,
    /// File contents at HEAD by absolute path; `None` = not in HEAD (new or untracked).
    pub head: HashMap<PathBuf, Option<String>>,
    pub head_pending: HashSet<PathBuf>,
    /// Bumped whenever the HEAD cache changes so gutters recompute.
    pub epoch: u64,
    /// Letter shown beside a path in the explorer; folders get a dot.
    pub decor: HashMap<PathBuf, char>,
    /// Name of an operation running in the background ("Pulling"...), and when it started.
    pub job: Option<String>,
    pub job_started: Option<Instant>,
}

pub(crate) struct Workspace {
    pub tree: FileTree,
    pub docs: Vec<Doc>,
    pub groups: Vec<Group>,
    pub focused_group: usize,
    pub focus_handle: FocusHandle,
    pub focus: Focus,

    pub settings: Settings,
    pub theme: Theme,
    /// The extension icon theme drawing file icons; `None` means Forge's own letter badges.
    pub icon_theme: Option<forge_core::icon_theme::IconTheme>,
    /// The product icon theme must be (re)loaded before the next frame: fonts need the app context.
    pub product_dirty: bool,
    pub keymap: Keymap,
    /// First keys of a multi-step chord already pressed (e.g. `ctrl+k`).
    pub pending_chord: String,
    pub status: SharedString,
    /// Whether the status message reports success or failure of something the user did, and when it was set.
    pub status_kind: StatusKind,
    pub status_at: Instant,
    pub char_width: Pixels,
    /// The font of code, the terminal and diffs (monospace), and of menus, tabs and the side bar (sans).
    pub code_font: SharedString,
    pub ui_font: SharedString,
    pub clipboard_lines: Option<String>,
    pub last_typing: Option<Instant>,
    pub close_armed: Option<(usize, usize)>,

    pub sidebar_visible: bool,
    pub sidebar_view: SidebarView,
    pub panel_visible: bool,
    pub panel_view: PanelView,
    pub explorer_scroll: UniformListScrollHandle,
    /// The tab strip of each editor group scrolls sideways; with the `(active tab, tab count)` it was last
    /// scrolled for, so the active tab is brought into view when it changes and the strip is left alone otherwise.
    pub tab_scrolls: std::cell::RefCell<Vec<(gpui::ScrollHandle, (usize, usize))>>,
    /// Sticky-scroll lines with their syntax colours, kept while the text and theme stay as they were,
    /// so the pinned lines are not highlighted again on every frame.
    pub sticky_cache: std::cell::RefCell<crate::render::StickyCache>,
    pub explorer_selected: Option<usize>,

    pub lsp: LspManager,
    pub lsp_progress: Option<String>,
    pub completion: Option<CompletionState>,
    pub completion_request: Option<u64>,
    pub signature: Option<SignatureHelp>,
    pub hover: Option<HoverInfo>,
    pub hover_request: Option<(u64, usize, usize)>,
    pub next_request: u64,
    pub mouse: Option<gpui::Point<Pixels>>,
    pub mouse_moved: Instant,
    pub symbols: Vec<Symbol>,
    pub code_actions: Vec<CodeAction>,
    pub references: Vec<RefEntry>,
    /// The candidates offered by the Definitions picker.
    pub definitions: Vec<RefEntry>,
    pub reference_selected: usize,
    pub problem_selected: usize,
    pub problems_scroll: UniformListScrollHandle,
    pub references_scroll: UniformListScrollHandle,

    pub find: FindState,
    pub find_revision: (usize, u64),
    pub overlay: Option<Overlay>,
    pub context_menu: Option<crate::context_menu::ContextMenu>,
    pub search: SearchPanel,
    pub file_index: Option<Vec<PathBuf>>,
    pub file_index_rx: Option<Receiver<Vec<PathBuf>>>,
    pub recent_files: Vec<PathBuf>,
    /// Tabs closed this session (newest last) with their caret, for Reopen Closed Editor.
    pub closed_tabs: Vec<(PathBuf, usize)>,
    /// Where the caret was before each jump (Go Back / Go Forward), as `(file, offset)`.
    pub nav_back: Vec<(PathBuf, usize)>,
    pub nav_forward: Vec<(PathBuf, usize)>,
    /// Set while Go Back / Forward move, so they do not record themselves.
    pub navigating: bool,
    /// Files cut or copied in the Explorer: the path, and whether it moves (cut) when pasted.
    pub explorer_clipboard: Option<(PathBuf, bool)>,
    /// Exit was chosen once with unsaved changes; choosing it again quits.
    pub quit_armed: bool,
    /// Sidebar and panel visibility from before Zen mode; `Some` while it is on.
    pub zen_saved: Option<(bool, bool)>,
    /// `(document, revision, since)` of the unsaved edit waiting for `files.autoSave: afterDelay`.
    pub auto_save_since: Option<(usize, u64, Instant)>,
    /// The file picked by "Compare Active File With...".
    pub compare_base: Option<PathBuf>,
    /// Folders opened before, for Open Recent.
    pub recent_folders: Vec<PathBuf>,
    pub recent_commands: Vec<Command>,

    pub terminals: Vec<Terminal>,
    pub active_terminal: usize,
    pub drag: Option<crate::input_handling::Drag>,

    pub git: Option<GitState>,
    pub git_tx: Sender<GitMsg>,
    pub git_rx: Receiver<GitMsg>,
    pub scm_message: forge_core::textarea::TextArea,
    /// A mouse drag is selecting text in the commit message box.
    pub scm_dragging: bool,
    /// A drag on the minimap is scrolling the editor.
    pub minimap_dragging: bool,
    /// The Markdown preview is open beside the focused editor, and the last parse of the document.
    /// Panel sizes the user can drag: the sidebar's width (at the default zoom), the bottom panel's
    /// height, the preview's width (None = half), the shares of split editor groups and of the Debug
    /// panel's three columns, the window size last drawn, and the drag in progress.
    pub sidebar_base: f32,
    pub panel_height: f32,
    pub preview_px: Option<f32>,
    pub group_fractions: Vec<f32>,
    pub debug_cols: [f32; 3],
    pub viewport: (f32, f32),
    pub resize: Option<crate::resize::Drag>,
    /// The session (open files) is saved only once it has been restored, so launching never overwrites it with nothing.
    pub session_ready: bool,
    pub session_saved: Option<forge_core::session::Session>,
    pub session_checked: Instant,
    pub preview_open: bool,
    /// The Markdown file shown as a full page instead of its source (Markdown: Open Preview).
    pub preview_full: Option<PathBuf>,
    /// The pointer is over a link in a Markdown preview, or over a word with Ctrl held in the editor (a
    /// go-to-definition link): the cursor becomes a hand.
    pub md_link_hover: bool,
    /// While set, files being opened get their syntax tree later (startup tabs that are not in front).
    pub defer_syntax: bool,
    /// The font settings the code and UI fonts and the character width were worked out for.
    pub font_key: Option<(String, String, u32)>,
    pub code_link_hover: bool,
    /// The Outline section of the Explorer: open or folded, what it lists, and what that list was made for.
    pub outline_open: bool,
    pub outline_items: Vec<forge_core::outline::OutlineItem>,
    pub outline_key: Option<(PathBuf, u64)>,
    /// The file and revision seen last, and when that changed: the outline waits for typing to pause.
    pub outline_seen: Option<(PathBuf, u64)>,
    pub outline_changed: Instant,
    /// The file a symbols request is out for.
    pub outline_requested: Option<PathBuf>,
    pub outline_scroll: UniformListScrollHandle,
    /// The entry the caret was in when the list last scrolled to follow it.
    pub outline_followed: std::cell::Cell<Option<usize>>,
    /// The theme colours the title bar was last painted with.
    pub titlebar_key: Option<crate::titlebar::Key>,
    /// Where the terminal view was last drawn, to turn mouse positions into cells.
    pub term_bounds: std::rc::Rc<std::cell::Cell<gpui::Bounds<gpui::Pixels>>>,
    pub term_sel: Option<crate::terminal_mouse::TermSel>,
    /// The link under the pointer in the terminal.
    pub term_link: Option<crate::terminal_mouse::TermLink>,
    /// The parsed table of the CSV file in the full-page preview, its list scroll and its sideways scroll.
    pub csv_cache: Option<crate::csv_view::CsvCache>,
    pub csv_scroll: UniformListScrollHandle,
    pub csv_scroll_x: f32,
    pub md_cache: Option<(usize, u64, Vec<forge_core::markdown::Block>)>,
    /// The preview's scroll state, and where the editor and preview were (as fractions of their height) at the last sync.
    pub preview_scroll: gpui::ScrollHandle,
    pub sync_last: (f32, f32),
    /// The running debug session, and the breakpoints (0-based lines) set per file.
    pub debug: Option<crate::debug_support::DebugSession>,
    /// The console of the last finished session, kept so its output can still be read.
    pub debug_console: Vec<(bool, String)>,
    /// A `cargo build` for a Rust debug session is running on a background thread.
    pub debug_build_rx: Option<Receiver<forge_core::cargo_build::BuildOutcome>>,
    pub breakpoints: std::collections::BTreeMap<PathBuf, std::collections::BTreeSet<usize>>,
    /// Whether the Staged Changes and Changes sections are expanded.
    pub scm_staged_open: bool,
    pub scm_changes_open: bool,
    /// A commit message is being generated by the model.
    pub ai_pending: bool,
    /// Typed by the user; held in memory for this session and never written anywhere.
    pub ai_key: Option<String>,
    /// The Settings page is showing, and the model field's text and focus.
    pub settings_open: bool,
    /// The image shown in the viewer over the editor.
    pub image_page: Option<PathBuf>,
    pub settings_model: LineInput,
    pub settings_model_focus: bool,
    /// A language-server request made while the server was still starting: run it once it is ready.
    pub lsp_retry: Option<(String, RequestTag, Instant)>,
    /// The folder's languages have not been scanned for servers to start yet.
    pub autostart_pending: bool,
    pub scm_scroll: UniformListScrollHandle,
    pub branches: Vec<Branch>,
    pub history: Vec<Commit>,
    /// Commits of every branch, for cherry-pick.
    pub pick_commits: Vec<Commit>,
    pub submodules: Vec<forge_core::git::Submodule>,
    pub worktrees: Vec<forge_core::git::Worktree>,
    /// The interactive rebase page, when open.
    pub rebase_page: Option<crate::rebase_view::RebasePage>,
    /// The commit graph and its lanes, and the active file's history, for the bottom panel.
    pub graph: Vec<forge_core::git::GraphCommit>,
    pub graph_rows: Vec<forge_core::graph::GraphRow>,
    pub graph_pending: bool,
    pub graph_scroll: UniformListScrollHandle,
    pub timeline: Option<(String, Vec<Commit>)>,
    pub timeline_pending: bool,
    pub timeline_scroll: UniformListScrollHandle,
    pub history_path: Option<String>,
    /// Result of a language-server install running in the background.
    pub install_rx: Option<Receiver<Result<String, String>>>,
    /// Installed extensions, and the background search or install in flight.
    pub extensions: Vec<forge_ext_host::Extension>,
    pub ext_rx: Option<Receiver<crate::ext_support::ExtEvent>>,
    pub ext_listings: Vec<forge_ext_host::openvsx::Listing>,
    pub ext_panel: ExtPanel,
    /// The extension being downloaded and installed, for its button.
    pub ext_installing: Option<String>,
    /// Background fetches of the README and facts for extension pages that are open.
    pub ext_detail_rx: Vec<Receiver<(String, Result<forge_ext_host::openvsx::Details, String>)>>,
    /// Questions git asked while a command ran, and the one on screen now.
    pub askpass_rx: Option<Receiver<forge_core::askpass::Request>>,
    pub askpass_pending: Option<std::sync::mpsc::Sender<Option<String>>>,
    /// The side-by-side diff: its scroll, the lines picked for staging, and the parsed rows.
    pub diff_scroll: UniformListScrollHandle,
    pub diff_selected: std::collections::BTreeSet<(usize, usize)>,
    pub diff_cache: std::cell::RefCell<Option<crate::diff_view::DiffCache>>,
    /// The merge-conflict page, when open.
    pub conflict_page: Option<crate::conflict_view::ConflictPage>,
    /// Languages and grammars the installed extensions contribute.
    pub ext_langs: Vec<forge_core::ext_lang::ExtLanguage>,
    pub tm: forge_syntax::TextMateRegistry,
    /// Snippets from extensions and the user's snippets folder, and the expansion in progress.
    pub snippets: Vec<crate::snippet_support::SnippetEntry>,
    pub snippet_session: Option<crate::snippet_support::SnippetSession>,
    /// The Node extension host and what it has told us.
    pub host: Option<forge_ext_host::host::ExtHost>,
    pub host_state: crate::host_support::HostState,
}

impl Focusable for Workspace {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Workspace {
    pub fn new(root: PathBuf, initial_file: Option<&Path>, cx: &mut Context<Self>) -> Self {
        let settings = std::fs::read_to_string(config_dir().join("settings.json")).map(|t| Settings::parse(&t)).unwrap_or_default();
        // The panel sizes the user left last time.
        let saved_layout = forge_core::ui_state::UiState::load();
        let mut keymap = Keymap::default();
        let mut status = format!("Opened {}", root.display());
        if let Ok(text) = std::fs::read_to_string(config_dir().join("keybindings.json")) {
            let warnings = keymap.apply_user(&text);
            if let Some(first) = warnings.first() {
                status = format!("keybindings.json: {first}");
            }
        }
        crate::prof::mark("  settings, keys, layout read");
        let extensions = crate::ext_support::load_extensions();
        crate::prof::mark("  extensions loaded");
        let (ext_langs, tm, _warnings) = crate::ext_support::build_ext_languages(&extensions);
        crate::prof::mark("  extension languages built");
        let snippets = crate::snippet_support::load_snippets(&extensions, settings.builtin_snippets);
        crate::prof::mark("  snippets loaded");
        let theme = crate::ext_support::resolve_theme(&extensions, &settings.theme);
        let icon_theme = crate::ext_support::resolve_icon_theme(&extensions, &settings.icon_theme);
        crate::prof::mark("  themes resolved");
        let (git_tx, git_rx) = mpsc::channel();
        let mut workspace = Self {
            lsp: LspManager::new(root.clone()),
            tree: FileTree::new(root),
            docs: Vec::new(),
            groups: vec![Group { views: Vec::new(), active: 0 }],
            focused_group: 0,
            focus_handle: cx.focus_handle(),
            focus: Focus::Editor,
            char_width: px(settings.font_size * 0.55),
            code_font: SharedString::from(FONT),
            ui_font: SharedString::from("Segoe UI"),
            settings,
            theme,
            icon_theme,
            product_dirty: true,
            keymap,
            pending_chord: String::new(),
            status: status.into(),
            status_kind: StatusKind::Info,
            status_at: Instant::now(),
            clipboard_lines: None,
            last_typing: None,
            close_armed: None,
            sidebar_visible: true,
            sidebar_view: SidebarView::Explorer,
            panel_visible: false,
            panel_view: PanelView::Terminal,
            explorer_scroll: UniformListScrollHandle::new(),
            explorer_selected: None,
            lsp_progress: None,
            completion: None,
            completion_request: None,
            signature: None,
            hover: None,
            hover_request: None,
            next_request: 0,
            mouse: None,
            mouse_moved: Instant::now(),
            symbols: Vec::new(),
            code_actions: Vec::new(),
            references: Vec::new(),
            definitions: Vec::new(),
            reference_selected: 0,
            problem_selected: 0,
            problems_scroll: UniformListScrollHandle::new(),
            references_scroll: UniformListScrollHandle::new(),
            find: FindState::default(),
            find_revision: (usize::MAX, 0),
            overlay: None,
            context_menu: None,
            search: SearchPanel {
                query: LineInput::default(),
                replace: LineInput::default(),
                replace_open: false,
                replace_focused: false,
                options: Default::default(),
                hits: Vec::new(),
                running: false,
                capped: false,
                error: None,
                rx: None,
                cancel: Arc::new(AtomicBool::new(false)),
                scroll: UniformListScrollHandle::new(),
                selected: 0,
                scope: None,
            },
            file_index: None,
            file_index_rx: None,
            recent_files: Vec::new(),
            closed_tabs: Vec::new(),
            nav_back: Vec::new(),
            nav_forward: Vec::new(),
            navigating: false,
            explorer_clipboard: None,
            quit_armed: false,
            zen_saved: None,
            auto_save_since: None,
            compare_base: None,
            recent_folders: Vec::new(),
            recent_commands: Vec::new(),
            terminals: Vec::new(),
            active_terminal: 0,
            drag: None,
            git: None,
            git_tx,
            git_rx,
            scm_message: forge_core::textarea::TextArea::default(),
            scm_dragging: false,
            minimap_dragging: false,
            sidebar_base: saved_layout.sidebar,
            panel_height: saved_layout.panel_height,
            preview_px: saved_layout.preview_width,
            group_fractions: Vec::new(),
            debug_cols: saved_layout.debug_columns,
            viewport: (1200.0, 800.0),
            resize: None,
            session_ready: false,
            session_saved: None,
            session_checked: Instant::now(),
            preview_open: false,
            preview_full: None,
            md_link_hover: false,
            defer_syntax: false,
            font_key: None,
            code_link_hover: false,
            outline_open: true,
            outline_items: Vec::new(),
            outline_key: None,
            outline_seen: None,
            outline_changed: Instant::now(),
            outline_requested: None,
            outline_scroll: UniformListScrollHandle::new(),
            outline_followed: std::cell::Cell::new(None),
            titlebar_key: None,
            term_bounds: Default::default(),
            term_sel: None,
            term_link: None,
            csv_cache: None,
            csv_scroll: UniformListScrollHandle::new(),
            csv_scroll_x: 0.0,
            md_cache: None,
            preview_scroll: gpui::ScrollHandle::new(),
            sync_last: (0.0, 0.0),
            debug: None,
            debug_console: Vec::new(),
            debug_build_rx: None,
            breakpoints: Default::default(),
            scm_staged_open: true,
            scm_changes_open: true,
            ai_pending: false,
            ai_key: None,
            settings_open: false,
            image_page: None,
            settings_model: LineInput::default(),
            settings_model_focus: false,
            lsp_retry: None,
            autostart_pending: false,
            scm_scroll: UniformListScrollHandle::new(),
            branches: Vec::new(),
            history: Vec::new(),
            pick_commits: Vec::new(),
            submodules: Vec::new(),
            worktrees: Vec::new(),
            rebase_page: None,
            graph: Vec::new(),
            graph_rows: Vec::new(),
            graph_pending: false,
            graph_scroll: UniformListScrollHandle::new(),
            tab_scrolls: std::cell::RefCell::new(Vec::new()),
            sticky_cache: std::cell::RefCell::new(Default::default()),
            timeline: None,
            timeline_pending: false,
            timeline_scroll: UniformListScrollHandle::new(),
            history_path: None,
            install_rx: None,
            extensions,
            ext_rx: None,
            ext_listings: Vec::new(),
            ext_panel: ExtPanel { query: LineInput::default(), results: Vec::new(), searched: false, waiting: false, error: None, shown_query: String::new() },
            ext_installing: None,
            ext_detail_rx: Vec::new(),
            askpass_rx: None,
            askpass_pending: None,
            diff_scroll: UniformListScrollHandle::new(),
            diff_selected: Default::default(),
            diff_cache: Default::default(),
            conflict_page: None,
            snippets,
            snippet_session: None,
            host: None,
            host_state: Default::default(),
            ext_langs,
            tm,
        };
        crate::prof::mark("  struct built");
        // Git asks for logins through this program; the questions come back here.
        if let Ok(exe) = std::env::current_exe() {
            workspace.askpass_rx = forge_core::askpass::start(&exe);
        }
        workspace.lsp.set_overrides(workspace.settings.language_servers.clone());
        crate::prof::mark("  askpass started");
        workspace.init_git();
        crate::prof::mark("  git initialised");
        workspace.restore_session(initial_file, cx);
        crate::prof::mark("  session restored");
        // Closing the window saves the session one last time, however recently it last changed.
        cx.on_app_quit(|this, _| {
            this.save_session(true);
            std::future::ready(())
        })
        .detach();
        cx.spawn(async move |this, cx| loop {
            cx.background_executor().timer(TICK).await;
            if this.update(cx, |workspace, cx| workspace.tick(cx)).is_err() {
                break;
            }
        })
        .detach();
        workspace.activate_for(forge_ext_host::host::ActivationEvent::Startup);
        crate::prof::mark("  extensions activated");
        if workspace.settings.lsp_autostart {
            workspace.autostart_pending = true;
            workspace.ensure_file_index();
        }
        if let Ok(script) = std::env::var("FORGE_REPLAY") {
            workspace.start_replay(script, cx);
        }
        workspace
    }

    /// Debugging and testing aid: `FORGE_REPLAY="ctrl+p text:main enter"` feeds keystrokes
    /// through the normal key handler, one per 120 ms. Tokens are chords (`ctrl+shift+p`,
    /// `down`, `f2`), `text:...` to type characters, `wait:MS`, and `status:` is ignored.
    fn start_replay(&mut self, script: String, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            // Let the window and language server settle before the first key.
            let delay = std::env::var("FORGE_REPLAY_DELAY_MS").ok().and_then(|v| v.parse().ok()).unwrap_or(3000u64);
            cx.background_executor().timer(Duration::from_millis(delay)).await;
            for token in script.split_whitespace() {
                if let Some(ms) = token.strip_prefix("wait:") {
                    cx.background_executor().timer(Duration::from_millis(ms.parse().unwrap_or(500))).await;
                    continue;
                }
                // `menu:tab` opens the tab bar's "more" menu where its button is.
                if token == "menu:tab" {
                    if this.update(cx, |workspace, cx| {
                        let g = workspace.focused_group;
                        workspace.open_tab_menu(g, gpui::point(gpui::px(900.0), gpui::px(60.0)), cx);
                    }).is_err() {
                        return;
                    }
                    continue;
                }
                if token.starts_with("resize:") {
                    if this.update(cx, |workspace, cx| {
                        workspace.replay_resize(token);
                        cx.notify();
                    }).is_err() {
                        return;
                    }
                    continue;
                }
                let events: Vec<gpui::KeyDownEvent> = match token.strip_prefix("text:") {
                    Some(text) => text.chars().map(|c| replay_event(&c.to_string(), c.is_uppercase(), Some(c.to_string()))).collect(),
                    None => vec![replay_event_from_chord(token)],
                };
                for event in events {
                    if this.update(cx, |workspace, cx| workspace.on_key_down(&event, cx)).is_err() {
                        return;
                    }
                    cx.background_executor().timer(Duration::from_millis(120)).await;
                }
            }
        })
        .detach();
    }

    // ---- accessors ----

    pub fn root(&self) -> &Path {
        self.tree.root()
    }

    pub fn line_height(&self) -> f32 {
        (self.settings.font_size * 1.45).round()
    }

    pub fn indent_style(&self) -> IndentStyle {
        let id = self.active_lang_id();
        IndentStyle { tab_size: self.settings.tab_size_for(id), use_spaces: self.settings.insert_spaces_for(id) }
    }

    /// VS Code's id for the language of the focused editor, which keys `"[language]"` settings.
    pub fn active_lang_id(&self) -> Option<&'static str> {
        let (g, v) = self.active_ix()?;
        let doc_ix = self.groups.get(g)?.views.get(v)?.doc;
        self.docs.get(doc_ix)?.language.map(|l| l.vscode_id())
    }

    /// `(group, view)` of the focused editor tab.
    pub fn active_ix(&self) -> Option<(usize, usize)> {
        let group = self.groups.get(self.focused_group)?;
        (group.active < group.views.len()).then_some((self.focused_group, group.active))
    }

    pub fn active_view(&self) -> Option<&View> {
        let (g, v) = self.active_ix()?;
        self.groups[g].views.get(v)
    }

    pub fn active_doc(&self) -> Option<&Doc> {
        self.docs.get(self.active_view()?.doc)
    }

    pub fn set_status(&mut self, text: impl Into<SharedString>) {
        self.status = text.into();
        self.status_kind = StatusKind::Info;
        self.status_at = Instant::now();
    }

    /// Report how something the user started ended: shown green or red in the status bar for a few seconds.
    pub fn set_result(&mut self, text: impl Into<SharedString>, ok: bool) {
        self.status = text.into();
        self.status_kind = if ok { StatusKind::Success } else { StatusKind::Error };
        self.status_at = Instant::now();
    }

    pub fn doc_index(&self, path: &Path) -> Option<usize> {
        self.docs.iter().position(|d| same_path(&d.path, path))
    }

    pub fn views_of(&self, doc: usize) -> impl Iterator<Item = (usize, usize)> + '_ {
        self.groups.iter().enumerate().flat_map(move |(g, group)| {
            group.views.iter().enumerate().filter(move |(_, v)| v.doc == doc).map(move |(vi, _)| (g, vi))
        })
    }

    // ---- opening, saving, closing ----

    /// Open `path` in the focused group (or switch to it). `at` is a `(line, UTF-16 column)`.
    pub fn open_file(&mut self, path: &Path, at: Option<(usize, usize)>, cx: &mut Context<Self>) {
        if crate::image_view::is_image(path) {
            self.open_image(path, cx);
            return;
        }
        self.image_page = None;
        let switching = self.active_doc().is_none_or(|d| !same_path(&d.path, path));
        if switching {
            self.auto_save_on_focus_change();
        }
        if switching || at.is_some() {
            self.mark_jump();
        }
        let group = self.focused_group;
        if let Some(vi) = self.groups[group].views.iter().position(|v| same_path(&self.docs[v.doc].path, path)) {
            self.groups[group].active = vi;
        } else if let Some(doc) = self.doc_index(path) {
            self.groups[group].views.push(View::new(doc));
            self.groups[group].active = self.groups[group].views.len() - 1;
        } else {
            match crate::prof::time("open: read file", || Self::read_file(path)) {
                Ok((text, bom)) => {
                    let defer = self.defer_syntax;
                    let mut new_doc = crate::prof::time("open: parse", || if defer { Doc::new_unparsed(path.to_path_buf(), &text) } else { Doc::new(path.to_path_buf(), &text) });
                    new_doc.bom = bom;
                    crate::prof::time("open: language", || self.attach_ext_language(&mut new_doc));
                    // No telling name (a script with no extension, ...): look at what is in it.
                    if new_doc.language.is_none() && new_doc.ext_lang.is_none() {
                        if let Some(found) = Language::from_content(&text) {
                            new_doc.set_language(Some(found));
                        }
                    }
                    self.docs.push(new_doc);
                    let new_ix = self.docs.len() - 1;
                    self.refresh_ext_diagnostics(new_ix);
                    self.activate_for_doc(new_ix);
                    let doc = self.docs.len() - 1;
                    self.groups[group].views.push(View::new(doc));
                    self.groups[group].active = self.groups[group].views.len() - 1;
                    crate::prof::time("open: attach server", || self.attach_lsp(doc));
                }
                Err(err) => {
                    // A file that is not text opens as a read-only hex dump (the start of it).
                    if err.ends_with("not valid UTF-8") {
                        if let Ok(bytes) = std::fs::read(path) {
                            let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                            let dump = forge_core::hexdump::hex_dump(&bytes, 256 * 1024);
                            self.open_virtual(&format!("hex:{}", path.display()), &format!("{name} (hex)"), &dump, false, cx);
                            self.set_status(format!("{} is not text: showing a hex dump", path.display()));
                            return;
                        }
                    }
                    self.set_status(format!("Cannot open: {err}"));
                    cx.notify();
                    return;
                }
            }
        }
        self.completion = None;
        self.hover = None;
        self.signature = None;
        self.focus = Focus::Editor;
        self.recent_files.retain(|p| !same_path(p, path));
        self.recent_files.insert(0, path.to_path_buf());
        self.recent_files.truncate(20);
        self.tree.reveal(path);
        if let Some((line, col)) = at {
            self.place_caret_utf16(line, col);
        }
        self.set_status(format!("Opened {}", path.display()));
        cx.notify();
    }

    /// Open `path` in a replaceable tab, as a single click in the Explorer does: it takes the place of the
    /// group's current preview tab. A file already open keeps its tab as it is.
    pub fn open_file_preview(&mut self, path: &Path, cx: &mut Context<Self>) {
        if !self.settings.enable_preview {
            self.open_file(path, None, cx);
            return;
        }
        let g = self.focused_group;
        let was_open = self.groups[g].views.iter().any(|v| same_path(&self.docs[v.doc].path, path));
        let old_doc = self.groups[g].views.iter().find(|v| v.preview && !same_path(&self.docs[v.doc].path, path)).map(|v| v.doc);
        self.open_file(path, None, cx);
        if was_open {
            return;
        }
        let Some(new_vi) = self.groups[g].views.iter().position(|v| same_path(&self.docs[v.doc].path, path)) else { return };
        self.groups[g].views[new_vi].preview = true;
        if let Some(old) = old_doc {
            if let Some(vi) = self.groups[g].views.iter().position(|v| v.doc == old && v.preview) {
                if !self.docs[old].dirty {
                    self.close_view(g, vi, cx);
                    // Closing a tab to the left shifts the one just opened.
                    if let Some(vi) = self.groups[g].views.iter().position(|v| same_path(&self.docs[v.doc].path, path)) {
                        self.groups[g].active = vi;
                    }
                }
            }
        }
    }

    /// Make a replaceable tab permanent.
    pub fn keep_open(&mut self, g: usize, vi: usize) {
        if let Some(view) = self.groups.get_mut(g).and_then(|gr| gr.views.get_mut(vi)) {
            view.preview = false;
        }
    }

    /// Set the language of document `ix` and bring everything that depends on it along: highlighting (and the
    /// languages embedded in it), the language server, extensions, tab size.
    pub fn choose_language(&mut self, ix: usize, choice: LangChoice, cx: &mut Context<Self>) {
        let Some(doc) = self.docs.get_mut(ix) else { return };
        if doc.readonly {
            self.set_status("This document is read-only");
            return;
        }
        let (old, was_open) = (doc.language, doc.lsp_open);
        let manual = !matches!(choice, LangChoice::Auto);
        let wanted = match choice {
            LangChoice::PlainText => None,
            LangChoice::Lang(l) => Some(l),
            LangChoice::Auto => {
                let by_name = if doc.untitled { None } else { Language::from_path(&doc.path) };
                by_name.or_else(|| Language::from_content(&doc.buffer.text_head(6000)))
            }
        };
        doc.lang_manual = manual;
        doc.set_language(wanted);
        if let (Some(language), true) = (old, was_open) {
            let path = self.docs[ix].path.clone();
            self.lsp.did_close(language, &path);
            self.docs[ix].lsp_open = false;
        }
        // Back to automatic with nothing built in found: an extension's language for the name may fit.
        if wanted.is_none() && !manual {
            let mut doc = std::mem::replace(&mut self.docs[ix], Doc::new_virtual(PathBuf::new(), "", "", false));
            self.attach_ext_language(&mut doc);
            self.docs[ix] = doc;
        }
        self.refresh_ext_diagnostics(ix);
        self.activate_for_doc(ix);
        self.attach_lsp(ix);
        self.outline_key = None;
        let name = self.docs[ix].syntax.as_ref().map(|s| s.language().name().to_owned()).or_else(|| self.docs[ix].ext_lang.as_ref().map(|l| l.name.clone())).unwrap_or_else(|| "Plain Text".to_owned());
        self.set_status(if manual { format!("Language mode: {name}") } else { format!("Language mode: Auto Detect ({name})") });
        cx.notify();
    }

    /// A new empty `Untitled-<n>` tab (Ctrl+N), with nothing asked until it is saved.
    pub fn new_untitled(&mut self, cx: &mut Context<Self>) {
        let n = (1..).find(|n| !self.docs.iter().any(|d| d.untitled && d.title.as_deref() == Some(format!("Untitled-{n}").as_str()))).unwrap_or(1);
        self.auto_save_on_focus_change();
        self.mark_jump();
        self.image_page = None;
        self.docs.push(Doc::new_untitled(n));
        let doc = self.docs.len() - 1;
        let g = self.focused_group;
        self.groups[g].views.push(View::new(doc));
        self.groups[g].active = self.groups[g].views.len() - 1;
        self.completion = None;
        self.hover = None;
        self.signature = None;
        self.focus = Focus::Editor;
        cx.notify();
    }

    /// Save the open document under a name picked in the system's save dialog. An untitled document becomes
    /// that file; a saved one is written to the new place and its tab follows.
    pub fn save_as(&mut self, cx: &mut Context<Self>) {
        let Some(view) = self.active_view() else { return };
        let doc = &self.docs[view.doc];
        if doc.readonly {
            self.set_status("This document is read-only");
            return;
        }
        let old = doc.path.clone();
        let (dir, suggested) = if doc.untitled {
            (self.root().to_path_buf(), doc.name())
        } else {
            (old.parent().map(Path::to_path_buf).unwrap_or_else(|| self.root().to_path_buf()), doc.name())
        };
        let picked = cx.prompt_for_new_path(&dir, Some(&suggested));
        cx.spawn(async move |this, cx| {
            if let Ok(Ok(Some(path))) = picked.await {
                let _ = this.update(cx, |workspace, cx| workspace.finish_save_as(&old, path, cx));
            }
        })
        .detach();
    }

    fn finish_save_as(&mut self, old: &Path, new: PathBuf, cx: &mut Context<Self>) {
        let Some(ix) = self.doc_index(old) else { return };
        if self.docs.iter().enumerate().any(|(i, d)| i != ix && same_path(&d.path, &new)) {
            self.set_status(format!("{} is open in another tab: close it first", new.display()));
            return;
        }
        let mut contents = self.docs[ix].buffer.text();
        if self.docs[ix].bom {
            contents.insert(0, char::from_u32(0xFEFF).unwrap_or(' '));
        }
        if let Err(err) = std::fs::write(&new, &contents) {
            self.set_status(format!("Save failed: {err}"));
            return;
        }
        let (language, was_open) = (self.docs[ix].language, self.docs[ix].lsp_open);
        if let (Some(language), true) = (language, was_open) {
            self.lsp.did_close(language, &self.docs[ix].path);
        }
        // The same text under its new name: the language, highlighting and server follow the extension.
        let text = self.docs[ix].buffer.text();
        let mut fresh = Doc::new(new.clone(), &text);
        fresh.bom = self.docs[ix].bom;
        fresh.buffer = std::mem::replace(&mut self.docs[ix].buffer, forge_text::Buffer::new(""));
        self.attach_ext_language(&mut fresh);
        self.docs[ix] = fresh;
        self.refresh_ext_diagnostics(ix);
        self.activate_for_doc(ix);
        self.attach_lsp(ix);
        for view in self.groups.iter_mut().flat_map(|g| g.views.iter_mut()).filter(|v| v.doc == ix) {
            view.preview = false;
            view.folds_revision = u64::MAX;
        }
        self.recent_files.retain(|p| !same_path(p, &new));
        self.recent_files.insert(0, new.clone());
        self.file_index = None;
        self.tree.refresh();
        self.tree.reveal(&new);
        self.git_request_refresh();
        self.set_status(format!("Saved {}", new.display()));
        cx.notify();
    }

    /// Remember the caret as a place to return to with Go Back (Alt+Left).
    pub fn mark_jump(&mut self) {
        if self.navigating {
            return;
        }
        let Some((path, offset)) = self.active_doc().zip(self.active_view()).filter(|(d, _)| !d.readonly).map(|(d, v)| (d.path.clone(), v.selections.primary().head)) else { return };
        if self.nav_back.last().is_some_and(|(p, o)| same_path(p, &path) && *o == offset) {
            return;
        }
        self.nav_back.push((path, offset));
        if self.nav_back.len() > 100 {
            self.nav_back.remove(0);
        }
        self.nav_forward.clear();
    }

    /// Alt+Left (`forward` false) or Alt+Right: move through the places recorded by [`Self::mark_jump`].
    pub fn navigate(&mut self, forward: bool, cx: &mut Context<Self>) {
        let here = self.active_doc().zip(self.active_view()).filter(|(d, _)| !d.readonly).map(|(d, v)| (d.path.clone(), v.selections.primary().head));
        let target = if forward { self.nav_forward.pop() } else { self.nav_back.pop() };
        let Some((path, offset)) = target else {
            self.set_status(if forward { "Nothing to go forward to" } else { "Nothing to go back to" });
            return;
        };
        if let Some(here) = here {
            if forward { &mut self.nav_back } else { &mut self.nav_forward }.push(here);
        }
        self.navigating = true;
        self.open_file(&path, None, cx);
        self.place_caret_offset(offset);
        self.navigating = false;
        cx.notify();
    }

    fn read_file(path: &Path) -> Result<(String, bool), String> {
        let meta = std::fs::metadata(path).map_err(|e| e.to_string())?;
        if meta.len() > MAX_FILE_BYTES {
            return Err(format!("{} is larger than {} MiB", path.display(), MAX_FILE_BYTES >> 20));
        }
        let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
        decode_utf8(bytes).ok_or_else(|| format!("{} is not valid UTF-8", path.display()))
    }

    pub fn place_caret_utf16(&mut self, line: usize, col: usize) {
        let Some((g, vi)) = self.active_ix() else { return };
        let view = &mut self.groups[g].views[vi];
        let doc = &self.docs[view.doc];
        let offset = doc.buffer.offset_from_utf16(line, col);
        view.selections = SelectionSet::single(Selection::caret(offset));
        self.reveal_caret(g, vi, true);
    }

    pub fn place_caret_offset(&mut self, offset: usize) {
        let Some((g, vi)) = self.active_ix() else { return };
        self.groups[g].views[vi].selections = SelectionSet::single(Selection::caret(offset));
        self.reveal_caret(g, vi, true);
    }

    /// Scroll so the primary caret is visible, unfolding anything that hides it.
    pub fn reveal_caret(&mut self, g: usize, vi: usize, center: bool) {
        let view = &mut self.groups[g].views[vi];
        let doc = &self.docs[view.doc];
        let (line, col) = doc.buffer.offset_to_point(view.selections.primary().head);
        while view.folds.is_hidden(line) {
            if !view.folds.unfold_at(line) {
                break;
            }
        }
        let wrapped = view.wrap.as_ref().and_then(|w| w.row_of(line, forge_text::columns::display_width(&doc.buffer.line(line), col)));
        let index = wrapped.unwrap_or_else(|| view.folds.visible_lines(doc.buffer.len_lines()).and_then(|v| v.iter().position(|l| *l == line)).unwrap_or(line));
        // Sideways: scroll just enough that the caret column is inside the text area, with a little room.
        if view.wrap.is_none() {
            let viewport = f32::from(view.scroll.0.borrow().base_handle.bounds().size.width) - GUTTER_WIDTH - TEXT_PAD;
            if viewport > 0.0 {
                let text = doc.buffer.line(line);
                let col = doc.buffer.offset_to_point(view.selections.primary().head).1;
                let x = forge_text::columns::display_width(&text, col) as f32 * f32::from(self.char_width);
                let margin = f32::from(self.char_width) * 4.0;
                if x < view.scroll_x {
                    view.scroll_x = (x - margin).max(0.0);
                } else if x + margin > view.scroll_x + viewport {
                    view.scroll_x = x + margin - viewport;
                }
            }
        }
        // Non-strict scrolling only moves when the line is off screen; `center` forces it.
        if center {
            view.scroll.scroll_to_item_strict(index, gpui::ScrollStrategy::Center);
        } else {
            view.scroll.scroll_to_item(index, gpui::ScrollStrategy::Center);
        }
    }

    pub fn write_doc(&mut self, doc_ix: usize) {
        let Some(doc) = self.docs.get_mut(doc_ix) else { return };
        // An untitled document has no file to write to: Save (or Save As) asks where it goes.
        if doc.untitled {
            return;
        }
        if doc.readonly {
            self.set_status("This document is read-only");
            return;
        }
        let mut contents = doc.buffer.text();
        if doc.bom {
            contents.insert(0, '\u{FEFF}');
        }
        match std::fs::write(&doc.path, contents) {
            Ok(()) => {
                doc.dirty = false;
                let path = doc.path.clone();
                let language = doc.language;
                let open = doc.lsp_open;
                self.set_status(format!("Saved {}", path.display()));
                if let (Some(language), true) = (language, open) {
                    self.lsp.did_save(language, &path);
                }
                self.reload_config_if_needed(&path);
                self.file_index = None;
                self.git_request_refresh();
            }
            Err(err) => self.set_status(format!("Save failed: {err}")),
        }
    }

    /// Saving `settings.json` or `keybindings.json` applies it immediately.
    pub(crate) fn reload_config_if_needed(&mut self, path: &Path) {
        if path.parent().is_some_and(|dir| same_path(dir, &config_dir().join("snippets"))) {
            self.snippets = crate::snippet_support::load_snippets(&self.extensions, self.settings.builtin_snippets);
            self.set_status(format!("Snippets reloaded ({} available)", self.snippets.len()));
        }
        if same_path(path, &config_dir().join("settings.json")) {
            if let Ok(text) = std::fs::read_to_string(path) {
                let provider = self.settings.ai_provider;
                self.settings = Settings::parse(&text);
                // A key typed for one provider is not the other's.
                if self.settings.ai_provider != provider {
                    self.ai_key = None;
                }
                self.snippets = crate::snippet_support::load_snippets(&self.extensions, self.settings.builtin_snippets);
                self.theme = crate::ext_support::resolve_theme(&self.extensions, &self.settings.theme);
                self.icon_theme = crate::ext_support::resolve_icon_theme(&self.extensions, &self.settings.icon_theme);
                self.product_dirty = true;
                self.lsp.set_overrides(self.settings.language_servers.clone());
                for ix in 0..self.docs.len() {
                    if !self.docs[ix].lsp_open {
                        self.attach_lsp(ix);
                    }
                }
                self.host_config_changed();
                self.set_status("Settings reloaded");
            }
        } else if same_path(path, &config_dir().join("keybindings.json")) {
            if let Ok(text) = std::fs::read_to_string(path) {
                let mut keymap = Keymap::default();
                let warnings = keymap.apply_user(&text);
                self.keymap = keymap;
                self.set_status(match warnings.first() {
                    Some(w) => format!("keybindings.json: {w}"),
                    None => "Keybindings reloaded".to_owned(),
                });
            }
        }
    }

    /// Close a tab. A modified document with no other tab needs a second request.
    pub fn close_view(&mut self, g: usize, vi: usize, cx: &mut Context<Self>) {
        let Some(view) = self.groups.get(g).and_then(|gr| gr.views.get(vi)) else { return };
        let doc_ix = view.doc;
        let others = self.views_of(doc_ix).count() > 1;
        if self.docs[doc_ix].dirty && !others && self.close_armed != Some((g, vi)) {
            self.close_armed = Some((g, vi));
            let name = self.docs[doc_ix].name();
            self.set_status(format!("{name} has unsaved changes: Ctrl+S to save, or close again to discard"));
            cx.notify();
            return;
        }
        self.close_armed = None;
        self.completion = None;
        self.hover = None;
        if !self.docs[doc_ix].readonly && self.docs[doc_ix].path.is_file() {
            let caret = self.groups[g].views[vi].selections.primary().head;
            let path = self.docs[doc_ix].path.clone();
            self.closed_tabs.retain(|(p, _)| !same_path(p, &path));
            self.closed_tabs.push((path, caret));
            if self.closed_tabs.len() > 30 {
                self.closed_tabs.remove(0);
            }
        }
        let group = &mut self.groups[g];
        group.views.remove(vi);
        group.active = group.active.min(group.views.len().saturating_sub(1));
        if self.groups[g].views.is_empty() && self.groups.len() > 1 {
            self.groups.remove(g);
            self.focused_group = self.focused_group.min(self.groups.len() - 1);
        }
        if self.views_of(doc_ix).next().is_none() {
            let doc = self.docs.remove(doc_ix);
            if let (Some(language), true) = (doc.language, doc.lsp_open) {
                self.lsp.did_close(language, &doc.path);
            }
            for view in self.groups.iter_mut().flat_map(|g| g.views.iter_mut()) {
                if view.doc > doc_ix {
                    view.doc -= 1;
                }
            }
        }
        cx.notify();
    }

    // ---- the edit pipeline ----

    /// Run an edit on the focused view's buffer and propagate its effects.
    pub fn mutate(&mut self, cx: &mut Context<Self>, f: impl FnOnce(&mut Buffer, &SelectionSet) -> SelectionSet) {
        let Some((g, vi)) = self.active_ix() else { return };
        if self.docs[self.groups[g].views[vi].doc].readonly {
            self.set_status("This document is read-only");
            return;
        }
        if self.full_preview_shown(g) {
            self.set_status("The preview is showing: press Ctrl+Shift+V to go back to the source and edit");
            return;
        }
        let view = &mut self.groups[g].views[vi];
        let doc = &mut self.docs[view.doc];
        let before = view.selections.clone();
        let after = f(&mut doc.buffer, &before);
        if after != before {
            view.history.push(before);
            if view.history.len() > 100 {
                view.history.remove(0);
            }
        }
        view.selections = after;
        view.goal = None;
        view.expand.clear();
        let doc_ix = view.doc;
        let edits = crate::prof::time("syntax reparse", || doc.sync_syntax());
        if !edits.is_empty() {
            crate::prof::time("after edits", || self.after_edits(doc_ix, Some((g, vi)), &edits));
        }
        self.reveal_caret(g, vi, false);
        cx.notify();
    }

    /// Update everything that depends on a document's text after `edits`.
    pub fn after_edits(&mut self, doc_ix: usize, origin: Option<(usize, usize)>, edits: &[TextEdit]) {
        self.docs[doc_ix].dirty = true;
        for view in self.groups.iter_mut().flat_map(|g| g.views.iter_mut()).filter(|v| v.doc == doc_ix) {
            view.preview = false;
        }
        self.docs[doc_ix].revision += 1;
        // A new untitled file takes the language of what is typed or pasted into it, until one is picked.
        if self.docs[doc_ix].untitled && !self.docs[doc_ix].lang_manual {
            let found = Language::from_content(&self.docs[doc_ix].buffer.text_head(6000));
            if found.is_some() && found != self.docs[doc_ix].language {
                self.docs[doc_ix].set_language(found);
                self.attach_lsp(doc_ix);
            }
        }
        for (g, vi) in self.views_of(doc_ix).collect::<Vec<_>>() {
            let view = &mut self.groups[g].views[vi];
            if Some((g, vi)) != origin {
                view.selections = view.selections.map_through_edits(edits);
            }
            for e in edits {
                view.folds.apply_edit(e.start_point.0, e.old_end_point.0, e.new_end_point.0);
            }
            // More than one edit between two builds is not worth tracking: the strip is rebuilt whole then.
            if view.minimap.is_some() {
                view.minimap_pending.clear();
                if let [e] = edits {
                    view.minimap_pending.push((e.start_point.0, e.old_end_point.0, e.new_end_point.0));
                }
            }
        }
        self.snippet_follow(doc_ix, edits);
        self.send_changes(doc_ix, edits);
        self.hover = None;
        self.hover_request = None;
        if self.find.open {
            self.refresh_find();
        }
    }

    fn send_changes(&mut self, doc_ix: usize, edits: &[TextEdit]) {
        let Some(doc) = self.docs.get_mut(doc_ix) else { return };
        let (Some(language), true, false) = (doc.language, doc.lsp_open, edits.is_empty()) else { return };
        doc.lsp_version += 1;
        let changes: Vec<ContentChange> = edits
            .iter()
            .map(|e| ContentChange {
                range: LspRange {
                    start: Position { line: e.start_utf16.0, character: e.start_utf16.1 },
                    end: Position { line: e.old_end_utf16.0, character: e.old_end_utf16.1 },
                },
                text: e.inserted.clone(),
            })
            .collect();
        self.lsp.did_change(language, &doc.path, doc.lsp_version, &changes);
    }

    /// Apply a server's text edits to a document as one undoable change.
    pub fn apply_text_edits(&mut self, doc_ix: usize, edits: &[TextEditSpec], cx: &mut Context<Self>) {
        let doc = &mut self.docs[doc_ix];
        let mut reps: Vec<(usize, usize, String)> = edits
            .iter()
            .map(|e| {
                (
                    doc.buffer.offset_from_utf16(e.range.start.line, e.range.start.character),
                    doc.buffer.offset_from_utf16(e.range.end.line, e.range.end.character),
                    e.new_text.clone(),
                )
            })
            .collect();
        reps.sort_by_key(|r| (r.0, r.1));
        // Overlapping edits from a misbehaving server are dropped rather than corrupting text.
        let mut kept: Vec<(usize, usize, String)> = Vec::new();
        for rep in reps {
            if kept.last().is_none_or(|last| last.1 <= rep.0) {
                kept.push(rep);
            }
        }
        if kept.is_empty() {
            return;
        }
        let anchor = self.views_of(doc_ix).next().map(|(g, vi)| self.groups[g].views[vi].selections.clone()).unwrap_or_else(|| SelectionSet::single(Selection::caret(0)));
        let doc = &mut self.docs[doc_ix];
        doc.buffer.edit_many(&anchor, &kept, |_, _| anchor.clone());
        let applied = doc.sync_syntax();
        self.after_edits(doc_ix, None, &applied);
        cx.notify();
    }

    // ---- language servers ----

    pub fn attach_lsp(&mut self, doc_ix: usize) {
        let Some(language) = self.docs[doc_ix].language.filter(|l| self.lsp.key_for(*l).is_some()) else {
            eprintln!("[lsp] {}: no language server for this file (language {:?})", self.docs[doc_ix].path.display(), self.docs[doc_ix].language);
            return;
        };
        match self.lsp.start(language) {
            Err(reason) => {
                eprintln!("[lsp] {}: cannot start a server: {reason}", self.docs[doc_ix].path.display());
                self.set_status(reason)
            }
            Ok(()) => {
                let doc = &mut self.docs[doc_ix];
                doc.lsp_open = self.lsp.did_open(language, &doc.path, doc.lsp_version, &doc.buffer.text());
                eprintln!("[lsp] {}: opened on server = {}", doc.path.display(), doc.lsp_open);
            }
        }
    }

    /// Why the active file has no language server to ask, in words for the status bar.
    pub fn lsp_unavailable_reason(&self) -> String {
        let Some(doc) = self.active_doc() else { return "no file is open".to_owned() };
        match doc.language {
            None => "Forge does not recognise this file type".to_owned(),
            Some(language) => self.lsp.unavailable_reason(language),
        }
    }

    pub fn lsp_target(&self) -> Option<(Language, String, Position)> {
        let doc = self.active_doc()?;
        if !doc.lsp_open {
            return None;
        }
        let view = self.active_view()?;
        let (line, character) = doc.buffer.utf16_point(view.selections.primary().head);
        Some((doc.language?, path_to_uri(&doc.path)?, Position { line, character }))
    }

    pub fn next_id(&mut self) -> u64 {
        self.next_request += 1;
        self.next_request
    }

    pub fn request_completion(&mut self) {
        let Some((language, uri, at)) = self.lsp_target() else {
            self.set_status("Language server is not ready");
            return;
        };
        let id = self.next_id();
        self.completion_request = Some(id);
        self.lsp.request(language, "textDocument/completion", text_document_position(&uri, at), RequestTag::Completion(id));
    }

    pub fn request_signature(&mut self) {
        let Some((language, uri, at)) = self.lsp_target() else { return };
        let id = self.next_id();
        self.lsp.request(language, "textDocument/signatureHelp", text_document_position(&uri, at), RequestTag::Signature(id));
    }

    pub fn request_at(&mut self, method: &str, tag: RequestTag) {
        let Some((language, uri, at)) = self.lsp_target() else {
            let language = self.active_doc().and_then(|d| d.language);
            if language.is_some_and(|l| self.lsp.is_starting(l)) {
                // The server is still loading the project: do this as soon as it is ready.
                eprintln!("[lsp] {method}: server is starting, will retry when ready");
                self.lsp_retry = Some((method.to_owned(), tag, Instant::now()));
                self.set_status("Language server is still starting; this will run as soon as it is ready");
                return;
            }
            let reason = self.lsp_unavailable_reason();
            eprintln!("[lsp] {method}: no target: {reason}");
            self.set_status(format!("Language server unavailable: {reason}"));
            return;
        };
        eprintln!("[lsp] {method} at {}:{} in {uri}", at.line, at.character);
        let params = match method {
            "textDocument/references" => forge_lsp::references_params(&uri, at),
            _ => text_document_position(&uri, at),
        };
        self.lsp.request(language, method, params, tag);
    }

    /// Drain background work. Called on a timer; re-renders only if something changed.
    pub fn tick(&mut self, cx: &mut Context<Self>) {
        let mut changed = false;
        for event in self.lsp.poll() {
            self.handle_lsp_event(event, cx);
            changed = true;
        }
        for terminal in &mut self.terminals {
            changed |= terminal.pump();
        }
        changed |= self.poll_background();
        changed |= self.debug_tick(cx);
        self.save_session(false);
        changed |= self.refresh_outline();
        // The editor scrolls during layout, after render has run, so the preview is kept in step here.
        if self.preview_open {
            if let Some((g, vi)) = self.active_ix().filter(|(g, _)| self.is_markdown(*g)) {
                changed |= self.sync_preview_scroll(g, vi);
            }
        }
        changed |= self.poll_host(cx);
        changed |= self.check_hover_dwell();
        changed |= self.git_tick(cx);
        changed |= self.auto_save_tick();
        if changed {
            cx.notify();
        }
    }

    /// `files.autoSave: afterDelay` saves the focused file once it has been left alone for the delay.
    fn auto_save_tick(&mut self) -> bool {
        if self.settings.auto_save != forge_core::settings::AutoSave::AfterDelay {
            self.auto_save_since = None;
            return false;
        }
        let Some(view) = self.active_view() else { return false };
        let ix = view.doc;
        let doc = &self.docs[ix];
        if !doc.dirty || doc.readonly {
            self.auto_save_since = None;
            return false;
        }
        let key = (ix, doc.revision);
        match self.auto_save_since {
            Some((i, r, at)) if (i, r) == key => {
                if at.elapsed() < Duration::from_millis(self.settings.auto_save_delay_ms) {
                    return false;
                }
                self.auto_save_since = None;
                self.write_doc(ix);
                true
            }
            _ => {
                self.auto_save_since = Some((key.0, key.1, Instant::now()));
                false
            }
        }
    }

    /// `files.autoSave: onFocusChange`: save every modified file when focus leaves the editor or its tab.
    pub fn auto_save_on_focus_change(&mut self) {
        if self.settings.auto_save != forge_core::settings::AutoSave::OnFocusChange {
            return;
        }
        let dirty: Vec<usize> = (0..self.docs.len()).filter(|&i| self.docs[i].dirty && !self.docs[i].readonly).collect();
        for ix in dirty {
            self.write_doc(ix);
        }
    }

    fn handle_lsp_event(&mut self, event: Event, cx: &mut Context<Self>) {
        match event {
            Event::Ready { key } => {
                for ix in 0..self.docs.len() {
                    let serves = self.docs[ix].language.and_then(|l| self.lsp.key_for(l)).as_deref() == Some(key.as_str());
                    if serves && !self.docs[ix].lsp_open {
                        self.attach_lsp(ix);
                    }
                }
                if let Some((method, tag, at)) = self.lsp_retry.take() {
                    if at.elapsed() < std::time::Duration::from_secs(90) {
                        self.request_at(&method, tag);
                    }
                }
            }
            Event::Diagnostics { path, diagnostics } => {
                if let Some(ix) = self.doc_index(&path) {
                    self.docs[ix].set_lsp_diagnostics(diagnostics);
                }
            }
            Event::Progress { title, text } => {
                let short = title.split(" (").next().unwrap_or(&title).to_owned();
                self.lsp_progress = text.map(|t| format!("{short}: {t}"));
            }
            Event::Failed { key, title, message } => {
                self.set_status(format!("{title}: {message}"));
                self.lsp_progress = None;
                self.completion = None;
                for doc in self.docs.iter_mut().filter(|d| d.language.and_then(|l| self.lsp.key_for(l)).as_deref() == Some(key.as_str())) {
                    doc.lsp_open = false;
                    doc.set_lsp_diagnostics(Vec::new());
                }
            }
            Event::Response { tag, result } => self.handle_response(tag, result, cx),
        }
    }

    fn handle_response(&mut self, tag: RequestTag, result: serde_json::Value, cx: &mut Context<Self>) {
        match tag {
            RequestTag::Completion(id) if self.completion_request == Some(id) => {
                self.show_completions(parse_completion(&result));
            }
            RequestTag::Completion(_) => {}
            RequestTag::Definition => {
                eprintln!("[lsp] definition result: {result}");
                self.go_to_locations(parse_locations(&result), cx)
            }
            RequestTag::Hover(id) => {
                if let (Some((want, line, col)), Some(text)) = (self.hover_request, parse_hover(&result).map(|t| collapse_blank_lines(&t))) {
                    if want == id {
                        self.hover = Some(HoverInfo { text, line, col });
                    }
                }
            }
            RequestTag::Signature(_) => self.signature = parse_signature_help(&result),
            RequestTag::References => {
                let locations = parse_locations(&result);
                if locations.is_empty() {
                    self.set_status("No references found");
                    return;
                }
                self.references = locations
                    .into_iter()
                    .take(500)
                    .map(|location| {
                        let preview = self.line_preview(&location);
                        RefEntry { location, preview }
                    })
                    .collect();
                self.reference_selected = 0;
                self.panel_view = PanelView::References;
                self.panel_visible = true;
                self.set_status(format!("{} references", self.references.len()));
            }
            RequestTag::Rename => {
                let edits = parse_workspace_edit(&result);
                self.apply_workspace_edit(edits, cx);
            }
            RequestTag::CodeActions => {
                self.code_actions = parse_code_actions(&result);
                if self.code_actions.is_empty() {
                    self.overlay = None;
                    self.set_status("No code actions available");
                } else {
                    self.refresh_overlay();
                }
            }
            RequestTag::Format { then_save } => {
                let edits = parse_text_edits(&result);
                if let Some((g, vi)) = self.active_ix() {
                    let doc_ix = self.groups[g].views[vi].doc;
                    if !edits.is_empty() {
                        self.apply_text_edits(doc_ix, &edits, cx);
                    }
                    if then_save {
                        self.write_doc(doc_ix);
                    } else if edits.is_empty() {
                        self.set_status("No formatting changes");
                    }
                }
            }
            RequestTag::Symbols => {
                self.symbols = parse_document_symbols(&result);
                self.refresh_overlay();
            }
            RequestTag::Outline => {
                let symbols = parse_document_symbols(&result);
                // Only while the file it was asked for is still the one in front.
                if self.outline_requested.take().is_some_and(|p| self.active_doc().is_some_and(|d| same_path(&d.path, &p))) && !symbols.is_empty() {
                    self.outline_items = symbols
                        .iter()
                        .map(|s| forge_core::outline::OutlineItem { name: s.name.clone(), kind: s.kind_label(), line: s.range.start.line, col: s.range.start.character, depth: s.depth })
                        .collect();
                }
            }
            RequestTag::FormatRange => {
                let edits = parse_text_edits(&result);
                match self.active_ix().map(|(g, vi)| self.groups[g].views[vi].doc) {
                    Some(doc_ix) if !edits.is_empty() => self.apply_text_edits(doc_ix, &edits, cx),
                    _ => self.set_status("No formatting changes (this server may not format selections)"),
                }
            }
            RequestTag::OrganizeImports => {
                let actions = parse_code_actions(&result);
                match actions.into_iter().find(|a| !a.edit.is_empty()) {
                    Some(action) => self.apply_workspace_edit(action.edit, cx),
                    None => self.set_status("Imports are already organized, or this server cannot organize them"),
                }
            }
        }
    }

    fn line_preview(&self, location: &Location) -> String {
        if let Some(doc) = self.doc_index(&location.path).map(|i| &self.docs[i]) {
            return doc.buffer.line(location.range.start.line.min(doc.buffer.len_lines() - 1)).trim().to_owned();
        }
        forge_core::project_search::read_text_file(&location.path)
            .and_then(|t| t.lines().nth(location.range.start.line).map(|l| l.trim().to_owned()))
            .unwrap_or_default()
    }

    pub fn apply_workspace_edit(&mut self, files: Vec<(PathBuf, Vec<TextEditSpec>)>, cx: &mut Context<Self>) {
        if files.is_empty() {
            self.set_status("Nothing to change");
            return;
        }
        let (mut total, mut count) = (0, 0);
        for (path, edits) in files {
            if edits.is_empty() {
                continue;
            }
            self.open_file(&path, None, cx);
            if let Some(ix) = self.doc_index(&path) {
                total += edits.len();
                count += 1;
                self.apply_text_edits(ix, &edits, cx);
            }
        }
        self.set_status(format!("Applied {total} edits in {count} files (unsaved)"));
    }

    fn show_completions(&mut self, items: Vec<CompletionItem>) {
        let Some(view) = self.active_view() else { return };
        let doc = &self.docs[view.doc];
        let caret = view.selections.primary().head;
        let prefix = doc.buffer.rope().slice(identifier_start(&doc.buffer, caret)..caret).to_string();
        let mut items = filter_items(items, &prefix);
        items.extend(self.snippet_completion_items(&prefix));
        self.completion = (!items.is_empty()).then(|| CompletionState::new(items));
    }

    pub fn go_to_locations(&mut self, locations: Vec<Location>, cx: &mut Context<Self>) {
        let mut unique: Vec<Location> = Vec::new();
        for location in locations {
            let seen = unique.iter().any(|u| same_path(&u.path, &location.path) && u.range.start == location.range.start);
            if !seen {
                unique.push(location);
            }
        }
        match unique.len() {
            0 => self.set_status("No definition found"),
            1 => {
                let location = &unique[0];
                self.open_file(&location.path, Some((location.range.start.line, location.range.start.character)), cx);
            }
            _ => {
                self.definitions = unique
                    .into_iter()
                    .take(200)
                    .map(|location| {
                        let preview = self.line_preview(&location);
                        RefEntry { location, preview }
                    })
                    .collect();
                self.open_overlay(OverlayKind::Definitions, "");
                self.set_status(format!("{} definitions", self.definitions.len()));
            }
        }
    }

    /// `path` relative to the project root when it is inside it.
    pub fn display_path(&self, path: &Path) -> String {
        path.strip_prefix(self.root()).unwrap_or(path).to_string_lossy().replace('\\', "/")
    }

    /// Hover requests start when the pointer has rested over text.
    fn check_hover_dwell(&mut self) -> bool {
        if self.mouse.is_none() || self.mouse_moved.elapsed() < HOVER_DELAY || self.hover.is_some() || self.hover_request.is_some() || self.overlay.is_some() {
            return false;
        }
        let Some(pos) = self.mouse else { return false };
        let Some((_, line, col)) = self.point_at(pos) else { return false };
        let Some(doc) = self.active_doc() else { return false };
        // Diagnostics show even without a server answer.
        if doc.diagnostics_on(line).next().is_some() && !doc.lsp_open {
            self.hover = Some(HoverInfo { text: String::new(), line, col });
            return true;
        }
        let Some((language, uri, _)) = self.lsp_target() else { return false };
        let doc = self.active_doc().expect("checked above");
        let (l, c) = doc.buffer.utf16_point(doc.buffer.point_to_offset(line, col));
        let id = self.next_id();
        self.hover_request = Some((id, line, col));
        self.lsp.request(language, "textDocument/hover", text_document_position(&uri, Position { line: l, character: c }), RequestTag::Hover(id));
        false
    }
}

fn replay_event(key: &str, shift: bool, key_char: Option<String>) -> gpui::KeyDownEvent {
    gpui::KeyDownEvent {
        keystroke: gpui::Keystroke {
            modifiers: gpui::Modifiers { shift, ..Default::default() },
            key: key.to_lowercase(),
            key_char,
        },
        is_held: false,
    }
}

/// `ctrl+shift+p` style token to a key event. Single plain characters also carry their text.
fn replay_event_from_chord(token: &str) -> gpui::KeyDownEvent {
    let normalized = forge_core::commands::normalize_sequence(token);
    let mut modifiers = gpui::Modifiers::default();
    let mut rest = normalized.as_str();
    loop {
        if let Some(r) = rest.strip_prefix("ctrl+") {
            modifiers.control = true;
            rest = r;
        } else if let Some(r) = rest.strip_prefix("shift+") {
            modifiers.shift = true;
            rest = r;
        } else if let Some(r) = rest.strip_prefix("alt+") {
            modifiers.alt = true;
            rest = r;
        } else if let Some(r) = rest.strip_prefix("meta+") {
            modifiers.platform = true;
            rest = r;
        } else {
            break;
        }
    }
    let typed = !modifiers.control && !modifiers.alt && !modifiers.platform;
    // Like real Windows events: only single printable keys carry a character; named keys
    // (space, enter, tab, arrows) do not.
    let key_char = (rest.chars().count() == 1 && typed).then(|| rest.to_owned());
    gpui::KeyDownEvent { keystroke: gpui::Keystroke { modifiers, key: rest.to_owned(), key_char }, is_held: false }
}

/// Markdown hovers carry runs of empty lines; show at most one between paragraphs.
fn collapse_blank_lines(text: &str) -> String {
    let mut out: Vec<&str> = Vec::new();
    for line in text.lines().map(str::trim_end) {
        if line.is_empty() && out.last().is_none_or(|l| l.is_empty()) {
            continue;
        }
        out.push(line);
    }
    while out.last().is_some_and(|l| l.is_empty()) {
        out.pop();
    }
    out.join("
")
}

#[cfg(test)]
mod hover_tests {
    use super::collapse_blank_lines;

    #[test]
    fn blank_runs_collapse_and_edges_are_trimmed() {
        assert_eq!(collapse_blank_lines("

probe2



fn f()

"), "probe2

fn f()");
        assert_eq!(collapse_blank_lines("a
b"), "a
b");
        assert_eq!(collapse_blank_lines("   
"), "");
    }
}

#[cfg(test)]
mod encoding_tests {
    use super::decode_utf8;

    #[test]
    fn a_byte_order_mark_is_split_off_and_reported() {
        assert_eq!(decode_utf8(b"\xEF\xBB\xBF<?xml?>".to_vec()), Some(("<?xml?>".to_owned(), true)));
        assert_eq!(decode_utf8(b"plain".to_vec()), Some(("plain".to_owned(), false)));
        assert_eq!(decode_utf8(Vec::new()), Some((String::new(), false)));
        // Only a leading mark counts, and invalid UTF-8 is still rejected.
        assert_eq!(decode_utf8("a\u{FEFF}b".as_bytes().to_vec()), Some(("a\u{FEFF}b".to_owned(), false)));
        assert_eq!(decode_utf8(vec![0xEF, 0xBB, 0xBF, 0xFF]), None);
    }
}

#[cfg(test)]
mod untitled_tests {
    use super::Doc;

    #[test]
    fn an_untitled_document_has_a_name_but_no_file() {
        let doc = Doc::new_untitled(3);
        assert_eq!(doc.name(), "Untitled-3");
        assert!(doc.untitled && !doc.dirty && !doc.path.is_file());
    }
}
