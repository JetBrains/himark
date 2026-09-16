// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use std::sync::Arc;

use crate::hichanges::{
    dir_forest, empty_side, entry_of, entry_serves, folder_scope, CatalogEntry, ChangeEntry,
    ChangesStatus, DirSink, DirTrie, Dispatched,
};
use crate::higent::ahp_types::actions::StateAction;
use crate::higent::ahp_types::state::ChangesetState;
use crate::higent::{
    AhpServer, DispatchChatActionEffect, PollChangesetEffect, SubscribeChangesetEffect,
    SubscribeHistoryEffect,
};
use crate::{
    AppCommand, ForestList, ForestNode, ForestSearcher, ModalRequest, ResourceLocation,
    ResourceType, SpeedSearchCommand, SpeedSearchView, TreeListCommand,
};
use himark_ahp_ext_types::history as history_wire;
use imba::tooltip::{TooltipCommand, TooltipView};
use imba::{
    arena::Arena,
    constraints::Constraints,
    container::container,
    effect::{AnyEffect, Effects},
    event::{Event, EventResult, Key as InputKey},
    leaf::leaf,
    store::Store,
    thunk_ext::ThunkExt,
    UiCtx, View,
};
use skia_safe::Size;

const NOTE_KIND: &str = "changes-note";

const PANEL_PAD: f32 = 6.0;

#[derive(Clone)]
pub struct CommitFiles {
    pub status: ChangesStatus,
    pub files: rpds::VectorSync<ChangeEntry>,
}

#[derive(Clone)]
pub struct FolderHistory {
    seat: Arc<dyn AhpServer>,
    session: String,
    channel: Option<String>,
    pub status: ChangesStatus,
    pub head: history_wire::HistoryHead,
    pub commits: rpds::VectorSync<history_wire::Commit>,
    pub more: Option<String>,

    pub commit_files: rpds::HashTrieMapSync<String, CommitFiles>,
}

impl FolderHistory {
    pub fn channel_named(&self) -> bool {
        self.channel.is_some()
    }
}

#[derive(Clone, Default)]
pub struct History {
    folders: rpds::HashTrieMapSync<ResourceLocation, FolderHistory>,

    generation: u64,
}

