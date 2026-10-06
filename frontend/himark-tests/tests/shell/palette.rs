#![allow(unused_imports)]
use super::*;

#[test]
fn palette_commands_follow_the_modal_focus() {
    use documents::OpenDocuments;
    use hikit::modal::ModalRequest;
    use hikit::modal::ModalView;
    use himark::app::AppCommand;
    use himark::app::AppFonts;
    use himark::app::Application;

    #[derive(Clone)]
    struct TestModal {
        document: documents::DocumentId,

        request: std::sync::Arc<std::sync::Mutex<Option<ModalRequest>>>,
    }
    #[derive(Clone, Debug)]
    enum TestModalCommand {
        Close,
        Show,
    }

    impl std::fmt::Display for TestModalCommand {
        fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            write!(out, "{self:?}")
        }
    }
    impl imba::View for TestModal {
        type Command = TestModalCommand;
        fn perform(
            &mut self,
            _store: &mut Store,
            _ui: &imba::ui::UiCtx,
            command: TestModalCommand,
            _fx: &mut imba::effect::Effects<'_, Self::Command>,
        ) {
            *self.request.lock().unwrap() = Some(match command {
                TestModalCommand::Close => ModalRequest::Close,
                TestModalCommand::Show => ModalRequest::ShowDocument(self.document),
            });
        }
        fn display<'a>(
            &'a self,
            _arena: &'a imba::arena::Arena,
            _store: &'a Store,
            _ui: &'a imba::ui::UiCtx,
        ) -> impl imba::layout::Layout<'a, TestModalCommand> + imba::layout::LayoutValue + 'a
        {
            imba::layout::laid(
                move |_arena: &'a imba::arena::Arena,
                      constraints: imba::constraints::Constraints| {
                    imba::leaf::leaf::<TestModalCommand>(
                        constraints.max.width,
                        constraints.max.height,
                    )
                },
            )
        }

        fn focus_data<'w>(
            &'w self,
            _store: &'w Store,
            _ui: &'w imba::ui::UiCtx,
        ) -> imba::focus::FocusData<'w, TestModalCommand> {
            imba::focus::FocusData::of_commands(vec![imba::PresentableCommand::new(
                "test.modal.close",
                "Close Test Modal",
                TestModalCommand::Close,
            )])
        }
    }
    impl ModalView for TestModal {
        fn clone_modal(&self) -> Box<dyn ModalView> {
            Box::new(self.clone())
        }

        fn take_request(&mut self) -> Option<ModalRequest> {
            self.request.lock().unwrap().take()
        }
        fn as_any(&self) -> &dyn std::any::Any {
            self
        }
    }

    let fonts = AppFonts::embedded();
    let mut app = Application::new(fonts);
    let _ = app.add_window();
    let mut surface = skia_safe::surfaces::raster_n32_premul((800, 600)).expect("surface");
    app.draw_window(app.sole_window(), surface.canvas());
    let ids = |app: &Application| -> Vec<&'static str> {
        himark::commands::palette_commands(app.store(), &app.ui_handle(), app.sole_window())
            .iter()
            .map(|command| command.id)
            .collect()
    };
    assert_eq!(
        ids(&app),
        [
            "editor.undo",
            "editor.redo",
            "editor.select-all",
            "editor.toggle-softwrap",
            "editor.select-next-occurrence",
            "editor.select-all-occurrences",
            "editor.add-caret-above",
            "editor.add-caret-below",
            "editor.backspace",
            "editor.delete-forward",
            "editor.delete-word-back",
            "editor.delete-word-forward",
            "editor.newline",
            "editor.newline-soft",
            "editor.indent",
            "editor.outdent",
            "editor.move-left",
            "editor.select-left",
            "editor.move-right",
            "editor.select-right",
            "editor.move-up",
            "editor.select-up",
            "editor.move-down",
            "editor.select-down",
            "editor.move-word-left",
            "editor.select-word-left",
            "editor.move-word-right",
            "editor.select-word-right",
            "editor.move-line-start",
            "editor.select-line-start",
            "editor.move-line-end",
            "editor.select-line-end",
            "editor.move-doc-start",
            "editor.select-doc-start",
            "editor.move-doc-end",
            "editor.select-doc-end",
            "find.open",
            "completion.trigger",
            "find.next",
            "find.previous",
            "workbench.new-document",
            "workbench.split-pane",
            "workbench.close",
            "workbench.close-pane",
            "navigation.back",
            "navigation.forward",
            "toc.toggle",
            "theme.toggle",
            "theme.dark",
            "theme.light",
            "chat.composer",
            "session.add-folder",
            "session.new",
        ],
        "text focus offers the editor's caret commands, then the workbench actions"
    );

    assert!(app.add_document(
        app.sole_window(),
        plain_document("beta body"),
        "beta".to_owned(),
        false
    ));
    let beta = OpenDocuments::list(app.store(), app.sole_documents())
        .into_iter()
        .find(|(_, info)| info.name() == "beta")
        .expect("beta is open")
        .0;

    assert!(app.open_modal(
        app.sole_window(),
        Box::new(TestModal {
            document: beta,
            request: Default::default(),
        })
    ));
    assert_eq!(
        ids(&app).first(),
        Some(&"test.modal.close"),
        "the modal's commands come first"
    );

    let close =
        himark::commands::palette_commands(app.store(), &app.ui_handle(), app.sole_window())
            .remove(0)
            .command;
    app.perform_batch(vec![close]);
    assert_eq!(
        ids(&app).first(),
        Some(&"editor.undo"),
        "the modal closed; the base surface is back"
    );

    assert!(app.open_modal(
        app.sole_window(),
        Box::new(TestModal {
            document: beta,
            request: Default::default(),
        })
    ));
    app.perform_batch(vec![AppCommand::Content(
        app.sole_window(),
        ::workbench::window::WindowCommand::Modal(imba::dyn_view::DynCommand::new(
            TestModalCommand::Show,
        )),
    )]);
    assert!(app.plugin_modal().is_none(), "the show dismissed the modal");
    assert_eq!(
        app.focused_document_text().as_deref(),
        Some("beta body"),
        "the requested document took the focused pane"
    );

    assert_eq!(app.pane_count(), 1);
    let split =
        himark::commands::palette_commands(app.store(), &app.ui_handle(), app.sole_window())
            .into_iter()
            .find(|command| command.id == "workbench.split-pane")
            .expect("the split action")
            .command;
    app.perform_batch(vec![split]);
    assert_eq!(app.pane_count(), 2, "the scheduled split ran");
}

