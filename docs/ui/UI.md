# UI

Immediate-mode lightweight framework: `frontend/imba`.

The family, from the framework out to the workbench:

- [effects.md](effects.md) — anything `perform` cannot afford becomes
  an effect: launched off-thread, landing rebases over what moved.
- [animation.md](animation.md) — animation without new machinery
  kinds: a frame is just another dispatch.
- [app-state.md](app-state.md) — `Application`, `AppState`, and the
  derived `Store`; hosts own sessions, sessions own everything.
- [list-view.md](list-view.md) — `ListView`, the rope-backed list of
  child views, and what lists of editors take.
- [list-tree.md](list-tree.md) — the same `ListView` with tree
  structure as data: one component for flat and nested.
- [scroll.md](scroll.md) — scroll-to-point: how a view deep in the
  tree asks to be revealed.
- [speedsearch.md](speedsearch.md) — type into any list or tree to
  filter it, no dedicated input field.
- [keymap.md](keymap.md) — shortcuts are data: chords to command ids.
- [ime.md](ime.md) — marked text and IME, decoupled from focus.
- [clipboard.md](clipboard.md) — copy, cut and paste without touching
  a platform pasteboard.
- [commands.md](commands.md) — named commands, the unit keybindings,
  menus, palettes and macros share.
- [navigation.md](navigation.md) — back/forward history per workbench
  pane, the undo-log shape.
- [workspace.md](workspace.md) — sessions as places to work: folders,
  trees, and the switcher.
- [toolbar.md](toolbar.md) — the header toolbar band.
- [toc.md](toc.md) — the table-of-contents drawer over the focused
  panel's structure.
- [peeker.md](peeker.md) — widget mounting, and the peeker as a
  buffer switcher.

The editor engine this framework hosts lives in `docs/editor/`
(start at [document.md](../editor/document.md), then
[documents.md](../editor/documents.md) and
[markup.md](../editor/markup.md)); the protocol features in
`docs/ahp/` (start at [host.md](../ahp/host.md) and
[ahp-ext.md](../ahp/ahp-ext.md)).

# Design

We do not retain a box hierarchy, but we still need to retain
transient UI state. Buttons are pushed, splitters are being dragged,
animations are running. Even though we just paint on a `Canvas`,
widgets must be composable, so we need a layout framework: the
Flutter/Compose approach, `Constraints` propagating down the tree,
sizes returned back.

## View

A possibly stateful UI component.
Views hold either plain values or ids; an id-holding view *gathers*
the value from the store on demand and *scatters* it back after
acting — nothing is cached, so there is nothing to refresh when
background work moves an entity.

Rendering is CURRIED twice — three frame artifacts, all transient,
none retained:

```
View ──display()──▶ Layout ──layout(constraints)──▶ Thunk ──realize(viewport)──▶ Widget
(retained state)    (structure)                     (sizes)  (materialized boxes + behavior)
```

A `Layout` is the STRUCTURE of a subtree with the state already read:
which pieces exist, in what arrangement, with every store-derived
fact captured — but no geometry yet. A `Thunk` is that structure
SIZED: constraints flowed down, sizes came back, nothing
viewport-dependent has been built. A `Widget` is the realized result
for one dispatch — what routes events, and what mints overlay
requests.

Each curry isolates one variable where it is decided:

- **`display()` reads the STATE.** Everything a subtree needs from
  the store and the view's fields is read here, once — the stage
  where a view is a view. Below this line no store exists.
- **`layout(constraints)` does the GEOMETRY.** Constraints down,
  sizes back — pure arithmetic over child layouts. Because it is a
  stage of its own, the arithmetic is WRITABLE ONCE: `Column`, `Row`
  and friends are `Layout` values any view returns, instead of every
  composite hand-placing children into raw containers.
- **`realize(viewport)` decides VISIBILITY.** What actually
  materializes — which lines' inlays, which rows — per dispatch.

