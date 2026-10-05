// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

//! The resident sessions subscription: every registered host is
//! connected, its sessions listed, and its event stream drained —
//! at all times, not while some view happens to be open. Views read
//! the catalog and dress it; this lane keeps the catalog true. It
//! runs at every batch tail and is idempotent: in-flight work is
//! marked (`Connecting` on the record, a drain token in `HostFeeds`),
//! so a quiet pass costs map reads.

use ahp_wire::client::{HostId, Servers, ServerEvent, SessionsPage, RootInfo};
use ahp_wire::effects::{ConnectServerEffect, ListSessionsEffect, PollServerEffect};
use imba::command::{DynamicCommand, DynamicOnceCommand, Fx, Verb};
use imba::effect::{AnyEffect, CancellationToken};
use imba::store::Store;
use imba::ui::UiCtx;

use super::agents::Agents;
use super::state::HostStatus;

/// The subscription's own bookkeeping. Connection status lives on the
/// host RECORD (`HostStatus` — it is catalog truth the views show);
/// what lives here is only the plumbing: which hosts have a drain in
/// flight, which have their session list already running.
#[derive(Clone, Default)]
pub struct HostFeeds {
    drains: rpds::HashTrieMapSync<HostId, CancellationToken>,
    listed: rpds::HashTrieSetSync<HostId>,
}

impl HostFeeds {
    fn update(store: &mut Store, mutate: impl FnOnce(&mut HostFeeds)) {
        let mut feeds = store.take::<HostFeeds>().unwrap_or_default();
        mutate(&mut feeds);
        store.put(feeds);
    }
}

/// The lane: connect what is new, list and drain what is connected.
/// A `Failed` host is NOT retried here — a lane that reconnects at
/// every batch tail hammers a dead server; retries ride an explicit
/// nudge (`RetryHosts`: the drawer opening, an add-host submit).
pub fn sync_sessions_lane(store: &mut Store, fx: &mut Fx<'_>) {
    for (server, record) in Agents::list(store) {
        match record.status {
            HostStatus::Idle => connect(store, server, fx),
            HostStatus::Connected => drain(store, server, fx),
            HostStatus::Connecting | HostStatus::Failed(_) => {}
        }
    }
}

/// The explicit retry nudge, as a verb so a shell can fire it from
/// any road: every `Failed` host gets one fresh connect attempt.
pub struct RetryHosts;

impl DynamicCommand for RetryHosts {
    fn id(&self) -> &'static str {
        "sessions.retry-hosts"
    }

    fn name(&self) -> String {
        "Reconnect failed hosts".to_owned()
    }

    fn perform(&self, store: &mut Store, _ui: &UiCtx, fx: &mut Fx<'_>) {
        for (server, record) in Agents::list(store) {
            if matches!(record.status, HostStatus::Failed(_)) {
                connect(store, server, fx);
            }
        }
    }
}

fn connect(store: &mut Store, server: HostId, fx: &mut Fx<'_>) {
    // One connect in flight, ever: the lane and the nudges may all
    // ask in one batch, and the first ask marks the record.
    if matches!(
        Agents::record(store, server).map(|record| record.status),
        Some(HostStatus::Connecting) | Some(HostStatus::Connected)
    ) {
        return;
    }
    let Some(client) = Servers::client(store, server) else {
        // A host row with no wire seat is not pending, it is
        // unreachable — mark it so. Left Idle it would read
        // "connecting…" forever (seats register before their rows are
        // seeded, so none arrives later; an add-host retry re-enters
        // here with the seat in place).
        Agents::set_status(store, server, HostStatus::Failed("unregistered".to_owned()));
        return;
    };
    Agents::set_status(store, server, HostStatus::Connecting);
    fx.push(
        AnyEffect::new(ConnectServerEffect { client: client.session.clone() })
            .map(move |result| Verb::Once(Box::new(Connected { server, result }))),
    );
}

fn drain(store: &mut Store, server: HostId, fx: &mut Fx<'_>) {
    let Some(client) = Servers::client(store, server) else {
        return;
    };
    let feeds = store.get::<HostFeeds>().cloned().unwrap_or_default();
    if !feeds.listed.contains(&server) {
        HostFeeds::update(store, |feeds| {
            feeds.listed.insert_mut(server);
        });
        list_page(store, server, None, fx);
    }
    if feeds.drains.get(&server).is_none() {
        let token = fx.push(
            AnyEffect::new(PollServerEffect { client: client.session.clone() })
                .map(move |events| Verb::Once(Box::new(Drained { server, events }))),
        );
        HostFeeds::update(store, |feeds| {
            feeds.drains.insert_mut(server, token);
        });
    }
}

