// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

//! The changes WIRE driver: owns the changeset subscribe/poll
//! chains and the session-catalog feed for one family, and applies
//! every landing through the `Changes` collection's public mutation
//! doors. The collection holds NO seat, channel, session or poll
//! state — that is all here, in the driver's own row, minted by the
//! family ceremony beside the collection it drives.

use std::sync::Arc;

use crate::hichanges::{
    digest_actions, digest_catalog, digest_state, entry_serves, CatalogEntry, ChangeSets, Changes,
};
use crate::higent::ahp_types::actions::StateAction;
use crate::higent::{AhpServer, PollChangesetEffect, SubscribeChangesetEffect};
use crate::{AppCommand, ResourceLocation};
use imba::{effect::AnyEffect, store::Store};

/// One folder's wire: the seat and AHP session its changeset rides,
/// the claimed channel once the catalog answers, and the poll-loop
/// serial (only the CURRENT loop's landing re-arms — a superseding
/// subscribe bumps it; docs/perf-issue.md §4 measure 5).
#[derive(Clone)]
pub(crate) struct FolderWire {
    pub(crate) seat: Arc<dyn AhpServer>,
    pub(crate) session: crate::higent::SessionUri,
    pub(crate) channel: Option<crate::higent::ChannelUri>,
    pub(crate) serial: u64,
}

#[derive(Clone)]
struct SessionWire {
    uri: crate::higent::SessionUri,
    seat: Arc<dyn AhpServer>,
}

/// The driver's row — wire state only, keyed beside the collection
/// in the family bundle.
#[derive(Clone)]
pub struct ChangesWire {
    changes: imba::store::Id<ChangeSets>,
    history_wire: imba::store::Id<crate::drivers::history::HistoryWire>,

    /// The host's location↔uri translation, stamped by the ceremony
    /// (mint, or the heal after a placeholder rekey).
    uris: Option<Arc<dyn crate::higent::ResourceUriMap>>,

    session: Option<SessionWire>,
    folders: rpds::HashTrieMapSync<ResourceLocation, FolderWire>,
}

impl ChangesWire {
    /// Test seam: seed a folder's wire the way `ensure_folder` would
    /// after routing — the unit tests drive landings without a host.
    #[cfg(test)]
    pub(crate) fn seed_folder_for_tests(
        store: &mut Store,
        wire: imba::store::Id<ChangesWire>,
        folder: ResourceLocation,
        held: FolderWire,
    ) {
        update(store, wire, |row| {
            row.folders.insert_mut(folder, held);
        });
    }

    pub fn wired(
        changes: imba::store::Id<ChangeSets>,
        history_wire: imba::store::Id<crate::drivers::history::HistoryWire>,
        uris: Option<Arc<dyn crate::higent::ResourceUriMap>>,
    ) -> Self {
        Self {
            changes,
            history_wire,
            uris,
            session: None,
            folders: rpds::HashTrieMapSync::new_sync(),
        }
    }

    pub(crate) fn stamp_uris(
        store: &mut Store,
        wire: imba::store::Id<ChangesWire>,
        uris: &Arc<dyn crate::higent::ResourceUriMap>,
    ) {
        update(store, wire, |row| row.uris = Some(Arc::clone(uris)));
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.session.is_none() && self.folders.is_empty()
    }
}

fn of(store: &Store, wire: imba::store::Id<ChangesWire>) -> Option<&ChangesWire> {
    store.entity(wire)
}

/// Mutate in place; a gone driver takes no write — never minted from
/// `Default` (a dangling id must not resurrect).
fn update(
    store: &mut Store,
    wire: imba::store::Id<ChangesWire>,
    mutate: impl FnOnce(&mut ChangesWire),
) {
    let Some(mut row) = store.entity::<ChangesWire>(wire).cloned() else {
        return;
    };
    mutate(&mut row);
    store.put_entity(wire, row);
}

/// Attach every folder of a session to its collection's wire.
pub fn ensure(
    store: &mut Store,
    window: crate::WindowId,
    wire: imba::store::Id<ChangesWire>,
    folders: Vec<ResourceLocation>,
    fx: &mut crate::AppFx<'_>,
) {
    for folder in folders {
        ensure_folder(store, window, wire, folder, fx);
    }
}

