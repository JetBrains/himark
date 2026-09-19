// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use imba::effect::{AnyEffect, Effect};
use imba::store::Store;

fn probe() -> bool {
    static ENABLED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ENABLED.get_or_init(|| std::env::var_os("HIMARK_WATCH_PROBE").is_some())
}
use crate::{FetchDocumentEffect, OpenDocuments};
use editor::ResourceLocation;
use imba::effect::Effects;

#[derive(Clone, Default)]
pub struct Watching;

impl Watching {
    pub fn install(store: &mut Store) {
        store.put(Watching);
    }

    pub fn installed(store: &Store) -> bool {
        store.get::<Watching>().is_some()
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Subscription(pub u64);

pub struct SubscribeEffect {
    pub location: ResourceLocation,
}

impl Effect for SubscribeEffect {
    type Result = Option<Subscription>;
}

pub struct UnsubscribeEffect {
    pub subscription: Subscription,
}

impl Effect for UnsubscribeEffect {
    type Result = ();
}

pub struct FilesChanged(pub std::sync::Arc<std::collections::HashSet<Subscription>>);

pub struct FileChanged {
    pub subscription: Subscription,
}

pub struct RefetchDiffEffect {
    pub baseline: editor::Text,

    pub current: editor::Text,

    pub fetched: String,

    /// The edge-installed policy (`editor::env::Differ`), captured at
    /// launch. The merge path passes no syntax — it wants the minimal
    /// exact edit, so any policy degrades to its text pass here
    /// (docs/editor/structural-diff.md, decision 3).
    pub policy: std::sync::Arc<dyn editor::diff::DiffPolicy>,
}

pub struct RefetchRebase {
    pub operation: operation::Operation,

    pub fetched: editor::Text,

    /// The disk text as fetched — carried through so a raced landing
    /// can re-diff without an O(file) rope extraction on the UI
    /// thread.
    pub fetched_source: String,

    pub clean: bool,

    /// Applying `operation` lands the buffer EXACTLY on the disk's
    /// text — computed on the worker, where the strings already
    /// exist; the landing must not compare texts on the UI thread.
    pub synced: bool,
}

impl Effect for RefetchDiffEffect {
    type Result = RefetchRebase;
}

pub struct RefetchDiffHandler;

/// One replacement of an operation, in its OLD text's coordinates:
/// where it starts, what it removes, what it puts there.
type Hunk = (u32, String, String);

fn hunks(operation: &operation::Operation) -> Vec<Hunk> {
    let mut out = Vec::new();
    let mut pos = 0u32;
    let mut open: Option<Hunk> = None;
    for step in operation.iter() {
        match step {
            operation::Op::Retain(n) => {
                if let Some(hunk) = open.take() {
                    out.push(hunk);
                }
                pos += n;
            }
            operation::Op::Insert(text) => {
                let hunk = open.get_or_insert((pos, String::new(), String::new()));
                hunk.2.push_str(&text);
            }
            operation::Op::Delete(text) => {
                let hunk = open.get_or_insert((pos, String::new(), String::new()));
                hunk.1.push_str(&text);
                pos += text.len() as u32;
            }
        }
    }
    if let Some(hunk) = open {
        out.push(hunk);
    }
    out
}

/// Three-way merge of the BASELINE's two descendants: what the
/// buffer made of it (ours) and what the disk holds now (theirs).
/// The echo family — the agent's edit arriving first as a shared op
/// and then as its file write — makes the two sides SHARE changes; a
/// plain operational transform cannot see the sharing and applies
/// the same insertion twice. Here shared hunks are taken once:
///
/// - disjoint hunks: both apply;
/// - same spot, same replacement: once;
/// - same spot, one replacement a prefix of the other: the longer
///   side (the other is the same change, not yet fully propagated);
/// - anything else overlapping: the union span is replaced by ours'
///   text then theirs' — nobody's bytes are dropped.
fn merged(base: &str, ours: &[Hunk], theirs: &[Hunk]) -> String {
    let mut out = String::new();
    let mut pos = 0usize;
    let (mut i, mut j) = (0usize, 0usize);
    let copy_to = |out: &mut String, pos: &mut usize, to: usize| {
        if to > *pos {
            out.push_str(&base[*pos..to]);
            *pos = to;
        }
    };
    while i < ours.len() || j < theirs.len() {
        let mine = ours.get(i);
        let disk = theirs.get(j);
        let (start, deleted, inserted, from_ours) = match (mine, disk) {
            (Some(o), Some(t)) => {
                let (o_start, o_end) = (o.0 as usize, o.0 as usize + o.1.len());
                let (t_start, t_end) = (t.0 as usize, t.0 as usize + t.1.len());
                if o_end <= t_start && o_start != t_start {
                    (o_start, &o.1, &o.2, true)
                } else if t_end <= o_start && o_start != t_start {
                    (t_start, &t.1, &t.2, false)
                } else if o_start == t_start && o.1 == t.1 {
                    // The same base span replaced on both sides: the
                    // shared change, possibly further along on one.
                    i += 1;
                    j += 1;
                    copy_to(&mut out, &mut pos, o_start);
                    pos += o.1.len();
                    if t.2.starts_with(&o.2) {
                        out.push_str(&t.2);
                    } else if o.2.starts_with(&t.2) {
                        out.push_str(&o.2);
                    } else {
                        out.push_str(&o.2);
                        out.push_str(&t.2);
                    }
                    continue;
                } else {
                    // A messy overlap: replace the UNION of the two
                    // base spans with ours' text, then theirs'.
                    let union_start = o_start.min(t_start);
                    let union_end = o_end.max(t_end);
                    i += 1;
                    j += 1;
                    copy_to(&mut out, &mut pos, union_start);
                    pos = union_end;
                    out.push_str(&o.2);
                    out.push_str(&t.2);
                    continue;
                }
            }
            (Some(o), None) => (o.0 as usize, &o.1, &o.2, true),
            (None, Some(t)) => (t.0 as usize, &t.1, &t.2, false),
            (None, None) => unreachable!(),
        };
        match from_ours {
            true => i += 1,
            false => j += 1,
        }
        copy_to(&mut out, &mut pos, start);
        pos += deleted.len();
        out.push_str(inserted);
    }
    out.push_str(&base[pos..]);
    out
}

pub(crate) fn text_string(text: &editor::Text) -> String {
    let mut view = text.view();
    let end = view.byte_count().min(u32::MAX as usize) as u32;
    view.substring(0..end)
}

impl imba::effect::EffectHandler<RefetchDiffEffect> for RefetchDiffHandler {
    async fn handle(&self, effect: RefetchDiffEffect) -> RefetchRebase {
        let fetched = editor::Text::from_string_exact(&effect.fetched);
        // The buffer already IS the disk — the fetch is the echo of
        // changes the document holds (the agent's shared edits, our
        // own save). Nothing to apply; the document is fully synced.
        if text_string(&effect.current) == effect.fetched {
            let len = effect.current.byte_count().min(u32::MAX as usize) as u32;
            return RefetchRebase {
                operation: operation::Operation::from_ops([operation::Op::Retain(len)]),
                fetched,
                fetched_source: effect.fetched,
                clean: true,
                synced: true,
            };
        }
        let theirs = effect.policy.diff(&effect.baseline, &fetched, None);
        let ours = effect.policy.diff(&effect.baseline, &effect.current, None);
        let clean = ours.iter().all(|op| matches!(op, operation::Op::Retain(_)));
        let (operation, synced) = match clean {
            true => (theirs, true),
            false => {
                // Merge the two descendants of the baseline, then
                // aim the buffer at the merge DIRECTLY — positions
                // computed against the current text, nothing
                // transformed past anything.
                let target = merged(
                    &text_string(&effect.baseline),
                    &hunks(&ours),
                    &hunks(&theirs),
                );
                let synced = target == effect.fetched;
                (
                    effect.policy.diff(
                        &effect.current,
                        &editor::Text::from_string_exact(&target),
                        None,
                    ),
                    synced,
                )
            }
        };
        RefetchRebase {
            operation,
            fetched,
            fetched_source: effect.fetched,
            clean,
            synced,
        }
    }
}

pub fn sync_document_watches<R: 'static>(
    store: &mut Store,
    fx: &mut Effects<'_, R>,
    wrap: impl Fn(crate::DocumentId, Option<Subscription>) -> R + Send + Clone + 'static,
) {
    if !Watching::installed(store) {
        return;
    }
    for (document, entity) in OpenDocuments::list(store) {
        if entity.watch.is_some() || entity.watch_requested || entity.host_synced {
            continue;
        }
        let Some(location) = entity.location.clone() else {
            continue;
        };

        if crate::is_synthetic(&location) {
            continue;
        }
        OpenDocuments::set_watch_requested(store, document);
        if probe() {
            eprintln!("[watch] sweep: subscribing /{}", location.path().join("/"));
        }
        let _ = fx.push(AnyEffect::new(SubscribeEffect { location }).map({
            let wrap = wrap.clone();
            move |subscription| wrap(document, subscription)
        }));
    }
}

pub fn refetch_watched<R: 'static>(
    store: &mut Store,
    subscription: Subscription,
    fx: &mut Effects<'_, R>,
    wrap: impl Fn(crate::DocumentId, u64, Option<String>) -> R + Send + Clone + 'static,
) {
    let riders = store
        .get::<OpenDocuments>()
        .map(|documents| documents.watch_riders(subscription))
        .unwrap_or_default();
    if probe() {
        eprintln!(
            "[watch] FileChanged #{} -> {} open document(s) re-fetch",
            subscription.0,
            riders.len()
        );
    }
    for document in riders {
        let Some(entity) = OpenDocuments::entity(store, document) else {
            continue;
        };
        if entity.host_synced {
            continue;
        }
        let Some(location) = entity.location else {
            continue;
        };

        let serial = OpenDocuments::stamp_refetch(store, document);
        let _ = fx.push(AnyEffect::new(FetchDocumentEffect { location }).map({
            let wrap = wrap.clone();
            move |text| wrap(document, serial, text)
        }));
    }
}

