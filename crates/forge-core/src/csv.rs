//! CSV and TSV files read into a table for the preview: quoted fields (with `""`, commas and line breaks inside),
//! a guessed delimiter, and column widths.

/// A parsed table. Short rows are padded so every row has `columns` cells.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Table {
    pub delimiter: char,
    pub rows: Vec<Vec<String>>,
    pub columns: usize,
    /// More rows existed than were read.
    pub truncated: bool,
}

/// Rows read at most; beyond this the preview says so instead of building millions of cells.
pub const MAX_ROWS: usize = 100_000;

/// Guess the delimiter from the first few lines: the one of `, ; tab |` that appears the same number of times
/// (at least once) on the most lines, outside quotes. A `.tsv` file is tab-separated whatever it contains.
pub fn detect_delimiter(text: &str, tsv: bool) -> char {
    if tsv {
        return '\t';
    }
    let mut best = (0usize, ',');
    for candidate in [',', ';', '\t', '|'] {
        let counts: Vec<usize> = text.lines().take(20).map(|line| count_outside_quotes(line, candidate)).collect();
        let first = counts.first().copied().unwrap_or(0);
        if first == 0 {
            continue;
        }
        let agreeing = counts.iter().filter(|c| **c == first).count();
        // More agreeing lines wins; on a tie, more columns wins.
        let score = agreeing * 1000 + first.min(999);
        if score > best.0 {
            best = (score, candidate);
        }
    }
    best.1
}

fn count_outside_quotes(line: &str, delimiter: char) -> usize {
    let mut quoted = false;
    let mut n = 0;
    for c in line.chars() {
        if c == '"' {
            quoted = !quoted;
        } else if c == delimiter && !quoted {
            n += 1;
        }
    }
    n
}

/// Read `text` as delimited rows. Never fails: an unclosed quote runs to the end of the file.
pub fn parse(text: &str, delimiter: char) -> Table {
    let text = text.strip_prefix('\u{FEFF}').unwrap_or(text);
    let mut rows: Vec<Vec<String>> = Vec::new();
    let mut row: Vec<String> = Vec::new();
    let mut field = String::new();
    let mut quoted = false;
    let mut field_started = false;
    let mut truncated = false;
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if quoted {
            if c == '"' {
                if chars.peek() == Some(&'"') {
                    field.push('"');
                    chars.next();
                } else {
                    quoted = false;
                }
            } else {
                field.push(c);
            }
            continue;
        }
        match c {
            '"' if !field_started || field.is_empty() => {
                quoted = true;
                field_started = true;
            }
            c if c == delimiter => {
                row.push(std::mem::take(&mut field));
                field_started = false;
            }
            '\r' => {
                if chars.peek() == Some(&'\n') {
                    chars.next();
                }
                row.push(std::mem::take(&mut field));
                rows.push(std::mem::take(&mut row));
                field_started = false;
                if rows.len() >= MAX_ROWS {
                    truncated = chars.peek().is_some();
                    break;
                }
            }
            '\n' => {
                row.push(std::mem::take(&mut field));
                rows.push(std::mem::take(&mut row));
                field_started = false;
                if rows.len() >= MAX_ROWS {
                    truncated = chars.peek().is_some();
                    break;
                }
            }
            c => {
                field.push(c);
                field_started = true;
            }
        }
    }
    if !truncated && (field_started || !field.is_empty() || !row.is_empty()) {
        row.push(field);
        rows.push(row);
    }
    let columns = rows.iter().map(Vec::len).max().unwrap_or(0);
    for r in &mut rows {
        r.resize(columns, String::new());
    }
    Table { delimiter, rows, columns, truncated }
}

impl Table {
    /// Width in characters of each column (the longest cell among the first rows), between `min` and `max`.
    pub fn column_widths(&self, min: usize, max: usize) -> Vec<usize> {
        (0..self.columns)
            .map(|c| {
                self.rows
                    .iter()
                    .take(2000)
                    .map(|r| r[c].lines().next().unwrap_or("").chars().count())
                    .max()
                    .unwrap_or(0)
                    .clamp(min, max)
            })
            .collect()
    }
}

