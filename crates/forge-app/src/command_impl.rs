//! What each [`Command`] does.

use std::time::{Duration, Instant};

use forge_core::commands::Command;
use forge_core::settings::{config_dir, Settings};
use forge_lsp::{code_action_params, document_symbol_params, formatting_params, Position, Range as LspRange};
use forge_text::brackets::matching_bracket;
use forge_text::words::word_range_at;
use forge_text::{Selection, SelectionSet};
use gpui::{ClipboardItem, Context, PathPromptOptions};

use crate::lsp::RequestTag;
use crate::workspace::{Focus, Group, OverlayKind, PanelView, SidebarView, View, Workspace};

impl Workspace {
    pub fn run_command(&mut self, command: Command, extend: bool, cx: &mut Context<Self>) {
        if !command.is_movement() {
            self.recent_commands.retain(|c| *c != command);
            self.recent_commands.insert(0, command);
            self.recent_commands.truncate(12);
        }
        let style = self.indent_style();
        let auto_close = self.settings.auto_closing_brackets;
        let language = self.active_doc().and_then(|d| d.language);
        let ext_config = self.active_doc().and_then(|d| d.ext_lang.as_ref()).map(|l| l.config.clone());
        use Command::*;
        match command {
            // ---- movement ----
            CursorLeft => self.move_cursors(cx, |b, s, _| b.move_horizontal(s, -1, extend)),
            CursorRight => self.move_cursors(cx, |b, s, _| b.move_horizontal(s, 1, extend)),
            CursorWordLeft => self.move_cursors(cx, |b, s, _| b.move_word(s, false, extend)),
            CursorWordRight => self.move_cursors(cx, |b, s, _| b.move_word(s, true, extend)),
            CursorHome => self.move_cursors(cx, |b, s, _| b.smart_home(s, extend)),
            CursorEnd => self.move_cursors(cx, |b, s, _| b.move_to_line_edge(s, true, extend)),
            CursorTop => self.move_cursors(cx, |b, s, _| b.move_to_doc_edge(s, false, extend)),
            CursorBottom => self.move_cursors(cx, |b, s, _| b.move_to_doc_edge(s, true, extend)),
            CursorUp => self.move_vertical_by(cx, -1, extend),
            CursorDown => self.move_vertical_by(cx, 1, extend),
            CursorPageUp => {
                let page = self.page_lines();
                self.move_vertical_by(cx, -page, extend)
            }
            CursorPageDown => {
                let page = self.page_lines();
                self.move_vertical_by(cx, page, extend)
            }

            // ---- basic editing ----
            Undo => self.mutate(cx, |b, s| b.undo().unwrap_or_else(|| s.clone())),
            Redo => self.mutate(cx, |b, s| b.redo().unwrap_or_else(|| s.clone())),
            SelectAll => self.set_selections(cx, |b, _| SelectionSet::single(Selection { anchor: 0, head: b.len_chars() })),
            Copy => self.copy_selection(cx, false),
            Cut => self.copy_selection(cx, true),
            Paste => self.paste(cx),
            DeleteLeft => {
                self.mutate(cx, move |b, s| b.backspace(s, style, auto_close));
                self.merge_typing();
                self.refresh_completion_after_edit(None, true);
            }
            DeleteRight => self.mutate(cx, |b, s| b.delete_forward(s)),
            DeleteWordLeft => self.mutate(cx, |b, s| b.delete_word(s, false)),
            DeleteWordRight => self.mutate(cx, |b, s| b.delete_word(s, true)),
            InsertNewline => {
                let colon = language.is_some_and(|l| l.indents_after_colon());
                self.mutate(cx, move |b, s| b.newline_with_indent(s, style, colon));
                self.completion = None;
                self.signature = None;
            }
            InsertTab => {
                if !self.snippet_tab(false, cx) {
                    self.mutate(cx, move |b, s| b.insert_tab(s, style));
                }
            }
            InsertSnippet => self.open_overlay(OverlayKind::Snippets, ""),
            ExtensionsRestartHost => {
                self.restart_host();
                self.set_status("Extension host restarted");
            }
            OutdentTab | OutdentLines => {
                if !(command == OutdentTab && self.snippet_tab(true, cx)) {
                    self.mutate(cx, move |b, s| b.outdent_lines(s, style));
                }
            }
            IndentLines => self.mutate(cx, move |b, s| b.indent_lines(s, style)),
            MoveLinesUp => self.mutate(cx, |b, s| b.move_lines(s, true)),
            MoveLinesDown => self.mutate(cx, |b, s| b.move_lines(s, false)),
            CopyLinesUp => self.mutate(cx, |b, s| b.copy_lines(s, true)),
            CopyLinesDown => self.mutate(cx, |b, s| b.copy_lines(s, false)),
            DeleteLines => self.mutate(cx, |b, s| b.delete_lines(s)),
            InsertLineBelow => self.mutate(cx, |b, s| b.insert_line(s, false)),
            InsertLineAbove => self.mutate(cx, |b, s| b.insert_line(s, true)),
            JoinLines => self.mutate(cx, |b, s| b.join_lines(s)),
            ToggleLineComment => match language.and_then(|l| l.line_comment()).map(str::to_owned).or_else(|| ext_config.as_ref().and_then(|c| c.line_comment.clone())) {
                Some(token) => self.mutate(cx, move |b, s| b.toggle_line_comment(s, &token)),
                None => self.set_status("This language has no line comment"),
            },
            ToggleBlockComment => match language.and_then(|l| l.block_comment()).map(|(o, c)| (o.to_owned(), c.to_owned())).or_else(|| ext_config.as_ref().and_then(|c| c.block_comment.clone())) {
                Some((open, close)) => self.mutate(cx, move |b, s| b.toggle_block_comment(s, &open, &close)),
                None => self.set_status("This language has no block comment"),
            },
            TransformUppercase => self.mutate(cx, |b, s| b.transform_selection(s, |t| t.to_uppercase())),
            TransformLowercase => self.mutate(cx, |b, s| b.transform_selection(s, |t| t.to_lowercase())),
            SortLinesAscending => self.mutate(cx, |b, s| b.sort_lines(s, false)),
            SortLinesDescending => self.mutate(cx, |b, s| b.sort_lines(s, true)),
            TrimTrailingWhitespace => self.mutate(cx, |b, s| b.trim_trailing_whitespace(s)),
            Transpose => self.mutate(cx, |b, s| b.transpose(s)),

            // ---- selection ----
            SelectLine => self.set_selections(cx, |b, s| b.select_line(s)),
            AddSelectionToNextMatch => self.set_selections(cx, |b, s| b.add_next_occurrence(s)),
            SelectAllOccurrences => self.set_selections(cx, |b, s| b.select_all_occurrences(s)),
            AddCursorAbove => self.set_selections(cx, |b, s| b.add_cursor_vertical(s, -1)),
            AddCursorBelow => self.set_selections(cx, |b, s| b.add_cursor_vertical(s, 1)),
            ExpandSelection => self.expand_selection(cx),
            ShrinkSelection => self.shrink_selection(cx),
            CursorUndo => self.cursor_undo(cx),
            GoToBracket => self.go_to_bracket(cx),

            // ---- find, navigation, quick pickers ----
            Find => self.open_find(false),
            Replace => self.open_find(true),
            FindNext => self.find_next(true, cx),
            FindPrevious => self.find_next(false, cx),
            FindInFiles => self.open_search_panel(),
            ReplaceInFiles => {
                self.open_search_panel();
                self.search.replace_open = true;
                self.search.replace_focused = true;
            }
            GoToLine => self.open_overlay(OverlayKind::GoToLine, ""),
            QuickOpen => self.open_overlay(OverlayKind::QuickOpen, ""),
            CommandPalette => self.open_overlay(OverlayKind::Palette, ""),
            GoToSymbol => self.open_symbols(),
            NextProblem => self.jump_to_problem(true, cx),
            PreviousProblem => self.jump_to_problem(false, cx),

            // ---- files and tabs ----
            Save => self.save_active(cx),
            SaveAll => {
                for ix in 0..self.docs.len() {
                    if self.docs[ix].dirty {
                        self.write_doc(ix);
                    }
                }
            }
            CloseTab => {
                if let Some((g, vi)) = self.active_ix() {
                    self.close_view(g, vi, cx);
                }
            }
            NextTab => self.cycle_tab(1),
            PreviousTab => self.cycle_tab(-1),
            OpenTab1 | OpenTab2 | OpenTab3 | OpenTab4 | OpenTab5 | OpenTab6 | OpenTab7 | OpenTab8 | OpenTab9 => {
                let n = [OpenTab1, OpenTab2, OpenTab3, OpenTab4, OpenTab5, OpenTab6, OpenTab7, OpenTab8, OpenTab9]
                    .iter()
                    .position(|c| *c == command)
                    .unwrap_or(0);
                let group = &mut self.groups[self.focused_group];
                if !group.views.is_empty() {
                    // Alt+9 is the last tab, as in VS Code.
                    group.active = if n == 8 { group.views.len() - 1 } else { n.min(group.views.len() - 1) };
                }
            }
            NewFile => self.prompt_new_entry(false),
            NewFolder => self.prompt_new_entry(true),
            OpenFolder => self.open_folder_dialog(cx),

            // ---- workbench ----
            ToggleSidebar => self.sidebar_visible = !self.sidebar_visible,
            TogglePanel => self.panel_visible = !self.panel_visible,
            ToggleTerminal => self.toggle_terminal(),
            NewTerminal => self.new_terminal(),
            TerminalNext => self.switch_terminal(1),
            TerminalPrevious => self.switch_terminal(-1),
            TerminalKill => self.kill_terminal(self.active_terminal),
            ShowProblems => {
                self.panel_view = PanelView::Problems;
                self.panel_visible = true;
            }
            FocusExplorer => {
                self.sidebar_visible = true;
                self.sidebar_view = SidebarView::Explorer;
                self.focus = Focus::Explorer;
                if self.explorer_selected.is_none() && !self.tree.entries.is_empty() {
                    self.explorer_selected = Some(0);
                }
            }
            SplitEditor => self.split_editor(),
            FocusFirstGroup => self.focus_group(0),
            FocusSecondGroup => self.focus_group(1),
            CloseGroup => self.close_group(cx),
            ZoomIn => self.settings.font_size = (self.settings.font_size + 1.0).min(40.0),
            ZoomOut => self.settings.font_size = (self.settings.font_size - 1.0).max(8.0),
            ZoomReset => self.settings.font_size = Settings::default().font_size,
            DebugStart | DebugStepOver | DebugStepInto | DebugStepOut | DebugStop => self.debug_control(command, cx),
            DebugInstallJs => self.install_debug_adapter(false),
            DebugInstallRust => self.install_debug_adapter(true),
            ToggleBreakpoint => self.toggle_breakpoint_at_caret(),
            ClearBreakpoints => self.clear_all_breakpoints(),
            MarkdownPreview => self.toggle_markdown_preview(),
            ToggleMinimap => {
                self.settings.minimap = !self.settings.minimap;
                self.set_status(if self.settings.minimap { "Minimap on" } else { "Minimap off" });
            }
            ToggleWordWrap => {
                self.settings.word_wrap = !self.settings.word_wrap;
                self.set_status(if self.settings.word_wrap { "Word wrap on" } else { "Word wrap off" });
                if let Some((g, vi)) = self.active_ix() {
                    self.groups[g].views[vi].wrap = None;
                    self.ensure_wrap(g, vi);
                    self.reveal_caret(g, vi, false);
                }
            }
            OpenSettings => self.open_settings_page(),
            OpenSettingsJson => self.open_config_file("settings.json", &Settings::template(), cx),
            OpenKeybindings => self.open_config_file(
                "keybindings.json",
                "// Place your key bindings in this file to override the defaults.\n// Example: { \"key\": \"ctrl+shift+d\", \"command\": \"editor.action.copyLinesDownAction\" }\n// Prefix a command with - to remove a default: { \"key\": \"ctrl+d\", \"command\": \"-editor.action.addSelectionToNextFindMatch\" }\n[\n]\n",
                cx,
            ),
            ChangeTheme => self.open_overlay(OverlayKind::Theme, ""),
            ChangeIconTheme => self.open_overlay(OverlayKind::IconTheme, ""),
            ChangeProductIconTheme => self.open_overlay(OverlayKind::ProductIconTheme, ""),

            // ---- language features ----
            GoToDefinition => self.request_at("textDocument/definition", RequestTag::Definition),
            TriggerSuggest => self.request_completion(),
            TriggerParameterHints => self.request_signature(),
            ShowHover => self.show_hover_at_caret(),
            Rename => self.rename_symbol(),
            FindReferences => self.request_at("textDocument/references", RequestTag::References),
            QuickFix => self.quick_fix(),
            FormatDocument => self.format_document(false),

            // ---- source control ----
            ShowGit | GitRefresh | GitInit | GitStageAll | GitUnstageAll | GitStageFile | GitUnstageFile | GitDiscardFile | GitOpenChanges
            | GitCommit | GitCommitAll | GitGenerateMessage | GitCheckout | GitCreateBranch | GitPull | GitPush | GitSync | GitFetch | GitStash | GitStashPop
            | GitFileHistory | GitLog | GitBlame | GitRebase | GitRebaseContinue | GitRebaseSkip | GitRebaseAbort | GitMerge | GitMergeContinue | GitMergeAbort
            | GitResolveConflicts | GitCancel => self.run_git_command(command, cx),
            ToggleDiffLayout => self.toggle_diff_layout(),
            CloseOtherTabs | CloseTabsToRight | CloseSavedTabs | CloseAllTabs | PinTab | UnpinTab | CopyFilePath | CopyRelativeFilePath | RevealActiveFile
            | ShowActiveFileInExplorer => self.run_tab_command(command, cx),
            AiSetKey => self.open_overlay(OverlayKind::AiKey, ""),
            AiForgetKey => self.forget_ai_key(),
            LanguageServers => self.open_overlay(OverlayKind::LanguageServers, ""),
            ShowExtensions => self.open_extensions_view(),
            ExtensionsInstall => self.open_overlay(OverlayKind::ExtSearch, ""),
            ExtensionsVsix => self.open_overlay(OverlayKind::ExtVsix, ""),
            ExtensionsManage => self.open_overlay(OverlayKind::ExtInstalled, ""),

            // ---- folding ----
            Fold => self.fold_command(|f, line| f.fold_at(line).is_some()),
            Unfold => self.fold_command(|f, line| f.unfold_at(line)),
            ToggleFold => self.fold_command(|f, line| f.toggle_at(line)),
            FoldAll => self.fold_command(|f, _| {
                f.fold_all();
                true
            }),
            UnfoldAll => self.fold_command(|f, _| {
                f.unfold_all();
                true
            }),
        }
        cx.notify();
    }

