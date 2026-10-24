// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use super::*;

#[test]
fn functions_emit_foldables() {
    let store = &imba::store::Store::new();
    let ui = &imba::ui::UiCtx::dont_use_too_slow();
    let source = "fn one() {\n    1;\n}\n\nfn tiny() { 2 }\n";
    let theme = editor::theme::embedded();
    let registry = std::sync::Arc::new(languages());
    let mut document = editor::document::Document::from_language(
        text::text::Text::from_string_exact(source),
        "rs",
        &registry,
        store,
        ui,
        &theme,
    );
    let outcome = editor::reparse::ReparseWork::capture(&document, registry)
        .expect("parse")
        .run_reparse();
    document.apply_reparse_outcome(
        outcome,
        store,
        ui,
        &theme,
        &mut imba::effect::Batch::new().effects(),
    );

    let start = source.find('{').expect("the brace") as u32 + 1;
    let end = source.find('}').expect("the brace") as u32;
    assert_eq!(
        document.foldables_in(0..source.len() as u32),
        vec![start..end],
        "the multi-line function's interior folds; the one-liner does not"
    );
}
