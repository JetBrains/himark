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

/// The sessions subscription is RESIDENT: a registered host connects,
/// lists its sessions and drains its event stream off the batch tail
/// alone — no drawer, no composer, no view of any kind is open here.
#[test]
fn the_sessions_subscription_runs_without_any_view() {
    struct Seat;
    macro_rules! off_road {
        ($($name:ident($($arg:ident: $ty:ty),*) -> $out:ty;)*) => {
            $(fn $name(&self, $($arg: $ty),*) -> $out {
                $(let _ = $arg;)*
                unreachable!("the subscription never takes this road")
            })*
        };
    }
    impl ahp_wire::client::SessionClient for Seat {
        fn connect(
            &self,
        ) -> ahp_wire::client::ClientFuture<Result<ahp_wire::client::RootInfo, String>> {
            Box::pin(std::future::ready(Ok(ahp_wire::client::RootInfo {
                agents: Vec::new(),
            })))
        }

        fn list_sessions(
            &self,
            cursor: Option<String>,
        ) -> ahp_wire::client::ClientFuture<Result<ahp_wire::client::SessionsPage, String>>
        {
            assert!(cursor.is_none(), "one page is the whole catalog here");
            Box::pin(std::future::ready(Ok(ahp_wire::client::SessionsPage {
                sessions: vec![ahp_types::state::SessionSummary {
                    origin: None,
                    provider: "test".to_owned(),
                    title: "found by nobody looking".to_owned(),
                    status: 0,
                    activity: None,
                    project: None,
                    working_directories: None,
                    annotations: None,
                    resource: "test-session:/resident".to_owned(),
                    created_at: String::new(),
                    modified_at: String::new(),
                    changes: None,
                    meta: None,
                }],
                next_cursor: None,
            })))
        }

        fn poll_root(&self) -> ahp_wire::client::ClientFuture<Vec<ahp_wire::client::ServerEvent>> {
            // The long poll parks: events are not this test's story.
            Box::pin(std::future::pending())
        }

        off_road! {
            create_session(dirs: Vec<String>, options: ahp_wire::client::SessionOptions) -> ahp_wire::client::ClientFuture<Result<ahp_wire::client::SessionUri, String>>;
            resolve_session_config(working_directory: Option<String>, config: Option<serde_json::Map<String, serde_json::Value>>) -> ahp_wire::client::ClientFuture<Result<ahp_types::commands::ResolveSessionConfigResult, String>>;
            dispose_session(session: ahp_wire::client::SessionUri) -> ahp_wire::client::ClientFuture<Result<(), String>>;
            subscribe_session(session: ahp_wire::client::SessionUri) -> ahp_wire::client::ClientFuture<Result<ahp_types::state::SessionState, String>>;
            poll_session(session: ahp_wire::client::SessionUri) -> ahp_wire::client::ClientFuture<Vec<ahp_types::actions::StateAction>>;
            dispatch_action(channel: ahp_wire::client::ChannelUri, action: ahp_types::actions::StateAction) -> ahp_wire::client::ClientFuture<Result<(), String>>;
        }
    }

    use himark::app::AppFonts;
    use himark::app::Himark;
    let mut app = Himark::new(AppFonts::embedded());
    let _ = app.add_window();
    himark::hiahp::register_all(&mut app);

    let (posted, arriving) = std::sync::mpsc::channel();
    let runner = app.attach_host(
        std::sync::Arc::new(move |command| {
            let _ = posted.send(command);
        }),
        std::sync::Arc::new(|| {}),
    );

    let host = app.register_client(ahp_wire::client::Client {
        session: std::sync::Arc::new(Seat),
        ..ahp_wire::client::inert()
    });
    ahp_session::session::agents::Agents::seed(&mut app.store_mut(), host, "Resident Host");

    // Any batch at all — its tail is where the subscription lives.
    assert!(app.perform_batch(vec![himark::app::AppCommand::Verb(
        imba::command::Verb::Dynamic(std::sync::Arc::new(
            ahp_session::session::driver::RetryHosts
        ))
    )]));
    for _ in 0..20 {
        runner.run();
        while let Ok(command) = arriving.try_recv() {
            app.perform_batch(vec![command]);
        }
    }

    let record =
        ahp_session::session::agents::Agents::record(app.store(), host).expect("the host record");
    assert!(
        matches!(
            record.status,
            ahp_session::session::state::HostStatus::Connected
        ),
        "the host connected with no view open"
    );
    assert_eq!(
        record
            .sessions
            .iter()
            .map(|summary| summary.title.as_str())
            .collect::<Vec<_>>(),
        vec!["found by nobody looking"],
        "and its sessions are in the catalog"
    );
}