impl History {
    pub fn generation(store: &Store) -> u64 {
        store
            .get::<History>()
            .map(|history| history.generation)
            .unwrap_or(0)
    }

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
        session: &str,
    ) {
        let known = store
            .get::<History>()
            .is_some_and(|history| history.folders.contains_key(folder));
        if known {
            return;
        }
        let seat = seat.clone();
        let session = session.to_owned();
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
                    commit_files: rpds::HashTrieMapSync::new_sync(),
                },
            );
            history.generation += 1;
        });
    }

    fn adopt_catalog(
        &mut self,
        session: &str,
        entries: &[CatalogEntry],
    ) -> Vec<(ResourceLocation, Arc<dyn AhpServer>, String)> {
        let mut fresh = Vec::new();
        for (folder, entry) in self.folders.clone().iter() {
            if entry.session != session || entry.channel.is_some() {
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
        self.generation += 1;
        fresh
    }

    fn adopt(&mut self, folder: &ResourceLocation, state: history_wire::HistoryState) {
        let Some(mut entry) = self.folders.get(folder).cloned() else {
            return;
        };
        entry.status = match state.status {
            history_wire::HistoryStatus::Computing => ChangesStatus::Computing,
            history_wire::HistoryStatus::Ready => ChangesStatus::Ready,
            history_wire::HistoryStatus::Error => {
                ChangesStatus::Error(state.error.unwrap_or_else(|| "history error".to_owned()))
            }
        };
        entry.head = state.head;
        entry.commits = state.commits.into_iter().collect();
        entry.more = state.more;
        self.folders.insert_mut(folder.clone(), entry);
        self.generation += 1;
    }

    fn adopt_error(&mut self, folder: &ResourceLocation, error: String) {
        let Some(mut entry) = self.folders.get(folder).cloned() else {
            return;
        };
        entry.status = ChangesStatus::Error(error);
        entry.commits = rpds::VectorSync::new_sync();
        entry.more = None;
        self.folders.insert_mut(folder.clone(), entry);
        self.generation += 1;
    }

    pub(crate) fn session_failed(store: &mut Store, session: &str, error: &str) {
        store.update::<History>(|history| {
            let riding: Vec<ResourceLocation> = history
                .folders
                .iter()
                .filter(|(_, entry)| entry.session == session)
                .map(|(folder, _)| folder.clone())
                .collect();
            for folder in riding {
                history.adopt_error(&folder, error.to_owned());
            }
        });
    }

    fn fold(&mut self, folder: &ResourceLocation, actions: &[StateAction]) {
        let mut moved = false;
        for action in actions {
            let StateAction::Unknown(value) = action else {
                continue;
            };
            if value["type"] == history_wire::HISTORY_RESET {
                let Ok(reset) = serde_json::from_value::<history_wire::HistoryReset>(value.clone())
                else {
                    continue;
                };
                self.adopt(folder, reset.state);
                continue;
            }
            let Some(mut entry) = self.folders.get(folder).cloned() else {
                continue;
            };
            if value["type"] == history_wire::HISTORY_APPENDED {
                let Ok(appended) =
                    serde_json::from_value::<history_wire::HistoryAppended>(value.clone())
                else {
                    continue;
                };
                for commit in appended.commits {
                    entry.commits.push_back_mut(commit);
                }
                entry.more = appended.more;
            } else if value["type"] == history_wire::HISTORY_PREPENDED {
                let Ok(prepended) =
                    serde_json::from_value::<history_wire::HistoryPrepended>(value.clone())
                else {
                    continue;
                };
                let mut commits = rpds::VectorSync::new_sync();
                for commit in prepended.commits {
                    commits.push_back_mut(commit);
                }
                for commit in entry.commits.iter() {
                    commits.push_back_mut(commit.clone());
                }
                entry.commits = commits;
                entry.head = prepended.head;
            } else {
                continue;
            }
            self.folders.insert_mut(folder.clone(), entry);
            moved = true;
        }
        if moved {
            self.generation += 1;
        }
    }

    fn adopt_commit_files(
        &mut self,
        uris: &dyn crate::higent::ResourceUriMap,
        folder: &ResourceLocation,
        commit: &str,
        result: &Result<ChangesetState, String>,
    ) {
        let files = match result {
            Ok(state) => CommitFiles {
                status: ChangesStatus::of_wire(
                    &state.status,
                    state.error.as_ref().map(|error| error.message.as_str()),
                ),
                files: state
                    .files
                    .iter()
                    .filter_map(|file| entry_of(uris, folder, file))
                    .collect(),
            },
            Err(error) => CommitFiles {
                status: ChangesStatus::Error(error.clone()),
                files: rpds::VectorSync::new_sync(),
            },
        };
        self.set_commit_files(folder, commit, files);
    }

    fn fold_commit_files(
        &mut self,
        uris: &dyn crate::higent::ResourceUriMap,
        folder: &ResourceLocation,
        commit: &str,
        actions: &[StateAction],
    ) {
        let Some(mut held) = self
            .folders
            .get(folder)
            .and_then(|entry| entry.commit_files.get(commit))
            .cloned()
        else {
            return;
        };
        for action in actions {
            match action {
                StateAction::ChangesetContentChanged(content) => {
                    held.files = content
                        .files
                        .iter()
                        .filter_map(|file| entry_of(uris, folder, file))
                        .collect();
                }
                StateAction::ChangesetStatusChanged(status) => {
                    held.status = ChangesStatus::of_wire(
                        &status.status,
                        status.error.as_ref().map(|error| error.message.as_str()),
                    );
                }
                _ => {}
            }
        }
        self.set_commit_files(folder, commit, held);
    }

    fn set_commit_files(&mut self, folder: &ResourceLocation, commit: &str, files: CommitFiles) {
        let Some(mut entry) = self.folders.get(folder).cloned() else {
            return;
        };
        entry.commit_files.insert_mut(commit.to_owned(), files);
        self.folders.insert_mut(folder.clone(), entry);
        self.generation += 1;
    }
}

pub(crate) fn subscribe_fresh(
    store: &mut Store,
    window: crate::WindowId,
    session: &str,
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
        store.update::<History>(|history| match &self.result {
            Ok(state) => history.adopt(&self.folder, state.clone()),
            Err(error) => history.adopt_error(&self.folder, error.clone()),
        });
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
        store.update::<History>(|history| history.fold(&self.folder, &self.actions));
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
    commit: String,
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
        let Some(uris) = crate::hichanges::Changes::uris(store) else {
            return;
        };
        store.update::<History>(|history| {
            history.adopt_commit_files(&*uris, &self.folder, &self.commit, &self.result)
        });
        settle_commit_fetch(store, window, &self.folder, &self.commit, fx);
    }
}

