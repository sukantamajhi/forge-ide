//! Markup tags (HTML, XML, JSX): finding them in text, the closing tag to type, and the partner of a tag.
//!
//! All offsets are char offsets. The scanner is forgiving, not a parser: it skips comments, strings and
//! `{...}` expressions inside a tag, and it never fails on half-typed input.

const VOID: [&str; 14] = ["area", "base", "br", "col", "embed", "hr", "img", "input", "link", "meta", "param", "source", "track", "wbr"];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Open,
    Close,
    /// `<br>` or `<Thing />`: nothing to close.
    Empty,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Tag {
    /// The `<` and one past the `>`.
    pub start: usize,
    pub end: usize,
    /// The name inside it (empty for a JSX fragment `<>`).
    pub name_start: usize,
    pub name_end: usize,
    pub name: String,
    pub kind: Kind,
}

fn is_name_char(c: char) -> bool {
    c.is_alphanumeric() || matches!(c, '-' | '_' | ':' | '.')
}

pub fn is_void(name: &str) -> bool {
    VOID.contains(&name.to_ascii_lowercase().as_str())
}

/// Every tag in `chars`, in order. `jsx` also treats `<>` and `</>` as tags.
pub fn scan(chars: &[char], jsx: bool) -> Vec<Tag> {
    let mut tags = Vec::new();
    let n = chars.len();
    let mut i = 0;
    while i < n {
        if chars[i] != '<' {
            i += 1;
            continue;
        }
        // Comments, doctypes, processing instructions and CDATA are not tags.
        if chars[i..].starts_with(&['<', '!', '-', '-']) {
            i = find(chars, i + 4, &['-', '-', '>']).map_or(n, |p| p + 3);
            continue;
        }
        if matches!(chars.get(i + 1), Some('!' | '?')) {
            i = find(chars, i + 2, &['>']).map_or(n, |p| p + 1);
            continue;
        }
        let mut j = i + 1;
        let closing = chars.get(j) == Some(&'/');
        if closing {
            j += 1;
        }
        let name_start = j;
        // A name starts with a letter; `<` followed by anything else (a comparison, a generic) is text.
        if !chars.get(j).is_some_and(|c| c.is_alphabetic()) {
            if jsx && chars.get(j) == Some(&'>') {
                tags.push(Tag { start: i, end: j + 1, name_start, name_end: name_start, name: String::new(), kind: if closing { Kind::Close } else { Kind::Open } });
                i = j + 1;
            } else {
                i += 1;
            }
            continue;
        }
        while chars.get(j).copied().is_some_and(is_name_char) {
            j += 1;
        }
        let name_end = j;
        // Attributes up to the `>`, skipping strings and `{...}`.
        let mut depth = 0usize;
        let mut quote: Option<char> = None;
        let mut self_closing = false;
        let mut end = None;
        while j < n {
            let c = chars[j];
            match quote {
                Some(q) => {
                    if c == q {
                        quote = None;
                    }
                }
                None => match c {
                    '"' | '\'' | '`' if depth == 0 || c == '`' => quote = Some(c),
                    '"' | '\'' => quote = Some(c),
                    '{' => depth += 1,
                    '}' => depth = depth.saturating_sub(1),
                    '<' if depth == 0 => break,
                    '>' if depth == 0 => {
                        self_closing = chars.get(j.wrapping_sub(1)) == Some(&'/') && j > name_end;
                        end = Some(j + 1);
                        break;
                    }
                    _ => {}
                },
            }
            j += 1;
        }
        let Some(end) = end else {
            // An unfinished tag (still being typed): not a tag yet.
            i = name_end;
            continue;
        };
        let name: String = chars[name_start..name_end].iter().collect();
        let kind = if closing {
            Kind::Close
        } else if self_closing || is_void(&name) {
            Kind::Empty
        } else {
            Kind::Open
        };
        tags.push(Tag { start: i, end, name_start, name_end, name, kind });
        i = end;
    }
    tags
}

fn find(chars: &[char], from: usize, pattern: &[char]) -> Option<usize> {
    (from..chars.len().saturating_sub(pattern.len() - 1)).find(|&p| chars[p..].starts_with(pattern))
}

/// The tag to type after `>` was just pressed: `before` ends with the `>` of an opening tag.
/// `None` when it is not an opening tag that needs one (void, self-closing, a generic like `Vec<T>`).
pub fn closing_for_open(before: &[char], jsx: bool) -> Option<String> {
    let tags = scan(&before[before.len().saturating_sub(4000)..], jsx);
    let offset = before.len().saturating_sub(4000);
    let tag = tags.last().filter(|t| t.end + offset == before.len() && t.kind == Kind::Open)?;
    // In JSX `Array<string>` or `useState<number>` are generics: a real tag is not glued to a word.
    let prev = (tag.start + offset).checked_sub(1).and_then(|p| before.get(p));
    if prev.is_some_and(|c| c.is_alphanumeric() || *c == '_') {
        return None;
    }
    Some(format!("</{}>", tag.name))
}