#[test]
fn the_add_host_row_takes_a_url_and_dispatches() {
    let mut app = Himark::new(AppFonts::embedded());
    let _ = app.add_window();
    let window = app.sole_window();
    let received = std::sync::Arc::new(std::sync::Mutex::new(None::<String>));
    {
        let received = received.clone();
        himark::higent::flows::AgentFlows::install_add_host(
            &mut app.store_mut(),
            std::sync::Arc::new(move |_store, url| {
                *received.lock().expect("recorder") = Some(url.to_owned());
                None
            }),
        );
    }

    let mut store = app.store_mut().clone();
    let ui = ::editor::test_document::test_ui();
    let mut panel = ahp_session::session::drawer::AgentsPanel::open(
        &store,
        &ui,
        himark::higent::drawer::drawer_asks(window),
    );
    {
        let mut boot: imba::effect::Batch<ahp_session::session::drawer::AgentsCommand> =
            imba::effect::Batch::new();
        imba::View::perform(
            &mut panel,
            &mut store,
            &ui,
            ahp_session::session::drawer::AgentsCommand::Boot,
            &mut boot.effects(),
        );
    }
    let rows = panel.rows();
    let index = rows
        .iter()
        .position(|(label, _)| label == "+ Add Host…")
        .expect("the add-host row stands");
    let mut batch = imba::effect::Batch::new();
    panel.activate(&mut store, &ui, index, &mut batch.effects());
    assert_eq!(
        panel.add_host_text().as_deref(),
        Some(""),
        "picking the row stands the URL input"
    );

    use imba::View;
    panel.perform(
        &mut store,
        &ui,
        ahp_session::session::drawer::AgentsCommand::AddHostInput(
            ::editor::editor_view::EditorCommand::InsertText {
                text: "ws://example:7/?tkn=t".to_owned(),
            },
        ),
        &mut batch.effects(),
    );
    panel.perform(
        &mut store,
        &ui,
        ahp_session::session::drawer::AgentsCommand::SubmitAddHost,
        &mut batch.effects(),
    );
    assert!(panel.add_host_text().is_none(), "Enter clears the input");
    let request = hikit::modal::ModalView::take_request(&mut panel).expect("the dispatch request");
    let hikit::modal::ModalRequest::Perform(verb) = request else {
        panic!("Enter dispatches a Perform request");
    };
    let command = himark::app::verb_command(app.sole_window(), verb).expect("a performable verb");
    assert!(app.perform_batch(vec![command]));
    assert_eq!(
        received.lock().expect("recorder").as_deref(),
        Some("ws://example:7/?tkn=t"),
        "the capability received the URL"
    );

    panel.activate(&mut store, &ui, index, &mut batch.effects());
    panel.perform(
        &mut store,
        &ui,
        ahp_session::session::drawer::AgentsCommand::CancelAddHost,
        &mut batch.effects(),
    );
    assert!(panel.add_host_text().is_none());
    assert!(hikit::modal::ModalView::take_request(&mut panel).is_none());
}

/// One summary as the catalog carries it — idle and read unless the
/// caller says otherwise, so lines stay bare of activity words.
fn summary_at(
    title: &str,
    folders: &[&str],
    modified: &str,
    status: u32,
) -> ahp_types::state::SessionSummary {
    ahp_types::state::SessionSummary {
        origin: None,
        provider: "test".to_owned(),
        title: title.to_owned(),
        status,
        activity: None,
        project: None,
        working_directories: (!folders.is_empty())
            .then(|| folders.iter().map(|folder| folder.to_string()).collect()),
        annotations: None,
        resource: format!("test-session:/{title}"),
        created_at: String::new(),
        modified_at: modified.to_owned(),
        changes: None,
        meta: None,
    }
}

/// An RFC 3339 stamp `ago` before now — the recency groups and the
/// ages read off the real clock, through the same humantime the
/// drawer parses with (summary.rs).
fn stamp_ago(ago: std::time::Duration) -> String {
    humantime::format_rfc3339_seconds(std::time::SystemTime::now() - ago).to_string()
}

