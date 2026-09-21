// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use super::*;

fn test_fonts() -> skia_safe::textlayout::FontCollection {
    himark::embedded_fonts::source()()
}

fn test_theme() -> himark::Theme {
    himark::Theme::embedded()
}

use himark::{MarkupBuilder, ReparseWork, StyleId, SyntaxLanguage};
use std::sync::Arc;

struct Toy;

impl SyntaxLanguage for Toy {
    fn parse(
        &self,
        text: &Text,
        range: std::ops::Range<u32>,
        old: Option<&dyn SyntaxTree>,
    ) -> Option<Box<dyn SyntaxTree>> {
        let old = old.and_then(TsTree::of);
        Some(Box::new(TsTree(parse_markdown_incremental(
            text, &range, old,
        ))))
    }

    fn markup_for_changes(
        &self,
        _text: &Text,
        range: std::ops::Range<u32>,
        _tree: &dyn SyntaxTree,
        _changed: &[std::ops::Range<u32>],
        replacement: &mut MarkupBuilder,
        invalidated: &mut Vec<std::ops::Range<u32>>,
        _fonts: &skia_safe::textlayout::FontCollection,
        _theme: &himark::Theme,
    ) {
        let len = range.end - range.start;
        invalidated.push(0..len);
        replacement.push_styled(0..len, StyleId::Keyword);
    }
}

fn keyword_spans(document: &Document, line: std::ops::Range<u32>) -> Vec<std::ops::Range<u32>> {
    let mut inline = Vec::new();
    let mut hidden = Vec::new();
    document
        .markup()
        .marks_inline_hidden_in(line, &mut inline, &mut hidden);
    inline
        .iter()
        .filter(|interval| interval.id == StyleId::Keyword)
        .map(|interval| interval.range.clone())
        .collect()
}

#[test]
fn fenced_blocks_highlight_through_one_hierarchical_reparse() {
    let store = &imba::store::Store::new();
    let ui = himark::test_document::test_ui();
    let source = "title\n\n```toy\nabc def\n```\n";
    let content_start = source.find("abc").unwrap() as u32;
    let content_end = content_start + "abc def\n".len() as u32;
    let mut languages = himark::SyntaxLanguages::new();
    languages.register(&["toy"], Arc::new(Toy));
    let toy_languages = std::sync::Arc::new(markdown_languages(languages));
    let mut document = document_from_markdown(source, store, ui, &test_fonts(), &test_theme());
    let _editor = document.add_editor(
        400.0,
        None,
        himark::EditorBuild::Bounded,
        &[],
        store,
        ui,
        &test_fonts(),
        &test_theme(),
        &mut imba::effect::Batch::new().effects(),
    );

    assert!(
        keyword_spans(&document, content_start..content_end).is_empty(),
        "no colors before the first landing"
    );
    let outcome = ReparseWork::capture(&document, toy_languages.clone())
        .expect("document has a parse")
        .run_reparse();
    document.apply_reparse_outcome(
        outcome,
        store,
        ui,
        &test_fonts(),
        &test_theme(),
        &mut imba::effect::Batch::new().effects(),
    );
    assert_eq!(
        keyword_spans(&document, content_start..content_end),
        vec![0..content_end - content_start],
        "the one landing colors the block"
    );

    document.edit(
        &operation::Operation::insert_at(content_start + 3, "zz"),
        store,
        ui,
        &test_fonts(),
        &test_theme(),
        &mut imba::effect::Batch::new().effects(),
    );
    let outcome = ReparseWork::capture(&document, toy_languages.clone())
        .expect("document has a parse")
        .run_reparse();
    document.apply_reparse_outcome(
        outcome,
        store,
        ui,
        &test_fonts(),
        &test_theme(),
        &mut imba::effect::Batch::new().effects(),
    );
    assert_eq!(
        keyword_spans(&document, content_start..content_end + 2),
        vec![0..content_end + 2 - content_start],
        "the descent covered the edited content"
    );

    let recheck = ReparseWork::capture(&document, toy_languages.clone())
        .expect("parse")
        .run_reparse();
    let mut recheck_batch = imba::effect::Batch::new();
    document.apply_reparse_outcome(
        recheck,
        store,
        ui,
        &test_fonts(),
        &test_theme(),
        &mut recheck_batch.effects(),
    );
    assert!(recheck_batch.is_empty(), "a no-op reparse lands nothing");
}
