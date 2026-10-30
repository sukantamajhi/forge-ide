//! A unified diff of one file as hunks and lines, for side-by-side display and for staging a
//! hunk or chosen lines: the pieces are written back out as small patches `git apply` takes.

use std::collections::BTreeSet;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LineKind {
    Context,
    Add,
    Del,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiffLine {
    pub kind: LineKind,
    pub text: String,
    /// The file has no newline after this line (`\ No newline at end of file`).
    pub no_newline: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Hunk {
    /// The `@@ -a,b +c,d @@ context` line.
    pub header: String,
    pub old_start: usize,
    pub new_start: usize,
    pub lines: Vec<DiffLine>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Patch {
    /// `diff --git`, `index`, `---` and `+++` lines, kept as git wrote them.
    pub header: Vec<String>,
    pub hunks: Vec<Hunk>,
}

/// One cell of a side-by-side row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Cell {
    /// Line number in that version of the file.
    pub number: usize,
    pub text: String,
    pub kind: LineKind,
    /// Index into the hunk's `lines`, which is what line selection refers to.
    pub line: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SideRow {
    pub hunk: usize,
    pub left: Option<Cell>,
    pub right: Option<Cell>,
}

fn parse_range(spec: &str) -> usize {
    // "-12,3" or "+12": the first number is where the hunk starts.
    spec.trim_start_matches(['-', '+']).split(',').next().and_then(|n| n.parse().ok()).unwrap_or(1)
}

/// Parse the diff of a single file. `None` when it has no hunks (binary, empty, "No changes.").
pub fn parse(diff: &str) -> Option<Patch> {
    let mut patch = Patch::default();
    let mut current: Option<Hunk> = None;
    for raw in diff.split('\n') {
        let line = raw.strip_suffix('\r').unwrap_or(raw);
        if line.starts_with("@@") {
            if let Some(h) = current.take() {
                patch.hunks.push(h);
            }
            let mut parts = line.split(' ');
            parts.next();
            let old_start = parts.next().map_or(1, parse_range);
            let new_start = parts.next().map_or(1, parse_range);
            current = Some(Hunk { header: line.to_owned(), old_start, new_start, lines: Vec::new() });
            continue;
        }
        match current.as_mut() {
            None => {
                if !line.is_empty() {
                    patch.header.push(line.to_owned());
                }
            }
            Some(hunk) => {
                if line.starts_with('\\') {
                    if let Some(last) = hunk.lines.last_mut() {
                        last.no_newline = true;
                    }
                // A file with Windows line endings has a `\r` at the end of each content line in the diff. It stays
                // in `text`, so the patches written back out still match the file.
                } else if let Some(text) = raw.strip_prefix('+') {
                    hunk.lines.push(DiffLine { kind: LineKind::Add, text: text.to_owned(), no_newline: false });
                } else if let Some(text) = raw.strip_prefix('-') {
                    hunk.lines.push(DiffLine { kind: LineKind::Del, text: text.to_owned(), no_newline: false });
                } else if let Some(text) = raw.strip_prefix(' ') {
                    hunk.lines.push(DiffLine { kind: LineKind::Context, text: text.to_owned(), no_newline: false });
                } else if line.is_empty() && raw.len() == line.len() {
                    // A trailing empty record after the final newline; a context line is never blank (it has its space).
                }
            }
        }
    }
    if let Some(h) = current.take() {
        patch.hunks.push(h);
    }
    (!patch.hunks.is_empty()).then_some(patch)
}

impl Hunk {
    /// Whether `line` can be chosen: only added and removed lines can.
    pub fn is_change(&self, line: usize) -> bool {
        self.lines.get(line).is_some_and(|l| l.kind != LineKind::Context)
    }

    pub fn change_lines(&self) -> BTreeSet<usize> {
        (0..self.lines.len()).filter(|&i| self.is_change(i)).collect()
    }
}

impl Patch {
    fn write_hunk(out: &mut String, hunk: &Hunk, keep: impl Fn(usize, &DiffLine) -> Keep) {
        let mut body = String::new();
        let (mut old_len, mut new_len) = (0usize, 0usize);
        for (i, line) in hunk.lines.iter().enumerate() {
            let prefix = match keep(i, line) {
                Keep::Drop => continue,
                Keep::Context => {
                    old_len += 1;
                    new_len += 1;
                    ' '
                }
                Keep::Change => match line.kind {
                    LineKind::Add => {
                        new_len += 1;
                        '+'
                    }
                    LineKind::Del => {
                        old_len += 1;
                        '-'
                    }
                    LineKind::Context => {
                        old_len += 1;
                        new_len += 1;
                        ' '
                    }
                },
            };
            body.push(prefix);
            body.push_str(&line.text);
            body.push('\n');
            if line.no_newline {
                body.push_str("\\ No newline at end of file\n");
            }
        }
        // git recounts from the body (`--recount`), so only the start matters here.
        let context = hunk.header.split("@@").nth(2).unwrap_or("").trim_end();
        out.push_str(&format!("@@ -{},{} +{},{} @@{}\n", hunk.old_start, old_len, hunk.old_start, new_len, context));
        out.push_str(&body);
    }

    fn header_text(&self) -> String {
        self.header.iter().map(|l| format!("{l}\n")).collect()
    }

    /// The patch for the whole of hunk `h`.
    pub fn hunk_patch(&self, h: usize) -> Option<String> {
        let hunk = self.hunks.get(h)?;
        let mut out = self.header_text();
        Self::write_hunk(&mut out, hunk, |_, _| Keep::Change);
        Some(out)
    }

    /// The patch for just `selected` lines of hunk `h`. `reverse` is for patches applied backwards
    /// (unstaging, discarding): there the lines left alone are the ones git must still find in place,
    /// so unselected additions stay as context and unselected removals drop out, the other way
    /// round from staging.
    pub fn lines_patch(&self, h: usize, selected: &BTreeSet<usize>, reverse: bool) -> Option<String> {
        let hunk = self.hunks.get(h)?;
        if !selected.iter().any(|&i| hunk.is_change(i)) {
            return None;
        }
        let mut out = self.header_text();
        Self::write_hunk(&mut out, hunk, |i, line| match line.kind {
            LineKind::Context => Keep::Context,
            _ if selected.contains(&i) => Keep::Change,
            LineKind::Add => if reverse { Keep::Context } else { Keep::Drop },
            LineKind::Del => if reverse { Keep::Drop } else { Keep::Context },
        });
        Some(out)
    }

    /// For the diff of a new file (one hunk, all additions): the file's text made of just the `selected` lines.
    /// `crlf` gives the lines Windows endings. The last line has no newline when the diff says so.
    pub fn new_file_text(&self, selected: &BTreeSet<usize>, crlf: bool) -> Option<String> {
        let hunk = self.hunks.first()?;
        let eol = if crlf { "\r\n" } else { "\n" };
        let mut text = String::new();
        for (i, line) in hunk.lines.iter().enumerate() {
            if line.kind != LineKind::Add || !selected.contains(&i) {
                continue;
            }
            text.push_str(line.text.trim_end_matches('\r'));
            if !line.no_newline {
                text.push_str(eol);
            }
        }
        (!text.is_empty()).then_some(text)
    }

    /// Rows for a two-column view: removed and added runs are laid side by side, context on both.
    pub fn side_by_side(&self) -> Vec<SideRow> {
        let mut rows = Vec::new();
        for (h, hunk) in self.hunks.iter().enumerate() {
            let (mut old_no, mut new_no) = (hunk.old_start, hunk.new_start);
            let mut i = 0;
            while i < hunk.lines.len() {
                let line = &hunk.lines[i];
                if line.kind == LineKind::Context {
                    let cell = |number| Cell { number, text: line.text.clone(), kind: LineKind::Context, line: i };
                    rows.push(SideRow { hunk: h, left: Some(cell(old_no)), right: Some(cell(new_no)) });
                    old_no += 1;
                    new_no += 1;
                    i += 1;
                    continue;
                }
                let dels_end = (i..hunk.lines.len()).find(|&j| hunk.lines[j].kind != LineKind::Del).unwrap_or(hunk.lines.len());
                let adds_end = (dels_end..hunk.lines.len()).find(|&j| hunk.lines[j].kind != LineKind::Add).unwrap_or(hunk.lines.len());
                let (dels, adds) = (i..dels_end, dels_end..adds_end);
                for k in 0..dels.len().max(adds.len()) {
                    let left = dels.clone().nth(k).map(|j| Cell { number: old_no + k, text: hunk.lines[j].text.clone(), kind: LineKind::Del, line: j });
                    let right = adds.clone().nth(k).map(|j| Cell { number: new_no + k, text: hunk.lines[j].text.clone(), kind: LineKind::Add, line: j });
                    rows.push(SideRow { hunk: h, left, right });
                }
                old_no += dels.len();
                new_no += adds.len();
                // An added run that directly follows context (no removals) ends here too.
                i = adds_end.max(dels_end).max(i + 1);
            }
        }
        rows
    }
}

enum Keep {
    Drop,
    Context,
    Change,
}

#[cfg(test)]
mod tests {
    use super::*;

    const DIFF: &str = "diff --git a/f.txt b/f.txt\nindex 111..222 100644\n--- a/f.txt\n+++ b/f.txt\n@@ -1,4 +1,4 @@ fn\n one\n-two\n+TWO\n three\n four\n@@ -10,3 +10,4 @@\n ten\n+ten-and-a-half\n eleven\n twelve\n\\ No newline at end of file\n";

    #[test]
    fn parses_header_hunks_lines_and_the_missing_newline_marker() {
        let p = parse(DIFF).unwrap();
        assert_eq!(p.header.len(), 4);
        assert_eq!(p.hunks.len(), 2);
        assert_eq!((p.hunks[0].old_start, p.hunks[0].new_start), (1, 1));
        assert_eq!(p.hunks[1].old_start, 10);
        assert_eq!(p.hunks[0].lines.iter().map(|l| l.kind).collect::<Vec<_>>(), [LineKind::Context, LineKind::Del, LineKind::Add, LineKind::Context, LineKind::Context]);
        assert!(p.hunks[1].lines.last().unwrap().no_newline);
        assert!(parse("No changes.\n").is_none());
    }

    #[test]
    fn a_hunk_patch_keeps_the_header_and_only_that_hunk() {
        let p = parse(DIFF).unwrap();
        let text = p.hunk_patch(1).unwrap();
        assert!(text.starts_with("diff --git a/f.txt b/f.txt\n"));
        assert!(text.contains("@@ -10,3 +10,4 @@"));
        assert!(text.contains("+ten-and-a-half\n"));
        assert!(!text.contains("TWO"));
        assert!(text.ends_with("\\ No newline at end of file\n"));
    }

    #[test]
    fn staging_chosen_lines_leaves_unchosen_removals_as_context_and_drops_unchosen_additions() {
        let p = parse(DIFF).unwrap();
        // Choose only the addition "TWO": "two" stays in the file, so it becomes context.
        let only_add: BTreeSet<usize> = [2].into();
        let text = p.lines_patch(0, &only_add, false).unwrap();
        assert!(text.contains("\n two\n"), "{text}");
        assert!(text.contains("\n+TWO\n"));
        assert!(text.contains("@@ -1,4 +1,5 @@"), "{text}");
        // Choose only the removal: the unchosen addition is dropped.
        let only_del: BTreeSet<usize> = [1].into();
        let text = p.lines_patch(0, &only_del, false).unwrap();
        assert!(text.contains("\n-two\n") && !text.contains("TWO"), "{text}");
        assert!(text.contains("@@ -1,4 +1,3 @@"), "{text}");
        // Reversed (unstaging), the unchosen addition stays as context instead.
        let text = p.lines_patch(0, &only_del, true).unwrap();
        assert!(text.contains("\n TWO\n") && text.contains("\n-two\n"), "{text}");
        assert!(p.lines_patch(0, &BTreeSet::from([0]), false).is_none(), "a context line is not a change");
    }

    #[test]
    fn the_text_of_chosen_new_file_lines_keeps_the_files_line_endings_and_missing_final_newline() {
        let diff = "diff --git a/n b/n\nnew file mode 100644\n--- /dev/null\n+++ b/n\n@@ -0,0 +1,3 @@\n+a\n+b\n+c\n\\ No newline at end of file\n";
        let p = parse(diff).unwrap();
        let all = p.hunks[0].change_lines();
        assert_eq!(p.new_file_text(&all, false).unwrap(), "a\nb\nc");
        assert_eq!(p.new_file_text(&all, true).unwrap(), "a\r\nb\r\nc");
        let first_two = BTreeSet::from([0usize, 1]);
        assert_eq!(p.new_file_text(&first_two, true).unwrap(), "a\r\nb\r\n");
        assert!(p.new_file_text(&BTreeSet::new(), false).is_none());
    }

    #[test]
    fn windows_line_endings_survive_a_round_trip_through_the_parser() {
        let diff = "diff --git a/f b/f\n--- a/f\n+++ b/f\n@@ -1,3 +1,3 @@\n one\r\n-two\r\n+TWO\r\n three\r\n";
        let p = parse(diff).unwrap();
        assert_eq!(p.hunks[0].lines[1].text, "two\r", "the carriage return stays with the line");
        let patch = p.hunk_patch(0).unwrap();
        assert!(patch.contains("\n-two\r\n+TWO\r\n"), "{patch:?}");
        assert_eq!(p.side_by_side()[1].left.as_ref().unwrap().text, "two\r");
    }

    #[test]
    fn side_by_side_pairs_removed_and_added_lines() {
        let p = parse(DIFF).unwrap();
        let rows = p.side_by_side();
        // one / (two | TWO) / three / four, then ten / (- | ten-and-a-half) / eleven / twelve
        assert_eq!(rows.len(), 8);
        assert_eq!(rows[1].left.as_ref().map(|c| (c.number, c.text.as_str())), Some((2, "two")));
        assert_eq!(rows[1].right.as_ref().map(|c| (c.number, c.text.as_str())), Some((2, "TWO")));
        assert_eq!(rows[5].left, None);
        assert_eq!(rows[5].right.as_ref().map(|c| (c.number, c.text.as_str())), Some((11, "ten-and-a-half")));
        assert_eq!(rows[6].left.as_ref().unwrap().number, 11);
        assert_eq!(rows[6].right.as_ref().unwrap().number, 12);
    }
}
