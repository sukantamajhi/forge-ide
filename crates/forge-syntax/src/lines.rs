//! Highlighting for formats without a tree-sitter grammar here, done one line at a time.
//! Spans are char columns, ascending and non-overlapping.

use crate::{HighlightClass, Language, Span};

struct Out {
    spans: Vec<Span>,
}

impl Out {
    fn new() -> Self {
        Self { spans: Vec::new() }
    }

    fn push(&mut self, start: usize, end: usize, class: HighlightClass) {
        if end > start && self.spans.last().is_none_or(|last| last.end_col <= start) {
            self.spans.push(Span { start_col: start, end_col: end, class });
        }
    }
}

pub fn highlight_line(language: Language, line: &str) -> Vec<Span> {
    let chars: Vec<char> = line.trim_end_matches(['\r', '\n']).chars().collect();
    let mut out = Out::new();
    match language {
        Language::Env => env_line(&chars, &mut out),
        Language::Ini => ini_line(&chars, &mut out),
        Language::Dockerfile => dockerfile_line(&chars, &mut out),
        Language::Makefile => makefile_line(&chars, &mut out),
        Language::Ignore => ignore_line(&chars, &mut out),
        Language::Diff => diff_line(&chars, &mut out),
        _ => {}
    }
    out.spans
}

fn skip_ws(chars: &[char], mut i: usize) -> usize {
    while i < chars.len() && chars[i].is_whitespace() {
        i += 1;
    }
    i
}

