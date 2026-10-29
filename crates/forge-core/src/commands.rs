//! Every editor command, its VS Code id, and the default Windows keybindings.
//!
//! Chords are lowercase and ordered `ctrl+shift+alt+meta+key`; multi-step chords are
//! space separated (`ctrl+k ctrl+s`).

use serde_json::Value;

use crate::settings::strip_json_comments;

macro_rules! commands {
    ($($variant:ident, $id:literal, $title:literal, [$($key:literal),*];)*) => {
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
        pub enum Command { $($variant),* }

        impl Command {
            pub const ALL: &'static [Command] = &[$(Command::$variant),*];

            pub fn id(self) -> &'static str {
                match self { $(Command::$variant => $id),* }
            }

            pub fn title(self) -> &'static str {
                match self { $(Command::$variant => $title),* }
            }
        }

        const DEFAULT_BINDINGS: &[(&str, Command)] = &[$($(($key, Command::$variant),)*)*];
    };
}

commands! {
    CursorLeft, "cursorLeft", "Cursor Left", ["left"];
    CursorRight, "cursorRight", "Cursor Right", ["right"];
    CursorUp, "cursorUp", "Cursor Up", ["up"];
    CursorDown, "cursorDown", "Cursor Down", ["down"];
    CursorWordLeft, "cursorWordLeft", "Cursor Word Left", ["ctrl+left"];
    CursorWordRight, "cursorWordRight", "Cursor Word Right", ["ctrl+right"];
    CursorHome, "cursorHome", "Cursor Home", ["home"];
    CursorEnd, "cursorEnd", "Cursor End", ["end"];
    CursorTop, "cursorTop", "Cursor to Start of File", ["ctrl+home"];
    CursorBottom, "cursorBottom", "Cursor to End of File", ["ctrl+end"];
    CursorPageUp, "cursorPageUp", "Cursor Page Up", ["pageup"];
    CursorPageDown, "cursorPageDown", "Cursor Page Down", ["pagedown"];

    Undo, "undo", "Undo", ["ctrl+z"];
    Redo, "redo", "Redo", ["ctrl+y", "ctrl+shift+z"];
    SelectAll, "editor.action.selectAll", "Select All", ["ctrl+a"];
    Copy, "editor.action.clipboardCopyAction", "Copy", ["ctrl+c", "ctrl+insert"];
    Cut, "editor.action.clipboardCutAction", "Cut", ["ctrl+x", "shift+delete"];
    Paste, "editor.action.clipboardPasteAction", "Paste", ["ctrl+v", "shift+insert"];
    DeleteLeft, "deleteLeft", "Delete Left", ["backspace", "shift+backspace"];
    DeleteRight, "deleteRight", "Delete Right", ["delete"];
    DeleteWordLeft, "deleteWordLeft", "Delete Word Left", ["ctrl+backspace"];
    DeleteWordRight, "deleteWordRight", "Delete Word Right", ["ctrl+delete"];
    InsertNewline, "editor.action.insertNewline", "Insert Newline", ["enter", "shift+enter"];
    InsertTab, "tab", "Insert Tab", ["tab"];
    OutdentTab, "outdent", "Outdent", ["shift+tab"];
    MoveLinesUp, "editor.action.moveLinesUpAction", "Move Line Up", ["alt+up"];
    MoveLinesDown, "editor.action.moveLinesDownAction", "Move Line Down", ["alt+down"];
    CopyLinesUp, "editor.action.copyLinesUpAction", "Copy Line Up", ["shift+alt+up"];
    CopyLinesDown, "editor.action.copyLinesDownAction", "Copy Line Down", ["shift+alt+down"];
    DeleteLines, "editor.action.deleteLines", "Delete Line", ["ctrl+shift+k"];
    InsertLineBelow, "editor.action.insertLineAfter", "Insert Line Below", ["ctrl+enter"];
    InsertLineAbove, "editor.action.insertLineBefore", "Insert Line Above", ["ctrl+shift+enter"];
    JoinLines, "editor.action.joinLines", "Join Lines", [];
    IndentLines, "editor.action.indentLines", "Indent Line", ["ctrl+]"];
    OutdentLines, "editor.action.outdentLines", "Outdent Line", ["ctrl+["];
    ToggleLineComment, "editor.action.commentLine", "Toggle Line Comment", ["ctrl+/"];
    ToggleBlockComment, "editor.action.blockComment", "Toggle Block Comment", ["shift+alt+a"];
    SelectLine, "expandLineSelection", "Expand Line Selection", ["ctrl+l"];
    AddSelectionToNextMatch, "editor.action.addSelectionToNextFindMatch", "Add Selection To Next Find Match", ["ctrl+d"];
    SelectAllOccurrences, "editor.action.selectHighlights", "Select All Occurrences of Find Match", ["ctrl+shift+l"];
    ExpandSelection, "editor.action.smartSelect.expand", "Expand Selection", ["shift+alt+right"];
    ShrinkSelection, "editor.action.smartSelect.shrink", "Shrink Selection", ["shift+alt+left"];
    CursorUndo, "cursorUndo", "Cursor Undo", ["ctrl+u"];
    AddCursorAbove, "editor.action.insertCursorAbove", "Add Cursor Above", ["ctrl+alt+up"];
    AddCursorBelow, "editor.action.insertCursorBelow", "Add Cursor Below", ["ctrl+alt+down"];
    TransformUppercase, "editor.action.transformToUppercase", "Transform to Uppercase", [];
    TransformLowercase, "editor.action.transformToLowercase", "Transform to Lowercase", [];
    SortLinesAscending, "editor.action.sortLinesAscending", "Sort Lines Ascending", [];
    SortLinesDescending, "editor.action.sortLinesDescending", "Sort Lines Descending", [];
    TrimTrailingWhitespace, "editor.action.trimTrailingWhitespace", "Trim Trailing Whitespace", ["ctrl+k ctrl+x"];
    Transpose, "editor.action.transposeLetters", "Transpose Letters", [];

    Find, "actions.find", "Find", ["ctrl+f"];
    Replace, "editor.action.startFindReplace", "Replace", ["ctrl+h"];
    FindNext, "editor.action.nextMatchFindAction", "Find Next", ["f3"];
    FindPrevious, "editor.action.previousMatchFindAction", "Find Previous", ["shift+f3"];
    FindInFiles, "workbench.action.findInFiles", "Find in Files", ["ctrl+shift+f"];
    ReplaceInFiles, "workbench.action.replaceInFiles", "Replace in Files", ["ctrl+shift+h"];
    GoToLine, "workbench.action.gotoLine", "Go to Line/Column...", ["ctrl+g"];
    QuickOpen, "workbench.action.quickOpen", "Go to File...", ["ctrl+p"];
    CommandPalette, "workbench.action.showCommands", "Show All Commands", ["ctrl+shift+p", "f1"];
    GoToSymbol, "workbench.action.gotoSymbol", "Go to Symbol in Editor...", ["ctrl+shift+o"];
    GoToBracket, "editor.action.jumpToBracket", "Go to Bracket", ["ctrl+shift+\\"];
    NextProblem, "editor.action.marker.next", "Go to Next Problem", ["f8"];
    PreviousProblem, "editor.action.marker.prev", "Go to Previous Problem", ["shift+f8"];

    Save, "workbench.action.files.save", "Save", ["ctrl+s"];
    SaveAll, "workbench.action.files.saveAll", "Save All", ["ctrl+k s"];
    CloseTab, "workbench.action.closeActiveEditor", "Close Editor", ["ctrl+w", "ctrl+f4"];
    NextTab, "workbench.action.nextEditor", "Open Next Editor", ["ctrl+pagedown", "ctrl+tab"];
    PreviousTab, "workbench.action.previousEditor", "Open Previous Editor", ["ctrl+pageup", "ctrl+shift+tab"];
    OpenTab1, "workbench.action.openEditorAtIndex1", "Open Editor 1", ["alt+1"];
    OpenTab2, "workbench.action.openEditorAtIndex2", "Open Editor 2", ["alt+2"];
    OpenTab3, "workbench.action.openEditorAtIndex3", "Open Editor 3", ["alt+3"];
    OpenTab4, "workbench.action.openEditorAtIndex4", "Open Editor 4", ["alt+4"];
    OpenTab5, "workbench.action.openEditorAtIndex5", "Open Editor 5", ["alt+5"];
    OpenTab6, "workbench.action.openEditorAtIndex6", "Open Editor 6", ["alt+6"];
    OpenTab7, "workbench.action.openEditorAtIndex7", "Open Editor 7", ["alt+7"];
    OpenTab8, "workbench.action.openEditorAtIndex8", "Open Editor 8", ["alt+8"];
    OpenTab9, "workbench.action.openEditorAtIndex9", "Open Last Editor", ["alt+9"];
    NewFile, "workbench.action.files.newFile", "New File...", ["ctrl+n"];
    NewFolder, "workbench.action.files.newFolder", "New Folder...", [];
    OpenFolder, "workbench.action.files.openFolder", "Open Folder...", ["ctrl+k ctrl+o"];

    ToggleSidebar, "workbench.action.toggleSidebarVisibility", "Toggle Primary Side Bar Visibility", ["ctrl+b"];
    TogglePanel, "workbench.action.togglePanel", "Toggle Panel Visibility", ["ctrl+j"];
    ToggleTerminal, "workbench.action.terminal.toggleTerminal", "Toggle Terminal", ["ctrl+`"];
    NewTerminal, "workbench.action.terminal.new", "Create New Terminal", ["ctrl+shift+`"];
    TerminalNext, "workbench.action.terminal.focusNext", "Terminal: Focus Next Terminal", [];
    TerminalPrevious, "workbench.action.terminal.focusPrevious", "Terminal: Focus Previous Terminal", [];
    TerminalKill, "workbench.action.terminal.kill", "Terminal: Kill the Active Terminal", [];
    ShowProblems, "workbench.actions.view.problems", "Show Problems", ["ctrl+shift+m"];
    FocusExplorer, "workbench.view.explorer", "Show Explorer", ["ctrl+shift+e"];
    SplitEditor, "workbench.action.splitEditor", "Split Editor", ["ctrl+\\"];
    FocusFirstGroup, "workbench.action.focusFirstEditorGroup", "Focus First Editor Group", ["ctrl+1"];
    FocusSecondGroup, "workbench.action.focusSecondEditorGroup", "Focus Second Editor Group", ["ctrl+2"];
    CloseGroup, "workbench.action.closeEditorsInGroup", "Close Editor Group", [];
    ZoomIn, "workbench.action.zoomIn", "Zoom In", ["ctrl+=", "ctrl+shift+="];
    ZoomOut, "workbench.action.zoomOut", "Zoom Out", ["ctrl+-"];
    ZoomReset, "workbench.action.zoomReset", "Reset Zoom", ["ctrl+numpad0"];
    ToggleWordWrap, "editor.action.toggleWordWrap", "Toggle Word Wrap", ["alt+z"];
    MarkdownPreview, "markdown.showPreviewToSide", "Markdown: Open Preview to the Side", ["ctrl+k v"];
    ToggleMinimap, "editor.action.toggleMinimap", "View: Toggle Minimap", [];

    OpenSettings, "workbench.action.openSettings", "Open Settings", ["ctrl+,"];
    OpenSettingsJson, "workbench.action.openSettingsJson", "Open User Settings (JSON)", [];
    OpenKeybindings, "workbench.action.openGlobalKeybindingsFile", "Open Keyboard Shortcuts (JSON)", ["ctrl+k ctrl+s"];
    ChangeTheme, "workbench.action.selectTheme", "Color Theme", ["ctrl+k ctrl+t"];
    ChangeIconTheme, "workbench.action.selectIconTheme", "File Icon Theme", [];
    ChangeProductIconTheme, "workbench.action.selectProductIconTheme", "Product Icon Theme", [];

    GoToDefinition, "editor.action.revealDefinition", "Go to Definition", ["f12"];
    DebugStart, "workbench.action.debug.start", "Debug: Start Debugging / Continue", ["f5"];
    DebugStepOver, "workbench.action.debug.stepOver", "Debug: Step Over", ["f10"];
    DebugStepInto, "workbench.action.debug.stepInto", "Debug: Step Into", ["f11"];
    DebugStepOut, "workbench.action.debug.stepOut", "Debug: Step Out", ["shift+f11"];
    DebugStop, "workbench.action.debug.stop", "Debug: Stop", ["shift+f5"];
    DebugInstallJs, "forge.debug.installJavaScript", "Debug: Install JavaScript Debugger", [];
    DebugInstallRust, "forge.debug.installRust", "Debug: Install Rust Debugger (CodeLLDB)", [];
    ToggleBreakpoint, "editor.debug.action.toggleBreakpoint", "Debug: Toggle Breakpoint", ["f9"];
    ClearBreakpoints, "workbench.debug.viewlet.action.removeAllBreakpoints", "Debug: Remove All Breakpoints", [];
    TriggerSuggest, "editor.action.triggerSuggest", "Trigger Suggest", ["ctrl+space"];
    TriggerParameterHints, "editor.action.triggerParameterHints", "Trigger Parameter Hints", ["ctrl+shift+space"];
    ShowHover, "editor.action.showHover", "Show or Focus Hover", ["ctrl+k ctrl+i"];
    Rename, "editor.action.rename", "Rename Symbol", ["f2"];
    FindReferences, "editor.action.goToReferences", "Go to References", ["shift+f12"];
    QuickFix, "editor.action.quickFix", "Quick Fix...", ["ctrl+."];
    FormatDocument, "editor.action.formatDocument", "Format Document", ["shift+alt+f"];

    Fold, "editor.fold", "Fold", ["ctrl+shift+["];
    Unfold, "editor.unfold", "Unfold", ["ctrl+shift+]"];
    ToggleFold, "editor.toggleFold", "Toggle Fold", ["ctrl+k ctrl+l"];
    FoldAll, "editor.foldAll", "Fold All", ["ctrl+k ctrl+0"];
    UnfoldAll, "editor.unfoldAll", "Unfold All", ["ctrl+k ctrl+j"];
    ShowGit, "workbench.view.scm", "Show Source Control", ["ctrl+shift+g"];
    GitRefresh, "git.refresh", "Git: Refresh", [];
    GitInit, "git.init", "Git: Initialize Repository", [];
    GitStageAll, "git.stageAll", "Git: Stage All Changes", [];
    GitUnstageAll, "git.unstageAll", "Git: Unstage All Changes", [];
    GitStageFile, "git.stage", "Git: Stage Changes in Current File", [];
    GitUnstageFile, "git.unstage", "Git: Unstage Current File", [];
    GitDiscardFile, "git.clean", "Git: Discard Changes in Current File", [];
    GitOpenChanges, "git.openChange", "Git: Open Changes of Current File", [];
    GitCommit, "git.commit", "Git: Commit", [];
    GitCommitAll, "git.commitAll", "Git: Commit All", [];
    GitGenerateMessage, "forge.git.generateCommitMessage", "Git: Generate Commit Message with AI", [];
    AiSetKey, "forge.ai.setApiKey", "AI: Enter OpenAI API Key...", [];
    AiForgetKey, "forge.ai.forgetApiKey", "AI: Forget Saved OpenAI API Key", [];
    GitCheckout, "git.checkout", "Git: Checkout to...", [];
    GitCreateBranch, "git.branch", "Git: Create Branch...", [];
    GitPull, "git.pull", "Git: Pull", [];
    GitPush, "git.push", "Git: Push", [];
    GitSync, "git.sync", "Git: Sync Changes (Pull, then Push)", [];
    GitFetch, "git.fetch", "Git: Fetch", [];
    GitStash, "git.stash", "Git: Stash (Include Untracked)", [];
    GitStashPop, "git.stashPop", "Git: Pop Latest Stash", [];
    GitFileHistory, "git.viewFileHistory", "Git: View File History", [];
    GitLog, "git.viewHistory", "Git: View History", [];
    GitBlame, "git.blame", "Git: Show Blame for Current Line", [];
    LanguageServers, "forge.languageServers", "Language Servers: Manage and Install...", [];
    ShowExtensions, "workbench.view.extensions", "View: Show Extensions", ["ctrl+shift+x"];
    ExtensionsInstall, "forge.extensions.install", "Extensions: Install from Open VSX...", [];
    ExtensionsVsix, "forge.extensions.vsix", "Extensions: Install from VSIX...", [];
    ExtensionsManage, "forge.extensions.manage", "Extensions: Manage Installed...", [];
    InsertSnippet, "editor.action.insertSnippet", "Insert Snippet...", [];
    ExtensionsRestartHost, "forge.extensions.restartHost", "Extensions: Restart Extension Host", [];
}

