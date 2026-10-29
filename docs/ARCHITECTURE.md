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
 ├─ forge-ext-host  extension manifests, .vsix install, Open VSX client, and the Node extension host process
 └─ terminal        (planned) portable-pty child, rendered in GPUI
```

Crates:

- `forge-text`: `ropey` rope; edits are char-offset replacements recorded as
  transactions for undo/redo. Selections are merged and applied back-to-front so
  multi-cursor edits are O(cursors) rope operations.
- `forge-lsp`: Content-Length framed JSON-RPC, a threaded stdio client, and the
  `initialize` handshake. Feature requests (completion, diagnostics, definition) build
  on this.
- `forge-ext-host`: VS Code extension manifests (`package.json`: `contributes` themes, languages,
  grammars, snippets; `main`, `activationEvents`), `.vsix` install and uninstall, and an Open VSX client
  (search, lookup, download with SHA-256 check, https and open-vsx.org only). Running an extension's
  code runs in a Node child process (`host/host.js` plus Forge's own `host/vscode.js`, compiled into the binary and
  written out at start) that speaks one JSON object per line over stdio. Rust side: `ExtHost` (reader and writer
  threads, request/response matching), activation rules, configuration defaults.
- `forge-app`: GPUI window, file tree, tabs, editor view.

## Milestone status

| Milestone | Status |
| --- | --- |
| Text buffer, multi-cursor, undo/redo, VS Code editing commands (lines, comments, pairs, indent, word ops), find/replace engine | implemented, unit-tested (`forge-text`, 59 tests) |
| LSP client: framing, non-blocking initialize, typed diagnostics/completion/definition, server-request replies, progress | implemented; unit-tested and tested against a real rust-analyzer (diagnostics, definition, completion, incremental `didChange`) |
| LSP in the editor: incremental sync, diagnostics underlines, completion popup, go-to-definition (F12 / Ctrl+click) | implemented and observed in a real window with rust-analyzer. Ctrl+Space and keyboard accept (Enter/Tab) were not driven end to end |
| Extensions, phase one (declarative): manifest + `package.nls.json`, `.vsix` install (zip-slip, size and entry limits, atomic replace), Open VSX search/lookup/download, uninstall | implemented, unit-tested; a real download, checksum, install and uninstall of `dracula-theme.theme-dracula` is an automated test (skipped offline) and was driven in a real window |
| VS Code color themes: JSONC, `include`, alpha colors, TextMate scope rules mapped to Forge's token classes | implemented (`forge-core::vscode_theme`), unit-tested; the real Dracula theme applied in a real window |
| Extension languages and `language-configuration.json` (detection by name, glob, extension; comment tokens) | implemented (`forge-core::ext_lang`), unit-tested; comment toggling observed for GraphQL. Brackets, auto-closing pairs and first-line detection are read or ignored, not used |
| TextMate grammars (`forge-syntax::textmate` on `irosashi`): lazy per-line tokenizing with saved state, scopes mapped to token classes | implemented, unit-tested (state across lines, edits equal a fresh tokenization, char columns); a real grammar-only extension (GraphQL, from Open VSX) colored a file in a real window. JSON grammars only |
| Snippets (`forge-core::snippet`, `forge-app::snippet_support`): VS Code syntax, Tab expansion, tab stops that follow edits, mirrors, an insert command | implemented, unit-tested (syntax) and observed in a real window (expansion, typing into placeholders, Tab through stops, mirrored editing). No completion-popup entries, no choice menu |
| Extension host (Phase 2): Node process, line-delimited JSON protocol, `vscode` API subset, activation events, palette commands, status bar items, diagnostics, messages, output channels, workspace edits, document and editor sync, configuration, restart | implemented; automated end-to-end tests run a real extension under real Node (`tests/host_live.rs`) and the purpose-built extension was driven in a real window (diagnostics underlined, status bar item, palette commands, an edit command). 21 unit tests cover activation, defaults, path confinement and message routing. **No third-party extension has been run** |
| Extension language-feature providers, webviews, tree views, terminals | not built: see `docs/EXTENSIONS_PLAN.md` phase 3 and "Not planned" |
| GPUI shell: folder tree, tabs, editing, multi-cursor (Ctrl+Alt+Up/Down, Alt+click), clipboard, save | implemented; rendering and typing observed in a real window on Windows, not covered by automated UI tests |
| Syntax highlighting: 29 languages (tree-sitter for 23 incl. TSX/JSX/XML, line-based for `.env`, INI, Dockerfile, Makefile, ignore files, diffs) | implemented: incremental re-parse, viewport-only queries, files over 4 MiB left plain; unit-tested on real samples per language and checked in a real window for TSX, JSX, XML and `.env` |
| Embedded languages: `<script>`/`<style>` in HTML, fenced code in Markdown (also nested, e.g. HTML in a fence); line-based languages inside fences (diff, bash...) | implemented in `forge-syntax::inject`: regions re-parsed on each edit with tree-sitter included ranges, equivalent to a fresh parse (tested), regions over 512 KiB left plain; observed in a real window for HTML and Markdown |
| Commands + keymap (121 commands, VS Code ids and default Windows bindings, multi-step chords, `keybindings.json`) | implemented, unit-tested |
| Find/Replace widget, Quick Open, command palette, Go to Line/Symbol, Find in Files | implemented; observed in a real window |
| Folding, bracket matching, breadcrumbs, indent guides, split editors, tabs | implemented; observed in a real window |
| Mouse: click, drag, double/triple click, Shift/Alt/Ctrl+click, gutter | implemented; click and drag observed in a real window |
| LSP: hover, signature help, references, rename, code actions, format, outline, Problems panel | implemented; protocol checked against a real rust-analyzer, UI observed in a real window |
| Language server registry and installer (`forge-core::servers`, `forge-app::server_support`): shared servers by key, managed install via npm or download, `languageServers` overrides, "Language Servers: Manage and Install..." | implemented; the installer ran from the real window and installed 11 servers. Verified end to end with a real typescript-language-server (`.tsx` diagnostics; integration test) and LemMinX (`.xml`); the rest are exercised by `every_installed_server_answers_for_its_language` (CSS, JSON, YAML, Python, Dockerfile, PHP, Markdown, Shell, HTML: all answered). rust-analyzer, gopls, clangd, jdtls, csharp-ls, ruby-lsp, lua-ls, taplo and sqls are configured but must be installed by the user |
| Explorer file operations (create, rename, delete to Recycle Bin) | implemented; rename prompt observed, file operations unit-tested |
| Integrated terminal (ConPTY + VT parser) | implemented; a real shell ran in a real window |
| Source control: status/stage/commit/branches/push/pull/stash/history/blame through the `git` CLI on background threads, gutter diff, diff tabs, SCM view | implemented; engine tested against real temporary repos (incl. a local bare remote), UI observed in a real window on a sandbox repo |
| Settings, themes (3), keybindings JSON, zoom | implemented; theme switch observed in a real window |
| Context menus (editor, explorer; `context_menu.rs`) and the go-to-definition picker for several results | implemented; compiles and the workspace tests pass, not yet observed in a real window |
| Minimap, word wrap, replace in files, debugger, settings UI | not built |
| VS Code extension host (Node + `vscode` API shim) | planned; largest risk |

## Risks

- VS Code extension compatibility needs a substantial `vscode` API surface plus a
  Node host; it will cost startup time and memory, partly offsetting the speed goal.
  Keep it out of the editing hot path and load it lazily.
- `gpui` on crates.io (0.2.x) is pre-1.0 and its API changes between releases; pin the
  version.
