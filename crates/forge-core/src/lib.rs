//! Editor logic with no UI dependency: themes, settings, commands and keybindings,
//! text input, fuzzy matching, folding, project search and the terminal session.

pub mod commands;
pub mod ext_lang;
pub mod fileops;
pub mod find;
pub mod folds;
pub mod fuzzy;
pub mod git;
pub mod gutter;
pub mod input;
pub mod jsonc;
pub mod project_search;
pub mod quick;
pub mod servers;
pub mod snippet;
pub mod settings;
pub mod terminal;
pub mod theme;
pub mod vscode_theme;
