# Sessions as places to work: folders, trees, and the switcher

There is no WORKSPACE concept: a SESSION is the place to work —
`SessionId { host, session }`, the identity join with the protocol
([app-state.md](app-state.md)). This file keeps its historical name; the
vocabulary inside is session-first.

A session is where work lives: its FOLDERS derive from the protocol's
`workingDirectories` (`higent::session_folders` — the subscribed
channel mirror first, the catalog summary as fallback; no client-side
folder list exists), and everything app-side a session owns — its open
documents and diffs, its file tree, its recents, its chats, changeset
and comment mirrors, terminal channels — lives in the session's
`SessionState` row under its `Host` ([app-state.md §3](app-state.md)). Scratch
work rides scratch-only sessions: ids minted locally
(`SessionId::mint_scratch`, `scratch-space:<n>`) that never touch a
host; the default place every fresh window shows is the local host's
fs session (`SessionId::local_default`).

## Per-window sessions

Every window SELECTS a session: `Window.current_session`. The window
keeps one workbench tree per visited session — conceptually
`window.workbenches[current]` is the active tree; in the value, the
active tree sits in the window's layers slot and only the INACTIVE
trees ride the map (a HAMT: it clones structurally with the entity on
every content command; the one real tree copy happens at switch-back).
Switching (`himark::switch_session`):

- overlays dismiss FIRST — a modal's released widgets restore by pane
  index into the OLD tree, never the new one;
- the flip is immediate; on a FIRST visit the fresh workbench (a
  single scratch pane, the same recipe a new window uses) installs as
  a BATCH FOLLOW-UP (`EnterFreshSession`) so its scratch registers
  under the ENTERED session's projection ([app-state.md §4](app-state.md)) —
  one scratch per session, ever; toggling creates nothing;
- the DOCK is NOT dismissed (D7): its content is the session-scoped
  family list, so it rides the workbench — stashed with the outgoing
  tree, restored SETTLED with the incoming one (no slide; a dock
  stashed mid-close resumes closing and dismisses ordinarily); the
  keys move to content either way;
- same-session switches are no-ops.

Stashed trees are ordinary values: repairs and reparses land on their
editors through their session's projection (document-addressed
commands resolve their session by id), so a stashed session stays
warm. A primary open that lands AFTER a switch opens into the current
tree — the document still joins the list (the same acceptance as a
landing whose window closed).

The displaced-widget SHELF is the WORKBENCH's (D6): a hidden
terminal or search is session-scoped — it stashes and restores with
its session, and eviction takes it by containment. Overlays dismiss
before the switch, so their released widgets land in the OUTGOING
workbench; the peeker lists the CURRENT workbench's shelf (the
cross-session listing — pick switches, never migrates — is a later
rider).

## The drawer

The window's side slot always holds a DRAWER — the himark-level
chrome that owns APPEARING and DISAPPEARING for every side panel (the
file tree, the switcher). The drawer rolls the panel in on show,
rolls it away on focus loss (`ModalView::focus_lost`, filed by the
window layer when an interaction lands outside), and converts the
content's `Close` request into an animated exit — the real close
files when the slide settles. Pick requests (`Perform`,
`OpenLocations`) pass through untouched: the drain dismisses
instantly and the follow-up lands the same frame. Content panels hold
NONE of this — they draw a `DRAWER_WIDTH` surface at the left edge
and speak plain requests.

## The switcher

`session.switch` ("Switch Session", shift-cmd-U) toggles the switcher
drawer: a peeker-styled list whose rows DERIVE (nothing is stored) —
the local space ("Local") first, then the window's visited
scratch-only sessions ("Scratch N"), then every host's session
catalog (rows named by summary title, else the first folder), with
"+ New Scratch" last. Up/Down move the selection; Enter or a click
picks: picking another session files `ModalRequest::Perform` with the
switch command (`session.switch-to`) — the drain dismisses the drawer
and the workbench swaps in the same frame; picking the current
session just closes; picking "+ New Scratch" mints a scratch-only
session and switches.

## Folder scoping

Opening a local folder CREATES (or rebinds to) a real session on the
local host whose `workingDirectories` name it — the picker's landing
runs the drawer's session-open flow minus the chat panel
([app-state.md §8](app-state.md) 5b). Adding a directory to a session is a
`session/workingDirectorySet` dispatch, never a local list write. The
tree (`files.tree`) shows the current session's folders; the peeker's
path-find receives them from its toggle; a search panel STAMPS its
window's current session at open and searches those folders for its
lifetime (a peeker-remounted panel keeps its birth stamp). A session
with no folders launches no Find — the hostless story unchanged.

## The file tree (cmd-E)

cmd-T shows the TABLE OF CONTENTS ([toc.md](toc.md)) — the focused
panel's structure; the file tree sits on cmd-E (its toolbar button
is unchanged).

