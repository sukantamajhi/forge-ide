# Plan: VS Code extension compatibility

Status: **phases 1 and 2 are built** (see each for what is and is not covered); phase 3 is a plan. Open VSX and Node were assumed acceptable: the questions at the end were not answered, so
confirm them before phase 2.
The goal is to let a Forge user install and run VS Code extensions. "All of them" is
not reachable: the extension API is thousands of members, and parts of it (webviews,
notebooks, the debugger UI, remote development) assume VS Code's own workbench. The plan
therefore grows compatibility in layers, each one useful and testable on its own, and
measures progress against real extensions instead of against the API surface.

## What an extension is made of

Most of an extension is **declarative** (JSON and data files, no code runs). Only the
`main` entry point needs a Node process.

| Contribution | Needs Node? | Forge today |
| --- | --- | --- |
| Color themes (`contributes.themes`: `colors` + `tokenColors`) | no | built-in themes only |
| Language definitions and `language-configuration.json` (comments, brackets, auto-closing, indentation) | no | hard-coded per language |
| TextMate grammars (`contributes.grammars`) | no | tree-sitter only |
| Snippets | no | not built |
| Keybindings, settings schema, icon themes | no | keymap and settings exist; no schema |
| Commands, status bar items, views, menus | yes (handlers run in the host) | manifest parsed only |
| Language features (completion, hover, diagnostics, code actions...) | yes (providers or a language client) | LSP client exists |

## Where extensions come from

- **Open VSX** (open-vsx.org) is the supported registry. Microsoft's Marketplace terms
  restrict its use to Microsoft's own products, so Forge must not use it.
- **`.vsix` files** installed from disk (a `.vsix` is a zip with an `extension/` folder).
- Installed under `%APPDATA%\Forge\extensions\<publisher>.<name>-<version>`, next to the
  language servers Forge already installs into its own folder.

## Phases

### Phase 1: declarative extensions (no Node) — built

What exists: install from Open VSX or a `.vsix` (checksum verified, confined to open-vsx.org,
zip-slip and size guarded), list and uninstall, themes, languages with comment toggling,
TextMate grammars (JSON), snippets with tab stops. Verified with real extensions from
Open VSX: Dracula (themes) and GraphQL Syntax Highlighting (a language and eight grammars).
What is left of the original scope: the theme scope mapping is partial (no semantic or font
styles), `language-configuration.json` bracket and auto-closing pairs are parsed but not
applied, TextMate plist/YAML grammars and injection grammars are skipped, first-line language
detection is missing, and snippets are not offered in the completion popup.

The plan as written:


Deliverable: install a `.vsix` or an Open VSX extension and get its themes, language
definitions, grammars, snippets and language configuration.

1. `.vsix` unpack and install, uninstall, list; "Extensions: Install from VSIX..." and
   "Extensions: Install from Open VSX..." commands (a search-and-pick overlay, like the
   Language Servers one).
2. Theme loader: VS Code theme JSON (`colors`, `tokenColors`, `include`) mapped onto
   Forge's `Theme` and `HighlightClass` set. Needs a scope-to-class mapping table; unknown
   scopes fall back to the nearest known parent scope.
3. `language-configuration.json` drives comment toggling, bracket pairs, auto-closing
   and indentation for languages Forge does not know.
4. TextMate grammars through the `syntect` crate (pure-Rust regex backend), used for
   languages without a tree-sitter grammar. Tree-sitter stays the default where both
   exist. This is what makes arbitrary language extensions colorable.
5. Snippets: parser for the VS Code snippet syntax (`$1`, `${2:default}`, `$TM_FILENAME`,
   choices, nested placeholders) plus tab-stop navigation in the editor.

Risks: the theme scope mapping is lossy; TextMate matching is slower than tree-sitter
(run it per visible line with per-line state caching); snippet placeholders need
multi-cursor-style linked editing.

Done when: a popular theme (e.g. a One Dark or Dracula port from Open VSX), a grammar-only
extension for a language Forge lacks, and a snippet pack install and visibly work.

### Phase 2: a Node extension host — built, not yet proven on third-party extensions

What exists: `crates/forge-ext-host/host/` (`host.js`, `vscode.js`) run by the user's Node, one process for all
extensions, started on first need; line-delimited JSON over stdio (not LSP framing: simpler, and nothing needs byte
counts); activation on `*`, `onStartupFinished`, `onLanguage`, `onCommand` and on contributed commands; the API
subset listed in the README (commands, window, diagnostics, workspace documents and edits, configuration, status
bar, output, data classes). Unsupported members throw a clear error when called. Crashing extensions are isolated.
Verified by automated tests against a purpose-built extension under real Node and by driving it in a real window.

