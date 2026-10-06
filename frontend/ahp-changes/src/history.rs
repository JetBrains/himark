// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

//! The history WIRE driver: owns the per-folder history channel
//! subscribe/poll chains, the commit-changeset fetches, and the
//! grow/commit dispatch asks — and applies every landing through
//! the `History` collection's public mutation doors, in the
//! collection's own MIRROR types. The wire vocabulary (clients,
//! channels, `history_wire` payloads) lives here and never in the
//! model.

use std::sync::Arc;

use crate::changes::{digest_actions, digest_state, entry_serves, CatalogEntry};
use ahp_types::actions::StateAction;
use ahp_wire::effects::{
    DispatchChatActionEffect, PollChangesetEffect, SubscribeChangesetEffect, SubscribeHistoryEffect,
};
use changesview::hichanges::Changes;
use changesview::hihistory::{
    CommitAuthor, CommitInfo, CommitRef, History, HistoryDelta, HistoryHead, HistorySnapshot,
};
use editor::location::ResourceLocation;
use himark_ahp_ext_types::history as history_wire;
use imba::command::{Fx, Verb};
use imba::{effect::AnyEffect, store::Store};

/// One folder's wire: the client and AHP session its history rides,
/// the claimed channel once the catalog answers, and the commit →
/// changeset-channel map harvested from the landed commits (the
/// model's rows never carry wire uris).
#[derive(Clone)]
struct FolderWire {
    client: ahp_wire::client::Client,
    session: ahp_wire::client::SessionUri,
    channel: Option<ahp_wire::client::ChannelUri>,
    commit_channels: rpds::HashTrieMapSync<String, ahp_wire::client::ChannelUri>,
}

/// The driver's row — wire state only, minted by the ceremony
/// beside the collection it drives.
#[derive(Clone)]
pub struct HistoryWire {
    history: imba::store::Id<History>,
    changes: imba::store::Id<changesview::hichanges::ChangeSets>,
    uris: Option<Arc<dyn ahp_wire::client::ResourceUriMap>>,
    folders: rpds::HashTrieMapSync<ResourceLocation, FolderWire>,
}

impl HistoryWire {
    pub fn wired(
        history: imba::store::Id<History>,
        changes: imba::store::Id<changesview::hichanges::ChangeSets>,
        uris: Option<Arc<dyn ahp_wire::client::ResourceUriMap>>,
    ) -> Self {
        Self {
            history,
            changes,
            uris,
            folders: rpds::HashTrieMapSync::new_sync(),
        }
    }

    pub fn stamp_uris(
        store: &mut Store,
        wire: imba::store::Id<HistoryWire>,
        uris: &Arc<dyn ahp_wire::client::ResourceUriMap>,
    ) {
        update(store, wire, |row| row.uris = Some(Arc::clone(uris)));
    }

    pub fn is_empty(&self) -> bool {
        self.folders.is_empty()
    }
}

fn of(store: &Store, wire: imba::store::Id<HistoryWire>) -> Option<&HistoryWire> {
    store.entity(wire)
}

/// Mutate in place; a gone driver takes no write — never minted
/// from `Default`.
fn update(
    store: &mut Store,
    wire: imba::store::Id<HistoryWire>,
    mutate: impl FnOnce(&mut HistoryWire),
) {
    let Some(mut row) = store.entity::<HistoryWire>(wire).cloned() else {
        return;
    };
    mutate(&mut row);
    store.put_entity(wire, row);
}

/// Attach a folder: the AHP session is the folder's WIRE, carried
/// from the changes driver's route; the model gets an empty row.
pub(crate) fn ensure_folder(
    store: &mut Store,
    wire: imba::store::Id<HistoryWire>,
    scope: &ahp_wire::SessionId,
    folder: &ResourceLocation,
    client: &ahp_wire::client::Client,
) {
    let Some(row) = of(store, wire) else {
        return;
    };
    if row.folders.contains_key(folder) {
        return;
    }
    let (history, changes) = (row.history, row.changes);
    let (client, session) = (client.clone(), scope.session.clone());
    update(store, wire, |row| {
        row.folders.insert_mut(
            folder.clone(),
            FolderWire {
                client,
                session,
                channel: None,
                commit_channels: rpds::HashTrieMapSync::new_sync(),
            },
        );
    });
    History::ensure_folder(store, history, folder);
    Changes::nudge_folder(store, changes, folder);
}

