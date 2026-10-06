#![allow(unused_imports)]
use super::dock::{entity, located, settle, show_dock};
use super::*;
use hikit::modal::ModalRequest;
use hikit::modal::ModalView;
use himark::app::AppCommand;
use himark::app::AppFonts;
use himark::app::Application;
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
    impl himark::commands::WindowedCommand for Switch {
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
    fn mounted_chat(app: &himark::app::Application) -> Option<String> {
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
    let mut app = Application::new(AppFonts::embedded());
    let _ = app.add_window();
    let window = app.sole_window();
    let mut surface = skia_safe::surfaces::raster_n32_premul((800, 600)).expect("surface");
    app.draw_window(window, surface.canvas());

    struct EnterSession;
    impl himark::commands::WindowedCommand for EnterSession {
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
            window: ::workbench::window::WindowId,
            fx: &mut himark::app::AppFx<'_>,
        ) {
            let target = ahp_wire::SessionId {
                host: ahp_wire::client::HostId::LOCAL,
                session: ahp_wire::client::SessionUri::new("ahp-session:/volatile"),
            };
            himark::app::switch_session(store, window, target, fx)
        }
    }
    assert!(app.perform_command(AppCommand::Windowed(window, Arc::new(EnterSession))));

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

    let slot_chat = |app: &himark::app::Application| -> Option<String> {
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
    let tree_chat = |app: &himark::app::Application| -> Option<String> {
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
    let with_pane = |app: &mut himark::app::Application,
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
    let mut app = Application::new(AppFonts::embedded());
    let _ = app.add_window();
    let window = app.sole_window();
    // House units are 2x pixels: 3200 here is a 1600pt window.
    let mut wide = skia_safe::surfaces::raster_n32_premul((3200, 1000)).expect("surface");
    app.draw_window(window, wide.canvas());

    struct EnterSession;
    impl himark::commands::WindowedCommand for EnterSession {
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
            window: ::workbench::window::WindowId,
            fx: &mut himark::app::AppFx<'_>,
        ) {
            let target = ahp_wire::SessionId {
                host: ahp_wire::client::HostId::LOCAL,
                session: ahp_wire::client::SessionUri::new("ahp-session:/volatile"),
            };
            himark::app::switch_session(store, window, target, fx)
        }
    }
    assert!(app.perform_command(AppCommand::Windowed(window, Arc::new(EnterSession))));

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

    fn held(app: &himark::app::Application) -> &::workbench::window::Window {
        ::workbench::window::Windows::window_ref(app.store(), app.sole_window()).expect("window")
    }
    let slot_filled =
        |app: &himark::app::Application| -> bool { held(app).workbench().chat().is_some() };
    let tree_chat = |app: &himark::app::Application| -> Option<String> {
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
    let vacant =
        |app: &himark::app::Application| -> bool { held(app).workbench().root.is_vacant() };

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
    impl himark::commands::WindowedCommand for OpenDoc {
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
            window: ::workbench::window::WindowId,
            fx: &mut himark::app::AppFx<'_>,
        ) {
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
    assert!(app.perform_command(AppCommand::Windowed(window, Arc::new(OpenDoc))));
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
    let mut app = Application::new(AppFonts::embedded());
    let _ = app.add_window();
    let window = app.sole_window();
    // The user's real squeezed window: 1392pt at 2x = 2784 device px.
    let mut narrow = skia_safe::surfaces::raster_n32_premul((2784, 1700)).expect("surface");
    app.draw_window(window, narrow.canvas());

    struct EnterSession;
    impl himark::commands::WindowedCommand for EnterSession {
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
            window: ::workbench::window::WindowId,
            fx: &mut himark::app::AppFx<'_>,
        ) {
            let target = ahp_wire::SessionId {
                host: ahp_wire::client::HostId::LOCAL,
                session: ahp_wire::client::SessionUri::new("ahp-session:/volatile"),
            };
            himark::app::switch_session(store, window, target, fx)
        }
    }
    assert!(app.perform_command(AppCommand::Windowed(window, Arc::new(EnterSession))));
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
    impl himark::commands::WindowedCommand for OpenDoc {
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
            window: ::workbench::window::WindowId,
            fx: &mut himark::app::AppFx<'_>,
        ) {
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
    assert!(app.perform_command(AppCommand::Windowed(window, Arc::new(OpenDoc))));
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
    let mut app = Application::new(AppFonts::embedded());
    let _ = app.add_window();
    let window = app.sole_window();
    let mut wide = skia_safe::surfaces::raster_n32_premul((1920, 1080)).expect("surface");
    app.draw_window(window, wide.canvas());

    struct EnterSession;
    impl himark::commands::WindowedCommand for EnterSession {
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
            window: ::workbench::window::WindowId,
            fx: &mut himark::app::AppFx<'_>,
        ) {
            let target = ahp_wire::SessionId {
                host: ahp_wire::client::HostId::LOCAL,
                session: ahp_wire::client::SessionUri::new("ahp-session:/volatile"),
            };
            himark::app::switch_session(store, window, target, fx)
        }
    }
    assert!(app.perform_command(AppCommand::Windowed(window, Arc::new(EnterSession))));

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

/// The single-panel presentation: Cmd-I fronts the hidden chat
/// over the panel, opening a panel hands the window back, and
/// Cmd-W closes the panel — a hidden chat's stale focus must
/// never block it. Closing the last panel returns the chat.
#[test]
fn single_panel_cmd_i_fronts_and_cmd_w_closes() {
    let mut app = Application::new(AppFonts::embedded());
    let _ = app.add_window();
    let window = app.sole_window();
    let mut narrow = skia_safe::surfaces::raster_n32_premul((800, 600)).expect("surface");
    app.draw_window(window, narrow.canvas());

    struct EnterSession;
    impl himark::commands::WindowedCommand for EnterSession {
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
            window: ::workbench::window::WindowId,
            fx: &mut himark::app::AppFx<'_>,
        ) {
            let target = ahp_wire::SessionId {
                host: ahp_wire::client::HostId::LOCAL,
                session: ahp_wire::client::SessionUri::new("ahp-session:/volatile"),
            };
            himark::app::switch_session(store, window, target, fx)
        }
    }
    assert!(app.perform_command(AppCommand::Windowed(window, Arc::new(EnterSession))));
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

    fn held(app: &himark::app::Application) -> &::workbench::window::Window {
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
    impl himark::commands::WindowedCommand for OpenDoc {
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
            window: ::workbench::window::WindowId,
            fx: &mut himark::app::AppFx<'_>,
        ) {
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
    assert!(app.perform_command(AppCommand::Windowed(window, Arc::new(OpenDoc))));
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
    assert!(app.perform_command(AppCommand::Windowed(window, Arc::new(OpenDoc))));
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
    let mut app = Application::new(AppFonts::embedded());
    let _ = app.add_window();
    let window = app.sole_window();
    let mut wide = skia_safe::surfaces::raster_n32_premul((3200, 1000)).expect("surface");
    app.draw_window(window, wide.canvas());

    struct EnterSession;
    impl himark::commands::WindowedCommand for EnterSession {
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
            window: ::workbench::window::WindowId,
            fx: &mut himark::app::AppFx<'_>,
        ) {
            let target = ahp_wire::SessionId {
                host: ahp_wire::client::HostId::LOCAL,
                session: ahp_wire::client::SessionUri::new("ahp-session:/volatile"),
            };
            himark::app::switch_session(store, window, target, fx)
        }
    }
    assert!(app.perform_command(AppCommand::Windowed(window, Arc::new(EnterSession))));
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

    fn held(app: &himark::app::Application) -> &::workbench::window::Window {
        ::workbench::window::Windows::window_ref(app.store(), app.sole_window()).expect("window")
    }

    assert!(app.perform_command(AppCommand::Content(
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

    assert!(app.perform_command(AppCommand::Content(
        window,
        ::workbench::window::WindowCommand::Base(
            ::workbench::workbench::WorkbenchCommand::MaximizeTree
        ),
    )));
    settle(&mut app, &mut wide);
    assert!(held(&app).workbench().chat_minimized());
    assert!(app.perform_command(AppCommand::Content(
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
    let mut app = Application::new(AppFonts::embedded());
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
fn new_session_leaves_the_previous_session_and_its_chat() {
    let mut app = Application::new(AppFonts::embedded());
    let _ = app.add_window();
    let window = app.sole_window();
    let mut surface = skia_safe::surfaces::raster_n32_premul((800, 600)).expect("surface");
    app.draw_window(window, surface.canvas());

    struct EnterSession;
    impl himark::commands::WindowedCommand for EnterSession {
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
            window: ::workbench::window::WindowId,
            fx: &mut himark::app::AppFx<'_>,
        ) {
            let target = ahp_wire::SessionId {
                host: ahp_wire::client::HostId::LOCAL,
                session: ahp_wire::client::SessionUri::new("ahp-session:/live"),
            };
            himark::app::switch_session(store, window, target, fx)
        }
    }
    assert!(app.perform_command(AppCommand::Windowed(window, Arc::new(EnterSession))));
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
    let chat_mounted = |app: &himark::app::Application| -> bool {
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

    assert!(app.perform_command(AppCommand::Windowed(
        window,
        Arc::new(himark::new_session::OpenNewSession { host: None }),
    )));
    settle(&mut app, &mut surface);
    let entity = ::workbench::window::Windows::window_ref(app.store(), window).expect("window");
    assert!(
        !himark::workspace::entity_session(&entity).names_session(),
        "the previous session is still current: {:?}",
        himark::workspace::entity_session(&entity)
    );
    assert!(
        !chat_mounted(&app),
        "the previous session's chat panel is still mounted"
    );
    let title = ::workbench::window::Windows::window_ref(app.store(), window)
        .expect("window")
        .workbench()
        .root
        .focused_pane()
        .title(app.store());
    assert_eq!(title, "New session", "the composer holds the focus");
}