Not verified, and the biggest open risk: **real third-party extensions**. They will hit API members that are
missing or subtly different, so each one chosen is likely to need fixes. `vscode-languageclient` loads and can show
diagnostics, but its other features go through providers, which are phase 3. Choose the extensions to try; the
corpus under `tests/` should grow from them.

The plan as written:


Deliverable: extensions whose job is to start a language server (most language
extensions) work.

- A **Forge-owned `vscode` module written in JavaScript**, shipped with Forge and run by the
  user's Node (Forge already requires Node for npm-installed servers). It is an
  independent implementation of the documented API, not VS Code's code.
- The host process talks to Forge over JSON-RPC on stdio, the same transport as LSP, so
  `forge-lsp`'s framing is reused. One host process for all extensions, started lazily on
  the first activation event.
- Activation: `onLanguage:*`, `workspaceContains:*`, `onCommand:*`, `*`,
  `onStartupFinished`. Parsed already; the host evaluates them.
- API subset, in the order real extensions need it: `ExtensionContext`, `commands`,
  `workspace` (folders, `getConfiguration`, `onDidChange*`, `fs`, `openTextDocument`),
  `window` (messages, output channels, status bar items, `createTerminal` last), `Uri`,
  `Disposable`/`EventEmitter`, `languages.createDiagnosticCollection`, `TextDocument`/
  `Range`/`Position`/`Selection`/`TextEdit`/`WorkspaceEdit`.
- `vscode-languageclient` support: it is the library most extensions use to run a server.
  It needs `workspace`, `languages.register*Provider`, `window.createOutputChannel` and
  file-watcher APIs. Forge can either satisfy that API or, for servers that need no
  extension-side logic, read the server command from the extension and launch it through
  the existing LSP manager.
- Security: extensions are arbitrary code. First activation of an extension asks for
  trust; the host runs with the user's permissions, which is also true in VS Code, and the
  UI says so.

Risks: the API surface is large and behaviours are subtle (event ordering, `TextDocument`
versioning, URI schemes). Mitigation is below: target real extensions, not the spec.

Done when: a chosen set of real language extensions starts its server, shows diagnostics and
completions in Forge, and survives a server crash.

### Phase 3: provider-based language features

Extensions can register completion, hover, definition and similar providers in JavaScript
instead of using a server. Forge asks the host over IPC and awaits the reply without
blocking the UI thread (the same request/poll pattern as `LspManager`). Needs cancellation
and a latency budget so a slow extension cannot make typing feel slow: results arrive
asynchronously, and a late completion list is dropped.

### Not planned

Webviews (an embedded browser per extension), notebooks, the debug adapter UI, remote
development (SSH, WSL, containers), live share, custom editors, the Chat/AI APIs, and tree
views beyond a basic list. Each would be its own large project; extensions that depend on
them will install but only partly work, and Forge will say which contributions it ignored.

## Measuring compatibility

Keep a small **corpus** under `crates/forge-ext-host/tests/corpus`, with a pinned version of
each extension and a scripted check that is run in CI where Node is available:

- Phase 1: one color theme, one grammar-only extension, one snippet pack.
- Phase 2: a minimal extension written for the tests (command + diagnostics + status bar),
  then two or three real language-client extensions chosen by the user.
- A **compatibility report** command lists, for an installed extension, which contributions
  and API calls Forge supports and which it ignored. Unimplemented API members throw a clear
  "not supported by Forge" error instead of returning `undefined`.

## Decisions needed from you

1. Is Open VSX acceptable as the registry (the only licensed option), or do you want `.vsix`
   from disk only?
2. Which extensions matter most to you? They become the compatibility corpus and decide the
   Phase 2 API order.
3. Is requiring Node.js on the machine acceptable for Phase 2? (The alternative, embedding a
   JavaScript runtime in Forge, costs binary size and startup time and works against the
   speed goal.)

## Suggested order and size

Phase 1 is a few weeks of focused work and delivers visible value without any JavaScript.
Phase 2 is the largest piece (months to reach broad compatibility) but a useful first cut
(one extension running end to end) is a short, well-defined step once Phase 1 has created
the install path. Phase 3 follows demand.