fn settle_commit_fetch(
    store: &Store,
    window: crate::WindowId,
    folder: &ResourceLocation,
    commit: &str,
    fx: &mut crate::AppFx<'_>,
) {
    let Some(entry) = History::folder(store, folder) else {
        return;
    };
    let Some(held) = entry.commit_files.get(commit) else {
        return;
    };
    let Some(wire) = entry.commits.iter().find(|wire| wire.id == commit) else {
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
                    channel: wire.changeset.clone(),
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
        _ => entry.seat.unsubscribe_changeset(&wire.changeset),
    }
}

struct CommitFilesPolled {
    folder: ResourceLocation,
    commit: String,
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
        let Some(uris) = crate::hichanges::Changes::uris(store) else {
            return;
        };
        store.update::<History>(|history| {
            history.fold_commit_files(&*uris, &self.folder, &self.commit, &self.actions)
        });
        settle_commit_fetch(store, window, &self.folder, &self.commit, fx);
    }
}

pub struct FetchCommitFiles {
    pub folder: ResourceLocation,
    pub commit: String,
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
        if entry.commit_files.get(&self.commit).is_some() {
            return;
        }
        let Some(commit) = entry.commits.iter().find(|held| held.id == self.commit) else {
            return;
        };
        let channel = commit.changeset.clone();
        store.update::<History>(|history| {
            history.set_commit_files(
                &self.folder,
                &self.commit,
                CommitFiles {
                    status: ChangesStatus::Computing,
                    files: rpds::VectorSync::new_sync(),
                },
            );
        });
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

#[derive(Clone)]
enum RowItem {
    Branch,

    Commit {
        folder: ResourceLocation,
        id: String,
    },

    File {
        folder: ResourceLocation,
        commit: String,
        new: ResourceLocation,
    },

    More {
        folder: ResourceLocation,
    },

    Note,
}

struct CommitSink<'a> {
    items: &'a mut rpds::HashTrieMapSync<ResourceLocation, RowItem>,
    folder: &'a ResourceLocation,
    commit: &'a str,
}

impl DirSink for CommitSink<'_> {
    fn branch(&mut self, key: &ResourceLocation) {
        self.items.insert_mut(key.clone(), RowItem::Branch);
    }

    fn file_key(&self, entry: &ChangeEntry, at: &ResourceLocation) -> ResourceLocation {
        at.child(ResourceType::document(), entry.working.name())
    }

    fn file(&mut self, entry: &ChangeEntry, key: &ResourceLocation) {
        // The canvas row key for this entry — the pair's new side,
        // exactly what `canvas_files` mints (docs/diff-canvas.md §6).
        let new = entry
            .after
            .clone()
            .unwrap_or_else(|| empty_side(&entry.working));
        self.items.insert_mut(
            key.clone(),
            RowItem::File {
                folder: self.folder.clone(),
                commit: self.commit.to_owned(),
                new,
            },
        );
    }
}

fn graph_node(
    folder: &ResourceLocation,
    history: Option<&FolderHistory>,
    items: &mut rpds::HashTrieMapSync<ResourceLocation, RowItem>,
    counts: (skia_safe::Color, skia_safe::Color),
) -> ForestNode<ResourceLocation> {
    let key = folder.child(ResourceType::new("history-graph"), "graph");
    items.insert_mut(key.clone(), RowItem::Branch);
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
                        RowItem::Commit {
                            folder: folder.clone(),
                            id: commit.id.clone(),
                        },
                    );

                    let trail: Vec<(String, skia_safe::Color)> = Vec::new();
                    let children = match history.commit_files.get(&commit.id) {
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
                        children,
                    });
                }
                if history.more.is_some() {
                    let more_key = key.child(ResourceType::new("history-more"), "more");
                    items.insert_mut(
                        more_key.clone(),
                        RowItem::More {
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
                let pad = 14.0f32;
                let line_h = chrome.value_size * 1.45;
                let width = self
                    .lines
                    .iter()
                    .map(|(line, _)| font.measure_str(line, None).0)
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
                            paint.set_color(if *dim {
                                colors.dim_text.0
                            } else {
                                colors.text.0
                            });
                            canvas.draw_str(line, (rect.left + pad, y), &font, &paint);
                            y += line_h;
                        }
                    },
                )
            },
        )
    }
}