pub fn ensure_folder(
    store: &mut Store,
    window: crate::WindowId,
    wire: imba::store::Id<ChangesWire>,
    folder: ResourceLocation,
    fx: &mut crate::AppFx<'_>,
) {
    // The folder's own authority names the WIRE that serves it: the
    // seat and the AHP session the feeds subscribe through.
    let Some((host, seat, session)) =
        crate::higent::seat::route_seat(store, folder.authority().as_str())
    else {
        return;
    };
    let scope = crate::SessionId {
        host,
        session: session.clone(),
    };
    let Some(row) = of(store, wire) else {
        return;
    };
    if row.folders.contains_key(&folder) {
        return;
    }
    let Some(uris) = row.uris.clone() else {
        eprintln!("[hichanges] folder NOT attached: no uri map stamped on the driver");
        return;
    };
    let (changes, history_wire) = (row.changes, row.history_wire);
    update(store, wire, |row| {
        row.folders.insert_mut(
            folder.clone(),
            FolderWire {
                seat: seat.clone(),
                session: session.clone(),
                channel: None,
                serial: 0,
            },
        );
    });
    // The MODEL half: the set exists (a canvas may have opened it
    // detached already — the door is idempotent and keeps it).
    Changes::ensure_working_set(store, changes, &folder);
    crate::drivers::history::ensure_folder(store, history_wire, &scope, &folder, &seat);
    Changes::nudge_folder(store, changes, &folder);

    let directory = uris.uri_of(&folder).into_string();
    fx.push(
        AnyEffect::new(crate::higent::DispatchChatActionEffect {
            seat: seat.clone(),
            channel: session.as_channel(),
            action: StateAction::SessionWorkingDirectorySet(
                crate::higent::ahp_types::actions::SessionWorkingDirectorySetAction { directory },
            ),
        })
        .map(move |result| AppCommand::Dynamic(window, Arc::new(Dispatched { result }))),
    );
    let feed_known = of(store, wire)
        .and_then(|row| row.session.as_ref())
        .is_some_and(|feed| feed.uri == session);
    if !feed_known {
        update(store, wire, |row| {
            row.session = Some(SessionWire {
                uri: session.clone(),
                seat: seat.clone(),
            });
        });
        let landing = scope.clone();
        fx.push(
            AnyEffect::new(crate::higent::SubscribeSessionEffect { seat, session }).map(
                move |result| {
                    AppCommand::Dynamic(
                        window,
                        Arc::new(SessionLanded {
                            home: landing.clone(),
                            wire,
                            result,
                        }),
                    )
                },
            ),
        );
    }
}

/// Re-ask the standing working-copy changesets whole (the refresh
/// chip): mark computing, re-subscribe — the snapshot supersedes the
/// old poll loop by serial.
pub fn refetch(
    store: &mut Store,
    window: crate::WindowId,
    wire: imba::store::Id<ChangesWire>,
    only: Option<&ResourceLocation>,
    fx: &mut crate::AppFx<'_>,
) {
    let Some(row) = of(store, wire) else {
        return;
    };
    let changes = row.changes;
    let riding: Vec<(
        ResourceLocation,
        Arc<dyn AhpServer>,
        crate::higent::ChannelUri,
    )> = row
        .folders
        .iter()
        .filter(|(folder, _)| only.is_none_or(|only| *folder == only))
        .filter_map(|(folder, held)| {
            Some((folder.clone(), held.seat.clone(), held.channel.clone()?))
        })
        .collect();
    let Some(uris) = row.uris.clone() else {
        return;
    };
    if riding.is_empty() {
        return;
    }
    for (folder, _, _) in &riding {
        Changes::mark_folder_computing(store, changes, folder);
        Changes::nudge_folder(store, changes, folder);
    }
    for (folder, seat, channel) in riding {
        fx.push(subscribe_set(
            window,
            wire,
            folder,
            seat,
            channel,
            uris.clone(),
        ));
    }
}

/// The session channel's catalog action, routed here from whichever
/// poller drained it.
pub(crate) fn adopt_session_catalog(
    store: &mut Store,
    window: crate::WindowId,
    home: &crate::SessionId,
    wire: imba::store::Id<ChangesWire>,
    changed: &crate::higent::ahp_types::actions::SessionChangesetsChangedAction,
    fx: &mut crate::AppFx<'_>,
) {
    let entries = digest_catalog(changed.changesets.as_deref().unwrap_or_default());
    subscribe_fresh(store, window, home, wire, entries, fx);
}

