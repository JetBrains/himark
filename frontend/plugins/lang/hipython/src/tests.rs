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
    let ui = &imba::UiCtx::dont_use_too_slow();
    let fonts = editor::embedded_fonts::source()();
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

const EXT: &str = "py";
const SOURCE: &str = r#"# note
def greet(name):
    x = "hi " + name
    return x

class Point:
    def norm(self):
        return 0
"#;

#[test]
fn highlights() {
    let document = parsed();
    assert_eq!(style_at(&document, "def"), Some(StyleId::Keyword), "def");
    assert_eq!(style_at(&document, "hi "), Some(StyleId::String), "hi ");
    assert_eq!(
        style_at(&document, "# note"),
        Some(StyleId::Comment),
        "# note"
    );
    assert_eq!(
        style_at(&document, "greet"),
        Some(StyleId::Function),
        "greet"
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
    assert_eq!(titles, vec!["def greet", "class Point", "def norm"]);
}

#[test]
fn folds() {
    let document = parsed();
    let folds = document.foldables_in(0..SOURCE.len() as u32);
    let texts: Vec<&str> = folds
        .iter()
        .map(|range| &SOURCE[range.start as usize..range.end as usize])
        .collect();
    assert_eq!(texts.len(), 2, "fold interiors: {texts:?}");
    assert!(texts[0].contains("return x"), "fold 0: {:?}", texts[0]);
    assert!(texts[1].contains("def norm"), "fold 1: {:?}", texts[1]);
}

#[test]
fn python_blocks_highlight_in_markdown() {
    let store = &imba::store::Store::new();
    let ui = &imba::UiCtx::dont_use_too_slow();
    let source = "title\n\n```python\ndef greet():\n    return \"hi\"\n```\n";
    let fonts = editor::embedded_fonts::source()();
    let theme = editor::Theme::embedded();
    let mut document = himarkdown::document_from_markdown(source, store, ui, &fonts, &theme);
    let parsers = std::sync::Arc::new(himarkdown::markdown_languages(languages()));
    let outcome = editor::ReparseWork::capture(&document, parsers)
        .expect("document has a parse")
        .run_reparse();
    document.apply_reparse_outcome(
        outcome,
        store,
        ui,
        &fonts,
        &theme,
        &mut imba::effect::Batch::new().effects(),
    );

    let at = source.find("def").unwrap() as u32;
    let mut inline = Vec::new();
    let mut hidden = Vec::new();
    document
        .markup()
        .marks_inline_hidden_in(at..at + 3, &mut inline, &mut hidden);
    let ids: Vec<StyleId> = inline.iter().map(|interval| interval.id).collect();
    assert!(
        ids.contains(&StyleId::Keyword),
        "the fenced block's `def` is a keyword: {ids:?}"
    );
}