```rust
trait View {
    type Command;

    fn perform(
        &mut self,
        store: &mut Store,
        ui: &UiCtx,
        command: Self::Command,
        fx: &mut Effects<'_, Self::Command>,
    );

    /// Read the state, name the structure. The ONLY stage with the
    /// store in scope; borrows from it and from the view ride the
    /// returned layout for the frame.
    fn display<'a>(
        &'a self,
        arena: &'a Arena,
        store: &'a Store,
        ui: &'a UiCtx,
    ) -> impl Layout<'a, Self::Command> + LayoutValue + 'a;

    /// Optional. This view is LEAVING FOR REAL — a discard from the
    /// hierarchy, not a frame clone dropping.
    fn destroy(&mut self, store: &mut Store, fx: &mut Effects<'_, Self::Command>) {}

    /// The SEMANTIC focus walk (see Focus below). Focus is state,
    /// and the view is the state carrier: a composite delegates to
    /// the child its own state says is focused, appending its own
    /// palette commands on the way. No layout, no arena, no
    /// viewport.
    fn focus_data<'w>(&'w self, store: &'w Store, ui: &'w UiCtx)
        -> FocusData<'w, Self::Command> { FocusData::default() }
}

/// A structured, unsized subtree: state read, geometry pending.
/// Consumed by sizing (single-shot, like every frame artifact). Type
/// erasure goes through `LayoutBox` — the same arena-box story as
/// `ThunkBox`/`WidgetBox`.
trait Layout<'a, Command> {
    fn layout(self, arena: &'a Arena, constraints: Constraints) -> ThunkBox<'a, Command>
    where Self: Sized;
}

/// A sized, unrealized subtree. `size` must answer WITHOUT the
/// viewport — parents place children and scrolls clamp against it —
/// and every lazy producer already can: sizes are metrics, not
/// materializations. Consumed by realization. Type erasure goes
/// through `ThunkBox`/`WidgetBox` — arena boxes whose destructors
/// still run with the frame tree, so a child costs a bump pointer,
/// not a malloc, and the arena reset reclaims the whole frame's
/// memory at once.
trait Thunk<'a, Command> {
    fn size(&self) -> Size;

    /// Compose's `FirstBaseline` alignment line: distance from this
    /// thunk's top to its first text baseline, when it has one.
    /// Wrappers forward it; containers propagate the topmost placed
    /// line; `Row` children can align by it.
    fn first_baseline(&self) -> Option<f32> { None }

    fn realize(self, arena: &'a Arena, viewport: Rect) -> WidgetBox<'a, Command>
    where Self: Sized;
}

trait Widget<'a, Command> {
    fn size(&self) -> Size;
    fn handle_event(&self, arena: &Arena, event: &Event, viewport: Rect) -> EventResult<Command>;

    /// Whether a point on this widget stops pointer fall-through in
    /// z-ordered stacks (overlays, ZBox) — true by default, so a
    /// popup shadows what it covers.
    fn blocks_pointer(&self, point: Point) -> bool { true }

    /// Out-of-flow requests this subtree wants displayed in some
    /// ancestor's OVERLAY HOST (see Overlays). DRAIN semantics: the
    /// realizing owner calls this exactly once, right after realizing
    /// the widget, and offsets the anchors by wherever it placed it.
    /// Default is empty — and an empty `Vec` does not allocate, so
    /// the common path costs nothing.
    fn overlays(&mut self) -> Vec<Overlay<'a, Command>> { Vec::new() }

    /// The LAYOUT-derived focus answers (the IME seat), folded up
    /// the realized tree with translate/clip like paint. The TARGET
    /// names the focused seat (harvested from the semantic walk); a
    /// widget answers by RECOGNIZING its own key — containers stay
    /// dumb folds, and no second copy of the focus routing exists on
    /// this side. See Focus.
    fn layout_data<'w>(&'w mut self, target: SeatKey) -> LayoutData<'w, Command>
    where 'a: 'w { LayoutData::default() }
}
```

The `'a` on the traits is the overlays' doing: widgets were always
frame-borrowing values, but a drained `Overlay` MOVES OUT of the
widget while still carrying that borrow, so the lifetime must be
nameable on the trait, not implied by the value.

Why the viewport curry is load-bearing: overlay requests are minted
by REALIZATION. A lazy subtree — the editor walking only the
viewport's lines for inlays, a list deriving only visible rows —
cannot answer `overlays()` from a widget that was built
viewport-free; there would be nothing to hang the requests on.
With the thunk stage, laziness and overlays stop conflicting:
`realize(viewport)` is the walk those types run to derive visible
content, promoted to a named stage that runs BEFORE routing — so the
things it materializes can carry requests, and only the VISIBLE ones
exist to be paid for. Off-screen placed children still realize — with
an honest empty viewport, so the drag pair (the one focus-owned event
family left on the tree) keeps reaching them — and the LEAF decides
what an empty viewport materializes (the editor: no lines).

An eager widget is trivially a thunk that ignores the viewport. The
design WANTS that as a blanket `impl<W: Widget> Thunk for W`, but
coherence forbids it: a downstream crate may legally implement
`Widget<'a, TheirCommand>` for an imba generic like
`Container<'a, TheirCommand>`, so the blanket would overlap every
manual `Thunk` impl. The lift is therefore ONE adapter —
`imba::eager(widget)`, the `Eager<W>` wrapper — applied at the
construction site; forgetting it is a TYPE error at the place site,
never a silent drop. `Eager` is also a `Widget` by delegation, so its
realize is a `Box` unsizing coercion, not an allocation (`leaf()`
returns pre-lifted). Manual `Thunk` impls exist only where realize
does real work — the lazy types, and cross-command wrappers whose
same struct serves as its own thunk and realized halves. The
combinator surface (`thunk_ext::ThunkExt`) composes THUNKS: `.map`,
the paint and event wrappers, `.focus_scope`, `.overlay`,
`.overlay_host` all defer their widget halves to realization, and two
combinators exist purely for the seam —
`.wrap(|realized| Shell { inner: realized, .. })` for compositor
shells over a lazy child, and `Container::wrap_realized` for
compositors that keep the realized container's `route_to`.
(`handle_event` still receives the viewport — paint clipping and
per-child intersection use it — and it is the SAME viewport
realization ran with.)

### Layouts

The layout stage exists to make placement arithmetic a LIBRARY. The
smell it removes: compute a size, mint a `container`,
`place(x, y, …)` children at hand-summed offsets, repeat the same
column math the neighboring view also hand-rolled — chrome pads,
header bands, min-height clamps, divider rules, all inline, none
shared. With the stage, that code is values:

```rust
fn display<'a>(&'a self, arena: &'a Arena, store: &'a Store, ui: &'a UiCtx)
    -> impl Layout<'a, Self::Command> + LayoutValue + 'a
{
    let chrome = env::Themes::of(store).ui().chat.clone();
    Column::new(arena)
        .gap(chrome.gap)
        .child(self.header.display(arena, store, ui).map(Command::Header))
        .child(self.body.display(arena, store, ui).map(Command::Body))
        .pad(chrome.pad)
}
```

