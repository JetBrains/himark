// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use super::*;
use editor::StyleId;
use std::ops::Range;

fn test_theme() -> editor::Theme {
    editor::Theme::embedded()
}

fn spans_on(document: &editor::Document, line: Range<u32>) -> Vec<(Range<u32>, StyleId)> {
    let mut inline = Vec::new();
    let mut hidden = Vec::new();
    document
        .markup()
        .marks_inline_hidden_in(line, &mut inline, &mut hidden);
    inline
        .iter()
        .filter_map(|interval| match interval.id {
            id @ (StyleId::Keyword
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
            | StyleId::Embedded) => Some((interval.range.clone(), id)),
            _ => None,
        })
        .collect()
}

#[test]
fn rust_blocks_highlight_for_real() {
    let store = &imba::store::Store::new();
    let ui = &imba::UiCtx::dont_use_too_slow();
    let source = "title\n\n```rust\nfn main() { let x = 1; }\n```\n";
    let content_start = source.find("fn main").unwrap() as u32;
    let content_end = content_start + "fn main() { let x = 1; }\n".len() as u32;
    let fonts = editor::embedded_fonts::source()();
    let mut document = himarkdown::document_from_markdown(source, store, ui, &fonts, &test_theme());
    let _editor = document.add_editor(
        400.0,
        None,
        editor::EditorBuild::Bounded,
        &[],
        store,
        ui,
        &fonts,
        &test_theme(),
        &mut imba::effect::Batch::new().effects(),
    );
    let parsers = std::sync::Arc::new(himarkdown::markdown_languages(languages()));
    let outcome = editor::ReparseWork::capture(&document, parsers)
        .expect("document has a parse")
        .run_reparse();
    document.apply_reparse_outcome(
        outcome,
        store,
        ui,
        &fonts,
        &test_theme(),
        &mut imba::effect::Batch::new().effects(),
    );

    let spans = spans_on(&document, content_start..content_end);
    assert!(!spans.is_empty(), "the block is highlighted");
    let theme_at = |needle: &str| {
        let at = "fn main() { let x = 1; }".find(needle).unwrap() as u32;
        spans
            .iter()
            .find(|(range, _)| range.start <= at && at < range.end)
            .map(|(_, theme)| *theme)
    };
    assert_eq!(theme_at("fn"), Some(StyleId::Keyword));
    assert_eq!(theme_at("let"), Some(StyleId::Keyword));
    assert_eq!(theme_at("main"), Some(StyleId::Function));

    assert!(matches!(
        theme_at("1"),
        Some(StyleId::Number | StyleId::Constant)
    ));
}

#[test]
fn unknown_languages_stay_plain() {
    assert!(languages().get("cobol").is_none());
}
