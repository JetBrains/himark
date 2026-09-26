// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use std::sync::Arc;

use crate::hichanges::{
    dir_forest, empty_side, entry_serves, folder_scope, CatalogEntry, ChangeEntry, ChangesStatus,
    DirSink, DirTrie, Dispatched,
};
use crate::higent::ahp_types::actions::StateAction;
use crate::higent::ahp_types::state::ChangesetState;
use crate::higent::{
    AhpServer, DispatchChatActionEffect, PollChangesetEffect, SubscribeChangesetEffect,
    SubscribeHistoryEffect,
};
use crate::{
    AppCommand, ForestList, ForestNode, ForestSearcher, ListKeyboardController, ResourceLocation,
    ResourceType,
};
use himark_ahp_ext_types::history as history_wire;
use imba::{
    effect::{AnyEffect, Effects},
    store::Store,
    thunk_ext::ThunkExt,
    UiCtx,
};

const NOTE_KIND: &str = "changes-note";

/// One commit row: the wire commit plus the id of the CHANGE SET that
/// is its content (docs/model-view.md — `Commit { change_set }`).
/// Minted eagerly when the row lands; content lands on the SET,
/// lazily. Deref keeps wire-field readers direct.
#[derive(Clone)]
pub struct Commit {
    pub wire: history_wire::Commit,
    pub change_set: crate::hichanges::ChangeSetId,
}

impl std::ops::Deref for Commit {
    type Target = history_wire::Commit;

    fn deref(&self) -> &Self::Target {
        &self.wire
    }
}

#[derive(Clone)]
pub struct FolderHistory {
    seat: Arc<dyn AhpServer>,
    session: crate::higent::SessionUri,
    channel: Option<crate::higent::ChannelUri>,
    pub status: ChangesStatus,
    pub head: history_wire::HistoryHead,
    pub commits: rpds::VectorSync<Commit>,
    pub more: Option<String>,
}

impl FolderHistory {
    pub fn channel_named(&self) -> bool {
        self.channel.is_some()
    }
}

/// The commit-list MODEL per folder. History manages the commit
/// change sets — minting them as rows land, populating them from
/// commits — while the tree views over them live on the `ChangeSets`
/// collection (crate::changes_view).
#[derive(Clone, Default)]
pub struct History {
    folders: rpds::HashTrieMapSync<ResourceLocation, FolderHistory>,
}

impl History {
    pub fn folder(store: &Store, folder: &ResourceLocation) -> Option<FolderHistory> {
        store.get::<History>()?.folders.get(folder).cloned()
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.folders.is_empty()
    }

    pub(crate) fn ensure_folder(
        store: &mut Store,
        folder: &ResourceLocation,
        seat: &Arc<dyn AhpServer>,
        session: &crate::higent::SessionUri,
    ) {
        let known = store
            .get::<History>()
            .is_some_and(|history| history.folders.contains_key(folder));
        if known {
            return;
        }
        let seat = seat.clone();
        let session = session.clone();
        store.update::<History>(|history| {
            history.folders.insert_mut(
                folder.clone(),
                FolderHistory {
                    seat,
                    session,
                    channel: None,
                    status: ChangesStatus::Computing,
                    head: history_wire::HistoryHead::default(),
                    commits: rpds::VectorSync::new_sync(),
                    more: None,
                },
            );
        });
        crate::hichanges::Changes::nudge_folder(store, folder);
    }

    fn adopt_catalog(
        &mut self,
        session: &crate::higent::SessionUri,
        entries: &[CatalogEntry],
    ) -> Vec<(
        ResourceLocation,
        Arc<dyn AhpServer>,
        crate::higent::ChannelUri,
    )> {
        let mut fresh = Vec::new();
        for (folder, entry) in self.folders.clone().iter() {
            if entry.session != *session || entry.channel.is_some() {
                continue;
            }
            let Some(matched) = entries
                .iter()
                .find(|candidate| entry_serves(folder, candidate))
            else {
                continue;
            };
            let mut entry = entry.clone();
            entry.channel = Some(matched.uri.clone());
            let seat = entry.seat.clone();
            let channel = matched.uri.clone();
            self.folders.insert_mut(folder.clone(), entry);
            fresh.push((folder.clone(), seat, channel));
        }
        fresh
    }

