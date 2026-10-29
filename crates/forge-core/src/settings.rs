//! `settings.json`: a small, forgiving subset of VS Code's user settings.

use std::path::PathBuf;

use serde_json::Value;

#[derive(Clone, Debug, PartialEq)]
pub struct Settings {
    pub font_size: f32,
    pub tab_size: usize,
    pub insert_spaces: bool,
    pub auto_closing_brackets: bool,
    pub theme: String,
    pub trim_trailing_whitespace: bool,
    pub format_on_save: bool,
    /// `languageServers` in settings.json: VS Code language id to command and arguments.
    pub language_servers: std::collections::BTreeMap<String, (String, Vec<String>)>,
    /// `editor.minimap.enabled`: the overview strip beside the editor.
    pub minimap: bool,
    /// `editor.wordWrap`: wrap long lines at the editor's width.
    pub word_wrap: bool,
    /// `lsp.autoStart` in settings.json: start the servers for the languages found in the opened folder at launch.
    pub lsp_autostart: bool,
    /// `ai.model` in settings.json: the OpenAI model that writes commit messages.
    pub ai_model: String,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            font_size: 14.0,
            tab_size: 4,
            insert_spaces: true,
            auto_closing_brackets: true,
            theme: "Forge Dark".to_owned(),
            trim_trailing_whitespace: false,
            format_on_save: false,
            language_servers: Default::default(),
            minimap: true,
            word_wrap: false,
            lsp_autostart: true,
            ai_model: crate::ai::DEFAULT_MODEL.to_owned(),
        }
    }
}

/// Remove `//` line comments and `/* */` blocks outside strings so VS Code-style
/// "JSON with comments" parses.
pub fn strip_json_comments(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    let (mut in_string, mut escaped) = (false, false);
    while let Some(c) = chars.next() {
        if in_string {
            out.push(c);
            if escaped {
                escaped = false;
            } else if c == '\\' {
                escaped = true;
            } else if c == '"' {
                in_string = false;
            }
            continue;
        }
        match (c, chars.peek()) {
            ('"', _) => {
                in_string = true;
                out.push(c);
            }
            ('/', Some('/')) => {
                for n in chars.by_ref() {
                    if n == '\n' {
                        out.push('\n');
                        break;
                    }
                }
            }
            ('/', Some('*')) => {
                chars.next();
                let mut prev = ' ';
                for n in chars.by_ref() {
                    if prev == '*' && n == '/' {
                        break;
                    }
                    if n == '\n' {
                        out.push('\n');
                    }
                    prev = n;
                }
            }
            _ => out.push(c),
        }
    }
    out
}

impl Settings {
    /// Unknown keys, wrong types and out-of-range values fall back to defaults. Invalid
    /// JSON yields all defaults.
    pub fn parse(text: &str) -> Self {
        let mut s = Self::default();
        let Ok(Value::Object(map)) = crate::jsonc::parse(text) else { return s };
        if let Some(v) = map.get("editor.fontSize").and_then(Value::as_f64) {
            s.font_size = (v as f32).clamp(8.0, 40.0);
        }
        if let Some(v) = map.get("editor.tabSize").and_then(Value::as_u64) {
            s.tab_size = (v as usize).clamp(1, 16);
        }
        if let Some(v) = map.get("editor.insertSpaces").and_then(Value::as_bool) {
            s.insert_spaces = v;
        }
        if let Some(v) = map.get("editor.autoClosingBrackets") {
            // VS Code uses strings ("always", "never"); accept those and booleans.
            s.auto_closing_brackets = match v {
                Value::Bool(b) => *b,
                Value::String(m) => m != "never",
                _ => s.auto_closing_brackets,
            };
        }
        if let Some(v) = map.get("workbench.colorTheme").and_then(Value::as_str) {
            s.theme = v.to_owned();
        }
        if let Some(v) = map.get("files.trimTrailingWhitespace").and_then(Value::as_bool) {
            s.trim_trailing_whitespace = v;
        }
        if let Some(v) = map.get("editor.formatOnSave").and_then(Value::as_bool) {
            s.format_on_save = v;
        }
        if let Some(v) = map.get("editor.minimap.enabled").and_then(Value::as_bool) {
            s.minimap = v;
        }
        if let Some(v) = map.get("editor.wordWrap") {
            // VS Code writes "on" / "off" (and "wordWrapColumn", "bounded"); booleans are accepted too.
            s.word_wrap = match v {
                Value::Bool(b) => *b,
                Value::String(m) => m != "off",
                _ => s.word_wrap,
            };
        }
        if let Some(v) = map.get("lsp.autoStart").and_then(Value::as_bool) {
            s.lsp_autostart = v;
        }
        if let Some(v) = map.get("ai.model").and_then(Value::as_str).map(str::trim).filter(|m| !m.is_empty()) {
            s.ai_model = v.to_owned();
        }
        if let Some(servers) = map.get("languageServers").and_then(Value::as_object) {
            for (id, entry) in servers {
                let Some(command) = entry.get("command").and_then(Value::as_str).filter(|c| !c.is_empty()) else { continue };
                let args = entry.get("args").and_then(Value::as_array).map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_owned)).collect()).unwrap_or_default();
                s.language_servers.insert(id.clone(), (command.to_owned(), args));
            }
        }
        s
    }

    /// Text for a freshly created `settings.json`.
    pub fn template() -> String {
        let d = Self::default();
        format!(
            "{{\n    // Editor font size in pixels (8-40)\n    \"editor.fontSize\": {},\n    \"editor.tabSize\": {},\n    \"editor.insertSpaces\": {},\n    \"editor.autoClosingBrackets\": \"always\",\n    // One of: Forge Dark, Forge Light, Forge High Contrast\n    \"workbench.colorTheme\": \"{}\",\n    \"files.trimTrailingWhitespace\": {},\n    \"editor.formatOnSave\": {}\n}}\n",
            d.font_size, d.tab_size, d.insert_spaces, d.theme, d.trim_trailing_whitespace, d.format_on_save
        )
    }
}

