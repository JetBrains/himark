# Perf issue: skia-sized status diff — 30 fps scrolling, multi-second UI freezes

> **Status:** measures 1–5 below are implemented on branch `perf-sweeps`.
> The sync lanes drain per-lane dirty queues written at the entry-write
> doors (`OpenDocuments::note_write`); `sync_diff_lanes` captures behind
> the staleness gate; changeset snapshots/polls are digested in the
> effect landing map on the worker (`digest_state`/`digest_actions`,
> which also drops superseded snapshots); poll loops are serial-guarded
> (`ChangeSets::polls`). Measure 6 (memory reclamation) is NOT done.

Diagnosed 2026-10-02 from an Instruments CPU Profiler trace (`skia-diff-scrolling.trace`, 56 s,
himark-macOS) and three 1 ms `sample` captures (`random-freeze.txt`, `another-random-freeze.txt`,
`changes-view-scroll-freeze.txt`) of the same process. Reproducer: the entire vendored skia tree
landed in the status diff (thousands of files). **Diagnosis only — nothing is fixed yet.**

Two distinct mechanisms, both linear UI-thread work in violation of the O(log n) convention:

1. Scrolling at 30 fps: the `perform_batch` tail sync lanes sweep **all** diffs / registered
   documents / diff views on every scroll tick.
2. Multi-second freezes (sessions drawer, changes view): `ChangeSets` digests full changeset
   snapshots on the UI thread at O(actions × files) with deep JSON clones per file.

The search/location-list rework is **not** implicated — no ahp-locations symbols are hot anywhere
in the trace.

## 1. Scrolling: batch-tail sync lanes are O(everything) per scroll tick

Trace profile: the main thread holds 94.5% of all sampled cycles. Actual paint
(`dispatch_paint`/`draw_with_size`) is only 6.9% of main-thread time; **80.9% is the batch-tail
sync lanes** at `frontend/himark/src/app.rs:962-975`, which run on every `perform_batch` — i.e.
once per scroll-wheel event (71.7% of main-thread time arrives via `HimarkView.scrollWheel`),
again per mouse-move (11.4%), again per display-link tick (10.7%).

### 1a. `sync_diff_lanes` — 45.3% of main-thread time

`frontend/documents/src/diffs.rs:515` iterates **every tracked diff** per batch. Worse, it does
the expensive capture *before* the staleness gate:

- the record clone, 3-4 rpds `entries.get()` lookups, base log + text clones, and
  `syntax_snapshot(&base_entity.document)` (`diffs.rs:539`) all happen first;
- `syntax_snapshot` (`diffs.rs:914`) calls `clone_tree()` → tree-sitter `ts_tree_copy` — a full
  tree copy of a C++ file **per diff, per scroll event** — which is immediately freed
  (`ts_tree_delete`, ~5.5%) when the `record.normalized == Some(now)` check short-circuits.

The pre-gate tree copy alone is ~17% of main-thread time (`ts_tree_copy` ~6.5% plus the clone
plumbing). rpds `HashTrieMap::get` under this lane is another ~13%. This capture-before-gate
shape came in with the diff-normalize stale-tree catch-up work.

### 1b. `sync_diff_dressing` — 18.8%

`frontend/himark/src/diffs.rs:110` loops all diff views; per view a `diff_view_ref` + two
`document_ref` rpds gets + `state.stale()`. Its own doc comment (`diffs.rs:109`) says
"O(views) stale checks on refs" — acceptable for a handful of panes, not for thousands of
diff-canvas rows, per scroll tick.

### 1c. `sync_scroll_stripe_lanes` — 16.8%

`frontend/documents/src/scroll_stripes.rs:34` scans **all registered documents**
(`entries.iter().filter(|(_, e)| e.document.wants_scroll_stripes())`, line 45), and
`wants_scroll_stripes` (`frontend/editor/src/document.rs:3167`) itself makes three passes over
the document's editors. The module header promises "O(1) when nothing is flagged" — the flag was
never implemented; it scans everything, every batch.

### The sweep tax is view-independent — tree scrolling included

The lanes run at the tail of `perform_batch`, and *every* input event routes there:
`scroll_phased_at_time` (`frontend/frontend-host/src/lib.rs:934`) dispatches the scroll into the
engine, and `dispatch`/`dispatch_timed` (`frontend/himark/src/app.rs:802`, `:819`) feed the
resulting commands into `perform_batch` (`app.rs:897-914`) regardless of which surface is under
the pointer. So scrolling the file tree — or any other pane — pays the same O(all diffs ×
tree-copy + all registered documents + all diff views) toll per tick, even though the tree
touches none of that state. Once the registries are bloated by a skia-sized diff, the whole app
scrolls at 30 fps, not just the diff canvas. (No tree-specific hot symbols appear in the trace;
the tree is a victim of the sweeps, not a second offender. The dirty-set fix below restores it
along with everything else.)

### Why n is enormous

