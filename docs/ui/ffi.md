# The imba FFI: bridging single views

The embedding contract we ship today
([embedding-api.md](../editor/embedding-api.md)) hands the host one
opaque engine that owns the whole window: the host gives it a canvas,
a viewport, and events, and the workbench composes everything inside
— docks, splits, panels, toolbars. That is the right contract for a
shell that wants "the app in a rectangle." It is the wrong one for a
host that wants to OWN the layout: put the changes tree in its own
native sidebar, the chat in a native drawer, a diff canvas in a
native tab — native components between them, native splitters around
them.

This document layers the FFI to allow exactly that. **Beneath the
app-level engine API sits an imba-level API: any imba view — not
only the app as a whole — can be mounted by the host as a bridge and
driven with the same handed-the-world discipline the engine already
follows.** The engine remains the owner of the world (`Store`,
`UiCtx`, effects, the wake); the host becomes the owner of the frame
— which views exist on screen, where, and in what native company.

**The end game is the iOS app rewrite: native touch and Liquid
Glass controls around bridged content.** The current iOS shell
(`apps/himark-apple/Sources/iOS`) imitates the platform by hand — a
`UIPanGestureRecognizer` feeds synthetic scroll deltas into imba's
own scrollers, momentum is a hand-rolled "glide" decay loop in the
display-link tick, taps become mouse downs, and every piece of
chrome is imba-drawn. The rewrite inverts that: `UIScrollView` owns
scrolling (real physics, rubber-banding, indicators, keyboard
avoidance), native Liquid Glass bars, sheets, and drawers own the
chrome, and bridges draw the content slices between them. Every
design point below that smells scroll-shaped — `measure`, the
`draw` viewport, reveal rects in content coordinates — exists to
make that inversion possible.

Three new object kinds cross the boundary, all opaque:

```
ImbaEvent    — an owned mirror of imba::Event, minted by factories
ViewBridge   — one mounted view: state carrier + event handler
ImbaCommand  — an opaque boxed command, in flight between the two
```

## Events: `ImbaEvent` factories

`imba::Event<'_>` is a borrowing enum — it cannot cross C as-is. The
FFI mirrors it with an owned `ImbaEvent`, one factory per variant,
plus one destructor. The mirror follows `frontend/imba/src/event.rs`
as closely as the ABI allows:

```
imba_event_mouse_down(x, y, button, mods, count) -> ImbaEvent*
imba_event_mouse_drag(x, y, mods)                -> ImbaEvent*
imba_event_mouse_up(x, y)                        -> ImbaEvent*
imba_event_mouse_move(x, y)                      -> ImbaEvent*
imba_event_hit_test(x, y, miss)                  -> ImbaEvent*
imba_event_scroll(x, y, dx, dy)                  -> ImbaEvent*
imba_event_text_input(utf8, len)                 -> ImbaEvent*
imba_event_key_down(key, mods)                   -> ImbaEvent*
imba_event_animation_clock(now_ms)               -> ImbaEvent*
imba_event_settle()                              -> ImbaEvent*
imba_event_theme_changed()                       -> ImbaEvent*
imba_event_window_left()                         -> ImbaEvent*

imba_free_event(ImbaEvent*)
```

An event is minted once and may be dispatched to ANY number of
bridges before it is freed — that is the point of reifying it. A
host that routes one `NSEvent` across three bridged views builds one
`ImbaEvent`, hands it to each, and frees it; per-bridge flattened
argument lists would triple the marshalling and drift from the enum.
Dispatch borrows; `imba_free_event` is the only owner-side call.

What deliberately does NOT get a factory:

- **`Paint`** — the canvas cannot be carried in a value. Painting is
  the bridge's own `draw` entry (below), which synthesizes
  `Event::Paint` around the canvas the host hands it, exactly as the
  engine's `draw` does today.
- **`UserEvent(&dyn Any)`** — a Rust-side channel; not
  representable, not needed by hosts.
- **`Scroll`'s `gesture`** — `ScrollGesture` is claim-tracking state
  (who owns the momentum), not event payload. Each bridge keeps its
  own tracker and supplies it at dispatch, the way the engine keeps
  `scroll_gesture` per engine today. The factory carries only the
  point and deltas.

`imba_event_window_left()` is the cursor-exit miss — sugar for
`hit_test(-1e6, -1e6, miss: true)`, mirroring `Event::window_left()`
so hosts don't re-invent the constant.

## `ViewBridge`: one mounted view

imba renders in three transient curries — `View --display-->
Layout --layout--> Thunk --realize--> Widget` ([UI.md](UI.md)) —
where the `View` is the only retained thing and the `Widget` is what
routes one dispatch. A host cannot be asked to drive four stages
with arena lifetimes across C. **The `ViewBridge` is the collapse of
that pipeline behind one handle: it retains the view (the state
carrier) and runs display/layout/realize internally on every
dispatch, so from the host's side it consumes events and returns
commands.** It is `View` and `Widget` combined, which is precisely
what the engine's own per-window dispatch does internally
(`Application::dispatch_event`: layout, realize, `handle_event`,
then perform) — the bridge is that loop, scoped to one view.

