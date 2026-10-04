// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

//! The changes WIRE driver: owns the changeset subscribe/poll
//! chains and the session-catalog feed for one session, and applies
//! every landing through the `Changes` collection's public mutation
//! doors. The collection holds NO client, channel, session or poll
//! state — that is all here, in the driver's own row, minted by the
//! session ceremony beside the collection it drives.

use std::sync::Arc;

use crate::higent::ahp_types::actions::StateAction;
use crate::higent::ahp_types::state::{ChangesetFile, ChangesetState, ChangesetStatus};
use crate::higent::{PollChangesetEffect, SubscribeChangesetEffect};
use changesview::hichanges::{
    before_ref_location, ChangeAction, ChangeEntry, ChangeSets, Changes, ChangesStatus,
    DigestedChangeset,
};
use editor::{ResourceLocation, ResourceType};
use himark_ahp_ext_types::history as history_wire;
use imba::command::{Fx, Verb};
use imba::{effect::AnyEffect, store::Store};

/// One folder's wire: the client and AHP session its changeset rides,
/// the claimed channel once the catalog answers, and the poll-loop
/// serial (only the CURRENT loop's landing re-arms — a superseding
/// subscribe bumps it; docs/perf-issue.md §4 measure 5).
#[derive(Clone)]
pub struct FolderWire {
    pub client: crate::higent::Client,
    pub session: crate::higent::SessionUri,
    pub channel: Option<crate::higent::ChannelUri>,
    pub serial: u64,
}

#[derive(Clone)]
struct SessionWire {
    uri: crate::higent::SessionUri,
    client: crate::higent::Client,
}