    /// Wrap wire commits into rows, minting each commit's CHANGE SET
    /// eagerly (light) — the row references its set from birth.
    fn commit_rows(
        store: &mut Store,
        folder: &ResourceLocation,
        commits: Vec<history_wire::Commit>,
    ) -> Vec<Commit> {
        let Some(entry) = History::folder(store, folder) else {
            return Vec::new();
        };
        commits
            .into_iter()
            .map(|wire| {
                let change_set = crate::hichanges::Changes::ensure_commit_set(
                    store,
                    folder,
                    &crate::hichanges::Revision::new(wire.id.clone()),
                    &entry.seat,
                    &entry.session,
                );
                Commit { wire, change_set }
            })
            .collect()
    }

    /// A history snapshot / reset lands: mint the rows' sets, then
    /// adopt.
    pub(crate) fn land_state(
        store: &mut Store,
        folder: &ResourceLocation,
        state: history_wire::HistoryState,
    ) {
        let rows = Self::commit_rows(store, folder, state.commits.clone());
        store.update::<History>(|history| history.adopt(folder, &state, rows));
    }

    fn adopt(
        &mut self,
        folder: &ResourceLocation,
        state: &history_wire::HistoryState,
        rows: Vec<Commit>,
    ) {
        let Some(mut entry) = self.folders.get(folder).cloned() else {
            return;
        };
        entry.status = match state.status {
            history_wire::HistoryStatus::Computing => ChangesStatus::Computing,
            history_wire::HistoryStatus::Ready => ChangesStatus::Ready,
            history_wire::HistoryStatus::Error => ChangesStatus::Error(
                state
                    .error
                    .clone()
                    .unwrap_or_else(|| "history error".to_owned()),
            ),
        };
        entry.head = state.head.clone();
        entry.commits = rows.into_iter().collect();
        entry.more = state.more.clone();
        self.folders.insert_mut(folder.clone(), entry);
    }

    fn adopt_error(&mut self, folder: &ResourceLocation, error: String) {
        let Some(mut entry) = self.folders.get(folder).cloned() else {
            return;
        };
        entry.status = ChangesStatus::Error(error);
        entry.commits = rpds::VectorSync::new_sync();
        entry.more = None;
        self.folders.insert_mut(folder.clone(), entry);
    }

    pub(crate) fn session_failed(
        store: &mut Store,
        session: &crate::higent::SessionUri,
        error: &str,
    ) {
        store.update::<History>(|history| {
            let riding: Vec<ResourceLocation> = history
                .folders
                .iter()
                .filter(|(_, entry)| entry.session == *session)
                .map(|(folder, _)| folder.clone())
                .collect();
            for folder in riding {
                history.adopt_error(&folder, error.to_owned());
            }
        });
        crate::hichanges::Changes::nudge_all(store);
    }

