# Documents: the storage

Where documents live, who may hold one, and when one dies — the
STORAGE story; the value's own anatomy is
[document.md](document.md). There is exactly one answer to each: the `OpenDocuments` repository owns every
document; views hold `(DocumentId, EditorId)` pairs, never values; and
a document leaves when its last editor does.

## The store is a catalog of repositories

`imba::Store` holds ONE value per type — `TypeId → Component`, with
`get::<T>()`, `put(value)`, and `update::<T: Default>(f)` (clone-or-
default, mutate, put). It mints no ids, holds no per-entity rows, and
has no `insert`/`iter`/`remove`. Identity lives INSIDE the domain
repositories, as keys the domain mints beside the records it owns:

| repository        | key           | owns                                   |
|-------------------|---------------|----------------------------------------|
| `OpenDocuments`   | `DocumentId`  | every document + its identity record   |
| `Windows`         | `WindowId`    | every window's layers/viewport         |
| registries (`Commands`, `ToolbarButtons`, themes/fonts/parsers, …) | — | plain singleton values |

The store stays a persistent HAMT: a `clone()` is an O(1) snapshot, a
frame renders from a value that can never see a half-applied mutation,
and `&mut Store` inside one `perform` is read-your-writes. Repositories
keep those properties by being persistent values themselves
(`rpds::HashTrieMapSync` entries, monotonic `next` mints, ids never
reused).

The store a dispatch receives is a per-(window, session) PROJECTION
of the app value (docs/ui/app-state.md), and `OpenDocuments` is a
SESSION family in it: the same file open in two sessions is two
documents, each session's registry gathered flat when its window
dispatches. The id mint is app-global, so a document-addressed
landing locates its session by forest scan and discards cleanly when
the session is gone. Everything below reads the same either way —
dispatch code sees one `OpenDocuments` component.

## The document repository

```rust
pub struct DocumentId(u64);                     // minted at register(), never reused

pub struct OpenDocuments {
    entries: HashTrieMapSync<DocumentId, OpenDocument>,
    by_location: HashTrieMapSync<ResourceLocation, DocumentId>,  // the dedup index
    by_watch: HashTrieMapSync<Subscription, VectorSync<DocumentId>>,
    diffs: Diffs,        // tracked diff pairs / stripe bases (docs/editor/diff-stripes.md)
}

pub struct OpenDocument {                       // fields crate-private — the
    document: Document,                         // record's SHAPE is the registry's
    location: Option<ResourceLocation>,         // business; consumers read through
    title: String,                              // name()/location()/document()/
    registered: u64,     // open-order stamp       saved_revision()/watch()…
    last_opened: u64,    // recency stamp
    saved_revision: u64, // the dirty flag's other half
    baseline: Text,      // the location's content as last known (open/
                         // save/refetch) — the external-MERGE ancestor:
                         // a re-fetch three-way merges ours and theirs
                         // over it (shared hunks once, conflicts keep
                         // both), so unsaved edits survive external
                         // changes without duplicating an edit that
                         // arrived twice (a plain OT transform would —
                         // it cannot tell a shared insertion from two)
    refetch_serial: u64, // THE refetch lane (last-LAUNCHED wins): both
                         // landings present it or drop — otherwise a
                         // slow fetch of OLD content landing late
                         // reverts a fresh reload permanently. A
                         // landing racing a fresher REVISION is not
                         // dropped: it re-diffs against the new buffer
                         // under the same serial until it converges
    save_token: …,       // THE save lane (docs/ahp/files.md)
    watch: …, watch_requested: bool,            // the host watch (docs/ahp/files.md)
    host_synced: bool,   // a LIVE document channel makes the AHP host
                         // the reload authority (docs/ahp/ahp-documents.md
                         // §6): while set, no client watch, no client
                         // refetch landings — external changes arrive
                         // as the host's own shared edits; cleared on
                         // channel death, which re-arms the watch and
                         // refetches once
    base_requested: bool, // the stripes-base ask ran (docs/editor/diff-stripes.md)
}
```

