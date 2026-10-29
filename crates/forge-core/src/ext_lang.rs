//! Languages contributed by extensions: how to recognise their files and their
//! `language-configuration.json` (comment tokens and bracket pairs).
//!
//! Languages Forge bundles (Rust, TypeScript, ...) are not described here; this is for the
//! rest, which have no tree-sitter grammar and are colored by a TextMate grammar if the
//! extension ships one.

use serde_json::Value;

use crate::jsonc;

/// What `language-configuration.json` tells the editor about a language.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LanguageConfig {
    pub line_comment: Option<String>,
    pub block_comment: Option<(String, String)>,
    pub brackets: Vec<(String, String)>,
    pub auto_closing: Vec<(String, String)>,
}

fn pair(value: &Value) -> Option<(String, String)> {
    match value {
        Value::Array(items) if items.len() == 2 => Some((items[0].as_str()?.to_owned(), items[1].as_str()?.to_owned())),
        Value::Object(map) => Some((map.get("open")?.as_str()?.to_owned(), map.get("close")?.as_str()?.to_owned())),
        _ => None,
    }
}

pub fn parse_language_configuration(text: &str) -> Result<LanguageConfig, String> {
    let value = jsonc::parse(text)?;
    let mut config = LanguageConfig::default();
    if let Some(comments) = value.get("comments") {
        config.line_comment = comments.get("lineComment").and_then(Value::as_str).filter(|c| !c.is_empty()).map(str::to_owned);
        config.block_comment = comments.get("blockComment").and_then(pair);
    }
    let pairs = |key: &str| value.get(key).and_then(Value::as_array).map(|a| a.iter().filter_map(pair).collect::<Vec<_>>()).unwrap_or_default();
    config.brackets = pairs("brackets");
    config.auto_closing = pairs("autoClosingPairs");
    // VS Code closes the declared brackets automatically when no pairs are listed.
    if config.auto_closing.is_empty() {
        config.auto_closing = config.brackets.clone();
    }
    Ok(config)
}

/// A language an extension contributes.
#[derive(Clone, Debug, Default)]
pub struct ExtLanguage {
    pub id: String,
    /// Shown in the status bar.
    pub name: String,
    /// Extensions with the dot, as in a manifest (`.zig`).
    pub extensions: Vec<String>,
    pub filenames: Vec<String>,
    /// Glob patterns such as `*.env` or `Dockerfile.*`.
    pub patterns: Vec<String>,
    pub config: LanguageConfig,
    /// The name its TextMate grammar is registered under, if it has one.
    pub grammar: Option<String>,
}

/// `*` (any run of characters) and `?` (one character), ignoring case.
pub fn glob_match(pattern: &str, text: &str) -> bool {
    fn go(pattern: &[char], text: &[char]) -> bool {
        match pattern.split_first() {
            None => text.is_empty(),
            Some(('*', rest)) => (0..=text.len()).any(|skip| go(rest, &text[skip..])),
            Some(('?', rest)) => !text.is_empty() && go(rest, &text[1..]),
            Some((c, rest)) => text.first().is_some_and(|t| t == c) && go(rest, &text[1..]),
        }
    }
    let pattern: Vec<char> = pattern.to_lowercase().chars().collect();
    let text: Vec<char> = text.to_lowercase().chars().collect();
    go(&pattern, &text)
}