/// The sessions are ONE recency list across hosts and folders: the
/// repo column carries the first folder's name (or the host's), the
/// age column leads with the turn's state when it wants a look, and
/// the hosts that are not connected stand in the foot — picking one
/// nudges a connect.
#[test]
fn the_drawer_lists_sessions_by_recency_with_their_repo() {
    let mut app = Himark::new(AppFonts::embedded());
    let _ = app.add_window();
    let window = app.sole_window();
    let offline = app.register_client(ahp_wire::client::inert());
    let mut store = app.store_mut().clone();
    let host = ahp_wire::SessionId::local_default(&store).host;
    ahp_session::session::agents::Agents::seed(&mut store, host, "Test Host");
    ahp_session::session::agents::Agents::set_status(
        &mut store,
        host,
        ahp_session::session::state::HostStatus::Connected,
    );
    ahp_session::session::agents::Agents::seed(&mut store, offline, "Far Host");
    ahp_session::session::agents::Agents::set_status(
        &mut store,
        offline,
        ahp_session::session::state::HostStatus::Failed("unreachable".to_owned()),
    );
    let name = ahp_session::session::agents::Agents::record(&store, host)
        .expect("the host record")
        .name;
    const HIMARK: &str = "file:///dev/himark";
    const DOCS: &str = "file:///dev/docs";
    let hours = |count: u64| stamp_ago(std::time::Duration::from_secs(count * 3600));
    ahp_session::session::agents::Agents::add_sessions(
        &mut store,
        host,
        vec![
            summary_at("older himark", &[HIMARK], &hours(24 * 30), 33),
            // Changed since last viewed: no word, a brighter title.
            summary_at("stray", &[], &hours(24 * 3), 1),
            // The last turn failed.
            summary_at("docs session", &[DOCS], &hours(24 * 2), 34),
            // A turn is streaming.
            summary_at("fresh himark", &[HIMARK], &hours(2), 40),
            // Blocked on the user's answer; the first folder names it.
            summary_at("pair", &[HIMARK, DOCS], &hours(1), 56),
            summary_at("pair reversed", &[DOCS, HIMARK], &hours(24 * 10), 33),
        ],
        true,
    );

    let ui = ::editor::test_document::test_ui();
    let mut panel = ahp_session::session::drawer::AgentsPanel::open(
        &store,
        &ui,
        himark::higent::drawer::drawer_asks(window),
    );
    let mut batch: imba::effect::Batch<ahp_session::session::drawer::AgentsCommand> =
        imba::effect::Batch::new();
    use imba::View;
    panel.perform(
        &mut store,
        &ui,
        ahp_session::session::drawer::AgentsCommand::Boot,
        &mut batch.effects(),
    );

    let line = |repo: &str, word: &str, age: &str| (repo.to_owned(), word.to_owned(), age.to_owned());
    assert_eq!(
        panel.lines(),
        vec![
            line("", "TODAY", ""),
            line("himark", "pair", "needs you · 1h"),
            line("himark", "fresh himark", "working · 2h"),
            line("", "THIS WEEK", ""),
            line("docs", "docs session", "failed · 2d"),
            line(&name, "stray", "3d"),
            line("", "EARLIER", ""),
            line("docs", "pair reversed", "10d"),
            line("himark", "older himark", "30d"),
            line(&name, "+ New Session…", ""),
            line("", "+ Add Host…", ""),
            line("", "Far Host", ""),
            line("", "unreachable", ""),
        ],
    );
    assert_eq!(
        panel.rows().get(11),
        Some(&("Far Host — offline".to_owned(), 0)),
        "the foot names the host and its state"
    );

    // Picking the offline host asks for a connect.
    panel.activate(&mut store, &ui, 11, &mut batch.effects());
    assert!(
        matches!(
            hikit::modal::ModalView::take_request(&mut panel),
            Some(ModalRequest::Perform(_))
        ),
        "the pick nudges the host"
    );
}

#[test]
fn the_drawer_lands_on_the_window_s_open_session() {
    let mut app = Himark::new(AppFonts::embedded());
    let _ = app.add_window();
    let window = app.sole_window();
    let mut store = app.store_mut().clone();
    let host = ahp_wire::SessionId::local_default(&store).host;
    ahp_session::session::agents::Agents::seed(&mut store, host, "Test Host");
    ahp_session::session::agents::Agents::set_status(
        &mut store,
        host,
        ahp_session::session::state::HostStatus::Connected,
    );
    let summary = |title: &str| ahp_types::state::SessionSummary {
        origin: None,
        provider: "test".to_owned(),
        title: title.to_owned(),
        status: 33,
        activity: None,
        project: None,
        working_directories: None,
        annotations: None,
        resource: format!("test-session:/{title}"),
        created_at: String::new(),
        modified_at: "2026-09-22T10:00:00Z".to_owned(),
        changes: None,
        meta: None,
    };
    ahp_session::session::agents::Agents::add_sessions(
        &mut store,
        host,
        vec![summary("alpha"), summary("beta")],
        true,
    );
    let mut entity = ::workbench::window::Windows::window(&store, window).expect("window");
    let beta = ahp_wire::SessionId {
        host,
        session: ahp_wire::client::SessionUri::new("test-session:/beta"),
    };
    let state = ahp_session::session::state::Hosts::ensure_state(&mut store, &beta);
    let _ = entity.switch_to(himark::workspace::SessionWorkspace::boxed(beta, state));
    ::workbench::window::Windows::put(&mut store, window, entity);

    let ui = ::editor::test_document::test_ui();
    let mut panel = ahp_session::session::drawer::AgentsPanel::open(
        &store,
        &ui,
        himark::higent::drawer::drawer_asks(window),
    );
    let mut batch: imba::effect::Batch<ahp_session::session::drawer::AgentsCommand> =
        imba::effect::Batch::new();
    use imba::View;
    panel.perform(
        &mut store,
        &ui,
        ahp_session::session::drawer::AgentsCommand::Boot,
        &mut batch.effects(),
    );

    let rows = panel.rows();
    let beta = rows
        .iter()
        .position(|(label, _)| label == "beta")
        .expect("the beta session stands in the list");
    assert_eq!(
        panel.selected_row(),
        Some(beta),
        "the open session is the selection: {rows:?}"
    );
}