The entity holds THE document — the single authoritative copy — beside
the identity record that connects it to the outside world: its
[`ResourceLocation`] (docs/ahp/files.md), the watch that re-fetches on
external change, the save lane, the stamps lists sort by. There are no
loose documents anywhere else; `register` is the only way a document
comes to exist in shared state:

```rust
OpenDocuments::register(store, document, location, title, saved_revision) -> DocumentId
```

Registration and release are also the OBSERVATION seam:
`OpenDocuments::install_hook` keeps a list of `DocumentHook`s
(`opened`/`closing`), fired at `register` and at release. Document
sync channels attach through it (`DocsyncHook` — docs/editor/rebase.md §6)
and comment cards materialize through it (`CommentsHook` —
docs/ahp/comments.md); nothing polls the registry.

**Scratches are located too**: every scratch mints a `scratch://`
identity (`next_scratch_location` — the path tail is the display name,
"scratch", "scratch 2", …), so it lists, joins the recents and
survives implicit releases; there is nowhere to re-fetch a scratch
from, so only the explicit close (`remove_on_close`, cmd-w) may
release a clean one. SYNTHETIC identities (scratch, demo —
`ResourceLocation::is_synthetic`) carry no host services: the watch
sweep skips them, editor-scoped commands skip them by default
(`DynamicEditorCommand::offers_at`), and the change seam stays quiet.
SAVE is the opt-in exception on hosts with a destination picker:
`file.save` on a synthetic identity runs
SAVE-AS — the host's save panel picks a real location
(`PickSaveEffect`), and the landing RE-POINTS the record
(`OpenDocuments::set_location`): the buffer and its editors stay, the
identity changes under them, the recents entry follows
(`RecentLocations::replace`), the store writes, and the watch sweep
subscribes the new place. A scratch becomes a file without anyone
losing their seat.

The shared `himark::SaveDocument` command is registered with
`with_save_as()` by the native host and `existing_files()` by web alongside
its store handler. Web offers Save only for non-synthetic, non-revision-pinned
locations; no picker is needed to write those files back to their host.
However launched, a save is one `StoreDocumentEffect` on the entity's
save lane; the handler routes it — through the document channel's
`storeDocument` request when the host mirrors the document, the plain
resource write otherwise (docs/ahp/files.md, Saving) — and the landing
stamps `saved_revision` and the baseline (`mark_saved`).

