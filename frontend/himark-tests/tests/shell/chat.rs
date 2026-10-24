#![allow(unused_imports)]
use super::dock::{entity, located, settle, show_dock};
use super::*;
use hikit::modal::ModalRequest;
use hikit::modal::ModalView;
use himark::app::AppCommand;
use himark::app::AppFonts;
use himark::app::Himark;
use himark::app_ext::AppExt;
use himark::test_driver;
use imba::anim::AnimationClock;
use imba::constraints::Constraints;
use imba::event::{Event, EventResult, Key};
use imba::leaf::leaf;
use imba::store::Store;
use imba::thunk_ext::ThunkExt as _;
use imba::{ui::UiCtx, View};
use std::sync::{Arc, Mutex};

#[test]
fn switching_workspaces_stashes_the_chat_panel() {
    use himark::app::AppFonts;
    use std::sync::Arc;

    struct Switch {
        target: Option<ahp_wire::SessionId>,
        made: Arc<std::sync::Mutex<Option<ahp_wire::SessionId>>>,
    }
    impl himark::commands::HimarkCommand for Switch {
        fn id(&self) -> &'static str {
            "test.switch-chat"
        }
        fn name(&self) -> String {
            "Test Switch".to_owned()
        }
        fn perform(
            &self,
            store: &mut Store,
            _ui: &imba::ui::UiCtx,
            bound: &imba::focus::Bindings,
            fx: &mut himark::app::AppFx<'_>,
        ) {
        let Some(window) = himark::commands::bound_window(self.id(), bound) else {
            return;
        };
            let target = match self.target.clone() {
                Some(target) => target,
                None => ahp_wire::SessionId::mint_scratch(store),
            };
            *self.made.lock().unwrap() = Some(target.clone());
            himark::app::switch_session(store, window, target, fx)
        }
    }
    let switch = |app: &mut himark::app::Himark,
                  target: Option<ahp_wire::SessionId>|
     -> ahp_wire::SessionId {
        let window = app.sole_window();
        let made = Arc::new(std::sync::Mutex::new(None));
        app.perform_batch(vec![himark::app::windowed(
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
    let mut app = himark::app::Himark::new(fonts);
    let _ = app.add_window();
    let window = app.sole_window();
    let first = himark::workspace::entity_session(
        ::workbench::window::Windows::window_ref(app.store(), window).expect("window"),
    );
    let first_chats =
        ahp_session::session::state::Hosts::ensure_state(&mut app.store_mut(), &first).chats();

    {
        let ui = app.ui_ctx();
        let mut store = app.store_mut();
        let mut entity = ::workbench::window::Windows::window(&store, window).expect("window");
        let mut batch = imba::effect::Batch::<himark::app::AppCommand>::new();
        let _ = entity.open_panel(
            &mut store,
            &ui,
            Box::new(ahp_chat::chats::ChatPane::new(
                first_chats,
                ahp_wire::client::ChatUri::new("ahp-chat:/a"),
            )),
            &mut batch.effects(),
        );
        ::workbench::window::Windows::put(&mut store, window, entity);
    }
    fn mounted_chat(app: &himark::app::Himark) -> Option<String> {
        let entity = ::workbench::window::Windows::window_ref(app.store(), app.sole_window())
            .expect("window");
        let chat = entity.workbench().chat()?;
        match chat.panel() {
            ::workbench::workbench_node::Panel::Plugin(view) => view
                .as_any()
                .downcast_ref::<ahp_chat::chats::ChatPane>()
                .map(|pane| pane.chat().as_str().to_owned()),
            ::workbench::workbench_node::Panel::Editor(_) => None,
        }
    }
    assert_eq!(mounted_chat(&app).as_deref(), Some("ahp-chat:/a"));

    let second = switch(&mut app, None);
    assert_eq!(mounted_chat(&app), None, "B never shows A's chat");

    let _ = switch(&mut app, Some(first));
    assert_eq!(
        mounted_chat(&app).as_deref(),
        Some("ahp-chat:/a"),
        "A's chat panel rides its stashed workbench home"
    );
    let _ = second;
}

/// `chat.composer` (\u{2318}I): the chat is always open from the
/// workbench's point of view — the command fills the dedicated
/// slot on first use, re-focuses it after, and the chat NEVER
/// lands in the split tree or closes.
#[test]
fn the_composer_command_fronts_the_chat_panel() {
    let mut app = Himark::new(AppFonts::embedded());
    let _ = app.add_window();
    let window = app.sole_window();
    let mut surface = skia_safe::surfaces::raster_n32_premul((800, 600)).expect("surface");
    app.draw_window(window, surface.canvas());

    struct EnterSession;
    impl himark::commands::HimarkCommand for EnterSession {
        fn id(&self) -> &'static str {
            "test.enter-session"
        }
        fn name(&self) -> String {
            String::new()
        }
        fn perform(
            &self,
            store: &mut Store,
            _ui: &imba::ui::UiCtx,
            bound: &imba::focus::Bindings,
            fx: &mut himark::app::AppFx<'_>,
        ) {
        let Some(window) = himark::commands::bound_window(self.id(), bound) else {
            return;
        };
            let target = ahp_wire::SessionId {
                host: ahp_wire::client::HostId::LOCAL,
                session: ahp_wire::client::SessionUri::new("ahp-session:/volatile"),
            };
            himark::app::switch_session(store, window, target, fx)
        }
    }
    assert!(app.perform_command(himark::app::windowed(window, Arc::new(EnterSession))));

    let uri = ahp_wire::client::ChatUri::new("ahp-chat:/volatile");
    let home = ahp_wire::SessionId {
        host: ahp_wire::client::HostId::LOCAL,
        session: ahp_wire::client::SessionUri::new("ahp-session:/volatile"),
    };
    let chats =
        ahp_session::session::state::Hosts::ensure_state(&mut app.store_mut(), &home).chats();
    let panel = ahp_chat::chat::ChatPanel::new(
        app.store(),
        &app.ui_ctx(),
        ahp_wire::client::HostId::LOCAL,
        "ahp-session:/volatile",
        chats,
        uri.clone(),
        ahp_chat::chats::Chats::catalog(app.store(), chats)
            .unwrap_or_else(ahp_chat::chats::Catalog::noop),
    );
    ahp_chat::chats::Chats::put(&mut app.store_mut(), chats, uri.clone(), panel);
    settle(&mut app, &mut surface);

    let slot_chat = |app: &himark::app::Himark| -> Option<String> {
        let entity = ::workbench::window::Windows::window_ref(app.store(), app.sole_window())
            .expect("window");
        let chat = entity.workbench().chat()?;
        match chat.panel() {
            ::workbench::workbench_node::Panel::Plugin(view) => view
                .as_any()
                .downcast_ref::<ahp_chat::chats::ChatPane>()
                .map(|pane| pane.chat().as_str().to_owned()),
            ::workbench::workbench_node::Panel::Editor(_) => None,
        }
    };
    let tree_chat = |app: &himark::app::Himark| -> Option<String> {
        let entity = ::workbench::window::Windows::window_ref(app.store(), app.sole_window())
            .expect("window");
        let mut found = None;
        entity.workbench().root.for_each_pane(&mut |panel| {
            if let ::workbench::workbench_node::Panel::Plugin(view) = panel {
                if let Some(pane) = view.as_any().downcast_ref::<ahp_chat::chats::ChatPane>() {
                    found = Some(pane.chat().as_str().to_owned());
                }
            }
        });
        found
    };
    assert_eq!(slot_chat(&app), None, "nothing filled the chat slot yet");

    assert!(app.perform_registered(window, "chat.composer"));
    settle(&mut app, &mut surface);
    assert_eq!(
        slot_chat(&app).as_deref(),
        Some("ahp-chat:/volatile"),
        "the command fills the workbench's chat slot"
    );
    assert_eq!(tree_chat(&app), None, "the chat never enters the tree");

    // Idempotent: the standing slot is fronted, not duplicated.
    assert!(app.perform_registered(window, "chat.composer"));
    settle(&mut app, &mut surface);
    assert_eq!(slot_chat(&app).as_deref(), Some("ahp-chat:/volatile"));

    // The chat never closes: \u{2318}W on the focused chat is refused.
    assert!(app.perform_registered(window, "workbench.close"));
    settle(&mut app, &mut surface);
    assert_eq!(
        slot_chat(&app).as_deref(),
        Some("ahp-chat:/volatile"),
        "the chat slot refuses to close"
    );

    // ⌘I means "type here": a view parked on the transcript (a click
    // there sticks) is re-pointed at the composer by the command.
    let with_pane =
        |app: &mut himark::app::Himark,
         f: &mut dyn FnMut(&ahp_chat::chats::ChatPane, &mut Store)| {
            let entity = ::workbench::window::Windows::window(app.store(), app.sole_window())
                .expect("window");
            if let Some(chat) = entity.workbench().chat() {
                if let ::workbench::workbench_node::Panel::Plugin(view) = chat.panel() {
                    if let Some(pane) = view.as_any().downcast_ref::<ahp_chat::chats::ChatPane>() {
                        let pane = pane.clone();
                        f(&pane, &mut app.store_mut());
                    }
                }
            }
            let window = app.sole_window();
            ::workbench::window::Windows::put(&mut app.store_mut(), window, entity);
        };
    with_pane(&mut app, &mut |pane, store| {
        pane.set_focus_area(store, ahp_chat::chat::ChatArea::Transcript);
        assert_eq!(
            pane.focus_area(store),
            Some(ahp_chat::chat::ChatArea::Transcript)
        );
    });
    assert!(app.perform_registered(window, "chat.composer"));
    settle(&mut app, &mut surface);
    with_pane(&mut app, &mut |pane, store| {
        assert_eq!(
            pane.focus_area(store),
            Some(ahp_chat::chat::ChatArea::Composer),
            "⌘I lands the keyboard in the composer"
        );
    });
}

/// Presentation is the layout's call alone: a vacant tree hands
/// the whole workbench to the chat, a document splits the space
/// when the window fits both, a narrow window shows the document
/// alone (the chat slot stays, hidden), and closing the last
/// panel hands it all back.
#[test]
fn the_chat_docks_left_when_the_window_is_wide() {
    let mut app = Himark::new(AppFonts::embedded());
    let _ = app.add_window();
    let window = app.sole_window();
    // House units are 2x pixels: 3200 here is a 1600pt window.
    let mut wide = skia_safe::surfaces::raster_n32_premul((3200, 1000)).expect("surface");
    app.draw_window(window, wide.canvas());

    struct EnterSession;
    impl himark::commands::HimarkCommand for EnterSession {
        fn id(&self) -> &'static str {
            "test.enter-session"
        }
        fn name(&self) -> String {
            String::new()
        }
        fn perform(
            &self,
            store: &mut Store,
            _ui: &imba::ui::UiCtx,
            bound: &imba::focus::Bindings,
            fx: &mut himark::app::AppFx<'_>,
        ) {
        let Some(window) = himark::commands::bound_window(self.id(), bound) else {
            return;
        };
            let target = ahp_wire::SessionId {
                host: ahp_wire::client::HostId::LOCAL,
                session: ahp_wire::client::SessionUri::new("ahp-session:/volatile"),
            };
            himark::app::switch_session(store, window, target, fx)
        }
    }
    assert!(app.perform_command(himark::app::windowed(window, Arc::new(EnterSession))));

    let uri = ahp_wire::client::ChatUri::new("ahp-chat:/volatile");
    let home = ahp_wire::SessionId {
        host: ahp_wire::client::HostId::LOCAL,
        session: ahp_wire::client::SessionUri::new("ahp-session:/volatile"),
    };
    let chats =
        ahp_session::session::state::Hosts::ensure_state(&mut app.store_mut(), &home).chats();
    let panel = ahp_chat::chat::ChatPanel::new(
        app.store(),
        &app.ui_ctx(),
        ahp_wire::client::HostId::LOCAL,
        "ahp-session:/volatile",
        chats,
        uri.clone(),
        ahp_chat::chats::Chats::catalog(app.store(), chats)
            .unwrap_or_else(ahp_chat::chats::Catalog::noop),
    );
    ahp_chat::chats::Chats::put(&mut app.store_mut(), chats, uri.clone(), panel);
    settle(&mut app, &mut wide);

    fn held(app: &himark::app::Himark) -> &::workbench::window::Window {
        ::workbench::window::Windows::window_ref(app.store(), app.sole_window()).expect("window")
    }
    let slot_filled =
        |app: &himark::app::Himark| -> bool { held(app).workbench().chat().is_some() };
    let tree_chat = |app: &himark::app::Himark| -> Option<String> {
        let mut found = None;
        held(app).workbench().root.for_each_pane(&mut |panel| {
            if let ::workbench::workbench_node::Panel::Plugin(view) = panel {
                if let Some(pane) = view.as_any().downcast_ref::<ahp_chat::chats::ChatPane>() {
                    found = Some(pane.chat().as_str().to_owned());
                }
            }
        });
        found
    };
    let vacant = |app: &himark::app::Himark| -> bool { held(app).workbench().root.is_vacant() };

    assert!(app.perform_registered(window, "chat.composer"));
    settle(&mut app, &mut wide);
    assert!(slot_filled(&app), "\u{2318}I fills the chat slot");
    assert_eq!(tree_chat(&app), None, "the chat never enters the tree");
    assert!(vacant(&app), "a fresh session has nothing open");

    // Narrowing changes NOTHING about the state — visibility is
    // the layout's business, the slot stays put.
    let mut narrow = skia_safe::surfaces::raster_n32_premul((800, 600)).expect("surface");
    settle(&mut app, &mut narrow);
    assert!(slot_filled(&app), "the slot survives a narrow window");
    assert_eq!(tree_chat(&app), None);

    settle(&mut app, &mut wide);
    assert!(slot_filled(&app));

    // Opening a document splits the space with the chat...
    struct OpenDoc;
    impl himark::commands::HimarkCommand for OpenDoc {
        fn id(&self) -> &'static str {
            "test.open-doc"
        }
        fn name(&self) -> String {
            String::new()
        }
        fn perform(
            &self,
            store: &mut Store,
            ui: &imba::ui::UiCtx,
            bound: &imba::focus::Bindings,
            fx: &mut himark::app::AppFx<'_>,
        ) {
        let Some(window) = himark::commands::bound_window(self.id(), bound) else {
            return;
        };
            let state = himark::workspace::session_state(store, window).expect("state");
            let id = documents::OpenDocuments::register(
                store,
                state.documents(),
                himark::app::markdown_scratch(),
                None,
                "doc".to_owned(),
                0,
            );
            let mut entity = ::workbench::window::Windows::window(store, window).expect("window");
            fx.scope(himark::app::AppCommand::Verb, |fx| {
                entity.show_document(store, &ui, window, id, None, true, fx)
            });
            ::workbench::window::Windows::put(store, window, entity);
        }
    }
    assert!(app.perform_command(himark::app::windowed(window, Arc::new(OpenDoc))));
    settle(&mut app, &mut wide);
    assert!(!vacant(&app), "the document fills the vacant leaf");
    assert!(slot_filled(&app), "the chat stays beside it");

    // ...and closing the last panel hands it all back.
    assert!(app.perform_registered(window, "workbench.close"));
    settle(&mut app, &mut wide);
    assert!(vacant(&app), "closing the last panel empties the tree");
    assert!(slot_filled(&app), "the chat owns the workbench again");
}

