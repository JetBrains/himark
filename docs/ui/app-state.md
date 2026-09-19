# App state: `Application`, `AppState`, and the derived `Store`

The state is organized around the protocol's own hierarchy: **hosts
own sessions; sessions own everything in them; windows project one
session at a time.** One persistent value holds it all; the `Store`
a dispatch receives is a DERIVED, per-dispatch working set built
from that value and written back after commands run.

```rust
/// The machinery: everything with live innards or out-of-store
/// plumbing. Never cloned, never snapshotted.
pub struct Application {
    state: AppState,
    /// The committed projection cache backing the pub `&Store` read
    /// API (`store()`) — memoization, never authority; refreshed at
    /// every commit.
    committed: Store,
    /// The seats: HostId → Arc<dyn AhpServer>. Live connection
    /// objects, OWNED here (registration = `register_seat`) and
    /// INJECTED into every gathered projection as a read-only
    /// component — `Servers::seat(store, id)` lookups work
    /// everywhere; scatter drops it.
    seats: higent::Servers,
    ui: Rc<UiCtx>,                 // the UI thread's !Send resources
    effects: EffectLauncher,
    handlers: Arc<Handlers>,
    workshop: Arc<editor::Workshop>,
    stats: Stats,
    ui_arena: Arena,
    pending_file_events: Vec<watch::Subscription>,
    pending_diff_events: Vec<DiffId>,
    settle_requested: bool,        // a perform raised Effects::settle
    settling: bool,
}

/// THE state: one persistent value, data all the way down
/// (docs/code-style.md: rpds everywhere, identifiers typed,
/// containment is the link). Cloning is pointer bumps; a clone IS a
/// transactional snapshot.
pub struct AppState {
    hosts: higent::Hosts,       // HostId → Host
    windows: Windows,           // WindowId → Window
    /// App-globals that belong to no host and no window: theme +
    /// font env, the registries, the space mint, capability
    /// markers.
    globals: Store,
}
```

## The shape

```rust
/// One agent host: the catalog the protocol feeds, and EVERY piece
/// of session-scoped state living UNDER it, keyed by session uri.
/// Lifetimes are structural: disposeSession removes one subtree; a
/// host row removed takes everything it served.
pub struct Host {
    name: String,
    status: HostStatus,                       // Idle | Connecting | Connected | Failed
    agents: VectorSync<AgentInfo>,
    /// The session catalog, protocol order (`listSessions` fills,
    /// the root feed mutates).
    sessions: VectorSync<SessionSummary>,
    /// The live channel mirrors: chats, default chat, working
    /// directories, session config — fed at subscribe and by the
    /// session feed's poll arms.
    states: HashTrieMapSync<Uri, SessionChannel>,
    /// The per-session families (below).
    families: HashTrieMapSync<Uri, SessionState>,
    /// The host's OWN uri codec, installed at registration
    /// (`Hosts::install_uris` / `Hosts::uris(store, host)`) — uri
    /// spelling is the host's business, not a global table.
    uris: Option<Arc<dyn ResourceUriMap>>,
}

/// Everything a session owns, as ONE row. Each field is a component
/// type dispatch reads directly — gather puts them in the store
/// flat, scatter takes them back. An all-empty row stores nothing.
struct SessionState {
    trees: hifiles::SessionTree,
    recents: RecentLocations,
    chats: higent::Chats,
    changes: hichanges::Changes,
    history: hihistory::History,
    comments: hicomments::Comments,
    terminals: terminal::Terminals,
    documents: OpenDocuments,
    scratch_names: documents::ScratchMint,
    location_lists: location_list::LocationLists,
}

pub struct Window {
    /// The window's layers: toolbar, focus, the ACTIVE workbench,
    /// and the transient side/modal overlays. Side and modal are
    /// window-level because a session switch dismisses them first —
    /// there is never overlay state of theirs to stash.
    content: Layers,
    viewport_size: Size,
    current_session: SessionId,
    dock_width: f32,                  // a chrome preference — window-level
    /// The INACTIVE workbenches, stashed per visited session; the
    /// active one sits in `content.workbench`.
    workbenches: HashTrieMapSync<SessionId, Workbench>,
    focused_location: Option<ResourceLocation>,
    focus_generation: u64,
}

pub struct Workbench {
    pub root: WorkbenchNode,
    /// The dock rides the workbench, so it SURVIVES the session
    /// switch with its tree: dock content (Files/Changes/Comments)
    /// is the session-scoped family list, so per-session is the
    /// honest home. Restore mounts SETTLED — no slide on
    /// switch-back (the slide is an open gesture).
    dock: Option<Dock>,
    /// The bottom sheet (terminal, search results …), same
    /// containment.
    bottom: Option<Sheet>,
}

/// A session viewed as a place to work — the identity IS the join;
/// there is no separate workspace mint and no binding maps.
pub struct SessionId {
    pub host: HostId,
    pub session: String,     // the server-minted session uri
}
```