/// The driver's row — wire state only, keyed beside the collection
/// in the session bundle.
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
    #[cfg(any(test, feature = "test-support"))]
    pub fn seed_folder_for_tests(
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

/// The collection a wire drives — the addressed-refetch consult.
pub fn changes_of(
    store: &Store,
    wire: imba::store::Id<ChangesWire>,
) -> Option<imba::store::Id<ChangeSets>> {
    of(store, wire).map(|row| row.changes)
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
    wire: imba::store::Id<ChangesWire>,
    folders: Vec<ResourceLocation>,
    fx: &mut Fx<'_>,
) {
    if let Some(changes) = of(store, wire).map(|row| row.changes) {
        Changes::adopt_folders(store, changes, &folders);
    }
    for folder in folders {
        ensure_folder(store, wire, folder, fx);
    }
}

pub fn ensure_folder(
    store: &mut Store,
    wire: imba::store::Id<ChangesWire>,
    folder: ResourceLocation,
    fx: &mut Fx<'_>,
) {
    // The folder's own authority names the WIRE that serves it: the
    // client and the AHP session the feeds subscribe through.
    let Some((host, client, session)) =
        crate::higent::client::route_client(store, folder.authority().as_str())
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
                client: client.clone(),
                session: session.clone(),
                channel: None,
                serial: 0,
            },
        );
    });
    // The MODEL half: the set exists (a canvas may have opened it
    // detached already — the door is idempotent and keeps it).
    Changes::ensure_working_set(store, changes, &folder);
    crate::drivers::history::ensure_folder(store, history_wire, &scope, &folder, &client);
    Changes::nudge_folder(store, changes, &folder);

    let directory = uris.uri_of(&folder).into_string();
    fx.push(
        AnyEffect::new(crate::higent::DispatchChatActionEffect {
            client: client.session.clone(),
            channel: session.as_channel(),
            action: StateAction::SessionWorkingDirectorySet(
                crate::higent::ahp_types::actions::SessionWorkingDirectorySetAction { directory },
            ),
        })
        .map(move |result| Verb::Dynamic(Arc::new(Dispatched { result }))),
    );
    let feed_known = of(store, wire)
        .and_then(|row| row.session.as_ref())
        .is_some_and(|feed| feed.uri == session);
    if !feed_known {
        update(store, wire, |row| {
            row.session = Some(SessionWire {
                uri: session.clone(),
                client: client.clone(),
            });
        });
        let landing = scope.clone();
        fx.push(
            AnyEffect::new(crate::higent::SubscribeSessionEffect {
                client: client.session.clone(),
                session,
            })
            .map(
                move |result| {
                    Verb::Dynamic(Arc::new(SessionLanded {
                        home: landing.clone(),
                        wire,
                        result,
                    }))
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
    wire: imba::store::Id<ChangesWire>,
    only: Option<&ResourceLocation>,
    fx: &mut Fx<'_>,
) {
    let Some(row) = of(store, wire) else {
        return;
    };
    let changes = row.changes;
    let riding: Vec<(
        ResourceLocation,
        crate::higent::Client,
        crate::higent::ChannelUri,
    )> = row
        .folders
        .iter()
        .filter(|(folder, _)| only.is_none_or(|only| *folder == only))
        .filter_map(|(folder, held)| {
            Some((folder.clone(), held.client.clone(), held.channel.clone()?))
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
    for (folder, client, channel) in riding {
        fx.push(subscribe_set(wire, folder, client, channel, uris.clone()));
    }
}

/// The session channel's catalog action, routed here from whichever
/// poller drained it.
pub fn adopt_session_catalog(
    store: &mut Store,
    home: &crate::SessionId,
    wire: imba::store::Id<ChangesWire>,
    changed: &crate::higent::ahp_types::actions::SessionChangesetsChangedAction,
    fx: &mut Fx<'_>,
) {
    let entries = digest_catalog(changed.changesets.as_deref().unwrap_or_default());
    subscribe_fresh(store, home, wire, entries, fx);
}

/// Match catalog entries to the folders riding this session's feed
/// that hold no channel yet — the one exact-serve rule plus the
/// lone-folder fallback. Pure, so the claim is unit-testable.
pub fn claim_channels(
    folders: &rpds::HashTrieMapSync<ResourceLocation, FolderWire>,
    session: &crate::higent::SessionUri,
    entries: &[CatalogEntry],
) -> Vec<(
    ResourceLocation,
    crate::higent::Client,
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
                .map(|matched| (folder.clone(), held.client.clone(), matched.uri.clone()))
        })
        .collect()
}

/// Claim catalog channels for the folders riding this session's
/// feed, then subscribe each fresh claim. Pure WIRE bookkeeping —
/// the collection first hears of it when a snapshot lands.
fn subscribe_fresh(
    store: &mut Store,
    home: &crate::SessionId,
    wire: imba::store::Id<ChangesWire>,
    entries: Vec<CatalogEntry>,
    fx: &mut Fx<'_>,
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

    crate::drivers::history::subscribe_fresh(store, home, history_wire, &entries, fx);
    let Some(uris) = of(store, wire).and_then(|row| row.uris.clone()) else {
        return;
    };
    for (folder, client, channel) in fresh {
        fx.push(subscribe_set(wire, folder, client, channel, uris.clone()));
    }
}

/// One folder's changeset subscribe — the snapshot comes home as
/// `SnapshotLanded`, DIGESTED in the landing map on the effect
/// worker; the UI thread receives finished entries
/// (docs/perf-issue.md §2).
fn subscribe_set(
    wire: imba::store::Id<ChangesWire>,
    folder: ResourceLocation,
    client: crate::higent::Client,
    channel: crate::higent::ChannelUri,
    uris: Arc<dyn crate::higent::ResourceUriMap>,
) -> imba::effect::AnyEffect<Verb> {
    AnyEffect::new(SubscribeChangesetEffect {
        client: client.changes.clone(),
        channel,
    })
    .map(move |result| {
        Verb::Dynamic(Arc::new(SnapshotLanded {
            wire,
            result: result.map(|state| digest_state(&*uris, &folder, &state)),
            folder,
        }))
    })
}

/// The driver's own poll relaunch: the next batch of the folder's
/// channel comes home as `PollDrained`, digested on the worker.
/// Bumps the folder's poll serial — the loop this launch starts is
/// the one standing loop.
fn relaunch_poll(
    store: &mut Store,
    wire: imba::store::Id<ChangesWire>,
    folder: &ResourceLocation,
    fx: &mut Fx<'_>,
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
    let client = held.client.clone();
    let serial = held.serial + 1;
    update(store, wire, |row| {
        if let Some(mut held) = row.folders.get(folder).cloned() {
            held.serial = serial;
            row.folders.insert_mut(folder.clone(), held);
        }
    });
    let landing = folder.clone();
    fx.push(
        AnyEffect::new(PollChangesetEffect {
            client: client.changes.clone(),
            channel,
        })
        .map(move |actions| {
            Verb::Dynamic(Arc::new(PollDrained {
                wire,
                serial,
                actions: digest_actions(&*uris, &landing, &actions),
                folder: landing,
            }))
        }),
    );
}

fn relaunch_session_poll(
    store: &Store,
    home: &crate::SessionId,
    wire: imba::store::Id<ChangesWire>,
    fx: &mut Fx<'_>,
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
            client: feed.client.session.clone(),
            session: home.session.clone(),
        })
        .map(move |actions| {
            Verb::Dynamic(Arc::new(SessionPolled {
                home: landing.clone(),
                wire,
                actions,
            }))
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
    fx: &mut Fx<'_>,
) {
    let Some((documents, rearms)) = Changes::take_rearms(store, changes) else {
        return;
    };
    for folder in rearms {
        let authority = folder.authority().clone();
        let prefix = format!("/{}/", folder.path().join("/"));
        documents::diffs::rearm_base_asks(store, documents, &|location| {
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
    result: Result<changesview::hichanges::DigestedChangeset, String>,
}

impl imba::command::DynamicCommand for SnapshotLanded {
    fn id(&self) -> &'static str {
        "changes.snapshot"
    }
    fn name(&self) -> String {
        "Changes Snapshot".to_owned()
    }
    fn perform(&self, store: &mut Store, ui: &imba::UiCtx, fx: &mut Fx<'_>) {
        apply_snapshot(store, ui, self.wire, &self.folder, self.result.clone(), fx);
    }
}

/// The snapshot landing's application — store-level so tests drive
/// it the way the command does.
pub fn apply_snapshot(
    store: &mut Store,
    ui: &imba::UiCtx,
    wire: imba::store::Id<ChangesWire>,
    folder: &ResourceLocation,
    result: Result<changesview::hichanges::DigestedChangeset, String>,
    fx: &mut Fx<'_>,
) {
    let Some(changes) = of(store, wire).map(|row| row.changes) else {
        return; // the state went while the ask flew
    };
    let adopted = result.is_ok();
    Changes::adopt_snapshot(store, changes, folder, result);
    run_tail(ui, store, changes, fx);
    if adopted {
        relaunch_poll(store, wire, folder, fx);
    }
}

/// A changeset poll drained for one folder's set.
struct PollDrained {
    wire: imba::store::Id<ChangesWire>,
    folder: ResourceLocation,
    serial: u64,
    actions: Vec<changesview::hichanges::ChangeAction>,
}

impl imba::command::DynamicCommand for PollDrained {
    fn id(&self) -> &'static str {
        "changes.polled"
    }
    fn name(&self) -> String {
        "Changes Poll".to_owned()
    }
    fn perform(&self, store: &mut Store, ui: &imba::UiCtx, fx: &mut Fx<'_>) {
        apply_poll(
            store,
            ui,
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
pub fn apply_poll(
    store: &mut Store,
    ui: &imba::UiCtx,
    wire: imba::store::Id<ChangesWire>,
    folder: &ResourceLocation,
    serial: u64,
    actions: Vec<changesview::hichanges::ChangeAction>,
    fx: &mut Fx<'_>,
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
        relaunch_poll(store, wire, folder, fx);
    }
}

/// The session channel's own landing — the WIRE is the session's, so
/// its address rides along; the driver it feeds is the id.
struct SessionLanded {
    home: crate::SessionId,
    wire: imba::store::Id<ChangesWire>,
    result: Result<crate::higent::ahp_types::state::SessionState, String>,
}

impl imba::command::DynamicCommand for SessionLanded {
    fn id(&self) -> &'static str {
        "changes.session-landed"
    }
    fn name(&self) -> String {
        "Changes Catalog".to_owned()
    }
    fn perform(&self, store: &mut Store, _ui: &imba::UiCtx, fx: &mut Fx<'_>) {
        let Some(row) = of(store, self.wire) else {
            return;
        };
        let (changes, history_wire) = (row.changes, row.history_wire);
        match &self.result {
            Ok(state) => {
                let entries = digest_catalog(state.changesets.as_deref().unwrap_or_default());
                subscribe_fresh(store, &self.home, self.wire, entries, fx);
                relaunch_session_poll(store, &self.home, self.wire, fx);
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

impl imba::command::DynamicCommand for SessionPolled {
    fn id(&self) -> &'static str {
        "changes.session-polled"
    }
    fn name(&self) -> String {
        "Changes Catalog Update".to_owned()
    }
    fn perform(&self, store: &mut Store, _ui: &imba::UiCtx, fx: &mut Fx<'_>) {
        // The catalog actions apply against windows — the installed
        // shell road carries them; the poll re-arms here either way.
        if let Some(road) = store
            .get::<crate::higent::ChannelActionsRoad>()
            .map(|road| road.0.clone())
        {
            road(store, &self.home, self.actions.clone());
        }
        relaunch_session_poll(store, &self.home, self.wire, fx);
    }
}

pub(crate) struct Dispatched {
    pub(crate) result: Result<(), String>,
}

impl imba::command::DynamicCommand for Dispatched {
    fn id(&self) -> &'static str {
        "changes.dispatched"
    }
    fn name(&self) -> String {
        "Changes Dispatch".to_owned()
    }
    fn perform(&self, _store: &mut Store, _ui: &imba::UiCtx, _fx: &mut Fx<'_>) {
        if let Err(error) = &self.result {
            eprintln!("[hichanges] workingDirectorySet failed: {error}");
        }
    }
}

// ---------------------------------------------------------------
// WIRE → MIRROR digestion: the changeset payload translation, run
// on the effect worker inside the landing maps — the model never
// sees an ahp type (docs/entities.md; the collection speaks its own
// mirrors).

#[derive(serde::Deserialize)]
struct WireSide {
    uri: String,
    #[serde(default)]
    content: Option<WireContent>,
}

#[derive(serde::Deserialize)]
struct WireContent {
    uri: String,
}

#[derive(serde::Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct WireCounts {
    #[serde(default)]
    added: Option<i64>,
    #[serde(default)]
    removed: Option<i64>,
}

pub(crate) fn entry_of(
    uris: &dyn crate::higent::ResourceUriMap,
    folder: &ResourceLocation,
    file: &ChangesetFile,
) -> Option<ChangeEntry> {
    use serde::Deserialize as _;
    // Deserialized BY REFERENCE: the wire `Value` trees are arbitrary
    // and large, and this reads two strings out of them — never pay a
    // deep clone for that (docs/perf-issue.md §2).
    let side = |value: &Option<serde_json::Value>| -> Option<WireSide> {
        value
            .as_ref()
            .and_then(|value| WireSide::deserialize(value).ok())
    };
    let before = side(&file.edit.before);
    let after = side(&file.edit.after);
    let working = after.as_ref().or(before.as_ref()).and_then(|side| {
        uris.location_of(
            &crate::higent::ResourceUri::new(side.uri.as_str()),
            ResourceType::document(),
            folder.authority(),
        )
    })?;
    if !working.path().starts_with(folder.path()) {
        return None;
    }
    let rel: Vec<String> = working.path()[folder.path().len()..].to_vec();
    if rel.is_empty() {
        return None;
    }
    let path: Vec<String> = working.path().to_vec();
    let after_ref = after.and_then(|side| side.content).map(|content| {
        before_ref_location(folder.authority().as_str(), &content.uri, path.clone())
    });
    let before = before.and_then(|side| side.content).map(|content| {
        before_ref_location(folder.authority().as_str(), &content.uri, path.clone())
    });
    let counts: WireCounts = file
        .edit
        .diff
        .as_ref()
        .and_then(|value| WireCounts::deserialize(value).ok())
        .unwrap_or_default();
    Some(ChangeEntry::assembled(
        file.id.clone(),
        rel,
        working,
        before,
        after_ref,
        counts.added,
        counts.removed,
    ))
}

pub fn digest_state(
    uris: &dyn crate::higent::ResourceUriMap,
    folder: &ResourceLocation,
    state: &ChangesetState,
) -> DigestedChangeset {
    DigestedChangeset {
        status: status_of_wire(
            &state.status,
            state.error.as_ref().map(|error| error.message.as_str()),
        ),
        entries: state
            .files
            .iter()
            .filter_map(|file| entry_of(uris, folder, file))
            .collect(),
    }
}

pub fn digest_actions(
    uris: &dyn crate::higent::ResourceUriMap,
    folder: &ResourceLocation,
    actions: &[StateAction],
) -> Vec<ChangeAction> {
    // A full snapshot wholesale replaces the file list, so every file
    // mutation BEFORE the batch's last one is superseded — skip its
    // conversion entirely; only status changes survive in order
    // (docs/perf-issue.md §4 measure 3).
    let last_content = actions
        .iter()
        .rposition(|action| matches!(action, StateAction::ChangesetContentChanged(_)));
    let mut digested = Vec::new();
    for (at, action) in actions.iter().enumerate() {
        let superseded = last_content.is_some_and(|last| at < last);
        match action {
            StateAction::ChangesetContentChanged(content) if !superseded => {
                digested.push(ChangeAction::Content(
                    content
                        .files
                        .iter()
                        .filter_map(|file| entry_of(uris, folder, file))
                        .collect(),
                ));
            }
            StateAction::ChangesetStatusChanged(status) => {
                digested.push(ChangeAction::Status(status_of_wire(
                    &status.status,
                    status.error.as_ref().map(|error| error.message.as_str()),
                )));
            }
            StateAction::ChangesetFileSet(set) if !superseded => {
                digested.push(ChangeAction::FileSet {
                    id: set.file.id.clone(),
                    entry: entry_of(uris, folder, &set.file),
                });
            }
            StateAction::ChangesetFileRemoved(removed) if !superseded => {
                digested.push(ChangeAction::FileRemoved(removed.file_id.clone()));
            }
            StateAction::ChangesetCleared(_) if !superseded => {
                digested.push(ChangeAction::Cleared);
            }
            _ => {}
        }
    }
    digested
}

/// WIRE → MIRROR for the changeset status.
pub(crate) fn status_of_wire(status: &ChangesetStatus, error: Option<&str>) -> ChangesStatus {
    match status {
        ChangesetStatus::Computing => ChangesStatus::Computing,
        ChangesetStatus::Ready => ChangesStatus::Ready,
        ChangesetStatus::Error => {
            ChangesStatus::Error(error.unwrap_or("changeset error").to_owned())
        }
        // A status from a newer protocol: not an error, not
        // provably ready — keep the chip spinning.
        ChangesetStatus::Unknown(_) => ChangesStatus::Computing,
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CatalogEntry {
    pub uri: crate::higent::ChannelUri,
    pub description: Option<String>,

    pub kind: String,
}

pub fn digest_catalog(
    changesets: &[crate::higent::ahp_types::state::Changeset],
) -> Vec<CatalogEntry> {
    changesets
        .iter()
        .filter(|entry| {
            (entry.change_kind == "uncommitted"
                || entry.change_kind == history_wire::HISTORY_CHANGE_KIND)
                && !entry.uri_template.contains('{')
        })
        .map(|entry| CatalogEntry {
            uri: crate::higent::ChannelUri::new(entry.uri_template.clone()),
            description: entry.description.clone(),
            kind: entry.change_kind.clone(),
        })
        .collect()
}

pub(crate) fn entry_serves(folder: &ResourceLocation, entry: &CatalogEntry) -> bool {
    let abs = format!("/{}", folder.path().join("/"));
    entry.description.as_deref() == Some(abs.as_str()) || entry.uri.as_str().ends_with(&abs)
}

/// The batch-tail changes lane: drain the model's refetch asks onto
/// the wire — a clean collection costs a map read.
pub fn sync(store: &mut Store, wire: imba::store::Id<ChangesWire>, fx: &mut Fx<'_>) {
    let Some(changes) = of(store, wire).map(|row| row.changes) else {
        return;
    };
    if !Changes::owes_refetch(store, changes) {
        return;
    }
    for only in Changes::take_refetch_asks(store, changes) {
        refetch(store, wire, only.as_ref(), fx);
    }
}
