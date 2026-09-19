# Files: locations, fetch/store, and the file tree

A document must have an origin: without one nothing can say "this
document IS that file" — so nothing can re-open instead of duplicate,
save in place, show a dirty dot, or point at a remote. So documents
carry an abstract location identity, fetching and storing are host
capabilities behind effect handlers, and a file tree (on the unified
list, docs/ui/list-tree.md) browses directories and opens documents. It
is the small, local-first step of `docs/ahp/remote.md`'s direction —
every seam here is the seam a remote agent plugs into, with no shell
knowing the difference. The storage half — documents and their
records in the `OpenDocuments` repository — is **docs/editor/documents.md**.

## Resource locations

```rust
pub struct ResourceLocation {
    kind: ResourceType,      // glorified str: "document", "dir", one day a vcs revision
    authority: Authority,    // glorified str: OPAQUE, fully host-owned
    path: Arc<[String]>,     // the REIFIED path within a virtual tree
}
```

One universal shape for every resource a host can point at, engine-side
but host-MINTED: a picker answers locations, a directory listing
answers locations, and the engine echoes them back into
fetch/store/list requests. Three parts:

- the **kind** names what the location points to. Open-ended on the
  host side; the engine acts on exactly two constants
  (`ResourceType::document()`, `ResourceType::directory()`).
- the **authority** is completely opaque to himark and fully controlled
  by the host — the remote machine id, the git revision, whatever the
  host wants to stick to the location. himark carries it, compares it,
  hands it back; never reads it.
- the **path** is reified segments, not a joined string — which is what
  makes navigation LEGAL without parsing: a tree item's location
  extends to its children (`child(kind, segment)`), a display name is
  the last segment (`name()`), the language route is its suffix
  (`extension()`). The engine never does string surgery on an identity.

Whether the authority says `local` or `remote:box` is the host's
business. That opacity is the whole point: `docs/ahp/remote.md`'s remote
documents are THESE locations under a different authority, answered by
different handlers — the engine code paths are already done.

## The document entity

Every document carries an identity record — the thing the user's
mental "that file" maps onto. The record and THE DOCUMENT VALUE
itself live together in the `OpenDocuments` repository, keyed by a
registry-minted `DocumentId` — the full storage story
(register-at-display, the editor-refcount retraction rule, the
gather/scatter round trips) is **docs/editor/documents.md**; the parts that
matter here:

```rust
pub struct DocumentEntity {                 // fields crate-private
    document: Document,                     // THE value — no loose documents
    location: Option<ResourceLocation>,     // None = scratch/untitled
    title: String,                          // display fallback for locationless ones
    registered: u64,                        // the open-order stamp
    last_opened: u64,                       // the recency stamp
    saved_revision: u64,                    // the document revision last stored
    save_token: …, watch: …,                // the save lane and the host watch
}
```

The name derives from the location's tail (locationless documents keep
their given title), and `saved_revision` against the document's
edit-log revision is the dirty flag ("modified since last store") the
UI can render honestly.

The entity is the dedup point: **opening a location that is already
open focuses the existing document** (the `show_document` + `touch`
path the peeker already uses) instead of minting a twin. Lookup is by
location over the open list — a handful of entries, no index needed.

## Fetch and store

Two host capabilities, declared as effects and implemented by
EXTERNAL handlers exactly like the picker (request id → callback →
park → fulfill):

```rust
pub struct FetchDocumentEffect { pub location: ResourceLocation }
// Result = Option<String>   — the text, or None (gone, unreadable)
// ask-answer: fire-and-forget — every launch runs and lands

pub struct StoreDocumentEffect { pub location: ResourceLocation, pub text: String }
// Result = bool              — stored or not
// save lane: the document entity stores the in-flight token; a newer
// save cancels it through the effects builder and launches afresh
```

Supersession is a delivery decision, not an optimization: a cancelled
launch's result is never delivered — so ask-answer effects (a fetch, a
listing, an open) are never put on a lane; every asker is owed its
answer. A lane exists only where the newest launch subsumes the older's
landing: a save (the higher-revision stamp covers the lower). Producers
that want one-ask-per-target dedup at the launch site.

Capability-table entries beside `pick_files` (`fetch_document`,
`store_document` callbacks carrying the request id and the location's
ABI form — kind, authority, segments), fulfilled through
`himark_host_fetched` / `himark_host_stored`. A host that brings no
fetch capability answers the way every absent capability answers: the
handler is never registered, calls yield `None`, and the commands that
need it never register either.

### Opening

The picker does not read files. Its answer is locations
(`HimarkLocation` on the ABI — Swift's `FileImport` keeps its
directory walk and extension filter; it never reads content). The
open flow:

```
picker → Vec<ResourceLocation>
  each location:
    already open?  → focus it (show_document + touch)   [main thread]
    else           → OpenByLocationEffect { window, location, primary }
```