/// Match catalog entries to the folders riding this session's feed
/// that hold no channel yet — the one exact-serve rule plus the
/// lone-folder fallback. Pure, so the claim is unit-testable.
pub(crate) fn claim_channels(
    folders: &rpds::HashTrieMapSync<ResourceLocation, FolderWire>,
    session: &crate::higent::SessionUri,
    entries: &[CatalogEntry],
) -> Vec<(
    ResourceLocation,
    Arc<dyn AhpServer>,
    crate::higent::ChannelUri,
)> {
    let changesets: Vec<&CatalogEntry> = entries
        .iter()
        .filter(|entry| entry.kind == "uncommitted")
        .collect();
    let lone_folder = folders
        .iter()
        .filter(|(_, held)| held.session == *session)
        .count()
        == 1;
    folders
        .iter()
        .filter(|(_, held)| held.session == *session && held.channel.is_none())
        .filter_map(|(folder, held)| {
            changesets
                .iter()
                .find(|candidate| entry_serves(folder, candidate))
                .or_else(|| (lone_folder && changesets.len() == 1).then(|| &changesets[0]))
                .map(|matched| (folder.clone(), held.seat.clone(), matched.uri.clone()))
        })
        .collect()
}

/// Claim catalog channels for the folders riding this session's
/// feed, then subscribe each fresh claim. Pure WIRE bookkeeping —
/// the collection first hears of it when a snapshot lands.
fn subscribe_fresh(
    store: &mut Store,
    window: crate::WindowId,
    home: &crate::SessionId,
    wire: imba::store::Id<ChangesWire>,
    entries: Vec<CatalogEntry>,
    fx: &mut crate::AppFx<'_>,
) {
    let Some(row) = of(store, wire) else {
        return;
    };
    let (changes, history_wire) = (row.changes, row.history_wire);
    let session = home.session.clone();
    let fresh = claim_channels(&row.folders, &session, &entries);
    update(store, wire, |row| {
        for (folder, _, channel) in &fresh {
            if let Some(mut held) = row.folders.get(folder).cloned() {
                held.channel = Some(channel.clone());
                row.folders.insert_mut(folder.clone(), held);
            }
        }
    });
    Changes::nudge_all_in(store, changes);

    crate::drivers::history::subscribe_fresh(store, window, home, history_wire, &entries, fx);
    let Some(uris) = of(store, wire).and_then(|row| row.uris.clone()) else {
        return;
    };
    for (folder, seat, channel) in fresh {
        fx.push(subscribe_set(
            window,
            wire,
            folder,
            seat,
            channel,
            uris.clone(),
        ));
    }
}

/// One folder's changeset subscribe — the snapshot comes home as
/// `SnapshotLanded`, DIGESTED in the landing map on the effect
/// worker; the UI thread receives finished entries
/// (docs/perf-issue.md §2).
fn subscribe_set(
    window: crate::WindowId,
    wire: imba::store::Id<ChangesWire>,
    folder: ResourceLocation,
    seat: Arc<dyn AhpServer>,
    channel: crate::higent::ChannelUri,
    uris: Arc<dyn crate::higent::ResourceUriMap>,
) -> crate::AppEffect {
    AnyEffect::new(SubscribeChangesetEffect { seat, channel }).map(move |result| {
        AppCommand::Dynamic(
            window,
            Arc::new(SnapshotLanded {
                wire,
                result: result.map(|state| digest_state(&*uris, &folder, &state)),
                folder,
            }),
        )
    })
}

/// The driver's own poll relaunch: the next batch of the folder's
/// channel comes home as `PollDrained`, digested on the worker.
/// Bumps the folder's poll serial — the loop this launch starts is
/// the one standing loop.
fn relaunch_poll(
    store: &mut Store,
    window: crate::WindowId,
    wire: imba::store::Id<ChangesWire>,
    folder: &ResourceLocation,
    fx: &mut crate::AppFx<'_>,
) {
    let Some(row) = of(store, wire) else {
        return;
    };
    let Some(uris) = row.uris.clone() else {
        return;
    };
    let Some(held) = row.folders.get(folder) else {
        return;
    };
    let Some(channel) = held.channel.clone() else {
        return;
    };
    let seat = held.seat.clone();
    let serial = held.serial + 1;
    update(store, wire, |row| {
        if let Some(mut held) = row.folders.get(folder).cloned() {
            held.serial = serial;
            row.folders.insert_mut(folder.clone(), held);
        }
    });
    let landing = folder.clone();
    fx.push(
        AnyEffect::new(PollChangesetEffect { seat, channel }).map(move |actions| {
            AppCommand::Dynamic(
                window,
                Arc::new(PollDrained {
                    wire,
                    serial,
                    actions: digest_actions(&*uris, &landing, &actions),
                    folder: landing,
                }),
            )
        }),
    );
}

