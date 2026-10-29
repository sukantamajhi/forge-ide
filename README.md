<p align="center">
  <img src="assets/forge-logo.svg" alt="Forge logo: an anvil shaped like the letter F, with a spark" width="128" height="128">
</p>

# Forge

An original, GPU-rendered code editor written in Rust on [GPUI](https://crates.io/crates/gpui),
aiming for Zed-class responsiveness and VS Code-style editing. See
[docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) for the design and for what is and is not built.

## Build and run (Windows)

Needs Rust (stable) and Visual Studio Build Tools with the C++ workload.

```bash
cargo run -p forge-app -- path/to/folder      # or path/to/file.rs
cargo test --workspace
cargo build --release -p forge-app            # LTO; slow first build
```

## What it does

- **Editing:** multi-cursor (Ctrl+D, Ctrl+Shift+L, Ctrl+Alt+Up/Down, Alt+click), word movement and
  deletion, move/copy/delete/join/insert lines, indent/outdent, line and block comments, auto-closing
  brackets and quotes with overtype and surround, auto-indent, smart Home, expand/shrink selection
  (syntax-tree based), cursor undo, sort lines, change case, trim whitespace, undo grouping, line-wise
  copy/cut/paste. Mouse: click, drag, double-click word, triple-click line, Shift+click, gutter line select.
- **Find and replace:** Ctrl+F / Ctrl+H with match case, whole word and regex (capture groups in the
  replacement), match highlighting, Find in Files (Ctrl+Shift+F, respects `.gitignore`) and Replace in Files
  (Ctrl+Shift+H: asks before changing anything, skips files with unsaved edits, keeps line endings).
- **Navigation:** Quick Open (Ctrl+P; prefix `>` commands, `:` line, `@` symbols), Command Palette
  (Ctrl+Shift+P), Go to Line (Ctrl+G), Go to Symbol (Ctrl+Shift+O), bracket matching and jump, folding,
  breadcrumbs, next/previous problem (F8), tabs, split editor (Ctrl+\\).
- **Language features** (through a language server): diagnostics, completion, hover, signature help,
  go to definition (F12 / Ctrl+click; several results open a picker), references (Shift+F12), rename (F2), quick fix (Ctrl+.), format
  document (Shift+Alt+F), outline, Problems panel.
- **Source control** (Ctrl+Shift+G; uses your `git`): changed-files view with staged / unstaged groups,
  stage, unstage, discard (untracked files go to the Recycle Bin), commit, commit all, pull, push, fetch,
  stash, AI-written commit subjects (see below), branch create and checkout, file history and commit diffs, blame for a line,
  change markers in the editor gutter that update as you type, status decorations in the explorer, and the
  branch with ahead/behind in the status bar. Everything is also in the Command Palette as `Git: ...`. Also:
  - **Side-by-side diffs.** Clicking a changed file opens a two-column diff (removed lines left, added lines right, changed
    lines paired). The **Inline** button, `Diff: Toggle Side by Side`, or `diffEditor.renderSideBySide` switch to the one-column view.
  - **Stage hunks or lines.** In a diff, **Stage Hunk** stages one block, or click lines to pick them and press **Stage
    Selected**. In a staged diff the same buttons say **Unstage**. Forge builds a small patch and runs `git apply --cached`
    (reversed for unstaging), so the rest of the file is untouched. The diff updates after every change.
  - **Merge conflicts.** A conflicted file (marked `!`) opens a page listing each conflict with the current and incoming
    sides next to each other and **Accept Current / Incoming / Both** (or all at once). When no markers are left,
    **Stage File** marks it resolved. `Git: Resolve Merge Conflicts in File` does the same for the file in the editor.
  - **Rebase and merge.** `Git: Rebase Branch...` and `Git: Merge Branch...` pick a branch. When git stops (usually for
    conflicts) the Source Control view shows a banner ("Rebase in progress (2/5)") with **Continue**, **Skip** and **Abort**
    (`Git: Continue Rebase`, `Git: Skip This Commit in the Rebase`, `Git: Abort Rebase`, `Git: Commit the Merge`,
    `Git: Abort Merge`). Interactive rebase (editing the todo list) is not supported.
  - **Logins.** A pull, push or fetch that needs a username, password, passphrase or a yes/no answer (host keys) asks in the
    editor instead of failing. Forge starts git with itself as the `GIT_ASKPASS` / `SSH_ASKPASS` helper, which asks the
    running editor over a local socket (127.0.0.1 only, with a random token), the way VS Code's git extension does.
    Passwords are shown as dots, go only to git, and Escape cancels the command. Credential helpers you already use still run first.
  - **Cancel.** While git runs, a **Cancel** button (Source Control view and status bar, or `Git: Cancel Running
    Operation`) stops the command and anything it started.
- **Markdown preview** (Ctrl+K V, or `Markdown: Open Preview to the Side`): a live preview beside the editor with headings,
  emphasis, links, lists and task lists, quotes, code blocks, tables and rules. Links are clickable (web and mail links open
  in your browser or mail app, links to project files open in Forge; `file:` and other schemes are refused), and the
  editor and preview scroll together. Raw HTML is not drawn.
- **Reopening:** Forge remembers each folder's open files (tab order, the active tab, and the caret in each) in
  `%APPDATA%\Forge\session.json` and brings them back when you open that folder again. Started with no argument, it reopens
  the last folder. Unsaved edits are not restored, and only the first editor group's tabs are.
- **Editor buttons** at the right of the tab bar: Open Preview (Markdown files only), Split Editor, and a **...** menu with
  Copy Path / Copy Relative Path / Reveal in File Explorer, Close / Close Others / Close All (files with unsaved changes stay
  open), Toggle Minimap, Go to Line, Command Palette, and the file's Git actions.
- **Tab right-click menu** (right-click any tab, not only the front one): Close, Close Others, Close to the Right, Close Saved,
  Close All, Copy Path, Copy Relative Path, Pin / Unpin, Split Right, Reveal in File Explorer, and Reveal in Explorer View.
  Bulk closes keep tabs with unsaved changes and pinned tabs, and leave the front tab in front. A pinned tab moves to the left,
  shows a pin instead of a close button (click the pin to unpin), and is not pinned again after a restart. Each item is also a
  command with VS Code's shortcut where it has one (Close Saved Ctrl+K U, Close All Ctrl+K Ctrl+W, Pin Ctrl+K Shift+Enter,
  Copy Path Shift+Alt+C, Copy Relative Path Ctrl+K Ctrl+Shift+C, Reveal in File Explorer Shift+Alt+R, Reveal in Explorer View Ctrl+K R).
