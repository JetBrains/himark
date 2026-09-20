// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use super::*;

#[test]
fn typing_mid_file_in_a_big_rust_document_stays_bounded() {
    let source = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../../editor/src/document.rs"
    ))
    .expect("document.rs");
    assert!(source.len() > 50_000, "the gate wants a BIG file");
    let fonts = editor::embedded_fonts::source()();
    let theme = editor::Theme::embedded();
    let registry = std::sync::Arc::new(languages());
    let mut document = editor::Document::from_language(
        editor::Text::from_string_exact(&source),
        "rs",
        &registry,
        &fonts,
        &theme,
    );
    let outcome = editor::ReparseWork::capture(&document, registry.clone())
        .expect("parse")
        .run_reparse();
    document.apply_reparse_outcome(
        outcome,
        &fonts,
        &theme,
        &mut imba::effect::Batch::new().effects(),
    );
    let mut at = source.len() / 2;
    while !source.is_char_boundary(at) {
        at += 1;
    }

    let workshop = editor::test_document::test_workshop(theme.clone());
    let mut batch = imba::effect::Batch::new();
    let editor = document.add_editor(
        900.0,
        None,
        editor::EditorBuild::Bounded,
        &[],
        &fonts,
        &theme,
        &mut batch.effects(),
    );
    let mut view = editor::EditorView {
        document,
        editor,
        reports_geometry: false,
        location: None,
        gutter_width: 0.0,
        base: None,
    };
    let mut store = imba::Store::new();
    let ui = imba::UiCtx::dont_use_too_slow();
    ui.set(editor::env::UiFonts(fonts.clone()));
    let drain = |view: &mut editor::EditorView,
                 store: &mut imba::Store,
                 batch: &mut imba::effect::Batch<editor::EditorCommand>| {
        let mut rounds = 0;
        loop {
            let pending = himark::test_support::surviving_launches(std::mem::replace(
                batch,
                imba::effect::Batch::new(),
            ));
            if pending.is_empty() {
                break;
            }
            for effect in pending {
                rounds += 1;
                assert!(rounds < 10_000, "the tail must converge");
                let command = himark::test_support::handle_effect(effect, &workshop);
                imba::View::perform(view, store, &ui, command, &mut batch.effects());
            }
        }
    };
    drain(&mut view, &mut store, &mut batch);
    view.document.set_caret(editor, at as u32);

    let mut samples = Vec::new();
    for _ in 0..30 {
        let started = std::time::Instant::now();
        let mut batch = imba::effect::Batch::new();
        imba::View::perform(
            &mut view,
            &mut store,
            &ui,
            editor::EditorCommand::InsertText {
                text: "x".to_owned(),
            },
            &mut batch.effects(),
        );
        drain(&mut view, &mut store, &mut batch);
        samples.push(started.elapsed());
    }
    assert_eq!(
        view.document.text().byte_count(),
        source.len() + 30,
        "every keystroke landed"
    );
    samples.sort();
    let p50 = samples[samples.len() / 2].as_secs_f64() * 1000.0;
    let p95 = samples[samples.len() * 95 / 100].as_secs_f64() * 1000.0;
    eprintln!("[probe] code typing: p50={p50:.2}ms p95={p95:.2}ms");
    imba::perf::record("code-typing", "p50_ms", p50);
    imba::perf::record("code-typing", "p95_ms", p95);
    assert!(
        p50 < 5.0,
        "typing in a code document costs {p50:.2}ms p50 — \
             a keystroke is shaping far more than its line"
    );
}