    // ---- selection helpers ----

    /// Change selections without editing text.
    pub fn set_selections(&mut self, cx: &mut Context<Self>, f: impl FnOnce(&forge_text::Buffer, &SelectionSet) -> SelectionSet) {
        let Some((g, vi)) = self.active_ix() else { return };
        let view = &mut self.groups[g].views[vi];
        let doc = &self.docs[view.doc];
        let after = f(&doc.buffer, &view.selections);
        if after != view.selections {
            let before = std::mem::replace(&mut view.selections, after);
            view.history.push(before);
            if view.history.len() > 100 {
                view.history.remove(0);
            }
        }
        view.goal = None;
        view.expand.clear();
        self.reveal_caret(g, vi, false);
        cx.notify();
    }

    fn move_cursors(&mut self, cx: &mut Context<Self>, f: impl FnOnce(&forge_text::Buffer, &SelectionSet, &mut Option<usize>) -> SelectionSet) {
        let Some((g, vi)) = self.active_ix() else { return };
        let view = &mut self.groups[g].views[vi];
        let doc = &self.docs[view.doc];
        let mut goal = None;
        let after = f(&doc.buffer, &view.selections, &mut goal);
        if after != view.selections {
            let before = std::mem::replace(&mut view.selections, after);
            view.history.push(before);
            if view.history.len() > 100 {
                view.history.remove(0);
            }
        }
        view.goal = goal;
        view.expand.clear();
        self.completion = None;
        self.reveal_caret(g, vi, false);
        cx.notify();
    }