fn relaunch_session_poll(
    store: &Store,
    window: crate::WindowId,
    home: &crate::SessionId,
    wire: imba::store::Id<ChangesWire>,
    fx: &mut crate::AppFx<'_>,
) {
    let Some(feed) = of(store, wire)
        .and_then(|row| row.session.clone())
        .filter(|feed| feed.uri == home.session)
    else {
        return;
    };
    let landing = home.clone();
    fx.push(
        AnyEffect::new(crate::higent::PollSessionEffect {
            seat: feed.seat,
            session: home.session.clone(),
        })
        .map(move |actions| {
            AppCommand::Dynamic(
                window,
                Arc::new(SessionPolled {
                    home: landing.clone(),
                    wire,
                    actions,
                }),
            )
        }),
    );
}

/// The landing's application tail: the stripe bases under the
/// folders the mutation touched re-ask. The model NOTES the folders
/// (its own row schema); the driver drains and runs the asks.
fn run_tail(
    ui: &imba::UiCtx,
    store: &mut Store,
    changes: imba::store::Id<ChangeSets>,
    fx: &mut crate::AppFx<'_>,
) {
    let Some((documents, rearms)) = Changes::take_rearms(store, changes) else {
        return;
    };
    for folder in rearms {
        let authority = folder.authority().clone();
        let prefix = format!("/{}/", folder.path().join("/"));
        crate::rearm_base_asks(store, documents, &|location| {
            location.authority() == &authority
                && format!("/{}", location.path().join("/")).starts_with(&prefix)
        });
        crate::sync_stripe_bases(store, documents, ui, fx);
    }
}

/// A changeset subscribe answered for one folder's set.
struct SnapshotLanded {
    wire: imba::store::Id<ChangesWire>,
    folder: ResourceLocation,
    result: Result<crate::hichanges::DigestedChangeset, String>,
}

impl crate::DynamicCommand for SnapshotLanded {
    fn id(&self) -> &'static str {
        "changes.snapshot"
    }
    fn name(&self) -> String {
        "Changes Snapshot".to_owned()
    }
    fn perform(
        &self,
        app: &mut crate::Application,
        store: &mut Store,
        window: crate::WindowId,
        fx: &mut crate::AppFx<'_>,
    ) {
        apply_snapshot(
            store,
            &app.ui_ctx(),
            window,
            self.wire,
            &self.folder,
            self.result.clone(),
            fx,
        );
    }
}

/// The snapshot landing's application — store-level so tests drive
/// it the way the command does.
pub(crate) fn apply_snapshot(
    store: &mut Store,
    ui: &imba::UiCtx,
    window: crate::WindowId,
    wire: imba::store::Id<ChangesWire>,
    folder: &ResourceLocation,
    result: Result<crate::hichanges::DigestedChangeset, String>,
    fx: &mut crate::AppFx<'_>,
) {
    let Some(changes) = of(store, wire).map(|row| row.changes) else {
        return; // the family went while the ask flew
    };
    let adopted = result.is_ok();
    Changes::adopt_snapshot(store, changes, folder, result);
    run_tail(ui, store, changes, fx);
    if adopted {
        relaunch_poll(store, window, wire, folder, fx);
    }
}

/// A changeset poll drained for one folder's set.
struct PollDrained {
    wire: imba::store::Id<ChangesWire>,
    folder: ResourceLocation,
    serial: u64,
    actions: Vec<crate::hichanges::ChangeAction>,
}

impl crate::DynamicCommand for PollDrained {
    fn id(&self) -> &'static str {
        "changes.polled"
    }
    fn name(&self) -> String {
        "Changes Poll".to_owned()
    }
    fn perform(
        &self,
        app: &mut crate::Application,
        store: &mut Store,
        window: crate::WindowId,
        fx: &mut crate::AppFx<'_>,
    ) {
        apply_poll(
            store,
            &app.ui_ctx(),
            window,
            self.wire,
            &self.folder,
            self.serial,
            self.actions.clone(),
            fx,
        );
    }
}