**Membership rule: has a location, lives here.** Everything a surface
displays registers the moment it is displayed — a real open, a peeker
preview fetch, a search hit fetched from the workspace, a references-
panel prefetch, and BOTH sides of a vcs diff (the revision-pinned old
side registers under its changeset before-ref location,
`saved_revision` = its build revision, so it is born clean;
docs/ahp/vcs.md). Registration is
also the dedup point: `by_location` answers the existing id, so opening
— or diffing, or previewing — the same resource twice converges on one
document. The only documents outside the registry are anonymous values
a view owns outright (a comment card's little markdown document) —
plain values, nobody else's business.

## Identity in views, values in the registry

A pane's node in the view tree is pure identity plus presentation
flags:

```rust
pub struct EditorIdView { document: DocumentId, editor: EditorId, blurred: bool }
```

Every layout GATHERS a fresh clone (`OpenDocuments::document(_ref)` —
cheap, persistent innards), binds it to the pane's editor, and drops it
with the frame; every `perform` is the same round trip with a SCATTER
(`put_document`) at the end. Nothing caches a document, so every editor
of a document sees every edit in the same pass. hidiff's `DiffPair` is
this scheme doubled; search rows and the document list are lists of it.

Per-editor view state (layout, carets, focus, viewport) lives inside
the `Document` keyed by `EditorId` — so the pair `(DocumentId,
EditorId)` is a complete address. Effects launched for a document carry
that address rather than being resolved by scanning: `entity_scope`
stamps the `DocumentId` at launch and the landing
(`AppCommand::Entity(DocumentId, EditorCommand)`) is a direct registry
lookup; a stale editor's items self-discard inside `Document::perform`.

## The retraction rule: editors are the refcount

A view that is done showing a document retracts its editors
(`close_editor`, or its own remove_editor round trip), then calls

```rust
OpenDocuments::remove_if_editorless(store, id, fx)
```

A document left with NO editors was only ever that view's — it leaves
whole: the registry entry, the document, and its host watch (the
unsubscribe rides `fx`). A document anyone else still shows keeps at
least one editor and stays untouched. **Except a DIRTY one**: unsaved
edits are never released automatically — a document whose revision is
past its `saved_revision` stays registered however editorless it ends
up. A document serving as a tracked diff's base or target is spared
too (the release checks the `diffs` tracking — docs/editor/diff-stripes.md; a
stripes base is editorless by nature). A SCRATCH is spared by every
implicit release too — cmd-w's
`remove_on_close` is the one path that kills a clean one. This is the
entire lifecycle story; there is no ownership flag anywhere:

- **Registration follows display.** A navigation displacing a pane —
  and cmd-w closing one — retracts its editor, and a clean document
  left editorless is RELEASED. Nothing is lost: unsaved edits are
  dirty and the release spares them, and the way back is the RECENTS
  list (docs/ui/navigation.md), which remembers places separately from
  keeping documents around — re-opening is an ordinary fetch by
  location.
- **Search previews die with the set**: the install drain retracts the
  row editors, and the sweep runs AFTER the next set installs, so a
  document re-matched by the new query keeps continuity instead of
  flickering through a remove/re-fetch.
- **Peeker previews die at close** (or on re-point away), except the
  picked one — the pick's `ShowDocument` gives it a pane editor within
  the same drain, and the cleanup's `keep` spares it the editorless
  instant in between.
- **Diff sides**: dismantle retracts both halves' editors; the
  revision-pinned side (only ever shown by the pane) goes, the working
  copy (a pane editor elsewhere) stays.
- The honest edge: a registered document that never had a pane editor
  (a background open) reaped by the first list that displays-then-
  drains it — its row editors genuinely were the last.

## Destroy: how retraction is guaranteed to run

The rule above only works if retraction actually RUNS at every
discard. That is `View::destroy` (docs/ui/UI.md — containers forward,
owners call it at every discard site), and the document side of it is
one impl: `EditorIdView::destroy` retracts its editor and calls
`remove_if_editorless` (dirty-guarded, above). Views that show
documents through `EditorIdView` get their document lifecycle for free
from the forwarding; owners with document resources beyond their
subtree (the peeker's fetched-but-never-previewed temp documents)
override `destroy` and release those beside the forwarded call.

The discard site that motivates this: modal dismissal — the toolbar
session ending, the `>` surface swap, `show_modal` replacing —
destroys the outgoing modal. Without it, a peeker dismissed from
OUTSIDE would never run its cleanup, and every document glanced at in
the preview would stay registered forever.

What destroy is NOT: pane displacement and pane close keep their
survival semantics (the buffer-list model above — a stashed widget is
alive, a closed pane's document stays reachable); `PanelView::
dismantle` remains the panel-level unwind the workbench calls on its
own schedule.

## Mounting editors, and where the location lives

One recipe, `lifecycle::mount_editor`, over the document VALUE: env
fetch, `add_editor`, the optional reveal target, the cold-open reparse
launch — answering the `EditorId` the caller pairs with its
`DocumentId`. `close_editor` is the inverse half of a re-point: a
re-point is retract + mount + swap the view's own pair.

**The editor does not know its location.** The location is the
registry RECORD's; the gather JOINS it onto the frame's `EditorView`
(`EditorView.location`), and that located view is the door for every
location-scoped concern: it offers and dispatches the editor-scoped
dynamic commands (docs/ahp/lsp.md §3), fires the change seam once per
revision-moving command (docs/ahp/lsp.md §2), and answers the focus-path
focused-location ask on the focus chain (reveal-in-tree). Anonymous values — inputs,
comment cards — gather no location and owe none of it. External
changes (a watch re-fetch) notify with the registry location directly.

The shape rules out whole bug classes by construction: no loose
document rows means no per-panel `owned` bookkeeping to leak; the
pair living in the view means no stored view↔document indirection to
desynchronize; register-at-display means membership never disagrees
with what is on screen; and landings carrying their address means no
repair ever finds its document by scanning.
