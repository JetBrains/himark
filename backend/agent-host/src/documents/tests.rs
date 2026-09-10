use super::*;
use himark_ahp_ext_types::{Replacement, TextOperation, TextPosition, TextRange};

fn position(line: u64, character: u64) -> TextPosition {
    TextPosition { line, character }
}

fn replacement(start: (u64, u64), end: (u64, u64), text: &str) -> Replacement {
    Replacement {
        range: TextRange {
            start: position(start.0, start.1),
            end: position(end.0, end.1),
        },
        text: text.to_owned(),
    }
}

fn operation(replacements: Vec<Replacement>) -> TextOperation {
    TextOperation { replacements }
}

fn apply(text: &str, operation: &TextOperation) -> Result<String, String> {
    let text = Text::from_string_exact(text);
    himark_ahp_ext_types::text::apply(&text, operation)
        .map(|applied| himark_ahp_ext_types::text::materialize(&applied))
}

#[test]
fn replacements_apply_simultaneously_against_the_base() {
    let text = "one\ntwo\nthree\n";

    let result = apply(
        text,
        &operation(vec![
            replacement((0, 0), (0, 3), "ONE"),
            replacement((1, 3), (2, 0), " "),
        ]),
    )
    .expect("applies");
    assert_eq!(result, "ONE\ntwo three\n");
}

#[test]
fn inserts_and_multibyte_boundaries() {
    let text = "héllo\n";

    let result = apply(text, &operation(vec![replacement((0, 3), (0, 3), "y")])).expect("applies");
    assert_eq!(result, "héyllo\n");

    assert!(apply(text, &operation(vec![replacement((0, 2), (0, 2), "x")])).is_err());
}

#[test]
fn crlf_terminators_and_the_final_empty_line() {
    let result = apply(
        "ab\r\ncd",
        &operation(vec![replacement((0, 2), (1, 0), "-")]),
    )
    .expect("applies");
    assert_eq!(result, "ab-cd");

    assert!(apply("ab\n", &operation(vec![replacement((1, 0), (1, 0), "!")])).is_ok());

    let result =
        apply("a\rb\n", &operation(vec![replacement((0, 3), (0, 3), "!")])).expect("applies");
    assert_eq!(result, "a\rb!\n");
}

#[test]
fn malformed_operations_answer_errors() {
    let text = "abc\ndef\n";

    assert!(apply(
        text,
        &operation(vec![
            replacement((0, 0), (0, 2), "x"),
            replacement((0, 1), (0, 3), "y"),
        ]),
    )
    .is_err());

    assert!(apply(
        text,
        &operation(vec![
            replacement((0, 1), (0, 1), "x"),
            replacement((0, 1), (0, 1), "y"),
        ]),
    )
    .is_err());

    assert!(apply(text, &operation(vec![replacement((0, 4), (0, 4), "x")])).is_err());

    assert!(apply(text, &operation(vec![replacement((9, 0), (9, 0), "x")])).is_err());

    assert!(apply(
        text,
        &operation(vec![
            replacement((0, 0), (0, 1), "x"),
            replacement((0, 1), (0, 2), "y"),
        ]),
    )
    .is_ok());
    assert!(apply(text, &operation(vec![])).is_ok());
}

#[test]
fn the_gate_applies_by_lineage_only() {
    let mut document = Document::open("x\n", mint(1));
    let creation = document.version();
    let edit = DocumentApplied {
        base: creation,
        operation: operation(vec![replacement((0, 1), (0, 1), "y")]),
        id: Uid(0xe1),
        origin: None,
    };
    assert!(document.dispatch(&edit));
    assert_eq!(document.version(), Uid(0xe1));

    assert!(!document.dispatch(&edit), "stale base");
    let malformed = DocumentApplied {
        base: Uid(0xe1),
        operation: operation(vec![replacement((9, 0), (9, 0), "!")]),
        id: Uid(0xe2),
        origin: None,
    };
    assert!(!document.dispatch(&malformed), "malformed discards");
    assert_eq!(document.version(), Uid(0xe1));
}

#[test]
fn edits_on_lines_ending_with_a_multibyte_character_apply() {
    let text = "width \u{00bc}\nsecond\n";
    let result = apply(text, &operation(vec![replacement((0, 0), (0, 0), "x")])).expect("applies");
    assert_eq!(result, "xwidth \u{00bc}\nsecond\n");

    let result = apply(text, &operation(vec![replacement((0, 8), (0, 8), "!")])).expect("applies");
    assert_eq!(result, "width \u{00bc}!\nsecond\n");
    assert!(apply(text, &operation(vec![replacement((0, 7), (0, 7), "!")])).is_err());

    let crlf = "tail \u{00bc}\r\nnext\n";
    let result = apply(crlf, &operation(vec![replacement((0, 7), (0, 7), "!")])).expect("applies");
    assert_eq!(result, "tail \u{00bc}!\r\nnext\n");
}
