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

### Step 2 — Name the ChangeSets model

`Changes` (working-copy sets per folder) and `History` (commit sets
per folder) are one model wearing two coats — `CanvasSource::
{WorkingCopy, Commit}` already names both shapes, and
`canvas_generation` already dispatches over them. Introduce
`ChangeSets` as the single surface: per-folder working-copy set +
commit sets, each with its generation, fed by the changeset/history
subscriptions. (This can start as a facade over the two structs;
physical unification can lag.)

### Step 3 — Tree views become ChangeSets-adjacent records

`ChangesView` / `HistoryView` (rows, items, `seen`) move off the
Window dock into a view-state registry beside ChangeSets in the
session family, keyed by a minted view id. The dock slot becomes a
thin reference pane. Consequences, all simplifications:

- `sync_changes_docks` / `sync_history_docks` stop walking windows and
  downcasting (`ModalView::as_any_mut` retires — it exists only for
  this); the generation bump rolls sibling view records model-locally.
- Session gather/scatter isolation comes free, as it already does for
  `Canvases` and `Chats`.
- Lifecycle: tree-view records DIE on pane close — their rows are pure
  derivation, rebuilding is cheap. (The chat rule — state survives the
  pane — does not generalize; it holds only where the record IS the
  model.)

### Step 4 — Canvases join the same registry

A canvas is the other view of the same ChangeSets model. Split what
hidiff's `Canvas` holds today:

- **View STATE** — the row list (keys, `DiffViewId`s, heights,
  phases, collapse stash, reveal, refs) is plain data over documents-
  level ids. It moves beside ChangeSets, next to the tree views.
  `himark::diff_canvas` already holds the shared canvas types
  (`CanvasSource`, `CanvasFile`, `canvas_files`) precisely because of
  this layering seam — the state follows them.
- **The FACE** — row rendering (`RowFrame`), the build effects and
  landings, header chrome — stays in hidiff, a renderer over the
  store-held state, exactly as it renders `DiffViewState` today.

The plugin session-family slot (`canvases_session_family`) and the
`CanvasBuildLanded` routing shrink accordingly: the state is reachable
through ChangeSets like any other record, and the sync observer's
reconcile becomes a model-local roll beside the tree views' — one
generation bump, all views of the set roll together.

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

Order: 1 → 3 → 4, with 2 starting as a facade whenever 3 needs a name
to hang registries on; 5 later. Step 1 is independent and pays
immediately (a paint probe dies). Step 3 kills `as_any_mut`. Step 4
has the one real design risk — the state/face split across the
himark/hidiff seam — mitigated by the precedent that already works:
`DiffViewState` lives in documents while hidiff renders it.

Non-goals: palette/peeker and other EPHEMERAL modals keep their
current life — they die on dismiss and have no model to live beside.
The files tree is out of scope here; its per-view state includes watch
subscriptions, which are arguably model state and deserve their own
pass.
