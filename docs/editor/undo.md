# Undo / Redo

Two stacks per **document**, riding what already exists: every text
mutation is an invertible [`Operation`] (deletes carry their text) and
already lands through the one edit door. Undo applies the inverse *as
a fresh edit* — a new revision through `Document::edit` — so repairs,
reparse, diff roll-forward, change notifications and multi-view caret
transforms all work unchanged, with zero special cases downstream.
Nothing rewinds; history only ever moves forward.

## 1. The model

```rust
/// On Document — a store value, so PERSISTENT structures throughout
/// (the [`EditLog`] discipline): a document clone shares the stacks
/// structurally, and each push is O(log n), never a copy.
struct UndoHistory {
    undo: rpds::VectorSync<UndoEntry>,
    redo: rpds::VectorSync<UndoEntry>,
    /// True while an undo/redo applies its operation — that edit must
    /// not record itself.
    replaying: bool,
}

/// One undoable step: the operation AS APPLIED (possibly several
/// coalesced keystrokes composed into one) plus the acting editor's
/// carets around it. Clone-cheap: operations are rope-backed,
/// multicarets share their spine. The snapshot is ONE optional value
/// — its parts never exist separately; `None` marks an edit recorded
/// outside a command dispatch (a host API call), which undoes with
/// transformed carets.
#[derive(Clone)]
struct UndoEntry {
    operation: Operation,
    snapshot: Option<CaretSnapshot>,
}

/// The editor the edit acted in, and its carets BEFORE the first
/// coalesced operation (the group's origin) and AFTER the last — undo
/// restores the before-state exactly (an edit is caret movement too),
/// redo the after-state.
#[derive(Clone)]
struct CaretSnapshot {
    editor: EditorId,
    before: MultiCaret,
    after: MultiCaret,
}
```

A persistent vector is a stack (`push_back_mut`/`drop_last_mut`), not
a deque — and that is fine: the stacks are UNBOUNDED, exactly like
the [`EditLog`] they sit beside (entries are cheap shares of ropes
the log retains anyway). No cap, no front-eviction machinery.

- **Record**: `UndoHistory::note_edit` runs in `edit_substance`
  beside `log.record`, for `Provenance::Ours` edits only — and never
  while `replaying`. Every recorded edit clears `redo`. "Composing"
  is read from the editors' marked state, which updates AFTER the
  edit — so a composition session's first step opens the entry and
  the rest absorb, commit included.
- **Stamp**: the caret snapshot is stamped by `stamp(editor, before,
  after)` after any non-history command that moved the revision —
  the one place the acting editor is known. `before` fills once, onto
  a freshly created entry only (a coalesced group keeps its origin);
  `after` refreshes on every stamp. Undo/Redo commands never stamp —
  they popped the entry they would deface. Edits arriving outside a
  dispatch carry `None` snapshots and undo with transformed carets.
- **Undo**: pop `undo`, apply `entry.operation.invert()` through
  `Document::edit`, push the entry onto `redo`. Refused while the
  editor is IME-marked (the `marked_of` guard).
- **Redo**: pop `redo`, apply `entry.operation` (the text is exactly
  the pre-entry text again — any interleaved edit cleared the stack),
  push back onto `undo`.
- **Replays are ordinary edits**: they go through the edit door as
  `Provenance::Ours` and append fresh [`EditLog`] revisions with
  fresh identities — `replaying` suppresses only the history
  recording itself. Downstream (repairs, reparse, sync) never learns
  an edit was an undo.
- **Carets**: `restore_snapshot` puts `before` (undo) or `after`
  (redo) onto the snapshot's editor if it still exists, else onto the
  invoking one; every OTHER view's carets transform through the
  applied operation like through any edit. The caret reveal is the
  command arms' generic reveal, not anything undo-specific.
- An entry that has been through undo/redo is closed to further
  coalescing — typing after an undo never grows a parked group.

**Shared documents**: a split shows one document — one history; undo
from either pane undoes the last edit wherever it was typed. Diff
halves are distinct documents, so each half has its own history.
Table cells swallow Undo/Redo in their own dispatch, so the commands
fall through to the hosting document's history.

## 2. Coalescing — the one deliberate wrinkle

Per-keystroke undo is technically simple and practically useless, so
a new entry MERGES into the top of the undo stack (by operation
composition; the group keeps the ORIGINAL entry's snapshot, so
`before` stays the group's origin) when either:

- the acting editor is **composing** (IME marked text) — the whole
  composition is one entry, or
- it **continues a word**: both the top entry and the new operation
  are plain inserts, the new insert lands exactly at the previous
  change's end, and the inserted text contains no whitespace —
  "typing a word" is one entry; a space, newline, caret
  move-and-return, or any non-insert breaks the group.

No timers, no clocks (store values cannot read them) — the rule is
purely structural, so it is deterministic and testable.

## 3. What clears history

- A **redo-invalidating edit**: any recorded edit clears `redo`
  (standard).
- An **external reload** clears BOTH stacks
  (`clear_undo_history`; called from `OpenDocuments::edit_external`
  and the refetch-absorbing path) — undoing across someone else's
  disk write into a state the file never had is a trap, not a
  feature.
- A **peer's edit over live sync** (`Provenance::Shared`,
  docs/editor/rebase.md) is NOT that: it clears nothing. Both stacks are
  carried across it (`UndoHistory::carry_across` — each entry's
  operation AND caret snapshot transformed the way a caret is, the
  foreign edit walked back through the undo stack one inverse at a
  time and forward through redo), so undo still removes exactly what
  we typed, where it now is. The peer's edit itself is nobody's to
  undo here.

## 4. The surface

- `EditorCommand::Undo` / `Redo`, arms beside the other edit
  commands; no-ops on empty stacks.
- Keymap: `cmd-Z` / `cmd-shift-Z`.
- Palette: `editor.undo` / `editor.redo`, contributed by
  `EditorView::caret_surface`.
- macOS: Edit-menu items forward as synthetic key events through the
  Rust keymap; himark ignores `NSUndoManager` — the document owns its
  history.
- winit/web: `cmd/ctrl-Z` arrives as a chord — no shell work.

## 5. The EditLog beside it

The history's substrate is the document's [`EditLog`] —
`{operations, identities: rpds::VectorSync}` — the append-only record
every revision lands in; `revision() == operations.len()`.
`record_as` right-pads an operation with `Retain` to the document's
width before pushing. Readers:

- `since(revision)` / `entries_since` — the raw tail;
- `compose_since(revision)` — the tail composed into one operation;
- `ranges_since(revision)` — the changed ranges of that composition;
- `as_of(revision)` — a log truncated to a past revision;
- `EditLog::transform_range(range, &op)` and `EditLog::coalesce(...)`
  — associated helpers for carrying ranges and merging adjacent
  operations;
- cross-log, free functions in the `edit_log` module (re-exported at
  the crate root): `common_base` finds two logs' shared prefix,
  `bridge` builds the operation carrying positions from one log's tip
  to the other's.

## 6. Out of scope (choices, not gaps)

Undo trees/branching, persistent (cross-session) history, per-caret
selective undo.