#[test]
fn registered_commands_present_and_dispatch_by_id() {
    use himark::app::AppFonts;
    use himark::app::Application;
    use himark::commands::WindowedCommand;

    #[derive(Clone)]
    struct Marker;
    struct Probe;
    impl WindowedCommand for Probe {
        fn id(&self) -> &'static str {
            "test.probe"
        }
        fn name(&self) -> String {
            "Probe".to_owned()
        }
        fn perform(
            &self,
            store: &mut Store,
            _ui: &imba::ui::UiCtx,
            _window: ::workbench::window::WindowId,
            _fx: &mut himark::app::AppFx<'_>,
        ) {
            store.put(Marker);
        }
    }

    let fonts = AppFonts::embedded();
    let mut app = Application::new(fonts);
    let _ = app.add_window();
    let mut surface = skia_safe::surfaces::raster_n32_premul((800, 600)).expect("surface");
    app.draw_window(app.sole_window(), surface.canvas());

    app.register_command(std::sync::Arc::new(Probe));
    let ids: Vec<&str> =
        himark::commands::palette_commands(app.store(), &app.ui_handle(), app.sole_window())
            .iter()
            .map(|command| command.id)
            .collect();
    assert_eq!(
        ids,
        [
            "editor.undo",
            "editor.redo",
            "editor.select-all",
            "editor.toggle-softwrap",
            "editor.select-next-occurrence",
            "editor.select-all-occurrences",
            "editor.add-caret-above",
            "editor.add-caret-below",
            "editor.backspace",
            "editor.delete-forward",
            "editor.delete-word-back",
            "editor.delete-word-forward",
            "editor.newline",
            "editor.newline-soft",
            "editor.indent",
            "editor.outdent",
            "editor.move-left",
            "editor.select-left",
            "editor.move-right",
            "editor.select-right",
            "editor.move-up",
            "editor.select-up",
            "editor.move-down",
            "editor.select-down",
            "editor.move-word-left",
            "editor.select-word-left",
            "editor.move-word-right",
            "editor.select-word-right",
            "editor.move-line-start",
            "editor.select-line-start",
            "editor.move-line-end",
            "editor.select-line-end",
            "editor.move-doc-start",
            "editor.select-doc-start",
            "editor.move-doc-end",
            "editor.select-doc-end",
            "find.open",
            "completion.trigger",
            "find.next",
            "find.previous",
            "workbench.new-document",
            "workbench.split-pane",
            "workbench.close",
            "workbench.close-pane",
            "navigation.back",
            "navigation.forward",
            "toc.toggle",
            "theme.toggle",
            "theme.dark",
            "theme.light",
            "chat.composer",
            "session.add-folder",
            "session.new",
            "test.probe",
        ],
        "registration order, after the built-ins"
    );

    assert!(
        !app.perform_registered(app.sole_window(), "no.such.command"),
        "unknown id: no-op"
    );
    assert!(app.perform_registered(app.sole_window(), "test.probe"));

    assert!(app.store().get::<Marker>().is_some());
}

