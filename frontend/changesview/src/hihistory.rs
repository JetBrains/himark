// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use crate::hichanges::{dir_forest, empty_side, ChangeEntry, ChangesStatus, DirSink, DirTrie};
use editor::{ResourceLocation, ResourceType};
use hikit::{ForestList, ForestNode, ForestSearcher, ListKeyboardController};
use imba::{effect::Effects, store::Store, thunk_ext::ThunkExt, UiCtx};

const NOTE_KIND: &str = "changes-note";

/// MIRRORS of the wire history state — the collection's OWN types
/// (ahp stays out of the model; the driver digests wire → mirror).
#[derive(Clone, Debug, PartialEq)]
pub struct CommitInfo {
    pub id: String,
    pub summary: String,
    pub message: Option<String>,
    pub author: CommitAuthor,
    pub refs: Vec<CommitRef>,
}

#[derive(Clone, Debug, PartialEq, Default)]
pub struct CommitAuthor {
    pub name: String,
    pub email: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct CommitRef {
    pub name: String,
}

#[derive(Clone, Debug, PartialEq, Default)]
pub struct HistoryHead {
    pub branch: Option<String>,
    pub upstream: Option<String>,
    pub ahead: Option<u32>,
    pub behind: Option<u32>,
}

/// A landed history snapshot, mirrored.
#[derive(Clone)]
pub struct HistorySnapshot {
    pub status: ChangesStatus,
    pub head: HistoryHead,
    pub commits: Vec<CommitInfo>,
    pub more: Option<String>,
}

/// A streamed history update, mirrored — the driver parses the wire
/// actions; the model folds values.
#[derive(Clone)]
pub enum HistoryDelta {
    Reset(HistorySnapshot),
    Appended {
        commits: Vec<CommitInfo>,
        more: Option<String>,
    },
    Prepended {
        commits: Vec<CommitInfo>,
        head: HistoryHead,
    },
}

/// One commit row: the mirrored commit plus the id of the CHANGE SET
/// that is its content (docs/model-view.md — `Commit { change_set }`).
/// Minted eagerly when the row lands; content lands on the SET,
/// lazily. Deref keeps field readers direct.
#[derive(Clone)]
pub struct Commit {
    pub info: CommitInfo,
    pub change_set: crate::hichanges::ChangeSetId,
}

impl std::ops::Deref for Commit {
    type Target = CommitInfo;

    fn deref(&self) -> &Self::Target {
        &self.info
    }
}

#[derive(Clone)]
pub struct FolderHistory {
    pub status: ChangesStatus,
    pub head: HistoryHead,
    pub commits: rpds::VectorSync<Commit>,
    pub more: Option<String>,
}

/// The commit-list MODEL per folder. History manages the commit
/// change sets — minting them as rows land, populating them from
/// commits — while the tree views over them live on the `ChangeSets`
/// collection (crate::changes_view).
#[derive(Clone)]
pub struct History {
    /// The sibling whose sets this collection's commits are — wired at
    /// the family mint (docs/entities.md law 4).
    changes: imba::store::Id<crate::hichanges::ChangeSets>,

    folders: rpds::HashTrieMapSync<ResourceLocation, FolderHistory>,

    /// Outbound intents the views NOTED — the wire driver's lane
    /// drains them each batch tail; the model never dispatches and
    /// no view carries a wire or a window for these.
    asks: Vec<HistoryAsk>,
}

/// What a face may ask of a folder's history — model vocabulary;
/// the driver turns an ask into wire traffic.
#[derive(Clone)]
pub enum HistoryAsk {
    /// Older commits behind the cursor the model holds.
    Grow(ResourceLocation),
    /// A commit's files into its change set.
    CommitFiles(ResourceLocation, crate::hichanges::Revision),
    /// Commit the working copy with a message.
    Commit(ResourceLocation, String),
}

impl History {
    /// A collection wired to the sets its commits are — minted by the
    /// family ceremony, and by tests that stand one up alone.
    pub fn wired(changes: imba::store::Id<crate::hichanges::ChangeSets>) -> Self {
        Self {
            changes,
            folders: rpds::HashTrieMapSync::new_sync(),
            asks: Vec::new(),
        }
    }

    pub fn changes(&self) -> imba::store::Id<crate::hichanges::ChangeSets> {
        self.changes
    }

    /// A folder's history, in the collection whose id reached here.
    pub fn folder(
        store: &Store,
        history: imba::store::Id<History>,
        folder: &ResourceLocation,
    ) -> Option<FolderHistory> {
        store.entity(history)?.folders.get(folder).cloned()
    }

    /// Mutate the collection in place; a gone collection takes no
    /// write (siblings are wired at the ceremony, never defaulted).
    fn update_folder(
        store: &mut Store,
        history: imba::store::Id<History>,
        mutate: impl FnOnce(&mut History),
    ) {
        let Some(mut row) = store.entity(history).cloned() else {
            return;
        };
        mutate(&mut row);
        store.put_entity(history, row);
    }

    pub fn is_empty(&self) -> bool {
        self.folders.is_empty() && self.asks.is_empty()
    }

