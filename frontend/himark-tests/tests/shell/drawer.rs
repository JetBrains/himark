#![allow(unused_imports)]
use super::*;
use super::dock::{entity, located, settle, show_dock};
use std::sync::{Arc, Mutex};
use imba::anim::AnimationClock;
use imba::constraints::Constraints;
use imba::event::{Event, EventResult, Key};
use imba::leaf::leaf;
use imba::store::Store;
use imba::thunk_ext::ThunkExt as _;
use imba::{ui::UiCtx, View};
use himark::test_driver;
use himark::app::AppCommand;
use himark::app_ext::AppExt;
use himark::app::AppFonts;
use himark::app::Application;
use hikit::modal::ModalRequest;
use hikit::modal::ModalView;


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

        fn poll_root(
            &self,
        ) -> ahp_wire::client::ClientFuture<Vec<ahp_wire::client::ServerEvent>> {
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

    use himark::app::Application;
    use himark::app::AppFonts;
    let mut app = Application::new(AppFonts::embedded());
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
    let mut app = Application::new(AppFonts::embedded());
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
    let mut panel = ahp_session::session::drawer::AgentsPanel::open(&store, &ui, himark::higent::drawer::drawer_asks(window));
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
        ahp_session::session::drawer::AgentsCommand::AddHostInput(::editor::editor_view::EditorCommand::InsertText {
            text: "ws://example:7/?tkn=t".to_owned(),
        }),
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
    let command =
        himark::app::verb_command(app.sole_window(), verb).expect("a performable verb");
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


#[test]
fn the_drawer_groups_sessions_by_folder_most_recent_first() {
    let mut app = Application::new(AppFonts::embedded());
    let _ = app.add_window();
    let window = app.sole_window();
    let mut store = app.store_mut().clone();
    let host = ahp_wire::SessionId::local_default(&store).host;
    ahp_session::session::agents::Agents::seed(&mut store, host, "Test Host");
    ahp_session::session::agents::Agents::set_status(&mut store, host, ahp_session::session::state::HostStatus::Connected);
    let summary =
        |title: &str, folders: &[&str], modified: &str| ahp_types::state::SessionSummary {
            origin: None,
            provider: "test".to_owned(),
            title: title.to_owned(),
            // Idle and read — labels stay bare of activity marks.
            status: 33,
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
        };
    const HIMARK: &str = "file:///dev/himark";
    const DOCS: &str = "file:///dev/docs";
    ahp_session::session::agents::Agents::add_sessions(
        &mut store,
        host,
        vec![
            summary("older himark", &[HIMARK], "2026-09-20T10:00:00Z"),
            {
                // Changed since last viewed — the filled dot.
                let mut stray = summary("stray", &[], "2026-09-21T10:00:00Z");
                stray.status = 1;
                stray
            },
            {
                // The last turn failed — the bang.
                let mut errored = summary("docs session", &[DOCS], "2026-09-21T12:00:00Z");
                errored.status = 34; // Error | IsRead
                errored
            },
            {
                // A turn is streaming — the hollow dot.
                let mut busy = summary("fresh himark", &[HIMARK], "2026-09-22T09:00:00Z");
                busy.status = 40; // InProgress | IsRead
                busy
            },
            // The pair sessions share a folder SET — order must not
            // split them into two groups.
            {
                // Blocked on the user's answer — the question mark.
                let mut asking = summary("pair", &[HIMARK, DOCS], "2026-09-22T11:00:00Z");
                asking.status = 56; // InputNeeded | IsRead
                asking
            },
            summary("pair reversed", &[DOCS, HIMARK], "2026-09-21T09:00:00Z"),
        ],
        true,
    );

    let ui = ::editor::test_document::test_ui();
    let mut panel = ahp_session::session::drawer::AgentsPanel::open(&store, &ui, himark::higent::drawer::drawer_asks(window));
    let mut batch: imba::effect::Batch<ahp_session::session::drawer::AgentsCommand> =
        imba::effect::Batch::new();
    use imba::View;
    panel.perform(
        &mut store,
        &ui,
        ahp_session::session::drawer::AgentsCommand::Boot,
        &mut batch.effects(),
    );

    assert_eq!(
        panel.rows(),
        vec![
            ("Local".to_owned(), 0),
            // The two-folder set holds the freshest session, so that
            // group leads; both orderings of the set land in it.
            ("docs, himark".to_owned(), 1),
            ("? pair".to_owned(), 2),
            ("pair reversed".to_owned(), 2),
            ("himark".to_owned(), 1),
            ("○ fresh himark".to_owned(), 2),
            ("older himark".to_owned(), 2),
            ("docs".to_owned(), 1),
            ("! docs session".to_owned(), 2),
            // No folder — the stray stays a plain row, ranked by its
            // own recency; unread, so it wears the filled dot.
            ("● stray".to_owned(), 1),
            ("+ New Session…".to_owned(), 1),
            ("+ Add Host…".to_owned(), 0),
        ],
    );

    // Folding a folder row hides its sessions and nothing else.
    let index = panel
        .rows()
        .iter()
        .position(|(label, depth)| label == "himark" && *depth == 1)
        .expect("the himark folder row stands");
    panel.activate(&mut store, &ui, index, &mut batch.effects());
    assert_eq!(
        panel.rows(),
        vec![
            ("Local".to_owned(), 0),
            ("docs, himark".to_owned(), 1),
            ("? pair".to_owned(), 2),
            ("pair reversed".to_owned(), 2),
            ("himark".to_owned(), 1),
            ("docs".to_owned(), 1),
            ("! docs session".to_owned(), 2),
            ("● stray".to_owned(), 1),
            ("+ New Session…".to_owned(), 1),
            ("+ Add Host…".to_owned(), 0),
        ],
    );
}


#[test]
fn the_drawer_lands_on_the_window_s_open_session() {
    let mut app = Application::new(AppFonts::embedded());
    let _ = app.add_window();
    let window = app.sole_window();
    let mut store = app.store_mut().clone();
    let host = ahp_wire::SessionId::local_default(&store).host;
    ahp_session::session::agents::Agents::seed(&mut store, host, "Test Host");
    ahp_session::session::agents::Agents::set_status(&mut store, host, ahp_session::session::state::HostStatus::Connected);
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
    let mut panel = ahp_session::session::drawer::AgentsPanel::open(&store, &ui, himark::higent::drawer::drawer_asks(window));
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

    let mut app = Application::new(AppFonts::embedded());
    let _ = app.add_window();
    let window = app.sole_window();
    let mut store = app.store_mut().clone();
    let host = ahp_wire::SessionId::local_default(&store).host;
    ahp_session::session::agents::Agents::seed(&mut store, host, "Test Host");
    ahp_session::session::agents::Agents::set_status(&mut store, host, ahp_session::session::state::HostStatus::Connected);
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
    let mut panel = ahp_session::session::drawer::AgentsPanel::open(&store, &ui, himark::higent::drawer::drawer_asks(window));
    use imba::View;
    let mut drive = |panel: &mut ahp_session::session::drawer::AgentsPanel,
                     command|
     -> imba::effect::Batch<ahp_session::session::drawer::AgentsCommand> {
        let mut batch = imba::effect::Batch::new();
        panel.perform(&mut store, &ui, command, &mut batch.effects());
        batch
    };
    let _ = drive(&mut panel, ahp_session::session::drawer::AgentsCommand::Boot);

    let typing = drive(
        &mut panel,
        ahp_session::session::drawer::AgentsCommand::Rows(hikit::list_keyboard::ListKeyCommand::Input(
            editor::editor_view::EditorCommand::InsertText {
                text: "bet".to_owned(),
            },
        )),
    );
    let mut matches = None;
    for message in typing.drain() {
        let (Message::Launch(_, effect) | Message::Relaunch(_, _, effect)) = message else {
            continue;
        };
        let (value, _) = effect.into_payload().split();
        if let Ok(effect) = value.downcast::<hikit::list_keyboard::SpeedSearchEffect>() {
            matches = Some(block_on(Box::pin(async move {
                hikit::list_keyboard::SpeedSearchHandler.handle(*effect).await
            })));
        }
    }
    let matches = matches.expect("typing launched the filter");
    let landing = drive(
        &mut panel,
        ahp_session::session::drawer::AgentsCommand::Rows(hikit::list_keyboard::ListKeyCommand::Landed(matches)),
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
        ahp_session::session::drawer::AgentsCommand::Rows(hikit::list_keyboard::ListKeyCommand::Clear),
    );
    assert_eq!(panel.match_count(), 0, "cleared");
}
