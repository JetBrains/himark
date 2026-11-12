#![allow(unused_imports)]
#[allow(unused_imports)]
use workbench_shell::app::WorkbenchShell as _;
#[allow(unused_imports)]
use workbench_shell::app_ext::AppExt as _;
#[allow(unused_imports)]
use workbench_shell::test_support::TestSupport as _;

use super::dock::{entity, located, settle, show_dock};
use super::*;
use hikit::modal::ModalRequest;
use hikit::modal::ModalView;
use himark::app::AppCommand;
use himark::app::AppFonts;
use himark::app::Himark;
use imba::anim::AnimationClock;
use imba::constraints::Constraints;
use imba::event::{Event, EventResult, Key};
use imba::leaf::leaf;
use imba::store::Store;
use imba::thunk_ext::ThunkExt as _;
use imba::{ui::UiCtx, View};
use std::sync::{Arc, Mutex};
use workbench_shell::test_driver;

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
                ..Default::default()
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
    let mut app = workbench_shell::app::workbench_app(AppFonts::embedded());
    let _ = app.add_window();
    hiahp::register_all(&mut app);

    let (posted, arriving) = std::sync::mpsc::channel();
    let runner = app.attach_host(
        std::sync::Arc::new(move |command| {
            let _ = posted.send(command);
        }),
        std::sync::Arc::new(|| {}),
    );

    let host = hiahp::engine::register_client(
        &mut app,
        ahp_wire::client::Client {
            session: std::sync::Arc::new(Seat),
            ..ahp_wire::client::inert()
        },
    );
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