    fn move_vertical_by(&mut self, cx: &mut Context<Self>, delta: isize, extend: bool) {
        let Some((g, vi)) = self.active_ix() else { return };
        let view = &mut self.groups[g].views[vi];
        let doc = &self.docs[view.doc];
        let mut goal = view.goal;
        let mut after = doc.buffer.move_vertical_with_goal(&view.selections, delta, extend, &mut goal);
        if view.folds.any_collapsed() {
            let folds = &view.folds;
            after = after.map(|s| {
                let (mut line, col) = doc.buffer.offset_to_point(s.head);
                if !folds.is_hidden(line) {
                    return s;
                }
                // Skip past (moving down) or back to the header of (moving up) a folded region.
                if delta > 0 {
                    while folds.is_hidden(line) && line + 1 < doc.buffer.len_lines() {
                        line += 1;
                    }
                } else {
                    while folds.is_hidden(line) && line > 0 {
                        line -= 1;
                    }
                }
                let head = doc.buffer.point_to_offset(line, col);
                Selection { anchor: if extend { s.anchor } else { head }, head }
            });
        }
        if after != view.selections {
            let before = std::mem::replace(&mut view.selections, after);
            view.history.push(before);
            if view.history.len() > 100 {
                view.history.remove(0);
            }
        }
        view.goal = goal;
        view.expand.clear();
        self.completion = None;
        self.reveal_caret(g, vi, false);
        cx.notify();
    }