/// `HIMARK_SHOT=<dir> cargo test -p himark dump_chat_narrow_screenshot -- --ignored`
/// — the squeezed case: a document open in a window too narrow
/// for both; the panel must win the whole workbench.
#[test]
#[ignore]
fn dump_chat_narrow_screenshot() {
    let Ok(dir) = std::env::var("HIMARK_SHOT") else {
        return;
    };
    let mut app = Himark::new(AppFonts::embedded());
    let _ = app.add_window();
    let window = app.sole_window();
    // The user's real squeezed window: 1392pt at 2x = 2784 device px.
    let mut narrow = skia_safe::surfaces::raster_n32_premul((2784, 1700)).expect("surface");
    app.draw_window(window, narrow.canvas());

    struct EnterSession;
    impl himark::commands::HimarkCommand for EnterSession {
        fn id(&self) -> &'static str {
            "test.enter-session"
        }
        fn name(&self) -> String {
            String::new()
        }
        fn perform(
            &self,
            store: &mut Store,
            _ui: &imba::ui::UiCtx,
            bound: &imba::focus::Bindings,
            fx: &mut himark::app::AppFx<'_>,
        ) {
        let Some(window) = himark::commands::bound_window(self.id(), bound) else {
            return;
        };
            let target = ahp_wire::SessionId {
                host: ahp_wire::client::HostId::LOCAL,
                session: ahp_wire::client::SessionUri::new("ahp-session:/volatile"),
            };
            himark::app::switch_session(store, window, target, fx)
        }
    }
    assert!(app.perform_command(himark::app::windowed(window, Arc::new(EnterSession))));
    let uri = ahp_wire::client::ChatUri::new("ahp-chat:/volatile");
    let home = ahp_wire::SessionId {
        host: ahp_wire::client::HostId::LOCAL,
        session: ahp_wire::client::SessionUri::new("ahp-session:/volatile"),
    };
    let chats =
        ahp_session::session::state::Hosts::ensure_state(&mut app.store_mut(), &home).chats();
    let panel = ahp_chat::chat::ChatPanel::new(
        app.store(),
        &app.ui_ctx(),
        ahp_wire::client::HostId::LOCAL,
        "ahp-session:/volatile",
        chats,
        uri.clone(),
        ahp_chat::chats::Chats::catalog(app.store(), chats)
            .unwrap_or_else(ahp_chat::chats::Catalog::noop),
    );
    ahp_chat::chats::Chats::put(&mut app.store_mut(), chats, uri.clone(), panel);
    settle(&mut app, &mut narrow);
    assert!(app.perform_registered(window, "chat.composer"));
    settle(&mut app, &mut narrow);

    struct OpenDoc;
    impl himark::commands::HimarkCommand for OpenDoc {
        fn id(&self) -> &'static str {
            "test.open-doc"
        }
        fn name(&self) -> String {
            String::new()
        }
        fn perform(
            &self,
            store: &mut Store,
            ui: &imba::ui::UiCtx,
            bound: &imba::focus::Bindings,
            fx: &mut himark::app::AppFx<'_>,
        ) {
        let Some(window) = himark::commands::bound_window(self.id(), bound) else {
            return;
        };
            let state = himark::workspace::session_state(store, window).expect("state");
            let document = himark::app::markdown_scratch();
            let id = documents::OpenDocuments::register(
                store,
                state.documents(),
                document,
                None,
                "narrow.md".to_owned(),
                0,
            );
            let mut entity = ::workbench::window::Windows::window(store, window).expect("window");
            fx.scope(himark::app::AppCommand::Verb, |fx| {
                entity.show_document(store, &ui, window, id, None, true, fx)
            });
            ::workbench::window::Windows::put(store, window, entity);
        }
    }
    assert!(app.perform_command(himark::app::windowed(window, Arc::new(OpenDoc))));
    settle(&mut app, &mut narrow);

    let image = narrow.image_snapshot();
    let data = image
        .encode(None, skia_safe::EncodedImageFormat::PNG, None)
        .expect("png");
    std::fs::write(format!("{dir}/chat-narrow.png"), data.as_bytes()).expect("write");
}