/// Claim history channels off the session catalog, then subscribe
/// each fresh claim.
pub(crate) fn subscribe_fresh(
    store: &mut Store,
    home: &ahp_wire::SessionId,
    wire: imba::store::Id<HistoryWire>,
    entries: &[CatalogEntry],
    fx: &mut Fx<'_>,
) {
    let Some(row) = of(store, wire) else {
        return;
    };
    let changes = row.changes;
    let histories: Vec<&CatalogEntry> = entries
        .iter()
        .filter(|entry| entry.kind == history_wire::HISTORY_CHANGE_KIND)
        .collect();
    if histories.is_empty() {
        return;
    }
    let fresh: Vec<(
        ResourceLocation,
        ahp_wire::client::Client,
        ahp_wire::client::ChannelUri,
    )> = row
        .folders
        .iter()
        .filter(|(_, held)| held.session == home.session && held.channel.is_none())
        .filter_map(|(folder, held)| {
            histories
                .iter()
                .find(|candidate| entry_serves(folder, candidate))
                .map(|matched| (folder.clone(), held.client.clone(), matched.uri.clone()))
        })
        .collect();
    update(store, wire, |row| {
        for (folder, _, channel) in &fresh {
            if let Some(mut held) = row.folders.get(folder).cloned() {
                held.channel = Some(channel.clone());
                row.folders.insert_mut(folder.clone(), held);
            }
        }
    });
    Changes::nudge_all_in(store, changes);
    for (folder, client, channel) in fresh {
        let landing = folder.clone();
        fx.push(
            AnyEffect::new(SubscribeHistoryEffect {
                client: client.history.clone(),
                channel,
            })
            .map(move |result| {
                Verb::Dynamic(Arc::new(SnapshotLanded {
                    wire,
                    folder: landing.clone(),
                    result: result.map(digest_snapshot),
                }))
            }),
        );
    }
}

/// The session channel failed: every folder riding that wire
/// reports it.
pub(crate) fn session_failed(
    store: &mut Store,
    wire: imba::store::Id<HistoryWire>,
    session: &ahp_wire::client::SessionUri,
    error: &str,
) {
    let Some(row) = of(store, wire) else {
        return;
    };
    let (history, changes) = (row.history, row.changes);
    let riding: Vec<ResourceLocation> = row
        .folders
        .iter()
        .filter(|(_, held)| held.session == *session)
        .map(|(folder, _)| folder.clone())
        .collect();
    for folder in riding {
        History::fold_error(store, history, &folder, error);
    }
    Changes::nudge_all_in(store, changes);
}

/// WIRE → MIRROR: the model's state types, built on the effect
/// worker. The commits' changeset uris are harvested separately —
/// driver state, never the model's.
#[doc(hidden)]
pub fn digest_snapshot(state: history_wire::HistoryState) -> (HistorySnapshot, Harvest) {
    let harvest = harvest_channels(&state.commits);
    let snapshot = HistorySnapshot {
        status: match state.status {
            history_wire::HistoryStatus::Computing => {
                changesview::hichanges::ChangesStatus::Computing
            }
            history_wire::HistoryStatus::Ready => changesview::hichanges::ChangesStatus::Ready,
            history_wire::HistoryStatus::Error => changesview::hichanges::ChangesStatus::Error(
                state.error.unwrap_or_else(|| "history error".to_owned()),
            ),
        },
        head: digest_head(&state.head),
        commits: state.commits.into_iter().map(digest_commit).collect(),
        more: state.more,
    };
    (snapshot, harvest)
}

type Harvest = Vec<(String, ahp_wire::client::ChannelUri)>;

fn harvest_channels(commits: &[history_wire::Commit]) -> Harvest {
    commits
        .iter()
        .map(|commit| {
            (
                commit.id.clone(),
                ahp_wire::client::ChannelUri::new(commit.changeset.clone()),
            )
        })
        .collect()
}

fn digest_head(head: &history_wire::HistoryHead) -> HistoryHead {
    HistoryHead {
        branch: head.branch.clone(),
        upstream: head.upstream.clone(),
        ahead: head.ahead,
        behind: head.behind,
    }
}