    /// Lines in one screenful, for Page Up / Page Down.
    fn page_lines(&self) -> isize {
        let height = self.active_view().map_or(0.0, |v| f32::from(v.scroll.0.borrow().base_handle.bounds().size.height));
        ((height / self.line_height()) as isize - 1).max(1)
    }

    fn cursor_undo(&mut self, cx: &mut Context<Self>) {
        let Some((g, vi)) = self.active_ix() else { return };
        let view = &mut self.groups[g].views[vi];
        if let Some(previous) = view.history.pop() {
            view.selections = previous;
            view.goal = None;
            self.reveal_caret(g, vi, false);
        }
        cx.notify();
    }

    fn expand_selection(&mut self, cx: &mut Context<Self>) {
        let Some((g, vi)) = self.active_ix() else { return };
        let view = &mut self.groups[g].views[vi];
        let doc = &self.docs[view.doc];
        let current = view.selections.clone();
        let next = current.map(|s| {
            let (start, end) = (s.start(), s.end());
            let by_syntax = doc.syntax.as_ref().and_then(|syn| syn.enclosing_range(doc.buffer.rope(), start, end));
            let (ns, ne) = by_syntax.unwrap_or_else(|| {
                // Without a syntax tree: word, then line, then everything.
                match word_range_at(doc.buffer.rope(), start) {
                    Some((ws, we)) if (ws, we) != (start, end) && ws <= start && we >= end => (ws, we),
                    _ => {
                        let (line, _) = doc.buffer.offset_to_point(start);
                        let (ls, le) = (doc.buffer.line_start(line), doc.buffer.line_end(line));
                        if (ls, le) != (start, end) && ls <= start && le >= end { (ls, le) } else { (0, doc.buffer.len_chars()) }
                    }
                }
            });
            Selection { anchor: ns, head: ne }
        });
        if next != current {
            view.expand.push(current);
            view.selections = next;
        }
        self.reveal_caret(g, vi, false);
        cx.notify();
    }