    /// Note an ask for the driver's lane — the faces' door: no wire,
    /// no window, just the model's own vocabulary.
    pub fn ask(store: &mut Store, history: imba::store::Id<History>, ask: HistoryAsk) {
        Self::update_folder(store, history, |held| held.asks.push(ask));
    }

    pub fn owes_asks(store: &Store, history: imba::store::Id<History>) -> bool {
        store
            .entity::<History>(history)
            .is_some_and(|held| !held.asks.is_empty())
    }

    pub fn take_asks(store: &mut Store, history: imba::store::Id<History>) -> Vec<HistoryAsk> {
        let Some(mut held) = store.entity::<History>(history).cloned() else {
            return Vec::new();
        };
        let asks = std::mem::take(&mut held.asks);
        store.put_entity(history, held);
        asks
    }

    /// The driver's attach door: an empty folder row, idempotent.
    pub fn ensure_folder(
        store: &mut Store,
        history: imba::store::Id<History>,
        folder: &ResourceLocation,
    ) {
        if Self::folder(store, history, folder).is_some() {
            return;
        }
        Self::update_folder(store, history, |history| {
            history.folders.insert_mut(
                folder.clone(),
                FolderHistory {
                    status: ChangesStatus::Computing,
                    head: HistoryHead::default(),
                    commits: rpds::VectorSync::new_sync(),
                    more: None,
                },
            );
        });
    }

    /// Wrap mirrored commits into rows, minting each commit's CHANGE
    /// SET eagerly (light) — the row references its set from birth.
    fn commit_rows(
        &self,
        store: &mut Store,
        folder: &ResourceLocation,
        commits: Vec<CommitInfo>,
    ) -> Vec<Commit> {
        commits
            .into_iter()
            .map(|info| {
                let change_set = crate::hichanges::Changes::ensure_commit_set(
                    store,
                    self.changes,
                    folder,
                    &crate::hichanges::Revision::new(info.id.clone()),
                );
                Commit { info, change_set }
            })
            .collect()
    }

    /// A history snapshot lands: mint the rows' sets (in the sibling
    /// collection), then adopt. The driver's door.
    pub fn land_snapshot(
        store: &mut Store,
        history: imba::store::Id<History>,
        folder: &ResourceLocation,
        snapshot: HistorySnapshot,
    ) {
        // Leased out: the row works against the store (it mints the
        // sibling's commit sets), then goes back whole.
        let Some(mut held) = store.entity::<History>(history).cloned() else {
            return;
        };
        held.land_snapshot_in_place(store, folder, snapshot);
        store.put_entity(history, held);
    }

    fn land_snapshot_in_place(
        &mut self,
        store: &mut Store,
        folder: &ResourceLocation,
        snapshot: HistorySnapshot,
    ) {
        let rows = self.commit_rows(store, folder, snapshot.commits);
        let Some(mut entry) = self.folders.get(folder).cloned() else {
            return;
        };
        entry.status = snapshot.status;
        entry.head = snapshot.head;
        entry.commits = rows.into_iter().collect();
        entry.more = snapshot.more;
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

    /// One folder's history resolves errored — the driver's door.
    pub fn fold_error(
        store: &mut Store,
        history: imba::store::Id<History>,
        folder: &ResourceLocation,
        error: &str,
    ) {
        Self::update_folder(store, history, |held| {
            held.adopt_error(folder, error.to_owned());
        });
    }

    /// The paging cursor the host handed with the last landing — the
    /// grow ask carries it back.
    pub fn more(
        store: &Store,
        history: imba::store::Id<History>,
        folder: &ResourceLocation,
    ) -> Option<String> {
        Self::folder(store, history, folder)?.more
    }

    /// Fold the driver's mirrored deltas — every incoming commit row
    /// mints its change set in the sibling collection first.
    pub fn fold_deltas(
        store: &mut Store,
        history: imba::store::Id<History>,
        folder: &ResourceLocation,
        deltas: Vec<HistoryDelta>,
    ) {
        let Some(mut held) = store.entity::<History>(history).cloned() else {
            return;
        };
        for delta in deltas {
            held.fold_delta_in_place(store, folder, delta);
        }
        store.put_entity(history, held);
    }

    fn fold_delta_in_place(
        &mut self,
        store: &mut Store,
        folder: &ResourceLocation,
        delta: HistoryDelta,
    ) {
        match delta {
            HistoryDelta::Reset(snapshot) => self.land_snapshot_in_place(store, folder, snapshot),
            HistoryDelta::Appended { commits, more } => {
                let rows = self.commit_rows(store, folder, commits);
                let Some(mut entry) = self.folders.get(folder).cloned() else {
                    return;
                };
                for commit in rows {
                    entry.commits.push_back_mut(commit);
                }
                entry.more = more;
                self.folders.insert_mut(folder.clone(), entry);
            }
            HistoryDelta::Prepended { commits, head } => {
                let rows = self.commit_rows(store, folder, commits);
                let Some(mut entry) = self.folders.get(folder).cloned() else {
                    return;
                };
                let mut all = rpds::VectorSync::new_sync();
                for commit in rows {
                    all.push_back_mut(commit);
                }
                for commit in entry.commits.iter() {
                    all.push_back_mut(commit.clone());
                }
                entry.commits = all;
                entry.head = head;
                self.folders.insert_mut(folder.clone(), entry);
            }
        }
    }
}

pub struct FetchCommitFiles {
    pub history: imba::store::Id<History>,
    pub folder: ResourceLocation,
    pub commit: crate::hichanges::Revision,
}

impl imba::command::DynamicCommand for FetchCommitFiles {
    fn id(&self) -> &'static str {
        "history.fetch-commit"
    }
    fn name(&self) -> String {
        "Fetch Commit".to_owned()
    }
    fn perform(&self, store: &mut Store, _ui: &imba::UiCtx, _fx: &mut imba::command::Fx<'_>) {
        History::ask(
            store,
            self.history,
            HistoryAsk::CommitFiles(self.folder.clone(), self.commit.clone()),
        );
    }
}

pub struct GrowHistory {
    pub history: imba::store::Id<History>,
    pub folder: ResourceLocation,
}

impl imba::command::DynamicCommand for GrowHistory {
    fn id(&self) -> &'static str {
        "history.grow"
    }
    fn name(&self) -> String {
        "Show More History".to_owned()
    }
    fn perform(&self, store: &mut Store, _ui: &imba::UiCtx, _fx: &mut imba::command::Fx<'_>) {
        History::ask(store, self.history, HistoryAsk::Grow(self.folder.clone()));
    }
}

