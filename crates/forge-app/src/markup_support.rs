//! Editing help for markup (HTML, XML, JSX, TSX): closing tags as you type and renaming a tag's partner.

use forge_core::tags;
use forge_syntax::Language;
use forge_text::{Selection, SelectionSet};
use gpui::Context;

use crate::workspace::Workspace;

/// Files bigger than this are not scanned on every keystroke.
const MAX_SCAN_CHARS: usize = 400_000;

impl Workspace {
    /// `Some(jsx)` when the focused file is markup: `jsx` is true for `.jsx` / `.tsx`.
    pub fn markup_kind(&self) -> Option<bool> {
        let doc = self.active_doc()?;
        match doc.language {
            Some(Language::Html | Language::Xml) => Some(false),
            Some(Language::JavaScriptReact | Language::TypeScriptReact) => Some(true),
            Some(_) => None,
            None => doc.ext_lang.as_ref().filter(|l| matches!(l.id.as_str(), "vue" | "svelte" | "astro" | "handlebars" | "razor")).map(|_| false),
        }
    }

    /// After `text` was typed: `>` closes the tag just opened, `</` completes the innermost open tag.
    pub fn after_type_markup(&mut self, text: &str, cx: &mut Context<Self>) {
        let Some(jsx) = self.markup_kind() else { return };
        if !self.settings.auto_closing_tags || (text != ">" && text != "/") {
            return;
        }
        let Some(view) = self.active_view() else { return };
        let Some(doc) = self.active_doc() else { return };
        if view.selections.as_slice().len() != 1 || !view.selections.primary().is_empty() {
            return;
        }
        let caret = view.selections.primary().head;
        let rope = doc.buffer.rope();
        let start = caret.saturating_sub(200_000);
        let before: Vec<char> = rope.slice(start..caret).chars().collect();
        let after: String = rope.slice(caret..(caret + 64).min(rope.len_chars())).to_string();
        let insert = if text == ">" {
            match tags::closing_for_open(&before, jsx) {
                Some(close) if !after.starts_with(&close) => close,
                _ => return,
            }
        } else {
            // `</` just typed: finish it with the name of the innermost open tag and the `>`.
            if before.len() < 2 || before[before.len() - 2] != '<' {
                return;
            }
            let Some(name) = tags::innermost_open(&before[..before.len() - 2], jsx) else { return };
            if after.starts_with('>') || after.starts_with(&name) {
                return;
            }
            format!("{name}>")
        };
        // The caret stays where it was: between the tags, or after the finished `</name>`.
        let keep_caret_before = text == ">";
        self.mutate(cx, move |b, s| {
            b.replace_range(s, caret, caret, &insert);
            let at = if keep_caret_before { caret } else { caret + insert.chars().count() };
            SelectionSet::single(Selection::caret(at))
        });
        self.merge_typing();
    }

    /// Tab after an Emmet abbreviation (`ul>li*3`, `m10`) in HTML, JSX / TSX or CSS: replace it with what it stands for.
    /// Returns whether it did.
    pub fn try_emmet(&mut self, cx: &mut Context<Self>) -> bool {
        if !self.settings.emmet {
            return false;
        }
        let Some(view) = self.active_view() else { return false };
        let Some(doc) = self.active_doc() else { return false };
        let selection = view.selections.primary();
        if view.selections.as_slice().len() != 1 || !selection.is_empty() {
            return false;
        }
        let css = matches!(doc.language, Some(Language::Css));
        let jsx = match doc.language {
            Some(Language::Html) => false,
            Some(Language::JavaScriptReact | Language::TypeScriptReact) => true,
            Some(Language::Css) => false,
            _ => return false,
        };
        let caret = selection.head;
        let rope = doc.buffer.rope();
        let (line, _) = doc.buffer.offset_to_point(caret);
        let line_start = doc.buffer.line_start(line);
        let before: Vec<char> = rope.slice(line_start..caret).chars().collect();
        if css {
            // Only inside a `{ ... }` block.
            let head: Vec<char> = rope.slice(caret.saturating_sub(100_000)..line_start).chars().collect();
            let depth = head.iter().filter(|c| **c == '{').count() as isize - head.iter().filter(|c| **c == '}').count() as isize;
            let open_on_line = before.iter().filter(|c| **c == '{').count() as isize - before.iter().filter(|c| **c == '}').count() as isize;
            if depth + open_on_line <= 0 {
                return false;
            }
        }
        // Walk back over the abbreviation: it ends at whitespace, outside any `[...]` or `{...}`.
        let mut depth = 0i32;
        let mut start = before.len();
        while start > 0 {
            let c = before[start - 1];
            match c {
                '}' | ']' => depth += 1,
                '{' | '[' => depth -= 1,
                _ => {}
            }
            if depth < 0 {
                break;
            }
            if depth == 0 && (c.is_whitespace() || matches!(c, '<' | ';' | '"' | '\'' | '`' | '=' | ',') || (css && matches!(c, '{' | '}'))) {
                break;
            }
            start -= 1;
        }
        let abbr: String = before[start..].iter().collect();
        if abbr.is_empty() {
            return false;
        }
        let style = self.indent_style();
        let indent = if style.use_spaces { " ".repeat(style.tab_size) } else { "\t".to_owned() };
        let expanded = if css { forge_core::emmet::expand_css(&abbr) } else { forge_core::emmet::expand_html(&abbr, &indent, jsx) };
        let Some(body) = expanded else { return false };
        let snippet = forge_core::snippet::Snippet { name: "Emmet".to_owned(), prefixes: Vec::new(), body, description: String::new() };
        self.insert_snippet(&snippet, (line_start + start, caret), cx);
        true
    }

    /// With the caret inside a tag's name, give the matching opening or closing tag the same name.
    pub fn sync_paired_tag(&mut self, cx: &mut Context<Self>) {
        if !self.settings.linked_editing {
            return;
        }
        let Some(jsx) = self.markup_kind() else { return };
        let Some(view) = self.active_view() else { return };
        let Some(doc) = self.active_doc() else { return };
        if view.selections.as_slice().len() != 1 || !view.selections.primary().is_empty() || doc.buffer.len_chars() > MAX_SCAN_CHARS {
            return;
        }
        let caret = view.selections.primary().head;
        let chars: Vec<char> = doc.buffer.rope().chars().collect();
        let Some((start, end, name)) = tags::rename_partner(&chars, caret, jsx) else { return };
        let delta = name.chars().count() as isize - (end - start) as isize;
        let new_caret = if start < caret { (caret as isize + delta).max(0) as usize } else { caret };
        self.mutate(cx, move |b, s| {
            b.replace_range(s, start, end, &name);
            SelectionSet::single(Selection::caret(new_caret))
        });
        self.merge_typing();
    }
}
