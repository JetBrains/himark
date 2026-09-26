# Model–view: one shape for every collection

The store is the truth, and every surface is a projection of it. This
document names the shape that makes that cheap to maintain, inventories
where we follow it and where we don't, and lays out the migration plan.

## The law

1. **A MODEL is a store collection of truth**, keyed by minted ids,
   owned by its session family where it is session state.
2. **A VIEW STATE is a record living NEXT TO its model** — in the same
   store collection, keyed by its own minted id. One model, many views
   (two windows on one document), exactly like `Document.editors`.
3. **A PANEL/PANE holds only ids.** The workbench and the dock carry
   thin reference views (`EditorIdView`, `PairPane(DiffViewId)`,
   `ChatPane(Uri)`, `DiffCanvasView(CanvasId)`); their `display()`
   reads the store each frame, their death never destroys model state
   (chat learned this the hard way — commit d2c0ac54).
4. **Collections reference each other by ids, weakly.** A diff view
   names two `DocumentId`s and a `DiffId`; a canvas row names a
   `DiffViewId`; a changeset entry names `ResourceLocation`s. No owner
   reaches into another collection's records.
5. **Updates land model-locally.** A landing that mutates the model
   rolls the adjacent view states in the same store mutation — the
   push road. No window walks, no downcasts, no waiting for a mounted
   panel to notice. Paint probes are reserved for LAYOUT-BOUND facts
   (a width that does not exist before layout, viewport nearness),
   never for model transitions.

## The exemplar: Documents → editors

`OpenDocuments` (documents crate) holds `Document`s; each `Document`
holds `editors: Map<EditorId, Editor>` (editor/document.rs:72) — the
per-view state (viewport, caret, focus) beside the text it views. The
workbench pane is `EditorIdView` (document id + editor id). An edit
mutates the document and every editor's state in one place.

## Inventory

| model | view states | today | target |
|---|---|---|---|
| `OpenDocuments` documents | editors | `Document.editors` | done — the exemplar |
| `Diffs` records (documents/diffs.rs:80) | diff views | `Diffs.diff_views: Map<DiffViewId, DiffView>` with `DiffViewState` inside — ALREADY adjacent | done structurally; the update-locality dividend is uncollected (step 1) |
| ChangeSets — today split as `Changes` (hichanges) + `History` (hihistory), session family | tree views AND canvases | trees live on the Window dock as `Box<dyn ModalView>`; canvases live in hidiff's own session-family value | steps 2–4 |
| `Chats` (session family) | chat view | `ChatPanel` fuses transcript model + view furniture in one record; `ChatPane` is a uri reference | step 5, later |

## The plan

### Step 1 — Diffs: roll view bases at the landing

`DiffViewState` already lives beside its `DiffRecord`, but a
`DiffNormalized` landing today only bumps the entry's generation; each
face waits for its PAINT stale-probe (`GatheredSplit`, hidiff) to emit
`Resync`. With the records adjacent there is nothing to wait for:
`land_normalized` (and the edit-door rebase in `apply_base_edits`)
walks `diff_views` filtered by that `DiffId` and rolls every attached
basis in the same mutation. The paint stale-probe retires; only
layout-bound repair (rewrap, viewport heal) stays on paint. This
completes the push doctrine for diffs and is the smallest step.

### Step 2 — The ChangeSets model (ruled 2026-09-26)

A **ChangeSet is one set of changes with its own minted `ChangeSetId`:
one per folder's WORKING COPY, one per COMMIT** — `CanvasSource`'s
shape made first-class. `ChangeSets` is the session-family collection
of these records; each record carries its file entries and its OWN
generation (today's session-wide `Changes.generation` splits per set).

**History is the tricky one, and it splits cleanly:** its MODEL is the
list of commits (per folder), each commit REFERENCING its change set
by `ChangeSetId` — an id reference, weak, like every cross-collection
link. Its VIEW is a single tree UNITING all those change sets.

### Step 3 — Views are owned by their model, referenced by both ids

`Document.editors` literally: the owning record holds
`views: Map<ViewId, ViewState>`, and a reference is BOTH ids —
`(ChangeSetId, ChangesViewId)`, as an editor is
`(DocumentId, EditorId)`. The dock holds thin reference panes; a
pane's `destroy` removes its record (tree rows are pure derivation).
In a multi-folder session the changes tree is one section-view per
working-copy set, composed by the pane.

The single uniting history tree is owned by the History model (the
commit list), keyed by `HistoryViewId` — the one view that belongs to
a LIST of sets rather than a single set.

**The update rule:** whenever a ChangeSet is updated, it updates ALL
its views in the same mutation — its owned ChangesViews (and canvas
views), AND every HistoryView uniting it. No window walks, no
downcasts, no paint probes.

*Landed intermediate:* the tree-view records already moved into the
models (`Changes.views`, `History.views`) with dock reference panes
(`ChangesPane`, `HistoryPane`) and model-local sync;
`ModalView::as_any_mut` retired. The re-keying by `ChangeSetId` comes
with the ChangeSets migration.

### Step 4 — Canvases attach to their ChangeSet (LANDED)

A canvas is the other view of the same set: `ChangeSet.canvases:
Map<CanvasId, Canvas>`, referenced by both ids. RULING (2026-09-26):
the hidiff plugin dissolved into himark rather than keeping a face
crate — `CanvasRow`'s single `imba::View` impl cannot split perform
(state) from display (face) across crates, and the face's real
machinery was editor's `UnifiedDiffView` all along. `diff_pane`
(PairPane, DiffPanelView) and `diff_canvas::canvas` are himark
modules; the canvas sweep is a direct batch-tail lane; canvas effects
ride the first-class sessioned `AppCommand::CanvasViewCommand`. The
plugin mechanisms this had required (SyncObservers, SessionFamilies)
retired with their only client. A set's wire side is
`feed: Option<SetFeed>` — a canvas may open a DETACHED set;
`ensure_folder` attaches the feed.

### Step 5 (later) — Split the chat into model and view

`ChatPanel` fuses the conversation MODEL (turns, actions, the feed
subscription, session link) with VIEW furniture (laid cells, composer
box, toolbar, scroll). Split: `Chats` keeps the transcript model; a
chat-view registry beside it holds per-view state, keyed by view id;
`ChatPane` references the view id. Then two panes can show one
conversation, and feed landings roll transcript + views model-locally
instead of performing through one fused record. Deferred: the cell
build is entangled with the feed apply today; untangling is its own
piece of work.

## Ordering and risks

Order: 1 (LANDED — the dressing sweep) → 3's intermediate (LANDED —
views inside the models, panes, `as_any_mut` gone) → 2's ChangeSets
migration re-keying views by `(ChangeSetId, ViewId)` → 4 (canvases
attach); 5 later. Step 4's one real design risk — the state/face
split across the himark/hidiff seam — is mitigated by the precedent
that already works: `DiffViewState` lives in documents while hidiff
renders it.

Non-goals: palette/peeker and other EPHEMERAL modals keep their
current life — they die on dismiss and have no model to live beside.
The files tree is out of scope here; its per-view state includes watch
subscriptions, which are arguably model state and deserve their own
pass.
