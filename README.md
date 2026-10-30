<p align="center">
  <img src="assets/forge-logo.svg" alt="Forge logo: lines of code with a text caret" width="128" height="128">
</p>

# Forge

An original, GPU-rendered code editor written in Rust on [GPUI](https://crates.io/crates/gpui),
aiming for Zed-class responsiveness and VS Code-style editing. See
[docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) for the design and for what is and is not built.

## Build and run

Forge is built and released for **Windows**. A **Linux** build is set up through the Makefile but has not been compiled or run yet (see "Linux" below); macOS is not supported yet.

**Windows:** needs Rust (stable) and Visual Studio Build Tools with the C++ workload.

```bash
cargo run -p forge-app -- path/to/folder      # or path/to/file.rs
cargo test --workspace
cargo build --release -p forge-app            # LTO; slow first build
```

**Makefile** (GNU make; on Windows from Git Bash, `winget install ezwinports.make`). `make help` lists everything:

| Command | What it does |
|---|---|
| `make release-windows` | Windows build, `target
eleaseorge.exe` (run on Windows) |
| `make release-linux` | Linux build. On Linux it builds in place; on Windows it runs inside WSL2 and writes to `target-linux/` |
| `make release-all` | both, from Windows with WSL2 |
| `make package-windows` | `dist/Forge-Setup-<version>.exe` (needs Inno Setup 6) |
| `make package-linux` | `dist/forge-<version>-linux-x86_64.tar.gz`: the program, a launcher entry, the icon and `install.sh` |
| `make package-all` | both packages |
| `make linux-deps` | install the libraries the Linux build needs (Ubuntu / Debian, asks for sudo) |

**Linux:** needs Rust and the system libraries from `make linux-deps` (fontconfig, Wayland, xkbcommon, X11/xcb, Vulkan loader, OpenSSL, ALSA), and a Vulkan-capable graphics driver to run. From Windows, install Rust inside your WSL distribution first, then `make linux-deps` and `make package-linux`. Unpack the archive and run `./install.sh` to install for the current user (`./install.sh --uninstall` removes it).
Not done for Linux: the OpenAI / Gemini key cannot be saved yet (it is encrypted with a Windows-only mechanism, so it is kept for the session), "Reveal in File Explorer" still calls Windows tools, and the default fonts are Windows ones (Forge falls back to what is installed).

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
    (reversed for unstaging), so the rest of the file is untouched. The diff updates after every change. Files with Windows
    line endings work: the patch keeps each line's `\r`. **Discard Hunk** and **Discard Selected** (working-tree diffs only)
    throw changes away after a confirmation, with `git apply --reverse`.
  - **New files.** The diff of an untracked file lets you click lines and **Stage Selected**, or **Stage All**: only those
    lines go into the index (a blob is written and the index entry added), the file on disk is untouched, and the tab then
    shows what is left as a normal working-tree diff.
  - **Merge conflicts.** A conflicted file (marked `!`) opens a page listing each conflict with the current and incoming
    sides next to each other and **Accept Current / Incoming / Both** (or all at once). A middle **Base (common ancestor)**
    column shows what both sides started from (read from the index and merged in memory, so the file is not touched).
    When no markers are left, **Stage File** marks it resolved. `Git: Resolve Merge Conflicts in File` does the same for
    the file in the editor.
  - **Rebase and merge.** `Git: Rebase Branch...` and `Git: Merge Branch...` pick a branch. When git stops (usually for
    conflicts) the Source Control view shows a banner ("Rebase in progress (2/5)", "Merge in progress", "Cherry-pick in
    progress", "Revert in progress") with **Continue**, **Skip** and **Abort** (`Git: Continue Rebase`, `Git: Skip This Commit
    in the Rebase`, `Git: Abort Rebase`, `Git: Commit the Merge`, `Git: Abort Merge`, and the same for cherry-pick and revert).
  - **Interactive rebase.** `Git: Interactive Rebase...` asks for the commit to rebase after, then opens a page listing the
    commits above it, oldest first. Each can be **pick**ed, **reword**ed (type the new message in its row), **squash**ed or
    **fixup**ed into the commit above, or **drop**ped, and moved with the arrows. **Start Rebase** gives git the list through
    its sequence editor (no editor window opens). Limits: merge commits are flattened (git's default), the `edit` and
    `break` steps are not offered, and if the rebase stops for conflicts, rewords still waiting keep their original messages.
  - **Cherry-pick and revert.** `Git: Cherry-pick Commit...` lists commits from every branch; `Git: Revert Commit...` lists
    the current branch's. Conflicts stop them in the banner above like a rebase.
  - **Commit graph and timeline.** The bottom panel's **GRAPH** tab draws every branch's history with lanes, dots and merge
    lines, with branch and tag labels (`Git: Show Commit Graph`); click a commit to see it. **TIMELINE** lists the commits of the
    file in the editor and follows it as you switch tabs (`Git: Show File Timeline`); click one to see what it changed in the
    file. The graph shows the latest 400 commits and 12 lanes.
  - **Submodules and worktrees.** `Git: Submodules...` lists them with their state (up to date, not initialised, at another
    commit than recorded, conflicts) and opens one in a new Forge window; `Git: Update Submodules` runs `git submodule update
    --init --recursive`. `Git: Worktrees...` lists the repository's worktrees and opens one in a new window; `Git: Add Worktree
    for Branch...` makes one for a local branch in a folder next to this one, and `Git: Remove Worktree...` removes one (git
    refuses when it has uncommitted changes).
  - **Logins.** A pull, push or fetch that needs a username, password, passphrase or a yes/no answer (host keys) asks in the
    editor instead of failing. Forge starts git with itself as the `GIT_ASKPASS` / `SSH_ASKPASS` helper, which asks the
    running editor over a local socket (127.0.0.1 only, with a random token), the way VS Code's git extension does.
    Passwords are shown as dots, go only to git, and Escape cancels the command. Credential helpers you already use still run first.
  - **GPG signing.** With `commit.gpgsign` (or a signing key) set, commits, merges, rebases, cherry-picks and reverts that
    need your key's passphrase ask for it in the editor. Forge tells git to use itself as `gpg.program`; it runs your real gpg
    first with prompts off (so a passphrase gpg-agent already holds works silently), and only if gpg says it needs one asks
    in the editor and runs gpg again with the answer in a short-lived file that is deleted right away. SSH signing
    (`gpg.format = ssh`) asks through the same SSH askpass as other ssh prompts. Hardware tokens that ask for a PIN on the
    device are not covered.
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
- **Menu bar and Explorer buttons:** a menu bar under the title bar has File, Edit, Selection, View, Go, Run, Terminal and Help.
  Click a title to open its menu, then move across the bar to switch menus. Every entry runs a command and shows its shortcut.
  File also has New Window and Exit (Alt+F4; with unsaved changes it asks once and a second Exit discards them). The Explorer
  header has **New File**, **New Folder**, **Refresh** and **Collapse Folders** buttons (also commands in the palette). New File
  and New Folder go in the selected folder, or the project root when nothing is selected. The Explorer's right-click menu (folders and files) also has
  **Open in Integrated Terminal** and **Find in Folder...** (folders: the search then looks only in that folder, until Ctrl+Shift+F),
  and **Cut / Copy / Paste** of files and folders (also Ctrl+X / C / V with the Explorer focused; paste goes into the selected folder,
  a copy that would clash gets "name copy", a cut never replaces anything, and open tabs follow a moved file).
  Right-clicking the empty part of the Explorer opens a menu for the project folder (New File, New Folder, Reveal, Open in
  Integrated Terminal, Find in Folder, Paste, Copy Path, Copy Relative Path); Add / Remove Folder from Workspace are not offered
  (no multi-root workspaces).
  **Drag and drop:** drag a file or folder by its row onto a folder to move it there (onto a file to move it next to that file, onto
  the empty part of the list for the project folder). A chip with the name follows the pointer and the target folder is
  highlighted. Moves ask first ("Press Enter to move 'a.txt' into 'src'"; turn that off with `"explorer.confirmDragAndDrop": false`);
  hold Ctrl while dropping to copy instead, which does not ask. Files dropped from Windows Explorer onto the Explorer are copied in.
  Open tabs follow a moved file, a folder cannot go into itself, and nothing is ever replaced. Not done: folders do not open
  while you hold a drag over them, and only one item can be dragged at a time.
  A file's menu also has **Open to the Side** (Ctrl+Enter with the Explorer focused), **Open in Integrated Terminal** (in its folder),
  **Select for Compare** then **Compare with Selected** on another file, and, inside a git repository, **File History** and
  **Copy Remote File URL** (the `origin` remote's web page for the file on the current branch; GitHub, GitLab and Bitbucket style
  addresses). VS Code's Open With, Share, Open Changes, Open on Remote, Timeline, Add to Chat, Images Preview and
  Copy Remote File URL From... entries are not offered. Menus open by mouse only (no Alt+F access keys yet).
- **Editor navigation and files:**
  - **Go Back / Go Forward** (Alt+Left / Alt+Right) return to where you were before a jump: opening another file, go to definition,
    a symbol or a line. **Reopen Closed Editor** (Ctrl+Shift+T) brings back the last closed tab with its caret (30 are kept this session).
  - **Open Recent** (Ctrl+R) lists folders opened before, newest first, and opens the chosen one in this window.
  - **Zen Mode** (Ctrl+K Z) hides the side bar and panel; the same shortcut brings them back.
  - **Auto Save:** `"files.autoSave": "afterDelay"` saves the open file once it has been left alone for `"files.autoSaveDelay"`
    milliseconds (200 or more; 1000 by default). `"onFocusChange"` saves every modified file when you switch tabs or open another file.
    `Toggle Auto Save` in the Command Palette flips between off and afterDelay. Format on save is not run by auto save.
  - **Compare Active File With...** picks a second file and shows the difference as a coloured diff tab (it compares what is
    in the editor, including unsaved changes, using `git diff --no-index`, so git must be installed; no repository is needed).
  - **Per-language settings:** `"[rust]": { "editor.tabSize": 2 }` blocks, as in VS Code, for `editor.tabSize`, `editor.insertSpaces`
    and `editor.formatOnSave`. One block can name several languages (`"[javascript][typescript]"`). Ids are VS Code's (`rust`, `python`,
    `javascript`, `typescriptreact`, `shellscript`, `csharp`...). The status bar, Tab key, indentation and the server's formatter use them.
  - **Format Selection** (Ctrl+K Ctrl+F) and **Organize Imports** (Shift+Alt+O) ask the language server for `rangeFormatting` and
    for its `source.organizeImports` action; they do nothing useful when the server does not offer them (the status bar says so).
    **Focus on Outline View** opens the symbols picker for the file (the same list as Go to Symbol in Editor), not a permanent tree.
  - **Images and binary files:** PNG, JPEG, GIF, BMP, WebP and ICO files open in a viewer over the editor (Escape or Close to leave).
    Any other file that is not UTF-8 opens as a read-only hex dump of its first 256 KiB.
- **CSV / TSV preview:** the same **Open Preview** command (Ctrl+Shift+V, or the page icon in the tab bar) shows a `.csv` or `.tsv` file as a table: the first row is the header, with row
  numbers, aligned columns (numbers right-aligned), zebra rows and a summary (data rows, columns, delimiter). Quoted fields, doubled quotes and line breaks inside quotes are read
  correctly, and the delimiter (comma, semicolon, tab or pipe) is guessed from the first lines. It scrolls both ways (a horizontal swipe or Shift + the wheel for sideways). Only the first
  100,000 rows are shown; editing is blocked while it shows, and Ctrl+Shift+V returns to the text. Not done: sorting, filtering, editing cells, a header-less mode.
- **Markdown preview:** **Markdown: Open Preview** (Ctrl+Shift+V, the page icon in the tab bar, View menu) replaces the source in the tab with the rendered page in a readable column;
  press it again, or the "Show Source" icon, to get the source back. While the page is showing, typing is blocked (the status bar says why) so you cannot edit what you cannot see.
  **Open Preview to the Side** (Ctrl+K V) is still there and scrolls in step with the source; the two are alternatives, asking for one closes the other.
- **Scrolling sideways:** long lines scroll with a horizontal mouse wheel or trackpad swipe, or Shift + the wheel; the line numbers stay put, and moving the caret
  (End, arrow keys, typing) scrolls it into view. In a side-by-side diff both columns scroll together. When many tabs are open, the tab strip scrolls with the wheel
  (the active tab is brought into view when you switch to it). With word wrap on, lines wrap instead.
- **Markup and CSS help:**
  - **Auto closing tags** (HTML, XML, JSX, TSX, and `.vue` / `.svelte` when an extension names them): typing `>` after `<div class="x"` adds `</div>` with the
    caret between; `</` finishes with the innermost open tag; void tags (`br`, `img`...), self-closing tags and generics like `Array<string>` get nothing.
    `"html.autoClosingTags": false` turns it off.
  - **Auto rename tags:** with the caret in a tag's name, the matching opening or closing tag gets the same name as you type or delete
    (`"editor.linkedEditing": false` turns it off). Files over 400,000 characters are skipped.
  - **Emmet:** type an abbreviation and press Tab in HTML, JSX / TSX or inside a CSS rule. HTML: `ul>li.item$*3`, `div#main.box>p{text}+a`, `(dl>dt+dd)*2`, `a[href=x]`, `^` to
    climb, `$` numbering, implicit tags (`ul>.x` makes `li`), `!` for an HTML5 page; JSX writes `className` and `<br />`. CSS: `m10-20`, `w100p`, `mt-5`, `m0-a`, `fz1.5r`, `df`, `jcsb`, `aic`, `c#f00`
    and about sixty more. A bare word expands only if it is a real tag name (so `return` + Tab stays a Tab). `"emmet.triggerExpansionOnTab": false` turns it off. Not supported: filters
    (`|c`), `@` numbering modifiers, `lorem`, and the less common CSS abbreviations.
- **Reading code:**
  - **Bracket pair colouring:** `()` `[]` `{}` are coloured by nesting depth (three colours, red for a stray closer). Brackets inside strings and `//` / `#` comments are skipped; a block comment
    or a multi-line string can confuse it. `"editor.bracketPairColorization.enabled": false` turns it off.
  - **Sticky scroll:** the lines that open the scopes you are inside (function, impl, block) stay pinned at the top of the editor, up to `"editor.stickyScroll.maxLineCount"` (5); click one to go
    to it. Scopes come from the folding regions. Off with `"editor.stickyScroll.enabled": false`, and not shown with word wrap on.
  - **Word suggestions:** with no language server, typing two letters offers words already in the file (most frequent first), below any matching snippets. Off with `"editor.wordBasedSuggestions": false`.
- **Snippets** (VS Code's format):
  - **In the completion popup.** As you type a word that starts a snippet's prefix, the popup lists it ("snippet: description") along
    with language-server items; Enter or Tab inserts it. Tab right after a full prefix still expands it. `"editor.snippetSuggestions": "none"` turns the listing off.
  - **Walking the stops.** Tab / Shift+Tab move between `$1`, `$2`, `${3:placeholder}`; `$0` is where the caret ends; Escape leaves the
    snippet. Typing in a stop edits every mirror of it at once. `${1|red,green,blue|}` opens a dropdown of the options (arrows then
    Enter or Tab; Tab also moves on).
  - **Variables:** `TM_FILENAME`, `TM_FILENAME_BASE`, `TM_DIRECTORY`, `TM_FILEPATH`, `RELATIVE_FILEPATH`, `WORKSPACE_NAME`, `WORKSPACE_FOLDER`,
    `TM_SELECTED_TEXT`, `TM_CURRENT_LINE`, `TM_CURRENT_WORD`, `TM_LINE_INDEX`, `TM_LINE_NUMBER`, `CLIPBOARD`, `LINE_COMMENT`, `BLOCK_COMMENT_START` / `END`,
    the `CURRENT_*` date and time ones (local time, including day and month names and `CURRENT_SECONDS_UNIX`), `RANDOM`, `RANDOM_HEX` and `UUID`.
    `${VAR:default}` uses the default when the variable is empty.
  - **Transforms:** `${TM_FILENAME_BASE/(.*)/${1:/upcase}/}`, with `/g` and `/i`, the `$1` and `${1:/downcase | capitalize | pascalcase | camelcase}`
    forms and `${1:+if}` / `${1:-else}` / `${1:?if:else}`. A transform is worked out once when the snippet is inserted; it does not follow later typing.
  - **Built in:** Forge ships about 280 snippets, kept in `assets/snippets` and built into the program: for Rust, Python,
    JavaScript / TypeScript (with React hooks and components for `.jsx` / `.tsx`), HTML, CSS, Markdown, Go, C, C++, Java, C#, PHP, Ruby, Lua, SQL,
    shell, YAML (GitHub Actions, Compose, Kubernetes), TOML (Cargo, pyproject), JSON (`package`, `tsconfig`, `launch`), Dockerfile and Makefile, plus
    ones for every language (`todo`, `fixme`, `note`, `date`, `datetime`, `uuid`, `lorem`, `clip`, `region`...). Type `fn`, `def`, `log`, `class`, `iferr`, `rfc`,
    `html5`... and press Tab. Your own snippet with the same prefix wins. `"editor.builtInSnippets": false` turns them all off.
  - **Your own snippets:** **Configure User Snippets** (File menu or the palette) asks for a language (or "New Global Snippets file") and opens
    `snippets\<language id>.json` (or `global.code-snippets`) next to `settings.json`, creating it with an explanation. Saving the file reloads the snippets.
    **Insert Snippet...** lists what applies to the file. Snippets from installed extensions work too.
- **Activity bar:** the Explorer, Search, Source Control and Extensions icons sit in one row at the top of the sidebar; the open view's icon has a rounded outlined box, Source Control shows its change count on the icon, and a "..." button opens Problems, Terminal, Outline, Git Graph and File Timeline. The menu bar starts with the Forge logo.
- **Welcome page:** with no file open, the editor area shows the logo, the name, clickable quick actions with their shortcuts (Open Folder, New File, Go to File, Show All Commands, Find in Files, Toggle Terminal) and up to five recent folders (click one to open it).
- **Fonts and icons:** code, the terminal and diffs use a monospace font (Cascadia Mono, then Cascadia Code, then Consolas); menus,
  tabs, the side bar, the status bar, Settings and dialogs use a sans font (Segoe UI Variable Text, then Segoe UI). Pick others
  with `"editor.fontFamily"` and `"workbench.fontFamily"` (a name Windows has installed; a missing one falls back to the list).
  Files and folders have Codicon pictures instead of two-letter badges (a code file, JSON, Markdown, image, archive, terminal,
  settings gear, key for `.env`, lock, folder open / closed...), coloured by language. Icon themes from extensions still win.
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
  terminal (Ctrl+`; several at once, each with its own tab, New Terminal with Ctrl+Shift+`; 5000 lines of scrollback with the mouse wheel or Shift+PageUp/PageDown; drag to select text, double-click a word, triple-click a line, Ctrl+C or Ctrl+Shift+C or a right-click copies the selection (Ctrl+C still interrupts when nothing is selected; a right-click with no selection pastes); web addresses and file paths in the output, with an optional `:line:col`, are underlined on hover and a click opens them in the browser or the editor), bottom panel, sidebar toggle (Ctrl+B), zoom, three themes, settings and keybindings JSON.

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
  "files.trimTrailingWhitespace": false, "editor.formatOnSave": false,
  "files.autoSave": "afterDelay", "files.autoSaveDelay": 1000, "explorer.confirmDragAndDrop": true,
  "[python]": { "editor.tabSize": 4 }, "[javascript][typescript]": { "editor.tabSize": 2 } }
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
| Snippets | Used: see "Snippets" above (popup, Tab stops, choices, variables, transforms). Your own: `snippets\<language id>.json` (or `*.code-snippets` for all) next to `settings.json`. |
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

**Built** (details in the sections above):

- **Editing:** multi-cursor editing, find / replace, Replace in Files, folding, snippets, Format Document / Selection, Organize Imports,
  auto save (`afterDelay`, `onFocusChange`), per-language settings (`"[rust]": { ... }` for tab size, spaces, format on save).
- **Navigation:** Quick Open, Command Palette, Go to Symbol / Line / Bracket, Go Back / Forward, Reopen Closed Editor,
  Open Recent, Zen mode, tab pinning and the tab right-click menu, split editors, compare two files.
- **Workbench:** a menu bar (File, Edit, Selection, View, Go, Run, Terminal, Help); the Explorer with New File / New Folder /
  Refresh / Collapse buttons and right-click menus for files, folders and empty space (Open to the Side, Open in Integrated
  Terminal, Find in Folder, Cut / Copy / Paste, Select for Compare, File History, Copy Remote File URL); a Settings page;
  resizable and remembered panels; Codicons icons with hover tooltips; session restore; an integrated terminal with several
  tabs and scrollback; Markdown preview with scroll sync; minimap; word wrap; an image viewer and a hex view for binary files.
- **Language servers:** completion, hover, signature help, go to definition (with a picker), references, rename, quick fix,
  diagnostics; servers start automatically.
- **Source Control:** stage, commit, push, publish, pull, sync, fetch, stash, branches, history, blame, AI commit messages
  (OpenAI or Gemini), side-by-side diffs, hunk and line staging (also new files, with discard), merge-conflict resolution with a
  base column, rebase (including interactive), merge, cherry-pick and revert with continue / skip / abort, login and
  GPG-passphrase prompts, cancel, commit graph, file timeline, submodules, worktrees.
- **Debugging:** Python, JavaScript / Node and Rust.
- **Extensions:** VS Code extensions from Open VSX (themes, languages, grammars, snippets, and a partial `vscode` API).
- **Highlighting:** 29 languages.

**Not built yet:**

- **Source control:** interactive rebase steps `edit` and `break` and `--rebase-merges`; accepting the base side or editing a
  merged result in the conflict view (it shows the base but only keeps current, incoming or both); actions on a commit inside
  the graph (checkout, cherry-pick, branch from it); submodule-specific commits and a Source Control view per submodule; adding a
  worktree for a new or remote branch; stash management beyond push and pop; hardware-token PIN prompts for signing.
- **Workbench:** multi-root workspaces (so no Add / Remove Folder from Workspace), drag-and-drop of tabs or text (files and folders in the Explorer can be dragged), restoring
  unsaved edits or a second editor group's tabs after a restart, Alt+letter access keys for the menu bar, Share / Open Changes /
  Open on Remote / Timeline / Add to Chat entries in the Explorer menus.
- **Language features:** inlay hints, semantic highlighting, Peek Definition / References, Call Hierarchy, Type
  Hierarchy, Code Lens, Linked Editing, workspace symbol search (Ctrl+T), a permanent Outline view (only a symbol picker),
  bookmarks, a Search Editor, spell checking.
- **Not verified against a running server:** Format Selection and Organize Imports (they build and send the requests, but were
  not tried with a language server); mouse-driven checks of some hover tooltips (verified by keyboard replay only); Cut / Copy /
  Paste in the Explorer (unit-tested, not tried by mouse); dropping files from Windows Explorer into the Explorer (the drop is
  wired up but could not be tried here); Exit with unsaved changes.
- **Auto save** only saves the file in front (afterDelay) or all modified files on a tab / file switch (onFocusChange); format on
  save is not run by it.
- **Debugging:** only Python, JavaScript / Node and Rust; no conditional breakpoints or watch expressions.
- **Extensions:** webviews, tree views, terminals, notebooks, debugging and tasks APIs, language-feature providers, and VS Code's
  own extensions as language servers. No third-party extension has been run yet. Plan: `docs/EXTENSIONS_PLAN.md`.
- **Text and files:** layout assumes a monospace font, one cell per char (tabs = 4); files must be UTF-8 (a leading BOM is kept
  out of the text and written back on save) and under 64 MiB; highlighting is skipped above 4 MiB.
- **Highlighting:** embedded code is highlighted (`<script>` / `<style>` in HTML, fenced code in Markdown) but not above 512 KiB,
  and not inside template strings, `style=""` attributes or PHP / HTML mixes. SCSS / Less use the CSS grammar and `.h` files are
  treated as C.
- **Completions** from language servers are inserted as plain text (a server's own snippet-format completions are not expanded); snippet transforms do not update live as you type.
- **Language servers:** rust-analyzer, gopls, clangd, jdtls, csharp-ls, ruby-lsp, lua-language-server, taplo and sqls are
  configured but you install them yourself; `.env`, INI, Makefile, ignore files and diffs get highlighting only.
- **Packaging:** the installer in `dist\` was built before the latest changes and has not been rebuilt.

## Performance notes

The editing aids run on every keystroke, so they are kept cheap (release-build timings on a 1.8 MB, 25,000-line file unless noted):

| Work | Before | After | How |
|---|---|---|---|
| Bracket nesting for the whole file (after each edit) | 16.2 ms | 3.6 ms | bytes scanned in place, no per-line allocation |
| Word suggestions on a 300 KB file | 2.2 ms | 0.8 ms | no string built for words that do not match; only 200,000 chars around the caret are read |
| Auto rename tags while typing text | whole file read and scanned (about 3 ms+) | a look at the 80 chars before the caret | the file is read only when the caret is in a tag name |
| `>` / `</` closing tags | up to 200,000 chars copied | 4,000 chars for `>`; nothing for a slash not after `<` | |
| Sticky scroll lines | highlighted again every frame | highlighted once per text change | cached per document revision and theme |
| Snippet matching as you type | lower-cased every prefix per key | lower-cased once at load | |

Run the timings yourself: `cargo test --release -p forge-text bench -- --ignored --nocapture` and
`cargo test --release -p forge-core --test bench -- --ignored --nocapture`. Parsing 100,000 CSV rows (7.7 MB) takes about 130 ms, once, when the preview opens.

### Whole-project profile

Measured on this machine (the `cargo build` profile, a 21,000-line / 0.8 MB Rust file in a git repository for typing, the Forge repository for startup):

| Work | Before | After | How |
|---|---|---|---|
| Building the minimap while typing in a 21,000-line file | 4 to 12 ms per frame (grew with the file) | 0.00 ms | only the lines an edit touched are measured again |
| Time to build one frame while typing in that file | 9 to 17 ms | 2 to 5 ms | the above, and the next two rows |
| Colouring the visible lines (asked for 3 times per frame) | 3 x 0.65 ms | 1 x 0.65 ms | the result is kept for the frame and, when the text is unchanged, for a few dozen lines around it |
| Opening a second file in a language already used | 17 ms (Rust) to 5 ms (others) to compile the highlight query | 3 microseconds | the compiled query is shared by every document of a language |
| Starting the window (first frame) | about 960 ms | about 760 ms | git is found on a thread (was 90 ms, two process starts); the gpg lookup waits until a git command needs it (was 58 ms); restored tabs that are not in front are parsed when first shown |
| Choosing the fonts | listed every installed font on every frame | once, and again when the font settings change | |

Where the rest goes: about 670 ms of startup is GPUI starting up (graphics device, fonts, window), which Forge cannot change; a tree-sitter reparse after a keystroke takes about 4 ms in a 0.8 MB file (8 ms in the dev profile) because the parser walks the file's 9,000 top-level nodes. Idle CPU is about 1.5 to 2% (no frames are built while nothing happens; the 50 ms background tick costs about 12 microseconds); an adaptive tick was tried and removed because it did not lower it.

## Testing aid

`FORGE_PROFILE=1` prints, every five seconds, the frames built per second and where the time went (folds, minimap, highlight, syntax reparse, opening a file...), and the startup phases with their times. `FORGE_REPLAY="ctrl+p text:main enter"` feeds keystrokes through the normal key handler after startup
(`FORGE_REPLAY_DELAY_MS` sets the initial delay). Tokens are chords (`ctrl+shift+p`, `f2`), `text:...` to
type, and `wait:MS`.