fn commit_tip(
    rows: &SpeedSearchView<ForestList<ResourceLocation>, ForestSearcher<ResourceLocation>>,
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

pub enum HistoryCommand {
    Rows(TooltipCommand<SpeedSearchCommand<TreeListCommand>>),

    Select(isize),

    Fold(bool),

    Pick,

    AutoGrow,

    Refresh,

    Dismiss,
}

pub struct HistoryView {
    list: TooltipView<
        SpeedSearchView<ForestList<ResourceLocation>, ForestSearcher<ResourceLocation>>,
        CommitTip,
    >,
    items: rpds::HashTrieMapSync<ResourceLocation, RowItem>,
    workspace: crate::SessionId,
    window: crate::WindowId,

    seen: u64,

    grown: rpds::HashTrieMapSync<ResourceLocation, String>,
    request: Option<ModalRequest>,
}

impl Clone for HistoryView {
    fn clone(&self) -> Self {
        Self {
            list: self.list.clone(),
            items: self.items.clone(),
            workspace: self.workspace.clone(),
            window: self.window,
            seen: self.seen,
            grown: self.grown.clone(),

            request: None,
        }
    }
}

impl HistoryView {
    pub fn open(
        store: &Store,
        ui: &UiCtx,
        window: crate::WindowId,
        workspace: crate::SessionId,
    ) -> Self {
        let mut section = Self {
            list: TooltipView::new(
                SpeedSearchView::new(
                    ForestList::new(store),
                    ForestSearcher::default(),
                    crate::env::Fonts::of(store),
                ),
                commit_tip,
            ),
            items: rpds::HashTrieMapSync::new_sync(),
            workspace,
            window,
            seen: 0,
            grown: rpds::HashTrieMapSync::new_sync(),
            request: None,
        };
        section.refresh(store, ui);
        section
    }

    pub(crate) fn stale(&self, store: &Store) -> bool {
        History::generation(store) != self.seen
    }

    #[cfg(any(test, feature = "test-support"))]
    pub fn rows(&self) -> Vec<(u8, String, bool)> {
        self.list.view().inner().forest.rows_trailed()
    }

    #[doc(hidden)]
    pub fn cursor_name(&self) -> Option<String> {
        self.list
            .view()
            .inner()
            .list()
            .cursor()
            .map(|key| key.name().to_owned())
    }

    #[allow(dead_code)]
    pub(crate) fn take_request(&mut self) -> Option<ModalRequest> {
        self.request.take()
    }

    pub(crate) fn refresh(&mut self, store: &Store, ui: &UiCtx) {
        self.seen = History::generation(store);
        let mut items = rpds::HashTrieMapSync::new_sync();
        let themes = crate::env::Themes::of(store);
        let ui_theme = themes.ui();
        let counts = (ui_theme.chat.added_color.0, ui_theme.chat.removed_color.0);
        let _accent = ui_theme.peeker.accent.0;
        let _dim = ui_theme.peeker.dim_text.0;
        let nodes: Vec<ForestNode<ResourceLocation>> =
            crate::higent::session_folders(store, &self.workspace)
                .iter()
                .map(|folder| {
                    graph_node(
                        folder,
                        History::folder(store, folder).as_ref(),
                        &mut items,
                        counts,
                    )
                })
                .collect();

        for (key, item) in items.iter() {
            if matches!(item, RowItem::Commit { .. }) && !self.items.contains_key(key) {
                self.list
                    .view_mut()
                    .inner_mut()
                    .forest
                    .preset_collapsed(key);
            }
        }
        self.items = items;
        self.list.view_mut().inner_mut().set(&nodes, store, ui);
    }

    fn activate(&mut self, index: usize, store: &Store, ui: &UiCtx) {
        let Some(key) = self.list.view().inner().list().key_at(index).cloned() else {
            return;
        };
        self.activate_key(&key, store, ui);
    }

    fn activate_key(&mut self, key: &ResourceLocation, store: &Store, ui: &UiCtx) {
        if !matches!(self.items.get(key), Some(RowItem::Note) | None) {
            self.list
                .view_mut()
                .inner_mut()
                .list_mut()
                .select_only(key.clone());
        }
        match self.items.get(key).cloned() {
            Some(RowItem::Branch) => self.list.view_mut().inner_mut().toggle(key, store, ui),
            Some(RowItem::Commit { folder, id }) => {
                self.list.view_mut().inner_mut().toggle(key, store, ui);

                // OpenDiffCanvas also ensures the commit's file
                // fetch — one request feeds the tree AND the canvas.
                self.request = Some(ModalRequest::Perform(AppCommand::Dynamic(
                    self.window,
                    Arc::new(crate::diff_canvas::OpenDiffCanvas {
                        source: crate::diff_canvas::CanvasSource::Commit { folder, id },
                        reveal: None,
                    }),
                )));
            }
            Some(RowItem::More { folder }) => {
                self.request = Some(ModalRequest::Perform(AppCommand::Dynamic(
                    self.window,
                    Arc::new(GrowHistory { folder }),
                )));
            }
            Some(RowItem::File {
                folder,
                commit,
                new,
            }) => {
                self.request = Some(ModalRequest::Perform(AppCommand::Dynamic(
                    self.window,
                    Arc::new(crate::diff_canvas::OpenDiffCanvas {
                        source: crate::diff_canvas::CanvasSource::Commit { folder, id: commit },
                        reveal: Some(new),
                    }),
                )));
            }
            Some(RowItem::Note) | None => {}
        }
    }
}

impl View for HistoryView {
    type Command = HistoryCommand;

    fn perform(
        &mut self,
        store: &mut Store,
        ui: &UiCtx,
        command: Self::Command,
        fx: &mut Effects<'_, Self::Command>,
    ) {
        match command {
            HistoryCommand::Rows(command) => {
                if let TooltipCommand::Host(SpeedSearchCommand::Inner(inner)) = &command {
                    if let Some((index, _)) = crate::tree_interaction(inner) {
                        return self.activate(index, store, ui);
                    }
                }
                fx.scope(HistoryCommand::Rows, |fx| {
                    self.list.perform(store, ui, command, fx)
                });
            }
            HistoryCommand::Select(delta) => self
                .list
                .view_mut()
                .inner_mut()
                .list_mut()
                .cursor_step(delta),
            HistoryCommand::Fold(expand) => {
                if expand {
                    if let Some(key) = self.list.view().inner().list().cursor().cloned() {
                        if let Some(RowItem::Commit { folder, id }) = self.items.get(&key).cloned()
                        {
                            let entry = History::folder(store, &folder);
                            let unfetched =
                                entry.is_some_and(|entry| entry.commit_files.get(&id).is_none());
                            if unfetched {
                                self.request = Some(ModalRequest::Perform(AppCommand::Dynamic(
                                    self.window,
                                    Arc::new(FetchCommitFiles { folder, commit: id }),
                                )));
                            }
                        }
                    }
                }
                self.list
                    .view_mut()
                    .inner_mut()
                    .fold_cursor(expand, store, ui);
            }
            HistoryCommand::Pick => {
                if let Some(key) = self.list.view().inner().list().cursor().cloned() {
                    self.activate_key(&key, store, ui);
                }
            }
            HistoryCommand::AutoGrow => {
                for folder in crate::higent::session_folders(store, &self.workspace) {
                    let Some(entry) = History::folder(store, &folder) else {
                        continue;
                    };
                    let Some(more) = entry.more else { continue };
                    if self.grown.get(&folder).map(String::as_str) == Some(more.as_str()) {
                        continue;
                    }
                    self.grown.insert_mut(folder.clone(), more);
                    self.request = Some(ModalRequest::Perform(AppCommand::Dynamic(
                        self.window,
                        Arc::new(GrowHistory { folder }),
                    )));

                    break;
                }
            }
            HistoryCommand::Refresh => self.refresh(store, ui),
            HistoryCommand::Dismiss => {
                self.request = Some(ModalRequest::Close);
            }
        }
    }

    fn display<'a>(
        &'a self,
        arena: &'a Arena,
        store: &'a Store,
        ui: &'a UiCtx,
    ) -> impl imba::Layout<'a, Self::Command> + imba::LayoutValue + 'a {
        imba::laid(move |_arena: &'a Arena, constraints: Constraints| {
            let size = constraints.max;
            let mut section = container(arena, size);

            let band = PANEL_PAD;

            let rows = self
                .list
                .layout(
                    arena,
                    store,
                    ui,
                    Constraints::tight(Size::new(size.width, (size.height - band).max(1.0))),
                )
                .map(HistoryCommand::Rows);
            section.place(0.0, band, rows);

            let searching = self.list.view().searching();
            let keymap = leaf::<HistoryCommand>(size.width, size.height).event(
                move |_arena, event, _size| match event {
                    Event::KeyDown {
                        key: InputKey::Escape,
                        ..
                    } if !searching => EventResult::Command(HistoryCommand::Dismiss),
                    Event::KeyDown {
                        key: InputKey::Up, ..
                    } if !searching => EventResult::Command(HistoryCommand::Select(-1)),
                    Event::KeyDown {
                        key: InputKey::Down,
                        ..
                    } if !searching => EventResult::Command(HistoryCommand::Select(1)),
                    Event::KeyDown {
                        key: InputKey::Left,
                        ..
                    } if !searching => EventResult::Command(HistoryCommand::Fold(false)),
                    Event::KeyDown {
                        key: InputKey::Right,
                        ..
                    } if !searching => EventResult::Command(HistoryCommand::Fold(true)),
                    Event::KeyDown {
                        key: InputKey::Enter,
                        ..
                    } if searching => EventResult::Commands(vec![
                        HistoryCommand::Pick,
                        HistoryCommand::Rows(TooltipCommand::Host(SpeedSearchCommand::Clear)),
                    ]),
                    Event::KeyDown {
                        key: InputKey::Enter,
                        ..
                    } => EventResult::Command(HistoryCommand::Pick),
                    _ => EventResult::Ignored,
                },
            );
            section.place(0.0, 0.0, keymap);

            let stale = self.stale(store);
            let rows_height = (size.height - band).max(1.0);
            let near_tail = {
                let list = self.list.view().inner();
                list.scroll_y() + rows_height
                    >= list.list().total_height() - 2.0 * crate::ui::space::XL
            };
            let pageable = crate::higent::session_folders(store, &self.workspace)
                .iter()
                .any(|folder| {
                    History::folder(store, folder).is_some_and(|entry| {
                        entry.more.as_deref().is_some_and(|more| {
                            self.grown.get(folder).map(String::as_str) != Some(more)
                        })
                    })
                });
            let grow = near_tail && pageable;
            section.wrap(move |inner| ReconcileShell { inner, stale, grow })
        })
    }
}

