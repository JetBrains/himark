// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use super::*;

#[test]
fn the_search_design_doc_cold_reparses_with_the_full_registry() {
    let store = &imba::store::Store::new();
    let ui = editor::test_document::test_ui();
    let source = include_str!("../../../himarkdown/fixtures/utf8-repro.md");
    let fonts = editor::test_document::test_fonts_collection();
    let theme = editor::Theme::embedded();
    let mut document = himarkdown::document_from_markdown(source, store, ui, &fonts, &theme);
    let parsers = std::sync::Arc::new(himarkdown::markdown_languages(languages()));

    let outcome = editor::ReparseWork::capture(&document, parsers.clone())
        .expect("a cold document reparses")
        .run_reparse();
    let _ = document.apply_reparse_outcome(
        outcome,
        store,
        ui,
        &fonts,
        &theme,
        &mut imba::effect::Batch::new().effects(),
    );

    if let Some(work) = editor::ReparseWork::capture(&document, parsers) {
        let outcome = work.run_reparse();
        let _ = document.apply_reparse_outcome(
            outcome,
            store,
            ui,
            &fonts,
            &theme,
            &mut imba::effect::Batch::new().effects(),
        );
    }

    let view = editor::EditorView::complete(
        document,
        theme.ui().window.first_pane_width,
        store,
        ui,
        &fonts,
        &theme,
    );
    assert!(view.find_misaligned_boundary().is_none());
}
