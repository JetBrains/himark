use super::tests::*;

#[test]
fn bounded_open_converges_over_a_mermaid_fence() {
    let mut source = String::from("# Head\n\n");
    for block in 0..30 {
        for _ in 0..100 {
            source.push_str("filler paragraph with enough words to occupy a line or two.\n\n");
        }
        source.push_str(&format!(
            "```mermaid\nflowchart TD\n    A{block} --> B{block}\n```\n\n"
        ));
    }
    let registry = languages();
    let mut document = himark::Document::from_language(
        himark::Text::from_string_exact(&source),
        "markdown",
        &registry,
        &fonts(),
        &theme(),
    );

    let workshop = himark::test_support::test_workshop(theme());
    let mut batch = imba::effect::Batch::new();
    let editor = document.add_editor(
        900.0,
        None,
        himark::EditorBuild::Bounded,
        &[],
        &fonts(),
        &theme(),
        &mut batch.effects(),
    );
    let mut view = himark::EditorView {
        document,
        editor,
        reports_geometry: false,
        location: None,
        gutter_width: 0.0,
        base: None,
    };
    let mut store = imba::Store::new();
    let ui = imba::UiCtx::new();

    let outcome = himark::ReparseHandler(himark::test_support::test_workshop(theme()))
        .reparse(himark::ReparseWork::capture(&view.document, registry.clone()).expect("parse"));
    imba::View::perform(
        &mut view,
        &mut store,
        &ui,
        himark::EditorCommand::ApplyReparse(outcome),
        &mut batch.effects(),
    );

    let mut rounds = 0;
    let mut pending = himark::test_support::surviving_launches(std::mem::replace(
        &mut batch,
        imba::effect::Batch::new(),
    ));
    while let Some(effect) = pending.pop() {
        rounds += 1;
        assert!(rounds < 10_000, "the bounded tail must converge");
        let command = himark::test_support::handle_effect(effect, &workshop);
        imba::View::perform(&mut view, &mut store, &ui, command, &mut batch.effects());
        pending.extend(himark::test_support::surviving_launches(std::mem::replace(
            &mut batch,
            imba::effect::Batch::new(),
        )));
    }
    let live = view.document.element_heights(editor);
    let laid: f32 = live.iter().map(|(_, height)| height).sum();
    let fresh = himark::EditorView::complete(view.document.clone(), 900.0, &fonts(), &theme())
        .element_heights();
    let complete: f32 = fresh.iter().map(|(_, height)| height).sum();
    eprintln!("[probe] converged in {rounds} rounds: laid={laid:.0} complete={complete:.0}");
    assert!(
        (laid - complete).abs() < 2.0,
        "the tail covered the whole document: laid={laid:.0} complete={complete:.0}"
    );
}
