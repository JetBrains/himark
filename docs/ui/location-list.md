# Location lists: streaming search and LSP navigation

Status: **LANDED** (all ten steps, 2026-09-20). The wire truth is
`docs/ahp/ahp-locations.md`; this file is the himark-side design
record. §10's steps read as the change log of the landing.

## 1. What is wrong today

The current shape was copied from Zed's results panel — occurrence
rows as live bounded editors over installed documents — and every
problem below follows from that choice:

- **Eager fetching.** The search panel fetches up to
  `MAX_WORKSPACE_DOCS = 50` never-opened documents just to render the
  panel (`frontend/plugins/search/src/lib.rs:329`), each through
  `FetchDocumentEffect → BuildDocumentEffect → install_document`
  (`frontend/himark/src/location_list.rs`), installing tint markup,
  fragment sets, and one `EditorView` per occurrence. Find-references
  is worse: `CodeNavigationHandler` fetches and builds up to
  `MAX_FETCHED_TARGETS = 50` target documents on the effect thread
  before the user sees anything
  (`frontend/plugins/hicode/src/lib.rs:76-140`).
- **No streaming.** `FindEffect` answers one `Vec<ResourceLocation>`
  at the end (`frontend/himark/src/workspace.rs:273`); the AHP
  `search` method answers one `{hits, truncated}` blob
  (docs/ahp/ahp-search.md §2.1). Nothing renders until the whole walk
  and the whole fetch fan-out finish.
- **No cancellation for references.** `code.references` pushes
  `CodeNavigationEffect` fire-and-forget — the token is discarded
  (`plugins/hicode/src/lib.rs:438`); two rapid invocations both land.
  Search cancellation exists but is oblique: a per-connection token
  server-side, serials plus `relaunch` client-side.
- **The results surface.** Occurrence rows are editors, so the list
  is heavy (fragments, markup, repair traffic per row), information
  density is whatever the editor's line layout gives, context is a
  ±5-line window that cost a full document build, and the keyboard
  model is editor-focus per row rather than one list cursor over
  leaves.

## 2. The design in one paragraph

One shared vocabulary — a **`Location` with its display context baked
in** — travels over one AHP channel scheme, `ahp-locations:/`. Text
search and LSP navigation (references, implementations) become
*producers* of that channel: the request answers a channel URI
immediately, results stream in as concatenation actions, and
**unsubscribing cancels the producer**. Because a `Location` carries
its own context line, the client renders a full results tree without
fetching a single document; documents are fetched only when the user
actually navigates. The UI is a **Search dock tab** (a fourth dock
owner beside `files.tree` / `changes.view` / `history.view`) holding a
query input and a keyboard-first file tree of locations, shared by
text search and find-references; plus a **peek inlay** in the editor
for quick random-access go-to-reference.

## 3. The `ahp-locations` channel

A mini-extension both the Search and LSP extensions depend on. Types
live in `protocol/ahp-ext-types/src/locations.rs` (crate
`himark-ahp-ext-types`), following the `search.rs`/`history.rs`
serde conventions.

```typescript
interface Location {
  /** Resource URI of the document. */
  uri: string;
  /** 0-based line. */
  line: number;
  /**
   * 0-based column, in UTF-8 bytes within the line — the unit
   * `LineCol.col` and the LSP utf-8 negotiation (ahp-lsp.md §4)
   * already pin.
   */
  column: number;
  /** Match length in bytes; may run past the line end (multi-line
   *  matches); clamp for display. (Addition beyond the brief: both
   *  producers know it, and highlight + navigation want it.) */
  length: number;
  /**
   * A slice of the match's line for display. Usually the whole
   * hard line (trailing whitespace trimmed); for huge lines a
   * bounded window around the match (MAX_CONTEXT bytes, utf-8
   * snapped).
   */
  context: string;
  /** The column (same unit) where `context` begins — 0 unless the
   *  line was too long to ship whole. The match renders inside
   *  `context` at `column - contextColumnStart`. */
  contextColumnStart: number;
}

interface LocationList {
  locations: Location[];
  /** The producer finished. Latches true. */
  done: boolean;
  /** The producer stopped early (limit or cancellation) — the
   *  honesty contract, as in SearchResult.truncated. */
  truncated: boolean;
}
```