impl Command {
    pub fn from_id(id: &str) -> Option<Command> {
        Command::ALL.iter().copied().find(|c| c.id() == id)
    }

    /// Cursor movements, which extend the selection when Shift is added to their key.
    pub fn is_movement(self) -> bool {
        use Command::*;
        matches!(
            self,
            CursorLeft | CursorRight | CursorUp | CursorDown | CursorWordLeft | CursorWordRight | CursorHome | CursorEnd
                | CursorTop | CursorBottom | CursorPageUp | CursorPageDown
        )
    }

    /// Commands worth listing in the palette; raw key actions are left out.
    pub fn in_palette(self) -> bool {
        use Command::*;
        !self.is_movement()
            && !matches!(
                self,
                DeleteLeft | DeleteRight | InsertNewline | InsertTab | OutdentTab | OpenTab1 | OpenTab2 | OpenTab3 | OpenTab4
                    | OpenTab5 | OpenTab6 | OpenTab7 | OpenTab8 | OpenTab9
            )
    }
}

/// Build a normalized chord from modifier flags and a GPUI key name.
pub fn chord(ctrl: bool, shift: bool, alt: bool, meta: bool, key: &str) -> String {
    let mut out = String::new();
    for (on, name) in [(ctrl, "ctrl"), (shift, "shift"), (alt, "alt"), (meta, "meta")] {
        if on {
            out.push_str(name);
            out.push('+');
        }
    }
    out.push_str(&key.to_lowercase());
    out
}