- **Resizable panels:** drag the edge of the sidebar, the top edge of the bottom panel, the left edge of the Markdown
  preview, the divider between split editors, or the dividers between the Debug panel's columns. The sidebar, bottom
  panel, preview and Debug columns are remembered in `%APPDATA%\Forge\ui-state.json`; split editors reset to equal shares.
- **View:** word wrap (Alt+Z or `"editor.wordWrap": "on"`; breaks at word boundaries), a minimap (click or drag to
  scroll; `"editor.minimap.enabled"`), and a Settings page (Ctrl+,) that edits `settings.json` in place and keeps
  its comments.
- **Debugging (Python, JavaScript/Node and Rust):** F5 starts the active file under the debugger (debugpy for Python, Microsoft's
  js-debug for Node, CodeLLDB for Rust, which builds with `cargo` first and shows the program's output in a terminal tab;
  install them with `Debug: Install JavaScript Debugger` / `Debug: Install Rust Debugger (CodeLLDB)`, which verify checksums), F9 or a click in the margin sets a breakpoint,
  F10 / F11 / Shift+F11 step, Shift+F5 stops. The Debug panel shows variables, the call stack and the console.
  Uses `debugpy` from the project's `.venv`, your Python, or the copy bundled with VS Code's Python extension.
- **Workbench:** right-click menus in the editor and explorer, explorer with new file/folder, rename (F2) and delete to the Recycle Bin, integrated
  terminal (Ctrl+`; several at once, each with its own tab, New Terminal with Ctrl+Shift+`; 5000 lines of scrollback with the mouse wheel or Shift+PageUp/PageDown), bottom panel, sidebar toggle (Ctrl+B), zoom, three themes, settings and keybindings JSON.