**State and action are the same type.** The snapshot is a
`LocationList`; the one action, `"locations/extend"` (riding
`StateAction::Unknown` with a `type` field, minted via an
`action_value`-style helper), carries a `LocationList`; the reducer is
**concatenation**: `locations` append, `done`/`truncated` latch by OR.
The final action a producer emits sets `done` (and `truncated` when it
was cut off). No deltas, no replacement, no ordering vocabulary — the
producer's emission order is the list order.

**Channel lifecycle.** Channels are per-request, host-minted
(`"ahp-locations:/<uuid>"`), never idempotent — every request mints a
fresh one. The producer starts at the request, not at subscribe;
results produced before the subscribe arrive in the snapshot (the
standard snapshot-then-actions contract absorbs the race).
**Unsubscribe is the cancel**: when the last subscriber leaves, the
host flips the producer's cancellation token and disposes the channel
entry; a client that never subscribes leaks nothing beyond the entry,
which connection close reaps. A cancelled producer still finishes the
contract: one final `locations/extend` with `done: true, truncated:
true` (delivered to no one if the channel is already gone — that is
fine, the state dies with it).

**Positions on the wire — a deliberate reversal.** ahp-search.md
argues positions would be stale by the time they are used; that
argument shaped a result that forces the client to fetch everything to
show anything. The reversal: locations are a **display snapshot**, not
live coordinates. Rendering needs no document at all; navigation
resolves `line/column` against the live text at click time (`offset_at`
clamps, `frontend/documents/src/change.rs:31`), so a document edited
since the search navigates to the nearest sane position — the same
tolerance navigation history already lives with. Content search reads
stored content only (unflushed document-channel edits are not
observed), exactly as `search` does today; the client MAY run a local
scan over open dirty buffers later (§8, deferred).

### 3.1 Producer: text search

New Search-extension method (the existing `search` stays for the
peeker's quick-open path lane):

```typescript
// searchLocations
interface SearchLocationsParams {
  channel: string;              // the session's channel URI
  folders?: string[];           // default: the session's working dirs
  query: string;
  kind?: "text" | "regex";      // no "fuzzy" — this is content search
  caseSensitive?: boolean;      // default false
  limit?: number;               // total locations; server-capped
}
interface SearchLocationsResult { channel: string; }  // ahp-locations:/…
```

Scope, walk rules, matching semantics, and errors are ahp-search.md
§2.2–§2.4 verbatim (folders under roots, ignore rules, binary skip,
regex dialect). What changes is the answer shape and the engine
contract: `backend/find` (`hifind`) grows a **streaming sink** — an
emit callback invoked per file with that file's `Vec<Location>`
(line/column/length from the matcher, context sliced from the read
buffer) — instead of collecting resource URIs. The host wraps each
per-file batch as one `locations/extend` (per-file granularity is the
natural coalescing; a safety cap splits monster files). Cancellation
is the channel token, checked between files — the per-connection
search token stays for the old `search` method only.

### 3.2 Producer: LSP references and implementations

New LSP-extension method:

```typescript
// lsp/locations
interface LspLocationsParams {
  channel: string;              // the session's channel URI
  /** Whitelisted: "textDocument/references",
   *  "textDocument/implementation". */
  method: string;
  /** LSP params, verbatim (utf-8 positions, ahp-lsp.md §4);
   *  references get their context field per LSP. */
  params: unknown;
}
interface LspLocationsResult { channel: string; }
```

The host forwards the LSP request exactly as `lsp/<method>` would
(routing §2.3, text truth §3 of ahp-lsp.md unchanged), then converts
the verbatim `Location[] | LocationLink[]` answer into our
`Location`s, **deriving context host-side**: for each distinct result
URI it reads the text it already owns — the synchronized document
channel when one exists (so unsaved edits are honored, unlike content
search), the disk file otherwise — and slices the context lines.
Grouped per URI, emitted as one `locations/extend` per file (a single
LSP answer may still stream as several actions), then the `done`
action. Channel cancellation maps to `lsp/$/cancelRequest` toward the
language server through the existing overtake-proof in-flight map.

An LSP error or `NoLanguageServer` after the channel was minted
resolves the channel with `done: true, truncated: true` and an empty
list — the request itself only errors when it fails before minting.

## 4. Client consumption

The seat trait (`frontend/himark/src/higent/seat.rs`) gains
`search_locations`, `lsp_locations`, `subscribe_locations`,
`poll_locations`, `unsubscribe_locations`; the wire implementation
(`frontend/hiahp/src/wire.rs`) follows the changeset/annotations
pump shape — and, unlike `subscribe_history`, feeds `last_seen` (the
bug the docsync comment at `wire.rs:1076` warns about).

Engine-side, the arc copies the **history template**
(`frontend/himark/src/hihistory.rs:311` /
`frontend/hiahp/src/registry.rs:135`): an ask effect answers the
channel URI, a subscribe effect lands the snapshot, a relaunched poll
effect lands action batches, and every landing carries the feed's
generation so stale streams discard on arrival. **Supersession is
unsubscribe**: replacing a query (or closing the surface) launches
`unsubscribe_locations` on the old channel — server-side cancellation
falls out of the lifecycle, no serials on the wire. Effects route by
the asked location's authority (the `fsroute::seat_of` precedent,
`frontend/hiahp/src/find.rs:39`); a multi-folder session asks each
folder's seat and merges the streams client-side under one feed.