/// The chord with its `shift+` modifier removed, or `None` if it has no Shift.
fn without_shift(chord: &str) -> Option<String> {
    let mut rest = chord;
    let mut kept = String::new();
    let mut had_shift = false;
    'modifiers: loop {
        for m in ["ctrl+", "shift+", "alt+", "meta+"] {
            if let Some(after) = rest.strip_prefix(m) {
                // "shift+" followed by nothing is the key itself, not a modifier.
                if after.is_empty() {
                    break 'modifiers;
                }
                if m == "shift+" {
                    had_shift = true;
                } else {
                    kept.push_str(m);
                }
                rest = after;
                continue 'modifiers;
            }
        }
        break;
    }
    had_shift.then(|| format!("{kept}{rest}"))
}

/// Normalize a user-written chord sequence: lowercase, canonical modifier order,
/// `cmd`/`win`/`super` treated as `meta`, `control` as `ctrl`, `option` as `alt`.
pub fn normalize_sequence(text: &str) -> String {
    text.split_whitespace()
        .map(|part| {
            let lower = part.to_lowercase();
            // A trailing "+" is the plus key itself ("ctrl++"); keep it as the key.
            let (mods, key) = match lower.strip_suffix('+') {
                Some(head) if !head.is_empty() && !head.ends_with('+') => (head.to_owned(), "+".to_owned()),
                Some(head) => (head.trim_end_matches('+').to_owned(), "+".to_owned()),
                None => match lower.rsplit_once('+') {
                    Some((m, k)) => (m.to_owned(), k.to_owned()),
                    None => (String::new(), lower.clone()),
                },
            };
            let (mut ctrl, mut shift, mut alt, mut meta) = (false, false, false, false);
            for m in mods.split('+').filter(|m| !m.is_empty()) {
                match m {
                    "ctrl" | "control" => ctrl = true,
                    "shift" => shift = true,
                    "alt" | "option" => alt = true,
                    "meta" | "cmd" | "win" | "super" => meta = true,
                    _ => {}
                }
            }
            chord(ctrl, shift, alt, meta, &key)
        })
        .collect::<Vec<_>>()
        .join(" ")
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Resolution {
    /// Run `command`; `extend` is set when Shift turned a movement into a selection.
    Run { command: Command, extend: bool },
    /// The keys so far begin a longer chord; the caller keeps them as the prefix.
    Pending(String),
    NoMatch,
}

#[derive(Clone, Debug)]
pub struct Keymap {
    bindings: Vec<(String, Command)>,
    /// Keys the user bound in `keybindings.json`; shown in preference to defaults.
    user_keys: std::collections::HashSet<String>,
}

impl Default for Keymap {
    fn default() -> Self {
        Self { bindings: DEFAULT_BINDINGS.iter().map(|(k, c)| ((*k).to_owned(), *c)).collect(), user_keys: Default::default() }
    }
}

impl Keymap {
    /// Apply a user `keybindings.json`: `{ "key": "...", "command": "id" }` entries add
    /// bindings (taking priority) and a `-id` command removes that binding. Returns warnings.
    pub fn apply_user(&mut self, text: &str) -> Vec<String> {
        let mut warnings = Vec::new();
        let Ok(Value::Array(entries)) = serde_json::from_str::<Value>(&strip_json_comments(text)) else {
            warnings.push("keybindings.json must be a JSON array".to_owned());
            return warnings;
        };
        let mut added = Vec::new();
        for entry in entries {
            let (Some(key), Some(command)) = (entry.get("key").and_then(Value::as_str), entry.get("command").and_then(Value::as_str)) else {
                warnings.push("skipped an entry without \"key\" and \"command\"".to_owned());
                continue;
            };
            let key = normalize_sequence(key);
            if let Some(id) = command.strip_prefix('-') {
                match Command::from_id(id) {
                    Some(cmd) => self.bindings.retain(|(k, c)| !(*k == key && *c == cmd)),
                    None => warnings.push(format!("unknown command \"{id}\"")),
                }
            } else {
                match Command::from_id(command) {
                    Some(cmd) => added.push((key, cmd)),
                    None => warnings.push(format!("unknown command \"{command}\"")),
                }
            }
        }
        // A user binding for a key replaces any default on that key.
        for (key, cmd) in added {
            self.bindings.retain(|(k, _)| *k != key);
            self.user_keys.insert(key.clone());
            self.bindings.push((key, cmd));
        }
        warnings
    }

    /// Resolve `chord` given the keys already pressed in a multi-step sequence.
    pub fn resolve(&self, pending: &str, chord: &str) -> Resolution {
        let sequence = if pending.is_empty() { chord.to_owned() } else { format!("{pending} {chord}") };
        if let Some((_, command)) = self.bindings.iter().rev().find(|(k, _)| *k == sequence) {
            return Resolution::Run { command: *command, extend: false };
        }
        let prefix = format!("{sequence} ");
        if self.bindings.iter().any(|(k, _)| k.starts_with(&prefix)) {
            return Resolution::Pending(sequence);
        }
        // Shift + a movement key extends the selection.
        if pending.is_empty() {
            if let Some(plain) = without_shift(chord) {
                if let Some((_, command)) = self.bindings.iter().rev().find(|(k, _)| *k == plain) {
                    if command.is_movement() {
                        return Resolution::Run { command: *command, extend: true };
                    }
                }
            }
        }
        Resolution::NoMatch
    }

    /// Display form of a command's first binding, e.g. `Ctrl+Shift+P`.
    pub fn shortcut_label(&self, command: Command) -> Option<String> {
        let mut for_command = self.bindings.iter().filter(|(_, c)| *c == command);
        let (key, _) = self.bindings.iter().find(|(k, c)| *c == command && self.user_keys.contains(k)).or_else(|| for_command.next())?;
        Some(
            key.split(' ')
                .map(|part| {
                    part.split('+')
                        .map(|p| match p {
                            "ctrl" => "Ctrl".to_owned(),
                            "shift" => "Shift".to_owned(),
                            "alt" => "Alt".to_owned(),
                            "meta" => "Win".to_owned(),
                            k if k.chars().count() == 1 => k.to_uppercase(),
                            k => {
                                let mut chars = k.chars();
                                chars.next().map(|f| f.to_uppercase().collect::<String>() + chars.as_str()).unwrap_or_default()
                            }
                        })
                        .collect::<Vec<_>>()
                        .join("+")
                })
                .collect::<Vec<_>>()
                .join(" "),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn ids_and_titles_are_unique_and_ids_round_trip() {
        let mut ids = HashSet::new();
        for c in Command::ALL {
            assert!(ids.insert(c.id()), "duplicate id {}", c.id());
            assert!(!c.title().is_empty());
            assert_eq!(Command::from_id(c.id()), Some(*c));
        }
        assert!(Command::from_id("nope").is_none());
        assert!(Command::ALL.len() > 100, "{} commands", Command::ALL.len());
    }

    #[test]
    fn default_bindings_are_normalized_and_unambiguous() {
        let mut seen = HashSet::new();
        for (key, _) in DEFAULT_BINDINGS {
            assert_eq!(normalize_sequence(key), *key, "binding {key:?} is not in canonical form");
            assert!(seen.insert(*key), "key {key} is bound twice");
        }
        // No single chord may also be the prefix of a longer one (ctrl+k alone would shadow).
        for (a, _) in DEFAULT_BINDINGS {
            for (b, _) in DEFAULT_BINDINGS {
                assert!(!b.starts_with(&format!("{a} ")), "{a} is both a binding and a prefix of {b}");
            }
        }
    }

    #[test]
    fn vs_code_headline_shortcuts_resolve() {
        let km = Keymap::default();
        let run = |c: &str| match km.resolve("", c) {
            Resolution::Run { command, .. } => command,
            other => panic!("{c}: {other:?}"),
        };
        assert_eq!(run("ctrl+shift+p"), Command::CommandPalette);
        assert_eq!(run("ctrl+p"), Command::QuickOpen);
        assert_eq!(run("ctrl+shift+k"), Command::DeleteLines);
        assert_eq!(run("alt+up"), Command::MoveLinesUp);
        assert_eq!(run("shift+alt+down"), Command::CopyLinesDown);
        assert_eq!(run("ctrl+d"), Command::AddSelectionToNextMatch);
        assert_eq!(run("ctrl+/"), Command::ToggleLineComment);
        assert_eq!(run("f12"), Command::GoToDefinition);
        assert_eq!(run("f2"), Command::Rename);
        assert_eq!(run("ctrl+`"), Command::ToggleTerminal);
    }

    #[test]
    fn multi_step_chords_pend_then_resolve_and_wrong_second_key_fails() {
        let km = Keymap::default();
        assert_eq!(km.resolve("", "ctrl+k"), Resolution::Pending("ctrl+k".to_owned()));
        assert_eq!(km.resolve("ctrl+k", "ctrl+s"), Resolution::Run { command: Command::OpenKeybindings, extend: false });
        assert_eq!(km.resolve("ctrl+k", "ctrl+0"), Resolution::Run { command: Command::FoldAll, extend: false });
        assert_eq!(km.resolve("ctrl+k", "q"), Resolution::NoMatch);
    }

    #[test]
    fn shift_extends_movement_but_not_other_commands() {
        let km = Keymap::default();
        assert_eq!(km.resolve("", "shift+left"), Resolution::Run { command: Command::CursorLeft, extend: true });
        assert_eq!(km.resolve("", "ctrl+shift+left"), Resolution::Run { command: Command::CursorWordLeft, extend: true });
        assert_eq!(km.resolve("", "shift+pagedown"), Resolution::Run { command: Command::CursorPageDown, extend: true });
        // An explicit binding wins over the shift fallback.
        assert_eq!(km.resolve("", "shift+alt+up"), Resolution::Run { command: Command::CopyLinesUp, extend: false });
        // Shift + a non-movement command is not a fallback.
        assert_eq!(km.resolve("", "shift+backspace"), Resolution::Run { command: Command::DeleteLeft, extend: false });
        assert_eq!(km.resolve("", "shift+q"), Resolution::NoMatch);
    }

    #[test]
    fn chords_normalize_regardless_of_how_they_were_typed() {
        assert_eq!(chord(true, true, false, false, "P"), "ctrl+shift+p");
        assert_eq!(normalize_sequence("Shift+Ctrl+P"), "ctrl+shift+p");
        assert_eq!(normalize_sequence("Cmd+K  Control+S"), "meta+k ctrl+s");
        assert_eq!(normalize_sequence("ctrl++"), "ctrl++");
        assert_eq!(normalize_sequence("ctrl+shift+="), "ctrl+shift+=");
    }

    #[test]
    fn user_bindings_override_remove_and_report_problems() {
        let mut km = Keymap::default();
        let warnings = km.apply_user(
            r#"[
                // swap the palette to a new key
                { "key": "Ctrl+Shift+O", "command": "workbench.action.showCommands" },
                { "key": "ctrl+shift+p", "command": "-workbench.action.showCommands" },
                { "key": "ctrl+q", "command": "not.a.command" },
                { "key": "ctrl+9" }
            ]"#,
        );
        assert_eq!(warnings.len(), 2, "{warnings:?}");
        assert_eq!(km.resolve("", "ctrl+shift+o"), Resolution::Run { command: Command::CommandPalette, extend: false });
        assert_eq!(km.resolve("", "ctrl+shift+p"), Resolution::NoMatch);
        assert_eq!(km.resolve("", "f1"), Resolution::Run { command: Command::CommandPalette, extend: false });

        let mut bad = Keymap::default();
        assert_eq!(bad.apply_user("{}").len(), 1);
        assert_eq!(bad.resolve("", "ctrl+p"), Resolution::Run { command: Command::QuickOpen, extend: false });
    }

    #[test]
    fn shortcut_labels_read_like_vs_code() {
        let km = Keymap::default();
        assert_eq!(km.shortcut_label(Command::CommandPalette).as_deref(), Some("Ctrl+Shift+P"));
        assert_eq!(km.shortcut_label(Command::OpenKeybindings).as_deref(), Some("Ctrl+K Ctrl+S"));
        assert_eq!(km.shortcut_label(Command::MoveLinesUp).as_deref(), Some("Alt+Up"));
        assert_eq!(km.shortcut_label(Command::JoinLines), None);
    }

    #[test]
    fn palette_omits_raw_key_actions() {
        assert!(Command::CommandPalette.in_palette());
        assert!(!Command::CursorLeft.in_palette());
        assert!(!Command::InsertNewline.in_palette());
    }
}