`HostId` is the registration-order mint. There is no "workspace"
vocabulary anywhere: a session's folders are a DERIVATION of its
`workingDirectories` (`higent::session_folders` — channel mirror
first, catalog summary as fallback); adding a folder is a
`session/workingDirectorySet` dispatch, never a local list write.
The switcher derives its rows from the hosts' catalogs plus the
window's visited scratch spaces; scratch-only spaces are
`SessionId::mint_scratch` ids that never touch a host.

Hosts register ASYNC, so a window opened before the local host is
designated rides the reserved hostless fs id
(`HostId::LOCAL` + `host_discovery::LOCAL_FS_SESSION`).
`Application::designate_local_host` — the one road both designation
sites call — sets the `LocalHost` component AND re-keys every
window's local fs-session ids onto the designated host
(`Window::adopt_local_host`): one space, one id, and the boot
scratch follows the designation.

## Why this shape

- **Lifetimes are containment.** "Session dies" is one subtree
  removal; a host disconnect is one row removal; nothing can leak
  because nothing session-scoped lives anywhere else. Before this
  shape, every channel family kept its own app-global singleton
  that had learned the session's name, and disposal meant sweeping
  seven of them.
- **Protocol management has one shape.** Every channel family we
  subscribe (session, chat, changesets, annotations, documents,
  terminals) is per-session state on some host — parallel fields
  with the same key, and "which seat serves this" has one answer (a
  `HostId` lookup) instead of installed resolver closures.
- **The remote story is free.** A remote workspace is a `Host` row
  whose seat crosses a byte-pipe ([remote.md](../ahp/remote.md)). The state shape
  does not know the difference.
- **The store stays honest.** The flat singleton catalog survives —
  as the DERIVED working set the dispatch code speaks — while the
  persistent truth has a real shape underneath it.

## Objects out of the state

The rule: **the state holds data — values that are what they say.
Live objects with their own out-of-store plumbing (seats and their
runtimes) live on `Application`.** One refinement: an Arc'd live end
whose EVERY consumer reaches it through the store may ride session
state as a value — one canonical copy in a family behind take/put
doors. The terminal family owns its `Arc<Session>` (the PTY;
`Session::drop` hangs up as the eviction backstop); the diff-pane
rows live in the diff registry that already owned the pairing;
`himark::Lists` owns each search/references result set;
`new_session::Composers` keys the composer value by window (a
global — windows are not session rows).

Panels are HANDLES over those rows. There is no displaced-widget
shelf: displacement drops the handle, and the peeker lists family
rows through `PanelView::family_row` + the `himark::family_rows`
catalog (`RowMinters` for plugin-owned kinds), minting fresh handles
at open. Inert callables (registered commands, navigators — `Arc<dyn>`
values that are configuration, not connections) stay in state:
"mostly data" is the standard, and a registry of immutable command
objects rides snapshots harmlessly.

## The derived Store: gather → perform → scatter

The `Store` type (`imba::store`) is untouched by all of this, and so
is every `X::of(store)` read and every `perform(&mut Store, …)`
signature. What the shape decides is where the store COMES FROM and
where its writes GO:

```
dispatch(window, event):
    store = state.gather(window, scope, seats)   // projection, O(components)
    commands = widget_tree(store).handle(event)
    perform each command against &mut store
    state.scatter(store)                         // write-back by scope
```

- **Gather is a projection by (window, session)**: the globals as-is
  (one `Store` clone — pointer bumps); the whole hosts forest; ONE
  window's entity (`Windows::project` — the kept entity stays
  SHARED rather than cloned, because paint records layout
  bookkeeping through interior atomics and a cloned entity would
  swallow those writes); the read-only `Servers` handle; and the
  scope session's families FLAT, as the very component types
  dispatch reads (`OpenDocuments`, `Chats`, `Changes`, `Comments`,
  `SessionTree`, `RecentLocations`, `terminal::Terminals`, …). One
  `Gathered(SessionId)` marker names the scope; a family write in a
  scopeless batch trips a debug assert.
- **Perform is untouched** — commands, effects, landings, the
  entity gather/scatter INSIDE a dispatch all work on the projection.
- **Scatter** drops the injected seats, takes the family components
  back into the scope session's `SessionState` row under its `Host`
  (all-empty rows store nothing), absorbs the touched window
  entries back (insert-only), and keeps the rest as the next
  `globals`.

The batch loop scopes PER COMMAND (scatter/regather between scope
changes); dispatch and paint gather per WINDOW; paint gathers
read-only and scatters nothing (paint-reported commands re-enter as
an ordinary batch). One projection is outstanding at a time —
dispatch is single-threaded — so scatter is a plain write-back,
never a merge. `store_mut` (tests, registration doors) is a
scatter-on-drop guard: gather, mutate, scatter.

Cross-session work rides the COMMAND SURFACE, never a second store:
`AppCommand::InSession(SessionId, Box<AppCommand>)` scopes
channel-feed landings (a chat poll carries its panel's OWNING
session — a `ChatPanel` field, fixed at birth; changes/comments
folds derive theirs from the folder authority or the launch scope),
and `BatchRequests` — a transient the batch loop drains between
commands, never scattered — carries enter-work: a first-visit
switch flips the window and files `EnterFreshSession` (the fresh
scratch registers under the ENTERED session); the session-open door
files `EnterSessionWork` (ensures + the chat panel mount after the
workbench install).

App-level walks (theme propagation, file/diff broadcasts, the focus
ask, validation, the engine's clock) read the VALUE
(`Windows::ids`/`entity`, `Application::window_ids` /
`window_viewport` / `window_store`) and run at the batch tail per
window.

## Documents

Documents live PER SESSION — the same file open in two sessions is
two documents, and the peeker and search see the session's world.
The id mint is APP-GLOBAL (`documents::DocumentMint` —
`DocumentId(u64)` stays `Copy` and serials never collide), so a
document-addressed landing (`AppCommand::Entity`, `Watched`,
`Refetched`, the diff base chain) locates its session by forest scan
— `Hosts::session_of_document` / `session_of_watch` /
`session_of_diff`, current projection checked first for same-batch
registrations — and a landing for a disposed session drops cleanly:
the lookup fails and the command discards, the ordinary
gone-document behavior.

Scratches home under the local host's fs session — the local row
exists unconditionally, seatless and `Offline` when no backend runs
(the honest contract, [Design.md](../Design.md)): one documents road, no
app-global side bucket. The diff registry moves with the documents
it pairs — a diff pair never crosses sessions (the working document
and its vcs base are served by the same one). The batch-tail sweeps
(diff lanes, watches, stripe bases) run against the projection like
everything else.

## What this does not touch

Effects and handlers ([effects.md](effects.md)), the command surface and its
window routing, the widget/dispatch contract, entity gather/scatter
for windows and documents inside a perform, the register-at-display
document rule, and the wire/seat traits ([agents.md](../ahp/agents.md)). The
protocol side of every channel family is exactly the wire's —
this shape is a home for client state, not a wire concept.