Results land **by identity, not tree path**: every result set (a
query, a references ask) is a FEED — `locations::LocationsFeeds`,
keyed by minted `FeedId`, the family-row pattern. A feed owns its
channel and its own app-level pump (`AttachFeedStream` starts it;
the `FeedBatch` landing folds and re-polls through
`AppCommand::Landing`), so it outlives any face: the dock tab and
the peek are FACES that refresh paint-driven off the feed's
generation, `SessionSearchFeeds` names which feed fronts the tab,
and the peek's promote chip fronts its feed in the dock without
asking again. Disposal (`DisposeFeed`) unsubscribes — the host-side
cancel; `StopFeed` keeps what landed.

## 5. The UI: locations tree

New module `frontend/himark/src/locations.rs` (the name
`LocationTree` is taken by hifiles' file tree; this one is
`LocationsTree`). **The full file tree, not a flat file list**: the
shape `TocView::for_locations` already builds today over a result
set's locations (`frontend/himark/src/toc.rs:110` — sorted locations
folded into a `DirTrie`, directories nested, files under their
directory, emitted as `ForestNode`s into a
`SpeedSearchView<ForestList<K>, ForestSearcher<K>>`) — extended one
level down so every location is its own leaf:

- **Directory rows** (branches): nested as in the TOC/hichanges
  trees; `TreeItemView` disclosure, collapse folds the subtree.
- **File rows** (branches under their directory): name plus an
  occurrence-count badge.
- **Occurrence rows** (leaves, children of their file): the `context`
  text as a cheap styled label — **not an editor** — with a trailing
  `line:col` position chip. The match-span tint inside the label
  (`StyleId::Match` colors at `column - contextColumnStart`, for
  `length` bytes) is a follow-up: `imba::Text` is single-run today,
  so the tint needs a run-capable label or a measured wash in
  `TreeLabel` — deliberately deferred behind the working tree.

```rust
enum LocationKey {
    Node(ResourceLocation),   // directory- or document-kind: the row's path
    Hit(ResourceLocation, u32 /*line*/, u32 /*column*/),
}
```

Domain keys, no id minting (unlike the TOC's minted `u64` + targets
side-map — the location is the key, the list-tree.md rule). The forest
is the `himark::Forest` consumer pattern (docs/ui/list-tree.md §7):
`Forest` holds the fold memory; each landed batch rebuilds the trie
and re-`set`s the forest — the hichanges refresh shape, UI-thread but
bounded by the stream's total-location cap; fold state and the cursor
survive relandings by key (`Forest` keeps its collapsed set across
`set`, the view re-selects the cursor key). Moving the trie +
`ListSlice` shaping onto a worker is an open optimization if the cap
ever rises. Search emission order is unspecified (the host's walk is
parallel); the trie build sorts, so the tree is always path-ordered
whatever order batches land in.

