// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use super::*;

struct DummyCommand;
impl himark::commands::HimarkCommand for DummyCommand {
    fn id(&self) -> &'static str {
        "test.dummy"
    }
    fn name(&self) -> String {
        "Dummy".to_owned()
    }
    fn perform(
        &self,
        _store: &mut imba::store::Store,
        _ui: &imba::ui::UiCtx,
        _bound: &imba::focus::Bindings,
        _fx: &mut himark::app::AppFx<'_>,
    ) {
    }
}
fn dummy_command() -> AppCommand {
    himark::app::windowed(
        workbench::window::WindowId::from_raw(0),
        std::sync::Arc::new(DummyCommand),
    )
}

fn palette_of(names: &[&str]) -> PaletteView {
    PaletteView::new(
        &Store::new(),
        &UiCtx::dont_use_too_slow(),
        names
            .iter()
            .map(|name| PresentableCommand::new("test.command", *name, dummy_command()))
            .collect(),
    )
}

#[test]
fn typing_filters_selection_moves_and_a_pick_hands_the_command_back() {
    let mut store = Store::new();
    let ui = UiCtx::dont_use_too_slow();
    let mut palette = palette_of(&["Toggle Checkbox", "Table: Insert Row Below"]);
    assert_eq!(
        palette.labels().len(),
        2,
        "everything matches the empty query"
    );

    // The palette OWNS its input now: typing goes through it.
    let mut batch: imba::effect::Batch<PaletteCommand> = imba::effect::Batch::new();
    palette.perform(
        &mut store,
        &ui,
        PaletteCommand::Input(editor::editor_view::EditorCommand::InsertText {
            text: "row".to_owned(),
        }),
        &mut batch.effects(),
    );
    assert_eq!(palette.labels(), vec!["Table: Insert Row Below"]);
    assert!(palette.take_request().is_none(), "typing asks nothing");

    // A five-row step over one row clamps: the same Select the key
    // table would emit lands on the only row.
    let step = palette.list.step_index(5).expect("a stepped row");
    let select = PaletteCommand::Rows(palette.list.select_command(step));
    palette.perform(
        &mut store,
        &ui,
        select,
        &mut imba::effect::Batch::new().effects(),
    );
    assert_eq!(palette.selected(), 0);

    palette.perform(
        &mut store,
        &ui,
        PaletteCommand::Pick(0),
        &mut imba::effect::Batch::new().effects(),
    );
    assert!(matches!(
        palette.take_request(),
        Some(ModalRequest::Perform(_))
    ));
    assert!(palette.take_request().is_none(), "drained once");
}

#[test]
fn escape_asks_to_close() {
    let mut store = Store::new();
    let ui = UiCtx::dont_use_too_slow();
    let mut palette = palette_of(&["Anything"]);
    palette.perform(
        &mut store,
        &ui,
        PaletteCommand::Close,
        &mut imba::effect::Batch::new().effects(),
    );
    assert!(matches!(palette.take_request(), Some(ModalRequest::Close)));
}

/// The palette over a window, a query typed — the redesign's sheet
/// (Palette.dc.html) at the mock's 2x size.
/// `HIMARK_SHOT=<dir> cargo test -p palette dump_palette_screenshot -- --ignored`
#[test]
#[ignore]
fn dump_palette_screenshot() {
    use himark::app_ext::AppExt;
    let Ok(dir) = std::env::var("HIMARK_SHOT") else {
        return;
    };
    let mut app = himark::app::Himark::new(himark::app::AppFonts::embedded());
    let _ = app.add_window();
    app.register_command(std::sync::Arc::new(TogglePalette));
    let mut surface = skia_safe::surfaces::raster_n32_premul((2880, 1800)).expect("surface");
    app.draw_window(app.sole_window(), surface.canvas());
    assert!(himark::test_driver::type_text(
        &mut app,
        "the document feed is an ordered lane — the servers hear edits in mirror order"
    ));
    assert!(app.perform_registered(app.sole_window(), "palette.toggle"));
    app.draw_window(app.sole_window(), surface.canvas());
    assert!(himark::test_driver::type_text(&mut app, "sel"));
    for tick in 0..30 {
        app.draw_window(app.sole_window(), surface.canvas());
        let busy = himark::test_driver::animate(
            &mut app,
            imba::anim::AnimationClock::from_millis(tick as f64 * 32.0),
        );
        if !busy && tick > 1 {
            break;
        }
    }
    let data = surface
        .image_snapshot()
        .encode(None, skia_safe::EncodedImageFormat::PNG, None)
        .expect("png");
    std::fs::create_dir_all(&dir).expect("the shot dir");
    std::fs::write(format!("{dir}/palette.png"), data.as_bytes()).expect("write");
}