The erasure already exists in imba: `DynView` (`dyn_view.rs`) wraps
any `View` whose command is `Send + Sync + 'static` behind
`perform_dyn` / `layout_dyn` / `destroy_dyn` / `focus_data_dyn`,
carrying `DynCommand = Box<dyn Any + Send + Sync>`. The workbench
ships plugin panels this way today (`Panel::Plugin(Box<dyn
DynPanelView>)`). The bridge is a `DynView` plus the engine context
it needs to run:

```
// minting — the app layer hands views out (next section)
view_bridge_destroy(ViewBridge*)                    // destroy_dyn + release

// the frame
view_bridge_measure(ViewBridge*, w, h)              -> content_size
view_bridge_draw(ViewBridge*, canvas,
                 w, h, viewport, scale)             -> needs_redraw
view_bridge_event(ViewBridge*, ImbaEvent*)          -> BridgeAnswer

// commands
view_bridge_perform(ViewBridge*, ImbaCommand*)      -> needs_redraw
view_bridge_pump(ViewBridge*)                       -> ImbaCommand*  // or null

// focus
view_bridge_set_focused(ViewBridge*, bool)
imba_free_command(ImbaCommand*)
```

- **`draw` takes the layout size AND a viewport — that is what lets
  a bridge live inside a native scroller.** The two are distinct on
  purpose: `w × h` is the size the view is laid out at (the
  document), the viewport is the visible rect of it in content
  coordinates (the porthole). imba is already built exactly this
  way — realize is the viewport-dependent curry (`Thunk::realize
  (arena, viewport)`, [UI.md](UI.md)): the thunk is sized for the
  whole content, but only what the viewport shows is materialized
  (a `ListView` realizes only the rows in view). A whole-window
  host passes `viewport = (0, 0, w, h)` and nothing changes; a
  native-scroll host lays the bridge out at its content size,
  realizes the visible slice, and pays for the slice.
- **`measure`** answers the other half of native scrolling: the
  host must size its scroller's document before it can scroll it.
  Measure runs layout at the given constraints (a fixed width and
  an unbounded height, typically) and returns the content size —
  `Thunk::size()`, no realize, no paint. Reveal rects (below) are
  in the same content coordinates, so the answer to "a deep view
  wants to be seen" is precisely "scroll your native scroller to
  this rect of the document."
- **Event points arrive in content coordinates too**; the host
  translates from its scroller exactly as it translates for draw.
  The bridge realizes event dispatch at the last-drawn layout size
  and viewport — between a native scroll and its next paint the
  pointer is inside the last viewport by construction.
- The host owns the surface, the scroller, and the compositor — the
  bridge, like the engine, is handed canvas, size, and viewport per
  frame and retains only the numbers it needs to route the next
  event.
- **`event`** runs the same realize-and-route pass and returns a
  `BridgeAnswer`: the `EventResult` made ABI-shaped —
  `needs_redraw`, zero or more `ImbaCommand*`s (from
  `Command`/`Commands`), and an optional **reveal rect**
  (`EventResult::Reveal`: a deep view asking to be scrolled into
  view). Inside the engine a `ScrollView` ancestor absorbs reveals;
  in a bridge whose scrolling may be NATIVE, the reveal must surface
  to the host — it is the host's scroller that has to move.
- **`perform`** consumes an `ImbaCommand`, calling `perform_dyn`
  with the engine's store and `UiCtx` and an effects scope. Commands
  returned from `event` belong to the host until it either performs
  or frees them — returning them rather than performing inline is
  what lets the host batch, defer to its own run loop, or drop a
  command whose native context has meanwhile closed.
- **`pump`** is the other half of the effects contract. Effects a
  `perform` launches run on the host's thread via the existing
  `run_pending`/wake road (embedding-api.md); their landings are
  commands addressed to this view. The engine's wake fires as today;
  the host then pumps each bridge, receiving landed commands over
  the same opaque type and feeding them back through `perform`. One
  command road, both directions.
- **`MouseMove` synthesizes its `HitTest`** inside the bridge
  (mirroring `Application::dispatch`), so hover arms without the
  host knowing the protocol. The host's remaining hover duty is the
  claim rule the window layers play today (`window.rs` route): the
  bridge under the pointer gets the real move; every other bridge
  gets `hit_test(miss: true)`; on leaving the window, all of them
  get `imba_event_window_left()`.
- **Focus is the host's call.** Native focus arbitration decides
  which bridge holds the keyboard; `set_focused` flips the bridge's
  seat and the host routes `key_down`/`text_input` events to the
  focused bridge only. Inside, the bridge walks `focus_data_dyn` —
  the semantic chain, no tree built for a keystroke — exactly as
  window dispatch does.