/// `HIMARK_SHOT=<dir> cargo test -p himark dump_chat_column_screenshot -- --ignored`
#[test]
#[ignore]
fn dump_chat_column_screenshot() {
    let Ok(dir) = std::env::var("HIMARK_SHOT") else {
        return;
    };
    let mut app = Himark::new(AppFonts::embedded());
    let _ = app.add_window();
    // The leftmost header's tabs are the app's registrations: the
    // chat word (the shell's) and the Sessions word (the agent host's).
    app.register_toolbar_button(::workbench::toolbar::composer_button());
    app.register_toolbar_button(::workbench::toolbar::ToolbarButton {
        command: "agent.toggle-agents",
        label: "Sessions",
        order: 0.0,
        side: ::workbench::toolbar::ToolbarSide::Left,
        glyph: std::sync::Arc::new(|_canvas, _rect, _color| {}),
    });
    let window = app.sole_window();
    let mut wide = skia_safe::surfaces::raster_n32_premul((1920, 1080)).expect("surface");
    app.draw_window(window, wide.canvas());

    struct EnterSession;
    impl himark::commands::HimarkCommand for EnterSession {
        fn id(&self) -> &'static str {
            "test.enter-session"
        }
        fn name(&self) -> String {
            String::new()
        }
        fn perform(
            &self,
            store: &mut Store,
            _ui: &imba::ui::UiCtx,
            bound: &imba::focus::Bindings,
            fx: &mut himark::app::AppFx<'_>,
        ) {
        let Some(window) = himark::commands::bound_window(self.id(), bound) else {
            return;
        };
            let target = ahp_wire::SessionId {
                host: ahp_wire::client::HostId::LOCAL,
                session: ahp_wire::client::SessionUri::new("ahp-session:/volatile"),
            };
            himark::app::switch_session(store, window, target, fx)
        }
    }
    assert!(app.perform_command(himark::app::windowed(window, Arc::new(EnterSession))));

    let uri = ahp_wire::client::ChatUri::new("ahp-chat:/volatile");
    let home = ahp_wire::SessionId {
        host: ahp_wire::client::HostId::LOCAL,
        session: ahp_wire::client::SessionUri::new("ahp-session:/volatile"),
    };
    let chats =
        ahp_session::session::state::Hosts::ensure_state(&mut app.store_mut(), &home).chats();
    let panel = ahp_chat::chat::ChatPanel::new(
        app.store(),
        &app.ui_ctx(),
        ahp_wire::client::HostId::LOCAL,
        "ahp-session:/volatile",
        chats,
        uri.clone(),
        ahp_chat::chats::Chats::catalog(app.store(), chats)
            .unwrap_or_else(ahp_chat::chats::Catalog::noop),
    );
    ahp_chat::chats::Chats::put(&mut app.store_mut(), chats, uri.clone(), panel);
    settle(&mut app, &mut wide);
    assert!(app.perform_registered(window, "chat.composer"));
    settle(&mut app, &mut wide);

    let image = wide.image_snapshot();
    let data = image
        .encode(None, skia_safe::EncodedImageFormat::PNG, None)
        .expect("png");
    std::fs::write(format!("{dir}/chat-column.png"), data.as_bytes()).expect("write");
}