/// Whether a cell reads as a number, so the preview right-aligns it.
pub fn is_numeric(cell: &str) -> bool {
    let t = cell.trim().trim_start_matches(['$', '€', '£', '-', '+']).trim_end_matches('%');
    !t.is_empty() && t.chars().any(|c| c.is_ascii_digit()) && t.chars().all(|c| c.is_ascii_digit() || matches!(c, '.' | ',' | ' '))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rows(text: &str, d: char) -> Vec<Vec<String>> {
        parse(text, d).rows
    }

    #[test]
    fn plain_rows_and_padding() {
        let t = parse("a,b,c\n1,2\n3,4,5,6\n", ',');
        assert_eq!(t.columns, 4);
        assert_eq!(t.rows.len(), 3);
        assert_eq!(t.rows[1], vec!["1", "2", "", ""]);
        assert!(!t.truncated);
    }

    #[test]
    fn quotes_commas_escaped_quotes_and_line_breaks_inside_fields() {
        let r = rows("name,note\n\"Smith, J\",\"said \"\"hi\"\"\"\n\"two\nlines\",x\n", ',');
        assert_eq!(r[1], vec!["Smith, J", "said \"hi\""]);
        assert_eq!(r[2], vec!["two\nlines", "x"]);
        assert_eq!(r.len(), 3);
    }

    #[test]
    fn crlf_blank_fields_and_no_final_newline() {
        assert_eq!(rows("a,b\r\n,\r\nx,y", ','), vec![vec!["a", "b"], vec!["", ""], vec!["x", "y"]]);
        assert_eq!(rows("", ','), Vec::<Vec<String>>::new());
        assert_eq!(rows("a,\n", ','), vec![vec!["a", ""]]);
        assert_eq!(rows("\u{FEFF}a,b", ','), vec![vec!["a", "b"]], "a byte-order mark is dropped");
    }

    #[test]
    fn a_stray_or_unclosed_quote_never_loses_the_rest() {
        let r = rows("a,b\"c,d\n1,2\n", ',');
        assert_eq!(r[0], vec!["a", "b\"c", "d"], "a quote inside a field is just text");
        let r = rows("a,\"unclosed\n1,2\n", ',');
        assert_eq!(r.len(), 1);
        assert!(r[0][1].starts_with("unclosed"));
    }

    #[test]
    fn the_delimiter_is_guessed() {
        assert_eq!(detect_delimiter("a,b,c\n1,2,3\n", false), ',');
        assert_eq!(detect_delimiter("a;b;c\n1;2;3\n", false), ';');
        assert_eq!(detect_delimiter("a\tb\n1\t2\n", false), '\t');
        assert_eq!(detect_delimiter("a|b|c\n1|2|3\n", false), '|');
        assert_eq!(detect_delimiter("\"a,b\";c\n\"1,2\";3\n", false), ';', "commas inside quotes do not count");
        assert_eq!(detect_delimiter("just text\n", false), ',');
        assert_eq!(detect_delimiter("a,b\n1,2\n", true), '\t');
    }

    #[test]
    fn widths_and_numbers() {
        let t = parse("id,name\n1,Alexander\n22,Bo\n", ',');
        assert_eq!(t.column_widths(3, 12), vec![3, 9]);
        assert_eq!(t.column_widths(3, 6), vec![3, 6]);
        assert!(is_numeric("1,234.50") && is_numeric("-12") && is_numeric("$9.99") && is_numeric("45%"));
        assert!(!is_numeric("") && !is_numeric("abc") && !is_numeric("-") && !is_numeric("12 kg"));
    }

    #[test]
    fn huge_files_are_cut_not_loaded_whole() {
        let text = "a\n".repeat(MAX_ROWS + 10);
        let t = parse(&text, ',');
        assert_eq!(t.rows.len(), MAX_ROWS);
        assert!(t.truncated);
        assert!(!parse(&"a\n".repeat(MAX_ROWS), ',').truncated);
    }
}