    /// Fold the channel's streamed actions — STORE-level, because
    /// every incoming commit row mints its change set first.
    pub(crate) fn fold_actions(
        store: &mut Store,
        folder: &ResourceLocation,
        actions: &[StateAction],
    ) {
        for action in actions {
            let StateAction::Unknown(value) = action else {
                continue;
            };
            if value["type"] == history_wire::HISTORY_RESET {
                let Ok(reset) = serde_json::from_value::<history_wire::HistoryReset>(value.clone())
                else {
                    continue;
                };
                Self::land_state(store, folder, reset.state);
            } else if value["type"] == history_wire::HISTORY_APPENDED {
                let Ok(appended) =
                    serde_json::from_value::<history_wire::HistoryAppended>(value.clone())
                else {
                    continue;
                };
                let rows = Self::commit_rows(store, folder, appended.commits);
                store.update::<History>(|history| {
                    let Some(mut entry) = history.folders.get(folder).cloned() else {
                        return;
                    };
                    for commit in rows {
                        entry.commits.push_back_mut(commit);
                    }
                    entry.more = appended.more.clone();
                    history.folders.insert_mut(folder.clone(), entry);
                });
            } else if value["type"] == history_wire::HISTORY_PREPENDED {
                let Ok(prepended) =
                    serde_json::from_value::<history_wire::HistoryPrepended>(value.clone())
                else {
                    continue;
                };
                let rows = Self::commit_rows(store, folder, prepended.commits);
                store.update::<History>(|history| {
                    let Some(mut entry) = history.folders.get(folder).cloned() else {
                        return;
                    };
                    let mut commits = rpds::VectorSync::new_sync();
                    for commit in rows {
                        commits.push_back_mut(commit);
                    }
                    for commit in entry.commits.iter() {
                        commits.push_back_mut(commit.clone());
                    }
                    entry.commits = commits;
                    entry.head = prepended.head.clone();
                    history.folders.insert_mut(folder.clone(), entry);
                });
            }
        }
    }
}

pub(crate) fn subscribe_fresh(
    store: &mut Store,
    window: crate::WindowId,
    session: &crate::higent::SessionUri,
    entries: &[CatalogEntry],
    fx: &mut crate::AppFx<'_>,
) {
    let histories: Vec<CatalogEntry> = entries
        .iter()
        .filter(|entry| entry.kind == history_wire::HISTORY_CHANGE_KIND)
        .cloned()
        .collect();
    if histories.is_empty() {
        return;
    }
    let mut fresh = Vec::new();
    store.update::<History>(|history| {
        fresh = history.adopt_catalog(session, &histories);
    });
    crate::hichanges::Changes::nudge_all(store);
    for (folder, seat, channel) in fresh {
        let landing = folder.clone();
        let scope = folder_scope(&folder);
        fx.push(
            AnyEffect::new(SubscribeHistoryEffect { seat, channel }).map(move |result| {
                let landed = Arc::new(SnapshotLanded {
                    folder: landing.clone(),
                    result,
                });
                match scope.clone() {
                    Some(scope) => AppCommand::dynamic_in(scope, window, landed),
                    None => AppCommand::Dynamic(window, landed),
                }
            }),
        );
    }
}

struct SnapshotLanded {
    folder: ResourceLocation,
    result: Result<history_wire::HistoryState, String>,
}

impl crate::DynamicCommand for SnapshotLanded {
    fn id(&self) -> &'static str {
        "history.landed"
    }
    fn name(&self) -> String {
        "History Snapshot".to_owned()
    }
    fn perform(
        &self,
        _app: &mut crate::Application,
        store: &mut Store,
        window: crate::WindowId,
        fx: &mut crate::AppFx<'_>,
    ) {
        match &self.result {
            Ok(state) => History::land_state(store, &self.folder, state.clone()),
            Err(error) => {
                let error = error.clone();
                store.update::<History>(|history| history.adopt_error(&self.folder, error));
            }
        }
        crate::hichanges::Changes::nudge_folder(store, &self.folder);
        if self.result.is_ok() {
            relaunch_poll(store, window, &self.folder, fx);
        }
    }
}

struct Polled {
    folder: ResourceLocation,
    actions: Vec<StateAction>,
}

impl crate::DynamicCommand for Polled {
    fn id(&self) -> &'static str {
        "history.polled"
    }
    fn name(&self) -> String {
        "History Update".to_owned()
    }
    fn perform(
        &self,
        _app: &mut crate::Application,
        store: &mut Store,
        window: crate::WindowId,
        fx: &mut crate::AppFx<'_>,
    ) {
        History::fold_actions(store, &self.folder, &self.actions);
        crate::hichanges::Changes::nudge_folder(store, &self.folder);
        relaunch_poll(store, window, &self.folder, fx);
    }
}

