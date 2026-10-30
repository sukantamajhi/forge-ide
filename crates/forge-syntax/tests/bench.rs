//! `cargo test --release -p forge-syntax --test bench -- --ignored --nocapture`

use std::time::Instant;

use forge_syntax::{Language, Syntax};
use forge_text::{Buffer, Selection, SelectionSet};

#[test]
#[ignore]
fn reparse_after_a_keystroke_in_a_big_rust_file() {
    let block = "pub fn function_1(a: usize, b: &str) -> Result<Vec<usize>, String> {\n    let value = compute(a, b) + 1;\n    if value > 10 { return Ok(vec![value, a]); }\n    // comment with some words\n    for i in 0..a { println!(\"{}\", i); }\n    Err(format!(\"failed {}\", b))\n}\n\n\n\n";
    let text = block.repeat(3000);
    let mut buffer = Buffer::new(&text);
    let t = Instant::now();
    let mut syntax = Syntax::new(Language::Rust).expect("rust grammar");
    eprintln!("Syntax::new (grammar + highlight query): {:?}", t.elapsed());
    let t = Instant::now();
    for lang in [Language::Python, Language::JavaScript, Language::TypeScript, Language::Html, Language::Css, Language::Json, Language::Markdown] {
        let _ = Syntax::new(lang);
    }
    eprintln!("Syntax::new for 7 more languages: {:?}", t.elapsed());
    let t = Instant::now();
    let _second = Syntax::new(Language::Rust);
    eprintln!("Syntax::new for a second Rust file (query already compiled): {:?}", t.elapsed());
    let start = Instant::now();
    syntax.parse(buffer.rope(), &[]);
    eprintln!("first parse, {} bytes: {:?}", text.len(), start.elapsed());
    for (label, at) in [("end of file", buffer.len_chars()), ("middle of file", buffer.len_chars() / 2)] {
        let mut total = std::time::Duration::ZERO;
        for _ in 0..20 {
            buffer.insert(&SelectionSet::single(Selection::caret(at)), "a");
            let edits = buffer.take_edits();
            let t = Instant::now();
            syntax.parse(buffer.rope(), &edits);
            total += t.elapsed();
        }
        eprintln!("reparse after typing at the {label}: {:?}", total / 20);
    }
    let t = Instant::now();
    let lines = syntax.highlight_lines(buffer.rope(), 10_000..10_040);
    eprintln!("highlight 40 lines: {:?} ({} spans)", t.elapsed(), lines.iter().map(Vec::len).sum::<usize>());
}
