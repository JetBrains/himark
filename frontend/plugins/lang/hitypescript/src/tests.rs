// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use super::*;
use editor::StyleId;

trait RunReparse {
    fn run_reparse(self) -> editor::ReparseOutcome;
}

impl RunReparse for editor::ReparseWork {
    fn run_reparse(self) -> editor::ReparseOutcome {
        editor::ReparseHandler(editor::test_document::test_workshop(
            editor::Theme::embedded(),
        ))
        .reparse(self)
    }
}

fn languages() -> editor::SyntaxLanguages {
    let mut registry = editor::SyntaxLanguages::new();
    register(&mut registry);
    registry
}

fn parsed() -> editor::Document {
    let store = &imba::store::Store::new();
    let ui = editor::test_document::test_ui();
    let fonts = editor::test_document::test_fonts_collection();
    let theme = editor::Theme::embedded();
    let registry = std::sync::Arc::new(languages());
    let mut document = editor::Document::from_language(
        editor::Text::from_string_exact(SOURCE),
        EXT,
        &registry,
        store,
        ui,
        &fonts,
        &theme,
    );
    let outcome = editor::ReparseWork::capture(&document, registry)
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

fn style_at(document: &editor::Document, needle: &str) -> Option<StyleId> {
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

const EXT: &str = "ts";
const SOURCE: &str = r#"// note
interface Shape {
  area(): number;
}

enum Color {
  Red,
  Green,
}

type Pair = [number, number];

function greet(name: string): string {
  const x: string = "hi " + name;
  return x;
}
"#;

#[test]
fn highlights() {
    let document = parsed();
    assert_eq!(
        style_at(&document, "function"),
        Some(StyleId::Keyword),
        "function"
    );
    assert_eq!(style_at(&document, "hi "), Some(StyleId::String), "hi ");
    assert_eq!(
        style_at(&document, "// note"),
        Some(StyleId::Comment),
        "// note"
    );
    assert_eq!(style_at(&document, "Shape"), Some(StyleId::Type), "Shape");
}

#[test]
fn outline() {
    let document = parsed();
    let items = document.outline_items();
    let titles: Vec<&str> = items
        .iter()
        .map(|(_, _, _, item)| item.title.as_str())
        .collect();
    assert_eq!(
        titles,
        vec![
            "interface Shape",
            "enum Color",
            "type Pair",
            "function greet"
        ]
    );
}

#[test]
fn folds() {
    let document = parsed();
    let folds = document.foldables_in(0..SOURCE.len() as u32);
    let texts: Vec<&str> = folds
        .iter()
        .map(|range| &SOURCE[range.start as usize..range.end as usize])
        .collect();
    assert_eq!(texts.len(), 3, "fold interiors: {texts:?}");
    assert!(texts[0].contains("area"), "fold 0: {:?}", texts[0]);
    assert!(texts[1].contains("Red"), "fold 1: {:?}", texts[1]);
    assert!(texts[2].contains("return x"), "fold 2: {:?}", texts[2]);
}