/// The extension language that owns the file called `file_name`: an exact file name first,
/// then a glob pattern, then the longest matching extension (`.d.ts` before `.ts`).
pub fn detect<'a>(languages: &'a [ExtLanguage], file_name: &str) -> Option<&'a ExtLanguage> {
    let lower = file_name.to_lowercase();
    languages
        .iter()
        .find(|l| l.filenames.iter().any(|f| f.to_lowercase() == lower))
        .or_else(|| languages.iter().find(|l| l.patterns.iter().any(|p| glob_match(p, file_name))))
        .or_else(|| {
            languages
                .iter()
                .flat_map(|l| l.extensions.iter().map(move |e| (l, e)))
                .filter(|(_, e)| e.len() > 1 && lower.ends_with(&e.to_lowercase()))
                .max_by_key(|(_, e)| e.len())
                .map(|(l, _)| l)
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn language_configuration_reads_comments_and_pairs() {
        let text = r#"{
            // comments are allowed
            "comments": { "lineComment": "//", "blockComment": ["/*", "*/"] },
            "brackets": [["{", "}"], ["[", "]"], ["(", ")"]],
            "autoClosingPairs": [
                { "open": "{", "close": "}" },
                ["(", ")"],
                { "open": "'", "close": "'", "notIn": ["string", "comment"] },
            ],
        }"#;
        let config = parse_language_configuration(text).unwrap();
        assert_eq!(config.line_comment.as_deref(), Some("//"));
        assert_eq!(config.block_comment, Some(("/*".to_owned(), "*/".to_owned())));
        assert_eq!(config.brackets.len(), 3);
        assert_eq!(config.auto_closing, vec![("{".into(), "}".into()), ("(".into(), ")".into()), ("'".into(), "'".into())]);
    }

    #[test]
    fn missing_sections_give_defaults_and_brackets_stand_in_for_pairs() {
        assert_eq!(parse_language_configuration("{}").unwrap(), LanguageConfig::default());
        let only_brackets = parse_language_configuration(r#"{ "brackets": [["<", ">"]], "comments": { "lineComment": "" } }"#).unwrap();
        assert_eq!(only_brackets.line_comment, None, "an empty token is no token");
        assert_eq!(only_brackets.auto_closing, vec![("<".into(), ">".into())]);
        // Malformed entries are skipped, malformed files are errors.
        let odd = parse_language_configuration(r#"{ "brackets": [["a"], 5, ["(", ")"]] }"#).unwrap();
        assert_eq!(odd.brackets, vec![("(".into(), ")".into())]);
        assert!(parse_language_configuration("{ nope").is_err());
    }

    #[test]
    fn globs_match_stars_and_question_marks_ignoring_case() {
        assert!(glob_match("*.env", "prod.ENV"));
        assert!(glob_match("Dockerfile.*", "dockerfile.dev"));
        assert!(glob_match("?.txt", "a.txt"));
        assert!(!glob_match("?.txt", "ab.txt"));
        assert!(glob_match("*", ""));
        assert!(!glob_match("a*b", "a-c"));
        assert!(glob_match("a*b*c", "aXXbYYc"));
    }

    fn lang(id: &str, extensions: &[&str], filenames: &[&str], patterns: &[&str]) -> ExtLanguage {
        let owned = |items: &[&str]| items.iter().map(|s| (*s).to_owned()).collect();
        ExtLanguage { id: id.to_owned(), name: id.to_owned(), extensions: owned(extensions), filenames: owned(filenames), patterns: owned(patterns), ..Default::default() }
    }

    #[test]
    fn files_are_matched_by_name_then_pattern_then_longest_extension() {
        let languages = vec![
            lang("zig", &[".zig"], &[], &[]),
            lang("build", &[], &["build.zig.zon"], &[]),
            lang("dts", &[".d.ts"], &[], &[]),
            lang("ts-ish", &[".ts"], &[], &[]),
            lang("logs", &[], &[], &["*.log.*"]),
        ];
        let id = |name: &str| detect(&languages, name).map(|l| l.id.as_str());
        assert_eq!(id("main.zig"), Some("zig"));
        assert_eq!(id("MAIN.ZIG"), Some("zig"));
        assert_eq!(id("build.zig.zon"), Some("build"), "an exact name beats the extension");
        assert_eq!(id("types.d.ts"), Some("dts"), "the longest extension wins");
        assert_eq!(id("a.ts"), Some("ts-ish"));
        assert_eq!(id("app.log.1"), Some("logs"));
        assert_eq!(id("readme"), None);
        assert_eq!(id(".zig"), Some("zig"));
    }

    #[test]
    fn a_bare_dot_is_not_an_extension() {
        let languages = vec![lang("x", &["."], &[], &[])];
        assert!(detect(&languages, "a.").is_none());
    }
}
