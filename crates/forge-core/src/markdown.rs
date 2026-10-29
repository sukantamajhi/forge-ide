//! Markdown for the preview: text in, a tree of blocks out. Drawing is the app's job.

use pulldown_cmark::{CodeBlockKind, Event, Options, Parser, Tag, TagEnd};

/// A run of text with one style.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Span {
    pub text: String,
    pub bold: bool,
    pub italic: bool,
    pub strike: bool,
    pub code: bool,
    /// The link target, when the text is a link.
    pub link: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ListItem {
    /// `Some` for a task list item (`- [ ]` / `- [x]`).
    pub checked: Option<bool>,
    pub blocks: Vec<Block>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Block {
    Heading { level: u8, spans: Vec<Span> },
    Paragraph(Vec<Span>),
    Code { lang: String, text: String },
    Quote(Vec<Block>),
    /// `start` is the first number of an ordered list.
    List { start: Option<u64>, items: Vec<ListItem> },
    Rule,
    Table { header: Vec<Vec<Span>>, rows: Vec<Vec<Vec<Span>>> },
}

enum Frame {
    Root(Vec<Block>),
    Quote(Vec<Block>),
    Item { checked: Option<bool>, blocks: Vec<Block> },
    List { start: Option<u64>, items: Vec<ListItem> },
}

#[derive(Default)]
struct Builder {
    stack: Vec<Frame>,
    inline: Vec<Span>,
    bold: u32,
    italic: u32,
    strike: u32,
    link: Option<String>,
    code: Option<(String, String)>,
    header: Vec<Vec<Span>>,
    rows: Vec<Vec<Vec<Span>>>,
    row: Vec<Vec<Span>>,
}

impl Builder {
    fn push_block(&mut self, block: Block) {
        match self.stack.last_mut() {
            Some(Frame::Root(blocks) | Frame::Quote(blocks) | Frame::Item { blocks, .. }) => blocks.push(block),
            // Blocks never belong directly to a list; the item holds them.
            _ => {}
        }
    }

    /// Text that is not inside a paragraph (a tight list item) becomes one.
    fn flush_paragraph(&mut self) {
        if !self.inline.is_empty() {
            let spans = std::mem::take(&mut self.inline);
            self.push_block(Block::Paragraph(spans));
        }
    }

    fn text(&mut self, text: &str, code: bool) {
        if let Some((_, buffer)) = self.code.as_mut() {
            buffer.push_str(text);
            return;
        }
        self.inline.push(Span { text: text.to_owned(), bold: self.bold > 0, italic: self.italic > 0, strike: self.strike > 0, code, link: self.link.clone() });
    }

    fn start(&mut self, tag: Tag) {
        match tag {
            Tag::BlockQuote(_) => {
                self.flush_paragraph();
                self.stack.push(Frame::Quote(Vec::new()));
            }
            Tag::List(start) => {
                self.flush_paragraph();
                self.stack.push(Frame::List { start, items: Vec::new() });
            }
            Tag::Item => self.stack.push(Frame::Item { checked: None, blocks: Vec::new() }),
            Tag::CodeBlock(kind) => {
                self.flush_paragraph();
                let lang = match kind {
                    CodeBlockKind::Fenced(lang) => lang.split_whitespace().next().unwrap_or("").to_owned(),
                    CodeBlockKind::Indented => String::new(),
                };
                self.code = Some((lang, String::new()));
            }
            Tag::Emphasis => self.italic += 1,
            Tag::Strong => self.bold += 1,
            Tag::Strikethrough => self.strike += 1,
            Tag::Link { dest_url, .. } => self.link = Some(dest_url.to_string()),
            Tag::Image { .. } => self.text("[image: ", false),
            Tag::Table(_) => {
                self.header.clear();
                self.rows.clear();
            }
            _ => {}
        }
    }

    fn end(&mut self, tag: TagEnd) {
        match tag {
            TagEnd::Paragraph => {
                let spans = std::mem::take(&mut self.inline);
                if !spans.is_empty() {
                    self.push_block(Block::Paragraph(spans));
                }
            }
            TagEnd::Heading(level) => {
                let spans = std::mem::take(&mut self.inline);
                self.push_block(Block::Heading { level: level as u8, spans });
            }
            TagEnd::BlockQuote(_) => {
                self.flush_paragraph();
                if let Some(Frame::Quote(blocks)) = self.stack.pop() {
                    self.push_block(Block::Quote(blocks));
                }
            }
            TagEnd::Item => {
                self.flush_paragraph();
                if let Some(Frame::Item { checked, blocks }) = self.stack.pop() {
                    if let Some(Frame::List { items, .. }) = self.stack.last_mut() {
                        items.push(ListItem { checked, blocks });
                    }
                }
            }
            TagEnd::List(_) => {
                if let Some(Frame::List { start, items }) = self.stack.pop() {
                    self.push_block(Block::List { start, items });
                }
            }
            TagEnd::CodeBlock => {
                if let Some((lang, text)) = self.code.take() {
                    self.push_block(Block::Code { lang, text: text.trim_end_matches('\n').to_owned() });
                }
            }
            TagEnd::Emphasis => self.italic = self.italic.saturating_sub(1),
            TagEnd::Strong => self.bold = self.bold.saturating_sub(1),
            TagEnd::Strikethrough => self.strike = self.strike.saturating_sub(1),
            TagEnd::Link => self.link = None,
            TagEnd::Image => self.text("]", false),
            TagEnd::TableCell => {
                let cell = std::mem::take(&mut self.inline);
                self.row.push(cell);
            }
            TagEnd::TableHead => self.header = std::mem::take(&mut self.row),
            TagEnd::TableRow => {
                let row = std::mem::take(&mut self.row);
                self.rows.push(row);
            }
            TagEnd::Table => {
                let (header, rows) = (std::mem::take(&mut self.header), std::mem::take(&mut self.rows));
                self.push_block(Block::Table { header, rows });
            }
            _ => {}
        }
    }
}

/// Parse Markdown (CommonMark plus tables, strikethrough and task lists) into blocks.
pub fn parse(text: &str) -> Vec<Block> {
    let mut b = Builder { stack: vec![Frame::Root(Vec::new())], ..Default::default() };
    for event in Parser::new_ext(text, Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TASKLISTS) {
        match event {
            Event::Start(tag) => b.start(tag),
            Event::End(tag) => b.end(tag),
            Event::Text(t) => b.text(&t, false),
            Event::Code(t) => b.text(&t, true),
            Event::SoftBreak => b.text(" ", false),
            Event::HardBreak => b.text("\n", false),
            Event::Rule => {
                b.flush_paragraph();
                b.push_block(Block::Rule);
            }
            Event::TaskListMarker(done) => {
                if let Some(Frame::Item { checked, .. }) = b.stack.last_mut() {
                    *checked = Some(done);
                }
            }
            // Raw HTML is not drawn.
            _ => {}
        }
    }
    b.flush_paragraph();
    match b.stack.into_iter().next() {
        Some(Frame::Root(blocks)) => blocks,
        _ => Vec::new(),
    }
}

/// The plain text of some spans.
pub fn plain(spans: &[Span]) -> String {
    spans.iter().map(|s| s.text.as_str()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn paragraph(blocks: &[Block], index: usize) -> &Vec<Span> {
        match &blocks[index] {
            Block::Paragraph(spans) => spans,
            other => panic!("expected a paragraph, got {other:?}"),
        }
    }

    #[test]
    fn headings_and_paragraphs_with_nested_emphasis() {
        let blocks = parse("# Title\n\nSome *italic*, **bold and _both_** and `code`.\n");
        assert_eq!(blocks[0], Block::Heading { level: 1, spans: vec![Span { text: "Title".into(), ..Default::default() }] });
        let spans = paragraph(&blocks, 1);
        assert_eq!(plain(spans), "Some italic, bold and both and code.");
        assert!(spans.iter().any(|s| s.text == "italic" && s.italic && !s.bold));
        assert!(spans.iter().any(|s| s.text == "both" && s.italic && s.bold));
        assert!(spans.iter().any(|s| s.text == "code" && s.code));
    }

    #[test]
    fn links_keep_their_target_and_soft_breaks_become_spaces() {
        let blocks = parse("see [the docs](https://example.com/a)\nnext line");
        let spans = paragraph(&blocks, 0);
        assert_eq!(plain(spans), "see the docs next line");
        assert!(spans.iter().any(|s| s.text == "the docs" && s.link.as_deref() == Some("https://example.com/a")));
        assert!(spans.iter().all(|s| s.text != "next line" || s.link.is_none()));
    }

    #[test]
    fn tight_nested_and_task_lists() {
        let blocks = parse("- one\n  - inner\n- [x] done\n- [ ] todo\n\n3. third\n4. fourth\n");
        let Block::List { start: None, items } = &blocks[0] else { panic!("{blocks:?}") };
        assert_eq!(items.len(), 3);
        assert_eq!(plain(paragraph(&items[0].blocks, 0)), "one");
        assert!(matches!(&items[0].blocks[1], Block::List { items, .. } if items.len() == 1));
        assert_eq!((items[1].checked, items[2].checked), (Some(true), Some(false)));
        let Block::List { start: Some(3), items } = &blocks[1] else { panic!("{blocks:?}") };
        assert_eq!(items.len(), 2);
    }

    #[test]
    fn code_blocks_quotes_rules_and_tables() {
        let blocks = parse("```rust\nfn main() {}\n```\n\n> quoted *text*\n\n---\n\n| a | b |\n|---|---|\n| 1 | 2 |\n| 3 | 4 |\n");
        assert_eq!(blocks[0], Block::Code { lang: "rust".into(), text: "fn main() {}".into() });
        let Block::Quote(inner) = &blocks[1] else { panic!("{blocks:?}") };
        assert_eq!(plain(paragraph(inner, 0)), "quoted text");
        assert_eq!(blocks[2], Block::Rule);
        let Block::Table { header, rows } = &blocks[3] else { panic!("{blocks:?}") };
        assert_eq!((plain(&header[0]), plain(&header[1])), ("a".to_owned(), "b".to_owned()));
        assert_eq!(rows.len(), 2);
        assert_eq!(plain(&rows[1][1]), "4");
    }

    #[test]
    fn strikethrough_images_and_raw_html() {
        let blocks = parse("~~gone~~ ![logo](a.png) <b>x</b>");
        let spans = paragraph(&blocks, 0);
        assert!(spans.iter().any(|s| s.text == "gone" && s.strike));
        assert!(plain(spans).contains("[image: logo]"));
        assert!(!plain(spans).contains("<b>"), "raw HTML is not shown");
    }

    #[test]
    fn empty_and_unterminated_input_do_not_panic() {
        assert!(parse("").is_empty());
        assert!(!parse("```\nunterminated").is_empty());
        assert!(!parse("> > > deep\n").is_empty());
    }
}
