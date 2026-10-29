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

## Not built yet

Minimap, word wrap, replace in files, side-by-side diffs, staging individual hunks or lines, merge-conflict
resolution, rebase, git credentials prompts (commands fail instead of prompting), debugger, snippets, multi-root
workspaces, a settings UI, right-click context menus, drag-and-drop of tabs or text, sticky scroll, inlay
hints, semantic highlighting, per-language settings, and **VS Code extension compatibility** (only
manifest parsing exists). Text layout assumes a monospace font, one cell per char (tabs = 4), no soft wrap.
Several definitions are not offered as a list (the first is taken); completions are plain text, not snippets;
files must be UTF-8 (a leading BOM is kept out of the text and written back on save) and under 64 MiB; syntax
highlighting is skipped above 4 MiB. There is no embedded-language highlighting (JavaScript inside HTML, code
fences in Markdown), SCSS/Less use the CSS grammar, and `.h` files are treated as C. VS Code's own extensions
cannot supply language servers yet, so "every VS Code language server" is not covered: only the servers above.

## Testing aid

`FORGE_REPLAY="ctrl+p text:main enter"` feeds keystrokes through the normal key handler after startup
(`FORGE_REPLAY_DELAY_MS` sets the initial delay). Tokens are chords (`ctrl+shift+p`, `f2`), `text:...` to
type, and `wait:MS`.