fn relaunch_poll(
    store: &Store,
    window: crate::WindowId,
    folder: &ResourceLocation,
    fx: &mut crate::AppFx<'_>,
) {
    let Some(entry) = History::folder(store, folder) else {
        return;
    };
    let Some(channel) = entry.channel else {
        return;
    };
    let landing = folder.clone();
    let scope = folder_scope(folder);
    fx.push(
        AnyEffect::new(PollChangesetEffect {
            seat: entry.seat,
            channel,
        })
        .map(move |actions| {
            let polled = Arc::new(Polled {
                folder: landing.clone(),
                actions,
            });
            match scope.clone() {
                Some(scope) => AppCommand::dynamic_in(scope, window, polled),
                None => AppCommand::Dynamic(window, polled),
            }
        }),
    );
}

struct CommitFilesLanded {
    folder: ResourceLocation,
    commit: crate::hichanges::Revision,
    result: Result<ChangesetState, String>,
}

impl crate::DynamicCommand for CommitFilesLanded {
    fn id(&self) -> &'static str {
        "history.commit-files-landed"
    }
    fn name(&self) -> String {
        "Commit Files".to_owned()
    }
    fn perform(
        &self,
        _app: &mut crate::Application,
        store: &mut Store,
        window: crate::WindowId,
        fx: &mut crate::AppFx<'_>,
    ) {
        crate::hichanges::Changes::adopt_commit_state(
            store,
            &self.folder,
            &self.commit,
            &self.result,
        );
        settle_commit_fetch(store, window, &self.folder, &self.commit, fx);
    }
}

fn settle_commit_fetch(
    store: &Store,
    window: crate::WindowId,
    folder: &ResourceLocation,
    commit: &crate::hichanges::Revision,
    fx: &mut crate::AppFx<'_>,
) {
    let Some(entry) = History::folder(store, folder) else {
        return;
    };
    let Some(held) = crate::hichanges::Changes::commit_set(store, folder, commit) else {
        return;
    };
    let Some(wire) = entry.commits.iter().find(|wire| wire.id == commit.as_str()) else {
        return;
    };
    match held.status {
        ChangesStatus::Computing => {
            let landing = folder.clone();
            let commit_id = commit.to_owned();
            let scope = folder_scope(folder);
            fx.push(
                AnyEffect::new(PollChangesetEffect {
                    seat: entry.seat.clone(),
                    channel: crate::higent::ChannelUri::new(wire.changeset.clone()),
                })
                .map(move |actions| {
                    let polled = Arc::new(CommitFilesPolled {
                        folder: landing.clone(),
                        commit: commit_id.clone(),
                        actions,
                    });
                    match scope.clone() {
                        Some(scope) => AppCommand::dynamic_in(scope, window, polled),
                        None => AppCommand::Dynamic(window, polled),
                    }
                }),
            );
        }
        _ => entry
            .seat
            .unsubscribe_changeset(&crate::higent::ChannelUri::new(wire.changeset.clone())),
    }
}

struct CommitFilesPolled {
    folder: ResourceLocation,
    commit: crate::hichanges::Revision,
    actions: Vec<StateAction>,
}

impl crate::DynamicCommand for CommitFilesPolled {
    fn id(&self) -> &'static str {
        "history.commit-files-polled"
    }
    fn name(&self) -> String {
        "Commit Files Update".to_owned()
    }
    fn perform(
        &self,
        _app: &mut crate::Application,
        store: &mut Store,
        window: crate::WindowId,
        fx: &mut crate::AppFx<'_>,
    ) {
        crate::hichanges::Changes::fold_commit_actions(
            store,
            &self.folder,
            &self.commit,
            &self.actions,
        );
        settle_commit_fetch(store, window, &self.folder, &self.commit, fx);
    }
}

pub struct FetchCommitFiles {
    pub folder: ResourceLocation,
    pub commit: crate::hichanges::Revision,
}

