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
        let Ok(Value::Object(map)) = serde_json::from_str::<Value>(&strip_json_comments(text)) else { return s };
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

/// Where user configuration lives: `%APPDATA%\Forge` on Windows, `~/.config/forge` elsewhere.
pub fn config_dir() -> PathBuf {
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
}