/// What `</` should be completed with: the name of the innermost tag still open before the caret.
pub fn innermost_open(before: &[char], jsx: bool) -> Option<String> {
    let tags = scan(&before[before.len().saturating_sub(200_000)..], jsx);
    let mut stack: Vec<&Tag> = Vec::new();
    for tag in &tags {
        match tag.kind {
            Kind::Open => stack.push(tag),
            Kind::Close => {
                if let Some(pos) = stack.iter().rposition(|open| open.name == tag.name) {
                    stack.truncate(pos);
                }
            }
            Kind::Empty => {}
        }
    }
    stack.last().map(|t| t.name.clone())
}

/// The index in `tags` of the tag that pairs with `tags[i]`: the closing tag for an opening one and the
/// other way round, found by nesting depth so it still works while one of the two names is being edited.
pub fn partner(tags: &[Tag], i: usize) -> Option<usize> {
    match tags.get(i)?.kind {
        Kind::Empty => None,
        Kind::Open => {
            let mut depth = 0;
            for (j, tag) in tags.iter().enumerate().skip(i + 1) {
                match tag.kind {
                    Kind::Open => depth += 1,
                    Kind::Close if depth == 0 => return Some(j),
                    Kind::Close => depth -= 1,
                    Kind::Empty => {}
                }
            }
            None
        }
        Kind::Close => {
            let mut depth = 0;
            for j in (0..i).rev() {
                match tags[j].kind {
                    Kind::Close => depth += 1,
                    Kind::Open if depth == 0 => return Some(j),
                    Kind::Open => depth -= 1,
                    Kind::Empty => {}
                }
            }
            None
        }
    }
}

/// When the caret is in a tag's name and the partner's name differs, the edit that makes it match:
/// `(start, end, new name)` over the partner's name.
pub fn rename_partner(chars: &[char], caret: usize, jsx: bool) -> Option<(usize, usize, String)> {
    let tags = scan(chars, jsx);
    let i = tags.iter().position(|t| caret >= t.name_start && caret <= t.name_end && t.kind != Kind::Empty && !(t.name.is_empty() && t.name_start != caret))?;
    let j = partner(&tags, i)?;
    let (here, other) = (&tags[i], &tags[j]);
    (here.name != other.name).then(|| (other.name_start, other.name_end, here.name.clone()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn c(s: &str) -> Vec<char> {
        s.chars().collect()
    }

    #[test]
    fn scan_finds_open_close_and_empty_tags() {
        let tags = scan(&c("<div class=\"a>b\"><br><img src='x'/></div><!-- <p> -->"), false);
        let shape: Vec<(&str, Kind)> = tags.iter().map(|t| (t.name.as_str(), t.kind)).collect();
        assert_eq!(shape, [("div", Kind::Open), ("br", Kind::Empty), ("img", Kind::Empty), ("div", Kind::Close)]);
    }

    #[test]
    fn jsx_expressions_and_fragments() {
        let tags = scan(&c("<><A on={() => go(1 > 0)} b=\"x\">t</A></>"), true);
        let shape: Vec<(&str, Kind)> = tags.iter().map(|t| (t.name.as_str(), t.kind)).collect();
        assert_eq!(shape, [("", Kind::Open), ("A", Kind::Open), ("A", Kind::Close), ("", Kind::Close)]);
        assert!(scan(&c("a < b && c > d"), false).is_empty(), "comparisons are not tags");
    }

    #[test]
    fn typing_gt_closes_real_tags_only() {
        assert_eq!(closing_for_open(&c("<div>"), false).as_deref(), Some("</div>"));
        assert_eq!(closing_for_open(&c("<p class=\"x\">"), false).as_deref(), Some("</p>"));
        assert_eq!(closing_for_open(&c("<br>"), false), None, "void elements");
        assert_eq!(closing_for_open(&c("<Thing />"), true), None, "self-closing");
        assert_eq!(closing_for_open(&c("</div>"), false), None);
        assert_eq!(closing_for_open(&c("const x: Array<string>"), true), None, "a generic");
        assert_eq!(closing_for_open(&c("return (<Foo.Bar>"), true).as_deref(), Some("</Foo.Bar>"));
        assert_eq!(closing_for_open(&c("<>"), true).as_deref(), Some("</>"));
    }

    #[test]
    fn slash_completes_the_innermost_open_tag() {
        assert_eq!(innermost_open(&c("<ul><li>a</li><li>b"), false).as_deref(), Some("li"));
        assert_eq!(innermost_open(&c("<ul><li>a</li>"), false).as_deref(), Some("ul"));
        assert_eq!(innermost_open(&c("<p>a<br>b"), false).as_deref(), Some("p"));
        assert_eq!(innermost_open(&c("<p>a</p>"), false), None);
    }

    #[test]
    fn editing_one_name_renames_the_other_by_nesting() {
        let text = c("<div><span>x</span></divx>");
        // The caret is at the end of the closing name "divx": the opener should become "divx".
        let caret = text.len() - 1;
        assert_eq!(rename_partner(&text, caret, false), Some((1, 4, "divx".to_owned())));
        // Editing the opener renames the closer.
        let text = c("<divy><span>x</span></div>");
        assert_eq!(rename_partner(&text, 5, false), Some((22, 25, "divy".to_owned())));
        // Names that already match, or a caret outside any name, do nothing.
        assert_eq!(rename_partner(&c("<a>x</a>"), 2, false), None);
        assert_eq!(rename_partner(&c("<a>x</a>"), 4, false), None);
        // Void elements have no partner.
        assert_eq!(rename_partner(&c("<br>"), 3, false), None);
    }
}
