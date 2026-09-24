// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use super::*;
use himark::Application;

struct DummyCommand;
impl himark::DynamicCommand for DummyCommand {
    fn id(&self) -> &'static str {
        "test.dummy"
    }
    fn name(&self) -> String {
        "Dummy".to_owned()
    }
    fn perform(
        &self,
        _app: &mut Application,
        _store: &mut imba::store::Store,
        _window: himark::WindowId,
        _fx: &mut himark::AppFx<'_>,
    ) {
    }
}
fn dummy_command() -> AppCommand {
    AppCommand::Dynamic(
        himark::WindowId::from_raw(0),
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

    let mut batch: imba::effect::Batch<imba::DynCommand> = imba::effect::Batch::new();
    ModalView::set_query(&mut palette, &mut store, &ui, "row", &mut batch.effects());
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
