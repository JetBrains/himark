// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

//! The diff canvas feed (docs/diff-canvas.md): what a canvas panel
//! renders is what the changes/history stores ALREADY adopt — this
//! module only shapes it. The panel itself lives in `plugins/hidiff`
//! (the workbench face rule) and is minted through `FamilyRow`.

use imba::store::Store;

use crate::hichanges::{empty_side, ChangeEntry, Changes, ChangesStatus};
use crate::{ResourceLocation, WindowId};

#[derive(Clone, PartialEq, Debug)]
pub enum CanvasSource {
    /// The uncommitted changeset of a workspace folder — the changes
    /// view's root row.
    WorkingCopy { folder: ResourceLocation },

    /// One commit's changeset — a history view revision row.
    Commit {
        folder: ResourceLocation,
        id: String,
    },
}

impl CanvasSource {
    pub fn folder(&self) -> &ResourceLocation {
        match self {
            CanvasSource::WorkingCopy { folder } => folder,
            CanvasSource::Commit { folder, .. } => folder,
        }
    }

    pub fn title(&self, store: &Store) -> String {
        match self {
            CanvasSource::WorkingCopy { folder } => format!("Changes — {}", folder.name()),
            CanvasSource::Commit { folder, id } => {
                let summary = crate::hihistory::History::folder(store, folder).and_then(|held| {
                    held.commits
                        .iter()
                        .find(|commit| commit.id == *id)
                        .map(|commit| commit.summary.clone())
                });
                match summary {
                    Some(summary) => summary,
                    None => {
                        let short: String = id.chars().take(8).collect();
                        format!("Commit {short}")
                    }
                }
            }
        }
    }
}

/// One canvas item: the pair the diff compares, normalized the way
/// the tree rows activate today (absent sides become the empty
/// authority). `new` doubles as the row key and the reveal key —
/// it is exactly what the tree's `RowItem::File` carries.
#[derive(Clone, PartialEq, Debug)]
pub struct CanvasFile {
    pub title: String,
    pub old: ResourceLocation,
    pub new: ResourceLocation,
    pub added: Option<i64>,
    pub removed: Option<i64>,
}

#[derive(Clone, PartialEq, Debug)]
pub enum CanvasListing {
    /// Nothing to lay rows for yet — the note tells the user why.
    Pending(String),
    Ready(Vec<CanvasFile>),
}

/// The per-frame staleness probe — O(1), no listing built. The panel
/// derives rows only when this moves (the ReconcileShell contract).
pub fn canvas_generation(store: &Store, source: &CanvasSource) -> u64 {
    match source {
        CanvasSource::WorkingCopy { .. } => Changes::generation(store),
        CanvasSource::Commit { .. } => crate::hihistory::History::generation(store),
    }
}

/// The staleness probe + the listing, in one read. The generation is
/// the owning store's (`Changes` / `History`) — the panel re-derives
/// on movement, the ReconcileShell contract.
pub fn canvas_files(store: &Store, source: &CanvasSource) -> (u64, CanvasListing) {
    match source {
        CanvasSource::WorkingCopy { folder } => {
            let generation = Changes::generation(store);
            let listing = match Changes::folder(store, folder) {
                None => CanvasListing::Pending("no changes source".to_owned()),
                Some(changes) => listing_of(&changes.status, changes.files.iter(), |entry| {
                    // The working-copy pair diffs the LIVE file — the
                    // same pair the changes tree activates.
                    (entry.before.clone(), entry.working.clone())
                }),
            };
            (generation, listing)
        }
        CanvasSource::Commit { folder, id } => {
            let generation = crate::hihistory::History::generation(store);
            let held = crate::hihistory::History::folder(store, folder)
                .and_then(|held| held.commit_files.get(id).cloned());
            let listing = match held {
                None => CanvasListing::Pending("fetching the commit…".to_owned()),
                Some(commit) => listing_of(&commit.status, commit.files.iter(), |entry| {
                    (
                        entry.before.clone(),
                        entry
                            .after
                            .clone()
                            .unwrap_or_else(|| empty_side(&entry.working)),
                    )
                }),
            };
            (generation, listing)
        }
    }
}

