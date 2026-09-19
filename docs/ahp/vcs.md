# VCS

himark's version-control surface is built on ONE principle: the
protocol already speaks change the direct way, so the client renders
protocol state instead of owning a git model. A session carries
CHANGESET channels — the aggregated diff of a working directory, per
file, with before/after content refs — and a HISTORY channel — the
revision log, each commit pointing at a commit-addressed changeset.
The git work runs in the agent host (`backend/git`, the `higit`
CLI backend), next to the working tree; the client consumes channels
(docs/ahp/ahp-ext.md, docs/ahp/ahp-history.md). The engine crates learn
nothing about git; the shells are git-free.

## The wire, briefly

- `hihost-changes:/<folder>` — the working copy's uncommitted
  changes; `hihost-changes:/<folder>?commit=<sha>` — one commit's
  files. Both advertised through the standard changeset catalog
  (`SessionState.changesets`: an `uncommitted` entry and a `history`
  entry per working directory).
- Sides travel BY REFERENCE: `hihost-git:/` blob refs (and plain
  working-file locations for the live side), fetched over
  `resourceRead` like any resource.
- Freshness is host-owned: a polled watch on the repository's reflog
  signal recomputes on commits/checkouts/resets, and the host's own
  `resourceWrite`s recompute directly — a save updates the changes
  list without the client asking.
- `hihost-history:/<folder>` — the log in windows of 100, paged by a
  `history/grow` dispatch; `history/commit` commits the working tree
  whole and the host re-answers both channels.

## The Changes view

`changes.view` — a dock panel over the session's uncommitted
changesets: one tree per working directory, files under their
folders, add/remove counts, and the session folders' roots opening
the DIFF CANVAS (docs/editor/diff-canvas.md) — the root row opens the whole
working copy as a scrolling wall of file diffs; a file row opens the
canvas revealed at that file. The COMMIT COMPOSER heads the
working-copy canvas as its first row (⌘⏎ or the COMMIT button):
committing dispatches the history extension's commit; the emptied
changes list and the new head arrive as ordinary channel updates —
nothing predicts, the views render what the host re-answers.

The panel mirrors before-refs into the shared `ChangeRefs` table as
listings land — the join the stripes machinery reads (below).

## The History view

`history.view` — a dock panel over the session's history channels:
the commit log (summary, refs, author, outgoing marker), paged
deeper on demand. Expanding a commit fetches its files — a
commit-addressed changeset channel — and the rows open the diff
canvas for the commit (the commit row opens the whole revision; a
file row opens it revealed at that file). Both sides of a commit's
file diff are immutable blob refs; the working copy's diff keeps its
live side.

## Stripes: the base chain rides changesets

A document's gutter stripes (docs/editor/diff-stripes.md) need a BASE — the
"before" this buffer is compared against. The resolution is one
effect with one honest answer: `FetchBaseEffect` is handled by the
seat's route (`RouteBase`), which looks the document's path up in
the mirrored `ChangeRefs` and answers the changeset's before-ref
under the same authority. A file the session has not touched has no
entry — no base, no stripes: the session's change IS the interesting
diff. The answered location enters the standing base chain (fetch →
diff → sweep) unchanged; the pinned base registers in the document
registry born clean, deduped by location like any open.

## Editing inside the diff

Both sides of a diff pane are ordinary editors (docs/editor/diff.md), and
the working side of an uncommitted diff is THE working document —
`by_location` dedup makes the diff's right half the very buffer the
user already has open. Edits compose into the diff live and save
through the ordinary save lane; the changeset channel's recompute
then confirms what the buffer already showed.

## What deliberately does not exist

- **No staging and no partial commit** — the commit is
  commit-all-with-a-message; hunks and index management are the
  agent's territory or the terminal's.
- **No branch operations, no blame, no per-file history UI** — the
  history channel answers the log; operations on it beyond commit
  stay outside.
- **No client-side git** — there is no in-process repository model
  and no local fallback arm; a folder with no live host has no vcs
  surface (the honest contract, [Design.md](../Design.md)).
