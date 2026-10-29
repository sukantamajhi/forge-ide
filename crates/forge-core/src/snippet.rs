//! VS Code snippets: the file format and the body syntax.
//!
//! A snippet body is text with tab stops (`$1`, `${2:default}`, nested `${1:a ${2:b}}`),
//! mirrors (the same number twice edits both at once), choices (`${1|a,b|}`, the first is
//! used) and variables (`$TM_FILENAME`, `${CURRENT_YEAR}`). [`expand`] turns a body into
//! plain text plus the ranges to visit, in the order Tab visits them.

use std::collections::BTreeMap;

use serde_json::Value;

use crate::jsonc;

/// One entry of a snippet file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Snippet {
    pub name: String,
    pub prefixes: Vec<String>,
    pub body: String,
    pub description: String,
}

/// Read a snippets file (`{ "Name": { "prefix": ..., "body": ..., "description": ... } }`).
/// Entries without a prefix or body are skipped.
pub fn parse_snippet_file(text: &str) -> Result<Vec<Snippet>, String> {
    let Value::Object(map) = jsonc::parse(text)? else { return Err("a snippets file is a JSON object".to_owned()) };
    let mut snippets = Vec::new();
    for (name, entry) in map {
        let strings = |v: &Value| match v {
            Value::String(s) => vec![s.clone()],
            Value::Array(a) => a.iter().filter_map(Value::as_str).map(str::to_owned).collect(),
            _ => Vec::new(),
        };
        let prefixes: Vec<String> = entry.get("prefix").map(strings).unwrap_or_default().into_iter().filter(|p| !p.is_empty()).collect();
        let body = match entry.get("body") {
            Some(Value::String(s)) => s.clone(),
            Some(Value::Array(lines)) => lines.iter().filter_map(Value::as_str).collect::<Vec<_>>().join("\n"),
            _ => continue,
        };
        if prefixes.is_empty() {
            continue;
        }
        let description = entry.get("description").map(|d| strings(d).join(" ")).unwrap_or_default();
        snippets.push(Snippet { name, prefixes, body, description });
    }
    Ok(snippets)
}

/// What variables like `$TM_FILENAME` expand to, and how continuation lines are indented.
#[derive(Clone, Debug, Default)]
pub struct Context {
    pub file_path: String,
    pub workspace_name: String,
    pub selected_text: String,
    pub current_line: String,
    pub line_comment: Option<String>,
    pub block_comment: Option<(String, String)>,
    /// Added after every newline in the body, so a snippet keeps the indentation it is typed at.
    pub indent: String,
    /// `(year, month, day)` for the date variables.
    pub date: (i64, u32, u32),
}

/// A snippet ready to insert.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Expanded {
    pub text: String,
    /// Char ranges within `text`, in the order Tab visits them: stop 1, 2, ..., then `$0`
    /// (or the end of the text when the snippet has none). Mirrors share one entry.
    pub stops: Vec<Vec<(usize, usize)>>,
}

struct Parser<'a> {
    chars: Vec<char>,
    pos: usize,
    out: String,
    /// Length of `out` in chars.
    len: usize,
    stops: BTreeMap<u32, Vec<(usize, usize)>>,
    /// Text of the first definition of each stop, so a mirror written before it matches.
    defaults: BTreeMap<u32, String>,
    context: &'a Context,
}

pub fn expand(body: &str, context: &Context) -> Expanded {
    // The first pass learns each stop's default text; the second uses it for earlier mirrors.
    let mut first = Parser::new(body, context, BTreeMap::new());
    first.sequence(false);
    let mut second = Parser::new(body, context, first.defaults);
    second.sequence(false);

    let mut stops: Vec<Vec<(usize, usize)>> = Vec::new();
    let mut final_stop = None;
    for (index, ranges) in second.stops {
        if index == 0 {
            final_stop = Some(ranges);
        } else {
            stops.push(ranges);
        }
    }
    stops.push(final_stop.unwrap_or_else(|| vec![(second.len, second.len)]));
    Expanded { text: second.out, stops }
}

impl<'a> Parser<'a> {
    fn new(body: &str, context: &'a Context, defaults: BTreeMap<u32, String>) -> Self {
        Self { chars: body.chars().collect(), pos: 0, out: String::new(), len: 0, stops: BTreeMap::new(), defaults, context }
    }

    fn peek(&self) -> Option<char> {
        self.chars.get(self.pos).copied()
    }

