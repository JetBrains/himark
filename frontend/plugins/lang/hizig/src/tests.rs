// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use super::*;
use editor::theme::StyleId;

trait RunReparse {
    fn run_reparse(self) -> editor::reparse::ReparseOutcome;
}

impl RunReparse for editor::reparse::ReparseWork {
    fn run_reparse(self) -> editor::reparse::ReparseOutcome {
        editor::reparse::ReparseHandler(editor::test_document::test_workshop(
            editor::theme::Theme::embedded(),
        ))
        .reparse(self)
    }
}

fn languages() -> editor::reparse::SyntaxLanguages {
    let mut registry = editor::reparse::SyntaxLanguages::new();
    register(&mut registry);
    registry
}

fn parsed() -> editor::document::Document {
    let store = &imba::store::Store::new();
    let ui = editor::test_document::test_ui();
    let fonts = editor::test_document::test_fonts_collection();
    let theme = editor::theme::Theme::embedded();
    let registry = std::sync::Arc::new(languages());
    let mut document = editor::document::Document::from_language(
        text::text::Text::from_string_exact(SOURCE),
        EXT,
        &registry,
        store,
        ui,
        &fonts,
        &theme,
    );
    let outcome = editor::reparse::ReparseWork::capture(&document, registry)
        .expect("the fixture parses")
        .run_reparse();
    document.apply_reparse_outcome(
        outcome,
        store,
        ui,
        &fonts,
        &theme,
        &mut imba::effect::Batch::new().effects(),
    );
    document
}

fn style_at(document: &editor::document::Document, needle: &str) -> Option<StyleId> {
    let at = SOURCE.find(needle).expect("the needle is in the fixture") as u32;
    let mut inline = Vec::new();
    let mut hidden = Vec::new();
    document
        .markup()
        .marks_inline_hidden_in(0..SOURCE.len() as u32, &mut inline, &mut hidden);
    inline
        .iter()
        .filter(|interval| {
            matches!(
                interval.id,
                StyleId::Keyword
                    | StyleId::String
                    | StyleId::Comment
                    | StyleId::Number
                    | StyleId::Type
                    | StyleId::Function
                    | StyleId::Variable
                    | StyleId::Constant
                    | StyleId::Operator
                    | StyleId::Punctuation
                    | StyleId::Attribute
                    | StyleId::Embedded
            )
        })
        .filter(|interval| interval.range.start <= at && at < interval.range.end)
        .map(|interval| interval.id)
        .last()
}

const EXT: &str = "zig";
const SOURCE: &str = r#"// note
const std = @import("std");

pub fn add(a: i32, b: i32) i32 {
    const c = a + b;
    return c;
}
"#;

#[test]
fn highlights() {
    let document = parsed();
    assert_eq!(
        style_at(&document, "return"),
        Some(StyleId::Keyword),
        "return"
    );
    assert_eq!(
        style_at(&document, "// note"),
        Some(StyleId::Comment),
        "// note"
    );
}

#[test]
fn outline() {
    let document = parsed();
    let items = document.outline_items();
    let titles: Vec<&str> = items
        .iter()
        .map(|(_, _, _, item)| item.title.as_str())
        .collect();
    assert_eq!(titles, vec!["pub fn add"]);
}

#[test]
fn folds() {
    let document = parsed();
    let folds = document.foldables_in(0..SOURCE.len() as u32);
    let texts: Vec<&str> = folds
        .iter()
        .map(|range| &SOURCE[range.start as usize..range.end as usize])
        .collect();
    assert_eq!(texts.len(), 1, "fold interiors: {texts:?}");
    assert!(texts[0].contains("a + b"), "fold 0: {:?}", texts[0]);
}