/// "Add host" lives in the header, not in the rows: it opens a
/// two-way menu — the tailnet, or a URL. The URL way stands the
/// field; Enter dispatches the add-host verb; Escape at either step
/// leaves nothing behind.
#[test]
fn the_add_host_menu_takes_a_url_and_dispatches() {
    use ahp_session::session::drawer::AgentsCommand;
    let mut app = workbench_shell::app::workbench_app(AppFonts::embedded());
    let _ = app.add_window();
    let window = app.sole_window();
    let received = std::sync::Arc::new(std::sync::Mutex::new(None::<String>));
    {
        let received = received.clone();
        hiahp::higent::flows::AgentFlows::install_add_host(
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
        workbench_shell::higent::drawer::drawer_asks(window),
    );
    use imba::View;
    let mut batch: imba::effect::Batch<AgentsCommand> = imba::effect::Batch::new();
    panel.perform(&mut store, &ui, AgentsCommand::Boot, &mut batch.effects());
    assert!(
        !panel
            .rows()
            .iter()
            .any(|(label, _)| label.contains("Add Host")),
        "no add-host row stands among the lines: {:?}",
        panel.rows()
    );

    panel.perform(
        &mut store,
        &ui,
        AgentsCommand::AddHost,
        &mut batch.effects(),
    );
    assert_eq!(
        panel.add_host_menu(),
        Some(vec![
            "Connect to tailnet".to_owned(),
            "Enter URL…".to_owned()
        ]),
        "the header word opens the two-way menu"
    );
    assert!(
        panel.add_host_text().is_none(),
        "the menu is not yet the field"
    );

    panel.perform(
        &mut store,
        &ui,
        AgentsCommand::EnterHostUrl,
        &mut batch.effects(),
    );
    assert!(panel.add_host_menu().is_none(), "the pick closes the menu");
    assert_eq!(
        panel.add_host_text().as_deref(),
        Some(""),
        "the URL way stands the input"
    );

    panel.perform(
        &mut store,
        &ui,
        AgentsCommand::AddHostInput(::editor::editor_view::EditorCommand::InsertText {
            text: "ws://example:7/?tkn=t".to_owned(),
        }),
        &mut batch.effects(),
    );
    panel.perform(
        &mut store,
        &ui,
        AgentsCommand::SubmitAddHost,
        &mut batch.effects(),
    );
    assert!(panel.add_host_text().is_none(), "Enter clears the input");
    let request = hikit::modal::ModalView::take_request(&mut panel).expect("the dispatch request");
    let hikit::modal::ModalRequest::Perform(verb) = request else {
        panic!("Enter dispatches a Perform request");
    };
    let command =
        workbench_shell::app::verb_command(app.sole_window(), verb).expect("a performable verb");
    assert!(app.perform_batch(vec![command]));
    assert_eq!(
        received.lock().expect("recorder").as_deref(),
        Some("ws://example:7/?tkn=t"),
        "the capability received the URL"
    );

    panel.perform(
        &mut store,
        &ui,
        AgentsCommand::AddHost,
        &mut batch.effects(),
    );
    panel.perform(
        &mut store,
        &ui,
        AgentsCommand::CancelAddHost,
        &mut batch.effects(),
    );
    assert!(panel.add_host_menu().is_none() && panel.add_host_text().is_none());
    assert!(hikit::modal::ModalView::take_request(&mut panel).is_none());

    // The tailnet way with no embedder flow installed: the ask is
    // noted (the status line says connecting), nothing is dispatched.
    panel.perform(
        &mut store,
        &ui,
        AgentsCommand::AddHost,
        &mut batch.effects(),
    );
    panel.perform(
        &mut store,
        &ui,
        AgentsCommand::ConnectTailnet,
        &mut batch.effects(),
    );
    assert!(panel.add_host_menu().is_none());
    assert!(hikit::modal::ModalView::take_request(&mut panel).is_none());
}

/// The tailnet way fires the embedder's connect flow when one is
/// installed — the drawer never knows tailscale, only the flow.
#[test]
fn the_add_host_menu_s_tailnet_way_fires_the_connect_flow() {
    use ahp_session::session::drawer::AgentsCommand;
    let mut app = workbench_shell::app::workbench_app(AppFonts::embedded());
    let _ = app.add_window();
    let window = app.sole_window();
    let asked = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    {
        let asked = asked.clone();
        app.store_mut().put(ahp_session::tailnet::TailnetFlows {
            connect: std::sync::Arc::new(move || {
                asked.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            }),
            resume: std::sync::Arc::new(|| {}),
        });
    }
    let mut store = app.store_mut().clone();
    let ui = ::editor::test_document::test_ui();
    let mut panel = ahp_session::session::drawer::AgentsPanel::open(
        &store,
        &ui,
        workbench_shell::higent::drawer::drawer_asks(window),
    );
    use imba::View;
    let mut batch: imba::effect::Batch<AgentsCommand> = imba::effect::Batch::new();
    panel.perform(
        &mut store,
        &ui,
        AgentsCommand::AddHost,
        &mut batch.effects(),
    );
    panel.perform(
        &mut store,
        &ui,
        AgentsCommand::ConnectTailnet,
        &mut batch.effects(),
    );
    assert_eq!(asked.load(std::sync::atomic::Ordering::SeqCst), 1);
    assert!(panel.add_host_menu().is_none());
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

/// Hosts are HEADINGS, each with its own "New session"; under a host
/// stand the sessions live right now (a running or blocked turn) or
/// open in this client, each with its folders; the rest follow as
/// older rows. A host that is not connected wears its state word and
/// a "Reconnect" — picking its heading nudges a connect.
#[test]
fn the_drawer_groups_sessions_under_their_hosts() {
    let mut app = workbench_shell::app::workbench_app(AppFonts::embedded());
    let _ = app.add_window();
    let window = app.sole_window();
    let offline = hiahp::engine::register_client(&mut app, ahp_wire::client::inert());
    let mut store = app.store_mut().clone();
    let host = ahp_wire::SessionId::local_default(&store).host;
    ahp_session::session::agents::Agents::seed(&mut store, host, "Test Host");
    ahp_session::session::agents::Agents::set_status(
        &mut store,
        host,
        ahp_session::session::state::HostStatus::Connected,
    );
    ahp_session::session::agents::Agents::set_identity(
        &mut store,
        host,
        Some("studio".to_owned()),
        Some("macOS".to_owned()),
        None,
    );
    ahp_session::session::agents::Agents::seed(&mut store, offline, "Far Host");
    ahp_session::session::agents::Agents::set_status(
        &mut store,
        offline,
        ahp_session::session::state::HostStatus::Failed("unreachable".to_owned()),
    );
    const HIMARK: &str = "file:///dev/himark";
    const DOCS: &str = "file:///dev/docs";
    let hours = |count: u64| stamp_ago(std::time::Duration::from_secs(count * 3600));
    ahp_session::session::agents::Agents::add_sessions(
        &mut store,
        host,
        vec![
            summary_at("older himark", &[HIMARK], &hours(24 * 30), 33),
            // Changed since last viewed: no word, the attention mark.
            summary_at("stray", &[], &hours(24 * 3), 1),
            // The last turn failed.
            summary_at("docs session", &[DOCS], &hours(24 * 2), 34),
            // A turn is streaming — live.
            summary_at("fresh himark", &[HIMARK], &hours(2), 40),
            // Blocked on the user's answer — live, both folders named.
            summary_at("pair", &[HIMARK, DOCS], &hours(1), 56),
            summary_at("pair reversed", &[DOCS, HIMARK], &hours(24 * 10), 33),
            // A newborn placeholder nobody prompted: not a session yet.
            summary_at("New Session", &[HIMARK], &hours(0), 33),
        ],
        true,
    );

    let ui = ::editor::test_document::test_ui();
    let mut panel = ahp_session::session::drawer::AgentsPanel::open(
        &store,
        &ui,
        workbench_shell::higent::drawer::drawer_asks(window),
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

    let line = |detail: &str, word: &str, trail: &str| {
        (detail.to_owned(), word.to_owned(), trail.to_owned())
    };
    assert_eq!(
        panel.lines(),
        vec![
            // The heading wears the machine's name and OS, not the label.
            line("macOS", "studio", "New session"),
            line("himark · docs", "pair", "needs you · 1h"),
            line("himark", "fresh himark", "working · 2h"),
            // The older rows group by their folders, newest group first.
            line("", "docs", ""),
            line("docs", "docs session", "failed · 2d"),
            line("", "no folder", ""),
            line("no folder", "stray", "3d"),
            line("", "docs · himark", ""),
            line("docs · himark", "pair reversed", "10d"),
            line("", "himark", ""),
            line("himark", "older himark", "30d"),
            line("offline", "Far Host", "Reconnect"),
            line("", "unreachable", ""),
        ],
    );
    assert_eq!(
        panel.rows().get(11),
        Some(&("Far Host — offline".to_owned(), 0)),
        "the heading names the host and its state"
    );
    assert_eq!(
        panel.rows().get(4).map(|(_, depth)| *depth),
        Some(2),
        "a grouped row stands one deeper than its heading"
    );
    assert_eq!(panel.new_session_row(host), Some(0));

    // Picking the offline host's heading asks for a connect.
    panel.activate(&mut store, &ui, 11, &mut batch.effects());
    assert!(
        matches!(
            hikit::modal::ModalView::take_request(&mut panel),
            Some(ModalRequest::Perform(_))
        ),
        "the pick nudges the host"
    );
    // Picking the connected host's heading asks for a new session.
    panel.activate(&mut store, &ui, 0, &mut batch.effects());
    assert!(matches!(
        hikit::modal::ModalView::take_request(&mut panel),
        Some(ModalRequest::Perform(_))
    ));
}

/// Past the preview, a host's older sessions fold under "Older · N":
/// the word toggles between show all and show less, and the fold
/// line keeps the cursor across the toggle.
#[test]
fn the_older_fold_shows_all_and_then_less() {
    let mut app = workbench_shell::app::workbench_app(AppFonts::embedded());
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
    let days = |count: u64| stamp_ago(std::time::Duration::from_secs(count * 86_400 + 60));
    let sessions: Vec<_> = (1..=8)
        .map(|day| summary_at(&format!("day {day}"), &["file:///dev/x"], &days(day), 33))
        .collect();
    ahp_session::session::agents::Agents::add_sessions(&mut store, host, sessions, true);

    let ui = ::editor::test_document::test_ui();
    let mut panel = ahp_session::session::drawer::AgentsPanel::open(
        &store,
        &ui,
        workbench_shell::higent::drawer::drawer_asks(window),
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
    let words = |panel: &ahp_session::session::drawer::AgentsPanel| -> Vec<(String, String)> {
        panel
            .lines()
            .into_iter()
            .map(|(_, word, trail)| (word, trail))
            .collect()
    };
    let fold = panel.fold_row(host).expect("the fold stands");
    assert_eq!(fold, 1, "the fold follows the heading");
    let shown = words(&panel);
    assert_eq!(shown[1], ("Older".to_owned(), "show all".to_owned()));
    assert_eq!(panel.lines()[1].0, "8", "the fold counts the older rows");
    // One folders group heads the preview; the preview counts rows,
    // not headings.
    assert_eq!(shown[2].0, "x", "the group is headed by its folders");
    assert_eq!(
        shown.len(),
        1 + 1 + 1 + 5,
        "five preview under the fold: {shown:?}"
    );
    assert_eq!(shown[3].0, "day 1");
    assert_eq!(shown[7].0, "day 5");

    panel.activate(&mut store, &ui, fold, &mut batch.effects());
    let shown = words(&panel);
    assert_eq!(shown[1], ("Older".to_owned(), "show less".to_owned()));
    assert_eq!(shown.len(), 1 + 1 + 1 + 8, "show all shows all: {shown:?}");
    assert_eq!(
        panel.selected_row(),
        Some(fold),
        "the cursor stays on the fold"
    );

    panel.activate(&mut store, &ui, fold, &mut batch.effects());
    let shown = words(&panel);
    assert_eq!(shown[1], ("Older".to_owned(), "show all".to_owned()));
    assert_eq!(shown.len(), 1 + 1 + 1 + 5);
}

#[test]
fn the_drawer_lands_on_the_window_s_open_session() {
    let mut app = workbench_shell::app::workbench_app(AppFonts::embedded());
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
    let _ = entity.switch_to(workbench_shell::workspace::SessionWorkspace::boxed(
        beta, state,
    ));
    ::workbench::window::Windows::put(&mut store, window, entity);

    let ui = ::editor::test_document::test_ui();
    let mut panel = ahp_session::session::drawer::AgentsPanel::open(
        &store,
        &ui,
        workbench_shell::higent::drawer::drawer_asks(window),
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

    let mut app = workbench_shell::app::workbench_app(AppFonts::embedded());
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
        workbench_shell::higent::drawer::drawer_asks(window),
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
    let mut app = workbench_shell::app::workbench_app(AppFonts::embedded());
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
    app.draw_window(window, &gfx_skia::SkiaCanvas::new(surface.canvas()));
    let offline = hiahp::engine::register_client(&mut app, ahp_wire::client::inert());
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
            HostStatus::Failed(
                "agent host unreachable at ws://127.0.0.1:8123/?tkn=4f1c".to_owned(),
            ),
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
            summary_at(
                "ok, what is possible with claude mods, and what is not yet",
                &[JB],
                &days(9),
                33,
            ),
            summary_at(
                "Screenshot 2026-10-09 at 16.05.00.png",
                &[JB],
                &days(14),
                33,
            ),
            summary_at("/statusline", &[JB], &days(14), 33),
            summary_at(
                "diagnostics are still not reported, no squiggles anywhere",
                &[KIT],
                &days(15),
                33,
            ),
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
        let _ = entity.switch_to(workbench_shell::workspace::SessionWorkspace::boxed(
            open.clone(),
            state,
        ));
        ::workbench::window::Windows::put(&mut store, window, entity);
    }
    assert!(app.perform_batch(vec![workbench_shell::shell::windowed(
        window,
        Arc::new(workbench_shell::higent::drawer::ToggleAgentsView),
    )]));
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

/// TEST SUPPORT: switch the window to a session directly — what
/// the drawer's pick does once the session is live here.
struct SwitchTo(ahp_wire::SessionId);
impl himark::commands::HimarkCommand for SwitchTo {
    fn id(&self) -> &'static str {
        "test.switch-to"
    }
    fn name(&self) -> String {
        "Switch To".to_owned()
    }
    fn perform(
        &self,
        store: &mut Store,
        _ui: &UiCtx,
        bound: &imba::focus::Bindings,
        fx: &mut himark::app::AppFx<'_>,
    ) {
        let Some(window) = workbench_shell::app::bound_window(self.id(), bound) else {
            return;
        };
        workbench_shell::app::switch_session(store, window, self.0.clone(), fx);
    }
}

/// Picking a session this client already ENTERED switches the window
/// in the pick's own batch — its workbench is stashed in the window,
/// so there is nothing to wait for. A session not yet entered goes
/// out to the host first and the window stays put until it answers.
#[test]
fn picking_an_entered_session_switches_in_the_same_batch() {
    let mut app = workbench_shell::app::workbench_app(AppFonts::embedded());
    let _ = app.add_window();
    let window = app.sole_window();
    let host = ahp_wire::SessionId::local_default(app.store()).host;
    let key = |name: &str| ahp_wire::SessionId {
        host,
        session: ahp_wire::client::SessionUri::new(format!("test-session:/{name}")),
    };
    {
        use ahp_session::session::agents::Agents;
        let mut store = app.store_mut();
        Agents::seed(&mut store, host, "Test Host");
        Agents::set_status(
            &mut store,
            host,
            ahp_session::session::state::HostStatus::Connected,
        );
        Agents::add_sessions(
            &mut store,
            host,
            vec![
                summary_at("alpha", &[], "2026-09-22T10:00:00Z", 33),
                summary_at("beta", &[], "2026-09-22T09:00:00Z", 33),
                summary_at("gamma", &[], "2026-09-22T08:00:00Z", 33),
            ],
            true,
        );
        for name in ["alpha", "beta"] {
            let _ = ahp_session::session::state::Hosts::ensure_state(&mut store, &key(name));
            Agents::set_channel(
                &mut store,
                &key(name),
                ahp_wire::client::SessionChannel::default(),
            );
        }
    }
    let session_of = |app: &Himark| workbench_shell::workspace::entity_session(entity(app));
    assert!(app.perform_batch(vec![workbench_shell::shell::windowed(
        window,
        Arc::new(SwitchTo(key("alpha")))
    )]));
    assert_eq!(session_of(&app), key("alpha"));

    // The drawer's pick of beta — entered: the window moves now.
    let pick = |name: &str| {
        let verb = (workbench_shell::higent::drawer::drawer_asks(window).open_row)(
            host,
            ahp_wire::client::SessionUri::new(format!("test-session:/{name}")),
        );
        workbench_shell::app::verb_command(window, verb).expect("a performable verb")
    };
    assert!(app.perform_batch(vec![pick("beta")]));
    assert_eq!(
        session_of(&app),
        key("beta"),
        "an entered session switches in the batch"
    );
    assert!(app.perform_batch(vec![pick("alpha")]));
    assert_eq!(session_of(&app), key("alpha"), "and back");

    // gamma was never entered: the pick asks the host, the window waits.
    assert!(app.perform_batch(vec![pick("gamma")]));
    assert_eq!(
        session_of(&app),
        key("alpha"),
        "a session not yet entered waits for the host"
    );
}

/// Switch between ENTERED sessions, drawer up, over a catalog the
/// size of a real one — where does the time go?
/// `cargo test -p himark-tests --release --test shell session_switch_timing -- --ignored --nocapture`
#[test]
#[ignore]
fn session_switch_timing() {
    let sessions: usize = std::env::var("HIMARK_BENCH_SESSIONS")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(800);
    let mut app = workbench_shell::app::workbench_app(AppFonts::embedded());
    let _ = app.add_window();
    let window = app.sole_window();
    let mut surface = skia_safe::surfaces::raster_n32_premul((2880, 1800)).expect("surface");
    app.draw_window(window, &gfx_skia::SkiaCanvas::new(surface.canvas()));
    let host = ahp_wire::SessionId::local_default(app.store()).host;
    let minutes = |count: u64| stamp_ago(std::time::Duration::from_secs(count * 60));
    {
        use ahp_session::session::agents::Agents;
        use ahp_session::session::state::HostStatus;
        let mut store = app.store_mut();
        Agents::seed(&mut store, host, "himark Host");
        Agents::set_status(&mut store, host, HostStatus::Connected);
        let folders = [
            "file:///dev/himark-jb",
            "file:///dev/hikit",
            "file:///dev/docs",
        ];
        let catalog: Vec<_> = (0..sessions)
            .map(|index| {
                let mut summary = summary_at(
                    &format!("session {index} about something or other"),
                    &[folders[index % folders.len()]],
                    &minutes(index as u64 * 37),
                    33,
                );
                summary.resource = format!("test-session:/{index}");
                summary
            })
            .collect();
        Agents::add_sessions(&mut store, host, catalog, true);
    }
    let entered: Vec<ahp_wire::SessionId> = (0..3)
        .map(|index| ahp_wire::SessionId {
            host,
            session: ahp_wire::client::SessionUri::new(format!("test-session:/{index}")),
        })
        .collect();
    for key in &entered {
        let mut store = app.store_mut();
        let _ = ahp_session::session::state::Hosts::ensure_state(&mut store, key);
        ahp_session::session::agents::Agents::set_channel(
            &mut store,
            key,
            ahp_wire::client::SessionChannel {
                working_directories: vec!["file:///dev/himark-jb".to_owned()]
                    .into_iter()
                    .collect(),
                ..Default::default()
            },
        );
    }
    let timed = |label: &str, app: &mut Himark, surface: &mut skia_safe::Surface, command| {
        let started = std::time::Instant::now();
        assert!(app.perform_batch(vec![command]));
        let batch = started.elapsed();
        let mut draws = Vec::new();
        for tick in 0..60 {
            let started = std::time::Instant::now();
            app.draw_window(window, &gfx_skia::SkiaCanvas::new(surface.canvas()));
            draws.push(started.elapsed());
            let busy = test_driver::animate(app, AnimationClock::from_millis(tick as f64 * 32.0));
            if !busy && tick > 1 {
                break;
            }
        }
        let total: std::time::Duration = draws.iter().sum();
        eprintln!(
            "{label}: batch {:.1} ms, {} draws totalling {:.1} ms (first {:.1} ms, max {:.1} ms)",
            batch.as_secs_f64() * 1e3,
            draws.len(),
            total.as_secs_f64() * 1e3,
            draws[0].as_secs_f64() * 1e3,
            draws.iter().max().unwrap().as_secs_f64() * 1e3,
        );
    };
    let switch = |key: &ahp_wire::SessionId| {
        workbench_shell::shell::windowed(window, Arc::new(SwitchTo(key.clone())))
    };
    timed(
        "enter A (drawer closed)",
        &mut app,
        &mut surface,
        switch(&entered[0]),
    );
    timed(
        "enter B (drawer closed)",
        &mut app,
        &mut surface,
        switch(&entered[1]),
    );
    timed(
        "back to A (drawer closed)",
        &mut app,
        &mut surface,
        switch(&entered[0]),
    );
    timed(
        "open drawer",
        &mut app,
        &mut surface,
        workbench_shell::shell::windowed(
            window,
            Arc::new(workbench_shell::higent::drawer::ToggleAgentsView),
        ),
    );
    timed(
        "to B (drawer up)",
        &mut app,
        &mut surface,
        switch(&entered[1]),
    );
    timed(
        "open drawer again",
        &mut app,
        &mut surface,
        workbench_shell::shell::windowed(
            window,
            Arc::new(workbench_shell::higent::drawer::ToggleAgentsView),
        ),
    );
    timed(
        "to C (drawer up)",
        &mut app,
        &mut surface,
        switch(&entered[2]),
    );
    // A catalog event while a session streams: the drawer's refresh.
    timed(
        "open drawer once more",
        &mut app,
        &mut surface,
        workbench_shell::shell::windowed(
            window,
            Arc::new(workbench_shell::higent::drawer::ToggleAgentsView),
        ),
    );
    {
        let mut store = app.store_mut();
        ahp_session::session::agents::Agents::set_status(
            &mut store,
            host,
            ahp_session::session::state::HostStatus::Connected,
        );
    }
    timed(
        "one catalog event (drawer up)",
        &mut app,
        &mut surface,
        himark::app::AppCommand::Verb(imba::command::Verb::Dynamic(Arc::new(
            ahp_session::session::driver::RetryHosts,
        ))),
    );
}