Folders do not open into anything: `files.open-folder` ("Open
Folder…") picks a directory and enters its session — that is the
whole command. Browsing is `files.tree`: a tree rolled over the
workbench's left edge — a quick view to open a file from, not a
persistent explorer occupying layout space.

The panel occupies the window's SIDE layer — a role of its own in the
window's three layers (workbench, side panel, modal), NOT a modal:
the workbench underneath stays alive and focused (carets included),
the panel takes only what it handles (clicks inside, Escape, the
animation clock), and every other interaction flows THROUGH to the
workbench while the window layer tells the panel it lost focus
(`ModalView::focus_lost`) — scrolling the editor keeps the panel up;
a click or a keystroke out in the workbench lands there AND rolls the
panel away. Opening a modal (the peeker, the palette) takes focus the
same way; the side panel sits under the modal layer. It slides in and
rolls out animated (`imba::anim`); the settled roll-out files
`ModalRequest::Close` — the side slot speaks the same request channel
as the modal slot, drained by the app after every content command.
Picking a document dismisses and opens through the standard flow into
the focused pane (`ModalRequest::OpenLocations`).

The tree itself is PRESERVED between appearances: the panel is a thin
face over the session's `hifiles::SessionTree` — THE one
`LocationTree` (rows, expansion, fetched listings, scroll) in the
session's row, cloned out at open (reconciling roots with the live
folder derivation: each folder is a top-level row) and written back
after every change. Dismissal ends the SEARCH (the speed-search query
clears in `destroy`); everything else survives reopening.

- **Rows** carry a `ResourceLocation` and render its `name()` with a
  disclosure triangle for directory-kind rows; the location IS the
  key ([list-tree.md](list-tree.md)) — no id minting, no reverse map.
- **Lazy by construction**: expanding a directory whose children were
  never fetched launches `ListDirectoryEffect` — deduped at the
  launch site by the pending set (mashing the triangle asks once;
  nothing downstream drops it — every launch lands) — and splices the
  answer in animated. Collapse folds the subtree back to one row and
  CACHES nothing — re-expanding re-fetches; expanded folders WATCH
  (docs: `crate::watch`), and events re-list.

## Sessions and Find

The search primitive runs over the session's folders:

```rust
pub struct FindEffect {
    pub folders: Vec<ResourceLocation>,
    pub term: String,
    pub target: FindTarget,   // Text (content) | Path (name navigation)
}
// Result = Vec<ResourceLocation>   — document locations only
// ask-answer: fire-and-forget — every launch runs and lands
```

Find answers LOCATIONS; what to do with them is the asker's business.
The handler is host-side: himark-api registers a NATIVE one beside the
fs capabilities — ripgrep's library halves (the `ignore` walker:
parallel-capable, .gitignore- and hidden-aware; `grep-searcher` /
`grep-regex` for content, literal and case-insensitive) run directly
over the filesystem, because for this distribution a `local`-authority
location IS a path. Foreign authorities are skipped — their finder is
whoever owns them (a remote agent's, [remote.md](../ahp/remote.md)). Every launch
site guards on a non-empty folder list, which is the whole hostless
story: with no session folders no Find ever launches, so no handler
is owed.

Both consumers surface never-opened documents with **full preview
parity**:

- **The peeker** (target Path) merges found rows under the open
  matches — same rendering, no second class. Selecting a found row
  fetches its text ONCE and builds a TEMPORARY document (the app's
  registered languages route it; never registered in `OpenDocuments`)
  that the one preview editor points at, exactly like an open
  document's preview. Picking it ADOPTS the temp document — registered
  under its location, shown instantly, no re-fetch; a pick that outran
  the fetch falls back to `ModalRequest::OpenLocations` (the modal
  mirror of the panel request) and the standard open flow. Closing
  without picking removes every temp document.
- **Search** (target Text) launches Find beside its open-document scan
  under one serial. The scanned open documents are the SESSION's —
  documents are per-session state ([app-state.md §5](app-state.md)). Found
  locations (already-open ones dropped, capped) each fetch; each
  landing scans the text with the live query's matcher, builds a temp
  document, and installs a `ResultGroup` — tint markup, fragments,
  bounded row editors — indistinguishable from an open document's
  group (the header shows the location's path). The install records
  own their temp documents and remove them on the next query's drain
  and on dismantle.

The install-ownership lesson this surfaced: the app clones the window
entity on every content command and puts the clone back, so a panel's
clone must CARRY its installed bookkeeping — a clone that drops it
orphans everything after the first command (a search fragment leak
taught this). The same sharing rule shapes the
projection itself: entities are never value-cloned into a gathered
store (paint records bookkeeping through interior atomics —
[app-state.md §4](app-state.md)).