/// The transcript in the typographic redesign, seeded like the
/// mock's conversation (docs/Design.md): a collapsed Bash run and a
/// cancelled turn, then your prompt, a call, prose with inline code,
/// a FAILED call with its reason, more prose, and the usage line —
/// over the composer's footer with its options words.
/// `HIMARK_SHOT=<dir> cargo test -p himark-tests --test shell dump_chat_transcript_screenshot -- --ignored`
#[test]
#[ignore]
fn dump_chat_transcript_screenshot() {
    let Ok(dir) = std::env::var("HIMARK_SHOT") else {
        return;
    };
    let mut app = Himark::new(AppFonts::embedded());
    let _ = app.add_window();
    let window = app.sole_window();
    // The cells build off-thread through the AHP build road, with
    // markdown's language — the host's boot wiring, minus the wire.
    let mut languages = ::editor::reparse::SyntaxLanguages::new();
    himark::plugin::Plugin::languages(&himarkdown::MarkdownPlugin, &mut languages);
    app.register_syntax_languages(languages.clone());
    himark::hiahp::install_build_handler(
        &mut app,
        Arc::new(languages),
        Arc::new(::editor::diff::ReplaceAll),
    );
    let (posted, arriving) = std::sync::mpsc::channel();
    let runner = app.attach_host(
        Arc::new(move |command| {
            let _ = posted.send(command);
        }),
        Arc::new(|| {}),
    );
    let mut surface = skia_safe::surfaces::raster_n32_premul((2880, 1800)).expect("surface");
    app.draw_window(window, surface.canvas());

    struct EnterSession;
    impl himark::commands::HimarkCommand for EnterSession {
        fn id(&self) -> &'static str {
            "test.enter-session"
        }
        fn name(&self) -> String {
            String::new()
        }
        fn perform(
            &self,
            store: &mut Store,
            _ui: &imba::ui::UiCtx,
            bound: &imba::focus::Bindings,
            fx: &mut himark::app::AppFx<'_>,
        ) {
            let Some(window) = himark::commands::bound_window(self.id(), bound) else {
                return;
            };
            let target = ahp_wire::SessionId {
                host: ahp_wire::client::HostId::LOCAL,
                session: ahp_wire::client::SessionUri::new("ahp-session:/volatile"),
            };
            himark::app::switch_session(store, window, target, fx)
        }
    }
    assert!(app.perform_command(himark::app::windowed(window, Arc::new(EnterSession))));

    let uri = ahp_wire::client::ChatUri::new("ahp-chat:/volatile");
    let home = ahp_wire::SessionId {
        host: ahp_wire::client::HostId::LOCAL,
        session: ahp_wire::client::SessionUri::new("ahp-session:/volatile"),
    };
    let chats =
        ahp_session::session::state::Hosts::ensure_state(&mut app.store_mut(), &home).chats();
    let panel = ahp_chat::chat::ChatPanel::new(
        app.store(),
        &app.ui_ctx(),
        ahp_wire::client::HostId::LOCAL,
        "ahp-session:/volatile",
        chats,
        uri.clone(),
        ahp_chat::chats::Chats::catalog(app.store(), chats)
            .unwrap_or_else(ahp_chat::chats::Catalog::noop),
    );
    ahp_chat::chats::Chats::put(&mut app.store_mut(), chats, uri.clone(), panel);
    settle(&mut app, &mut surface);
    assert!(app.perform_registered(window, "chat.composer"));
    settle(&mut app, &mut surface);

    let bash = |id: &str, command: &str, ok: bool, output: &str| {
        serde_json::json!({
            "kind": "toolCall",
            "toolCall": {
                "status": "completed",
                "toolCallId": id,
                "toolName": "bash",
                "displayName": "Bash",
                "invocationMessage": format!("Running {command}"),
                "toolInput": command,
                "success": ok,
                "confirmed": "not-needed",
                "pastTenseMessage": format!("Ran {command}"),
                "content": [{ "type": "text", "text": output }],
            }
        })
    };
    let said = |id: &str, text: &str| serde_json::json!({ "kind": "markdown", "id": id, "content": text });
    let user = |text: &str| serde_json::json!({ "text": text, "origin": { "kind": "user" } });
    let mut write = bash(
        "w1",
        "duplicate-resumed-session-races-the-tree.md",
        false,
        "resourceRead ahp-content:/863ab320-0fa3-4e81-86d8-d3f6c89efeff \u{2192} rpc \u{2212}32002: no content",
    );
    write["toolCall"]["displayName"] = "Write".into();
    let state: ahp_types::state::ChatState = serde_json::from_value(serde_json::json!({
        "resource": "ahp-chat:/volatile",
        "title": "hello",
        "status": 0,
        "modifiedAt": "",
        "turns": [
            {
                "id": "t1",
                "message": user("land the islands redesign"),
                "responseParts": [
                    bash("b1", "cargo test -p hikit", true, "ok"),
                    bash("b2", "cargo test -p workbench", true, "ok"),
                    bash("b3", "git stash list", true, "stash@{0}"),
                ],
                "state": "cancelled",
            },
            {
                "id": "t2",
                "message": user("ok. revert everything, it\u{2019}s hopeless"),
                "responseParts": [
                    bash("b4", "git checkout 1addff9c -- . && git stash list", true, ""),
                    said("p1", "Reverted everything cleanly to `1addff9c`; the full islands diff is saved at `/tmp/islands-work-2026-10-23.patch` if you want it back later. The file chaos was caused by a second headless Claude process running the same session in parallel (which I\u{2019}ve now killed), explaining the stray edits and mystery stash."),
                    write,
                    said("p2", "The tree is back to a clean `1addff9c` and I\u{2019}ve noted the duplicate-session failure mode in memory so it gets caught early if it ever happens again."),
                    bash("b5", "git status --porcelain", true, ""),
                    said("p3", "If you want to retry the islands redesign later, the whole thing is one `git apply /tmp/islands-work-2026-10-23.patch` away \u{2014} and with only one session writing this time, the screenshot-iterate loop should actually be sane."),
                ],
                "usage": { "inputTokens": 8, "outputTokens": 2757 },
                "state": "complete",
            },
        ],
    }))
    .expect("the seeded chat state");
    app.perform_command(AppCommand::Verb(imba::command::Verb::at(
        chats,
        ahp_chat::chats::ChatsCommand::Panel(
            uri.clone(),
            ahp_chat::chat::ChatPanelCommand::Snapshot(Ok(state)),
        ),
    )));
    settle(&mut app, &mut surface);
    if let Some(mut panel) = ahp_chat::chats::Chats::chat(app.store(), chats, &uri) {
        panel.seed_toolbar(app.store(), &app.ui_ctx(), ["Fable", "High", "Auto"]);
        ahp_chat::chats::Chats::put(&mut app.store_mut(), chats, uri.clone(), panel);
    }
    // The cells' documents build off the UI thread: give the
    // landings real time to arrive.
    for _ in 0..30 {
        std::thread::sleep(std::time::Duration::from_millis(20));
        runner.run();
        while let Ok(command) = arriving.try_recv() {
            app.perform_batch(vec![command]);
        }
        settle(&mut app, &mut surface);
    }

    let image = surface.image_snapshot();
    let data = image
        .encode(None, skia_safe::EncodedImageFormat::PNG, None)
        .expect("png");
    std::fs::write(format!("{dir}/chat-transcript.png"), data.as_bytes()).expect("write");
}

