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
  replacement), match highlighting, Find in Files (Ctrl+Shift+F, respects `.gitignore`).
- **Navigation:** Quick Open (Ctrl+P; prefix `>` commands, `:` line, `@` symbols), Command Palette
  (Ctrl+Shift+P), Go to Line (Ctrl+G), Go to Symbol (Ctrl+Shift+O), bracket matching and jump, folding,
  breadcrumbs, next/previous problem (F8), tabs, split editor (Ctrl+\\).
- **Language features** (through a language server): diagnostics, completion, hover, signature help,
  go to definition (F12 / Ctrl+click), references (Shift+F12), rename (F2), quick fix (Ctrl+.), format
  document (Shift+Alt+F), outline, Problems panel.
- **Source control** (Ctrl+Shift+G; uses your `git`): changed-files view with staged / unstaged groups,
  stage, unstage, discard (untracked files go to the Recycle Bin), commit, commit all, pull, push, fetch,
  stash, branch create and checkout, file history and commit diffs, blame for a line, unified diff tabs,
  change markers in the editor gutter that update as you type, status decorations in the explorer, and the
  branch with ahead/behind in the status bar. Everything is also in the Command Palette as `Git: ...`.
- **Workbench:** explorer with new file/folder, rename (F2) and delete to the Recycle Bin, integrated
  terminal (Ctrl+`), bottom panel, sidebar toggle (Ctrl+B), zoom, three themes, settings and keybindings JSON.

Every command has a VS Code id and is listed in the Command Palette with its shortcut. Syntax
highlighting covers Rust, JSON, JavaScript, Python and TOML.

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

Add or replace a server in `settings.json`, keyed by language id:
`"languageServers": { "go": { "command": "gopls" }, "python": { "command": "pylsp", "args": [] } }`.
Not every language has a server (`.env`, INI, Makefile, ignore files and diffs get highlighting only). A missing
server is reported in the status bar and the editor keeps working. Server start, ready and failure are logged to stderr.

## Extensions

Forge installs VS Code extensions from **Open VSX** (open-vsx.org, the registry that may be used outside
Microsoft's products) or from a `.vsix` file. It uses what they *declare* (themes, grammars, snippets, languages)
and, if Node.js is on `PATH`, **runs their code** in an extension host: a separate Node process with Forge's own
implementation of the `vscode` API. Running extension code is new and only a part of that API exists (see below);
an extension that needs more installs and activates but the missing parts throw "not supported by Forge yet". Commands (Ctrl+Shift+P): **Extensions: Install from Open VSX...** (search text or an exact `publisher.name`),
**Extensions: Install from VSIX...** (a file path), **Extensions: Manage Installed...** (Enter uninstalls),
**Preferences: Color Theme**, **Insert Snippet...**. Extensions live in `%APPDATA%\Forge\extensions`
(`FORGE_CONFIG_DIR` moves everything, which is how the tests stay out of your real settings).

| An extension's contribution | In Forge |
| --- | --- |
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


## Not built yet

Minimap, word wrap, replace in files, side-by-side diffs, staging individual hunks or lines, merge-conflict
resolution, rebase, git credentials prompts (commands fail instead of prompting), debugger, multi-root
workspaces, a settings UI, right-click context menus, drag-and-drop of tabs or text, sticky scroll, inlay
hints, semantic highlighting, per-language settings, and **running extension code** (see Extensions). Text layout assumes a monospace font, one cell per char (tabs = 4), no soft wrap.
Several definitions are not offered as a list (the first is taken); completions are plain text, and snippets are not offered in the completion popup;
files must be UTF-8 (a leading BOM is kept out of the text and written back on save) and under 64 MiB; syntax
highlighting is skipped above 4 MiB. Embedded code is highlighted (`<script>` and `<style>` in HTML, fenced code in
Markdown, nested such as HTML in a Markdown fence) but embedded regions over 512 KiB are left plain, and there is no
highlighting inside template strings, `style=""` attributes or PHP/HTML mixes. SCSS/Less use the CSS grammar, and
`.h` files are treated as C. VS Code's own extensions cannot supply language servers yet, so "every VS Code language
server" is not covered: only the servers above. The plan for extension compatibility is in `docs/EXTENSIONS_PLAN.md`.

## Testing aid

`FORGE_REPLAY="ctrl+p text:main enter"` feeds keystrokes through the normal key handler after startup
(`FORGE_REPLAY_DELAY_MS` sets the initial delay). Tokens are chords (`ctrl+shift+p`, `f2`), `text:...` to
type, and `wait:MS`.