fn is_ident(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// `$VAR` and `${VAR}` references inside `start..end`, as `(from, to)`.
fn variable_refs(chars: &[char], start: usize, end: usize) -> Vec<(usize, usize)> {
    let mut found = Vec::new();
    let mut i = start;
    while i < end {
        if chars[i] == '$' && i + 1 < end {
            if chars[i + 1] == '{' {
                if let Some(close) = (i + 2..end).find(|j| chars[*j] == '}') {
                    found.push((i, close + 1));
                    i = close + 1;
                    continue;
                }
            } else if is_ident(chars[i + 1]) {
                let mut j = i + 1;
                while j < end && is_ident(chars[j]) {
                    j += 1;
                }
                found.push((i, j));
                i = j;
                continue;
            }
        }
        i += 1;
    }
    found
}

/// A string `start..end` with variable references in a separate class.
fn string_with_vars(chars: &[char], start: usize, end: usize, class: HighlightClass, out: &mut Out) {
    let mut at = start;
    for (from, to) in variable_refs(chars, start, end) {
        out.push(at, from, class);
        out.push(from, to, HighlightClass::Escape);
        at = to;
    }
    out.push(at, end, class);
}

fn literal_class(text: &str) -> HighlightClass {
    let lower = text.to_ascii_lowercase();
    if text.parse::<f64>().is_ok() {
        HighlightClass::Number
    } else if matches!(lower.as_str(), "true" | "false" | "null" | "yes" | "no" | "on" | "off") {
        HighlightClass::Constant
    } else {
        HighlightClass::String
    }
}

// ---- .env ----

fn env_line(chars: &[char], out: &mut Out) {
    let start = skip_ws(chars, 0);
    if start >= chars.len() {
        return;
    }
    if chars[start] == '#' {
        out.push(start, chars.len(), HighlightClass::Comment);
        return;
    }
    let mut p = start;
    let export: Vec<char> = "export".chars().collect();
    if chars[p..].starts_with(&export) && chars.get(p + 6).is_some_and(|c| c.is_whitespace()) {
        out.push(p, p + 6, HighlightClass::Keyword);
        p = skip_ws(chars, p + 6);
    }
    let Some(eq) = (p..chars.len()).find(|i| chars[*i] == '=') else {
        // A bare name (some tools allow it) is still a variable.
        let end = (p..chars.len()).find(|i| chars[*i].is_whitespace()).unwrap_or(chars.len());
        out.push(p, end, HighlightClass::Property);
        return;
    };
    let mut key_end = eq;
    while key_end > p && chars[key_end - 1].is_whitespace() {
        key_end -= 1;
    }
    out.push(p, key_end, HighlightClass::Property);
    out.push(eq, eq + 1, HighlightClass::Operator);

    let v = skip_ws(chars, eq + 1);
    if v >= chars.len() {
        return;
    }
    let quote = chars[v];
    if matches!(quote, '"' | '\'' | '`') {
        // Find the closing quote; a backslash escapes inside double quotes.
        let mut i = v + 1;
        while i < chars.len() && chars[i] != quote {
            i += if quote == '"' && chars[i] == '\\' { 2 } else { 1 };
        }
        let end = (i + 1).min(chars.len());
        // Single quotes never interpolate.
        if quote == '\'' {
            out.push(v, end, HighlightClass::String);
        } else {
            string_with_vars(chars, v, end, HighlightClass::String, out);
        }
        let rest = skip_ws(chars, end);
        if rest < chars.len() && chars[rest] == '#' {
            out.push(rest, chars.len(), HighlightClass::Comment);
        }
        return;
    }
    // Unquoted: an inline comment starts at whitespace followed by '#'.
    let comment = (v + 1..chars.len()).find(|i| chars[*i] == '#' && chars[*i - 1].is_whitespace());
    let mut end = comment.unwrap_or(chars.len());
    while end > v && chars[end - 1].is_whitespace() {
        end -= 1;
    }
    let value: String = chars[v..end].iter().collect();
    match literal_class(&value) {
        HighlightClass::String => string_with_vars(chars, v, end, HighlightClass::String, out),
        class => out.push(v, end, class),
    }
    if let Some(c) = comment {
        out.push(c, chars.len(), HighlightClass::Comment);
    }
}

// ---- INI / .properties / .conf ----

fn ini_line(chars: &[char], out: &mut Out) {
    let start = skip_ws(chars, 0);
    if start >= chars.len() {
        return;
    }
    if matches!(chars[start], ';' | '#') {
        out.push(start, chars.len(), HighlightClass::Comment);
        return;
    }
    if chars[start] == '[' {
        let end = (start..chars.len()).find(|i| chars[*i] == ']').map_or(chars.len(), |i| i + 1);
        out.push(start, end, HighlightClass::Type);
        return;
    }
    let Some(sep) = (start..chars.len()).find(|i| matches!(chars[*i], '=' | ':')) else {
        out.push(start, chars.len(), HighlightClass::Property);
        return;
    };
    let mut key_end = sep;
    while key_end > start && chars[key_end - 1].is_whitespace() {
        key_end -= 1;
    }
    out.push(start, key_end, HighlightClass::Property);
    out.push(sep, sep + 1, HighlightClass::Operator);
    let v = skip_ws(chars, sep + 1);
    let mut end = chars.len();
    while end > v && chars[end - 1].is_whitespace() {
        end -= 1;
    }
    if v < end {
        let value: String = chars[v..end].iter().collect();
        let quoted = matches!(chars[v], '"' | '\'');
        out.push(v, end, if quoted { HighlightClass::String } else { literal_class(&value) });
    }
}

// ---- Dockerfile ----

const DOCKER_INSTRUCTIONS: &[&str] = &[
    "FROM", "RUN", "CMD", "LABEL", "MAINTAINER", "EXPOSE", "ENV", "ADD", "COPY", "ENTRYPOINT", "VOLUME", "USER", "WORKDIR", "ARG", "ONBUILD",
    "STOPSIGNAL", "HEALTHCHECK", "SHELL",
];

fn dockerfile_line(chars: &[char], out: &mut Out) {
    let start = skip_ws(chars, 0);
    if start >= chars.len() {
        return;
    }
    if chars[start] == '#' {
        out.push(start, chars.len(), HighlightClass::Comment);
        return;
    }
    let word_end = (start..chars.len()).find(|i| !is_ident(chars[*i])).unwrap_or(chars.len());
    let word: String = chars[start..word_end].iter().collect();
    let mut i = start;
    if DOCKER_INSTRUCTIONS.contains(&word.to_ascii_uppercase().as_str()) {
        out.push(start, word_end, HighlightClass::Keyword);
        i = word_end;
    }
    while i < chars.len() {
        let c = chars[i];
        if c == '"' || c == '\'' {
            let mut j = i + 1;
            while j < chars.len() && chars[j] != c {
                j += if chars[j] == '\\' { 2 } else { 1 };
            }
            let end = (j + 1).min(chars.len());
            string_with_vars(chars, i, end, HighlightClass::String, out);
            i = end;
        } else if c == '$' {
            let refs = variable_refs(chars, i, chars.len());
            match refs.first() {
                Some(&(from, to)) if from == i => {
                    out.push(from, to, HighlightClass::Escape);
                    i = to;
                }
                _ => i += 1,
            }
        } else if c == '-' && chars.get(i + 1) == Some(&'-') && (i == 0 || chars[i - 1].is_whitespace()) {
            let end = (i..chars.len()).find(|j| chars[*j] == '=' || chars[*j].is_whitespace()).unwrap_or(chars.len());
            out.push(i, end, HighlightClass::Attribute);
            i = end;
        } else if c.is_alphabetic() && i > start {
            let end = (i..chars.len()).find(|j| !is_ident(chars[*j])).unwrap_or(chars.len());
            let token: String = chars[i..end].iter().collect();
            if token.eq_ignore_ascii_case("as") && chars[i - 1].is_whitespace() {
                out.push(i, end, HighlightClass::Keyword);
            }
            i = end;
        } else if c.is_ascii_digit() {
            let end = (i..chars.len()).find(|j| !chars[*j].is_ascii_digit()).unwrap_or(chars.len());
            if i == 0 || !is_ident(chars[i - 1]) {
                out.push(i, end, HighlightClass::Number);
            }
            i = end;
        } else {
            i += 1;
        }
    }
}

// ---- Makefile ----

const MAKE_DIRECTIVES: &[&str] =
    &["include", "-include", "sinclude", "ifeq", "ifneq", "ifdef", "ifndef", "else", "endif", "define", "endef", "export", "unexport", "override", "vpath"];

fn makefile_line(chars: &[char], out: &mut Out) {
    if chars.is_empty() {
        return;
    }
    let first = skip_ws(chars, 0);
    if first >= chars.len() {
        return;
    }
    if chars[first] == '#' {
        out.push(first, chars.len(), HighlightClass::Comment);
        return;
    }
    let is_recipe = chars[0] == '\t';
    let mut i = 0;
    if !is_recipe {
        let word_end = (first..chars.len()).find(|j| chars[*j].is_whitespace() || *j >= chars.len()).unwrap_or(chars.len());
        let word: String = chars[first..word_end].iter().collect();
        if MAKE_DIRECTIVES.contains(&word.as_str()) {
            out.push(first, word_end, HighlightClass::Keyword);
            i = word_end;
        } else if let Some(colon) = (first..chars.len()).find(|j| chars[*j] == ':') {
            let before: String = chars[first..colon].iter().collect();
            let is_assign = chars.get(colon + 1) == Some(&'=');
            let is_double_colon = chars.get(colon + 1) == Some(&':') && chars.get(colon + 2) != Some(&'=');
            if is_assign || (!before.contains('=') && !before.contains('$') || is_double_colon) {
                let class = if is_assign { HighlightClass::Property } else { HighlightClass::Function };
                let name = if before.starts_with('.') && !is_assign { HighlightClass::Keyword } else { class };
                let mut name_end = colon;
                while name_end > first && chars[name_end - 1].is_whitespace() {
                    name_end -= 1;
                }
                out.push(first, name_end, name);
                // `:=` and `::` are two characters wide, a plain `:` one.
                let op_end = if is_assign || is_double_colon { colon + 2 } else { colon + 1 };
                out.push(colon, op_end, HighlightClass::Operator);
                i = op_end;
            }
        } else if let Some(eq) = (first..chars.len()).find(|j| chars[*j] == '=') {
            // `NAME = value`, `NAME ?= value`, `NAME += value`.
            let mut name_end = eq;
            while name_end > first && matches!(chars[name_end - 1], '?' | '+' | '!' | ':' | ' ' | '\t') {
                name_end -= 1;
            }
            out.push(first, name_end, HighlightClass::Property);
            out.push(name_end, eq + 1, HighlightClass::Operator);
            i = eq + 1;
        }
    }
    // Variables and automatic variables anywhere after the head of the line.
    while i < chars.len() {
        if chars[i] == '$' && i + 1 < chars.len() {
            let end = match chars[i + 1] {
                '(' | '{' => {
                    let close = if chars[i + 1] == '(' { ')' } else { '}' };
                    (i + 2..chars.len()).find(|j| chars[*j] == close).map_or(chars.len(), |j| j + 1)
                }
                _ => i + 2,
            };
            out.push(i, end, HighlightClass::Escape);
            i = end;
        } else if chars[i] == '#' && !is_recipe {
            out.push(i, chars.len(), HighlightClass::Comment);
            break;
        } else {
            i += 1;
        }
    }
}

// ---- unified diff and patches ----

fn diff_line(chars: &[char], out: &mut Out) {
    let text: String = chars.iter().take(12).collect();
    const HEADERS: [&str; 13] =
        ["+++", "---", "diff ", "index ", "new file", "deleted file", "similarity", "rename ", "commit ", "Author:", "Date:", "From ", "Subject:"];
    let class = if HEADERS.iter().any(|h| text.starts_with(h)) {
        HighlightClass::Comment
    } else {
        match chars.first() {
            Some('+') => HighlightClass::String,
            Some('-') => HighlightClass::Tag,
            Some('@') => HighlightClass::Constant,
            _ => return,
        }
    };
    out.push(0, chars.len(), class);
}

// ---- .gitignore and friends ----

fn ignore_line(chars: &[char], out: &mut Out) {
    let start = skip_ws(chars, 0);
    if start >= chars.len() {
        return;
    }
    if chars[start] == '#' {
        out.push(start, chars.len(), HighlightClass::Comment);
        return;
    }
    let mut i = start;
    if chars[i] == '!' {
        out.push(i, i + 1, HighlightClass::Operator);
        i += 1;
    }
    let mut run = i;
    let flush = |out: &mut Out, from: usize, to: usize| out.push(from, to, HighlightClass::String);
    while i < chars.len() {
        match chars[i] {
            '*' | '?' => {
                flush(out, run, i);
                out.push(i, i + 1, HighlightClass::Operator);
                run = i + 1;
            }
            '[' => {
                flush(out, run, i);
                let end = (i..chars.len()).find(|j| chars[*j] == ']').map_or(chars.len(), |j| j + 1);
                out.push(i, end, HighlightClass::Operator);
                i = end - 1;
                run = end;
            }
            '/' => {
                flush(out, run, i);
                out.push(i, i + 1, HighlightClass::Punctuation);
                run = i + 1;
            }
            _ => {}
        }
        i += 1;
    }
    flush(out, run, chars.len());
}

#[cfg(test)]
mod tests {
    use super::*;
    use HighlightClass as C;

    fn hl(language: Language, line: &str) -> Vec<(String, HighlightClass)> {
        let chars: Vec<char> = line.chars().collect();
        highlight_line(language, line).into_iter().map(|s| (chars[s.start_col..s.end_col].iter().collect(), s.class)).collect()
    }

    fn get(spans: &[(String, HighlightClass)], text: &str) -> Option<HighlightClass> {
        spans.iter().find(|(t, _)| t == text).map(|(_, c)| *c)
    }

    #[test]
    fn env_key_value_comment_export_and_quotes() {
        let s = hl(Language::Env, "DATABASE_URL=postgres://u:p@localhost/db");
        assert_eq!(get(&s, "DATABASE_URL"), Some(C::Property));
        assert_eq!(get(&s, "="), Some(C::Operator));
        assert_eq!(get(&s, "postgres://u:p@localhost/db"), Some(C::String));

        assert_eq!(hl(Language::Env, "# a comment"), vec![("# a comment".to_owned(), C::Comment)]);
        let s = hl(Language::Env, "export PORT=8080 # default");
        assert_eq!((get(&s, "export"), get(&s, "PORT"), get(&s, "8080"), get(&s, "# default")), (Some(C::Keyword), Some(C::Property), Some(C::Number), Some(C::Comment)));

        let s = hl(Language::Env, r#"GREETING="hello ${NAME}!" # hi"#);
        assert_eq!(get(&s, "${NAME}"), Some(C::Escape), "interpolation inside double quotes");
        assert_eq!(get(&s, "\"hello "), Some(C::String));
        assert_eq!(get(&s, "# hi"), Some(C::Comment));

        let s = hl(Language::Env, "RAW='no ${interp} here'");
        assert_eq!(s.iter().filter(|(_, c)| *c == C::Escape).count(), 0, "single quotes do not interpolate");
        assert_eq!(get(&hl(Language::Env, "DEBUG=true"), "true"), Some(C::Constant));
        assert_eq!(get(&hl(Language::Env, "A = b"), "A"), Some(C::Property), "spaces around the equals sign");
    }

    #[test]
    fn env_edge_cases_never_panic_or_overlap() {
        for line in ["", "   ", "=", "KEY=", "KEY=\"unterminated", "export", "export X", "A=b#c", "é=😀", "$=$", "K=${", "'"] {
            let spans = highlight_line(Language::Env, line);
            for pair in spans.windows(2) {
                assert!(pair[0].end_col <= pair[1].start_col, "{line:?}: {spans:?}");
            }
            assert!(spans.iter().all(|s| s.end_col <= line.chars().count()), "{line:?}");
        }
        // A `#` without a preceding space is part of the value.
        assert_eq!(get(&hl(Language::Env, "A=b#c"), "b#c"), Some(C::String));
    }

    #[test]
    fn ini_sections_keys_values_and_comments() {
        assert_eq!(get(&hl(Language::Ini, "[database]"), "[database]"), Some(C::Type));
        let s = hl(Language::Ini, "port = 5432");
        assert_eq!((get(&s, "port"), get(&s, "="), get(&s, "5432")), (Some(C::Property), Some(C::Operator), Some(C::Number)));
        assert_eq!(get(&hl(Language::Ini, "; note"), "; note"), Some(C::Comment));
        assert_eq!(get(&hl(Language::Ini, "name: \"x y\""), "\"x y\""), Some(C::String));
        assert_eq!(get(&hl(Language::Ini, "key.with.dots=on"), "on"), Some(C::Constant));
    }

    #[test]
    fn dockerfile_instructions_flags_variables_and_strings() {
        let s = hl(Language::Dockerfile, "FROM node:20 AS build");
        assert_eq!((get(&s, "FROM"), get(&s, "AS")), (Some(C::Keyword), Some(C::Keyword)));
        let s = hl(Language::Dockerfile, "copy --from=build /app $HOME/app \"x\"");
        assert_eq!((get(&s, "copy"), get(&s, "--from"), get(&s, "$HOME"), get(&s, "\"x\"")), (Some(C::Keyword), Some(C::Attribute), Some(C::Escape), Some(C::String)));
        assert_eq!(get(&hl(Language::Dockerfile, "# comment"), "# comment"), Some(C::Comment));
        assert_eq!(get(&hl(Language::Dockerfile, "EXPOSE 8080"), "8080"), Some(C::Number));
        // Not an instruction: no keyword.
        assert!(get(&hl(Language::Dockerfile, "  && apt-get update"), "apt").is_none());
    }

    #[test]
    fn makefile_targets_assignments_directives_and_recipes() {
        let s = hl(Language::Makefile, "build: main.o util.o");
        assert_eq!((get(&s, "build"), get(&s, ":")), (Some(C::Function), Some(C::Operator)));
        let s = hl(Language::Makefile, "CC := gcc");
        assert_eq!((get(&s, "CC"), get(&s, ":=")), (Some(C::Property), Some(C::Operator)));
        assert_eq!(get(&hl(Language::Makefile, "CFLAGS += -O2"), "CFLAGS"), Some(C::Property));
        assert_eq!(get(&hl(Language::Makefile, ".PHONY: all"), ".PHONY"), Some(C::Keyword));
        assert_eq!(get(&hl(Language::Makefile, "include config.mk"), "include"), Some(C::Keyword));
        let s = hl(Language::Makefile, "\t$(CC) -o $@ $^");
        assert_eq!((get(&s, "$(CC)"), get(&s, "$@"), get(&s, "$^")), (Some(C::Escape), Some(C::Escape), Some(C::Escape)));
        assert_eq!(get(&hl(Language::Makefile, "# note"), "# note"), Some(C::Comment));
    }

    #[test]
    fn diff_lines_by_prefix() {
        assert_eq!(hl(Language::Diff, "+added line"), vec![("+added line".to_owned(), C::String)]);
        assert_eq!(hl(Language::Diff, "-removed line"), vec![("-removed line".to_owned(), C::Tag)]);
        assert_eq!(hl(Language::Diff, "@@ -1,2 +1,3 @@")[0].1, C::Constant);
        assert_eq!(hl(Language::Diff, "+++ b/file.rs")[0].1, C::Comment, "file headers are not additions");
        assert_eq!(hl(Language::Diff, "--- a/file.rs")[0].1, C::Comment);
        assert!(hl(Language::Diff, " context").is_empty());
        assert!(hl(Language::Diff, "").is_empty());
    }

    #[test]
    fn ignore_patterns() {
        let s = hl(Language::Ignore, "!src/*.rs");
        assert_eq!((get(&s, "!"), get(&s, "*")), (Some(C::Operator), Some(C::Operator)));
        assert_eq!(get(&s, "/"), Some(C::Punctuation));
        assert_eq!(get(&hl(Language::Ignore, "# build output"), "# build output"), Some(C::Comment));
        assert_eq!(get(&hl(Language::Ignore, "target/"), "target"), Some(C::String));
        assert_eq!(get(&hl(Language::Ignore, "*.log"), "*"), Some(C::Operator));
    }
}