/// Re-run the diff for a fetch whose landing raced a fresher
/// revision: same serial (it is still the newest disk text — a newer
/// fetch supersedes it by serial), fresh baseline/current/revision.
pub fn rediff<R: 'static>(
    store: &mut Store,
    document_id: crate::DocumentId,
    serial: u64,
    fetched: String,
    fx: &mut Effects<'_, R>,
    wrap: impl Fn(crate::DocumentId, u64, u64, RefetchRebase) -> R + Send + 'static,
) {
    let Some(entity) = OpenDocuments::entity(store, document_id) else {
        return;
    };
    if entity.refetch_serial != serial {
        return;
    }
    let Some(document) = OpenDocuments::document_ref(store, document_id) else {
        return;
    };
    if probe() {
        eprintln!(
            "[watch] landing raced revision {} — re-diffing",
            document.revision()
        );
    }
    let base_revision = document.revision();
    let _ = fx.push(
        AnyEffect::new(RefetchDiffEffect {
            baseline: entity.baseline.clone(),
            current: document.text().clone(),
            fetched,
            policy: editor::env::Differ::of(store),
        })
        .map(move |rebase| wrap(document_id, base_revision, serial, rebase)),
    );
}

pub fn apply_refetched<R: 'static>(
    store: &mut Store,
    document_id: crate::DocumentId,
    serial: u64,
    text: Option<String>,
    fx: &mut Effects<'_, R>,
    wrap: impl Fn(crate::DocumentId, u64, u64, RefetchRebase) -> R + Send + 'static,
) {
    let Some(text) = text else {
        if probe() {
            eprintln!("[watch] refetch landed: gone/unreadable — keeping ours");
        }
        return;
    };
    let Some(entity) = OpenDocuments::entity(store, document_id) else {
        return;
    };
    if entity.refetch_serial != serial {
        if probe() {
            eprintln!(
                "[watch] refetch landed: superseded (serial {serial} vs {}) — dropped",
                entity.refetch_serial
            );
        }
        return;
    }
    let Some(document) = OpenDocuments::document_ref(store, document_id) else {
        return;
    };
    if probe() && document.revision() != entity.saved_revision {
        eprintln!(
            "[watch] refetch landed: dirty (revision {} vs saved {}) — merging over the baseline",
            document.revision(),
            entity.saved_revision
        );
    }
    let base_revision = document.revision();
    let _ = fx.push(
        AnyEffect::new(RefetchDiffEffect {
            baseline: entity.baseline.clone(),
            current: document.text().clone(),
            fetched: text,
            policy: editor::env::Differ::of(store),
        })
        .map(move |rebase| wrap(document_id, base_revision, serial, rebase)),
    );
}
