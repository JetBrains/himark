// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use super::*;
use crate::{AppFonts, Application};

fn app_with_folder() -> (Application, crate::WindowId) {
    let fonts = AppFonts::embedded();
    let mut app = Application::new(fonts);
    let _ = app.add_window();
    let window = app.sole_window();

    let root = crate::ResourceLocation::new(
        crate::ResourceType::directory(),
        crate::Authority::new("test"),
        vec!["project".to_owned()],
    );
    struct Add(crate::ResourceLocation);
    impl crate::DynamicCommand for Add {
        fn id(&self) -> &'static str {
            "test.add"
        }
        fn name(&self) -> String {
            "Add".to_owned()
        }
        fn perform(
            &self,
            _app: &mut Application,
            store: &mut Store,
            window: crate::WindowId,
            _fx: &mut crate::AppFx<'_>,
        ) {
            let id = crate::test_support::seed_session_folders(store, &[self.0.clone()]);
            let mut discarded = imba::effect::Batch::new();
            crate::switch_session(store, window, id, &mut discarded.effects());
        }
    }
    app.perform_batch(vec![crate::AppCommand::Dynamic(
        window,
        std::sync::Arc::new(Add(root)),
    )]);
    (app, window)
}

#[test]
fn the_switcher_lists_workspaces_with_the_current_selected() {
    let (app, window) = app_with_folder();
    let view = SessionSwitcherView::open(app.store(), window);
    assert_eq!(view.labels(), &["Local", "project", "+ New Scratch"]);
    assert_eq!(view.selected(), 1, "the seeded session is current");
}

#[test]
fn picks_file_the_switch_requests() {
    let (app, window) = app_with_folder();
    let ui = ::editor::test_document::test_ui();

    let mut scratch = Store::new();
    let mut view = SessionSwitcherView::open(app.store(), window);

    view.perform(
        &mut scratch,
        &ui,
        SwitcherCommand::Pick(1),
        &mut imba::effect::Batch::new().effects(),
    );
    let Some(ModalRequest::Close) = ModalView::take_request(&mut view) else {
        panic!("picking the current workspace closes");
    };

    view.perform(
        &mut scratch,
        &ui,
        SwitcherCommand::Pick(2),
        &mut imba::effect::Batch::new().effects(),
    );
    let Some(ModalRequest::Perform(crate::AppCommand::Dynamic(_, command))) =
        ModalView::take_request(&mut view)
    else {
        panic!("the new-workspace pick performs the switch");
    };
    assert_eq!(command.id(), "session.switch-to");
}

#[test]
fn selection_moves_and_clamps() {
    let (app, window) = app_with_folder();
    let ui = ::editor::test_document::test_ui();
    let mut scratch = Store::new();
    let mut view = SessionSwitcherView::open(app.store(), window);
    view.perform(
        &mut scratch,
        &ui,
        SwitcherCommand::Select(1),
        &mut imba::effect::Batch::new().effects(),
    );
    assert_eq!(view.selected(), 2);
    view.perform(
        &mut scratch,
        &ui,
        SwitcherCommand::Select(1),
        &mut imba::effect::Batch::new().effects(),
    );
    assert_eq!(view.selected(), 2, "clamped at the last row");
    view.perform(
        &mut scratch,
        &ui,
        SwitcherCommand::Select(-1),
        &mut imba::effect::Batch::new().effects(),
    );
    view.perform(
        &mut scratch,
        &ui,
        SwitcherCommand::Select(-1),
        &mut imba::effect::Batch::new().effects(),
    );
    assert_eq!(view.selected(), 0);
    view.perform(
        &mut scratch,
        &ui,
        SwitcherCommand::Select(-1),
        &mut imba::effect::Batch::new().effects(),
    );
    assert_eq!(view.selected(), 0, "clamped at the first row");
}
