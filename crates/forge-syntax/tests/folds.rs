//! Where fold arrows go: on the line that introduces a block, and the fold stops before whatever follows it.
//! Regions are 1-based `(first, last)` line numbers, as the gutter shows them.

use forge_syntax::{Language, Syntax};
use ropey::Rope;

fn regions(language: Language, src: &str) -> Vec<(usize, usize)> {
    let rope = Rope::from_str(src);
    let mut syntax = Syntax::new(language).unwrap_or_else(|| panic!("{language:?} grammar failed to load"));
    syntax.parse(&rope, &[]);
    syntax.fold_regions().into_iter().map(|(a, b)| (a + 1, b + 1)).collect()
}

fn starts(r: &[(usize, usize)]) -> Vec<usize> {
    r.iter().map(|x| x.0).collect()
}

#[test]
fn go_folds_the_braced_block_not_the_statement_list_inside_it() {
    let r = regions(Language::Go, "package main\n\nfunc main() {\n\tif x {\n\t\ta()\n\t}\n\tfor i := 0; i < 3; i++ {\n\t\tb()\n\t}\n}\n");
    assert_eq!(r, vec![(3, 10), (4, 6), (7, 9)], "{r:?}");
}

#[test]
fn yaml_folds_a_key_with_its_children_only() {
    let r = regions(Language::Yaml, "a:\n  b: 1\n  c:\n    - x\n    - y\nd:\n  e: 2\n  f: 3\n");
    assert_eq!(r, vec![(1, 5), (3, 5), (6, 8)], "{r:?}");
}

#[test]
fn toml_and_markdown_regions_stop_before_the_next_heading() {
    let r = regions(Language::Toml, "[a]\nx = 1\ny = 2\n\n[[b]]\nz = 1\nw = 2\n");
    assert!(r.contains(&(1, 3)), "{r:?}");
    let r = regions(Language::Markdown, "# T\n\ntext\n\n```rust\nlet x = 1;\nlet y = 2;\n```\n\nafter\n");
    assert!(r.contains(&(5, 8)), "fenced block ends on its closing fence: {r:?}");
}

#[test]
fn a_brace_on_its_own_line_puts_the_arrow_on_the_header() {
    let cs = regions(Language::CSharp, "namespace N\n{\n    class C\n    {\n        void F()\n        {\n            a();\n            b();\n        }\n    }\n}\n");
    assert_eq!(starts(&cs), vec![1, 3, 5], "one arrow per header, none on a lone brace: {cs:?}");
    let js = regions(Language::JavaScript, "function g()\n{\n  return 1;\n  return 2;\n}\n");
    assert_eq!(js, vec![(1, 5)], "{js:?}");
    let c = regions(Language::C, "int main(void)\n{\n  if (x) {\n    a();\n    b();\n  }\n  return 0;\n}\n");
    assert_eq!(c, vec![(1, 8), (3, 6)], "{c:?}");
}

#[test]
fn a_brace_after_a_multi_line_signature_keeps_the_arrow_on_the_brace_line() {
    let r = regions(Language::Rust, "fn a(\n    x: i32,\n) -> i32 {\n    1\n}\n");
    assert!(r.contains(&(3, 5)), "{r:?}");
}

#[test]
fn function_bodies_fold_in_c_family_and_shell() {
    assert!(regions(Language::Cpp, "class C {\n  void f() {\n    a();\n    b();\n  }\n};\n").contains(&(2, 5)));
    assert!(regions(Language::Php, "<?php\nfunction f() {\n  a();\n  b();\n}\n").contains(&(2, 5)));
    assert!(regions(Language::Shell, "f() {\n  a\n  b\n}\n").contains(&(1, 4)));
}

#[test]
fn indented_bodies_fold_from_the_header_in_python_and_lua() {
    let py = regions(Language::Python, "class A:\n    def f(self,\n          x):\n        if x:\n            return 1\n        else:\n            return 3\n");
    assert_eq!(starts(&py), vec![1, 2, 4, 6], "{py:?}");
    let lua = regions(Language::Lua, "function f(x)\n  if x then\n    a()\n    b()\n  end\nend\n");
    assert!(lua.contains(&(1, 6)) && lua.contains(&(2, 4)), "{lua:?}");
}
