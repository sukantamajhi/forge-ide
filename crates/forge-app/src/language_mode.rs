//! Choosing a file's language: the picker behind the status bar's language, with Auto Detect.

use forge_core::fuzzy::rank;
use forge_syntax::Language;

use crate::workspace::{Item, ItemAction, LangChoice, Workspace};

impl Workspace {
    /// What the status bar calls the active file's language.
    pub fn language_label(&self) -> String {
        self.active_doc()
            .map(|doc| doc.syntax.as_ref().map(|s| s.language().name().to_owned()).or_else(|| doc.ext_lang.as_ref().map(|l| l.name.clone())).unwrap_or_else(|| "Plain Text".to_owned()))
            .unwrap_or_default()
    }

    /// Auto Detect, then every language Forge knows, then Plain Text; the current one is marked.
    pub fn language_items(&self, query: &str) -> Vec<Item> {
        let Some(doc) = self.active_doc() else { return Vec::new() };
        let current = self.language_label();
        let mut choices: Vec<(String, String, LangChoice)> = Vec::new();
        let auto_detail = if doc.lang_manual { "Guess it from the file name or contents".to_owned() } else { format!("Current: {current} (guessed from the file name or contents)") };
        choices.push(("Auto Detect".to_owned(), auto_detail, LangChoice::Auto));
        for language in Language::ALL {
            let mark = if doc.lang_manual && doc.language == Some(*language) { "Current".to_owned() } else { language.vscode_id().to_owned() };
            choices.push((language.name().to_owned(), mark, LangChoice::Lang(*language)));
        }
        let plain_mark = if doc.lang_manual && doc.language.is_none() { "Current" } else { "plaintext" };
        choices.push(("Plain Text".to_owned(), plain_mark.to_owned(), LangChoice::PlainText));
        let names: Vec<&str> = choices.iter().map(|c| c.0.as_str()).collect();
        rank(query, &names, 60)
            .into_iter()
            .map(|(i, m)| {
                let (label, detail, choice) = &choices[i];
                Item { label: label.clone(), detail: detail.clone(), positions: m.positions, action: ItemAction::SetLanguage(*choice) }
            })
            .collect()
    }
}