/// `text` (the contents of settings.json) with `key` set to `json_value` (already valid JSON).
///
/// Edits in place so comments, ordering and every other key survive: an existing `"key": value`
/// has just its value replaced, and a new key is added before the final `}`. Only when the file
/// cannot be edited safely this way (a trailing line comment on the last entry) is it rewritten
/// from its parsed content, which drops comments.
pub fn set_value(text: &str, key: &str, json_value: &str) -> String {
    let needle = format!("\"{key}\"");
    let mut offset = 0;
    for line in text.split_inclusive('\n') {
        let trimmed = line.trim_start();
        if !trimmed.starts_with("//") {
            if let Some(at) = line.find(&needle) {
                let after = &line[at + needle.len()..];
                let colon = after.trim_start();
                if let Some(rest) = colon.strip_prefix(':') {
                    let value_start = offset + at + needle.len() + (after.len() - colon.len()) + 1 + (rest.len() - rest.trim_start().len());
                    let value_end = offset + at + needle.len() + (after.len() - colon.len()) + 1 + value_length(rest);
                    return format!("{}{}{}", &text[..value_start], json_value, &text[value_end..]);
                }
            }
        }
        offset += line.len();
    }
    let Some(close) = text.rfind('}') else {
        return format!("{{\n    {needle}: {json_value}\n}}\n");
    };
    let before = text[..close].trim_end();
    let last_line = before.rsplit('\n').next().unwrap_or("");
    if last_line.contains("//") {
        let mut map = match crate::jsonc::parse(text) {
            Ok(Value::Object(map)) => map,
            _ => serde_json::Map::new(),
        };
        map.insert(key.to_owned(), serde_json::from_str(json_value).unwrap_or(Value::Null));
        return serde_json::to_string_pretty(&Value::Object(map)).unwrap_or_default() + "\n";
    }
    let comma = if before.ends_with('{') || before.ends_with(',') { "" } else { "," };
    format!("{before}{comma}\n    {needle}: {json_value}\n{}", &text[close..])
}

/// Byte length of the JSON value at the start of `rest` (after the colon): a string, or a bare
/// number/boolean/null up to the next `,`, `}` or line end.
fn value_length(rest: &str) -> usize {
    let lead = rest.len() - rest.trim_start().len();
    let body = &rest[lead..];
    if let Some(inner) = body.strip_prefix('"') {
        let mut escaped = false;
        for (i, c) in inner.char_indices() {
            match c {
                '\\' if !escaped => escaped = true,
                '"' if !escaped => return lead + i + 2,
                _ => escaped = false,
            }
        }
        return rest.len();
    }
    let end = body.find([',', '}', '\n', '/']).unwrap_or(body.len());
    lead + body[..end].trim_end().len()
}