#[test]
fn the_drawer_speed_search_filters_sessions() {
    use imba::effect::{block_on, EffectHandler, Message};

    let mut app = Himark::new(AppFonts::embedded());
    let _ = app.add_window();
    let window = app.sole_window();
    let mut store = app.store_mut().clone();
    let host = ahp_wire::SessionId::local_default(&store).host;
    ahp_session::session::agents::Agents::seed(&mut store, host, "Test Host");
    ahp_session::session::agents::Agents::set_status(
        &mut store,
        host,
        ahp_session::session::state::HostStatus::Connected,
    );
    let summary = |title: &str| ahp_types::state::SessionSummary {
        origin: None,
        provider: "test".to_owned(),
        title: title.to_owned(),
        status: 33,
        activity: None,
        project: None,
        working_directories: None,
        annotations: None,
        resource: format!("test-session:/{title}"),
        created_at: String::new(),
        modified_at: "2026-09-22T10:00:00Z".to_owned(),
        changes: None,
        meta: None,
    };
    ahp_session::session::agents::Agents::add_sessions(
        &mut store,
        host,
        vec![summary("alpha"), summary("beta"), summary("gamma")],
        true,
    );

    let ui = ::editor::test_document::test_ui();
    let mut panel = ahp_session::session::drawer::AgentsPanel::open(
        &store,
        &ui,
        himark::higent::drawer::drawer_asks(window),
    );
    use imba::View;
    let mut drive = |panel: &mut ahp_session::session::drawer::AgentsPanel,
                     command|
     -> imba::effect::Batch<ahp_session::session::drawer::AgentsCommand> {
        let mut batch = imba::effect::Batch::new();
        panel.perform(&mut store, &ui, command, &mut batch.effects());
        batch
    };
    let _ = drive(
        &mut panel,
        ahp_session::session::drawer::AgentsCommand::Boot,
    );

    let typing = drive(
        &mut panel,
        ahp_session::session::drawer::AgentsCommand::Rows(
            hikit::list_keyboard::ListKeyCommand::Input(
                editor::editor_view::EditorCommand::InsertText {
                    text: "bet".to_owned(),
                },
            ),
        ),
    );
    let mut matches = None;
    for message in typing.drain() {
        let (Message::Launch(_, effect) | Message::Relaunch(_, _, effect)) = message else {
            continue;
        };
        let (value, _) = effect.into_payload().split();
        if let Ok(effect) = value.downcast::<hikit::list_keyboard::SpeedSearchEffect>() {
            matches = Some(block_on(Box::pin(async move {
                hikit::list_keyboard::SpeedSearchHandler
                    .handle(*effect)
                    .await
            })));
        }
    }
    let matches = matches.expect("typing launched the filter");
    let landing = drive(
        &mut panel,
        ahp_session::session::drawer::AgentsCommand::Rows(
            hikit::list_keyboard::ListKeyCommand::Landed(matches),
        ),
    );
    // The first-match jump rides the announce round trip.
    if let Some(select) = crate::drain_announced(landing) {
        let _ = drive(&mut panel, select);
    }
    assert_eq!(panel.match_count(), 1, "only beta matches");
    let rows = panel.rows();
    let beta = rows
        .iter()
        .position(|(label, _)| label == "beta")
        .expect("the beta row stands");
    assert_eq!(
        panel.selected_row(),
        Some(beta),
        "the cursor jumped to the match: {rows:?}"
    );

    let step = panel.matched_step_rows(1).expect("a match to step");
    let _ = drive(&mut panel, step);
    assert_eq!(panel.selected_row(), Some(beta), "wrapped in place");

    let _ = drive(
        &mut panel,
        ahp_session::session::drawer::AgentsCommand::Rows(
            hikit::list_keyboard::ListKeyCommand::Clear,
        ),
    );
    assert_eq!(panel.match_count(), 0, "cleared");
}