    fn push(&mut self, c: char) {
        self.out.push(c);
        self.len += 1;
        if c == '\n' {
            for i in self.context.indent.chars() {
                self.out.push(i);
                self.len += 1;
            }
        }
    }

    fn push_str(&mut self, text: &str) {
        for c in text.chars() {
            self.push(c);
        }
    }

    /// Text up to the `}` that closes the enclosing `${...}` (or the end). Stops before the `}`.
    fn sequence(&mut self, inside_braces: bool) {
        while let Some(c) = self.peek() {
            match c {
                '}' if inside_braces => return,
                '\\' => {
                    self.pos += 1;
                    match self.peek() {
                        Some(next @ ('$' | '}' | '\\')) => {
                            self.push(next);
                            self.pos += 1;
                        }
                        _ => self.push('\\'),
                    }
                }
                '$' => {
                    self.pos += 1;
                    self.dollar();
                }
                _ => {
                    self.push(c);
                    self.pos += 1;
                }
            }
        }
    }

    fn number(&mut self) -> Option<u32> {
        let start = self.pos;
        while self.peek().is_some_and(|c| c.is_ascii_digit()) {
            self.pos += 1;
        }
        (self.pos > start).then(|| self.chars[start..self.pos].iter().collect::<String>().parse().ok()).flatten()
    }

    fn name(&mut self) -> Option<String> {
        let start = self.pos;
        while self.peek().is_some_and(|c| c.is_ascii_alphanumeric() || c == '_') {
            self.pos += 1;
        }
        (self.pos > start && !self.chars[start].is_ascii_digit()).then(|| self.chars[start..self.pos].iter().collect())
    }

    fn dollar(&mut self) {
        // `$` was consumed. Anything that is not a stop or a variable is a literal dollar.
        if let Some(index) = self.number() {
            return self.stop(index, None);
        }
        if let Some(name) = self.name() {
            return self.variable(&name, None);
        }
        if self.peek() == Some('{') {
            self.pos += 1;
            if let Some(index) = self.number() {
                return self.braced_stop(index);
            }
            if let Some(name) = self.name() {
                return self.braced_variable(name);
            }
            self.out.push_str("${");
            self.len += 2;
            return;
        }
        self.push('$');
    }

    fn close_brace(&mut self) {
        if self.peek() == Some('}') {
            self.pos += 1;
        }
    }

    /// After `${N`: `}`, `:default}` or `|a,b|}`.
    fn braced_stop(&mut self, index: u32) {
        match self.peek() {
            Some(':') => {
                self.pos += 1;
                self.stop(index, Some(()));
                self.close_brace();
            }
            Some('|') => {
                self.pos += 1;
                let mut choices = vec![String::new()];
                while let Some(c) = self.peek() {
                    self.pos += 1;
                    match c {
                        '\\' if matches!(self.peek(), Some(',' | '|' | '\\')) => {
                            let escaped = self.peek().unwrap_or('\\');
                            self.pos += 1;
                            if let Some(choice) = choices.last_mut() {
                                choice.push(escaped);
                            }
                        }
                        ',' => choices.push(String::new()),
                        '|' => break,
                        _ => {
                            if let Some(choice) = choices.last_mut() {
                                choice.push(c);
                            }
                        }
                    }
                }
                self.close_brace();
                let first = choices.into_iter().next().unwrap_or_default();
                self.place(index, &first);
            }
            _ => {
                self.close_brace();
                self.stop(index, None);
            }
        }
    }

    /// Record stop `index` over its default text. `Some(())` means the default is written
    /// inline (parsed here); `None` means a bare `$N`, which takes the default seen elsewhere.
    fn stop(&mut self, index: u32, inline: Option<()>) {
        match inline {
            Some(()) => {
                let (start_len, start_out) = (self.len, self.out.len());
                self.sequence(true);
                let text = self.out[start_out..].to_owned();
                self.stops.entry(index).or_default().push((start_len, self.len));
                self.defaults.entry(index).or_insert(text);
            }
            None => {
                // A mirror: it takes the text of the stop's definition and defines nothing itself.
                let text = self.defaults.get(&index).cloned().unwrap_or_default();
                let start = self.len;
                self.push_str(&text);
                self.stops.entry(index).or_default().push((start, self.len));
            }
        }
    }