Keyboard is the unified list's, configured once: `with_selection`,
cursor on arrows, Enter activates, reveal keeps it on screen;
left/right collapse/expand branches; **full speed-search** from day
one — `ForestSearcher` over the same rows, exactly as the TOC and
hichanges wire it (docs/ui/speedsearch.md): typing filters/highlights
across directories, files, and occurrence contexts alike.
**Activating a leaf performs ordinary location-based navigation**:
`open_by_location_effect(window, location, primary, Some(range))`
(`frontend/himark/src/workspace.rs:162`) with the target range built
from `line/column(+length)` — through the one navigation door, history
recorded, `offset_at` clamping staleness. Activating a file row opens
the file at its first occurrence. No document is fetched or subscribed
before that moment.

## 6. The UI: Search dock tab

A fourth dock owner, id `"search.view"`, following the
`ToggleSessionTree` recipe exactly (`frontend/hifiles.rs:877-919`):
a `DynamicCommand` toggle + toolbar button; content is a `ModalView`
over the session's search feed row (D7: the dock rides the workbench
across session switches; the view is a face, the feed row is the
substance — reopening the tab shows the standing results).

Layout, top to bottom:

- **Query input** — the `EditorView::input` recipe the current panel
  uses, with a trailing **cancel/stop affordance** shown while the
  feed is not `done` (click = unsubscribe, partial results stay,
  `truncated` note shows). Typing requeries: unsubscribe the old
  channel, `searchLocations` the new one, the tree drains and
  restreams. The `MIN_QUERY = 2` guard and the too-short → clear
  behavior carry over from `plugins/search`.
- **The locations tree** (§5).
- A **status band**: running spinner / `N results in M files` /
  truncation note.