/// TEST SUPPORT: no production caller outside this crate.
#[doc(hidden)]
pub fn digest_commit(commit: history_wire::Commit) -> CommitInfo {
    CommitInfo {
        id: commit.id,
        summary: commit.summary,
        message: commit.message,
        author: CommitAuthor {
            name: commit.author.name,
            email: commit.author.email,
        },
        refs: commit
            .refs
            .into_iter()
            .map(|reference| CommitRef {
                name: reference.name,
            })
            .collect(),
    }
}

/// Parse the channel's streamed actions into the model's deltas.
pub fn digest_deltas(actions: &[StateAction]) -> (Vec<HistoryDelta>, Harvest) {
    let mut harvest = Vec::new();
    let deltas = actions
        .iter()
        .filter_map(|action| match action {
            StateAction::Unknown(value) if value["type"] == history_wire::HISTORY_RESET => {
                serde_json::from_value::<history_wire::HistoryReset>(value.clone())
                    .ok()
                    .map(|reset| {
                        let (snapshot, channels) = digest_snapshot(reset.state);
                        harvest.extend(channels);
                        HistoryDelta::Reset(snapshot)
                    })
            }
            StateAction::Unknown(value) if value["type"] == history_wire::HISTORY_APPENDED => {
                serde_json::from_value::<history_wire::HistoryAppended>(value.clone())
                    .ok()
                    .map(|appended| {
                        harvest.extend(harvest_channels(&appended.commits));
                        HistoryDelta::Appended {
                            commits: appended.commits.into_iter().map(digest_commit).collect(),
                            more: appended.more,
                        }
                    })
            }
            StateAction::Unknown(value) if value["type"] == history_wire::HISTORY_PREPENDED => {
                serde_json::from_value::<history_wire::HistoryPrepended>(value.clone())
                    .ok()
                    .map(|prepended| {
                        harvest.extend(harvest_channels(&prepended.commits));
                        HistoryDelta::Prepended {
                            head: digest_head(&prepended.head),
                            commits: prepended.commits.into_iter().map(digest_commit).collect(),
                        }
                    })
            }
            _ => None,
        })
        .collect();
    (deltas, harvest)
}

fn store_harvest(
    store: &mut Store,
    wire: imba::store::Id<HistoryWire>,
    folder: &ResourceLocation,
    harvest: Harvest,
) {
    if harvest.is_empty() {
        return;
    }
    update(store, wire, |row| {
        if let Some(mut held) = row.folders.get(folder).cloned() {
            for (id, channel) in harvest {
                held.commit_channels.insert_mut(id, channel);
            }
            row.folders.insert_mut(folder.clone(), held);
        }
    });
}

fn relaunch_poll(
    store: &Store,
    wire: imba::store::Id<HistoryWire>,
    folder: &ResourceLocation,
    fx: &mut Fx<'_>,
) {
    let Some(held) = of(store, wire).and_then(|row| row.folders.get(folder)) else {
        return;
    };
    let Some(channel) = held.channel.clone() else {
        return;
    };
    let (client, landing) = (held.client.clone(), folder.clone());
    fx.push(
        AnyEffect::new(PollChangesetEffect {
            client: client.changes.clone(),
            channel,
        })
        .map(move |actions| {
            let (deltas, harvest) = digest_deltas(&actions);
            Verb::Dynamic(Arc::new(Polled {
                wire,
                folder: landing.clone(),
                deltas,
                harvest,
            }))
        }),
    );
}

/// A history subscribe answered for one folder.
struct SnapshotLanded {
    wire: imba::store::Id<HistoryWire>,
    folder: ResourceLocation,
    result: Result<(HistorySnapshot, Harvest), String>,
}

impl imba::command::DynamicCommand for SnapshotLanded {
    fn id(&self) -> &'static str {
        "history.snapshot"
    }
    fn name(&self) -> String {
        "History Snapshot".to_owned()
    }
    fn perform(&self, store: &mut Store, _ui: &imba::ui::UiCtx, fx: &mut Fx<'_>) {
        let Some(row) = of(store, self.wire) else {
            return;
        };
        let (history, changes) = (row.history, row.changes);
        match self.result.clone() {
            Ok((snapshot, harvest)) => {
                store_harvest(store, self.wire, &self.folder, harvest);
                History::land_snapshot(store, history, &self.folder, snapshot);
                Changes::nudge_folder(store, changes, &self.folder);
                relaunch_poll(store, self.wire, &self.folder, fx);
            }
            Err(error) => {
                History::fold_error(store, history, &self.folder, &error);
                Changes::nudge_folder(store, changes, &self.folder);
            }
        }
    }
}