- **Stock layouts.** `Column` and `Row` (Compose shape: `gap` =
  spacedBy, per-child `weight` over one shared flex algorithm,
  `align_items` cross alignment) hold child LAYOUTS, split the
  incoming constraints (fixed, weighted, and intrinsic children — a
  child sized by its content reports through its thunk), and answer
  one container thunk with every child placed. The leaves: `Text`
  (single-line, self-measuring, optional `tracking`), `Fill`,
  `Fixed`, `spacer`. The modifier surface on `LayoutExt`:
  `pad`/`pad_xy`/`pad_insets`, `align` with the 9-point `Alignment`,
  `width`/`height`/`sized`, `map_layout`, `.backdrop(painter)`
  (`Modifier.background`, painter-shaped), `.on_click`/`.on_event`
  (`Modifier.clickable` and the raw-event escape hatch; handlers are
  `EventHandler` values — closures via the blanket impl, `OnClick`
  reified), and `.shield()` — the reified press-stopper with
  fallback semantics: content answers first, the shield eats what
  falls through. `ZBox` is Compose's `Box` (std owns the name):
  visual stacking, distinct from `imba::stack::Stack`, which is the
  base+modal VIEW. `Button` is Compose's `Button(onClick){content}`:
  a CONTENT SLOT (any layout — a `Text`, a `Row` of texts) centered
  in a padded fill/stroke/radius surface, `onClick` a
  command-MINTING lambda (no `Clone` bound on commands),
  `.enabled(false)` swallowing presses. `Container` remains the
  ESCAPE HATCH for genuinely free-form placement — Column and Row
  are built on it, and a view may still be, but reaching for it raw
  is the exception that signals "this arrangement has no name yet".
- **Baseline alignment.** `Thunk::first_baseline` is the
  `FirstBaseline` alignment line — `Text` provides it from its
  font's ascent (`WithBaseline` is the provider wrapper for custom
  thunks), every thunk wrapper forwards it, containers propagate the
  topmost placed line (min-merge, offset by placement — so `Pad`
  shifts it for free), and `Row::child_by_baseline` is
  `Modifier.alignByBaseline`: the group shares its deepest line,
  sized line-plus-deepest-descent, with `Row::child_aligned` as the
  per-child `Modifier.align`.
- **Command mapping happens at every stage.** `LayoutExt::map` wraps
  a layout so the thunk (and widget, and overlays) it eventually
  produces are mapped — the same compile-enforced boundary story as
  `ThunkExt::map`, available one stage earlier so composition reads
  at the place a child is named.
- **`LayoutValue`** is the Command-free marker bound (one line per
  layout struct) that keeps the modifier surface inference-free on
  command-generic layouts like `Text` without leaking methods onto
  unrelated types. `View::display` requires it on the opaque return,
  so view outputs compose with modifiers directly
  (`view.display(…).map_layout`).
- **The closure adapter.** `imba::laid(f)` lifts
  `FnOnce(&'a Arena, Constraints) -> Thunk` into a `Layout` — the
  hand-rolled form. A `laid` body at a call site marks placement
  arithmetic not yet extracted into a named layout; a few remain
  (panel shells, card stacks, the constraint-capturing gathered
  panes, whose realize IS the work — each earns a reified layout
  struct when touched). A layout combinator earns its place in the
  stock set by deleting the same code twice.