    fn shrink_selection(&mut self, cx: &mut Context<Self>) {
        let Some((g, vi)) = self.active_ix() else { return };
        let view = &mut self.groups[g].views[vi];
        if let Some(previous) = view.expand.pop() {
            view.selections = previous;
        }
        cx.notify();
    }

    fn go_to_bracket(&mut self, cx: &mut Context<Self>) {
        let Some(view) = self.active_view() else { return };
        let doc = &self.docs[view.doc];
        let head = view.selections.primary().head;
        match matching_bracket(doc.buffer.rope(), head) {
            Some((_, partner)) => self.place_caret_offset(partner),
            None => self.set_status("No matching bracket"),
        }
        cx.notify();
    }

    // ---- clipboard ----

    fn copy_selection(&mut self, cx: &mut Context<Self>, cut: bool) {
        let Some(view) = self.active_view() else { return };
        let doc = &self.docs[view.doc];
        let sels = view.selections.clone();
        let whole_lines = sels.as_slice().iter().all(|s| s.is_empty());
        let text = if whole_lines {
            // With nothing selected VS Code copies the whole line, newline included.
            let eol = doc.buffer.eol();
            doc.buffer.touched_blocks(&sels).iter().map(|(a, b)| (*a..=*b).map(|l| format!("{}{eol}", doc.buffer.line(l))).collect::<String>()).collect::<String>()
        } else {
            sels.as_slice().iter().filter(|s| !s.is_empty()).map(|s| doc.buffer.rope().slice(s.start()..s.end()).to_string()).collect::<Vec<_>>().join("\n")
        };
        if text.is_empty() {
            return;
        }
        cx.write_to_clipboard(ClipboardItem::new_string(text.clone()));
        self.clipboard_lines = whole_lines.then_some(text);
        if cut {
            if whole_lines {
                self.mutate(cx, |b, s| b.delete_lines(s));
            } else {
                self.mutate(cx, |b, s| b.insert(s, ""));
            }
        }
    }