/// The Sessions drawer over a seeded catalog, 2880×1800 — two hosts
/// (one offline), sessions across today, this week and earlier, the
/// window's own session the here row.
/// `HIMARK_SHOT=<dir> cargo test -p himark-tests --test shell dump_sessions_drawer_screenshot -- --ignored`
#[test]
#[ignore]
fn dump_sessions_drawer_screenshot() {
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
    let offline = app.register_client(ahp_wire::client::inert());
    let host = ahp_wire::SessionId::local_default(app.store()).host;
    const JB: &str = "file:///Users/dev/himark-jb";
    const KIT: &str = "file:///Users/dev/hikit";
    let minutes = |count: u64| stamp_ago(std::time::Duration::from_secs(count * 60));
    let days = |count: u64| stamp_ago(std::time::Duration::from_secs(count * 86_400 + 600));
    let seed = |store: &mut Store| {
        use ahp_session::session::agents::Agents;
        use ahp_session::session::state::HostStatus;
        Agents::seed(store, host, "himark Host");
        Agents::set_status(store, host, HostStatus::Connected);
        Agents::seed(store, offline, "VS Code Agent Host");
        Agents::set_status(
            store,
            offline,
            HostStatus::Failed("agent host unreachable at ws://127.0.0.1:8123/?tkn=4f1c".to_owned()),
        );
    };
    seed(&mut app.store_mut());
    ahp_session::session::agents::Agents::add_sessions(
        &mut app.store_mut(),
        host,
        vec![
            summary_at("hello", &[JB], &minutes(5), 33),
            summary_at("", &[JB], &minutes(7), 33),
            summary_at("", &[KIT], &minutes(12), 33),
            summary_at("", &[JB], &minutes(60), 33),
            summary_at("hey", &[JB], &days(4), 33),
            summary_at("scroll-bar marks on the panel edge", &[KIT], &days(6), 33),
            summary_at("how do I run from xcode in release?", &[JB], &days(6), 33),
            summary_at("ok, what is possible with claude mods, and what is not yet", &[JB], &days(9), 33),
            summary_at("Screenshot 2026-10-09 at 16.05.00.png", &[JB], &days(14), 33),
            summary_at("/statusline", &[JB], &days(14), 33),
            summary_at("diagnostics are still not reported, no squiggles anywhere", &[KIT], &days(15), 33),
            summary_at("just testing, say hi", &[JB], &days(20), 33),
            summary_at("hey, you here?", &[JB], &days(21), 33),
        ]
        .into_iter()
        .enumerate()
        .map(|(index, mut summary)| {
            // Untitled sessions would share a resource — tell them apart.
            if summary.title.is_empty() {
                summary.resource = format!("test-session:/untitled-{index}");
            }
            summary
        })
        .collect(),
        true,
    );
    let open = ahp_wire::SessionId {
        host,
        session: ahp_wire::client::SessionUri::new("test-session:/hello"),
    };
    {
        let mut store = app.store_mut();
        let mut entity = ::workbench::window::Windows::window(&store, window).expect("window");
        let state = ahp_session::session::state::Hosts::ensure_state(&mut store, &open);
        let _ = entity.switch_to(himark::workspace::SessionWorkspace::boxed(open.clone(), state));
        ::workbench::window::Windows::put(&mut store, window, entity);
    }
    assert!(app.perform_command(himark::app::windowed(
        window,
        Arc::new(himark::higent::drawer::ToggleAgentsView),
    )));
    super::dock::settle(&mut app, &mut surface);
    // Opening the drawer nudges every host; the seeded states stand
    // again over whatever the nudge reached.
    seed(&mut app.store_mut());
    super::dock::settle(&mut app, &mut surface);
    let rows = ::workbench::window::Windows::window_ref(app.store(), window)
        .expect("window")
        .side_panel()
        .expect("the drawer is up")
        .as_any()
        .downcast_ref::<ahp_session::session::drawer::AgentsPanel>()
        .map(|panel| panel.lines());
    eprintln!("{rows:#?}");
    let data = surface
        .image_snapshot()
        .encode(None, skia_safe::EncodedImageFormat::PNG, None)
        .expect("png");
    std::fs::write(format!("{dir}/sessions-drawer.png"), data.as_bytes()).expect("write");
}
