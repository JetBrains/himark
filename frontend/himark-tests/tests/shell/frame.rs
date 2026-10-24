// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0
//! The window FRAME's screenshot: every column with its header —
//! the chat, the tree, the side column — rendered at the design's
//! size (1440×900 at 2x) for a look at the chrome itself.

use std::sync::Arc;

use imba::store::Store;

use himark::app::AppFonts;
use himark::app::Himark;
use himark::app_ext::AppExt as _;

/// `HIMARK_SHOT=<dir> cargo test -p himark-tests --test shell frame::dump_frame_screenshot -- --ignored`
#[test]
#[ignore]
fn dump_frame_screenshot() {
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
    super::dock::settle(&mut app, &mut surface);
    assert!(app.perform_registered(window, "chat.composer"));
    super::dock::settle(&mut app, &mut surface);

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
            let location = editor::location::ResourceLocation::new(
                editor::location::ResourceType::document(),
                editor::location::Authority::new("local"),
                vec![
                    "backend".to_owned(),
                    "agent-host".to_owned(),
                    "src".to_owned(),
                    "server.rs".to_owned(),
                ],
            );
            let id = documents::OpenDocuments::register(
                store,
                state.documents(),
                document,
                Some(location),
                "server.rs".to_owned(),
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
    super::dock::settle(&mut app, &mut surface);
    // The side column's tabs, as the shell registers them.
    for (order, (label, command)) in [
        ("History", "history.view"),
        ("Files", "files.tree"),
        ("Find", "search.view"),
        ("Changes", "changes.view"),
        ("Comments", "comments.view"),
    ]
    .into_iter()
    .enumerate()
    {
        app.register_toolbar_button(::workbench::toolbar::ToolbarButton {
            command,
            label,
            order: order as f32,
            side: ::workbench::toolbar::ToolbarSide::Right,
            glyph: Arc::new(|_canvas, _rect, _color| {}),
        });
    }
    super::dock::show_dock(&mut app, "files", "test.files");
    super::dock::settle(&mut app, &mut surface);

    let image = surface.image_snapshot();
    let data = image
        .encode(None, skia_safe::EncodedImageFormat::PNG, None)
        .expect("png");
    std::fs::write(format!("{dir}/frame.png"), data.as_bytes()).expect("write");
}