/// The single-panel presentation: Cmd-I fronts the hidden chat
/// over the panel, opening a panel hands the window back, and
/// Cmd-W closes the panel — a hidden chat's stale focus must
/// never block it. Closing the last panel returns the chat.
#[test]
fn single_panel_cmd_i_fronts_and_cmd_w_closes() {
    let mut app = Himark::new(AppFonts::embedded());
    let _ = app.add_window();
    let window = app.sole_window();
    let mut narrow = skia_safe::surfaces::raster_n32_premul((800, 600)).expect("surface");
    app.draw_window(window, narrow.canvas());

    struct EnterSession;
    impl himark::commands::HimarkCommand for EnterSession {
        fn id(&self) -> &'static str {
            "test.enter-session"
        }
        fn name(&self) -> String {
            String::new()
        }
        fn perform(
            &self,
            store: &mut Store,
            _ui: &imba::ui::UiCtx,
            bound: &imba::focus::Bindings,
            fx: &mut himark::app::AppFx<'_>,
        ) {
        let Some(window) = himark::commands::bound_window(self.id(), bound) else {
            return;
        };
            let target = ahp_wire::SessionId {
                host: ahp_wire::client::HostId::LOCAL,
                session: ahp_wire::client::SessionUri::new("ahp-session:/volatile"),
            };
            himark::app::switch_session(store, window, target, fx)
        }
    }
    assert!(app.perform_command(himark::app::windowed(window, Arc::new(EnterSession))));
    let uri = ahp_wire::client::ChatUri::new("ahp-chat:/volatile");
    let home = ahp_wire::SessionId {
        host: ahp_wire::client::HostId::LOCAL,
        session: ahp_wire::client::SessionUri::new("ahp-session:/volatile"),
    };
    let chats =
        ahp_session::session::state::Hosts::ensure_state(&mut app.store_mut(), &home).chats();
    let panel = ahp_chat::chat::ChatPanel::new(
        app.store(),
        &app.ui_ctx(),
        ahp_wire::client::HostId::LOCAL,
        "ahp-session:/volatile",
        chats,
        uri.clone(),
        ahp_chat::chats::Chats::catalog(app.store(), chats)
            .unwrap_or_else(ahp_chat::chats::Catalog::noop),
    );
    ahp_chat::chats::Chats::put(&mut app.store_mut(), chats, uri.clone(), panel);
    settle(&mut app, &mut narrow);

    fn held(app: &himark::app::Himark) -> &::workbench::window::Window {
        ::workbench::window::Windows::window_ref(app.store(), app.sole_window()).expect("window")
    }

    assert!(app.perform_registered(window, "chat.composer"));
    settle(&mut app, &mut narrow);
    assert!(held(&app).workbench().chat().is_some());
    assert!(
        !held(&app).workbench().chat_fronted(),
        "a vacant tree shows the chat without fronting"
    );

    struct OpenDoc;
    impl himark::commands::HimarkCommand for OpenDoc {
        fn id(&self) -> &'static str {
            "test.open-doc"
        }
        fn name(&self) -> String {
            String::new()
        }
        fn perform(
            &self,
            store: &mut Store,
            ui: &imba::ui::UiCtx,
            bound: &imba::focus::Bindings,
            fx: &mut himark::app::AppFx<'_>,
        ) {
        let Some(window) = himark::commands::bound_window(self.id(), bound) else {
            return;
        };
            let state = himark::workspace::session_state(store, window).expect("state");
            let id = documents::OpenDocuments::register(
                store,
                state.documents(),
                himark::app::markdown_scratch(),
                None,
                "doc".to_owned(),
                0,
            );
            let mut entity = ::workbench::window::Windows::window(store, window).expect("window");
            fx.scope(himark::app::AppCommand::Verb, |fx| {
                entity.show_document(store, &ui, window, id, None, true, fx)
            });
            ::workbench::window::Windows::put(store, window, entity);
        }
    }
    assert!(app.perform_command(himark::app::windowed(window, Arc::new(OpenDoc))));
    settle(&mut app, &mut narrow);
    assert!(!held(&app).workbench().root.is_vacant());
    assert!(
        !held(&app).workbench().chat_focused(),
        "the panel took the window and the keyboard"
    );

    // Cmd-I over the single panel FRONTS the chat.
    assert!(app.perform_registered(window, "chat.composer"));
    settle(&mut app, &mut narrow);
    assert!(
        held(&app).workbench().chat_fronted(),
        "\u{2318}I shows the hidden chat"
    );

    // Opening a panel hands the window back.
    assert!(app.perform_command(himark::app::windowed(window, Arc::new(OpenDoc))));
    settle(&mut app, &mut narrow);
    assert!(
        !held(&app).workbench().chat_fronted(),
        "a landing panel wins the window back"
    );

    // A STALE chat focus while the chat is hidden must not block
    // closing the panel.
    {
        let mut entity = ::workbench::window::Windows::window(app.store(), window).expect("window");
        entity.workbench_mut().focus_chat(true);
        ::workbench::window::Windows::put(&mut app.store_mut(), window, entity);
    }
    assert!(app.perform_registered(window, "workbench.close"));
    settle(&mut app, &mut narrow);
    assert!(
        held(&app).workbench().root.is_vacant(),
        "\u{2318}W closed the panel despite the hidden chat's focus"
    );
}

