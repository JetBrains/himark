// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

pub(crate) use helpers::*;

mod helpers {
    use super::super::*;

    pub(crate) fn fonts() -> skia_safe::textlayout::FontCollection {
        himark::embedded_fonts::source()()
    }

    pub(crate) fn theme() -> himark::Theme {
        himark::Theme::embedded()
    }

    pub(crate) fn languages() -> Arc<SyntaxLanguages> {
        let mut registry = SyntaxLanguages::new();
        register(&mut registry);
        Arc::new(himarkdown::markdown_languages(registry))
    }

    pub(crate) fn enrichers() -> Arc<himark::Enrichers> {
        let mut registry = himark::Enrichers::new();
        register_enricher(&mut registry);
        Arc::new(registry)
    }

    pub(crate) fn settle(document: &mut himark::Document, registry: &Arc<SyntaxLanguages>) {
        let store = &imba::store::Store::new();
        let ui = &imba::UiCtx::dont_use_too_slow();
        let outcome = himark::ReparseWork::capture(document, registry.clone())
            .expect("parse")
            .run_reparse();
        let invalidated = document.apply_reparse_outcome(
            outcome,
                store, ui,
            &fonts(),
            &theme(),
            &mut imba::effect::Batch::new().effects(),
        );
        if let Some(invalidated) = invalidated {
            document.enrich_sync(&enrichers(), &invalidated,
                store, ui, &fonts(), &theme());
        }
    }

    trait RunReparse {
        fn run_reparse(self) -> himark::ReparseOutcome;
    }

    impl RunReparse for himark::ReparseWork {
        fn run_reparse(self) -> himark::ReparseOutcome {
            himark::ReparseHandler(himark::test_support::test_workshop(theme())).reparse(self)
        }
    }
}

use super::*;

fn diagram_inlay(
    document: &himark::Document,
) -> Option<(Range<u32>, InlayMode, bool, Option<String>)> {
    let len = document.text().byte_count() as u32;
    document
        .all_inlays_in(0..len)
        .into_iter()
        .find_map(|interval| {
            let view = interval.inlay.view_as::<MermaidView>()?;
            Some((
                interval.range.clone(),
                interval.inlay.mode(),
                view.is_diagram(),
                view.svg().map(str::to_owned),
            ))
        })
}

#[test]
fn a_mermaid_fence_carries_the_diagram_under_it() {
        let store = &imba::store::Store::new();
        let ui = &imba::UiCtx::dont_use_too_slow();
    let source = "# Title\n\n```mermaid\nflowchart TD\n    Start --> Finish\n```\n\ntail\n";
    let registry = languages();
    let mut document = himark::Document::from_language(
        himark::Text::from_string_exact(source),
        "markdown",
        &registry,
                store, ui,
        &fonts(),
        &theme(),
    );
    settle(&mut document, &registry);
    let (range, mode, is_diagram, svg) =
        diagram_inlay(&document).expect("the fence renders a diagram inlay");
    assert_eq!(mode, InlayMode::Under, "the diagram sits BELOW the source");
    assert!(
        is_diagram,
        "the source parses: a diagram, not an error strip"
    );
    assert!(svg.expect("svg").contains("<svg"));
    assert!(range.end > range.start);
    let size = document
        .all_inlays_in(0..source.len() as u32)
        .into_iter()
        .find_map(|interval| {
            interval
                .inlay
                .view_as::<MermaidView>()
                .map(|view| view.scaled(600.0))
        })
        .expect("inlay size");
    assert!(
        size.height > 20.0,
        "the diagram takes real height: {size:?}"
    );
}

#[test]
fn typing_in_the_fence_rerenders_the_diagram() {
        let store = &imba::store::Store::new();
        let ui = &imba::UiCtx::dont_use_too_slow();
    let source = "```mermaid\nflowchart TD\n    Start --> Middle\n```\n";
    let registry = languages();
    let mut document = himark::Document::from_language(
        himark::Text::from_string_exact(source),
        "markdown",
        &registry,
                store, ui,
        &fonts(),
        &theme(),
    );
    settle(&mut document, &registry);
    let (_, _, _, before) = diagram_inlay(&document).expect("initial diagram");

    let at = source.find("\n```").expect("closing fence") as u32;
    document.edit(
        &operation::Operation::insert_at(at, "\n    Middle --> Finish"),
                store, ui,
        &fonts(),
        &theme(),
        &mut imba::effect::Batch::new().effects(),
    );
    settle(&mut document, &registry);

    let (_, _, is_diagram, after) = diagram_inlay(&document).expect("diagram survives");
    assert!(is_diagram);
    let after = after.expect("svg");
    assert_ne!(
        Some(after.clone()),
        before,
        "the landing brings a FRESH render, not the carried live view"
    );
    assert!(
        after.contains("Finish"),
        "the new node is in the rendered diagram"
    );
}

#[test]
fn a_pure_mermaid_file_renders_source_plus_diagram() {
        let store = &imba::store::Store::new();
        let ui = &imba::UiCtx::dont_use_too_slow();
    let source = "flowchart LR\n    A --> B\n    B --> C\n";
    let mut document = himark::Document::from_language(
        himark::Text::from_string_exact(source),
        "mmd",
        &languages(),
                store, ui,
        &fonts(),
        &theme(),
    );

    document.enrich_now(&enrichers(),
                store, ui, &fonts(), &theme());
    let (_, mode, is_diagram, _) =
        diagram_inlay(&document).expect("the whole file renders a diagram");
    assert_eq!(mode, InlayMode::Under);
    assert!(is_diagram);

    let extras: Vec<_> = document.document_scoped_markups().collect();
    assert!(himark::OverlaidMarkup::new(document.markup(), &extras)
        .block_marks_in(0..source.len() as u32)
        .ids()
        .contains(&himark::StyleId::SourceCode));
}

#[test]
fn broken_source_shows_the_error_strip_until_it_parses() {
        let store = &imba::store::Store::new();
        let ui = &imba::UiCtx::dont_use_too_slow();
    let source = "```mermaid\nnot a diagram at all\n```\n";
    let registry = languages();
    let mut document = himark::Document::from_language(
        himark::Text::from_string_exact(source),
        "markdown",
        &registry,
                store, ui,
        &fonts(),
        &theme(),
    );
    settle(&mut document, &registry);
    let (_, _, is_diagram, _) = diagram_inlay(&document).expect("an inlay still shows");
    assert!(!is_diagram, "unparseable source is the error strip");

    let at = source.find("not a diagram").expect("start") as u32;
    document.edit(
        &operation::Operation::from_ops([
            operation::Op::Retain(at),
            operation::Op::Delete("not a diagram at all".into()),
            operation::Op::Insert("flowchart TD\n    A --> B".into()),
        ]),
                store, ui,
        &fonts(),
        &theme(),
        &mut imba::effect::Batch::new().effects(),
    );
    settle(&mut document, &registry);
    let (_, _, is_diagram, _) = diagram_inlay(&document).expect("inlay");
    assert!(is_diagram, "fixed source renders again");
}
