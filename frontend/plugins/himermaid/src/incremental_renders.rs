// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use super::tests::*;
use super::*;

fn marked_renders(marker: &str) -> usize {
    RENDER_LOG
        .lock()
        .expect("render log")
        .iter()
        .filter(|source| source.contains(marker))
        .count()
}

#[test]
fn the_parse_never_renders_a_diagram() {
    let store = &imba::store::Store::new();
    let ui = &imba::UiCtx::dont_use_too_slow();
    let source = "```mermaid\nflowchart TD\n    ParseGateQx --> Nothing\n```\n";
    let registry = languages();
    let mut document = himark::Document::from_language(
        himark::Text::from_string_exact(source),
        "markdown",
        &registry,
        store,
        ui,
        &fonts(),
        &theme(),
    );

    let outcome = himark::ReparseHandler(himark::test_support::test_workshop(theme()))
        .reparse(himark::ReparseWork::capture(&document, registry.clone()).expect("parse"));
    let invalidated = document
        .apply_reparse_outcome(
            outcome,
            store,
            ui,
            &fonts(),
            &theme(),
            &mut imba::effect::Batch::new().effects(),
        )
        .expect("the landing applies");
    assert_eq!(
        marked_renders("ParseGateQx"),
        0,
        "the parse pipeline billed a render — the enrichment migration's whole point"
    );

    document.enrich_sync(&enrichers(), &invalidated, store, ui, &fonts(), &theme());
    assert_eq!(marked_renders("ParseGateQx"), 1, "the pass rendered it");
}

#[test]
fn typing_rerenders_only_the_touched_fence() {
    let store = &imba::store::Store::new();
    let ui = &imba::UiCtx::dont_use_too_slow();
    let source = "\
```mermaid\nflowchart TD\n    AlphaQx --> BetaQx\n```\n\n\
middle paragraph between the fences with plain words.\n\n\
```mermaid\nflowchart TD\n    GammaQx --> DeltaQx\n```\n";
    let registry = languages();
    let mut document = himark::Document::from_language(
        himark::Text::from_string_exact(source),
        "markdown",
        &registry,
        store,
        ui,
        &fonts(),
        &theme(),
    );
    settle(&mut document, &registry);
    assert_eq!(
        marked_renders("AlphaQx"),
        1,
        "the first fence rendered once"
    );
    assert_eq!(
        marked_renders("GammaQx"),
        1,
        "the second fence rendered once"
    );

    let at = source.find("AlphaQx --> BetaQx").expect("edge") as u32;
    document.edit(
        &operation::Operation::insert_at(at, "X"),
        store,
        ui,
        &fonts(),
        &theme(),
        &mut imba::effect::Batch::new().effects(),
    );
    settle(&mut document, &registry);
    assert_eq!(
        marked_renders("AlphaQx"),
        2,
        "the touched fence re-rendered"
    );
    assert_eq!(marked_renders("GammaQx"), 1, "the sibling did NOT");

    let at = source.find("middle").expect("paragraph") as u32 + 1;
    document.edit(
        &operation::Operation::insert_at(at, "y"),
        store,
        ui,
        &fonts(),
        &theme(),
        &mut imba::effect::Batch::new().effects(),
    );
    settle(&mut document, &registry);
    assert_eq!(
        marked_renders("AlphaQx"),
        2,
        "untouched fences never re-render"
    );
    assert_eq!(marked_renders("GammaQx"), 1);
}