/// The pump's landing: fold, then poll again while the stream runs.
struct Polled {
    wire: imba::store::Id<HistoryWire>,
    folder: ResourceLocation,
    deltas: Vec<HistoryDelta>,
    harvest: Harvest,
}

impl imba::command::DynamicCommand for Polled {
    fn id(&self) -> &'static str {
        "history.polled"
    }
    fn name(&self) -> String {
        "History Update".to_owned()
    }
    fn perform(&self, store: &mut Store, _ui: &imba::ui::UiCtx, fx: &mut Fx<'_>) {
        let Some(row) = of(store, self.wire) else {
            return;
        };
        let (history, changes) = (row.history, row.changes);
        store_harvest(store, self.wire, &self.folder, self.harvest.clone());
        History::fold_deltas(store, history, &self.folder, self.deltas.clone());
        Changes::nudge_folder(store, changes, &self.folder);
        relaunch_poll(store, self.wire, &self.folder, fx);
    }
}

/// Fetch a commit's files into its change set, then keep polling the
/// commit's changeset channel while the set computes.
pub(crate) fn fetch_commit_files(
    store: &mut Store,
    wire: imba::store::Id<HistoryWire>,
    folder: &ResourceLocation,
    commit: &changesview::hichanges::Revision,
    fx: &mut Fx<'_>,
) {
    let Some(row) = of(store, wire) else {
        return;
    };
    let changes = row.changes;
    if Changes::commit_generation(store, changes, folder, commit) > 0 {
        return;
    }
    let Some(held) = of(store, wire).and_then(|row| row.folders.get(folder)) else {
        return;
    };
    let Some(channel) = held.commit_channels.get(commit.as_str()).cloned() else {
        return;
    };
    let Some(uris) = of(store, wire).and_then(|row| row.uris.clone()) else {
        return;
    };
    let client = held.client.clone();
    // Mark the SET computing (it exists from the row's birth).
    Changes::mark_commit_computing(store, changes, folder, commit);
    let (landing, commit_id) = (folder.clone(), commit.to_owned());
    fx.push(
        AnyEffect::new(SubscribeChangesetEffect {
            client: client.changes.clone(),
            channel,
        })
        .map(move |result| {
            Verb::Dynamic(Arc::new(CommitFilesLanded {
                wire,
                folder: landing.clone(),
                commit: commit_id.clone(),
                result: result.map(|state| digest_state(&*uris, &landing, &state)),
            }))
        }),
    );
}

/// A commit's changeset snapshot landed: adopt onto the SET, then
/// keep polling while it computes, let go once it is ready.
struct CommitFilesLanded {
    wire: imba::store::Id<HistoryWire>,
    folder: ResourceLocation,
    commit: changesview::hichanges::Revision,
    result: Result<changesview::hichanges::DigestedChangeset, String>,
}

impl imba::command::DynamicCommand for CommitFilesLanded {
    fn id(&self) -> &'static str {
        "history.commit-files"
    }
    fn name(&self) -> String {
        "Commit Files".to_owned()
    }
    fn perform(&self, store: &mut Store, _ui: &imba::ui::UiCtx, fx: &mut Fx<'_>) {
        let Some(changes) = of(store, self.wire).map(|row| row.changes) else {
            return;
        };
        Changes::adopt_commit_state(store, changes, &self.folder, &self.commit, &self.result);
        settle_commit_fetch(store, self.wire, &self.folder, &self.commit, fx);
    }
}

/// Streamed updates for a commit set's changeset channel.
struct CommitFilesPolled {
    wire: imba::store::Id<HistoryWire>,
    folder: ResourceLocation,
    commit: changesview::hichanges::Revision,
    actions: Vec<changesview::hichanges::ChangeAction>,
}

impl imba::command::DynamicCommand for CommitFilesPolled {
    fn id(&self) -> &'static str {
        "history.commit-files-polled"
    }
    fn name(&self) -> String {
        "Commit Files Update".to_owned()
    }
    fn perform(&self, store: &mut Store, _ui: &imba::ui::UiCtx, fx: &mut Fx<'_>) {
        let Some(changes) = of(store, self.wire).map(|row| row.changes) else {
            return;
        };
        Changes::fold_commit_actions(
            store,
            changes,
            &self.folder,
            &self.commit,
            self.actions.clone(),
        );
        settle_commit_fetch(store, self.wire, &self.folder, &self.commit, fx);
    }
}