## `ImbaCommand`: opaque, owned, one-shot

An `ImbaCommand` is a boxed `DynCommand` in flight. Ownership is
linear: minted by `event` or `pump`, it is consumed by exactly one
`perform` or released by `imba_free_command`. It is bound to the
view family that minted it — performing a command at a foreign
bridge is a checked error (each command carries its bridge's brand),
because `DynCommand` downcasts by type and a wrong-view downcast is
only a `debug_assert` today — a release build drops the command on
the floor. The FFI turns that into a refusal the host can see.

## Where views come from: the workbench hands them out

The imba layer does not invent views; **the app layer mints bridges
for views it already owns.** The workbench keeps being the factory
and the registry — sessions own state, panels are built against a
window's store (`frame_store(window)` — stores are per-window, so a
bridge is minted against an engine AND a window and shares that
window's store, `UiCtx`, theme, and fonts):

```
himark_bridge_panel(engine, window, panel_id) -> ViewBridge*
```

Claiming a view is a MOVE, not a copy: a panel mounted by the host
leaves the window's own composition (the workbench no longer draws
it in a dock or split), and `view_bridge_destroy` runs the panel's
teardown (`dismantle` for plugin panels) or returns it to the
workbench. One view, one owner, one painter — two painters of one
`View` would double-perform its commands.

The engine-level API stays for whole-window hosts; nothing existing
moves. The layering is strictly downward: the app FFI is a client of
the imba FFI's types, never the reverse.

## Touch platforms

Touch needs no touch events. The host recognizes gestures natively
(`UITapGestureRecognizer` and kin) and hands the bridge the mouse
roads they resolve to:

- **A tap is a `mouse_down` + `mouse_up`** at the point, `count`
  carrying double-taps — the translation the iOS shell already does
  today, minus the parts that stop existing (below). Selection
  drags ride `mouse_drag`.
- **Scrolling never enters the bridge.** A pan belongs to the
  native `UIScrollView`, which moves `draw`'s viewport; the
  synthetic `scroll` deltas, the hand-rolled glide loop, and the
  imba scroller wrapping the content all retire. The `scroll`
  factory stays for pointer platforms — and for the iPad trackpad,
  where it means a real wheel/trackpad gesture again.
- **No hover, nothing to clear.** Pure touch mints no
  `mouse_move`/`hit_test` stream, so hover popups never arm and
  `window_left` has no work. When iPadOS attaches a pointer, the
  hover roads apply as on desktop.
- **Text input keeps its road**: the shell's `UITextInput`
  integration points at the focused bridge — the marked-text trio
  and caret-rect probe from the IME decision, nothing extra.

## Rules of the road

- **One thread.** Bridges live on the same host main thread as the
  engine; every call above is main-thread, like the whole existing
  surface.
- **The engine still owns time.** `animation_clock` fan-out is the
  host's job (its display link ticks every bridge that animates);
  `settle` is dispatched by the bridge itself between a perform
  batch and the next paint when a perform raised the settle bit
  (`Effects::settle`) — hosts never mint it, the factory exists for
  symmetry and tests.
- **Overlays stay inside.** Tooltips and popups a widget mints
  (`Widget::overlays`) composite within the bridge's canvas, clipped
  to its viewport. An overlay that should escape into native chrome
  is a different feature (a host-side popup contract) — out of scope
  here.

## Decisions

- **A reveal is a bare rect.** Content coordinates — the same space
  `measure` sizes and `draw`'s viewport cuts — and nothing more. The
  `Placement`/`Motion` detail `Reveal` carries stays inside the
  bridge, where its own scrollers already honor it; a native
  scroller handed the rect brings it into view however it likes.
  Revisit only if a host ever wants the engine's placement idioms
  (golden-ratio aim, animated motion) natively.

- **IME rides the focused bridge.** The problem, plainly: an input
  method (dead keys, CJK composition) needs two things from the
  app — a caret rectangle to hang its candidate window on, and a
  place to hold the not-yet-committed ("marked") text as it
  composes. The engine already answers both per window
  (`himark_set_marked_text`, `himark_marked_range`,
  `himark_has_marked_text`); the bridge FFI repeats that trio per
  bridge, plus a caret-rect probe, and the host points the OS input
  context at whichever bridge it has focused. Same roads as
  [ime.md](ime.md), scoped to a bridge — no new machinery.

- **`PanelRequest`s route by a flag; the workbench's perform is
  exposed.** A mounted panel still raises requests (open a file,
  navigate). Minting takes a routing flag: a ROUTED bridge hands its
  requests straight to the workbench, which answers them exactly as
  it does for its own panels; an UNROUTED bridge surfaces them on
  `pump`, and the exposed workbench perform lets the host hand any
  of them back after looking. Both roads end in the same place — the
  flag only decides whether the host stands in the middle.
