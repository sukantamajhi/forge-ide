//! Editor logic with no UI dependency: themes, settings, commands and keybindings,
//! text input, fuzzy matching, folding, project search and the terminal session.

pub mod ai;
pub mod cargo_build;
pub mod commands;
pub mod debug_install;
pub mod ext_lang;
pub mod fileops;
pub mod find;
pub mod folds;
pub mod fuzzy;
pub mod git;
pub mod gutter;
pub mod hexdump;
pub mod input;
pub mod jsonc;
pub mod markdown;
pub mod project_search;
pub mod quick;
pub mod servers;
pub mod session;
pub mod snippet;
pub mod settings;
pub mod terminal;
pub mod textarea;
pub mod theme;
pub mod ui_state;
pub mod icon_theme;
pub mod askpass;
pub mod patch;
pub mod conflict;
pub mod gpg_wrap;
pub mod graph;
pub mod product_icons;
pub mod vscode_theme;