/// Where user configuration lives: `%APPDATA%\Forge` on Windows, `~/.config/forge` elsewhere
/// (or `FORGE_CONFIG_DIR`).
pub fn config_dir() -> PathBuf {
    // `FORGE_CONFIG_DIR` relocates everything (tests, portable installs).
    if let Some(dir) = std::env::var_os("FORGE_CONFIG_DIR").filter(|d| !d.is_empty()) {
        return PathBuf::from(dir);
    }
    let base = if cfg!(windows) {
        std::env::var_os("APPDATA").map(PathBuf::from)
    } else {
        std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
    };
    base.unwrap_or_else(std::env::temp_dir).join(if cfg!(windows) { "Forge" } else { "forge" })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_known_keys_with_comments_and_clamps() {
        let text = r#"{
            // font
            "editor.fontSize": 99, /* too big */
            "editor.tabSize": 2,
            "editor.insertSpaces": false,
            "editor.autoClosingBrackets": "never",
            "workbench.colorTheme": "Forge Light",
            "files.trimTrailingWhitespace": true,
            "url": "http://not-a-comment.example", "unknown": 1
        }"#;
        let s = Settings::parse(text);
        assert_eq!(s.font_size, 40.0);
        assert_eq!(s.tab_size, 2);
        assert!(!s.insert_spaces && !s.auto_closing_brackets && s.trim_trailing_whitespace);
        assert_eq!(s.theme, "Forge Light");
    }

    #[test]
    fn invalid_or_wrongly_typed_input_keeps_defaults() {
        assert_eq!(Settings::parse("{ not json"), Settings::default());
        // Trailing commas are fine in VS Code settings files.
        assert_eq!(Settings::parse(r#"{ "editor.tabSize": 2, }"#).tab_size, 2);
        assert_eq!(Settings::parse("[1,2]"), Settings::default());
        let s = Settings::parse(r#"{ "editor.tabSize": "four", "editor.fontSize": null }"#);
        assert_eq!(s, Settings::default());
    }

    #[test]
    fn language_server_overrides_parse_and_skip_entries_without_a_command() {
        let s = Settings::parse(r#"{ "languageServers": {
            "python": { "command": "pylsp", "args": ["-v"] },
            "go": { "command": "gopls" },
            "lua": { "args": ["x"] },
            "ruby": "solargraph"
        } }"#);
        assert_eq!(s.language_servers.len(), 2);
        assert_eq!(s.language_servers["python"], ("pylsp".to_owned(), vec!["-v".to_owned()]));
        assert_eq!(s.language_servers["go"], ("gopls".to_owned(), vec![]));
    }

    #[test]
    fn the_template_parses_back_to_the_defaults() {
        assert_eq!(Settings::parse(&Settings::template()), Settings::default());
    }

    #[test]
    fn comment_stripping_leaves_strings_alone() {
        assert_eq!(strip_json_comments(r#"{"a": "x//y" // c
}"#), "{\"a\": \"x//y\" \n}");
        assert_eq!(strip_json_comments(r#""q\"//still string""#), r#""q\"//still string""#);
    }

    #[test]
    fn set_value_replaces_in_place_and_keeps_comments() {
        let text = "{\n    // size in pixels\n    \"editor.fontSize\": 14, // keep\n    \"workbench.colorTheme\": \"Forge Dark\",\n    \"editor.insertSpaces\": true\n}\n";
        let out = set_value(text, "editor.fontSize", "18");
        assert_eq!(out, "{\n    // size in pixels\n    \"editor.fontSize\": 18, // keep\n    \"workbench.colorTheme\": \"Forge Dark\",\n    \"editor.insertSpaces\": true\n}\n");
        let out = set_value(&out, "workbench.colorTheme", "\"Forge Light\"");
        assert!(out.contains("\"workbench.colorTheme\": \"Forge Light\","), "{out}");
        let out = set_value(&out, "editor.insertSpaces", "false");
        assert!(out.contains("\"editor.insertSpaces\": false\n}"), "{out}");
        assert_eq!(Settings::parse(&out).font_size, 18.0);
        assert!(!Settings::parse(&out).insert_spaces);
    }

    #[test]
    fn set_value_ignores_commented_out_keys_and_adds_new_ones() {
        let text = "{\n    // \"editor.tabSize\": 8,\n    \"editor.fontSize\": 14\n}\n";
        let out = set_value(text, "editor.tabSize", "2");
        assert!(out.contains("// \"editor.tabSize\": 8,"), "the comment is untouched: {out}");
        assert!(out.contains("\"editor.fontSize\": 14,\n    \"editor.tabSize\": 2\n}"), "{out}");
        assert_eq!(Settings::parse(&out).tab_size, 2);
        let empty = set_value("{}", "ai.model", "\"gpt-5-nano\"");
        assert_eq!(empty, "{\n    \"ai.model\": \"gpt-5-nano\"\n}");
        assert_eq!(Settings::parse(&set_value("", "ai.model", "\"x\"")).ai_model, "x");
    }

    #[test]
    fn set_value_falls_back_to_a_rewrite_when_the_last_entry_has_a_comment() {
        let out = set_value("{\n    \"editor.fontSize\": 14 // last\n}\n", "editor.tabSize", "2");
        let parsed = Settings::parse(&out);
        assert_eq!((parsed.font_size, parsed.tab_size), (14.0, 2), "{out}");
    }
}
