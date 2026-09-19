# Navigation: history per workbench pane

Back/forward on each workbench pane, the undo-log shape: two persistent
stacks riding the pane, recording WHERE the pane was — an editor at a
caret, a diff pair, a search — so cmd-[ walks back through real places,
not just files.

## NavigationLocation

What the stacks store. Distinct from [`ResourceLocation`]
([files.md](../ahp/files.md)) on purpose: a resource names a thing in the world; a
navigation location names a PLACE IN A VIEW — it may carry
view-internal state (caret byte, scroll offset, a diff's side) that no
resource identity should.

TYPE-SAFE, the effects pattern ([effects.md](effects.md)): there is no kind
string — **the payload's type IS the kind**, dispatch is by `TypeId`,
and the one downcast lives in the registry plumbing. Declaring a place
kind is declaring a type:

```rust
/// A navigable place — one implementing type per kind.
pub trait Place: Clone + Send + Sync + 'static {}

/// The engine's built-in:
#[derive(Clone)]
pub struct EditorPlace { location: ResourceLocation, caret: u32, scroll_y: f32 }
impl Place for EditorPlace {}

/// The type-erased carrier the stacks store — typed at both ends,
/// exactly like `AnyEffect`.
pub struct NavigationLocation { /* TypeId + Arc<dyn Any + Send + Sync> */ }
impl NavigationLocation {
    pub fn new<P: Place>(place: P) -> Self;
    pub fn place<P: Place>(&self) -> Option<&P>;   // None = not that kind
}
```

## The traits

```rust
/// One per place type. Registration erases it (the EffectHandler
/// recipe): `Navigators` keys `TypeId::of::<N::Place>()` to a closure
/// that downcasts and calls the TYPED navigate — a navigator can never
/// receive the wrong payload, and a mismatched register cannot compile.
pub trait Navigator: Send + Sync + 'static {
    type Place: Place;
    /// Resolve the place into pane content — fetching/registering
    /// whatever that takes. `window` is where the answer lands — what
    /// a miss needs to launch the standard open pipeline against.
    fn navigate(&self, store, window: WindowId, place: &Self::Place, fx) -> Option<Panel>;
}

Navigators::register(store, EditorNavigator);          // infers Place = EditorPlace
Navigators::navigate(store, &location, fx) -> Option<Panel>   // dispatches by the carrier's TypeId
```

There is no separate "navigation view" — **the panel IS the navigation
view**. `PanelView` itself goes typed: it gains the place type and the
two hooks beside title/dismantle/take_request; what the workbench BOXES
is the blanket-erased companion (the split `CloneDynView` already does
for cloning — one more erased face, not a new concept):

```rust
pub trait PanelView: … {
    type Place: Place;
    // …title, dismantle, take_request, as_any…

    /// The pane's CURRENT place — asked right before this panel is
    /// displaced (or updated in place), recorded onto the back stack.
    /// `None` = nothing to record right now (an empty state).
    fn navigation_location(&self, store) -> Option<Self::Place>;
    /// In-place navigation: take the target WITHOUT being replaced —
    /// an editor pane already showing the place's file moves its
    /// caret and reveals; `false` = displace me.
    fn navigate_to(&mut self, store, place: &Self::Place, fx) -> bool;
}

/// What `Panel::Plugin` boxes — blanket-implemented over every
/// PanelView; the ONLY downcast in the design lives here: a foreign
/// place type falls out of `location.place::<P::Place>()` as `None`
/// → false → displacement.
pub trait DynPanelView { /* the erased faces of everything above */ }
```

`Panel::Editor`'s place logic (`Place = EditorPlace`) lives directly
in the enum's arms — the pane holds only ids, so the arm joins the
registry location and the live caret itself; `Panel::Plugin` defers to
the erased faces. A
panel that is NOT navigable says so in the types — an uninhabited
place, so a `navigate_to` it could never honor cannot even be
constructed:

```rust
pub enum NoPlace {}                       // uninhabited; impl Place
impl PanelView for ClosedPanel {
    type Place = NoPlace;
    fn navigation_location(&self, _) -> Option<NoPlace> { None }
    fn navigate_to(&mut self, _, place: &NoPlace, _) -> bool { match *place {} }
    // …
}
```

Displaced plugin panels take the ordinary displaced-panel fate —
SURVIVAL on the unmounted list ([peeker.md](peeker.md); a hidden terminal
keeps its PTY, a hidden search its installs). A displaced editor pane
additionally retracts its editor (below).

## The history, and the one door

The workbench leaf grows the stacks beside its panel:

```rust
WorkbenchNode::Leaf(PaneSlot)
pub struct PaneSlot {
    panel: Panel,
    back: VectorSync<NavigationLocation>,      // persistent, like the undo log
    forward: VectorSync<NavigationLocation>,
}
```

Every programmatic content change funnels through one door on the
window:

```
navigate(pane, target: NavigationLocation):
    let outgoing = pane.panel.navigation_location(store)       // where we WERE, as it was left
    if pane.panel.navigate_to(store, &target, fx) {            // in place — a same-file
        record(outgoing) unless outgoing.same(&target)         // definition jump records
        return                                                 // like any other; AT the
    }                                                          // target already = no move
    record(outgoing); retire displaced panel
    pane.panel = Navigators::navigate(store, window, &target, fx)?  // TypeId dispatch
    // record(x) = pane.back.push(x); pane.forward.clear()     — a new branch forgets redo

back():                                        // forward() mirrors
    let target = pane.back.last()?;             // popped only on success:
    …in-place attempt, else Navigators::navigate…   // an unresolved place
    pane.back.pop(); current place → pane.forward…  // stays for a retry
```

`open_panel` (a pinned search, `diff.open`) funnels through the same
recording half — the door's rule is REPLACEMENT records, whatever
caused it; only the peeker's widget unmount/remount surgery
(`replace_focused_panel`) stays raw, displacement-for-survival is not
navigation.

The current place is captured AT NAVIGATION — so the caret/scroll
recorded is where the user actually left, with no per-keystroke history
writes. The same-place test is `Place: PartialEq` across the erasure
(`EditorPlace` compares position, not scroll — presentation is not a
move). Split copies the slot, history included (a browser's
new-tab-from-here). `navigation.back`/`navigation.forward` are
workbench commands on the focused pane, offered when the stack is
non-empty.

Existing flows re-route through the door rather than replacing panels
directly: `show_document` and the open landings build an editor place
(they hold the location and the reveal target already); go-to-definition
becomes a navigation with a caret; a tree/peeker pick likewise. Panels
that open by other means (a pinned search, cmd-R's repositories) still
record: the door is where REPLACEMENT happens, whatever caused it.

## EditorNavigator

The engine's one built-in — `Navigator<Place = EditorPlace>`:

- `by_location` hit → the open document; miss → fetch/build/register
  through the standard open pipeline ([files.md](../ahp/files.md)) — history entries
  hold no liveness, so a reaped document simply re-opens.
- mount an editor (`mount_editor`), restore the payload's caret and
  scroll, answer the `Panel::Editor`.
- the miss is ASYNC: the navigator launches the open and answers
  `None`. **The walk PARKS** (this replaced an earlier "record as a
  fresh navigation" edge — the miss is the everyday case, since
  displacement releases clean documents, and recording it broke the
  history: the landing pushed the current place onto back and wiped
  forward).
  `PaneSlot.pending` holds the walk's target and step (Back / Forward
  / Replace — the close-reveal); the `Opened` landing re-enters the
  door, which recognizes the parked LOCATION and COMPLETES the walk
  with the parked place — recorded caret and scroll restored, the
  stacks stepped (`complete_walk`), nothing freshly recorded. The
  entry stays on its stack while parked (a failed fetch loses
  nothing; a retry re-parks); any OTHER navigation through the door
  clears the stale walk, and its late landing behaves as the fresh
  open it now is.

The editor pane's typed `navigate_to(place: &EditorPlace)`: same
`ResourceLocation` → set caret + reveal + adopt scroll, true. Its
`navigation_location`: its document's registry location + live
caret/scroll — a PRISTINE scratch (revision 0) answers `None` and
stays out of history (scratches carry scratch:// locations, and
recording the untouched startup scratch would plant a phantom "back
to an empty buffer" step at the bottom of every pane's history; a
scratch WITH content records like any place).

Two more door rules:

- **re-showing the focused document with no target is a NO-OP** beyond
  recency — `show_document` used to fabricate a caret-0 place, which
  jumped the caret to the top, scrolled to 0, recorded a bogus branch
  and wiped forward on every peeker pick of the already-open file.
  Compared by LOCATION (the `Opened` dedup below shares the reason);
  suppressed while a parked walk waits on that location.
- **`Opened` dedups by location** before registering — an
  unconditional register minted an orphan registry twin per same-file
  landing; the live document (it may carry edits) wins and the fresh
  build drops.

Displaced editor panes retract their editor (`close_editor`), ALWAYS
— and the retraction rule applies: a document left editorless and
CLEAN is released from the registry ([documents.md](../editor/documents.md); dirty ones
stay, unsaved edits are never released). Registration follows display;
back to a released place re-fetches through the navigator's miss path.
Getting back at all is the RECENTS list's job, not the registry's:

```rust
RecentLocations           // a store singleton, most recent first, capped
```

Every successful located navigation touches it at the door. It is
COMPLETELY separate from `OpenDocuments` — remembering where the user
was is not keeping documents around. The peeker lists it as its
default rows ([peeker.md](peeker.md)); a released place previews and opens by
fetching, an open one takes the registry fast path. (Displacement
survival for plugin widgets is untouched: terminals and search results
retract only on explicit close, never by navigation.)

## Closing

`cmd-w` (`workbench.close`) closes the focused pane's CONTENT, not the
pane; `cmd-shift-w` (`workbench.close-pane`) un-splits. The close is
FOR REAL — displacement's survival does not apply: an editor retracts
and releases its document (dirty-guarded; a clean SCRATCH releases
here and only here — [documents.md](../editor/documents.md)), a widget dismantles (a
terminal's PTY hangs up, a search's installs unwind — the explicit
kill, [peeker.md](peeker.md)). The pane then shows the last
place its history holds — the back stack pops WITHOUT recording the
closed view (it is gone, not navigated away from). An empty stack
leaves the empty focusable blank; closing over the blank keeps walking
the history.

## Panel places beyond the editor

Diff declares `DiffPlace { old, new }` (both sides' resource
locations); its navigator lives at the himark-api composition point —
a hit on both sides rebuilds the pane (`hidiff::diff_panel`), a
reaped side re-fetches through `OpenDiffByLocationsEffect` and the
landing re-opens the pane.

There is no `SearchPlace` — the search panel's place type is
`NoPlace`, deliberately: restoring a search means RELAUNCHING its
scan, and a navigator has no pane-scoped effect path to do that (the
panel would need an inward command channel); a displaced search
survives on the unmounted list with its results meanwhile. Also
absent by choice: history caps (persistent stacks make deep ones
cheap), cross-pane history (a global "recent places" list), mouse
buttons 4/5. The keymap binds cmd-[ / cmd-] to
`navigation.back`/`navigation.forward`.