/// The poll landing's application — store-level so tests drive it
/// the way the command does.
#[allow(clippy::too_many_arguments)]
pub(crate) fn apply_poll(
    store: &mut Store,
    ui: &imba::UiCtx,
    window: crate::WindowId,
    wire: imba::store::Id<ChangesWire>,
    folder: &ResourceLocation,
    serial: u64,
    actions: Vec<crate::hichanges::ChangeAction>,
    fx: &mut crate::AppFx<'_>,
) {
    let Some(row) = of(store, wire) else {
        return;
    };
    let changes = row.changes;
    let current = row
        .folders
        .get(folder)
        .is_some_and(|held| held.serial == serial);
    Changes::fold_folder(store, changes, folder, actions);
    run_tail(ui, store, changes, fx);
    // Only the CURRENT loop re-arms: a superseding subscribe bumped
    // the serial and owns the next poll. A stale landing still folds
    // its batch — a drained action is never dropped — but it does
    // not multiply loops (docs/perf-issue.md §2).
    if current {
        relaunch_poll(store, window, wire, folder, fx);
    }
}

/// The session channel's own landing — the WIRE is the session's, so
/// its address rides along; the driver it feeds is the id.
struct SessionLanded {
    home: crate::SessionId,
    wire: imba::store::Id<ChangesWire>,
    result: Result<crate::higent::ahp_types::state::SessionState, String>,
}

impl crate::DynamicCommand for SessionLanded {
    fn id(&self) -> &'static str {
        "changes.session-landed"
    }
    fn name(&self) -> String {
        "Changes Catalog".to_owned()
    }
    fn perform(
        &self,
        _app: &mut crate::Application,
        store: &mut Store,
        window: crate::WindowId,
        fx: &mut crate::AppFx<'_>,
    ) {
        let Some(row) = of(store, self.wire) else {
            return;
        };
        let (changes, history_wire) = (row.changes, row.history_wire);
        match &self.result {
            Ok(state) => {
                let entries = digest_catalog(state.changesets.as_deref().unwrap_or_default());
                subscribe_fresh(store, window, &self.home, self.wire, entries, fx);
                relaunch_session_poll(store, window, &self.home, self.wire, fx);
            }
            Err(error) => {
                eprintln!("[hichanges] session subscribe failed: {error}");
                let riding: Vec<ResourceLocation> = of(store, self.wire)
                    .map(|row| {
                        row.folders
                            .iter()
                            .filter(|(_, held)| held.session == self.home.session)
                            .map(|(folder, _)| folder.clone())
                            .collect()
                    })
                    .unwrap_or_default();
                for folder in riding {
                    Changes::fold_error(store, changes, &folder, error);
                }
                Changes::nudge_all_in(store, changes);
                crate::drivers::history::session_failed(
                    store,
                    history_wire,
                    &self.home.session,
                    error,
                );
            }
        }
    }
}

/// The pump's landing: this poller and higent's drain the SAME wire
/// feed; the winner takes the whole batch, so hand every action kind
/// to the shared application, not just the changeset ones.
struct SessionPolled {
    home: crate::SessionId,
    wire: imba::store::Id<ChangesWire>,
    actions: Vec<StateAction>,
}

impl crate::DynamicCommand for SessionPolled {
    fn id(&self) -> &'static str {
        "changes.session-polled"
    }
    fn name(&self) -> String {
        "Changes Catalog Update".to_owned()
    }
    fn perform(
        &self,
        _app: &mut crate::Application,
        store: &mut Store,
        window: crate::WindowId,
        fx: &mut crate::AppFx<'_>,
    ) {
        crate::higent::apply_channel_actions(store, window, &self.home, &self.actions, fx);
        relaunch_session_poll(store, window, &self.home, self.wire, fx);
    }
}

pub(crate) struct Dispatched {
    pub(crate) result: Result<(), String>,
}

impl crate::DynamicCommand for Dispatched {
    fn id(&self) -> &'static str {
        "changes.dispatched"
    }
    fn name(&self) -> String {
        "Changes Dispatch".to_owned()
    }
    fn perform(
        &self,
        _app: &mut crate::Application,
        _store: &mut Store,
        _window: crate::WindowId,
        _fx: &mut crate::AppFx<'_>,
    ) {
        if let Err(error) = &self.result {
            eprintln!("[hichanges] workingDirectorySet failed: {error}");
        }
    }
}
