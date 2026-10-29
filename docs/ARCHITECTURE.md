# Forge architecture

Forge is an original Rust code editor. It targets the same *category* of product as
VS Code (project editor with LSP and extensions) but shares no code, name, icons or
branding with it. Performance goal: Zed-class latency, via a GPU-rendered UI (GPUI),
a rope text buffer, and no per-keystroke JavaScript on the hot path.

## What was learned from VS Code (via REA)

Target: VS Code 1.143.1 installed at `D:\ides\Microsoft VS Code` (unpacked
`resources/app`, 3,943 files in the install tree). Findings are static only; nothing
was executed.

| Fact | Source | Confidence |
| --- | --- | --- |
| App is an Electron app: `main` = `./out/main.js`; unpacked `out/` ~74 MB, bundled `extensions/` ~260 MB | `package.json`, directory listing | observed |
| Large bundles per process role: `workbench.desktop.main.js` (renderer UI, ~21 MB), `extensionHostProcess.js` (~2.3 MB), `sharedProcessMain.js` (~1.5 MB), `ptyHostMain.js` (~0.4 MB), `editorWebWorkerMain.js` (~0.3 MB), `cliProcessMain.js`, `agentHostMain.js` | file sizes in `out/` | observed |
| Process split: main, renderer workbench, shared process, pty host, extension host, editor web worker | file names + REA Electron relations (3 BrowserWindow sites, 1 utility process, 3 main-process IPC handlers with dynamic channel names) | derived from 6 of ~4,700 sources |
| Terminal runs in a separate pty host process | `ptyHostMain.js` present | observed (file) / inferred (role) |
| Source layers `base`, `platform`, `editor`, `workbench`, `code` | `out/vs/*` directory names | observed |
| IPC channel names, per-feature wiring, and renderer internals are **unknown** | REA analysis was partial (6 sources analyzed; semantic graph hit its 100k unknown ceiling) | unknown |

REA coverage is partial, so everything below beyond the table is design, not
reverse-engineered fact. VS Code's own source is MIT-licensed upstream, but Forge is
written independently and does not copy it.

## Design

Process model (chosen to keep the UI thread free of language/extension work):

```
forge (GPUI app, UI + rendering thread)
 ├─ forge-text      rope buffer, multi-cursor selections, undo/redo      (in-process)
 ├─ forge-syntax    tree-sitter incremental parse, highlight spans, folds, expand-selection (in-process)
 ├─ forge-core      UI-free editor logic: commands + keymap, folds, find, fuzzy, input, search, terminal, settings, themes
 ├─ forge-lsp       LSP client, one child process per language server    (stdio JSON-RPC)
 ├─ forge-ext-host  extension manifest + (planned) Node extension host   (separate process)
 └─ terminal        (planned) portable-pty child, rendered in GPUI
```

Crates:

- `forge-text`: `ropey` rope; edits are char-offset replacements recorded as
  transactions for undo/redo. Selections are merged and applied back-to-front so
  multi-cursor edits are O(cursors) rope operations.
- `forge-lsp`: Content-Length framed JSON-RPC, a threaded stdio client, and the
  `initialize` handshake. Feature requests (completion, diagnostics, definition) build
  on this.
- `forge-ext-host`: parses VS Code extension manifests (`package.json`:
  `main`, `activationEvents`, `contributes`). Running real VS Code extensions
  requires hosting the `vscode` API in Node; that host is **not implemented**.
- `forge-app`: GPUI window, file tree, tabs, editor view.

## Milestone status

| Milestone | Status |
| --- | --- |
| Text buffer, multi-cursor, undo/redo, VS Code editing commands (lines, comments, pairs, indent, word ops), find/replace engine | implemented, unit-tested (`forge-text`, 59 tests) |
| LSP client: framing, non-blocking initialize, typed diagnostics/completion/definition, server-request replies, progress | implemented; unit-tested and tested against a real rust-analyzer (diagnostics, definition, completion, incremental `didChange`) |
| LSP in the editor: incremental sync, diagnostics underlines, completion popup, go-to-definition (F12 / Ctrl+click) | implemented and observed in a real window with rust-analyzer. Ctrl+Space and keyboard accept (Enter/Tab) were not driven end to end |
| Extension manifest parsing | implemented, unit-tested |
| GPUI shell: folder tree, tabs, editing, multi-cursor (Ctrl+Alt+Up/Down, Alt+click), clipboard, save | implemented; rendering and typing observed in a real window on Windows, not covered by automated UI tests |
| Syntax highlighting: 29 languages (tree-sitter for 23 incl. TSX/JSX/XML, line-based for `.env`, INI, Dockerfile, Makefile, ignore files, diffs) | implemented: incremental re-parse, viewport-only queries, files over 4 MiB left plain; unit-tested on real samples per language and checked in a real window for TSX, JSX, XML and `.env`. No embedded-language (injection) support |
| Commands + keymap (121 commands, VS Code ids and default Windows bindings, multi-step chords, `keybindings.json`) | implemented, unit-tested |
| Find/Replace widget, Quick Open, command palette, Go to Line/Symbol, Find in Files | implemented; observed in a real window |
| Folding, bracket matching, breadcrumbs, indent guides, split editors, tabs | implemented; observed in a real window |
| Mouse: click, drag, double/triple click, Shift/Alt/Ctrl+click, gutter | implemented; click and drag observed in a real window |
| LSP: hover, signature help, references, rename, code actions, format, outline, Problems panel | implemented; protocol checked against a real rust-analyzer, UI observed in a real window |
| Language server registry and installer (`forge-core::servers`, `forge-app::server_support`): shared servers by key, managed install via npm or download, `languageServers` overrides, "Language Servers: Manage and Install..." | implemented; the installer ran from the real window and installed 11 servers. Verified end to end with a real typescript-language-server (`.tsx` diagnostics; integration test) and LemMinX (`.xml`); the other installed servers were installed but not exercised. rust-analyzer, gopls, clangd, jdtls, csharp-ls, ruby-lsp, lua-ls, taplo and sqls are configured but must be installed by the user |
| Explorer file operations (create, rename, delete to Recycle Bin) | implemented; rename prompt observed, file operations unit-tested |
| Integrated terminal (ConPTY + VT parser) | implemented; a real shell ran in a real window |
| Source control: status/stage/commit/branches/push/pull/stash/history/blame through the `git` CLI on background threads, gutter diff, diff tabs, SCM view | implemented; engine tested against real temporary repos (incl. a local bare remote), UI observed in a real window on a sandbox repo |
| Settings, themes (3), keybindings JSON, zoom | implemented; theme switch observed in a real window |
| Minimap, word wrap, replace in files, git/SCM, debugger, snippets, settings UI, context menus | not built |
| VS Code extension host (Node + `vscode` API shim) | planned; largest risk |

## Risks

- VS Code extension compatibility needs a substantial `vscode` API surface plus a
  Node host; it will cost startup time and memory, partly offsetting the speed goal.
  Keep it out of the editing hot path and load it lazily.
- `gpui` on crates.io (0.2.x) is pre-1.0 and its API changes between releases; pin the
  version.