- **What stays where.** Sizing stays on the THUNK (`Thunk::size` is
  the parent's placement input); visibility stays on REALIZATION;
  focus stays on VIEWS (the semantic walk) with the IME's geometry
  answered by WIDGETS (see Focus). A layout captures `&Store`
  borrows from `display` but never receives the store itself —
  passing it down is a type error, which is the whole point:
  geometry code that cannot read state cannot conflate itself with
  state management.
- **`UiCtx` rides `display`,** like the store: fonts and chrome
  typefaces are state to read at the top, not ambient context for
  arithmetic.
- **The boxing is a measured default.** `Layout::layout` returns
  `ThunkBox` — one arena box and one virtual call per node, the same
  cost class the thunk→widget boundary already pays; per-frame
  pipeline metrics stay flat under it on the heavy screens. If a
  trace ever blames it, the trait switches to the RPITIT form
  (`-> impl Thunk<'a, Command> + 'a where Self: Sized`) so erasure
  survives only where something actually boxes — the shapes are
  mechanically interconvertible.

### Destroy

A view may hold store resources — himark's `EditorIdView` holds an
editor riding a registry document. `Drop` cannot release them: view
values are clones riding persistent store snapshots, dropped by the
dozen per frame, and only the owner knows which drop is the LAST one.
So teardown is explicit, and the contract has two halves:

- **Containers forward.** Every combinator that owns child views
  (`ScrollView`, `ListView`, `SplitView`, the overlay, the erased
  boxes) passes `destroy` down, so one call at the root of a discarded
  subtree unwinds everything it holds.
- **Owners call it at every discard site** — the place that decides a
  view is gone (a modal dismissed, a slot re-pointed, rows replaced)
  calls `destroy` before dropping the value. A view merely UNMOUNTED
  to survive elsewhere is not discarded — survival paths never call
  it.

Views with resources beyond their children override `destroy`, release
their own, and forward to the children they own.

### Events

`Event` is one flat vocabulary; what differs is the ROUTING, and
containers own it:

- **Positional** (`MouseDown`, `Scroll`): containment-tested against
  each placed child, translated into its coordinates. `MouseDown`
  carries the point, button, modifiers, and the CLICK COUNT (1/2/3 —
  an editor selects the word on a double, the hard line on a triple;
  hosts that know the real count pass it through, the engine
  synthesizes it for the ones that don't).
- **Pointer continuations** (`MouseDrag`, `MouseUp`, `MouseMove`,
  `HitTest`): translated like a press but with NO containment test —
  they reach every child however far the pointer strays.
- **Broadcasts** (`Paint`, `AnimationClock`, `Settle`,
  `ThemeChanged`, `UserEvent`): forwarded to all children untouched.

**Drag ownership IS the mouse capture.** A press plants the drag
origin in the leaf that claims it (the editor arms its drag state and
takes focus); drag/up continuations are then broadcast, and only the
leaf whose own state says it owns the gesture answers — everyone
else ignores them. No capture state exists anywhere in the tree, and
a container never re-decides who is dragging. The editor extends its
selection from the pressed origin by the press's unit (char / word /
line) and arms the scroll-to-caret reveal, so dragging past the
viewport edge glides ([scroll.md](scroll.md)). `focus_scope(false)` prunes
drag/up delivery to a subtree whose pane is not focused and masks the
paint pass's `focused` bit — the residual widget-side focus gate for
panes that share a window.

`KeyDown` and `TextInput` exist in the vocabulary for hosts and
harnesses, but the app dispatch does not route them through the
widget tree — the keyboard rides the focus walk (see Focus).

`Settle` is the engine's synchronous reconcile pulse: dispatched
between a perform batch and the paint whenever a view raised the
settle bit (`Effects::settle`). Anchor-holding widgets answer with
exact-placement reveals; nothing else should react
([viewport-preservation.md](../editor/viewport-preservation.md)).

## Focus

Focus is STATE, and views are the state carriers. A window has one
focus path — the workbench knows its focused panel, a split knows its
focused pane, a document knows its focused editor — and everything
keyboard-shaped follows that path: keystrokes, typed text, the
palette's command surface, the clipboard, the IME, "what file am I
in". None of that needs geometry, so none of it builds a widget tree.

`View::focus_data` is the one walk. A composite delegates to the
child its own state says is focused — the same hand-routing idiom as
`perform` — and each level on the path contributes its own commands
and maps the child's data into its command type on the way up:

```rust
struct FocusData<'w, Command> {
    /// The palette/keymap surface — collected along the focus path,
    /// innermost first, so the keys-holder wins duplicate ids.
    commands: Vec<PresentableCommand<Command>>,
    /// Key/text handlers are COMMAND MINTERS (state mutates in
    /// perform, like every event), boxed per level — an ask happens
    /// at keystroke rate, so the handful of boxes is noise.
    on_key: Option<Box<dyn FnMut(Key, Modifiers) -> EventResult<Command> + 'w>>,
    on_text: Option<Box<dyn FnMut(&str) -> EventResult<Command> + 'w>>,
    /// A seat is a closure that builds its client on the stack
    /// (borrowing the level's innards), runs the asker's visitor
    /// against it, and answers the stashed mutation as an event
    /// result — call-locality: a ClipboardClient never outlives an
    /// ask.
    clipboard: Option<ClipboardSeat<'w, Command>>,
    /// The focused resource's identity (himark: ResourceLocation),
    /// for dock-follow and "reveal in files".
    location: Option<Box<dyn Any>>,
    /// The focused TEXT seat's identity — see below.
    seat: Option<SeatKey>,
}
```

Composition is three operations, all on `FocusData`: `map` rebuilds
into the parent's command type (`PresentableCommand::map` for the
surface, one boxed wrapper per handler); `merge_under`/`merge_over`
compose two levels — commands concatenate innermost-first, handlers
compose as fallbacks (the inner tries first, the outer on `Ignored`),
single-seat slots take the innermost `Some`. The compile-enforced
mapping events ride is the same mapping focus rides — it cannot be
forgotten or mis-typed.

The askers are all state walks:

- **Keystrokes and typed text**: the dispatch collects the chain and
  runs `on_key`/`on_text`; an unresolved chord falls back to the
  keymap, resolved against the same chain's `commands`. No widget
  tree is built for a keystroke.
- **The palette**: lists the chain's `commands` — the surface IS the
  focus path, never a parallel view-tree walk.
- **The clipboard, the focused location**: the chain's seats,
  visited in place.