#[test]
fn keymap_chords_resolve_through_the_palette_surface() {
    use himark::app::AppFonts;
    use himark::app::Application;
    use imba::event::{Key, Modifiers};
    let fonts = AppFonts::embedded();
    let mut app = Application::new(fonts);
    let _ = app.add_window();
    let mut surface = skia_safe::surfaces::raster_n32_premul((800, 600)).expect("surface");
    app.draw_window(app.sole_window(), surface.canvas());

    let cmd = Modifiers {
        command: true,
        ..Default::default()
    };

    assert!(himark::test_driver::type_text(&mut app, "typed"));
    assert_eq!(app.focused_document_text().as_deref(), Some("typed"));
    assert!(
        himark::test_driver::key(&mut app, Key::Char('z'), cmd),
        "the chord consumed"
    );
    assert_eq!(
        app.focused_document_text().as_deref(),
        Some(""),
        "cmd-z undid via the keymap"
    );
    let shifted = Modifiers {
        command: true,
        shift: true,
        ..Default::default()
    };

    assert!(himark::test_driver::key(&mut app, Key::Char('Z'), shifted));
    assert_eq!(
        app.focused_document_text().as_deref(),
        Some("typed"),
        "cmd-shift-Z redid via the keymap"
    );

    himark::keymap::Keymaps::set(
        &mut app.store_mut(),
        himark::keymap::Keymap::from_json(r#"{ "cmd-9": "no.such-command" }"#).expect("parses"),
    );
    assert!(
        !himark::test_driver::key(&mut app, Key::Char('9'), cmd),
        "an unoffered id leaves the key unconsumed"
    );

    himark::keymap::Keymaps::set(
        &mut app.store_mut(),
        himark::keymap::Keymap::from_json(r#"{ "backspace": "editor.undo" }"#).expect("parses"),
    );
    assert!(himark::test_driver::backspace(&mut app));
    assert_eq!(
        app.focused_document_text().as_deref(),
        Some(""),
        "backspace fired the rebound undo, not a delete"
    );

    himark::keymap::Keymaps::set(&mut app.store_mut(), himark::keymap::Keymap::embedded());
    assert!(himark::test_driver::key(&mut app, Key::Char('Z'), shifted));
    assert_eq!(app.focused_document_text().as_deref(), Some("typed"));
    assert!(himark::test_driver::backspace(&mut app));
    assert_eq!(
        app.focused_document_text().as_deref(),
        Some("type"),
        "the default keymap's backspace deletes"
    );
}
