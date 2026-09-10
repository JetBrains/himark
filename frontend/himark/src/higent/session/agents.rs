use std::sync::Arc;

use super::state::{Host, HostStatus, Hosts, SessionChannel};
use crate::higent::{HostId, ServerEvent};
use crate::SessionId;
use ahp_types::common::Uri;
use ahp_types::notifications::PartialSessionSummary;
use ahp_types::state::{AgentInfo, SessionSummary};
use imba::store::Store;

pub type NewSessionFlow = Arc<dyn Fn(HostId) -> Arc<dyn crate::DynamicCommand> + Send + Sync>;

pub type AddHostFlow =
    Arc<dyn Fn(&mut crate::Application, &mut Store, &str) -> Option<HostId> + Send + Sync>;

#[derive(Clone, Default)]
pub struct Agents {
    new_session: Option<NewSessionFlow>,
    add_host: Option<AddHostFlow>,

    latest_turns: rpds::HashTrieMapSync<SessionId, String>,
}

impl Agents {
    pub fn seed(store: &mut Store, server: HostId, name: impl Into<String>) {
        Hosts::ensure_row(store, server, name.into());
    }

    pub fn install_new_session(store: &mut Store, flow: NewSessionFlow) {
        store.update::<Agents>(|agents| agents.new_session = Some(flow));
    }

    pub fn new_session_flow(store: &Store) -> Option<NewSessionFlow> {
        store.get::<Agents>()?.new_session.clone()
    }

    pub fn install_add_host(store: &mut Store, flow: AddHostFlow) {
        store.update::<Agents>(|agents| agents.add_host = Some(flow));
    }

    pub fn add_host_flow(store: &Store) -> Option<AddHostFlow> {
        store.get::<Agents>()?.add_host.clone()
    }

    pub fn list(store: &Store) -> Vec<(HostId, Host)> {
        Hosts::list(store)
    }

    pub fn record(store: &Store, server: HostId) -> Option<Host> {
        Hosts::host(store, server)
    }

    fn update_record(store: &mut Store, server: HostId, mutate: impl FnOnce(&mut Host)) {
        Hosts::update(store, server, mutate);
    }

    pub fn set_status(store: &mut Store, server: HostId, status: HostStatus) {
        Self::update_record(store, server, |record| record.status = status);
    }

    pub fn set_agents(store: &mut Store, server: HostId, agents: Vec<AgentInfo>) {
        Self::update_record(store, server, |record| {
            record.agents = agents.into_iter().collect();
        });
    }

    pub fn add_sessions(
        store: &mut Store,
        server: HostId,
        sessions: Vec<SessionSummary>,
        first: bool,
    ) {
        Self::update_record(store, server, |record| {
            if first {
                record.sessions = sessions.into_iter().collect();
            } else {
                for summary in sessions {
                    record.sessions.push_back_mut(summary);
                }
            }
        });
    }

    pub fn apply_event(store: &mut Store, server: HostId, event: ServerEvent) {
        match event {
            ServerEvent::SessionAdded(summary) => {
                Self::update_record(store, server, |record| {
                    let kept: rpds::VectorSync<SessionSummary> = record
                        .sessions
                        .iter()
                        .filter(|held| held.resource != summary.resource)
                        .cloned()
                        .collect();
                    let mut fresh = rpds::VectorSync::new_sync();
                    fresh.push_back_mut(summary);
                    for held in kept.iter() {
                        fresh.push_back_mut(held.clone());
                    }
                    record.sessions = fresh;
                });
            }
            ServerEvent::SessionRemoved(session) => {
                Self::update_record(store, server, |record| {
                    record.sessions = record
                        .sessions
                        .iter()
                        .filter(|held| held.resource != session)
                        .cloned()
                        .collect();
                    record.states.remove_mut(&session);
                });
            }
            ServerEvent::SessionChanged { session, changes } => {
                Self::update_record(store, server, |record| {
                    let mut rows: Vec<SessionSummary> = record.sessions.iter().cloned().collect();
                    let Some(at) = rows.iter().position(|held| held.resource == session) else {
                        return;
                    };
                    let moved = changes.modified_at.is_some();
                    merge_summary(&mut rows[at], &changes);
                    if moved {
                        let held = rows.remove(at);
                        rows.insert(0, held);
                    }
                    record.sessions = rows.into_iter().collect();
                });
            }
            ServerEvent::AgentsChanged(agents) => Self::set_agents(store, server, agents),
        }
    }

    pub fn note_turn(store: &mut Store, server: HostId, chat: &Uri, turn: &str) {
        let session = Hosts::host(store, server).and_then(|host| {
            host.states
                .iter()
                .find(|(_, channel)| {
                    channel
                        .chats
                        .iter()
                        .any(|summary| &summary.resource == chat)
                })
                .map(|(session, _)| session.clone())
        });
        let Some(session) = session else {
            return;
        };
        let key = SessionId {
            host: server,
            session,
        };
        store.update::<Agents>(|agents| {
            agents.latest_turns.insert_mut(key, turn.to_owned());
        });
    }

    pub fn latest_turn(store: &Store, key: &SessionId) -> Option<String> {
        store.get::<Agents>()?.latest_turns.get(key).cloned()
    }

    pub fn live_session(store: &Store, workspace: &SessionId) -> Option<SessionId> {
        let live = Hosts::host_ref(store, workspace.host)?
            .states
            .contains_key(&workspace.session);
        live.then(|| workspace.clone())
    }

    pub fn set_channel(store: &mut Store, key: &SessionId, channel: SessionChannel) {
        let session = key.session.clone();
        Self::update_record(store, key.host, |record| {
            record.states.insert_mut(session, channel);
        });
    }

    pub fn channel(store: &Store, key: &SessionId) -> Option<SessionChannel> {
        Hosts::host_ref(store, key.host)?
            .states
            .get(&key.session)
            .cloned()
    }
}

fn merge_summary(summary: &mut SessionSummary, changes: &PartialSessionSummary) {
    if let Some(title) = &changes.title {
        summary.title = title.clone();
    }
    if let Some(status) = changes.status {
        summary.status = status;
    }
    if let Some(activity) = &changes.activity {
        summary.activity = Some(activity.clone());
    }
    if let Some(project) = &changes.project {
        summary.project = Some(project.clone());
    }
    if let Some(directories) = &changes.working_directories {
        summary.working_directories = Some(directories.clone());
    }
    if let Some(annotations) = &changes.annotations {
        summary.annotations = Some(annotations.clone());
    }
    if let Some(modified) = &changes.modified_at {
        summary.modified_at = modified.clone();
    }
    if let Some(changed) = &changes.changes {
        summary.changes = Some(changed.clone());
    }
}