Every command has a VS Code id and is listed in the Command Palette with its shortcut. Syntax
highlighting covers Rust, JSON, JavaScript, Python and TOML.

## Installer

```powershell
powershell -ExecutionPolicy Bypass -File installer\build.ps1     # writes dist\Forge-Setup-0.1.0.exe
```

Needs [Inno Setup 6](https://jrsoftware.org/isinfo.php) (`winget install JRSoftware.InnoSetup`). The setup installs for the
current user only (no admin prompt) to `%LOCALAPPDATA%\Programs\Forge`, adds a Start menu entry, and offers optional tasks:
**Open with Forge** in the right-click menu of files, of folders, and of the empty space in a folder window; a desktop icon;
and adding Forge to `PATH` (so `forge .` works in a terminal). Forge also appears in Windows' "Open with" list. Uninstall it
from Settings > Apps; that removes the files, menu entries and `PATH` entry (settings in `%APPDATA%\Forge` are kept).
On Windows 11 the entry is under **Show more options** (Shift+F10). The setup is unsigned, so SmartScreen may warn.
The release build is a GUI program, so no console window opens.

## Configuration

`%APPDATA%\Forge\settings.json` (Ctrl+,) and `keybindings.json` (Ctrl+K Ctrl+S); both reload when saved.

```jsonc
// settings.json
{ "editor.fontSize": 14, "editor.tabSize": 4, "editor.insertSpaces": true,
  "editor.autoClosingBrackets": "always", "workbench.colorTheme": "Forge Dark",
  "files.trimTrailingWhitespace": false, "editor.formatOnSave": false }
// keybindings.json: add or remove (prefix the command with -) bindings
[ { "key": "ctrl+shift+d", "command": "editor.action.copyLinesDownAction" },
  { "key": "ctrl+d", "command": "-editor.action.addSelectionToNextFindMatch" } ]
```

## Git progress

Commit, push, publish, pull, sync and fetch run in the background and show that they are running: the main button turns
into "Committing…" / "Pushing…" / "Publishing branch…" with a pulsing icon, counting dots and a sliding bar, the status bar
shows the same, and other git actions are ignored until it finishes. The result then appears in the status bar for ten
seconds, green on success and red on failure, naming the operation that failed.

## AI commit messages

Stage your changes, then press **Write message with AI** in the Source Control view (or run `Git: Generate Commit
Message with AI`). Forge fills the message box with a one-line subject you can edit before committing.

- **Choose who writes them:** OpenAI or Google **Gemini**. Pick one under Settings, AI Commit Messages, Provider, or run
  `AI: Write Commit Messages with OpenAI` / `AI: Write Commit Messages with Gemini`, or set `"ai.provider"` to `"openai"`
  (the default) or `"gemini"` in `settings.json`. Gemini is called through its **OpenAI-compatible endpoint**
  (`generativelanguage.googleapis.com/v1beta/openai/chat/completions`, a Bearer key), so both providers get the same request.
- The first time, Forge asks for the chosen provider's API key in a masked box (OpenAI: platform.openai.com/api-keys, Gemini:
  aistudio.google.com/apikey) and saves it once, encrypted with Windows DPAPI: `%APPDATA%\Forge\openai-key.dpapi` or
  `gemini-key.dpapi` holds no plaintext and only your Windows account can decrypt it. Each provider keeps its own key. A key is never in
  `settings.json` or the registry, and is handed to `curl` on stdin so it does not appear in a
  process list. Run `AI: Forget Saved API Key` to delete the chosen provider's key. If `OPENAI_API_KEY` (OpenAI) or
  `GEMINI_API_KEY` / `GOOGLE_API_KEY` (Gemini) is set in your environment, that is used instead.
