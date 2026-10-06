#![allow(unused_imports)]
use super::*;

#[test]
fn switching_workspaces_stashes_and_restores_the_workbench() {
    use himark::app::AppFonts;
    use std::sync::Arc;

    struct Switch {
        target: Option<ahp_wire::SessionId>,
        made: Arc<std::sync::Mutex<Option<ahp_wire::SessionId>>>,
    }
    impl himark::commands::WindowedCommand for Switch {
        fn id(&self) -> &'static str {
            "test.switch"
        }
        fn name(&self) -> String {
            "Test Switch".to_owned()
        }
        fn perform(
            &self,
            store: &mut Store,
            _ui: &imba::ui::UiCtx,
            window: ::workbench::window::WindowId,
            fx: &mut himark::app::AppFx<'_>,
        ) {
            let target = match self.target.clone() {
                Some(target) => target,
                None => ahp_wire::SessionId::mint_scratch(store),
            };
            *self.made.lock().unwrap() = Some(target.clone());
            himark::app::switch_session(store, window, target, fx)
        }
    }
    let switch = |app: &mut himark::app::Application,
                  target: Option<ahp_wire::SessionId>|
     -> ahp_wire::SessionId {
        let window = app.sole_window();
        let made = Arc::new(std::sync::Mutex::new(None));
        app.perform_batch(vec![himark::app::AppCommand::Windowed(
            window,
            Arc::new(Switch {
                target,
                made: Arc::clone(&made),
            }),
        )]);
        let result = made.lock().unwrap().take().expect("the switch ran");
        result
    };

    let fonts = AppFonts::embedded();
    let mut app = himark::app::Application::new(fonts);
    let _ = app.add_window();
    let window = app.sole_window();
    let first = himark::workspace::entity_session(
        ::workbench::window::Windows::window_ref(app.store(), window).expect("window"),
    );

    assert!(app.perform_registered(window, "workbench.split-pane"));
    assert_eq!(app.pane_count(), 2);
    assert!(himark::test_driver::type_text(&mut app, "hello A"));
    let a_text = app.focused_document_text().expect("A text");
    assert!(a_text.contains("hello A"));
    let documents_before = app.document_count();

    let second = switch(&mut app, None);
    assert_eq!(
        himark::workspace::entity_session(
            ::workbench::window::Windows::window_ref(app.store(), window).expect("window"),
        ),
        second
    );
    assert_eq!(app.pane_count(), 1, "a fresh workbench for B");
    assert_eq!(
        app.document_count(),
        0,
        "B starts with nothing open — a fresh session's workbench is vacant"
    );
    assert!(
        app.focused_document_text().is_none(),
        "B does not show A's document"
    );

    let _ = switch(&mut app, Some(first.clone()));
    assert_eq!(app.pane_count(), 2, "A's split survived the stash");
    assert!(app
        .focused_document_text()
        .expect("A restored")
        .contains("hello A"));
    let _ = switch(&mut app, Some(second.clone()));
    assert_eq!(
        app.document_count(),
        0,
        "toggling back mints nothing — B stays as it was left"
    );
    let _ = switch(&mut app, Some(first.clone()));
    assert_eq!(
        app.document_count(),
        documents_before,
        "A's world is its own again"
    );

    let _ = switch(&mut app, Some(first.clone()));
    assert_eq!(app.pane_count(), 2);
    assert_eq!(
        himark::workspace::entity_session(
            ::workbench::window::Windows::window_ref(app.store(), window).expect("window"),
        ),
        first
    );
}

#[test]
fn switching_dismisses_the_overlays_first() {
    use himark::app::AppFonts;
    use std::sync::Arc;

    #[derive(Clone)]
    struct NullModal;
    impl imba::View for NullModal {
        type Command = std::convert::Infallible;
        fn perform(
            &mut self,
            _store: &mut Store,
            _ui: &imba::ui::UiCtx,
            _command: Self::Command,
            _fx: &mut imba::effect::Effects<'_, Self::Command>,
        ) {
        }
        fn display<'a>(
            &'a self,
            _arena: &'a imba::arena::Arena,
            _store: &'a Store,
            _ui: &'a imba::ui::UiCtx,
        ) -> impl imba::layout::Layout<'a, std::convert::Infallible> + imba::layout::LayoutValue + 'a
        {
            imba::layout::laid(
                move |_arena: &'a imba::arena::Arena,
                      constraints: imba::constraints::Constraints| {
                    imba::leaf::leaf::<std::convert::Infallible>(
                        constraints.max.width,
                        constraints.max.height,
                    )
                },
            )
        }
    }
    impl hikit::modal::ModalView for NullModal {
        fn clone_modal(&self) -> Box<dyn hikit::modal::ModalView> {
            Box::new(self.clone())
        }
        fn take_request(&mut self) -> Option<hikit::modal::ModalRequest> {
            None
        }
        fn as_any(&self) -> &dyn std::any::Any {
            self
        }
    }

    struct SwitchFresh(Arc<std::sync::Mutex<Option<ahp_wire::SessionId>>>);
    impl himark::commands::WindowedCommand for SwitchFresh {
        fn id(&self) -> &'static str {
            "test.switch"
        }
        fn name(&self) -> String {
            "Test Switch".to_owned()
        }
        fn perform(
            &self,
            store: &mut Store,
            _ui: &imba::ui::UiCtx,
            window: ::workbench::window::WindowId,
            fx: &mut himark::app::AppFx<'_>,
        ) {
            let target = ahp_wire::SessionId::mint_scratch(store);
            *self.0.lock().unwrap() = Some(target.clone());
            himark::app::switch_session(store, window, target, fx)
        }
    }

    let fonts = AppFonts::embedded();
    let mut app = himark::app::Application::new(fonts);
    let _ = app.add_window();
    let window = app.sole_window();
    assert!(app.open_modal(window, Box::new(NullModal)));
    assert!(app.plugin_modal().is_some());

    let made = Arc::new(std::sync::Mutex::new(None));
    app.perform_batch(vec![himark::app::AppCommand::Windowed(
        window,
        Arc::new(SwitchFresh(Arc::clone(&made))),
    )]);
    let second = made.lock().unwrap().take().expect("the switch ran");
    let entity = ::workbench::window::Windows::window_ref(app.store(), window).expect("window");
    assert!(
        entity.plugin_modal().is_none() && entity.side_panel().is_none(),
        "the switch dismissed the overlays"
    );
    assert_eq!(himark::workspace::entity_session(&entity), second);
}