    fn paste(&mut self, cx: &mut Context<Self>) {
        let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) else { return };
        if text.is_empty() {
            return;
        }
        // A whole-line copy pastes above the current line, as in VS Code.
        if self.clipboard_lines.as_deref() == Some(text.as_str()) {
            self.mutate(cx, move |b, s| {
                let reps: Vec<(usize, usize, String)> = b.touched_blocks(s).iter().map(|(a, _)| (b.line_start(*a), b.line_start(*a), text.clone())).collect();
                let old = s.clone();
                let mapped = reps.clone();
                b.edit_many(s, &reps, move |_, _| {
                    SelectionSet::new(old.as_slice().iter().map(|x| Selection { anchor: forge_text::map_through(&mapped, x.anchor, true), head: forge_text::map_through(&mapped, x.head, true) }).collect())
                })
            });
            return;
        }
        self.mutate(cx, move |b, s| {
            let lines: Vec<&str> = text.trim_end_matches('\n').split('\n').collect();
            // One line per cursor: give each cursor its own line.
            if s.as_slice().len() > 1 && lines.len() == s.as_slice().len() {
                let reps: Vec<(usize, usize, String)> =
                    s.as_slice().iter().zip(&lines).map(|(sel, line)| (sel.start(), sel.end(), line.trim_end_matches('\r').to_owned())).collect();
                let lens: Vec<usize> = reps.iter().map(|r| r.2.chars().count()).collect();
                return b.edit_many(s, &reps, move |_, starts| SelectionSet::new(starts.iter().zip(&lens).map(|(st, n)| Selection::caret(st + n)).collect()));
            }
            b.insert(s, &text)
        });
    }

    // ---- typing ----

    pub fn type_text(&mut self, text: &str, cx: &mut Context<Self>) {
        let auto_close = self.settings.auto_closing_brackets;
        let owned = text.to_owned();
        self.mutate(cx, move |b, s| b.type_text(s, &owned, auto_close));
        let identifier_run = text.chars().all(|c| c.is_alphanumeric() || c == '_');
        if identifier_run {
            self.merge_typing();
        } else {
            self.last_typing = None;
        }
        self.refresh_completion_after_edit(Some(text), false);
        if text == "(" || text == "," {
            self.request_signature();
        } else if text == ")" {
            self.signature = None;
        }
    }

    /// Fold this keystroke's undo step into the previous one when typing in a run.
    pub fn merge_typing(&mut self) {
        let recent = self.last_typing.is_some_and(|t| t.elapsed() < Duration::from_millis(1000));
        if let Some((g, vi)) = self.active_ix() {
            if recent {
                let doc_ix = self.groups[g].views[vi].doc;
                self.docs[doc_ix].buffer.merge_last_transactions();
            }
        }
        self.last_typing = Some(Instant::now());
    }

    /// Open, refresh or dismiss completions after an edit.
    pub fn refresh_completion_after_edit(&mut self, typed: Option<&str>, backspace: bool) {
        let Some(view) = self.active_view() else { return };
        let doc = &self.docs[view.doc];
        let caret = view.selections.primary().head;
        let rope = doc.buffer.rope();
        let identifier = typed.is_some_and(|t| !t.is_empty() && t.chars().all(|c| c.is_alphanumeric() || c == '_'));
        let start = crate::completion::identifier_start(&doc.buffer, caret);
        let after_access = start > 0 && matches!(rope.char(start - 1), '.' | ':');
        let just_typed_access = match typed {
            Some(".") => true,
            Some(":") => caret >= 2 && rope.char(caret - 2) == ':',
            _ => false,
        };
        let open = self.completion.is_some();
        if just_typed_access || (identifier && (open || after_access)) || (open && backspace) {
            self.request_completion();
        } else if open {
            self.completion = None;
        }
    }

    // ---- files, tabs, groups ----

    fn save_active(&mut self, cx: &mut Context<Self>) {
        let Some(view) = self.active_view() else { return };
        let doc_ix = view.doc;
        if self.docs[doc_ix].readonly {
            self.set_status("This document is read-only");
            return;
        }
        if self.settings.trim_trailing_whitespace {
            self.mutate(cx, |b, s| b.trim_trailing_whitespace(s));
        }
        if self.settings.format_on_save && self.docs[doc_ix].lsp_open {
            self.format_document(true);
        } else {
            self.write_doc(doc_ix);
        }
    }

    fn cycle_tab(&mut self, delta: isize) {
        let group = &mut self.groups[self.focused_group];
        let n = group.views.len() as isize;
        if n > 0 {
            group.active = (group.active as isize + delta).rem_euclid(n) as usize;
        }
        self.completion = None;
    }

    fn split_editor(&mut self) {
        if self.groups.len() >= 2 {
            self.focus_group(1 - self.focused_group.min(1));
            return;
        }
        let Some(view) = self.active_view() else { return };
        let doc = view.doc;
        let selections = view.selections.clone();
        let mut copy = View::new(doc);
        copy.selections = selections;
        self.groups.push(Group { views: vec![copy], active: 0 });
        self.focused_group = 1;
    }

    fn focus_group(&mut self, ix: usize) {
        if ix < self.groups.len() {
            self.focused_group = ix;
            self.focus = Focus::Editor;
            self.completion = None;
        }
    }

    fn close_group(&mut self, cx: &mut Context<Self>) {
        if self.groups.len() < 2 {
            return;
        }
        while let Some(view_count) = self.groups.get(self.focused_group).map(|g| g.views.len()) {
            if view_count == 0 || self.groups.len() < 2 {
                break;
            }
            let g = self.focused_group;
            let before = self.groups.len();
            self.close_view(g, view_count - 1, cx);
            if self.close_armed.is_some() || self.groups.len() < before {
                break;
            }
        }
    }

    fn open_config_file(&mut self, name: &str, template: &str, cx: &mut Context<Self>) {
        let dir = config_dir();
        let path = dir.join(name);
        if !path.exists() {
            if let Err(e) = std::fs::create_dir_all(&dir).and_then(|()| std::fs::write(&path, template)) {
                self.set_status(format!("Cannot create {}: {e}", path.display()));
                return;
            }
        }
        self.open_file(&path, None, cx);
    }

    fn open_folder_dialog(&mut self, cx: &mut Context<Self>) {
        if self.docs.iter().any(|d| d.dirty) {
            self.set_status("Save or close modified files before opening another folder");
            return;
        }
        let picked = cx.prompt_for_paths(PathPromptOptions { files: false, directories: true, multiple: false, prompt: None });
        cx.spawn(async move |this, cx| {
            if let Ok(Ok(Some(paths))) = picked.await {
                if let Some(path) = paths.into_iter().next() {
                    let _ = this.update(cx, |workspace, cx| workspace.open_folder(path, cx));
                }
            }
        })
        .detach();
    }

    pub fn open_folder(&mut self, root: std::path::PathBuf, cx: &mut Context<Self>) {
        self.save_session(true);
        self.session_ready = false;
        for doc in &self.docs {
            if let (Some(language), true) = (doc.language, doc.lsp_open) {
                self.lsp.did_close(language, &doc.path);
            }
        }
        self.docs.clear();
        self.groups = vec![Group { views: Vec::new(), active: 0 }];
        self.focused_group = 0;
        self.lsp = crate::lsp::LspManager::new(root.clone());
        self.tree = crate::tree::FileTree::new(root.clone());
        self.init_git();
        self.branches.clear();
        self.history.clear();
        self.file_index = None;
        self.explorer_selected = None;
        self.terminals.clear();
        self.search.hits.clear();
        self.set_status(format!("Opened folder {}", root.display()));
        self.restore_session(None, cx);
        cx.notify();
    }

    // ---- language features ----

    fn show_hover_at_caret(&mut self) {
        let Some(view) = self.active_view() else { return };
        let doc = &self.docs[view.doc];
        let (line, col) = doc.buffer.offset_to_point(view.selections.primary().head);
        let id = self.next_id();
        self.hover_request = Some((id, line, col));
        self.request_at("textDocument/hover", RequestTag::Hover(id));
    }

    fn rename_symbol(&mut self) {
        if self.focus == Focus::Explorer {
            self.prompt_rename_entry();
            return;
        }
        let Some(view) = self.active_view() else { return };
        let doc = &self.docs[view.doc];
        if !doc.lsp_open {
            self.set_status("Language server is not ready");
            return;
        }
        let word = word_range_at(doc.buffer.rope(), view.selections.primary().head)
            .map(|(s, e)| doc.buffer.rope().slice(s..e).to_string())
            .unwrap_or_default();
        self.open_overlay(OverlayKind::Rename, &word);
    }

    pub fn send_rename(&mut self, new_name: &str) {
        let Some((language, uri, at)) = self.lsp_target() else { return };
        self.lsp.request(language, "textDocument/rename", forge_lsp::rename_params(&uri, at, new_name), RequestTag::Rename);
    }

    fn quick_fix(&mut self) {
        let Some(view) = self.active_view() else { return };
        let doc = &self.docs[view.doc];
        let (Some(language), true) = (doc.language, doc.lsp_open) else {
            self.set_status("Language server is not ready");
            return;
        };
        let Some(uri) = forge_lsp::path_to_uri(&doc.path) else { return };
        let sel = view.selections.primary();
        let (sl, sc) = doc.buffer.utf16_point(sel.start());
        let (el, ec) = doc.buffer.utf16_point(sel.end());
        let range = LspRange { start: Position { line: sl, character: sc }, end: Position { line: el, character: ec } };
        let nearby: Vec<_> = doc.diagnostics.iter().filter(|d| d.range.start.line <= el && d.range.end.line >= sl).cloned().collect();
        self.code_actions.clear();
        self.open_overlay(OverlayKind::CodeActions, "");
        self.lsp.request(language, "textDocument/codeAction", code_action_params(&uri, range, &nearby), RequestTag::CodeActions);
    }

    pub fn format_document(&mut self, then_save: bool) {
        let Some(view) = self.active_view() else { return };
        let doc = &self.docs[view.doc];
        let (Some(language), true) = (doc.language, doc.lsp_open) else {
            if then_save {
                let ix = view.doc;
                self.write_doc(ix);
            } else {
                self.set_status("Formatting needs a running language server");
            }
            return;
        };
        let Some(uri) = forge_lsp::path_to_uri(&doc.path) else { return };
        let params = formatting_params(&uri, self.settings.tab_size, self.settings.insert_spaces);
        self.lsp.request(language, "textDocument/formatting", params, RequestTag::Format { then_save });
    }

    fn open_symbols(&mut self) {
        let Some((language, uri, _)) = self.lsp_target() else {
            self.set_status("Symbols need a running language server");
            return;
        };
        self.symbols.clear();
        self.open_overlay(OverlayKind::Symbols, "");
        self.lsp.request(language, "textDocument/documentSymbol", document_symbol_params(&uri), RequestTag::Symbols);
    }

    fn jump_to_problem(&mut self, forward: bool, cx: &mut Context<Self>) {
        let Some(view) = self.active_view() else { return };
        let doc = &self.docs[view.doc];
        let caret = doc.buffer.utf16_point(view.selections.primary().head);
        let mut problems: Vec<(usize, usize)> = doc
            .diagnostics
            .iter()
            .filter(|d| d.severity != forge_lsp::Severity::Hint)
            .map(|d| (d.range.start.line, d.range.start.character))
            .collect();
        problems.sort_unstable();
        problems.dedup();
        let target = if forward {
            problems.iter().find(|p| **p > caret).or(problems.first())
        } else {
            problems.iter().rev().find(|p| **p < caret).or(problems.last())
        };
        match target.copied() {
            Some((line, col)) => self.place_caret_utf16(line, col),
            None => self.set_status("No problems in this file"),
        }
        cx.notify();
    }

    // ---- folding ----

    fn fold_command(&mut self, f: impl FnOnce(&mut forge_core::folds::Folds, usize) -> bool) {
        let Some((g, vi)) = self.active_ix() else { return };
        self.ensure_folds(g, vi, true);
        let view = &mut self.groups[g].views[vi];
        let doc = &self.docs[view.doc];
        let (line, _) = doc.buffer.offset_to_point(view.selections.primary().head);
        if !f(&mut view.folds, line) {
            return;
        }
        // Keep the caret on a visible line.
        let mut target = line;
        while view.folds.is_hidden(target) && target > 0 {
            target -= 1;
        }
        if target != line {
            let off = doc.buffer.line_end(target);
            view.selections = SelectionSet::single(Selection::caret(off));
        }
    }

    /// Recompute fold regions when the text changed enough to matter.
    /// Rebuild the word-wrap row map if the text, the editor's width or the folds changed.
    pub fn ensure_wrap(&mut self, g: usize, vi: usize) {
        let char_w = f32::from(self.char_width).max(1.0);
        let enabled = self.settings.word_wrap;
        let view = &mut self.groups[g].views[vi];
        let doc = &self.docs[view.doc];
        let total = doc.buffer.len_lines();
        let width = f32::from(view.scroll.0.borrow().base_handle.bounds().size.width);
        // Two columns of slack for the caret, the scrollbar and font measurement rounding.
        let columns = ((width - crate::workspace::GUTTER_WIDTH - 2.0 * crate::workspace::TEXT_PAD) / char_w).floor() - 2.0;
        // Very large files are left unwrapped: the map is rebuilt on every edit.
        if !enabled || total > 50_000 || columns < 8.0 {
            view.wrap = None;
            return;
        }
        let visible = view.folds.visible_lines(total);
        let key = (doc.revision, columns as usize, total, visible.as_ref().map_or(total, Vec::len));
        if view.wrap.as_ref().is_some_and(|w| w.key == key) {
            return;
        }
        let shown = |line: usize| (line, forge_text::columns::expand_tabs(&doc.buffer.line(line)));
        let map = match &visible {
            Some(v) => crate::wrap::WrapMap::build(key, total, v.iter().map(|l| shown(*l))),
            None => crate::wrap::WrapMap::build(key, total, (0..total).map(shown)),
        };
        view.wrap = Some(map);
    }

    pub fn ensure_folds(&mut self, g: usize, vi: usize, force: bool) {
        let view = &mut self.groups[g].views[vi];
        let doc = &self.docs[view.doc];
        let lines = doc.buffer.len_lines();
        let stale = view.folds_revision == u64::MAX
            || view.folds_lines != lines
            || doc.revision.saturating_sub(view.folds_revision) > 40
            || (force && view.folds_revision != doc.revision);
        if !stale {
            return;
        }
        let regions = doc.syntax.as_ref().map(|s| s.fold_regions()).unwrap_or_default();
        view.folds.set_regions(regions);
        view.folds_revision = doc.revision;
        view.folds_lines = lines;
    }
}