fn listing_of<'a>(
    status: &ChangesStatus,
    files: impl ExactSizeIterator<Item = &'a ChangeEntry>,
    pair: impl Fn(&ChangeEntry) -> (Option<ResourceLocation>, ResourceLocation),
) -> CanvasListing {
    match (status, files.len() == 0) {
        (ChangesStatus::Error(message), _) => CanvasListing::Pending(message.clone()),
        (ChangesStatus::Computing, true) => CanvasListing::Pending("computing…".to_owned()),
        (ChangesStatus::Ready, true) => CanvasListing::Pending("no changes".to_owned()),
        _ => CanvasListing::Ready(
            files
                .map(|entry| {
                    let (old, new) = pair(entry);
                    CanvasFile {
                        title: entry.rel.join("/"),
                        old: old.unwrap_or_else(|| empty_side(&new)),
                        new,
                        added: entry.added,
                        removed: entry.removed,
                    }
                })
                .collect(),
        ),
    }
}

/// The reveal mailbox: `OpenDiffCanvas` posts here, the canvas
/// panel's paint probe sees a match and its perform TAKES it. A slot
/// rather than an event so it survives the panel being minted in the
/// same batch that armed it.
#[derive(Clone, Default)]
pub struct CanvasReveal(Option<(CanvasSource, ResourceLocation)>);

impl CanvasReveal {
    pub fn post(store: &mut Store, source: CanvasSource, key: ResourceLocation) {
        store.put(CanvasReveal(Some((source, key))));
    }

    /// The widget-side probe — read-only, cheap.
    pub fn pending_for(store: &Store, source: &CanvasSource) -> bool {
        store
            .get::<CanvasReveal>()
            .and_then(|slot| slot.0.as_ref())
            .is_some_and(|(held, _)| held == source)
    }

    pub fn take_for(store: &mut Store, source: &CanvasSource) -> Option<ResourceLocation> {
        let held = store.get::<CanvasReveal>()?.0.clone()?;
        if held.0 != *source {
            return None;
        }
        store.put(CanvasReveal(None));
        Some(held.1)
    }
}

/// Open (or re-mint) the canvas for a source; an armed reveal rides
/// the mailbox. The panel type lives in `plugins/hidiff` — minting
/// goes through the `FamilyRow` road like every restorable panel.
pub struct OpenDiffCanvas {
    pub source: CanvasSource,
    pub reveal: Option<ResourceLocation>,
}

impl crate::DynamicCommand for OpenDiffCanvas {
    fn id(&self) -> &'static str {
        "diff.open-canvas"
    }
    fn name(&self) -> String {
        "Open Diff Canvas".to_owned()
    }
    fn perform(
        &self,
        _app: &mut crate::Application,
        store: &mut Store,
        window: WindowId,
        fx: &mut crate::AppFx<'_>,
    ) {
        if let Some(key) = &self.reveal {
            CanvasReveal::post(store, self.source.clone(), key.clone());
        }
        // A commit canvas needs its changeset — the same fetch the
        // tree's expansion runs; the pending set dedups a double ask.
        if let CanvasSource::Commit { folder, id } = &self.source {
            crate::DynamicCommand::perform(
                &crate::hihistory::FetchCommitFiles {
                    folder: folder.clone(),
                    commit: id.clone(),
                },
                _app,
                store,
                window,
                fx,
            );
        }
        let row = crate::FamilyRow::Canvas(self.source.clone());
        let Some(panel) = crate::family_rows::mint(store, &row) else {
            eprintln!("[himark] no canvas minter registered");
            return;
        };
        let Some(mut entity) = crate::Windows::window(store, window) else {
            return;
        };
        let _ = entity.open_panel(store, panel, fx);
        crate::Windows::put(store, window, entity);
    }
}
