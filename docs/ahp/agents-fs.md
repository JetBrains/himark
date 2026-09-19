# FS over AHP: the session as a filesystem

A workspace that HOLDS a session reads, lists, stores and WATCHES its
files THROUGH the session's AHP server (the step beyond the agents
arc, docs/ahp/agents.md). The protocol carries the whole surface —
`resourceRead` / `resourceWrite` / `resourceList` /
`createResourceWatch` — and himark's file capabilities are effects
behind one seam (docs/ahp/host.md's tables). This design is the join.

Why route through the seat at all: a session's workspace should be
exact wherever the agent host runs. With a local host the files
happen to be reachable both ways; with a remote one (the remote-host
design, docs/ahp/remote.md — arriving through AHP instead of ssh) the
session's directories exist ONLY on the server's side of the wire.
Routing session workspaces through the seat makes the local case
prove the pipe the remote case lives on.

Scope, per direction: **list, read, store, watch.** (Watch, with a
carve-out: a document mirrored over the documents extension is NOT
client-watched — the HOST is its reload authority and broadcasts disk
changes as its own document operations; see docs/ahp/ahp-documents.md §6.
The client watch serves everything else, and every document when the
extension is absent.) VCS (stripes,
higit), workspace search (ripgrep) and LSP are OUT — each shells out
against local paths and needs its own AHP extension design (remote
grep, remote git, remote language servers) before it can follow.

## The decisions

1. **The AUTHORITY is the router.** `ResourceLocation` carries an
   authority precisely so that where-a-resource-lives is part of its
   identity. A session workspace's folders mint under the `ahp`
   authority — `ahp:<server>:<session-uri>`, encoded and decoded ONLY
   by `hiahp::fs` (authorities are opaque to everyone else, as ever).
   Children inherit the authority through the ordinary
   `location.child(…)` walk, so the tree's listings, the editors
   opened from them, saves, and watches all carry the session address
   without any new plumbing. The one changed line in higent: the
   workspace-creation flow (`folder_location`) mints `ahp` instead of
   `local` when the session's seat answers the resource surface.
2. **The four effects stay THE seam; the ONE handler forks on
   authority.** No new effect types, no second handler (the hard
   rule): himark-api's `FetchDocumentHandler`, `StoreDocumentHandler`,
   `ListDirectoryHandler`, `SubscribeHandler`/`UnsubscribeHandler` —
   and the hostless native defaults — grow an `ahp` arm exactly the
   way `FetchDocumentHandler` already forks for higit's
   revision-scoped locations. Locations under `local` behave as
   ever; locations under `ahp` route to the seat. Nothing downstream
   can tell (the door is the location, not the caller).
3. **The seat directory: the worker-side mirror.** File handlers run
   on the worker, far from the store, and these effects are launched
   deep inside engine flows that cannot carry seats. So himark-api
   hands the file handlers an `Arc<hiahp::fs::SeatDirectory>` — a
   plain shared map `ServerId → Arc<dyn AhpServer>` written at
   registration time (the two built-ins) and read by the `ahp` arms.
   Host-territory state beside `HostBridge`, not a second registry:
   the STORE's `Servers` stays the truth the UI resolves against; the
   directory is the handlers' mirror of the same registrations.
4. **The seat trait grows the resource surface.**

   ```rust
   // AhpServer, the fs half — session-scoped: `channel` is the
   // session uri, resource uris are the server's own (file:///…).
   fn resource_read(&self, session: Uri, uri: Uri)
       -> SeatFuture<Result<Option<String>, String>>;   // None = gone
   fn resource_write(&self, session: Uri, uri: Uri, text: String)
       -> SeatFuture<Result<(), String>>;
   fn resource_list(&self, session: Uri, uri: Uri)
       -> SeatFuture<Result<Option<Vec<(String, bool)>>, String>>;
                                     // (name, is_directory); None = gone
   /// createResourceWatch (recursive: false — himark's ONE-level
   /// contract is the protocol's default) + subscribe the answered
   /// watch channel; `events` receives one call per
   /// `resourceWatch/changed` batch for as long as the watch lives.
   fn resource_watch(&self, session: Uri, uri: Uri,
                     events: Arc<dyn Fn() + Send + Sync>)
       -> SeatFuture<Result<Option<WatchHandle>, String>>;
   fn resource_unwatch(&self, handle: WatchHandle) -> SeatFuture<()>;
   ```

   The wire maps one-to-one onto the protocol (`ResourceReadParams`
   etc., utf8 only — binary answers an error, as file edits do). The FAKE seat implements the same surface over an
   in-memory tree per session (seeded by tests and the demo), with
   test doors that mutate it and emit watch batches — the whole arc
   is testable without a live host.
5. **Watch lifecycle is the protocol's.** `createResourceWatch`
   answers a watch CHANNEL (`ahp-resource-watch:/<id>`); subscribing
   it starts events, unsubscribing releases the watcher — no dispose
   command. The `ahp` subscribe arm mints an ordinary
   `himark::Subscription`, keeps `subscription → (seat, handle)` in
   the directory, and delivers every changed batch as the standing
   `AppCommand::FileChanged(subscription)` through the same
   inbox/wake path hiwatch uses — the clean-diff-apply refetch, the
   tree relists, save-echo safety all ride unchanged. The protocol
   coalesces batches server-side; himark's per-subscription refetch
   is batch-shaped already, so a batch simply becomes one signal.
6. **The exclusions guard by authority, answering their nothing.**
   `NativeFindHandler` skips `ahp` folders (workspace search over a
   session workspace finds only what other folders provide — honest
   until remote grep exists); `GitBaseHandler` answers `None` for
   `ahp` documents (no stripes); hilsp's location conversion skips
   them (no goto). Each is one authority check at the handler's
   door, and each lifts when its extension arc lands.

## What the user sees

Opening a session (drawer pick or "+") looks like any other open —
the workspace appears with the session's directories — but the tree's
listings, every opened document, every save and every external-change
refetch round-trips the agent host. With a local host the
difference is invisible (that is the point); with a remote one it is
the difference between a workspace and nothing.

## Testing

- **fake fs unit**: the in-memory tree lists/reads/writes,
  directories first; a write fires a standing watch at ONE level
  (direct child yes, grandchild no); releasing the watch silences it
  (the subscription-tied lifecycle); a created session's directories
  list empty, not gone.
- **engine e2e** (the drawer test's fs chapter, at the effect seam —
  the tree UI stays behind its host gate): the session workspace's
  folder carries the `ahp` authority, list/fetch/store round-trip the
  seat through the REAL worker, listed children inherit the
  authority, and the FULL WATCH PIPE runs — a document opened through
  the seat, an external write through the seat, the watch fires,
  `FileChanged` lands, the refetch updates the open editor.
- **live wire** (`#[ignore]`, `wire_serves`): the VS Code agent
  host's actual resource surface, seat-level — list, read, a write
  VERIFIED ON DISK, a `std::fs` mutation arriving as a watch event,
  dispose. (The host's resource surface was probed raw before any
  client code was written — the wire-facts rule; the whole surface is
  served.)

## Deferred, deliberately

- VCS, search, LSP over AHP (extension designs of their own).
- Binary content (Base64), `resourceCopy`/`Delete`/`Move`/`Mkdir`
  (no himark surface asks for them yet), the remote folder PICKER
  (`resourceList` is built for it — the "+" flow on a remote server
  needs it the moment one exists).
- Persistence of session workspaces across restarts (workspaces are
  in-memory; `ServerId` is per-run, and the `ahp` authority
  encoding inherits that).