impl crate::DynamicCommand for FetchCommitFiles {
    fn id(&self) -> &'static str {
        "history.fetch-commit"
    }
    fn name(&self) -> String {
        "Fetch Commit".to_owned()
    }
    fn perform(
        &self,
        _app: &mut crate::Application,
        store: &mut Store,
        window: crate::WindowId,
        fx: &mut crate::AppFx<'_>,
    ) {
        let Some(entry) = History::folder(store, &self.folder) else {
            return;
        };
        if crate::hichanges::Changes::commit_generation(store, &self.folder, &self.commit) > 0 {
            return;
        }
        let Some(commit) = entry
            .commits
            .iter()
            .find(|held| held.id == self.commit.as_str())
        else {
            return;
        };
        let channel = crate::higent::ChannelUri::new(commit.changeset.clone());
        // Mark the SET computing (it exists from the row's birth).
        crate::hichanges::Changes::mark_commit_computing(store, &self.folder, &self.commit);
        let landing = self.folder.clone();
        let commit_id = self.commit.clone();
        let scope = folder_scope(&self.folder);
        fx.push(
            AnyEffect::new(SubscribeChangesetEffect {
                seat: entry.seat,
                channel,
            })
            .map(move |result| {
                let landed = Arc::new(CommitFilesLanded {
                    folder: landing.clone(),
                    commit: commit_id.clone(),
                    result,
                });
                match scope.clone() {
                    Some(scope) => AppCommand::dynamic_in(scope, window, landed),
                    None => AppCommand::Dynamic(window, landed),
                }
            }),
        );
    }
}

pub struct GrowHistory {
    pub folder: ResourceLocation,
}

impl crate::DynamicCommand for GrowHistory {
    fn id(&self) -> &'static str {
        "history.grow"
    }
    fn name(&self) -> String {
        "Show More History".to_owned()
    }
    fn perform(
        &self,
        _app: &mut crate::Application,
        store: &mut Store,
        window: crate::WindowId,
        fx: &mut crate::AppFx<'_>,
    ) {
        let Some(entry) = History::folder(store, &self.folder) else {
            return;
        };
        let (Some(channel), Some(before)) = (entry.channel, entry.more) else {
            return;
        };
        fx.push(
            AnyEffect::new(DispatchChatActionEffect {
                seat: entry.seat,
                channel,
                action: StateAction::Unknown(history_wire::action_value(
                    history_wire::HISTORY_GROW,
                    &history_wire::HistoryGrow {
                        before,
                        limit: None,
                    },
                )),
            })
            .map(move |result| AppCommand::Dynamic(window, Arc::new(Dispatched { result }))),
        );
    }
}

pub struct CommitHistory {
    pub folder: ResourceLocation,
    pub message: String,
}

impl crate::DynamicCommand for CommitHistory {
    fn id(&self) -> &'static str {
        "history.commit"
    }
    fn name(&self) -> String {
        "Commit".to_owned()
    }
    fn perform(
        &self,
        _app: &mut crate::Application,
        store: &mut Store,
        window: crate::WindowId,
        fx: &mut crate::AppFx<'_>,
    ) {
        if self.message.trim().is_empty() {
            return;
        }
        let Some(entry) = History::folder(store, &self.folder) else {
            return;
        };
        let Some(channel) = entry.channel else {
            return;
        };
        fx.push(
            AnyEffect::new(DispatchChatActionEffect {
                seat: entry.seat,
                channel,
                action: StateAction::Unknown(history_wire::action_value(
                    history_wire::HISTORY_COMMIT,
                    &history_wire::HistoryCommit {
                        message: self.message.clone(),
                    },
                )),
            })
            .map(move |result| AppCommand::Dynamic(window, Arc::new(Dispatched { result }))),
        );
    }
}

use crate::changes_view::RowItem;

struct CommitSink<'a> {
    items: &'a mut rpds::HashTrieMapSync<ResourceLocation, RowItem>,
    folder: &'a ResourceLocation,
    commit: &'a str,
}

