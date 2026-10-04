// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

//! The diff canvas feed (docs/editor/diff-canvas.md): what a canvas panel
//! renders is what the changes/history stores ALREADY adopt — this
//! module only shapes it. The panel itself lives in `plugins/hidiff`
//! (the workbench face rule) and is minted through `PaneRow`.

use imba::store::Store;

use changesview::hichanges::{empty_side, ChangeEntry, Changes, ChangesStatus};
use editor::location::ResourceLocation;

use changesview::hichanges::CanvasSource;

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
    /// The owning store's generation at which the host last touched
    /// this entry (`ChangeEntry::updated`) — the canvas reconcile
    /// rebuilds a row exactly when this moves past the row's build.
    pub updated: u64,
}

#[derive(Clone, PartialEq, Debug)]
pub enum CanvasListing {
    /// Nothing to lay rows for yet — the note tells the user why.
    /// TRANSIENT (computing, error, no source): a populated canvas
    /// holds its rows through it.
    Pending(String),
    /// The set is genuinely empty (Ready status, zero files — the
    /// after-commit state): a populated canvas must reconcile to
    /// empty, not hold stale rows.
    Empty(String),
    Ready(Vec<CanvasFile>),
}

/// What the canvas's FIRST row shows, per source: a commit canvas
/// heads with the commit's message and author; the working-copy
/// canvas heads with the commit composer (the box that used to sit in
/// the changes dock).
#[derive(Clone, PartialEq, Debug)]
pub enum CanvasBanner {
    Composer {
        folder: ResourceLocation,
    },
    Commit {
        /// The full message when the host sent one, the summary
        /// otherwise.
        message: String,
        author: String,
    },
}

pub fn canvas_banner(
    store: &Store,
    changes: imba::store::Id<Changes>,
    source: &CanvasSource,
) -> Option<CanvasBanner> {
    match source {
        CanvasSource::WorkingCopy { folder } => Some(CanvasBanner::Composer {
            folder: folder.clone(),
        }),
        CanvasSource::Commit { folder, id } => {
            let history = Changes::of(store, changes)?.history();
            let held = changesview::hihistory::History::folder(store, history, folder)?;
            let commit = held
                .commits
                .iter()
                .find(|commit| commit.id == id.as_str())?;
            Some(CanvasBanner::Commit {
                message: commit
                    .message
                    .clone()
                    .unwrap_or_else(|| commit.summary.clone()),
                author: match &commit.author.email {
                    Some(email) => format!("{} <{}>", commit.author.name, email),
                    None => commit.author.name.clone(),
                },
            })
        }
    }
}

/// The per-frame staleness probe — O(1), no listing built. The panel
/// derives rows only when this moves (the ReconcileShell contract).
pub fn canvas_generation(
    store: &Store,
    changes: imba::store::Id<Changes>,
    source: &CanvasSource,
) -> u64 {
    match source {
        // Per-SET staleness (docs/model-view.md): a canvas re-derives
        // when ITS set moved, not when anything in the session did.
        CanvasSource::WorkingCopy { folder } => Changes::folder_generation(store, changes, folder),
        CanvasSource::Commit { folder, id } => {
            Changes::commit_generation(store, changes, folder, id)
        }
    }
}

/// The staleness probe + the listing, in one read. The generation is
/// the owning store's (`Changes` / `History`) — the panel re-derives
/// on movement, the ReconcileShell contract.
pub fn canvas_files(
    store: &Store,
    changes: imba::store::Id<Changes>,
    source: &CanvasSource,
) -> (u64, CanvasListing) {
    match source {
        CanvasSource::WorkingCopy { folder } => {
            let generation = Changes::folder_generation(store, changes, folder);
            let listing = match Changes::folder(store, changes, folder) {
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
            // Per-SET staleness: the commit's own ChangeSet carries
            // the content and the generation (docs/model-view.md).
            let generation = Changes::commit_generation(store, changes, folder, id);
            let held =
                Changes::commit_set(store, changes, folder, id).filter(|set| set.generation() > 0);
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
        (ChangesStatus::Ready, true) => CanvasListing::Empty("no changes".to_owned()),
        _ => {
            // TREE order — the changes view's `dir_forest` walk
            // (subdirectories first, alphabetical, then files): the
            // canvas must list files in the exact order the tree
            // shows them. Sorting here also makes the listing STABLE
            // across host touches (the feed re-appends on every
            // touch), which is what lets the canvas keep row order
            // without rows jumping.
            let mut entries: Vec<&ChangeEntry> = files.collect();
            entries.sort_by(|a, b| tree_order(&a.rel, &b.rel));
            CanvasListing::Ready(
                entries
                    .into_iter()
                    .map(|entry| {
                        let (old, new) = pair(entry);
                        CanvasFile {
                            title: entry.rel.join("/"),
                            old: old.unwrap_or_else(|| empty_side(&new)),
                            new,
                            added: entry.added,
                            removed: entry.removed,
                            updated: entry.updated,
                        }
                    })
                    .collect(),
            )
        }
    }
}

/// The changes tree's traversal order over two relative paths:
/// at each level, entries descending into a subdirectory come before
/// files of that directory, and siblings sort alphabetically.
fn tree_order(a: &[String], b: &[String]) -> std::cmp::Ordering {
    let mut level = 0;
    loop {
        match (level + 1 == a.len(), level + 1 == b.len()) {
            (true, true) => return a[level].cmp(&b[level]),
            (true, false) => return std::cmp::Ordering::Greater,
            (false, true) => return std::cmp::Ordering::Less,
            (false, false) => match a[level].cmp(&b[level]) {
                std::cmp::Ordering::Equal => level += 1,
                other => return other,
            },
        }
    }
}

/// The canvas's navigation place. Canvases are STORE-HELD state
/// (hidiff's `Canvases` collection); the panel is a reference view,
/// and opening one goes through the ordinary navigation road: the
/// focused pane answers `navigate_to` in place, otherwise the
/// registered canvas navigator finds — or creates — the source's
/// canvas and hands back a view of it. Equality is the SOURCE alone:
/// the reveal is a delivery, not an identity.
#[derive(Clone)]
pub struct CanvasPlace {
    /// The collection the canvas reads from — a walk back has to land
    /// in the same one.
    pub changes: imba::store::Id<Changes>,
    pub source: CanvasSource,
    pub reveal: Option<ResourceLocation>,
}

impl PartialEq for CanvasPlace {
    fn eq(&self, other: &Self) -> bool {
        self.source == other.source
    }
}

impl hikit::navigation::Place for CanvasPlace {}
