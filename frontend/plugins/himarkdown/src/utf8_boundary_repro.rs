use super::*;
use himark::{EditorCommand, EditorIdView, ReparseOutcome, ReparseWork};
use imba::{store::Store, View};

fn test_fonts() -> skia_safe::textlayout::FontCollection {
    himark::embedded_fonts::source()()
}

fn test_theme() -> himark::Theme {
    himark::Theme::embedded()
}

fn test_languages() -> std::sync::Arc<himark::SyntaxLanguages> {
    std::sync::Arc::new(markdown_languages(himark::SyntaxLanguages::new()))
}

fn test_cx() -> std::sync::Arc<himark::Workshop> {
    std::sync::Arc::new(himark::Workshop::new(
        himark::embedded_fonts::source(),
        test_theme(),
    ))
}

#[test]
fn the_search_design_doc_lays_out_completely() {
    let source = include_str!("../fixtures/utf8-repro.md");
    let fonts = himark::embedded_fonts::source()();
    let theme = himark::Theme::embedded();
    let document = document_from_markdown(source, &fonts, &theme);
    let width = theme.ui().window.first_pane_width;
    let view = himark::EditorView::complete(document, width, &fonts, &theme);
    assert!(
        view.find_misaligned_boundary().is_none(),
        "the layout tiles the text on char boundaries"
    );
}

#[test]
fn search_doc_edits_reparses_and_repairs_keep_boundaries_char_aligned() {
    let mut seed = 0x9e3779b97f4a7c15u64;
    let mut rand = move || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed
    };

    let source = include_str!("../fixtures/utf8-repro.md");
    let mut store = Store::new();
    let mut document = document_from_markdown(source, &test_fonts(), &test_theme());
    let narrow_editor = document.add_editor(
        320.0,
        None,
        himark::EditorBuild::Complete,
        &[],
        &test_fonts(),
        &test_theme(),
        &mut imba::effect::Batch::new().effects(),
    );
    let wide_editor = document.add_editor(
        720.0,
        None,
        himark::EditorBuild::Complete,
        &[],
        &test_fonts(),
        &test_theme(),
        &mut imba::effect::Batch::new().effects(),
    );
    let document_id =
        himark::OpenDocuments::register(&mut store, document.clone(), None, "test".to_owned(), 0);
    let narrow = EditorIdView::new(document_id, narrow_editor);
    let wide = EditorIdView::new(document_id, wide_editor);
    store.put(himark::env::Fonts(himark::embedded_fonts::source()));

    let texts = [
        "—",
        "…",
        "x",
        "\n\n",
        "**x**",
        "| a | b |\n|---|---|\n",
        "é",
    ];
    let mut pending_reparses: Vec<ReparseOutcome> = Vec::new();
    let mut pending_commands: Vec<EditorCommand> = Vec::new();

    for step in 0..1200u32 {
        let editor = if rand() % 2 == 0 { narrow } else { wide };
        let mut batch: imba::effect::Batch<EditorCommand> = imba::effect::Batch::new();
        match rand() % 8 {
            0..=3 => {
                let mut view = editor;
                let command = EditorCommand::InsertText {
                    text: texts[(rand() % texts.len() as u64) as usize].to_owned(),
                };
                view.perform(
                    &mut store,
                    &imba::UiCtx::new(),
                    command,
                    &mut batch.effects(),
                );
            }
            4 => {
                let mut view = editor;
                view.perform(
                    &mut store,
                    &imba::UiCtx::new(),
                    EditorCommand::Backspace,
                    &mut batch.effects(),
                );
            }
            5 => {
                let mut view = editor;
                let y = (rand() % 12_000) as f32;
                view.perform(
                    &mut store,
                    &imba::UiCtx::new(),
                    EditorCommand::Click {
                        kind: himark::ClickKind::Set,
                        point: skia_safe::Point::new((rand() % 500) as f32, y),
                    },
                    &mut batch.effects(),
                );
            }
            6 => {
                if let Some(work) = ReparseWork::capture(
                    himark::OpenDocuments::document_ref(&store, document_id).expect("document"),
                    test_languages(),
                ) {
                    pending_reparses.push(work.run_reparse());
                }
            }
            _ => {
                if !pending_reparses.is_empty() && rand() % 2 == 0 {
                    let index = (rand() % pending_reparses.len() as u64) as usize;
                    let outcome = pending_reparses.swap_remove(index);
                    let mut document = himark::OpenDocuments::document_ref(&store, document_id)
                        .expect("document")
                        .clone();
                    let mut local = imba::effect::Batch::new();
                    document.apply_reparse_outcome(
                        outcome,
                        &test_fonts(),
                        &test_theme(),
                        &mut local.effects(),
                    );
                    himark::OpenDocuments::put_document(&mut store, document_id, document);
                    pending_commands.extend(
                        himark::test_support::surviving_launches(local)
                            .into_iter()
                            .map(|effect| himark::test_support::handle_effect(effect, &test_cx())),
                    );
                } else if !pending_commands.is_empty() {
                    let index = (rand() % pending_commands.len() as u64) as usize;
                    let command = pending_commands.swap_remove(index);
                    let mut view = editor;
                    view.perform(
                        &mut store,
                        &imba::UiCtx::new(),
                        command,
                        &mut batch.effects(),
                    );
                }
            }
        }
        for effect in himark::test_support::surviving_launches(batch) {
            pending_commands.push(himark::test_support::handle_effect(effect, &test_cx()));
        }

        for (name, id) in [("narrow", narrow), ("wide", wide)] {
            let view = id.gathered(&store).expect("editor");
            if let Some(boundary) = view.find_misaligned_boundary() {
                panic!(
                    "step {step}: {name} layout boundary at byte {boundary} \
                         splits a UTF-8 character"
                );
            }
        }
    }
}

#[test]
fn the_search_design_doc_survives_the_bounded_open_tail() {
    let source = include_str!("../fixtures/utf8-repro.md");
    let fonts = himark::embedded_fonts::source()();
    let theme = himark::Theme::embedded();
    let mut document = document_from_markdown(source, &fonts, &theme);
    let width = theme.ui().window.first_pane_width;
    let mut batch = imba::effect::Batch::new();
    let editor = document.add_editor(
        width,
        None,
        himark::EditorBuild::Bounded,
        &[],
        &fonts,
        &theme,
        &mut batch.effects(),
    );

    let cx = std::sync::Arc::new(himark::Workshop::new(
        himark::embedded_fonts::source(),
        theme.clone(),
    ));
    let mut view = himark::EditorView {
        document,
        editor,
        reports_geometry: false,
        location: None,
        gutter_width: 0.0,
        base: None,
    };
    let mut store = imba::Store::new();
    let mut pending = himark::test_support::surviving_launches(std::mem::replace(
        &mut batch,
        imba::effect::Batch::new(),
    ));
    let mut rounds = 0;
    while let Some(effect) = pending.pop() {
        rounds += 1;
        assert!(rounds < 10_000, "the tail must converge");
        let command = himark::test_support::handle_effect(effect, &cx);
        imba::View::perform(
            &mut view,
            &mut store,
            &imba::UiCtx::new(),
            command,
            &mut batch.effects(),
        );
        pending.extend(himark::test_support::surviving_launches(std::mem::replace(
            &mut batch,
            imba::effect::Batch::new(),
        )));
    }
    assert!(
        view.find_misaligned_boundary().is_none(),
        "the repaired layout tiles the text on char boundaries"
    );
}