fn list_page(store: &mut Store, server: HostId, cursor: Option<String>, fx: &mut Fx<'_>) {
    let Some(client) = Servers::client(store, server) else {
        return;
    };
    let first = cursor.is_none();
    fx.push(
        AnyEffect::new(ListSessionsEffect { client: client.session.clone(), cursor }).map(
            move |result| {
                Verb::Once(Box::new(Listed {
                    server,
                    first,
                    result,
                }))
            },
        ),
    );
}

struct Connected {
    server: HostId,
    result: Result<RootInfo, String>,
}

impl DynamicOnceCommand for Connected {
    fn perform(self: Box<Self>, store: &mut Store, _ui: &UiCtx, _fx: &mut Fx<'_>) {
        match self.result {
            Ok(info) => {
                Agents::set_agents(store, self.server, info.agents);
                Agents::set_status(store, self.server, HostStatus::Connected);
                // The tail lane of THIS batch sees Connected and
                // starts the listing and the drain.
            }
            Err(error) => {
                Agents::set_status(store, self.server, HostStatus::Failed(error));
            }
        }
    }
}

struct Listed {
    server: HostId,
    first: bool,
    result: Result<SessionsPage, String>,
}

impl DynamicOnceCommand for Listed {
    fn perform(self: Box<Self>, store: &mut Store, _ui: &UiCtx, fx: &mut Fx<'_>) {
        match self.result {
            Ok(page) => {
                let next = page.next_cursor.clone();
                Agents::add_sessions(store, self.server, page.sessions, self.first);
                if next.is_some() {
                    list_page(store, self.server, next, fx);
                }
            }
            Err(error) => {
                eprintln!("[higent] listSessions failed: {error}");
                // Let a later pass (or retry nudge) list afresh.
                HostFeeds::update(store, |feeds| {
                    feeds.listed.remove_mut(&self.server);
                });
            }
        }
    }
}

struct Drained {
    server: HostId,
    events: Vec<ServerEvent>,
}

impl DynamicOnceCommand for Drained {
    fn perform(self: Box<Self>, store: &mut Store, _ui: &UiCtx, _fx: &mut Fx<'_>) {
        for event in self.events {
            Agents::apply_event(store, self.server, event);
        }
        // The drain is a one-shot long poll: clear the token and the
        // tail lane re-arms it.
        HostFeeds::update(store, |feeds| {
            feeds.drains.remove_mut(&self.server);
        });
    }
}

/// One host's explicit connect nudge — the drawer fires it when the
/// user clicks a failed (or not-yet-tried) host row.
pub struct ConnectHost(pub HostId);

impl DynamicCommand for ConnectHost {
    fn id(&self) -> &'static str {
        "sessions.connect-host"
    }

    fn name(&self) -> String {
        "Connect host".to_owned()
    }

    fn perform(&self, store: &mut Store, _ui: &UiCtx, fx: &mut Fx<'_>) {
        connect(store, self.0, fx);
    }
}

/// The session MIRROR's lifeline: one standing long-poll per entered
/// session, draining the wire's session-channel feed into the local
/// mirror (`channel::apply_channel_actions`) and re-arming itself.
/// The chain's one legitimate end is a disposed session.
pub fn relaunch_session_poll(store: &Store, key: ahp_wire::SessionId, fx: &mut Fx<'_>) {
    let Some(client) = Servers::client(store, key.host) else {
        return;
    };
    fx.push(
        AnyEffect::new(ahp_wire::effects::PollSessionEffect {
            client: client.session.clone(),
            session: key.session.clone(),
        })
        .map(move |actions| {
            Verb::Once(Box::new(SessionActionsLanded {
                key: key.clone(),
                actions,
            }))
        }),
    );
}

struct SessionActionsLanded {
    key: ahp_wire::SessionId,
    actions: Vec<ahp_types::actions::StateAction>,
}

impl DynamicOnceCommand for SessionActionsLanded {
    fn perform(self: Box<Self>, store: &mut Store, _ui: &UiCtx, fx: &mut Fx<'_>) {
        super::channel::apply_channel_actions(store, &self.key, &self.actions, fx);
        if Agents::live_session(store, &self.key).is_some() {
            relaunch_session_poll(store, self.key, fx);
        } else {
            // Anything else parked here is a mirror frozen for good,
            // so the retirement leaves a trace.
            eprintln!(
                "[higent] session poll retired: {} is no longer live",
                self.key.session.as_str()
            );
        }
    }
}