/// The tree header's MAXIMIZE hides the chat; \u{2318}I and the
/// minimize button bring it back.
#[test]
fn maximize_hides_the_chat_and_restore_brings_it_back() {
    let mut app = Himark::new(AppFonts::embedded());
    let _ = app.add_window();
    let window = app.sole_window();
    let mut wide = skia_safe::surfaces::raster_n32_premul((3200, 1000)).expect("surface");
    app.draw_window(window, wide.canvas());

    struct EnterSession;
    impl himark::commands::HimarkCommand for EnterSession {
        fn id(&self) -> &'static str {
            "test.enter-session"
        }
        fn name(&self) -> String {
            String::new()
        }
        fn perform(
            &self,
            store: &mut Store,
            _ui: &imba::ui::UiCtx,
            bound: &imba::focus::Bindings,
            fx: &mut himark::app::AppFx<'_>,
        ) {
        let Some(window) = himark::commands::bound_window(self.id(), bound) else {
            return;
        };
            let target = ahp_wire::SessionId {
                host: ahp_wire::client::HostId::LOCAL,
                session: ahp_wire::client::SessionUri::new("ahp-session:/volatile"),
            };
            himark::app::switch_session(store, window, target, fx)
        }
    }
    assert!(app.perform_command(himark::app::windowed(window, Arc::new(EnterSession))));
    let uri = ahp_wire::client::ChatUri::new("ahp-chat:/volatile");
    let home = ahp_wire::SessionId {
        host: ahp_wire::client::HostId::LOCAL,
        session: ahp_wire::client::SessionUri::new("ahp-session:/volatile"),
    };
    let chats =
        ahp_session::session::state::Hosts::ensure_state(&mut app.store_mut(), &home).chats();
    let panel = ahp_chat::chat::ChatPanel::new(
        app.store(),
        &app.ui_ctx(),
        ahp_wire::client::HostId::LOCAL,
        "ahp-session:/volatile",
        chats,
        uri.clone(),
        ahp_chat::chats::Chats::catalog(app.store(), chats)
            .unwrap_or_else(ahp_chat::chats::Catalog::noop),
    );
    ahp_chat::chats::Chats::put(&mut app.store_mut(), chats, uri.clone(), panel);
    settle(&mut app, &mut wide);
    assert!(app.perform_registered(window, "chat.composer"));
    settle(&mut app, &mut wide);

    fn held(app: &himark::app::Himark) -> &::workbench::window::Window {
        ::workbench::window::Windows::window_ref(app.store(), app.sole_window()).expect("window")
    }

    assert!(app.perform_command(himark::app::content(
        window,
        ::workbench::window::WindowCommand::Base(
            ::workbench::workbench::WorkbenchCommand::MaximizeTree
        ),
    )));
    settle(&mut app, &mut wide);
    assert!(
        held(&app).workbench().chat_minimized(),
        "maximize hides the chat"
    );

    assert!(app.perform_registered(window, "chat.composer"));
    settle(&mut app, &mut wide);
    assert!(
        !held(&app).workbench().chat_minimized(),
        "\u{2318}I brings the chat back"
    );

    assert!(app.perform_command(himark::app::content(
        window,
        ::workbench::window::WindowCommand::Base(
            ::workbench::workbench::WorkbenchCommand::MaximizeTree
        ),
    )));
    settle(&mut app, &mut wide);
    assert!(held(&app).workbench().chat_minimized());
    assert!(app.perform_command(himark::app::content(
        window,
        ::workbench::window::WindowCommand::Base(
            ::workbench::workbench::WorkbenchCommand::RestoreChat
        ),
    )));
    settle(&mut app, &mut wide);
    assert!(
        !held(&app).workbench().chat_minimized(),
        "the minimize button brings the chat back"
    );
}

/// EVERY open road converges on the chat slot: a chat pane sent
/// down the generic `open_panel` lands there, never in the tree.
#[test]
fn any_open_road_lands_the_chat_in_the_slot() {
    let mut app = Himark::new(AppFonts::embedded());
    let _ = app.add_window();
    let window = app.sole_window();
    let mut surface = skia_safe::surfaces::raster_n32_premul((800, 600)).expect("surface");
    app.draw_window(window, surface.canvas());

    let uri = ahp_wire::client::ChatUri::new("ahp-chat:/walkable");
    let home = ahp_wire::SessionId {
        host: ahp_wire::client::HostId::LOCAL,
        session: ahp_wire::client::SessionUri::new("ahp-session:/walkable"),
    };
    let chats =
        ahp_session::session::state::Hosts::ensure_state(&mut app.store_mut(), &home).chats();
    let panel = ahp_chat::chat::ChatPanel::new(
        app.store(),
        &app.ui_ctx(),
        ahp_wire::client::HostId::LOCAL,
        "ahp-session:/walkable",
        chats,
        uri.clone(),
        ahp_chat::chats::Chats::catalog(app.store(), chats)
            .unwrap_or_else(ahp_chat::chats::Catalog::noop),
    );
    ahp_chat::chats::Chats::put(&mut app.store_mut(), chats, uri.clone(), panel);
    {
        let ui = app.ui_ctx();
        let mut store = app.store_mut();
        let mut entity = ::workbench::window::Windows::window(&store, window).expect("window");
        let mut batch = imba::effect::Batch::<himark::app::AppCommand>::new();
        let _ = entity.open_panel(
            &mut store,
            &ui,
            Box::new(ahp_chat::chats::ChatPane::new(chats, uri.clone())),
            &mut batch.effects(),
        );
        ::workbench::window::Windows::put(&mut store, window, entity);
    }
    settle(&mut app, &mut surface);

    let entity = ::workbench::window::Windows::window_ref(app.store(), window).expect("window");
    assert!(
        entity.workbench().chat().is_some(),
        "the generic road landed the chat in the slot"
    );
    let mut in_tree = false;
    entity.workbench().root.for_each_pane(&mut |panel| {
        if let ::workbench::workbench_node::Panel::Plugin(view) = panel {
            in_tree |= view.as_any().is::<ahp_chat::chats::ChatPane>();
        }
    });
    assert!(!in_tree, "the chat never enters the tree");
}