fn settle_commit_fetch(
    store: &mut Store,
    wire: imba::store::Id<HistoryWire>,
    folder: &ResourceLocation,
    commit: &changesview::hichanges::Revision,
    fx: &mut Fx<'_>,
) {
    let Some(row) = of(store, wire) else {
        return;
    };
    let changes = row.changes;
    let Some(held) = row.folders.get(folder) else {
        return;
    };
    let Some(channel) = held.commit_channels.get(commit.as_str()).cloned() else {
        return;
    };
    let Some(set) = Changes::commit_set(store, changes, folder, commit) else {
        return;
    };
    let Some(uris) = of(store, wire).and_then(|row| row.uris.clone()) else {
        return;
    };
    let client = of(store, wire)
        .and_then(|row| row.folders.get(folder))
        .map(|held| held.client.clone());
    let Some(client) = client else {
        return;
    };
    match set.status {
        changesview::hichanges::ChangesStatus::Computing => {
            let (landing, commit_id) = (folder.clone(), commit.to_owned());
            fx.push(
                AnyEffect::new(PollChangesetEffect {
                    client: client.changes.clone(),
                    channel,
                })
                .map(move |actions| {
                    Verb::Dynamic(Arc::new(CommitFilesPolled {
                        wire,
                        folder: landing.clone(),
                        commit: commit_id.clone(),
                        actions: digest_actions(&*uris, &landing, &actions),
                    }))
                }),
            );
        }
        _ => client.changes.unsubscribe_changeset(&channel),
    }
}

/// Ask the host for older commits — the model's cursor, the
/// driver's wire.
pub(crate) fn grow(
    store: &mut Store,
    wire: imba::store::Id<HistoryWire>,
    folder: &ResourceLocation,
    fx: &mut Fx<'_>,
) {
    let Some(row) = of(store, wire) else {
        return;
    };
    let Some(before) = History::more(store, row.history, folder) else {
        return;
    };
    let Some(held) = row.folders.get(folder) else {
        return;
    };
    let Some(channel) = held.channel.clone() else {
        return;
    };
    fx.push(
        AnyEffect::new(DispatchChatActionEffect {
            client: held.client.session.clone(),
            channel,
            action: StateAction::Unknown(history_wire::action_value(
                history_wire::HISTORY_GROW,
                &history_wire::HistoryGrow {
                    before,
                    limit: None,
                },
            )),
        })
        .map(move |result| Verb::Dynamic(Arc::new(crate::changes::Dispatched { result }))),
    );
}

/// Commit the working copy with a message.
pub(crate) fn commit(
    store: &mut Store,
    wire: imba::store::Id<HistoryWire>,
    folder: &ResourceLocation,
    message: String,
    fx: &mut Fx<'_>,
) {
    let Some(held) = of(store, wire).and_then(|row| row.folders.get(folder)) else {
        return;
    };
    let Some(channel) = held.channel.clone() else {
        return;
    };
    fx.push(
        AnyEffect::new(DispatchChatActionEffect {
            client: held.client.session.clone(),
            channel,
            action: StateAction::Unknown(history_wire::action_value(
                history_wire::HISTORY_COMMIT,
                &history_wire::HistoryCommit {
                    message: message.clone(),
                },
            )),
        })
        .map(move |result| Verb::Dynamic(Arc::new(crate::changes::Dispatched { result }))),
    );
}

/// The batch-tail history lane: drain the model's asks onto the
/// folder wires — a clean collection costs a map read.
pub fn sync(store: &mut Store, wire: imba::store::Id<HistoryWire>, fx: &mut Fx<'_>) {
    let Some(history) = of(store, wire).map(|row| row.history) else {
        return;
    };
    if !History::owes_asks(store, history) {
        return;
    }
    for ask in History::take_asks(store, history) {
        match ask {
            changesview::hihistory::HistoryAsk::Grow(folder) => grow(store, wire, &folder, fx),
            changesview::hihistory::HistoryAsk::CommitFiles(folder, commit) => {
                fetch_commit_files(store, wire, &folder, &commit, fx)
            }
            changesview::hihistory::HistoryAsk::Commit(folder, message) => {
                commit(store, wire, &folder, message, fx)
            }
        }
    }
}
