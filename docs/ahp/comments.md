# Comments over AHP, and the comments view

Two features, one arc:

1. **Comments ride the wire.** A comment card is an AHP
   **annotation** on the current session — persistent across
   restarts (hihost logs them), visible to every client of the
   session, and writable by the agent (an agent can leave a comment;
   a comment is addressed feedback the agent can read).
2. **The comments view.** A **dock panel** — the right-hand split
   (`himark/src/dock.rs`), in the Files/Changes content-swap
   family — showing every commented file as a compacted tree,
   comments nested under their files, click navigates to the
   anchor.

Annotations are **core protocol** (AHP 0.4.0), not one of our
extensions — the whole vocabulary is in `ahp-types`:
`Annotation`, `AnnotationEntry`, `AnnotationsState`,
`SnapshotState::Annotations`, the five `annotations/*` actions, and
the official reducer `ahp::reducers::apply_action_to_annotations`.
No `protocol/ahp-ext-types` additions, no raw-JSON snapshot escape
hatch, no capability advertisement. We implement both halves: hihost
serves the channel, the frontend consumes it — and against a foreign
host (the VS Code seat) the same client code just works, or degrades
to local-only if the host answers subscribe with an error (the
`search` absence-tolerance rule, `docs/ahp/ahp-ext.md`).

## 1. The wire model, and our mapping

One channel per session: `ahp-session:/<uuid>/annotations`.
Snapshot is `AnnotationsState { annotations: Vec<Annotation> }`;
updates are five actions, **all client-dispatchable** through the
write-ahead reducer — the client assigns ids, dispatches, and the
echo comes home through the ordinary poll. The server MAY originate
them too (the agent commenting).

```
Annotation      { id, turnId, resource, range?, resolved, entries }
AnnotationEntry { id, text: string | { markdown }, _meta? }
```

An annotation MUST have at least one entry; removing the last entry
is `annotations/removed`, never an empty shell.

Mapping onto what we have:

| AHP | himark |
|---|---|
| `Annotation` | one comment card (`CommentView` inlay) |
| `AnnotationEntry` | one body block in the card; our own entry is the card's markdown editor |
| `Annotation.resource` | the host document's `ResourceLocation`, crossed through `ResourceUris` (the one visible mapping) |
| `Annotation.range` | the inlay interval, projected to `TextRange` |
| `Annotation.resolved` | card affordance (check next to the cross) + dim chrome |
| `Annotation.turnId` | advisory "as of" stamp — §3 |
| `AnnotationEntry._meta` | `{ "himark": { "author": "user" \| "agent" } }` — the protocol has no author field |

`TextRange` positions: the base protocol leaves the `character` unit
unspecified; we write and read **UTF-8 code units of the line's
content**, the unit `documents@1` pinned
(`docs/ahp/ahp-documents.md` §2.4) and the unit `himark::line_col_at`
speaks. One text model everywhere.

## 2. Identity: the `Comments` singleton

