// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use super::*;
use himark::ReparseWork;

fn theme() -> himark::Theme {
    let json = include_str!("../../../editor/assets/theme.json").replace(
        "\"function\": {",
        "\"function\": { \"font_size\": 44.0, \"bold\": true,",
    );
    himark::Theme::from_json(&json).expect("the probe theme parses")
}

#[test]
fn a_markdown_rooted_scratch_styles_the_first_typed_heading() {
    let store = &imba::store::Store::new();
    let ui = himark::test_document::test_ui();
    let fonts = himark::embedded_fonts::source()();
    let theme = himark::Theme::embedded();
    let mut document = himark::Document::new(Text::from_string_exact(""), himark::Markup::new())
        .with_syntax(
            himark::Syntax::new("markdown".to_owned(), None, himark::Markup::new()),
            &[],
        );
    let editor = document.add_editor(
        400.0,
        None,
        himark::EditorBuild::Bounded,
        &[],
        store,
        ui,
        &fonts,
        &theme,
        &mut imba::effect::Batch::new().effects(),
    );
    let _ = editor;
    document.edit(
        &operation::Operation::insert_at(0, "# hi"),
        store,
        ui,
        &fonts,
        &theme,
        &mut imba::effect::Batch::new().effects(),
    );

    let parsers = std::sync::Arc::new(markdown_languages(himark::SyntaxLanguages::new()));
    let outcome = himark::ReparseWork::capture(&document, parsers)
        .expect("a rooted scratch reparses")
        .run_reparse();
    document.apply_reparse_outcome(
        outcome,
        store,
        ui,
        &fonts,
        &theme,
        &mut imba::effect::Batch::new().effects(),
    );

    let mut inline = Vec::new();
    let mut hidden = Vec::new();
    let marks = document
        .markup()
        .marks_inline_hidden_in(0..1, &mut inline, &mut hidden);
    assert!(
        marks
            .ids()
            .iter()
            .any(|id| matches!(id, himark::StyleId::Header(_))),
        "the typed heading styles after the landing (ids: {:?})",
        marks.ids()
    );
}

#[test]
fn rich_tokens_keep_incremental_and_fresh_layouts_equal() {
    let store = &imba::store::Store::new();
    let ui = himark::test_document::test_ui();
    let fonts = himark::embedded_fonts::source()();
    let source = "intro\n\n```rust\nfn main() { let x = 1; }\nfn other() {}\n```\n\noutro\n";
    let mut document = document_from_markdown(source, store, ui, &fonts, &theme());
    let editor = document.add_editor(
        700.0,
        None,
        himark::EditorBuild::Complete,
        &[],
        store,
        ui,
        &fonts,
        &theme(),
        &mut imba::effect::Batch::new().effects(),
    );
    let parsers = std::sync::Arc::new(markdown_languages({
        let mut registry = himark::SyntaxLanguages::new();
        registry.register(&["rust", "rs"], std::sync::Arc::new(RustLike));
        registry
    }));
    let outcome = ReparseWork::capture(&document, parsers)
        .expect("parse")
        .run_reparse();
    let mut batch = imba::effect::Batch::new();
    document.apply_reparse_outcome(outcome, store, ui, &fonts, &theme(), &mut batch.effects());

    let cx = test_cx_for_probe();
    for effect in himark::test_support::surviving_launches(batch) {
        let command = himark::test_support::handle_effect(effect, &cx);
        if let himark::EditorCommand::ApplyRepair(items) = command {
            for item in items {
                document.apply_repair(item);
            }
        }
    }

    let live = document.element_heights(editor);
    let fresh_view =
        himark::EditorView::complete(document.clone(), 700.0, store, ui, &fonts, &theme());
    let fresh = fresh_view.element_heights();
    assert_eq!(live, fresh, "incremental layout diverged from from-scratch");
}

fn test_cx_for_probe() -> std::sync::Arc<himark::Workshop> {
    std::sync::Arc::new(himark::Workshop::new(
        himark::embedded_fonts::source(),
        himark::Theme::embedded(),
    ))
}

struct RustLike;

impl himark::SyntaxLanguage for RustLike {
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
        text: &Text,
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
        let mut view = text.view();
        let raw = view.byte_string(range.start as usize, range.end as usize);
        for (at, _) in raw.match_indices("fn ") {
            replacement.push_styled(at as u32..at as u32 + 2, StyleId::Function);
        }
    }
}