struct ReconcileShell<Inner> {
    inner: Inner,
    stale: bool,

    grow: bool,
}

impl<'a, Inner: imba::Widget<'a, HistoryCommand>> imba::Widget<'a, HistoryCommand>
    for ReconcileShell<Inner>
{
    fn size(&self) -> Size {
        self.inner.size()
    }

    fn overlays(&mut self) -> Vec<imba::overlay::Overlay<'a, HistoryCommand>> {
        self.inner.overlays()
    }

    fn handle_event(
        &self,
        arena: &Arena,
        event: &Event<'_>,
        viewport: skia_safe::Rect,
    ) -> EventResult<HistoryCommand> {
        let mut result = self.inner.handle_event(arena, event, viewport);
        if matches!(event, Event::Paint { .. }) {
            if self.stale {
                result = result.merge(EventResult::Command(HistoryCommand::Refresh));
            }
            if self.grow {
                result = result.merge(EventResult::Command(HistoryCommand::AutoGrow));
            }
        }
        result
    }

    fn focus_data<'w>(&'w mut self) -> imba::focus::FocusData<'w, HistoryCommand>
    where
        'a: 'w,
    {
        self.inner.focus_data()
    }
}

impl crate::ModalView for HistoryView {
    fn clone_modal(&self) -> Box<dyn crate::ModalView> {
        Box::new(self.clone())
    }

    fn take_request(&mut self) -> Option<ModalRequest> {
        self.request.take()
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
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
        let panel = HistoryView::open(store, &_app.ui_ctx(), window, workspace);
        let owner = self.id();
        fx.scope(
            move |command| crate::AppCommand::Content(window, command),
            |fx| entity.show_dock(store, Box::new(panel), owner, fx),
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
