use super::*;
use crate::test_document::plain_document;

fn at(source: &str, pat: &str) -> u32 {
    source.find(pat).expect("pattern") as u32
}

#[test]
fn word_range_covers_the_identifier_under_the_byte() {
    let source = "let value = other.field;\n";
    let doc = plain_document(source);
    let value = at(source, "value");

    for col in value..value + 5 {
        assert_eq!(
            super::word_range(&doc, col),
            Some(value..value + 5),
            "column {col} resolves to the whole word"
        );
    }

    assert_eq!(super::word_range(&doc, at(source, " =")), None);

    assert_eq!(super::word_range(&doc, at(source, ".field")), None);

    let field = at(source, "field");
    assert_eq!(super::word_range(&doc, field + 2), Some(field..field + 5));

    assert_eq!(super::word_range(&doc, source.len() as u32 + 10), None);
}

#[test]
fn word_range_stops_at_line_edges() {
    let source = "alpha\nbeta\n";
    let doc = plain_document(source);
    let beta = at(source, "beta");
    assert_eq!(super::word_range(&doc, beta), Some(beta..beta + 4));
    assert_eq!(super::word_range(&doc, beta + 4), None, "the newline is not the word");
}