- Only the **staged diff** and your **last 10 commit subjects** (to match your commit style) are sent to the
  chosen provider's API, and only when you ask. Lock files, `.env*`, `*.pem` and `*.key` are left out of the diff. Long diffs
  are cut at about 24,000 characters.
- Models: `"ai.model"` is the OpenAI model (default `gpt-5-nano`, the cheapest GPT text model on OpenAI's price list, $0.05 in /
  $0.40 out per 1M tokens, checked October 2026) and `"ai.geminiModel"` is the Gemini model (default `gemini-2.5-flash`). The
  Settings page edits whichever belongs to the chosen provider. Reasoning models spend part of the output budget thinking,
  so Forge keeps that small: OpenAI's GPT-5 models get minimal reasoning and a 2,000-token budget; Gemini gets 4,096 tokens,
  with thinking turned off for 2.5 Flash and Flash-Lite and set to low for models that cannot turn it off (2.5 Pro, Gemini 3).
  If a model still runs out of tokens, the message says so; pick a model that thinks less.

## Icons

The icons are VS Code's **Codicons** (`@vscode/codicons` 0.0.46-24, © Microsoft, icons under CC BY 4.0, code under MIT). The font is
embedded from `assets/fonts/codicon.ttf`; the licences are beside it. Every icon button shows its name and shortcut on hover.

## Language servers

Highlighting covers 29 languages and file kinds: Rust, JavaScript, JSX, TypeScript, TSX, JSON, HTML, CSS (also
SCSS and Less), XML (also SVG, plist, csproj...), YAML, TOML, Markdown, Python, Shell, Go, C, C++ (and `.h`), Java,
C#, PHP, Ruby, Lua, SQL, plus line-based `.env` (`.env.*`, `*.env`), INI/`.editorconfig`, Dockerfile, Makefile,
`.gitignore`-style files and diffs. Files are recognised by name first (`.env`, `Dockerfile`, `Makefile`,
`tsconfig.json`...) and then by extension.

**Language servers: Command Palette > "Language Servers: Manage and Install..."** lists every server, shows
whether it is installed, and installs the missing ones into Forge's own folder (`%APPDATA%\Forge\servers`;
nothing global is touched). Servers are looked up there first, then on `PATH`. One TypeScript server serves
`.js`, `.jsx`, `.ts` and `.tsx`.

| Installs for you | Needs you to install it (configured, used once found on `PATH`) |
| --- | --- |
| TypeScript/JavaScript (+JSX/TSX), HTML, CSS/SCSS/Less, JSON, YAML, Shell, Python (pyright), Dockerfile, PHP (intelephense) via npm; XML (LemMinX, needs Java) and Markdown (marksman) by download | rust-analyzer, gopls, clangd, jdtls, csharp-ls, ruby-lsp, lua-language-server, taplo, sqls |

When you open a folder, Forge starts the servers for the languages it finds there (up to four, only languages that
are a real part of the project, and only servers that are installed), so go to definition works as soon as you open a
file. A request made while a server is still loading waits and runs when it is ready. Turn this off with
`"lsp.autoStart": false`. A file opened on its own is rooted at its repository, or its `pyproject.toml`,
`package.json`, `go.mod` or `Cargo.toml`, or above its Python package.

Add or replace a server in `settings.json`, keyed by language id:
`"languageServers": { "go": { "command": "gopls" }, "python": { "command": "pylsp", "args": [] } }`.
Not every language has a server (`.env`, INI, Makefile, ignore files and diffs get highlighting only). A missing
server is reported in the status bar and the editor keeps working. Server start, ready and failure are logged to stderr.

