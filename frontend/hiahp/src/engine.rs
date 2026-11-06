// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

//! The SESSION API over the engine — catalog snapshots, headless
//! enter/create, the local-host designation, the client mint: free
//! fns over `Himark`'s public doors (this crate owns the types).

use std::sync::Arc;

use imba::store::Store;

use himark::app::Himark;
use himark::bridge::ViewBridgeId;

/// One catalog snapshot — plain rows, cloned out, immutable.
pub struct Catalog {
    pub hosts: Vec<HostRow>,
    pub sessions: Vec<SessionRow>,
}

pub struct HostRow {
    pub host: ahp_wire::client::HostId,
    pub name: String,
    /// 0 idle · 1 connecting · 2 connected · 3 failed.
    pub status: u32,
    /// The connect error when failed — what the native chrome shows
    /// under the row.
    pub error: Option<String>,
}

pub struct SessionRow {
    pub host: ahp_wire::client::HostId,
    pub uri: String,
    pub provider: String,
    pub title: String,
    pub status: i64,
    pub modified_at: String,
}

/// The session catalog, cloned out whole for native chrome — the
/// QUERY half of the session surface (docs/ui/himark-ffi.md).
pub fn catalog(app: &Himark) -> Catalog {
    let store = app.store();
    let mut hosts = Vec::new();
    let mut sessions = Vec::new();
    for (host, record) in ahp_session::session::agents::Agents::list(store) {
        hosts.push(HostRow {
            host,
            name: record.name.clone(),
            status: match record.status {
                ahp_session::session::state::HostStatus::Idle => 0,
                ahp_session::session::state::HostStatus::Connecting => 1,
                ahp_session::session::state::HostStatus::Connected => 2,
                ahp_session::session::state::HostStatus::Failed(_) => 3,
            },
            error: match &record.status {
                ahp_session::session::state::HostStatus::Failed(error) => Some(error.clone()),
                _ => None,
            },
        });
        for summary in record.sessions.iter() {
            sessions.push(SessionRow {
                host,
                uri: summary.resource.clone(),
                provider: summary.provider.clone(),
                title: summary.title.clone(),
                status: summary.status as i64,
                modified_at: summary.modified_at.clone(),
            });
        }
    }
    Catalog { hosts, sessions }
}

/// Enter a session HEADLESSLY — subscribe, adopt, ensure the
/// wires, relaunch the poll; no window switches, no chat opens.
/// The native shell's selection road.
pub fn enter_session(
    app: &mut Himark,
    host: ahp_wire::client::HostId,
    session: ahp_wire::client::SessionUri,
) -> bool {
    app.perform_batch(vec![himark::app::AppCommand::Verb(imba::command::Verb::once(
        crate::higent::open_session::EnterHeadless {
            server: host,
            session,
        },
    ))])
}

/// Create a session HEADLESSLY on the given host (or the local
/// default); the landing enters it, the catalog carries the rest.
pub fn create_session(app: &mut Himark, host: Option<ahp_wire::client::HostId>) -> bool {
    app.perform_batch(vec![himark::app::AppCommand::Verb(imba::command::Verb::once(
        crate::higent::open_session::CreateHeadless { host },
    ))])
}

/// Every command the SESSION can run: ask each view mounted into
/// this workspace what it offers and concatenate. The global
/// registry rides on top at the call site.
pub fn session_commands(
    app: &Himark,
    session: &ahp_wire::SessionId,
) -> Vec<imba::PresentableCommand<himark::app::AppCommand>> {
    let mut collected = Vec::new();
    for id in app.mounted_ids() {
        if crate::sessions::session_of(app.store(), id).as_ref() == Some(session) {
            collected.extend(app.bridge_commands(id));
        }
    }
    collected
}

/// Designate the LOCAL host — the store half: the `LocalHost`
/// cell, the session rekey, the uri stamp. The workbench adds its
/// windows' workspace adoption on top.
pub fn set_local_host(app: &mut Himark, host: ahp_wire::client::HostId) {
    app.setup(|store| {
        let previous = store
            .get::<ahp_wire::client::LocalHost>()
            .and_then(|local| local.0);
        store.update::<ahp_wire::client::LocalHost>(|local| local.0 = Some(host));
        store.update::<ahp_session::session::state::Hosts>(|hosts| {
            hosts.rekey_local_sessions(previous, host);
        });
        // Families minted under the LOCAL placeholder carried no
        // uri map; now that they live under the real host, stamp
        // its map onto them (docs/entities.md law 4).
        ahp_session::session::state::Hosts::stamp_host_uris(store, host);
    });
}

pub fn register_client(
    app: &mut Himark,
    client: ahp_wire::client::Client,
) -> ahp_wire::client::HostId {
    let mut minted = ahp_wire::client::HostId::LOCAL;
    app.setup(|store| {
        store.update::<ahp_wire::client::Servers>(|servers| minted = servers.mint(client));
    });
    minted
}

/// The session prelude: the session state the UI bound, or a loud
/// drop.
pub fn bound_session(
    command: &str,
    bound: &imba::focus::Bindings,
) -> Option<ahp_session::session::state::SessionState> {
    let state = bound
        .bound::<ahp_session::session::state::SessionState>()
        .cloned();
    if state.is_none() {
        eprintln!("[shell] a session command landed with no session bound — dropped: {command}");
    }
    state
}

/// The session RESIDENTS a boot installs: the hosts/servers cells
/// and the `@`-mention context over the session folders — what
/// `Himark::bare` used to hardwire.
pub fn install_residents(app: &mut Himark) {
    app.setup(|store| {
        store.put(ahp_session::session::state::Hosts::default());
        store.put(ahp_wire::client::Servers::default());
        ::editor::completion::Mentions::install(store, |store, location| {
            let state = ahp_session::session::state::Hosts::states(store)
                .into_iter()
                .find(|state| {
                    documents::OpenDocuments::by_location(store, state.documents(), location)
                        .is_some()
                })?;
            let (session, state) =
                ahp_session::session::state::Hosts::home_of_documents(store, state.documents())?;
            Some(::editor::completion::MentionContext {
                folders: Arc::new(ahp_session::session::folders::session_folders(
                    store, &session,
                )),
                recents: recents::RecentLocations::list(store, state.recents()),
            })
        });
    });
}