**The IME is the one geometry question**, and it is answered by the
realized tree, not by the walk. `FocusData::seat` carries an opaque
`SeatKey` — a globally minted identity every text seat owns (each
editor mints one at construction). The IME ask runs the semantic walk
first to NAME the focused seat, then builds one bounded frame and
folds `Widget::layout_data(target)` over it: containers are dumb
folds that translate and clip like paint, and the editor leaf answers
its `ImeSeat` (caret origin, clip, and the ask closure) iff its own
key IS the target. The fold RECOGNIZES the key; it never re-decides
focus — the two trees share one value instead of two copies of the
routing, so they cannot diverge. The seat also rides the frame store
(himark's `FrameFocus`) so an editor bakes selection visibility in at
build time; there is no paint-time "focused upgrade".

Hit events (press, scroll) and broadcasts stay on `handle_event`;
drags keep the ownership rule on the hit side — a drag is pointer
input, not focus traffic, even though a press moves focus.

## Overlays

OUT-OF-FLOW widgets break the box model on purpose: they are ANCHORED
to a widget deep in the tree but must PAINT above everything, ESCAPE
their ancestors' clips, and PLACE themselves in some larger enclosing
region's coordinates. Popups are the obvious kind — completions,
hover cards, context menus, dropdown panels — placed by the space
available around the anchor. Editor INLAYS are the other: chrome
riding a text position that must cover the GUTTER and escape the
content's horizontal scroll — the same mechanism with the editor
pane as the host. The overlay system is that escape hatch: a request
is DATA that bubbles up with the widget (`Widget::overlays`) until a
HOST claims it.

```rust
/// Where popups land — just a key. Hosts declare one; requests name
/// one. Well-known keys are plain consts ("window" at the window
/// root; an editor pane hosts its own inlays).
struct OverlayHost(&'static str);

struct Overlay<'a, C> {
    host: OverlayHost,
    /// The ORIGIN's box. Minted widget-local (origin at zero); every
    /// owner that places the carrying widget offsets it — by the time
    /// a host sees it, the anchor is in HOST coordinates.
    anchor: Rect,
    /// The out-of-flow content itself, placement deferred until a
    /// host claims it. The one box is PER REQUEST, never per widget.
    content: Box<dyn OverlayContent<'a, C> + 'a>,
}

/// The deferred half: the host calls `layout` with ITS size and the
/// accumulated anchor, so the content decides side and extent by real
/// available space (below the anchor if it fits, above if not,
/// clamped to the host…) and answers ANY NUMBER of positioned THUNKS
/// — one for a popup, n for a batch of inlays. Thunks, not widgets:
/// the host realizes each against its own clipped viewport, so a
/// popup whose content is itself lazy (a completion list over a big
/// document) materializes only what shows. Closures are contents
/// (`FnOnce(Size, Rect) -> Vec<(Point, ThunkBox)>` gets a blanket
/// impl), so `.overlay` call sites just write a closure; the trait
/// exists so the type has a NAME and `.map` can wrap it the way it
/// wraps widgets. `host` and `anchor` stay plain fields on purpose:
/// hosts partition by key and owners offset anchors — bookkeeping on
/// data, not virtual calls.
trait OverlayContent<'a, C> {
    fn layout(self: Box<Self>, arena: &'a Arena, host_size: Size, anchor: Rect)
        -> Vec<(Point, ThunkBox<'a, C>)>;
}
```

### The anchor contract

`.overlay(host, layout)` attaches a request with
`anchor = Rect::from_size(self.size())` — its own box, self-relative;
a lazy realize attaches requests to the widget it builds the same way
(the editor: one per VISIBLE inlay). From there the INVARIANT is:
**whoever transforms a child's coordinates transforms its overlay
anchors identically.** The drain runs at REALIZATION: a container's
`realize` realizes each placed child against its clipped viewport,
drains the child widget's `overlays()` on the spot, offsets each
anchor by the child's placement, and appends them to its own pending
vec (`Vec::append` — ownership transfer, no clones; empty vecs cost
nothing); the realized container's own `overlays()` hands that
accumulation up. A scroll's realize offsets by the scroll, so anchors
ride it for free. Wrapper widgets (`.map`, the paint and focus
combinators) delegate `overlays()` to their inner widget — `.map`
additionally maps each drained overlay, wrapping its content so the
popup thunks it will produce are mapped too. That recursion is why a
request minted deep inside a plugin's subtree crosses every command
boundary the subtree itself crosses: when its popup later produces
commands, they arrive through EXACTLY the scope chain the origin view
already speaks. No new routing concept.

Clips never clip anchors — a popup anchored to a half-scrolled-away
row is the layout closure's judgment call, at host coordinates, with
the host's size in hand.

### Hosts

`thunk.overlay_host(key)` — a thunk combinator, sizing-transparent
(`size` = the child thunk's). Resolution runs inside ITS `realize`,
where BOTH deciders are finally in hand: the host's size for
placement, the viewport for realizing what it places:

1. realize the child with the host's viewport; drain its overlays;
   partition by key — foreign keys go on the realized wrapper's own
   pending vec and keep bubbling (their anchors are already in this
   widget's coordinates, and owners above keep offsetting them);
2. resolve matching requests as a WORKLIST: call
   `content.layout(host_size, anchor)`, realize each answered thunk
   against the host viewport clipped to its rect (placement point +
   thunk size), keep the widgets — and drain each of THEIR overlays
   in turn. Same key → offset anchors by the placement point, push
   back on the worklist (this is the recursion: a menu opens a
   submenu into the same host); other keys → bubble (a popup can ask
   an outer host for something bigger than this one). The worklist
   terminates like any immediate-mode tree does — you get out what
   this frame's realization materialized;
3. the wrapper keeps the placed widgets in resolution order and
   routes:
   - **paint**: base first, then placed widgets in order (later = on
     top), each translated and clipped to the HOST's bounds — this is
     the clip escape: a popup clips to the host, never to its
     origin's ancestors;
   - **positional events** (press, scroll): placed widgets
     topmost-first with containment (`blocks_pointer` decides whether
     a covered point stops there), then the base — a popup shadows
     what it covers;
   - **broadcasts** (clock, theme, settle): base and overlays,
     merged;
   - reveal rects from placed widgets translate by their placement,
     like any child's.

### Scale

Overlays are NOT rare: recursion multiplies them, and inlays make
them steady-state bulk — an editor can mint dozens per frame, every
frame. The costs, exactly:

- **per request**: one `content` box and one `Vec` slot — that is
  all;
- **bubbling**: `Vec::append` moves the small `Overlay` records (a
  key, a rect, a pointer); contents are never touched, nothing
  re-allocates per level;
- **`.map`**: the one per-BOUNDARY cost — each command boundary an
  overlay crosses re-boxes its content once.

Three rules keep that flat at bulk:

- **Host locality.** Declare hosts CLOSE to bulk producers: the
  editor pane hosts its own inlays, so they resolve one level up and
  never ride the deep `.map` chain to the window. Only genuinely
  window-level popups travel far, and those are singular.
- **Batching.** A bulk producer mints ONE request answering all n
  inlays as n positioned thunks — the anchor hands its content
  origin in host coordinates, and it knows every inlay's position
  from the same layout that minted the anchor. n inlays, one request,
  one content box.
- **Visibility is free.** Requests are minted at realization, and
  realization is viewport-driven — the editor mints requests for the
  VISIBLE inlays only, because the off-screen ones were never
  materialized. Nothing is filtered; nothing off-screen is paid for.

### What overlays are NOT

Whether an overlay EXISTS is view state, exactly like everything else
in this file — a completion list open is a field the editor view lays
out from, an inlay derives from the document; dismissal is a command;
the overlay system only solves placement, stacking, clipping and
routing for one frame. There is no retained popup tree, no imperative
open/close API, and no z-index arithmetic — order is resolution
order, and "above" is a property of being an overlay at all.

`imba::stack::Stack` — the base+modal VIEW at the window root — is a
different thing: a modal owns the whole screen and needs no anchor.

## Commands

A `Command` is a request for a state transition. `Widget`s build commands in
response to raw UI events; commands are event *outputs*, routed and performed
by value. One event is handled like this:

1. the event bubbles down the transient `Widget` tree;
2. out comes a root-bound (app-level) `Command` — each parent widget mapped
   its child's command type into its own on the way up;
3. the root `View` performs it: handle it, or unwrap one layer and route it
   to a child view — which does the same. Routing is the *view's* job; a
   command carries payload, never a route.

`perform` receives the store mutably. It mutates its own transient state
freely and writes entity novelty straight into the store — later readers in
the same event see earlier writes (read-your-writes), which is what lets one
event act on several projections of one document without losing updates.
The host hands the root `perform` a working copy (an O(1) clone) and
publishes it when the event completes: **one event, one publish.**

The command SURFACE — what the palette lists and the keymap resolves
against — is the focus walk's `FocusData` (see Focus above), never a
parallel view-tree walk.

Commands that reference entities do so the way entities reference each
other — by id in the *payload*, resolved against whatever the receiving view
gathers. If the gather no longer contains the target (a pane closed, an
entity re-pointed), the command discards itself. Identity is never
complected with the value that moves.

## Store

The store is a CATALOG OF SINGLETONS: one value per type
(`TypeId → Component`), each a domain-owning repository
(`OpenDocuments`, `Windows`, the command registries,
themes/fonts/parsers). The store mints no ids and holds no per-entity
rows; identity lives inside the repositories as domain-minted keys
(`DocumentId`, `WindowId`, `SessionId`) beside the records they
describe — values carry no identity of their own, and ids appear only
as repository keys and as the references records hold about each
other ([documents.md](../editor/documents.md)).

The store a dispatch receives is DERIVED — a PROJECTION of the
persistent `AppState` value, gathered per (window, session): the
globals and registries as-is, the hosts forest, ONE window's entity,
and the scope session's families flat as their own component types
([app-state.md](app-state.md)). Scatter writes it back at commit. Everything
below — the two-`&mut` story, snapshots, effects — describes the
store's mechanics regardless of where the store comes from.

### The problem it solved: two `&mut`

The store exists because of one concrete problem. Two panes project the
same document: type in the left pane and the right pane must show the edit.
In an ownership tree, that document has no home — whichever view owns it,
the other needs `&mut` through a path the borrow checker (correctly) will
not grant, and every workaround is one of the usual suspects: `Rc<RefCell>`
aliasing with runtime panics, message-passing ceremony, or a God-object
holding everything and routing every touch. The tension is fundamental:
*two views need mutable access to one value, and neither should own it.*

The store dissolves it by making ownership one-place and access value-wise:

- the **repositories own every entity**; views hold ids;
- a view **gathers** the value on demand — a clone of a persistent value is
  a pointer bump — acts on its own copy, and **scatters** the result back
  with `store.put`. Nobody holds a long-lived reference, so nobody aliases;
- an editing gather takes *the document and every projection of it* in one
  scoop, so one keystroke updates both panes' state in a single pass — the
  "other pane" is not a special case, it is just more of the gather;
- everyone in a pass reads the **same immutable snapshot** — rendering,
  hit-testing and background captures can never observe a half-applied
  mutation.

### Mutation and snapshots

Mutation is direct: `get::<T>()` reads the component, `put` installs
the next value, `update::<T>` is the clone-or-default/mutate/put
pattern repositories build their domain methods on (a document
registers through `OpenDocuments::register`, which mints the id and
puts the grown map) — all effective immediately. There is no
transaction object; an event's atomicity comes from the dispatch,
which runs the event against a **working copy** (the gathered
projection — O(1) clones of persistent values) and scatters it into
the `AppState` value when the event completes — dropping the copy
instead aborts without a trace.

Snapshots do the rest: because the store is persistent, a clone taken at
any moment is a value that never changes, so anything captured from one —
an effect's input, a render pass — stays internally consistent forever.
That property is what the whole effect system leans on: capturing "the
document as of revision N" is a clone, not a lock.


## Effects

`perform` must stay cheap — the budget is logarithmic work on the UI loop.
Anything heavier (layout repair over megabytes, a reparse, a file parse, a
question only the host can answer) becomes an **effect**: a declared piece
of data describing deferred work, launched from `perform` instead of being
done in it. The system's own story — batch commit, lanes, the runner —
is `docs/ui/effects.md`; here is the view-side contract.

```rust
/// An effect: a declared piece of data. No behavior — the handler
/// registered for this TYPE interprets it.
pub trait Effect: Send + Sync + 'static {
    type Result;
}

/// The handler for ONE effect type — defined right next to its effect,
/// holding its own state explicitly. Plain async: in-process compute
/// completes on its first poll; an external handler awaits its own
/// completion. Never used as a trait object — erasure happens at
/// registration, where the concrete type is known.
pub trait EffectHandler<E: Effect>: Send + Sync + 'static {
    async fn handle(&self, effect: E) -> E::Result;
}

/// The builder `perform` receives: launches mint cancellation tokens,
/// cancels ride the same transactional batch, scopes map child command
/// types to the parent's, `settle` raises the reconcile-pulse bit.
pub struct Effects<'a, R> { /* a writer over the root's Batch<R> */ }
```

`perform` returns nothing; it pushes into the builder. `fx.launch(effect)`
records a launch and hands back a `CancellationToken` — an inert `Copy`
id a view or entity stores if (and only if) it owns a supersession lane.
`fx.cancel(token)` records a cancel; `fx.relaunch(&mut slot, effect)` is
the lane idiom (cancel the stored token, launch, store the new one).
`fx.settle()` asks the engine for one synchronous `Settle` pulse before
the next paint ([viewport-preservation.md](../editor/viewport-preservation.md)). Everything a perform
pushes accumulates in the root's `Batch` and commits transactionally
after the store publishes — a dropped batch launches and cancels nothing.

- **An effect is pure data.** It captures persistent values (documents,
  layouts, texts) at creation; there is nothing to run *on* it — no store,
  no transaction, no ids-for-lookup, and no behavior of its own. It is
  **dispatched, by type, to the one handler registered for that type** —
  and it just so happens that some handlers are external (the host
  process, a browser sandbox, a remote peer). Nothing in the pipeline
  distinguishes the cases; see `docs/ui/effects.md` and `docs/ahp/host.md`
  for the external half.
- **Handlers hold their state explicitly.** What the work needs beyond
  the effect's own capture is the handler's business, held in its fields:
  the in-process editor handlers share a `Workshop` (the lazily
  materialized font collection and the LIVE theme — a theme switch writes
  the slot, every later handle reads the current one); an external
  handler holds its line home. There is no ambient context and no memo
  map — state is constructor-domain wiring, visible at the registration
  site.
- **Its outcome comes home as a command.** `handle` returns the effect's
  `Result`; the lift chain the tree built turns it into a command in the
  root's own type, and the host posts it to the platform's main loop,
  where it is applied exactly like an event command: perform → publish →
  launch. There is no second delivery mechanism — deferred results
  re-enter through the front door.
- **Both faces are routed by the tree.** An effect travels as `AnyEffect`,
  the erased transport: typed rims, erased middle. The payload stays the
  concrete effect value end to end (the one downcast happens under the
  dispatch registry's `TypeId` guarantee); the result side carries a
  composable lift. A parent does not touch effects one by one — it opens a
  **scope** that maps every launch inside to its own command type, exactly
  as widgets wrap child commands:

  ```rust
  fx.scope(SplitCommand::First, |fx| self.first.perform(store, ui, command, fx))
  ```

  Tokens ride the lift untouched: the token minted at the leaf is the
  token the root batch records, so whoever stored it can cancel it later
  from anywhere in the tree.
- **Cancellation drives supersession, not correctness.** Every launch
  runs and lands unless its owner cancels it first — the engine never
  drops or merges work on its own. A lane owner (a document's reparse, a diff's pair
  repair, an entity's save) stores the in-flight token and `relaunch`es
  when newer work supersedes it. Cancel is best-effort: it stops a queued
  effect from starting and drops a parked future, but a result already
  dispatched lands — the receiving view's guards decide, as ever.

### What happens when a result comes home late

The world may have moved while the effect ran. The receiving view has three
sound options, chosen per effect type by cost:

- **Discard** (cheap-to-redo work): layout repairs carry the revision,
  markup generation and width they were computed against; any mismatch drops
  the arrival. This never strands work — whatever moved the world also left
  a fresher effect in flight behind it.
- **Rebase** (expensive work worth salvaging): a reparse outcome shifts its
  ranges, its complete markup tree and its syntax tree by the composition of
  the document's edit log since capture (`ops_since`), then lands as a swap.
  Text operations compose; markup does not rebase over markup — that is
  exactly the line between the two strategies.
- **Replay-on-top** (commuting operations): reparse *adds* damage ranges,
  repairs only *remove or shift* them — so a late reparse marks its
  (rebased) ranges over whatever repairs already healed, and the worst case
  is a range repairing twice, once under each markup.

### Dispatch

Registration is constructor-domain wiring: `AppState::register_handler`
captures the concrete handler into a typed thunk under the effect's
`TypeId` — one effect type, one handler; plugins and hosts register
theirs beside their commands. Dispatching an unregistered type is a dev
panic (a release build drops it loudly).

The queue drains on ONE worker thread the host supplies (himark spawns
nothing; see `docs/ui/effects.md`). The runner drives handle-futures with a
small poll set: an in-process compute handler completes on its first
poll, bit for bit the synchronous work it used to be; an external
handler's future parks until its completion arrives from outside, and a
parked handle never blocks the queue — the next queued effect dispatches
right past it. The channel from the UI thread is single-producer FIFO of
launches and cancels in committed program order: a cancel always trails
its launch, so the runner just deletes the queued entry (or drops the
parked future) it names; a cancel matching nothing means the work already
completed and is a no-op. On wasm without threads the same dispatch runs
inline between the commit and the next frame, cancel-wins within each
commit.

### The loop, end to end

```
event ──▶ widget tree ──▶ Command ──▶ perform(&mut store, fx) ─▶ Batch
                                            │                    │
                                        publish              launch + cancel
                                            │                 (program order)
                                       next snapshot             │
                                                     dispatch by TypeId to the
                                                     registered handler; the
                                                     handle-future polls on the
                                                     host's worker (external ones
                                                     park until fulfilled)
                                                                 │
        apply(command) ◀── main queue ◀── lift(Result) ──────────┘
             │
   perform → publish → launch   (same path as an event; may defer again)
```

Frames are decoupled from all of this: events and landings mark
`needs_redraw` and unpause the display link; the next vsync renders one
coalesced frame. Nothing on the loop ever waits for the GPU.

## API

### Paint: layout reports back

Constraints flow down at layout; paint is the return path. The paint pass
walks the freshly realized widget tree with the real viewport (broadcast with
merge, like the animation clock), so a widget whose view state disagrees
with the geometry it was just granted — an editor laid out at a stale
width, a list row whose cached height went stale — answers with the command
that reconciles it; a settled widget paints silently. Geometry
reconciliation therefore lives in the tree and composes like everything
else: no out-of-band walks comparing sizes, no duplicated layout
arithmetic, and no second traversal — the pass that paints is the pass
that reports.

One paint pass runs per frame — never a loop within one. Cascades settle
over consecutive frames (a resize this frame moves a height next frame),
which is why a reconciling frame must *report* itself: `draw` returns
`true` when the paint pass issued commands, and the host schedules one
more redraw. The chain terminates because a settled tree answers with
silence; a paint pass that stays noisy for hundreds of frames trips a
diagnostic — two views are fighting over one entity's geometry.

### The frame, exactly

Every dispatch — an input event, an effect landing, a clock tick, a mount
pass — is the same cycle: **display** the view tree (state read, the
layout tree named), **lay it out** (constraints down, sizes back),
**realize** it against the root viewport (lazy nodes materialize their
visible content; overlay hosts resolve), route the event through the
realized tree, **perform** the resulting commands against a working store
(read-your-writes), run the **settle pulse** if any perform raised it,
**publish** the store, **launch** the returned effects. One dispatch, one
publish. Both `display` and `perform` also receive the app-owned
`UiCtx` — the UI thread's once-materialized, `!Send` resources (the font
collection, the chrome typeface), constructed at startup and borrowed
everywhere; effect handlers hold the worker-side equivalent explicitly
(the `Workshop` they were constructed with). Nothing expensive is ambient
and nothing expensive constructs per frame or per keystroke.

An input event or a landed effect result runs that cycle immediately and
marks the host's `needs_redraw`. The frame itself, on the host's
display-link tick, is two dispatches in a fixed order:

1. **AnimationClock** (host tick, before render): running animations answer
   with their advance commands; perform → publish → launch. A responding
   tick keeps the display link running.
2. **Paint** (inside `draw`): the tree built from the *post-animation*
   store paints and reports geometry mismatches in the same traversal; its
   commands perform → publish → launch. This is where resizes land and
   list heights splice — the frame shows them settling on the *next*
   frame, which the report schedules.

`draw` returns whether the paint pass reconciled anything. The next frame
is scheduled by exactly four things:

- an input event that was handled (`needs_redraw`);
- an effect result draining in (`needs_redraw`);
- `draw` returning `true` — reconciliation cascades until a silent paint;
- a running animation (the clock tick answering keeps the link unpaused).

A fully settled, idle UI does none of these: the link pauses and no frames
render — silence is the scheduler.

`Container` spans the lower stages: its thunk half places child
THUNKS by their sizes; its `realize` realizes each against the
clipped viewport and hoists the children's overlay requests, anchors
offset by the placements (see Overlays); the realized half routes
z-ordered as before. Above it, `Column`/`Row` and the layout
combinators are the LAYOUT-stage compositors (see Layouts) — they own
the placement arithmetic and answer container thunks. The child list
is part of the transient frame artifacts; nothing is retained between
passes. `Leaf` draws itself and handles hit events. `ScrollView`,
`SplitView`, `Stack` are the stock view-level compositors — each routes
commands down and scopes effects up. `DynView` erases a view's command
type for heterogeneous containers; its blanket impl scopes launches into
the boxed command type, tokens untouched.

## Example

A split of two independent views: commands route down, effect scopes map
launches up, tokens ride unchanged.

```rust
struct SplitView<First, Second> {
    first: First,
    second: Second,
    ratio: f32,
}

enum SplitCommand<F, S> {
    First(F),
    Second(S),
    ChangeRatio(f32),
}

impl<First: View, Second: View> View for SplitView<First, Second> {
    type Command = SplitCommand<First::Command, Second::Command>;

    fn perform(
        &mut self,
        store: &mut Store,
        ui: &UiCtx,
        command: Self::Command,
        fx: &mut Effects<'_, Self::Command>,
    ) {
        match command {
            SplitCommand::First(command) => {
                fx.scope(SplitCommand::First, |fx| self.first.perform(store, ui, command, fx))
            }
            SplitCommand::Second(command) => /* symmetric */,
            SplitCommand::ChangeRatio(ratio) => self.ratio = ratio,
        }
    }

    fn display<'a>(&'a self, arena: &'a Arena, store: &'a Store, ui: &'a UiCtx)
        -> impl Layout<'a, Self::Command> + LayoutValue + 'a
    {
        // State read HERE (the ratio, the children's displays);
        // geometry NOT — the returned Row owns the split arithmetic.
        Row::new(arena)
            .weighted(self.ratio, self.first.display(arena, store, ui).map(SplitCommand::First))
            .weighted(1.0 - self.ratio, self.second.display(arena, store, ui).map(SplitCommand::Second))
        // Sizing runs when the frame lays the root out (constraints
        // down, sizes back); realization — materializing each child
        // against its clipped viewport, hoisting overlay requests —
        // when the frame realizes the root.
    }

    fn focus_data<'w>(&'w self, store: &'w Store, ui: &'w UiCtx)
        -> FocusData<'w, Self::Command>
    {
        // Focus is state: delegate to the pane our state says is
        // focused, mapped like any child output.
        match self.focused {
            Side::First => self.first.focus_data(store, ui).map(SplitCommand::First),
            Side::Second => self.second.focus_data(store, ui).map(SplitCommand::Second),
        }
    }
}
```

The same shape scales to the app root: the demo's `AppState` is a `View`
whose command type tops the hierarchy (`AppCommand`), whose
`perform` routes to the workbench tree or the peeker overlay, and whose
host loop is nothing but *dispatch, publish, launch* — for events and for
homecoming effects alike.
