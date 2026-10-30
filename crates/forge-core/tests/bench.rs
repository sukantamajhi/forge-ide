//! Timings for the per-keystroke paths. They are `#[ignore]`d so a normal test run stays fast:
//! `cargo test --release -p forge-core --test bench -- --ignored --nocapture`

use std::time::Instant;

use forge_core::{csv, fuzzy, tags};

fn per_run(label: &str, runs: u32, mut f: impl FnMut()) {
    let start = Instant::now();
    for _ in 0..runs {
        f();
    }
    eprintln!("{label}: {:?}", start.elapsed() / runs);
}

#[test]
#[ignore]
fn word_suggestions_on_a_300k_file() {
    let text = "fn render_line(self, wonderful: usize) -> renderer { let rendering = 1; } ".repeat(4_000);
    per_run(&format!("words_with_prefix, {} chars", text.len()), 20, || {
        std::hint::black_box(fuzzy::words_with_prefix(&text, "ren", 12));
    });
}

#[test]
#[ignore]
fn tag_scan_and_rename_on_a_300k_html_file() {
    let html: Vec<char> = "<div class=\"card\"><p>some text here</p><img src='a.png'><span>x</span></div>\n".repeat(4_000).chars().collect();
    per_run(&format!("tags::scan, {} chars", html.len()), 10, || {
        std::hint::black_box(tags::scan(&html, false));
    });
    // A caret in the middle of text, the usual case while typing in a document.
    let caret = html.len() / 2 + 20;
    per_run("rename_partner with the caret in text", 10, || {
        std::hint::black_box(tags::rename_partner(&html, caret, false));
    });
}

#[test]
#[ignore]
fn csv_parse_100k_rows() {
    let text = "1,\"Person, Jr\",City,13.5,\"said \"\"hi\"\"\",active,2026-10-01,owner1,a;b;c,1,EU,7\n".repeat(100_000);
    per_run(&format!("csv::parse, {} bytes", text.len()), 3, || {
        std::hint::black_box(csv::parse(&text, ','));
    });
}
