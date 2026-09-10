use std::ops::Range;

use editor::{Assist, AssistKind, AssistRequest, SyntaxLanguage};
use operation::Op;

fn language() -> hisitter::TreeSitterLanguage {
    hisitter::TreeSitterLanguage::new(
        tree_sitter_rust::LANGUAGE.into(),
        tree_sitter_rust::HIGHLIGHTS_QUERY,
    )
}

fn applied(source: &str, assist: &Assist) -> String {
    let mut out = String::new();
    let mut position = 0usize;
    for op in assist.operation.iter() {
        match op {
            Op::Retain(len) => {
                out.push_str(&source[position..position + len as usize]);
                position += len as usize;
            }
            Op::Insert(text) => out.push_str(&text),
            Op::Delete(text) => {
                assert_eq!(&source[position..position + text.len()], text);
                position += text.len();
            }
        }
    }
    out.push_str(&source[position..]);
    out
}

fn assist_on(source: &str, location: Range<u32>, kind: AssistKind) -> Option<(String, u32)> {
    let language = language();
    let text = text::Text::from_string_exact(source);
    let len = source.len() as u32;
    let tree = language.parse(&text, 0..len, None)?;
    let assist = language.assist(&AssistRequest {
        kind,
        text: &text,
        tree: tree.as_ref(),
        range: 0..len,
        location,
    })?;
    let caret = assist.caret.expect("code assists name their caret");
    Some((applied(source, &assist), caret))
}

#[test]
fn enter_between_braces_double_breaks() {
    let (text, caret) =
        assist_on("fn main() {}", 11..11, AssistKind::Enter { soft: false }).expect("assists");
    assert_eq!(text, "fn main() {\n    \n}");
    assert_eq!(caret, 16, "the caret sits on the indented middle line");
}

#[test]
fn enter_after_an_opener_indents_one_deeper() {
    let source = "fn main() {\n    foo(";
    let caret = source.len() as u32;
    let (text, after) =
        assist_on(source, caret..caret, AssistKind::Enter { soft: false }).expect("assists");
    assert_eq!(text, "fn main() {\n    foo(\n        ");
    assert_eq!(after, source.len() as u32 + 9);
}

#[test]
fn enter_copies_the_line_indent() {
    let source = "fn main() {\n    let x = 1;";
    let caret = source.len() as u32;
    let (text, after) =
        assist_on(source, caret..caret, AssistKind::Enter { soft: false }).expect("assists");
    assert_eq!(text, "fn main() {\n    let x = 1;\n    ");
    assert_eq!(after, source.len() as u32 + 5);
}

#[test]
fn balanced_parens_auto_close() {
    let (text, caret) = assist_on("fn main() {}", 11..11, AssistKind::Typed('(')).expect("assists");
    assert_eq!(text, "fn main() {()}");
    assert_eq!(caret, 12, "the caret parks between the pair");
}

#[test]
fn an_unmatched_paren_suppresses_the_close() {
    let source = "fn main() { (1 }";
    assert!(assist_on(source, 15..15, AssistKind::Typed('(')).is_none());
}

#[test]
fn quotes_close_on_even_counts_only() {
    let (text, caret) = assist_on("fn main() {}", 11..11, AssistKind::Typed('"')).expect("assists");
    assert_eq!(text, "fn main() {\"\"}");
    assert_eq!(caret, 12);

    let source = "fn main() { let s = \"unterminated; }";
    assert!(assist_on(source, 35..35, AssistKind::Typed('"')).is_none());
}

#[test]
fn tab_with_a_selection_indents_its_lines() {
    let source = "let a = 1;\nlet b = 2;";
    let (text, caret) =
        assist_on(source, 0..source.len() as u32, AssistKind::Indent).expect("assists");
    assert_eq!(text, "    let a = 1;\n    let b = 2;");
    assert_eq!(caret, 4);
}

#[test]
fn tab_with_a_collapsed_caret_stays_plain() {
    assert!(assist_on("let a = 1;", 4..4, AssistKind::Indent).is_none());
}

#[test]
fn shift_tab_outdents_the_line() {
    let source = "fn main() {\n        let x = 1;\n}";
    let (text, caret) = assist_on(source, 20..20, AssistKind::Outdent).expect("assists");
    assert_eq!(text, "fn main() {\n    let x = 1;\n}");
    assert_eq!(caret, 16);
}