A live inlay's `(DocumentId, InlayKey)` is gone at restart and
meaningless off-process. The wire needs client-assigned stable ids,
and the drawer needs to list comments on files that are **not
open**. Neither can come from markup enumeration. So the feature has
a store component, in `hicomments::sync`, on the
`hichanges::Changes` model (subscription state that outlives any
view). It is PER SESSION (docs/ui/app-state.md §3): each session's row
holds its own `Comments` instance — the install capability
(`CommentsInstall`) stays a global flag, and the annotations channel
is singular per session (`Comments.channel`, the uri inside as the
landings' staleness guard):

```rust
Comments {
    // every annotation we know, wire-fed and locally born
    records: HashTrieMapSync<AnnotationId, CommentRecord>,
    // live cards: which record is materialized where
    cards: HashTrieMapSync<AnnotationId, (DocumentId, InlayKey)>,
    // per-session channel bookkeeping (subscribed, poll parked)
    channels: HashTrieMapSync<Uri, ChannelRecord>,
    generation: u64,          // bumped on every landing; drawers reconcile on it
}

CommentRecord {
    session: Uri,             // owning session (channel = session + "/annotations")
    resource: ResourceLocation,
    range: Option<Range<LineCol>>,   // wire-truth range (turn-pinned)
    turn_id: String,
    resolved: bool,
    entries: Vec<EntryRecord>,       // id, text (a rope), ours: bool
    synced: bool,             // false while local-only (no session yet, or dispatch deferred)
    sending: bool,            // a send to the agent is in flight (§8)
}
```

Entry bodies are ROPES end to end — `Text`, never `String`; change
detection rides revisions and stamps, never a text compare
(docs/code-style.md).

The singleton is the **source of truth for the drawer**; the live
inlay is the source of truth for **position** while the document is
open. `CommentView` carries one field for the join, its
`AnnotationId`, which is also the card→record direction: every door
that starts from a card (edit, cross, resolve) reads the id off the
view it already holds, so no reverse `(DocumentId, InlayKey) →
AnnotationId` map exists. `cards` covers the other direction
(record → card, for inbound folds). The add/remove/edit doors report
to `Comments` (below). `hicomments` deps `higent`, the `hichanges`
precedent: a feature plugin consuming agent data purely through
`higent`'s exported seat + effects.

Ids are minted client-side (`hiahp::uuid_v4`) — annotation ids and
entry ids both, per the spec.

## 3. Turn anchoring

`turnId` is the annotation's version pin: `resource` + `range` are
addressed against the file as that turn left it. himark's authority
is the **live inlay** — it tracks edits through the interval
machinery locally (and documents@1 across clients). The wire
annotation is a projection of that — written at real interactions,
never chased keystroke by keystroke. Under
that reading `turnId` does no anchoring work for us; it is an
honest "as of" stamp for consumers that don't have our buffer.
Rules:

- **Stamp = the latest completed turn** of the owning session at
  dispatch time, across its chats. `higent` maintains this
  (`Agents` learns it from the chat folds it already performs);
  `Comments` reads it at dispatch. Never an active turn — its file
  output is still moving.
- **Zero turns** (fresh session): against hihost, dispatch anyway
  with `turnId: ""` — hihost accepts any turn stamp verbatim
  (§5, it validates nothing it cannot resolve). Against a foreign
  host, hold the record `synced: false` and flush when the first
  turn completes.
- **Drift**: the live inlay IS the position, correct by construction
  (intervals ride the edit machinery); the record mirrors it locally
  so the panel and a later close keep the last-known place. It is
  NEVER continuously re-projected onto the wire: user edits are not
  turns, so a per-keystroke range write would stamp coordinates no
  turn produced — and chatter the channel besides. The wire range
  crosses at `annotations/set` (creation, deferred flush) and stays
  as the last real interaction wrote it; a late opener resolves it
  best-effort against current content (§3 inbound rule).
- **No live session at all**: comments work fully locally
  (`synced: false`); when the session's channel comes live
  (`higent::Agents` binding), `Comments` flushes every unsynced
  record as `annotations/set`.

Inbound, the mirror problem — placing a turn-pinned range into the
current buffer: the range resolves against **current content**,
clamped to document bounds. Diff-forward remapping through the
turn's changeset is deliberately absent (§9); in practice the agent
comments on files as of its latest turn, which *is* current content.

## 4. Client plumbing

The house pattern for late channel families, verbatim from
changesets/documents (`higent::AhpServer`, `docs/ahp/agents.md`):

- The seat trait has an annotations half:
  `subscribe_annotations(session) -> SeatFuture<AnnotationsState>`,
  `poll_annotations(session) -> SeatFuture<Vec<StateAction>>`,
  `dispatch_annotations(&Uri, StateAction)` (fire-and-forget, eager
  spawn), `unsubscribe_annotations(&Uri)` (fire-and-forget). The
  fake in `higent` mirrors it, for tests.
- Wire (`hiahp::wire`): channel uri = session uri + `/annotations`;
  `subscribe_pumped` + the generic `poll_channel` do all the work —
  the actions are **typed** (`StateAction::Annotations*`), no
  `Unknown` filtering, and the snapshot is typed
  (`SnapshotState::Annotations`, disambiguated in the untagged union
  by its required `annotations` key). This family is strictly less
  wire code than documents was.
- Effects (`higent::effects` + stateless handlers in
  `hiahp::registry`, registered in `frontend-host`):
  `SubscribeAnnotationsEffect`, `PollAnnotationsEffect`. Dispatch
  and unsubscribe ride the seat directly (fire-and-forget needs no
  landing), the changeset precedent.

**Subscription lifecycle** — `Comments::ensure(store, session)`,
called from the same place `hichanges` ensures its subscriptions
(session opened / channel live): subscribe → fold snapshot →
park poll → fold actions → repark. Folding is the five-case match
(set / updated / removed / entrySet / entryRemoved) applied to
`records` — the reducer is trivial and `higent` only deps
`ahp-types`, so we write the five arms rather than pull the `ahp`
crate into the frontend. Every landing bumps `generation`.

**Echo discipline**: our own dispatches come back through the poll.
All five actions are upserts/removals keyed by client-assigned ids —
folding an echo is **idempotent**, so no origin tracking, no
conflation keys, no write-ahead ledger (the documents lesson does
not apply: no positional rebase is needed because the singleton
stores the same value the echo carries).

**Foreign edits to our entries**: a foreign `entrySet` targeting an
entry the user authored is recorded in the singleton but NEVER
rewrites the live editor — there is no focus-tracking conflict
guard; the record is the wire truth, the open card keeps the user's
text. Everything else is last-writer-wins, applied immediately.

## 5. Card materialization and the doors

Cards exist only for open documents; records exist for everything.
Cards follow documents through THE `DocumentHook` — a one-slot
`OpenDocuments::install_hook` seam: open → materialize, close →
capture + prune. No polling anywhere.

- **Document opens** (registration-follows-display, the hook's open
  arm): `Comments` installs a card for every record on that
  resource — `ensure_document_markup` + `push_inlay` +
  `swap_inlay`, the existing recipe, range resolved per §3. The
  install is a landing (`AppRequests`), not a layout-time store
  write.
- **`comments.add`** (selection → card, focus lands in it): mints
  ids, registers the record, dispatches `annotations/set` with the
  user's entry, `_meta.author: "user"`.
- **Body edit**: debounced-on-idle `annotations/entrySet` with the
  card's markdown text. The card's inner document is an anonymous
  markdown value; the text crosses as `{ markdown: ... }`.
- **Cross** (`comments.remove`): also `annotations/removed` and
  record removal.
- **Resolve**: check affordance next to the cross →
  `annotations/updated { resolved }`; resolved cards render dim
  (chrome variant), records dim in the drawer.
- **Foreign entries** (agent replies, other clients): the card is a
  **stack of entry blocks** — ours editable (the inner editor),
  foreign ones read-only markdown blocks below, in entry order.
  Foreign entries render unparsed markdown — cold roots reparse only
  on edit. A compose-reply affordance is deliberately absent (§9).
- **Inbound arrival, live**: a fold that adds a record whose
  resource is currently open installs the card **immediately**
  (same `AppRequests` landing as the document-open path) — a
  comment left by another client, or by the agent, appears on
  screen without any local action. Poll landings are the trigger;
  there is no repaint-time store write.
- **Inbound removal/update** on an open document: remove/update the
  card through the existing `RemoveComment` /
  `AppCommand::Entity` roads — never a direct store mutation from a
  fold.

A comment whose whole span is deleted locally collapses to a
zero-length interval and stops rendering (existing `drop_empty:
false` behavior). That is a **local display fact**, not a removal:
the record and the wire annotation survive; the drawer still lists
it and navigation still works (clamped). Deleting the comment is
only ever the cross.

## 6. hihost: serving the channel

The seams, in `backend/agent-host/src/server.rs` unless noted:

- **State**: `SessionEntry` holds `annotations: AnnotationsState`
  (session-scoped, like `documents`). Channel uri = session uri +
  `/annotations`.
- **Subscribe**: an arm in the `subscribe_channel` if-else
  chain answering `SnapshotState::Annotations` — the first channel
  family since chat that needs **no raw-JSON escape hatch**.
- **Dispatch**: a prefix/suffix route in `dispatch()` (before the
  catch-all, which would otherwise silently fold `annotations/*`
  against nothing): validate the non-empty invariant (reject
  `entryRemoved` of the last entry; reject `set` with zero
  entries), then `apply()` through
  `ahp::reducers::apply_action_to_annotations` → envelope → fan-out
  → replay ring. `turnId` is stored verbatim, **never validated**
  — hihost is lenient by design (§3); a client that can name a real
  turn should, one that can't still gets a durable comment.
- **Origination**: the agent halves (tool results proposing
  comments) are out of scope here; the door is `apply()` with a
  server-built action, `_meta.author: "agent"` — the mechanism is
  the same fold.
- **Persistence** (`store.rs`): annotations ride the session
  manifest — `session.json` carries an `annotations:
  AnnotationsState` field, rewritten with the manifest on every
  fold (the local-fs session included). Their lifetime is exactly
  the session's, so the manifest is the right home: no separate
  log, no restart replay (the state is a plain value, not an
  operation history — the chat-log replay-through-reducers rule
  buys nothing here), and `remove_session`'s `remove_dir_all`
  deletes them with the session.
- **Summary**: `SessionSummary.annotations`
  (`AnnotationsSummary { resource, annotationCount, entryCount }`)
  maintained on every fold; published via the existing
  `SessionChanged` partial road — `merge_summary` client-side
  copies it, so the agents drawer badges sessions for free.
- **Reconnect**: the annotations channels sit in the `known`
  if-chain, so re-arm + replay covers them.
- **Lifecycle**: annotations are **persistent session state**, not
  compute-on-demand — last-unsubscriber cleanup (the
  changeset/document rule) does NOT drop them; they die with
  `disposeSession` (`remove_session` deletes the session dir).

Proven by the host battery in `tests/host.rs` (snapshot, the five
folds, invariant rejections, restart survival, reconnect replay,
summary counts), the card battery in `hicomments`, and a
two-wire-client annotations e2e over the real socket (a comment set
on one wire client, seen by the other).

## 7. The comments view

The `hichanges` **dock** recipe, whole — the panel lives in the
right-hand split (`dock.rs`), not the floating drawer: the workbench
narrows, editors re-lay, and the dock's owner-keyed content swap
means Comments/Files/Changes share the one slot:

- **Toggle**: `DynamicCommand` id `comments.view`, the dock toggle
  body verbatim from `ToggleChangesView`: `dock_owner() ==
  Some(id)` → `roll_away_dock()` (settled slide files the Close);
  else ensure subscriptions, build the panel under
  `himark::dock_scope(window)`, `show_dock(store, panel, owner,
  fx)` (which swaps in place when another view owns the dock —
  width and reveal stand). Toolbar button + keymap entry,
  cmd-shift-C.
- **Composition**: `SpeedSearchView<ForestList<ResourceLocation>,
  ForestSearcher<ResourceLocation>>` + a `RowItem` side map, exactly
  the `ChangesView` shape.
- **Tree**: `DirTrie` compaction (single-child, file-less chains
  become one `a/b/c` band, `pick: false, dim: true`); files as
  pickable branches; **comment rows as pickable leaves under their
  file**, keyed
  `location.child(ResourceType::new(COMMENT_KIND), annotation_id)`
  — real domain keys, the `hichanges` note-row trick made pickable.
  Comment row label = first line of the first entry, prefixed with
  an entry-count suffix when > 1 (`"fix the boundary check  ·3"`),
  dim when resolved. Single-line `TreeLabel` rows — richer
  multi-line preview rows mean leaving `Forest` for a hand-built
  `ListSlice` (the `AgentsPanel` pattern) and are deliberately
  absent.
- **Data**: the `Comments` singleton — every record of the session,
  its files open or not. Refresh is the paint-reconcile discipline:
  the panel remembers `seen: u64`, a `ReconcileShell` compares
  against `Comments::generation(store)` and emits `Refresh` while
  stale — no push wiring, subscriptions outlive the panel.
- **Click roads**: file row → toggle fold (branch behavior);
  comment row → `ModalRequest::Perform(AppCommand::Dynamic(window,
  NavigateToComment))` — an app-root landing; the dock's drain
  passes it through WITHOUT dismissing (unlike the drawer:
  navigating keeps the panel up, `dock.rs`'s stated contract). The
  landing does the open-or-show dual: document open →
  `Window::show_document(store, window, id, Some(range), fx)`,
  not open → `open_by_location_effect(..., Some(target))` (the
  `ShowWorkingCopy` pattern). Range = live interval when a card
  exists, wire range clamped otherwise. Navigation records a place
  (history works).
- Keyboard: `Select(±1)` / `Fold` / `Pick` on the cursor, speed
  search for free — all inherited from the composition.

## 8. Sending comments to the agent

The delivery leg (`MessageAttachmentKind::Annotations`): comments
reach the agent as an ATTACHMENT on an ordinary turn — AHP has no
other agent input door.

- **The buttons**: a paper-plane on each card (next to the check)
  sends THAT comment; a SEND ALL chip atop the dock panel sends every
  listed one. Both land in `comments.send` (`SendComments { ids }`).
- **The turn**: `Comments::send_to_agent` groups ids by owning
  session and dispatches ONE `chat/turnStarted` per group — text
  "Please address the attached review comment(s).", attachment
  `{ type: "annotations", resource: <session>/annotations,
  annotationIds }`. The chat: the comments' own session's default
  chat when it has one; otherwise the WINDOW's session-bound agent
  session on the SAME server (the local shape: files ride the fs
  session, the talk rides the agent session, one host serves both —
  same connection, so ordering holds).
- **The host expands** (`expanded_prompt`, server.rs): inside the
  turnStarted arm, each annotations attachment resolves against the
  live channel state and renders into the provider's prompt
  (`<review-comments>` block — file, 1-based lines, thread with
  `[author]` tags). The TRANSCRIPT keeps the message verbatim.
- **Sent = consumed**: the send's SUCCESS landing (`Sent`) removes
  the comments — cards, records, wire annotations (inline removal;
  `AppRequests` only drain inside content dispatches). A FAILED send
  keeps everything: feedback that never reached the agent must not
  vanish. `CommentRecord.sending` guards re-entry (the double-click
  pile-up) and survives echo folds; unsynced records never send.
- **Ordering**: the host reads its connection sequentially and the
  expansion runs inside the turnStarted arm, so the removals
  dispatched after the send's landing always fold AFTER the
  expansion read the annotations.
- **The remount hole this leg surfaced** (higent, not comments): a
  chat panel displaced by an open lost its pane-routed poll landings
  and the feed DIED until a session reopen. `PanelView::remounted`
  (the pane door's remount signal, called at `mount_focused` and the
  widget restores) re-boots the chat: epoch-guarded landings, the
  stale poll cancelled, a fresh subscribe rebuilds the rows.

## 9. Deliberately absent

- **Reply composition** in the card (the stack renders foreign
  entries; authoring a second entry needs a compose affordance).
- **Changeset-diff re-anchoring** of inbound stale ranges (resolve
  against the stamped turn's changeset, diff forward) — inbound
  ranges clamp against current content.
- **Gutter affordance** for adding comments on hover
  (`docs/editor/gutter.md` lists it).
- **Foreign-host hardening**: the VS Code host's actual tolerance
  for our `turnId` values is unprobed; the deferred-flush path (§3)
  is the safe default until probed.
