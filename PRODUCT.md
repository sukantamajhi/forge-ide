# Product

<!-- impeccable:product-schema 1 -->

## Platform

web

## Stack

Static HTML/CSS/JS in a `site/` folder with separate CSS, JS and assets (user's choice). No build step.

## Users

Developers who live in an editor all day and are willing to try an early-stage one: Rust and systems programmers, people who like fast native tools (Zed-class), and VS Code users who want familiar editing without Electron.

## Product Purpose

Forge is an original, GPU-rendered code editor written in Rust on GPUI, aiming for Zed-class responsiveness with VS Code-style editing. Success for the landing page: a visitor understands what Forge is and what it already does, and takes one of three actions: download the Windows installer, star or read the repo, or join an early-access list.

## Positioning

A native Rust/GPUI editor that still installs VS Code extensions from Open VSX and runs their code in a Node extension host, with VS Code command ids and keybindings, so switching costs little. Windows-only for now.

## Operating Context

Windows 10/11. Per-user installer (no admin), Start menu entry, optional "Open with Forge" and PATH. Settings in `%APPDATA%\Forge`. Unsigned installer (SmartScreen may warn).

## Capabilities and Constraints

Documented in README.md: multi-cursor and VS Code-style editing, find/replace and Find in Files, Quick Open and Command Palette, LSP features (diagnostics, completion, hover, go to definition, references, rename, quick fix, format), source control view, Markdown preview, split editors and tab menus, resizable panels, debugging for Python, Node and Rust, integrated terminals, minimap, word wrap, three themes, settings/keybindings JSON, extension install from Open VSX or .vsix with a partial `vscode` API, AI-written commit messages (OpenAI key, DPAPI-encrypted, staged diff only, on request), highlighting for 29 languages. Only a part of the `vscode` API exists; webviews, tree views, notebooks are not supported. Free and open source under MIT (Cargo.toml). Windows-only for now.

## Brand Commitments

Name: Forge. Logo: `assets/forge-logo.svg` (an anvil shaped like the letter F, with a spark); icon `assets/forge.ico`. Repository: https://github.com/sukantamajhi/forge-ide. Claims limited to what README.md states.

## Evidence on Hand

Installer `dist/Forge-Setup-0.1.0.exe`. A real screenshot of the extensions page (user-supplied, in conversation only; not in the repo). No testimonials, customers, benchmarks, pricing or release-hosting URL exist: do not fabricate. No waitlist backend exists; the form needs an endpoint from the owner.

## Product Principles

- Say only what the README says; show the real editor doing real things.
- Native speed is the reason to try it; VS Code familiarity is the reason to stay.
- Be plain about what is not built yet.
- Three equal doors: download, read the source, join the list.

## Accessibility & Inclusion

None specified beyond standard web accessibility (contrast, keyboard, reduced motion).