#[test]
fn new_session_without_a_host_keeps_the_current_chat() {
    let mut app = Himark::new(AppFonts::embedded());
    let _ = app.add_window();
    let window = app.sole_window();
    let mut surface = skia_safe::surfaces::raster_n32_premul((800, 600)).expect("surface");
    app.draw_window(window, surface.canvas());

    struct EnterSession;
    impl himark::commands::HimarkCommand for EnterSession {
        fn id(&self) -> &'static str {
            "test.enter-session"
        }
        fn name(&self) -> String {
            String::new()
        }
        fn perform(
            &self,
            store: &mut Store,
            _ui: &imba::ui::UiCtx,
            bound: &imba::focus::Bindings,
            fx: &mut himark::app::AppFx<'_>,
        ) {
        let Some(window) = himark::commands::bound_window(self.id(), bound) else {
            return;
        };
            let target = ahp_wire::SessionId {
                host: ahp_wire::client::HostId::LOCAL,
                session: ahp_wire::client::SessionUri::new("ahp-session:/live"),
            };
            himark::app::switch_session(store, window, target, fx)
        }
    }
    assert!(app.perform_command(himark::app::windowed(window, Arc::new(EnterSession))));
    let uri = ahp_wire::client::ChatUri::new("ahp-chat:/live");
    let home = ahp_wire::SessionId {
        host: ahp_wire::client::HostId::LOCAL,
        session: ahp_wire::client::SessionUri::new("ahp-session:/live"),
    };
    let chats =
        ahp_session::session::state::Hosts::ensure_state(&mut app.store_mut(), &home).chats();
    let panel = ahp_chat::chat::ChatPanel::new(
        app.store(),
        &app.ui_ctx(),
        ahp_wire::client::HostId::LOCAL,
        "ahp-session:/live",
        chats,
        uri.clone(),
        ahp_chat::chats::Chats::catalog(app.store(), chats)
            .unwrap_or_else(ahp_chat::chats::Catalog::noop),
    );
    ahp_chat::chats::Chats::put(&mut app.store_mut(), chats, uri.clone(), panel);
    {
        let ui = app.ui_ctx();
        let mut store = app.store_mut();
        let mut entity = ::workbench::window::Windows::window(&store, window).expect("window");
        let mut batch = imba::effect::Batch::<himark::app::AppCommand>::new();
        let _ = entity.open_panel(
            &mut store,
            &ui,
            Box::new(ahp_chat::chats::ChatPane::new(chats, uri)),
            &mut batch.effects(),
        );
        ::workbench::window::Windows::put(&mut store, window, entity);
    }
    settle(&mut app, &mut surface);
    let chat_mounted = |app: &himark::app::Himark| -> bool {
        ::workbench::window::Windows::window_ref(app.store(), app.sole_window())
            .expect("window")
            .workbench()
            .chat()
            .is_some_and(|chat| match chat.panel() {
                ::workbench::workbench_node::Panel::Plugin(view) => {
                    view.as_any().is::<ahp_chat::chats::ChatPane>()
                }
                ::workbench::workbench_node::Panel::Editor(_) => false,
            })
    };
    let entity = ::workbench::window::Windows::window_ref(app.store(), window).expect("window");
    assert!(himark::workspace::entity_session(&entity).names_session());
    assert!(chat_mounted(&app), "the chat panel stands");

    // `session.new` creates a REAL session on a host now (the
    // composer is gone); with no host registered there is nothing to
    // create on, so the command is a no-op and the current session
    // keeps its chat. The live road is covered end-to-end in
    // frontend-host (a_new_session_is_a_live_chat_from_the_first_frame).
    let _ = app.perform_command(himark::app::windowed(
        window,
        Arc::new(himark::higent::open_session::OpenNewSession { host: None }),
    ));
    settle(&mut app, &mut surface);
    let entity = ::workbench::window::Windows::window_ref(app.store(), window).expect("window");
    assert!(
        himark::workspace::entity_session(&entity).names_session(),
        "no host: the current session stays"
    );
    assert!(chat_mounted(&app), "no host: the chat panel stays");
}

/// The Chat TAB toggles the column through the real press path:
/// standing, a click rolls it away; gone, a click brings it back.
#[test]
fn the_chat_tab_toggles_the_column() {
    let mut app = Himark::new(AppFonts::embedded());
    let _ = app.add_window();
    app.register_toolbar_button(::workbench::toolbar::composer_button());
    app.register_toolbar_button(::workbench::toolbar::ToolbarButton {
        command: "agent.toggle-agents",
        label: "Sessions",
        order: 0.0,
        side: ::workbench::toolbar::ToolbarSide::Left,
        glyph: std::sync::Arc::new(|_canvas, _rect, _color| {}),
    });
    let window = app.sole_window();
    let (width, height) = (3200.0f32, 1000.0f32);
    let mut wide =
        skia_safe::surfaces::raster_n32_premul((width as i32, height as i32)).expect("surface");
    app.draw_window(window, wide.canvas());

    struct EnterSession;
    impl himark::commands::HimarkCommand for EnterSession {
        fn id(&self) -> &'static str {
            "test.enter-session"
        }
        fn name(&self) -> String {
            String::new()
        }
        fn perform(
            &self,
            store: &mut Store,
            _ui: &imba::ui::UiCtx,
            bound: &imba::focus::Bindings,
            fx: &mut himark::app::AppFx<'_>,
        ) {
            let Some(window) = himark::commands::bound_window(self.id(), bound) else {
                return;
            };
            let target = ahp_wire::SessionId {
                host: ahp_wire::client::HostId::LOCAL,
                session: ahp_wire::client::SessionUri::new("ahp-session:/volatile"),
            };
            himark::app::switch_session(store, window, target, fx)
        }
    }
    assert!(app.perform_command(himark::app::windowed(window, Arc::new(EnterSession))));
    let uri = ahp_wire::client::ChatUri::new("ahp-chat:/volatile");
    let home = ahp_wire::SessionId {
        host: ahp_wire::client::HostId::LOCAL,
        session: ahp_wire::client::SessionUri::new("ahp-session:/volatile"),
    };
    let chats =
        ahp_session::session::state::Hosts::ensure_state(&mut app.store_mut(), &home).chats();
    let panel = ahp_chat::chat::ChatPanel::new(
        app.store(),
        &app.ui_ctx(),
        ahp_wire::client::HostId::LOCAL,
        "ahp-session:/volatile",
        chats,
        uri.clone(),
        ahp_chat::chats::Chats::catalog(app.store(), chats)
            .unwrap_or_else(ahp_chat::chats::Catalog::noop),
    );
    ahp_chat::chats::Chats::put(&mut app.store_mut(), chats, uri.clone(), panel);
    settle(&mut app, &mut wide);
    assert!(app.perform_registered(window, "chat.composer"));
    settle(&mut app, &mut wide);

    fn held(app: &himark::app::Himark) -> &::workbench::window::Window {
        ::workbench::window::Windows::window_ref(app.store(), app.sole_window()).expect("window")
    }
    assert!(!held(&app).workbench().chat_minimized(), "the chat stands");

    // The Chat tab's center: after the Sessions tab and a tab gap,
    // from the leftmost header's lead inset.
    let chrome = ::editor::env::Themes::of(app.store()).part(::hikit::chrome::ToolbarChrome::KEY);
    let styles = hikit::header::HeaderStyles::of(app.store(), &app.ui_ctx());
    let y = chrome.title_baseline - chrome.title_size * 0.4;
    // A tab's slot is its title-rank width whatever rank it shows
    // in; the word sits at the slot's start.
    let sessions = styles.tab_width("Sessions");
    let chat_standing = styles.title.shaper.advance(&styles.title.font, "Chat");
    let chat_dim = styles.dim.shaper.advance(&styles.dim.font, "Chat");
    let lead = chrome.lead_inset;

    let x = lead + sessions + chrome.tab_gap + chat_standing * 0.5;
    assert!(test_driver::click(&mut app, x, y, width, height), "the standing Chat tab takes the press");
    settle(&mut app, &mut wide);
    assert!(
        held(&app).workbench().chat_minimized(),
        "the Chat tab rolls the standing column away"
    );

    // Gone, the tree is leftmost and carries the tabs from the
    // greater of the lead inset and the code text edge.
    let window_chrome = ::editor::env::Themes::of(app.store()).part(::editor::theme::WindowChrome::KEY);
    let lead = chrome.lead_inset.max(window_chrome.code_text_edge);
    let x = lead + sessions + chrome.tab_gap + chat_dim * 0.5;
    assert!(test_driver::click(&mut app, x, y, width, height), "the dim Chat tab takes the press");
    settle(&mut app, &mut wide);
    assert!(
        !held(&app).workbench().chat_minimized(),
        "the Chat tab brings the column back"
    );
}

