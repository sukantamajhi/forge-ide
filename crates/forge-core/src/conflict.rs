//! Merge conflicts in a file: find the `<<<<<<<` / `=======` / `>>>>>>>` blocks git leaves
//! behind and replace one with either side (or both).

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Conflict {
    /// Line numbers (0-based) of the opening and closing markers.
    pub start: usize,
    pub end: usize,
    /// What follows the markers: usually the branch or commit names.
    pub current_label: String,
    pub incoming_label: String,
    /// The lines of each side, line endings included.
    pub current: Vec<String>,
    pub incoming: Vec<String>,
    /// The common ancestor, when git was set to write it (`merge.conflictStyle = diff3`).
    pub base: Option<Vec<String>>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Choice {
    Current,
    Incoming,
    /// Current first, then incoming.
    Both,
}

fn marker<'a>(line: &'a str, symbol: char) -> Option<&'a str> {
    let line = line.trim_end_matches(['\r', '\n']);
    let rest = line.strip_prefix(&symbol.to_string().repeat(7))?;
    // "<<<<<<<" alone or followed by a space; "========" is not a marker.
    if rest.is_empty() {
        Some("")
    } else {
        rest.strip_prefix(' ')
    }
}

fn is_separator(line: &str) -> bool {
    line.trim_end_matches(['\r', '\n']) == "======="
}

pub fn find(text: &str) -> Vec<Conflict> {
    let lines: Vec<&str> = text.split_inclusive('\n').collect();
    let mut found = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        let Some(current_label) = marker(lines[i], '<') else {
            i += 1;
            continue;
        };
        let mut current = Vec::new();
        let mut base: Option<Vec<String>> = None;
        let mut incoming = Vec::new();
        let mut state = 0; // 0 current, 1 base, 2 incoming
        let mut j = i + 1;
        let mut closed = None;
        while j < lines.len() {
            let line = lines[j];
            if state == 0 && marker(line, '|').is_some() {
                state = 1;
                base = Some(Vec::new());
            } else if state < 2 && is_separator(line) {
                state = 2;
            } else if state == 2 {
                if let Some(label) = marker(line, '>') {
                    closed = Some((j, label.trim_end().to_owned()));
                    break;
                }
                incoming.push(line.to_owned());
            } else if state == 1 {
                base.as_mut().unwrap().push(line.to_owned());
            } else {
                current.push(line.to_owned());
            }
            j += 1;
        }
        match closed {
            Some((end, incoming_label)) => {
                found.push(Conflict { start: i, end, current_label: current_label.trim_end().to_owned(), incoming_label, current, incoming, base });
                i = end + 1;
            }
            // An unfinished block is not a conflict; look again after its opening line.
            None => i += 1,
        }
    }
    found
}

/// `text` with conflict number `index` replaced by the chosen side. `None` if there is no such conflict.
pub fn resolve(text: &str, index: usize, choice: Choice) -> Option<String> {
    let conflict = find(text).into_iter().nth(index)?;
    let lines: Vec<&str> = text.split_inclusive('\n').collect();
    let mut out = String::with_capacity(text.len());
    lines[..conflict.start].iter().for_each(|l| out.push_str(l));
    match choice {
        Choice::Current => conflict.current.iter().for_each(|l| out.push_str(l)),
        Choice::Incoming => conflict.incoming.iter().for_each(|l| out.push_str(l)),
        Choice::Both => {
            conflict.current.iter().for_each(|l| out.push_str(l));
            // The first side may lack a final newline only at the end of a file; keep the two lines apart.
            if conflict.current.last().is_some_and(|l| !l.ends_with('\n')) {
                out.push('\n');
            }
            conflict.incoming.iter().for_each(|l| out.push_str(l));
        }
    }
    lines[conflict.end + 1..].iter().for_each(|l| out.push_str(l));
    Some(out)
}

/// Take `choice` for every conflict.
pub fn resolve_all(text: &str, choice: Choice) -> String {
    let mut text = text.to_owned();
    while let Some(next) = resolve(&text, 0, choice) {
        text = next;
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEXT: &str = "top\n<<<<<<< HEAD\nours 1\nours 2\n=======\ntheirs\n>>>>>>> feature\nmiddle\n<<<<<<< HEAD\na\n||||||| base\nb\n=======\nc\n>>>>>>> feature\nbottom\n";

    #[test]
    fn finds_every_block_with_labels_sides_and_the_optional_base() {
        let found = find(TEXT);
        assert_eq!(found.len(), 2);
        assert_eq!((found[0].start, found[0].end), (1, 6));
        assert_eq!((found[0].current_label.as_str(), found[0].incoming_label.as_str()), ("HEAD", "feature"));
        assert_eq!(found[0].current, ["ours 1\n", "ours 2\n"]);
        assert_eq!(found[0].incoming, ["theirs\n"]);
        assert_eq!(found[0].base, None);
        assert_eq!(found[1].base.as_deref(), Some(&["b\n".to_owned()][..]));
    }

    #[test]
    fn resolving_replaces_one_block_and_leaves_the_rest() {
        assert_eq!(resolve(TEXT, 0, Choice::Current).unwrap().lines().take(4).collect::<Vec<_>>(), ["top", "ours 1", "ours 2", "middle"]);
        let incoming = resolve(TEXT, 0, Choice::Incoming).unwrap();
        assert!(incoming.starts_with("top\ntheirs\nmiddle\n<<<<<<< HEAD"), "{incoming}");
        let both = resolve(TEXT, 0, Choice::Both).unwrap();
        assert!(both.starts_with("top\nours 1\nours 2\ntheirs\nmiddle\n"));
        assert_eq!(resolve(TEXT, 5, Choice::Current), None);
        assert_eq!(resolve_all(TEXT, Choice::Incoming), "top\ntheirs\nmiddle\nc\nbottom\n");
        assert!(find(&resolve_all(TEXT, Choice::Both)).is_empty());
    }

    #[test]
    fn crlf_files_keep_their_line_endings_and_lookalikes_are_ignored() {
        let text = "a\r\n<<<<<<< HEAD\r\nx\r\n=======\r\ny\r\n>>>>>>> b\r\nz\r\n";
        assert_eq!(resolve(text, 0, Choice::Incoming).unwrap(), "a\r\ny\r\nz\r\n");
        let lookalike = "========\n<<<<<<<<\n>>>>>>> no opening\n=======\n";
        assert!(find(lookalike).is_empty());
        assert!(find("<<<<<<< HEAD\nunfinished\n").is_empty());
    }
}