impl DirSink for CommitSink<'_> {
    fn branch(&mut self, key: &ResourceLocation) {
        self.items
            .insert_mut(key.clone(), RowItem::Branch { select: true });
    }

    fn file_key(&self, entry: &ChangeEntry, at: &ResourceLocation) -> ResourceLocation {
        at.child(ResourceType::document(), entry.working.name())
    }

    fn file(&mut self, entry: &ChangeEntry, key: &ResourceLocation) {
        // The canvas row key for this entry — the pair's new side,
        // exactly what `canvas_files` mints (docs/editor/diff-canvas.md §6).
        let new = entry
            .after
            .clone()
            .unwrap_or_else(|| empty_side(&entry.working));
        self.items.insert_mut(
            key.clone(),
            RowItem::Open {
                source: crate::diff_canvas::CanvasSource::Commit {
                    folder: self.folder.clone(),
                    id: crate::hichanges::Revision::new(self.commit),
                },
                reveal: Some(new),
                toggle: false,
                select: true,
            },
        );
    }
}

pub(crate) fn graph_node(
    store: &Store,
    folder: &ResourceLocation,
    history: Option<&FolderHistory>,
    items: &mut rpds::HashTrieMapSync<ResourceLocation, RowItem>,
    counts: (skia_safe::Color, skia_safe::Color),
) -> ForestNode<ResourceLocation> {
    let key = folder.child(ResourceType::new("history-graph"), "graph");
    items.insert_mut(key.clone(), RowItem::Branch { select: true });
    let note = |text: &str,
                under: &ResourceLocation,
                items: &mut rpds::HashTrieMapSync<ResourceLocation, RowItem>| {
        let key = under.child(ResourceType::new(NOTE_KIND), text);
        items.insert_mut(key.clone(), RowItem::Note);
        ForestNode {
            key,
            label: text.to_owned(),
            pick: false,
            dim: true,
            trail: Vec::new(),
            tint: crate::TreeTint::Label,
            action: None,
            children: Vec::new(),
        }
    };
    let children = match history {
        None => vec![note("no history source", &key, items)],
        Some(history) => match (&history.status, history.commits.is_empty()) {
            (ChangesStatus::Error(message), _) => vec![note(message, &key, items)],
            (ChangesStatus::Computing, true) => vec![note("computing…", &key, items)],
            (ChangesStatus::Ready, true) => vec![note("no commits", &key, items)],
            _ => {
                let mut rows = Vec::new();
                for commit in history.commits.iter() {
                    let commit_key = folder.child(ResourceType::new("history-commit"), &commit.id);
                    items.insert_mut(
                        commit_key.clone(),
                        RowItem::Open {
                            source: crate::diff_canvas::CanvasSource::Commit {
                                folder: folder.clone(),
                                id: crate::hichanges::Revision::new(commit.id.clone()),
                            },
                            reveal: None,
                            toggle: true,
                            select: true,
                        },
                    );

                    let trail: Vec<(String, skia_safe::Color)> = Vec::new();
                    let held = crate::hichanges::Changes::commit_set(
                        store,
                        folder,
                        &crate::hichanges::Revision::new(commit.id.clone()),
                    )
                    .filter(|set| set.generation() > 0);
                    let children = match held.as_ref() {
                        None => Vec::new(),
                        Some(files) => match (&files.status, files.files.is_empty()) {
                            (ChangesStatus::Error(message), _) => {
                                vec![note(message, &commit_key, items)]
                            }
                            (ChangesStatus::Computing, _) => {
                                vec![note("loading…", &commit_key, items)]
                            }
                            (ChangesStatus::Ready, true) => {
                                vec![note("no files", &commit_key, items)]
                            }
                            (ChangesStatus::Ready, false) => {
                                let mut trie = DirTrie::default();
                                for entry in files.files.iter() {
                                    trie.insert(entry.clone());
                                }
                                dir_forest(
                                    &commit_key,
                                    trie,
                                    counts,
                                    &mut CommitSink {
                                        items,
                                        folder,
                                        commit: &commit.id,
                                    },
                                )
                            }
                        },
                    };

                    let children = match children.is_empty() {
                        true => vec![note("…", &commit_key, items)],
                        false => children,
                    };

                    rows.push(ForestNode {
                        key: commit_key,
                        label: commit.summary.clone(),
                        pick: true,
                        dim: false,
                        trail,
                        tint: crate::TreeTint::Label,
                        action: None,
                        children,
                    });
                }
                if history.more.is_some() {
                    let more_key = key.child(ResourceType::new("history-more"), "more");
                    items.insert_mut(
                        more_key.clone(),
                        RowItem::Grow {
                            folder: folder.clone(),
                        },
                    );
                    rows.push(ForestNode {
                        key: more_key,
                        label: "· · ·  loading older commits  · · ·".to_owned(),
                        pick: false,
                        dim: true,
                        trail: Vec::new(),
                        tint: crate::TreeTint::Label,
                        action: None,
                        children: Vec::new(),
                    });
                }
                rows
            }
        },
    };

    let mut label = folder.name().to_owned();
    if let Some(branch) = history.and_then(|history| history.head.branch.as_deref()) {
        label = format!("{label} — {branch}");
    }
    ForestNode {
        key,
        label,
        pick: false,
        dim: false,
        trail: Vec::new(),
        tint: crate::TreeTint::Label,
        action: None,
        children,
    }
}