pub struct CommitHistory {
    pub history: imba::store::Id<History>,
    pub folder: ResourceLocation,
    pub message: String,
}

impl imba::command::DynamicCommand for CommitHistory {
    fn id(&self) -> &'static str {
        "history.commit"
    }
    fn name(&self) -> String {
        "Commit".to_owned()
    }
    fn perform(&self, store: &mut Store, _ui: &imba::UiCtx, _fx: &mut imba::command::Fx<'_>) {
        if self.message.trim().is_empty() {
            return;
        }
        History::ask(
            store,
            self.history,
            HistoryAsk::Commit(self.folder.clone(), self.message.clone()),
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
                source: crate::hichanges::CanvasSource::Commit {
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

pub fn graph_node(
    store: &Store,
    history: imba::store::Id<History>,
    folder: &ResourceLocation,
    held: Option<&FolderHistory>,
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
            tint: hikit::TreeTint::Label,
            action: None,
            children: Vec::new(),
        }
    };
    let children = match held {
        None => vec![note("no history source", &key, items)],
        Some(entry) => match (&entry.status, entry.commits.is_empty()) {
            (ChangesStatus::Error(message), _) => vec![note(message, &key, items)],
            (ChangesStatus::Computing, true) => vec![note("computing…", &key, items)],
            (ChangesStatus::Ready, true) => vec![note("no commits", &key, items)],
            _ => {
                let mut rows = Vec::new();
                for commit in entry.commits.iter() {
                    let commit_key = folder.child(ResourceType::new("history-commit"), &commit.id);
                    items.insert_mut(
                        commit_key.clone(),
                        RowItem::Open {
                            source: crate::hichanges::CanvasSource::Commit {
                                folder: folder.clone(),
                                id: crate::hichanges::Revision::new(commit.id.clone()),
                            },
                            reveal: None,
                            toggle: true,
                            select: true,
                        },
                    );

                    let trail: Vec<(String, skia_safe::Color)> = Vec::new();
                    let held = store.entity(history).and_then(|held| {
                        crate::hichanges::Changes::commit_set(
                            store,
                            held.changes,
                            folder,
                            &crate::hichanges::Revision::new(commit.id.clone()),
                        )
                    });
                    let held = held.filter(|set| set.generation() > 0);
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
                        tint: hikit::TreeTint::Label,
                        action: None,
                        children,
                    });
                }
                if entry.more.is_some() {
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
                        tint: hikit::TreeTint::Label,
                        action: None,
                        children: Vec::new(),
                    });
                }
                rows
            }
        },
    };

    let mut label = folder.name().to_owned();
    if let Some(branch) = held.and_then(|entry| entry.head.branch.as_deref()) {
        label = format!("{label} — {branch}");
    }
    ForestNode {
        key,
        label,
        pick: false,
        dim: false,
        trail: Vec::new(),
        tint: hikit::TreeTint::Label,
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

    #[doc(hidden)]
    pub fn of(commit: &CommitInfo) -> Self {
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
                let theme = editor::env::Themes::of(store);
                let chrome = theme.ui().combo.clone();
                let colors = theme.ui().peeker.clone();
                let font = hikit::fonts::ui_text_font(ui, chrome.value_size);
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

pub fn commit_tip(
    rows: &ListKeyboardController<ForestList<ResourceLocation>, ForestSearcher<ResourceLocation>>,
    store: &Store,
    history: imba::store::Id<History>,
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
    let entry = History::folder(store, history, &folder)?;
    let commit = entry.commits.iter().find(|commit| commit.id == id)?;
    Some((anchor, CommitTip::of(commit)))
}