Focus areas mirror `SearchArea::{Input, Results}`: Tab and Down
from the input enter the tree; a CLICK moves the keyboard to the
clicked area before landing (the face's widget shell routes it);
typing in the tree is speed-search; Escape rolls the dock away.
**Selection IS navigation**: the keyboard cursor landing on a file
or hit row opens it in the pane through the same door a click uses
(`navigate_selection` → `pick`), deduped by the last-navigated key
so feed rebuilds re-asserting the cursor never re-open. The door
carries a **focus preference** (`OpenFoundLocation.focus`, threaded
to `Window::show_document`): only **Enter** is the deliberate jump
that moves the keyboard to the editor (`LayerFocus::Content`);
selection moves AND clicks just show the location — the keyboard
stays with the tree, so navigating on continues. The
feed keeps streaming past a closed face — the pump is app-level; a
requery or the stop affordance cancels (`DisposeFeed`/`StopFeed`),
never mere displacement. cmd-shift-F re-points here (the old panel
is retired): it OPENS the dock tab, and re-invoked on an open one
it FOCUSES the query input (`Window::focus_dock` + a
`SearchCommand::Focus(Input)` through the dock's dyn road) — never
a toggle-away; Escape closes.

## 7. The UI: references and implementations

`code.references` reworked, `code.implementations` added (same shape,
`textDocument/implementation`): the ask mints a feed and fronts it in
the dock AT ASK TIME — the tab opens saying "searching…" before the
channel lands, so a failing ask resolves cut-off in plain sight,
never a silent no-op. The steps:

1. Build the LSP params from the caret (the existing
   `line_col_at`/`identifier_at` code in `plugins/hicode`), title
   `` References to `foo` ``.
2. Ask `lsp_locations` → channel URI.
3. Install the stream into the **session's search feed row** —
   displacing whatever query/results the Search tab held (the old
   channel unsubscribes; its input clears to a title band naming the
   request) — and activate the dock tab (`show_dock`, the hifiles
   recipe; swap-in-place when the dock is already up).

This deliberately reuses one surface for one workflow — processing
many occurrences linearly with the list cursor. Master–detail (several
result sets held side by side) is future work; the displacement rule
makes room for it without blocking on it: a feed row per result set is
already the shape, only the tab's face would grow a selector.

A supersession rule replaces today's nothing: the feed row holds one
live channel; any new request — search keystroke or references ask —
unsubscribes the previous one. Two rapid `code.references` no longer
both land.

## 8. The UI: go-to reference peek (inlay)

The quick random-access interface, per VSCode/Fleet: a command (id
`code.go-to-reference`) that opens an **inlay under the caret's line**
— `InlayMode::Under` (`frontend/editor/src/markup.rs:661`), a widget
in a feature markup on the document, shifting with edits — containing
a master–detail pair:

- **Master (left)**: a tree of locations GROUPED BY FILE (no
  directory nesting — the card is compact) over the card's own feed,
  streaming in as the channel answers; cursor row synced to the
  detail. The header carries the feed's title, the stream state, and
  the promote chip — "open in Search" fronts the SAME feed in the
  dock tab, results moving without a re-ask.
- **Detail (right)**: a preview `EditorView` over the selected
  location's document — resolved through the peeker's exact recipe
  (docs/ui/peeker.md, docs/ui/navigation.md): an open document
  previews through the registry; a released one fetches once, builds
  a temp document, registers at display, **adopts on pick**. Only the
  selected location's document is ever fetched.

Interaction: up/down move the master cursor (detail follows), Enter
navigates to the selection and dismisses, Escape dismisses; both
dismissal paths remove the inlay markup, unsubscribe the feed's
channel, and reap unadopted temp documents. Key routing while the
editor keeps focus follows the completion popup's interception
precedent (`frontend/editor/src/popup.rs`). Height is capped (a
themed row budget); the master scrolls within it. **Exactly one
result with `done` set navigates directly** — no flash of UI for the
trivial case. `code.definition` keeps its current direct-navigation
path; multi-target definitions may later reroute here.

## 9. What retires, what stays

Retired once §6–§8 reach parity:

- `frontend/plugins/search` — the modal overlay, the pinned panel,
  the `SearchEffect` open-document scan lane, the prebuild pipeline,
  and its `FindEffect` text lane.
- The eager `himark::LocationList` machinery
  (`frontend/himark/src/location_list.rs`): install/fetch/lift_budget,
  `ListPanel`, `FamilyRow::List` and its peeker/family-row plumbing.
- `CodeNavigationEffect`'s fetch-everything handler in
  `plugins/hicode` / `frontend-host/src/lsproute.rs`'s references
  route (definition's stays).
- `FindTarget::Text` (the path lane stays for the peeker).

Stays untouched: the in-document find bar (cmd-F,
docs/ahp/search.md §5 — a different feature on the pane slot), the
peeker's path find (`search` with `fuzzy`/`path`), the old `search`
method itself, go-to-definition's direct jump.

Two accepted losses, stated plainly: occurrence rows are no longer
editable in place (the bounded-editor trick was the Zed shape we are
leaving; navigation is one Enter away), and match washes no longer
appear in main editors from the results list (the find bar still
tints the focused document; a follow-up could tint from the feed on
navigation).

Deferred, deliberately: a live-buffer lane (scanning open dirty
documents client-side into the same feed shape — the feed's merge
semantics already admit a second producer); master–detail in the dock
tab; persisting feeds across restarts.

## 10. Steps

Each step lands independently; the axis is protocol → client → UI →
retirement.

1. **Vocabulary.** `protocol/ahp-ext-types/src/locations.rs`:
   `Location`, `LocationList`, `LOCATIONS_EXTEND`, the action helper,
   serde round-trip tests. Write `docs/ahp/ahp-locations.md`
   (normative: types, reducer, lifecycle, cancellation contract) and
   add the scheme to the lists in `docs/ahp/ahp-ext.md` (extension
   mechanics) and `docs/ahp/agent-host.md` §3.
2. **Host channel plumbing.** `backend/agent-host/src/server.rs`:
   `LOCATIONS_PREFIX`, a `State` map of channel entries `{session,
   state: LocationList, cancel: Arc<AtomicBool>}`; wire the prefix
   into all four routing points — `subscribe` (~`:1201`), the
   reconnect ladder (~`:1166`), the reconnect known-channel predicate
   (~`:1125`), and last-subscriber disposal in `unsubscribe`
   (~`:1234`, flip the token, drop the entry). Fix `close_connection`
   (~`:885`) to run the same disposal for channels the dying
   connection was the last subscriber of — today only the search token
   is honored there. A generic `emit_locations(channel, batch, done,
   truncated)` that folds into the entry and fans out the envelope
   (the `ls_event` hand-rolled shape). Socketpair test with a scripted
   producer: snapshot-covers-the-race, concat reduction, unsubscribe
   flips the token, connection death flips it too.
3. **Streaming search producer.** `backend/find`: sink-based scan
   (per-file `Vec<Location>` emit with line/column/length/context,
   `MAX_CONTEXT` window with `contextColumnStart`, utf-8 snapping;
   walk rules unchanged). `searchLocations` in the host: mint, spawn
   the scan on the blocking pool with the channel token, emit per
   file, final done/truncated action, total-location cap. Tests
   against the seeded tree: streaming order, huge-line windowing,
   cancellation mid-walk, limit truncation.
4. **LSP locations producer.** `lsp/locations` in the host: method
   whitelist, forward via the pooled server, in-flight map entry so
   channel-cancel reaches `$/cancelRequest`, context derivation from
   the synchronized mirror else disk, per-URI batching, error →
   resolve-truncated. Extend the scripted-fake-server tests:
   references round-trip with contexts, cancel-on-unsubscribe reaches
   the fake, implementation method passes the whitelist.
5. **Client seat + effects.** Seat methods and wire pump
   (attach-before-poll, `last_seen` fed); himark effects on the
   history template (`AskLocations{Search,Lsp}Effect` →
   `SubscribeLocationsEffect` → poll relaunch loop →
   `UnsubscribeLocationsEffect`), authority-routed via `fsroute`,
   generation-gated landings; the `LocationsFeed` store row.
   Registry entries in `frontend/hiahp/src/registry.rs`; hermetic
   tests through a fake seat: concat folding, supersession
   unsubscribes, stale-generation discard.
6. **The locations tree.** `frontend/himark/src/locations.rs`:
   `LocationsTree` — the full dirs → files → occurrence-leaves tree,
   generalizing `TocView::for_locations`'s `DirTrie` build
   (`toc.rs:110`) onto `LocationKey` domain keys; `Forest` fold
   memory with off-thread trie + `ListSlice` rebuilds per landed
   batch, context labels with match tint, count badges; full
   speed-search (`ForestSearcher`, the TOC/hichanges wiring);
   selection/cursor/Enter → `open_by_location_effect` with the range
   target, expand/collapse. Widget tests: rebuild under out-of-order
   batches preserves fold + selection by key, speed-search matches
   contexts, Enter navigates through the door (history recorded),
   zero fetches before activation.
7. **Search dock tab.** `ToggleSearchView` (`"search.view"`) +
   toolbar button (gated on the seat serving `searchLocations`);
   the ModalView face over the session feed: input + cancel
   affordance + tree + status band; requery = unsubscribe + fresh
   ask; focus areas and Escape; shift-cmd-F and the `%` well
   re-pointed. Dock tests extend the `editor_tests.rs` dock suite:
   toggle, session-switch survival (D7), results survive
   close/reopen, cancel stops the stream.
8. **References into the dock.** Rework `code.references`, add
   `code.implementations`; stream into the search feed row with the
   displacement + supersession rule; title band; activate/swap the
   dock. Test: rapid double-ask leaves exactly one live channel.
9. **Go-to peek inlay.** `code.go-to-reference`: Under-inlay
   master–detail over its own feed, popup-precedent key routing,
   lazy preview with register-at-display/adopt-on-pick, single-hit
   direct navigation, dismissal unsubscribes and reaps. Widget tests
   mirror the completion popup's.
10. **Retirement.** Delete the §9 list, migrate the peeker/family-row
    seams off `FamilyRow::List`, sweep docs (docs/ahp/search.md
    rewrites around the find bar; docs/ui/list-view.md's search
    references; this file loses its PLAN banner and becomes the
    design record).

Verification per step: the owning crate's tests plus
`scripts/build-web.sh --check` (the bare cargo pipe false-greens on
the web target).