#[derive(Clone)]
pub struct CommitTip {
    lines: Vec<(String, bool)>,
}

impl CommitTip {
    #[doc(hidden)]
    pub fn lines(&self) -> &[(String, bool)] {
        &self.lines
    }

    fn of(commit: &himark_ahp_ext_types::history::Commit) -> Self {
        let mut lines: Vec<(String, bool)> = Vec::new();
        let message = commit.message.as_deref().unwrap_or(commit.summary.as_str());
        for raw in message.lines().take(14) {
            let mut line: String = raw.chars().take(96).collect();
            if raw.chars().count() > 96 {
                line.push('…');
            }
            lines.push((line, false));
        }
        lines.push((String::new(), true));
        let author = match &commit.author.email {
            Some(email) => format!("{} <{}>", commit.author.name, email),
            None => commit.author.name.clone(),
        };
        lines.push((author, true));
        if !commit.refs.is_empty() {
            let branches: Vec<&str> = commit
                .refs
                .iter()
                .map(|reference| reference.name.as_str())
                .collect();
            lines.push((branches.join("  ·  "), true));
        }
        Self { lines }
    }
}

impl imba::View for CommitTip {
    type Command = std::convert::Infallible;

    fn perform(
        &mut self,
        _store: &mut Store,
        _ui: &UiCtx,
        command: Self::Command,
        _fx: &mut Effects<'_, Self::Command>,
    ) {
        match command {}
    }

    fn display<'a>(
        &'a self,
        _arena: &'a imba::arena::Arena,
        store: &'a Store,
        ui: &'a UiCtx,
    ) -> impl imba::Layout<'a, Self::Command> + imba::LayoutValue + 'a {
        imba::laid(
            move |_arena: &'a imba::arena::Arena, _constraints: imba::constraints::Constraints| {
                let theme = crate::env::Themes::of(store);
                let chrome = theme.ui().combo.clone();
                let colors = theme.ui().peeker.clone();
                let font = crate::fonts::ui_text_font(ui, chrome.value_size);
                let shaper = imba::TextShaper::of(ui);
                let pad = 14.0f32;
                let line_h = chrome.value_size * 1.45;
                let width = self
                    .lines
                    .iter()
                    .map(|(line, _)| shaper.advance(&font, line))
                    .fold(120.0f32, f32::max)
                    + pad * 2.0;
                let height = self.lines.len() as f32 * line_h + pad * 2.0;
                let lines = self.lines.clone();
                imba::leaf::leaf::<Self::Command>(width.min(560.0), height).paint_instead(
                    move |_arena, canvas, rect| {
                        let mut paint = skia_safe::Paint::default();
                        paint.set_anti_alias(true);
                        paint.set_color(chrome.menu_fill.0);
                        canvas.draw_round_rect(rect, 8.0, 8.0, &paint);
                        let mut y = rect.top + pad + chrome.value_size;
                        for (line, dim) in &lines {
                            let color = if *dim {
                                colors.dim_text.0
                            } else {
                                colors.text.0
                            };
                            shaper.draw(canvas, &font, line, color, 0.0, rect.left + pad, y);
                            y += line_h;
                        }
                    },
                )
            },
        )
    }
}