Dressed diff-canvas rows register base+target as persistent documents and deliberately do not die
with the canvas (`frontend/himark/src/diff_canvas/canvas.rs:914`: "the rows no longer die with
the canvas now that they ARE registered documents"). Scrolling through the skia diff monotonically
accumulates thousands of entries in `OpenDocuments.entries`, `Diffs.records`, and
`Diffs.diff_views` — and every one of them gets swept by all three lanes on every scroll tick.

## 2. Freezes: full-snapshot digestion on the UI thread in `ChangeSets`

All three `sample` captures show the identical picture: ~100% of main-thread samples for the full
2 s window inside one `himark_drain → Application::perform_batch → Store::route →
ChangeSets::perform` drain, every other thread parked. Not a blocking wait, not lock inversion —
pure computation.

Mechanism, all in `frontend/himark/src/hichanges.rs`:

- `adopt` (`hichanges.rs:1199`) rebuilds **every** entry via `entry_of` for each full snapshot,
  then re-stamps all entries against the previous list (`same_entry`, `hichanges.rs:154`).
- `fold` (`hichanges.rs:1243`) deep-clones the whole previous entry list, then for **every**
  `ChangesetContentChanged` action in the polled batch (`hichanges.rs:1260`) rebuilds the entire
  file list from scratch — even though each subsequent snapshot wholesale overwrites the last.
  A batch of A snapshot actions over F files costs O(A × F) `entry_of` calls.
- `entry_of` (`hichanges.rs:102`) deep-clones the full `serde_json::Value` trees for `before`,
  `after`, *and* `diff` of every file (`value.clone()` then `from_value`) just to read a `uri`
  string. With `preserve_order` serde_json this is the `IndexMap::clone` dominating the samples
  (~880/2014 in capture 1), plus ~300 samples re-parsing URIs in `FileUris::location_of`
  (`frontend/hiahp/src/uris.rs:14`), plus hundreds more just *dropping* the cloned trees.
- `perform_batch` (`frontend/himark/src/app.rs:920`) drains the whole command queue before
  yielding, so the backlog executes as one uninterruptible main-thread block — no frame for the
  drawer animation.
- **Self-amplifying:** `relaunch_poll` (`hichanges.rs:525`) re-arms the next poll immediately, so
  while the UI chews batch N the host accumulates a bigger batch N+1 of full snapshots. Hence the
  identical repeat freeze minutes later.

Trigger paths: opening the sessions drawer / entering a session row (`OpenSessionRow` →
`EnterSessionWork`, `frontend/himark/src/higent/session/open.rs:157`) calls `Changes::ensure`,
which lands a `Snapshot` carrying the full `ChangesetState` — every file with its JSON edit
payloads — digested synchronously. The changes-view scroll freeze is the same `fold` path hit by
a polled landing.

## 3. Memory growth (8.9 GB → 16.2 GB over ~6 minutes)

Consistent with both mechanisms:

- Every dressed row permanently retains two registered documents — full text rope, parsed
  tree-sitter tree (large for C++), editors, markup (`diff_canvas/canvas.rs:914` persistence by
  design; teardown only on explicit row removal, never on scroll-off).
- `EditLog` (`frontend/editor/src/edit_log.rs:20-71`) is strictly append-only — never truncated.
- Backlogged snapshot batches (full per-file JSON) queue undrained while the main thread is
  pinned, and each fold re-clones the entire entry list.
- Per-scroll-tick `ts_tree_copy` + malloc/free churn (7-8% of process time each in the allocator)
  inflates allocator footprint on top.

## 4. Fix directions (agreed, not implemented)

Highest value first:

1. **Dirty-set gating for all three sync lanes.** Maintain sets of diffs / documents / views
   touched in the current batch (written at mutation sites, keyed off `by_target`/`by_base` for
   diffs) and sweep only those — O(changed) instead of O(all ever). For scroll stripes this just
   delivers on the module header's existing "O(1) when nothing is flagged" promise.
2. **Gate before capture in `sync_diff_lanes`.** Read the cheap staleness inputs first; never
   clone a record, log, text, or syntax tree for a diff that is already normalized.
3. **Coalesce snapshots in `fold`.** Only the last `ChangesetContentChanged` per batch matters
   (modulo interleaved `FileSet`/`FileRemoved` ordering) — skip rebuilding for superseded ones.
4. **Move changeset digestion off the UI thread.** Do the `ChangesetFile → ChangeEntry`
   conversion (serde parsing, `location_of`, stamping) in the effect/background and land a
   finished entry list to swap in — the same pattern as the no-diff-on-UI-thread rule. Also stop
   cloning `Value`s in `entry_of` (deserialize by reference or use typed wire structs).
5. **Bound the poll feedback loop.** One pending `Polled` per folder, newest wins — also stops
   the backlog-driven memory growth.
6. **Memory reclamation.** Retire far-off-screen rows' syntax trees (or whole registered
   documents for diff-only rows), and truncate `EditLog` below the oldest revision any diff still
   rebases from.