    /// Write a definition's `text` (a choice) and record stop `index` around it.
    fn place(&mut self, index: u32, text: &str) {
        let start = self.len;
        self.push_str(text);
        self.stops.entry(index).or_default().push((start, self.len));
        self.defaults.entry(index).or_insert_with(|| text.to_owned());
    }

    fn braced_variable(&mut self, name: String) {
        if self.peek() == Some(':') {
            self.pos += 1;
            // The default is used when the variable has no value.
            let value = self.value_of(&name);
            let (start_len, start_out) = (self.len, self.out.len());
            self.sequence(true);
            if let Some(value) = value.filter(|v| !v.is_empty()) {
                self.out.truncate(start_out);
                self.len = start_len;
                self.push_str(&value);
            }
            self.close_brace();
        } else {
            self.close_brace();
            self.variable(&name, None);
        }
    }

    fn variable(&mut self, name: &str, _default: Option<&str>) {
        // Unknown variables stay visible as their name, as in VS Code.
        let text = self.value_of(name).unwrap_or_else(|| name.to_owned());
        self.push_str(&text);
    }

    /// The value of a known variable (possibly empty), or `None` if it is not one.
    fn value_of(&self, name: &str) -> Option<String> {
        let c = self.context;
        let path = c.file_path.replace('\\', "/");
        let file = path.rsplit('/').next().unwrap_or("").to_owned();
        let (y, m, d) = c.date;
        Some(match name {
            "TM_FILENAME" => file,
            "TM_FILENAME_BASE" => file.rsplit_once('.').map_or(file.clone(), |(base, _)| base.to_owned()),
            "TM_DIRECTORY" => path.rsplit_once('/').map_or(String::new(), |(dir, _)| dir.to_owned()),
            "TM_FILEPATH" => c.file_path.clone(),
            "TM_SELECTED_TEXT" => c.selected_text.clone(),
            "TM_CURRENT_LINE" => c.current_line.clone(),
            "TM_CURRENT_WORD" | "CLIPBOARD" => String::new(),
            "WORKSPACE_NAME" => c.workspace_name.clone(),
            "LINE_COMMENT" => c.line_comment.clone().unwrap_or_default(),
            "BLOCK_COMMENT_START" => c.block_comment.as_ref().map(|b| b.0.clone()).unwrap_or_default(),
            "BLOCK_COMMENT_END" => c.block_comment.as_ref().map(|b| b.1.clone()).unwrap_or_default(),
            "CURRENT_YEAR" => y.to_string(),
            "CURRENT_YEAR_SHORT" => format!("{:02}", y.rem_euclid(100)),
            "CURRENT_MONTH" => format!("{m:02}"),
            "CURRENT_DATE" => format!("{d:02}"),
            _ => return None,
        })
    }
}