/// A window too narrow for both columns, a file open: the chat
/// hides and the Chat tab brings it over the panels.
#[test]
fn the_chat_tab_fronts_the_chat_in_a_narrow_window() {
    let mut app = Himark::new(AppFonts::embedded());
    let _ = app.add_window();
    app.register_toolbar_button(::workbench::toolbar::composer_button());
    app.register_toolbar_button(::workbench::toolbar::ToolbarButton {
        command: "agent.toggle-agents",
        label: "Sessions",
        order: 0.0,
        side: ::workbench::toolbar::ToolbarSide::Left,
        glyph: std::sync::Arc::new(|_canvas, _rect, _color| {}),
    });
    let window = app.sole_window();
    let (width, height) = (1600.0f32, 900.0f32);
    let mut narrow =
        skia_safe::surfaces::raster_n32_premul((width as i32, height as i32)).expect("surface");
    app.draw_window(window, narrow.canvas());

    struct EnterSession;
    impl himark::commands::HimarkCommand for EnterSession {
        fn id(&self) -> &'static str {
            "test.enter-session"
        }
        fn name(&self) -> String {
            String::new()
        }
        fn perform(
            &self,
            store: &mut Store,
            _ui: &imba::ui::UiCtx,
            bound: &imba::focus::Bindings,
            fx: &mut himark::app::AppFx<'_>,
        ) {
            let Some(window) = himark::commands::bound_window(self.id(), bound) else {
                return;
            };
            let target = ahp_wire::SessionId {
                host: ahp_wire::client::HostId::LOCAL,
                session: ahp_wire::client::SessionUri::new("ahp-session:/volatile"),
            };
            himark::app::switch_session(store, window, target, fx)
        }
    }
    assert!(app.perform_command(himark::app::windowed(window, Arc::new(EnterSession))));
    let uri = ahp_wire::client::ChatUri::new("ahp-chat:/volatile");
    let home = ahp_wire::SessionId {
        host: ahp_wire::client::HostId::LOCAL,
        session: ahp_wire::client::SessionUri::new("ahp-session:/volatile"),
    };
    let chats =
        ahp_session::session::state::Hosts::ensure_state(&mut app.store_mut(), &home).chats();
    let panel = ahp_chat::chat::ChatPanel::new(
        app.store(),
        &app.ui_ctx(),
        ahp_wire::client::HostId::LOCAL,
        "ahp-session:/volatile",
        chats,
        uri.clone(),
        ahp_chat::chats::Chats::catalog(app.store(), chats)
            .unwrap_or_else(ahp_chat::chats::Catalog::noop),
    );
    ahp_chat::chats::Chats::put(&mut app.store_mut(), chats, uri.clone(), panel);
    settle(&mut app, &mut narrow);
    assert!(app.perform_registered(window, "chat.composer"));
    settle(&mut app, &mut narrow);

    // A file lands in the tree: too narrow for both, the tree alone.
    assert!(app.new_scratch(window));
    settle(&mut app, &mut narrow);

    fn held(app: &himark::app::Himark) -> &::workbench::window::Window {
        ::workbench::window::Windows::window_ref(app.store(), app.sole_window()).expect("window")
    }
    let window_chrome = ::editor::env::Themes::of(app.store()).part(::editor::theme::WindowChrome::KEY);
    assert!(
        width < window_chrome.chat_min_width + window_chrome.first_pane_width.max(window_chrome.main_min_width),
        "the window is too narrow for both columns"
    );
    assert!(!held(&app).workbench().chat_fronted(), "the panel took the window");

    let chrome = ::editor::env::Themes::of(app.store()).part(::hikit::chrome::ToolbarChrome::KEY);
    let styles = hikit::header::HeaderStyles::of(app.store(), &app.ui_ctx());
    let y = chrome.title_baseline - chrome.title_size * 0.4;
    let sessions = styles.tab_width("Sessions");
    let chat_dim = styles.dim.shaper.advance(&styles.dim.font, "Chat");
    let chat_standing = styles.title.shaper.advance(&styles.title.font, "Chat");
    let lead = chrome.lead_inset.max(window_chrome.code_text_edge);
    let x = lead + sessions + chrome.tab_gap + chat_dim * 0.5;
    assert!(test_driver::click(&mut app, x, y, width, height), "the dim Chat tab takes the press");
    settle(&mut app, &mut narrow);
    assert!(
        held(&app).workbench().chat_fronted(),
        "the Chat tab fronts the chat over the panels"
    );

    // Standing over the panels, the tab rolls it away again.
    let x = chrome.lead_inset + sessions + chrome.tab_gap + chat_standing * 0.5;
    assert!(test_driver::click(&mut app, x, y, width, height), "the standing Chat tab takes the press");
    settle(&mut app, &mut narrow);
    assert!(!held(&app).workbench().chat_fronted(), "the tab rolls the chat away");
    assert!(held(&app).workbench().chat_minimized(), "the roll-away lands as minimized");

    let x = lead + sessions + chrome.tab_gap + chat_dim * 0.5;
    assert!(test_driver::click(&mut app, x, y, width, height));
    settle(&mut app, &mut narrow);
    assert!(held(&app).workbench().chat_fronted(), "and the tab brings it back once more");
}