`OpenByLocationEffect`'s handler is the first real user of the
handler-composition seam (`EffectCaller`, docs/ui/effects.md "Effects calling
effects"): it CALLS `FetchDocumentEffect` and awaits the text — parking
on the host round trip without blocking the queue — then builds the
document right there with the Workshop's fonts and theme
(`document_for`, routed by the location's `extension()`), landing as
the ordinary `AppCommand::Opened` carrying the location. `Opened`'s arm
registers the `DocumentEntity` with it and `saved_revision` = the fresh
revision. A fetch that answers `None` lands a loud no-op (the file is
gone; nothing to show — a proper error surface is its own arc).

The demo/scratch paths land `Opened` with `location: None` and behave
like any locationless document.

### Saving

Saving is an engine command — `file.save`, registered only when
the host brings `store_document`:

```
file.save → focused DocumentEntity
    located    → StoreDocumentEffect { location, text: snapshot }
                 landing stamps saved_revision = the snapshot's revision
    locationless / synthetic → save-as: a host save-panel pick answers
                 a fresh location, the store runs against it, and the
                 document re-points (a saved scratch becomes that file)
```

The snapshot is the document text at launch (documents are persistent
values — the capture is free); the landing compares stamps, so a save
overlapped by typing leaves the entity honestly dirty.

## Directories

The same capability shape, one level up:

```rust
pub struct ListDirectoryEffect { pub location: ResourceLocation }
// Result = Option<Vec<ResourceLocation>>  — each child's KIND says what it is
// ask-answer: fire-and-forget — every launch lands (see above)
```

No entry enum: a listing answers full locations, and the kind field
already distinguishes them. A `list_directory` callback +
`himark_host_listed` fulfill; `None` = gone/unreadable, distinct from
an EMPTY directory. Entries come back host-sorted (directories first,
then names — the host knows its filesystem's collation better than we
do). The host also brings a folder picker: `files.open-folder` ("Open
Folder…") asks for ONE directory and answers its location — the root
the file tree grows from.

### Reveal in tree

Opening the tree (cmd-E, `files.tree`) focuses it on the CURRENT
file. The toggle
asks for the target through the focus chain — the
the focus chain's `location` slot (docs/ui/UI.md, Focus)
(the DataContext pattern, see docs/ui/ime.md): whatever LOCATED editor
holds focus answers with its stamped `ResourceLocation`; a search
row's editor names its own file; nothing walks panel structure. The
resolved target passes into `SessionTreeView::open(…, reveal, fx)`.

Because the tree is lazy, the reveal is a PARKED TARGET, not a
procedure: `pending_reveal` on the panel plus one descend step —
expand the deepest visible ancestor; if it is collapsed, launch its
one listing (the triangle-mash dedup applies) and stop. Every listing
landing re-runs the step one level deeper, until the row exists: it
becomes the SELECTION (the tree chrome's `highlight` fill) and arms a
scroll reveal — the row's rect re-emits per clock tick from INSIDE
the scroll (the list's own cursor reveal, docs/ui/list-tree.md §5) until the glide has
served it. Any user click cancels the pending walk — a scroll that
keeps yanking away from browsing is worse than none. A target outside
every root, or missing from its parent's listing, quietly does
nothing.

## The session as the place to work

Folders, the file tree, the switcher, Find and its consumers live in
their own document: docs/ui/workspace.md (sessions are the places to
work: per-window current session, one workbench tree per session,
folders derived from the session's `workingDirectories`, the drawer
abstraction).

## The FFI surface, summarized

The location's ABI form is `HimarkLocation { kind, authority, segments
}` over `HimarkStr` slices — borrowed for the duration of a call, both
sides copy on receipt. Capability callbacks (all optional, all
worker-thread): `fetch_document(ctx, request, location)`,
`store_document(ctx, request, location, text)`, `list_directory(ctx,
request, location)`, `pick_folder(ctx, request, window)`; the picker's
answer is locations. Fulfill entry points (main-thread):
`himark_host_picked_folder(request, location | null)`,
`himark_host_fetched(request, text | null)`,
`himark_host_stored(request, ok)`, `himark_host_listed(request,
entries | null)`. The Swift shells implement fetch/store/list with
ordinary file reads and writes as capability answers; no other
ability is asked of the host.

## Related seams, and what is deliberately absent

- **Watching** — external-change reload lives in `himark::watch`
  (subscriptions per expanded folder and open document, `FileChanged`
  landings, refetch); the session-over-AHP variant is
  docs/ahp/agents-fs.md.
- **Error surfaces** — fetch/store failures land as loud no-ops;
  proper UI (toasts, conflict banners) is its own arc.
- **Tree niceties** — there are no file icons, git status badges, or
  rename/create/delete affordances; when they come, they are store
  capabilities too, same shape.
- **Remote locations** — nothing to do here by design; a remote host
  mints locations under its own authority and its agent-backed
  handlers answer them (`docs/ahp/remote.md`).