## Extensions

Forge installs VS Code extensions from **Open VSX** (open-vsx.org, the registry that may be used outside
Microsoft's products) or from a `.vsix` file. It uses what they *declare* (themes, grammars, snippets, languages)
and, if Node.js is on `PATH`, **runs their code** in an extension host: a separate Node process with Forge's own
implementation of the `vscode` API. Running extension code is new and only a part of that API exists (see below);
an extension that needs more installs and activates but the missing parts throw "not supported by Forge yet". The **Extensions** tab beside Source Control (Ctrl+Shift+X) searches Open VSX as you press Enter, lists what is installed, and has Install and Uninstall buttons. Clicking an extension opens its page in an editor tab (README, license, links, Install or Uninstall); close it like any tab (the x, or Ctrl+W). Commands (Ctrl+Shift+P): **Extensions: Install from Open VSX...** (search text or an exact `publisher.name`),
**Extensions: Install from VSIX...** (a file path), **Extensions: Manage Installed...** (Enter uninstalls),
**Preferences: Color Theme**, **Insert Snippet...**. Extensions live in `%APPDATA%\Forge\extensions`
(`FORGE_CONFIG_DIR` moves everything, which is how the tests stay out of your real settings).

| An extension's contribution | In Forge |
| --- | --- |
| Product icon themes | Used: replaces Forge's interface icons (activity bar, tab bar, Git, debug buttons) with the theme's font. Pick one with **Product Icon Theme** or `workbench.productIconTheme`. Fonts must be TTF, OTF or WOFF; themes that ship only WOFF2 are refused with a message. Icons the theme does not define keep the Codicons. |
| File icon themes | Used: image icons (SVG/PNG) for files and folders, picked with **File Icon Theme** or `workbench.iconTheme` (for example Material Icon Theme). Themes built on icon fonts, and light/high-contrast variants, are not read. Without one, Forge draws its own letter badges. |
| Color themes | Used: workbench colors and token rules are mapped onto Forge's colors and token classes. Pick one in the theme list or set `workbench.colorTheme`. Semantic colors, font styles and descendant selectors (`a b`) are ignored. |
| Languages | Used for languages Forge does not bundle: file detection by extension, name or glob, the status-bar name, comment toggling from `language-configuration.json`. Bracket and auto-closing pairs are read but not yet used. First-line (shebang) detection is not. |
| TextMate grammars | Used for those languages, through a pure-Rust port of vscode-textmate (`irosashi`): colors follow the active theme. JSON grammars only (`.tmLanguage.json`); plist and YAML grammars are skipped. Files over 1 MiB stay plain. Embedded languages and injection grammars are ignored. |
| Snippets | Used: type a prefix and press Tab, Tab/Shift+Tab move between stops, Escape ends; mirrors edit together; choices insert their first option. Your own: `snippets\<language id>.json` (or `*.code-snippets` for all) next to `settings.json`. |
| Commands, views, status bar, anything in `main` | Not run. |

### The extension host

Extensions start on the events they declare (`*`, `onStartupFinished`, `onLanguage:x`, `onCommand:x`) or when one of
their contributed commands is run from the palette, and the host process is only started once something needs it.
**Extensions: Restart Extension Host** restarts it (installing or removing an extension does too). Extension code
runs with your permissions, as in VS Code. Extensions' `console.*` output goes to Forge's stderr.

| `vscode` API | In Forge |
| --- | --- |
| `commands` (register, execute), contributed commands in the palette | Works. Built-in Forge commands can be run by id from an extension. |
| `window`: messages, status bar items (clickable), output channels, active editor and selection | Works. Messages show in the status bar and have no buttons; input boxes and quick picks return `undefined`; an output channel opens as a document when the extension shows it. |
| `languages.createDiagnosticCollection` | Works: diagnostics are underlined and counted like language-server ones. |
| `workspace`: documents and their open/change/save/close events, configuration (settings.json plus the extension's defaults), `applyEdit`, `fs`, `findFiles`, folders | Works. A change event carries one edit that turns the old text into the new. File watchers never fire. |
| `Uri`, `Range`, `Position`, `Selection`, `TextEdit`, `WorkspaceEdit`, `Diagnostic`, `EventEmitter`, `Disposable`, `ExtensionContext` (state, storage paths) | Works. |
| `languages.register*Provider` (completion, hover, code actions, ...) | Accepted and ignored: Forge does not ask extensions for language features yet. This includes `vscode-languageclient` features beyond diagnostics. |
| Webviews, tree views, terminals, notebooks, debugging, tasks, authentication, `chat`, `lm` | Not supported. |

Checked with a purpose-built test extension under real Node (activation, diagnostics, status bar, commands, settings,
edits, crash isolation; automated, skipped without Node) and in a real window. **No third-party extension has been
run yet**: that is the next step and needs you to choose which ones.


## Status: what is built and what is not

**Built** (details in the sections above): multi-cursor editing, find/replace and Replace in Files, Quick Open and the
Command Palette, language-server features (with a go-to-definition picker and servers started automatically), a
VS Code-style Source Control view (stage, commit, push, publish, pull, sync, fetch, stash, branches, history, blame, AI
commit messages (OpenAI or Gemini), visible progress and results, side-by-side diffs, hunk and line staging, merge-conflict resolution,
rebase and merge with continue / skip / abort, login prompts, cancel), Markdown preview with scroll sync, minimap, word wrap, a Settings page,
right-click menus, resizable and remembered panels, editor tab-bar buttons and menu, Codicons icons with hover tooltips,
session restore, debugging for Python / Node / Rust, VS Code extensions from Open VSX (themes, languages, grammars,
snippets, and a partial `vscode` API for running code), and syntax highlighting for 29 languages.

**Not built yet:**

- Source control: interactive rebase, cherry-pick and revert, a three-way (base) view of conflicts, staging or discarding
  single hunks of a new (untracked) file or discarding a single hunk, GPG-signing prompts, submodules, worktrees.
- Workbench: multi-root workspaces, drag-and-drop of tabs or text, sticky scroll, inlay hints, semantic highlighting,
  per-language settings, restoring unsaved edits or a second editor group's tabs, mouse-driven checks of the new
  hover tooltips (verified by keyboard replay only).
- Debugging: only Python, JavaScript/Node and Rust; no conditional breakpoints or watch expressions.
- Extensions: webviews, tree views, terminals, notebooks, debugging and tasks APIs, language-feature providers, and
  VS Code's own extensions as language servers. No third-party extension has been run yet. Plan: `docs/EXTENSIONS_PLAN.md`.
- Text and files: layout assumes a monospace font, one cell per char (tabs = 4); files must be UTF-8 (a leading BOM is
  kept out of the text and written back on save) and under 64 MiB; highlighting is skipped above 4 MiB.
- Highlighting: embedded code is highlighted (`<script>`/`<style>` in HTML, fenced code in Markdown) but not above
  512 KiB, and not inside template strings, `style=""` attributes or PHP/HTML mixes. SCSS/Less use the CSS grammar and
  `.h` files are treated as C.
- Completions are plain text, and snippets are not offered in the completion popup.
- Language servers: rust-analyzer, gopls, clangd, jdtls, csharp-ls, ruby-lsp, lua-language-server, taplo and sqls are
  configured but you install them yourself; `.env`, INI, Makefile, ignore files and diffs get highlighting only.

## Testing aid

`FORGE_REPLAY="ctrl+p text:main enter"` feeds keystrokes through the normal key handler after startup
(`FORGE_REPLAY_DELAY_MS` sets the initial delay). Tokens are chords (`ctrl+shift+p`, `f2`), `text:...` to
type, and `wait:MS`.