/// `(year, month, day)` of a Unix time in seconds (UTC), for the date variables.
pub fn civil_date(unix_seconds: i64) -> (i64, u32, u32) {
    let days = unix_seconds.div_euclid(86_400) + 719_468;
    let era = days.div_euclid(146_097);
    let day_of_era = days.rem_euclid(146_097);
    let year_of_era = (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let mp = (5 * day_of_year + 2) / 153;
    let day = (day_of_year - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (year_of_era + era * 400 + i64::from(month <= 2), month, day)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plain(body: &str) -> Expanded {
        expand(body, &Context::default())
    }

    fn chars(e: &Expanded, range: (usize, usize)) -> String {
        e.text.chars().skip(range.0).take(range.1 - range.0).collect()
    }

    #[test]
    fn snippet_files_accept_strings_arrays_and_comments() {
        let text = r#"{
            // a comment
            "Log": { "prefix": ["log", "cl"], "body": ["console.log($1);", "$0"], "description": "Print" },
            "One": { "prefix": "one", "body": "single" },
            "NoPrefix": { "body": "x" },
            "NoBody": { "prefix": "nb" },
        }"#;
        let snippets = parse_snippet_file(text).unwrap();
        assert_eq!(snippets.len(), 2);
        assert_eq!(snippets[0].prefixes, vec!["log", "cl"]);
        assert_eq!(snippets[0].body, "console.log($1);\n$0");
        assert_eq!(snippets[0].description, "Print");
        assert_eq!(snippets[1].body, "single");
        assert!(parse_snippet_file("[1]").is_err());
        assert!(parse_snippet_file("{ nope").is_err());
    }

    #[test]
    fn plain_text_gets_one_final_stop_at_its_end() {
        let e = plain("hello");
        assert_eq!(e, Expanded { text: "hello".to_owned(), stops: vec![vec![(5, 5)]] });
    }

    #[test]
    fn tab_stops_are_visited_in_order_with_zero_last() {
        let e = plain("for ($2; $1; $0) {}");
        assert_eq!(e.text, "for (; ; ) {}");
        assert_eq!(e.stops, vec![vec![(7, 7)], vec![(5, 5)], vec![(9, 9)]], "1, then 2, then $0");
    }

    #[test]
    fn placeholders_carry_default_text_and_nest() {
        let e = plain("fn ${1:name}(${2:a: ${3:i32}}) {}");
        assert_eq!(e.text, "fn name(a: i32) {}");
        assert_eq!(chars(&e, e.stops[0][0]), "name");
        assert_eq!(chars(&e, e.stops[1][0]), "a: i32");
        assert_eq!(chars(&e, e.stops[2][0]), "i32");
    }

    #[test]
    fn mirrors_share_a_stop_and_text_even_when_written_before_the_default() {
        let e = plain("$1 and ${1:thing} and ${1}");
        assert_eq!(e.text, "thing and thing and thing");
        assert_eq!(e.stops[0].len(), 3);
        for range in &e.stops[0] {
            assert_eq!(chars(&e, *range), "thing");
        }
    }

    #[test]
    fn choices_use_their_first_option() {
        let e = plain("let ${1|mut,const|} x");
        assert_eq!(e.text, "let mut x");
        assert_eq!(chars(&e, e.stops[0][0]), "mut");
        assert_eq!(plain("${1|a\\,b,c|}").text, "a,b");
    }

    #[test]
    fn escapes_and_stray_dollars_are_literal() {
        assert_eq!(plain("cost: \\$5 and \\} and \\\\ and \\n").text, "cost: $5 and } and \\ and \\n");
        assert_eq!(plain("a $ b $-c").text, "a $ b $-c");
        assert_eq!(plain("${").text, "${");
        // A `}` outside a placeholder is plain text.
        assert_eq!(plain("{ a }").text, "{ a }");
    }

    #[test]
    fn newlines_keep_the_indentation_and_offsets_account_for_it() {
        let context = Context { indent: "  ".to_owned(), ..Default::default() };
        let e = expand("if ($1) {\n\t$0\n}", &context);
        assert_eq!(e.text, "if () {\n  \t\n  }");
        assert_eq!(e.stops[0], vec![(4, 4)]);
        assert_eq!(e.stops[1], vec![(11, 11)], "the final stop is after the added indent and the tab");
    }

    #[test]
    fn variables_expand_with_defaults_and_unknown_ones_show_their_name() {
        let context = Context {
            file_path: "C:\\proj\\src\\main.rs".to_owned(),
            workspace_name: "proj".to_owned(),
            line_comment: Some("//".to_owned()),
            date: (2026, 3, 9),
            ..Default::default()
        };
        let e = expand("$TM_FILENAME|${TM_FILENAME_BASE}|$TM_DIRECTORY|$WORKSPACE_NAME|$LINE_COMMENT|$CURRENT_YEAR-$CURRENT_MONTH-$CURRENT_DATE|$CURRENT_YEAR_SHORT", &context);
        assert_eq!(e.text, "main.rs|main|C:/proj/src|proj|//|2026-03-09|26");
        assert_eq!(expand("${TM_SELECTED_TEXT:fallback}|${WORKSPACE_NAME:none}|$UNKNOWN_VAR", &context).text, "fallback|proj|UNKNOWN_VAR");
    }

    #[test]
    fn civil_dates_are_right() {
        assert_eq!(civil_date(0), (1970, 1, 1));
        assert_eq!(civil_date(951_782_400), (2000, 2, 29), "a leap day");
        assert_eq!(civil_date(1_700_000_000), (2023, 11, 14));
        assert_eq!(civil_date(-1), (1969, 12, 31));
    }

    #[test]
    fn weird_input_never_panics() {
        for body in ["$", "${", "${1", "${1:", "${1:${2:", "${1|", "${1|a", "\\", "$1$2$3", "${a:${b:${c}}}", "}}}", "${1:}", "$99999999999999999999"] {
            let _ = plain(body);
        }
        assert_eq!(plain("${1:}").stops[0], vec![(0, 0)]);
    }
}