pub(crate) fn commit_tip(
    rows: &ListKeyboardController<ForestList<ResourceLocation>, ForestSearcher<ResourceLocation>>,
    store: &Store,
    point: skia_safe::Point,
) -> Option<(skia_safe::Rect, CommitTip)> {
    let (key, anchor) = rows.inner().hover_row(point)?;
    if *key.kind() != ResourceType::new("history-commit") {
        return None;
    }
    let id = key.name().to_owned();
    let folder = ResourceLocation::new(
        ResourceType::directory(),
        key.authority().clone(),
        key.path()[..key.path().len().saturating_sub(1)].to_vec(),
    );
    let entry = History::folder(store, &folder)?;
    let commit = entry.commits.iter().find(|commit| commit.id == id)?;
    Some((anchor, CommitTip::of(commit)))
}

pub struct ToggleHistoryView;

impl crate::DynamicCommand for ToggleHistoryView {
    fn id(&self) -> &'static str {
        "history.view"
    }
    fn name(&self) -> String {
        "History".to_owned()
    }
    fn perform(
        &self,
        _app: &mut crate::Application,
        store: &mut Store,
        window: crate::WindowId,
        fx: &mut crate::AppFx<'_>,
    ) {
        let mut entity = crate::Windows::window(store, window).expect("the window entity");
        if entity.dock_owner() == Some(self.id()) {
            entity.roll_away_dock();
            crate::Windows::put(store, window, entity);
            return;
        }
        let workspace = entity.current_session();
        crate::hichanges::Changes::ensure(store, window, workspace.clone(), fx);
        fx.scope(
            move |command| crate::AppCommand::Content(window, command),
            |fx| entity.dismiss_modal(store, fx),
        );
        let view = crate::hichanges::Changes::mint_view(
            store,
            crate::changes_view::ChangesView::open(
                store,
                &_app.ui_ctx(),
                window,
                workspace,
                crate::changes_view::ViewSets::History,
            ),
        );
        let owner = self.id();
        fx.scope(
            move |command| crate::AppCommand::Content(window, command),
            |fx| {
                entity.show_dock(
                    store,
                    Box::new(crate::changes_view::ChangesPane::new(view)),
                    owner,
                    fx,
                )
            },
        );
        crate::Windows::put(store, window, entity);
    }
}

pub fn toolbar_button() -> crate::ToolbarButton {
    crate::ToolbarButton {
        command: "history.view",
        order: 1.1,
        side: crate::ToolbarSide::Right,
        glyph: Arc::new(|canvas, rect, color| {
            let mut paint = skia_safe::Paint::default();
            paint.set_anti_alias(true);
            paint.set_color(color);
            paint.set_style(skia_safe::paint::Style::Stroke);
            paint.set_stroke_width((rect.width() * 0.09).max(1.0));
            let (l, t, w, h) = (rect.left, rect.top, rect.width(), rect.height());

            let x = l + w * 0.35;
            let radius = w * 0.10;
            let dots = [t + h * 0.22, t + h * 0.5, t + h * 0.78];
            let mut path = skia_safe::PathBuilder::new();
            path.move_to((x, dots[0] + radius));
            path.line_to((x, dots[2] - radius));
            canvas.draw_path(&path.detach(), &paint);
            for (index, y) in dots.iter().enumerate() {
                canvas.draw_circle((x, *y), radius, &paint);
                if index == 1 {
                    let mut branch = skia_safe::PathBuilder::new();
                    branch.move_to((x + radius, *y - radius * 0.4));
                    branch.line_to((l + w * 0.72, t + h * 0.32));
                    canvas.draw_path(&branch.detach(), &paint);
                }
            }
        }),
    }
}

#[cfg(test)]
mod tests;
