//! "JSON with comments", as VS Code reads its configuration files: `//` and `/* */`
//! comments and trailing commas are allowed. Themes, snippets, language configurations
//! and settings all use it.

use serde_json::Value;

use crate::settings::strip_json_comments;

/// Remove a comma that is followed only by whitespace and a closing `}` or `]`.
/// Commas inside strings are left alone.
pub fn strip_trailing_commas(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    let (mut in_string, mut escaped) = (false, false);
    for (i, &c) in chars.iter().enumerate() {
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
        match c {
            '"' => {
                in_string = true;
                out.push(c);
            }
            ',' if chars[i + 1..].iter().find(|n| !n.is_whitespace()).is_some_and(|n| matches!(n, '}' | ']')) => {}
            _ => out.push(c),
        }
    }
    out
}

/// Parse JSON that may carry comments, trailing commas and a byte-order mark.
pub fn parse(text: &str) -> Result<Value, String> {
    let cleaned = strip_trailing_commas(&strip_json_comments(text.trim_start_matches('\u{feff}')));
    serde_json::from_str(&cleaned).map_err(|e| format!("invalid JSON: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn comments_trailing_commas_and_a_bom_are_accepted() {
        let text = "\u{feff}{\n  // a comment\n  \"a\": [1, 2, 3,],\n  \"b\": { \"c\": 1, /* x */ },\n}\n";
        assert_eq!(parse(text).unwrap(), json!({ "a": [1, 2, 3], "b": { "c": 1 } }));
    }

    #[test]
    fn commas_and_comment_markers_inside_strings_survive() {
        let text = r#"{ "url": "http://x.example/a,]", "list": ["a,", "b",] }"#;
        assert_eq!(parse(text).unwrap(), json!({ "url": "http://x.example/a,]", "list": ["a,", "b"] }));
        assert_eq!(parse(r#"{ "q": "say \",\" }" , }"#).unwrap(), json!({ "q": "say \",\" }" }));
    }

    #[test]
    fn genuinely_broken_json_is_still_an_error() {
        assert!(parse("{ \"a\": }").is_err());
        assert!(parse("{ \"a\": 1").is_err());
        assert!(parse("").is_err());
        // A comma that is not trailing is still an error.
        assert!(parse("[1,,2]").is_err());
    }
}
