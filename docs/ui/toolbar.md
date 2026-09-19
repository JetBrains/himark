# The Header Toolbar

A slim strip across the top of the window — the band
`document_geometry.top` reserves above the panes. Three things live
on it:

- **Buttons**, registered by features on either side of the center
  well (`ToolbarButtons`): the files tree, the changes view, the
  comments panel and the history view on the right; the agents
  drawer and the table of contents ([toc.md](toc.md)) on the left; a
  composer button inside the well itself. A press performs the
  registered command ([commands.md](commands.md)).
- **Center**: THE query input — the one input the peeker
  ([peeker.md](peeker.md)), the palette and the search modal used to
  each own a copy of, extracted and made window chrome. VSCode's
  "command center", in our dress.
- Unfocused, the center well shows the **focused panel's title**
  (`Panel::title(store)` — the document name, or the plugin panel's
  name); a click focuses it and the overlay session begins.

With the well naming the focused panel there are **no per-pane
headers**: `Panel::layout` mounts content over the whole pane and the
split dividers run full height. `Panel::title` remains — the toolbar
reads it.

## 1. One input, three surfaces

Focusing the well opens the **peeker** in the overlay slot. The first
character of the query then picks the surface, VSCode-style:

| prefix | surface | keyboard entry |
|--------|---------|---------------|
| *(none)* | peeker (documents, widgets, workspace finds) | cmd-P |
| `>` | command palette | cmd-shift-P |
| `%` | text search | cmd-shift-F |

Typing `>` as the first character *swaps* the open modal to the
palette mid-session; deleting it swaps back to the peeker. The query
the surface sees is everything *after* the prefix. Escape — or the
modal filing `ModalRequest::Close`, or a pick — ends the session:
the modal dismisses (releasing held widgets, as every dismissal
does), the input clears and unfocuses, the well shows the focused
panel title again.

The keyboard entries keep their ids and their meanings, based on the
toolbar: `peeker.toggle` focuses the well with an empty query,
`palette.toggle` seeds `>`, `search.open` seeds `%`. Each toggle
carries its surface: `toggle_toolbar_session(store, window,
&OverlaySurface, seed, fx)` takes the surface VALUE, so the toggle
commands work without any registry lookup. The `OverlaySurfaces`
registry serves only typed-prefix switching mid-session — plugins
register an opener keyed by prefix (peeker at `None`, palette at
`'>'`, search at `'%'`); himark-api and the web shell register all
three at startup.

**Session lifecycle.** `Window::dismiss_modal` ends the toolbar
session — every dismissal path (Escape, a pick, a drawer toggle's
dismiss) resets the well. The mid-session swap path uses
`release_modal_for_swap` + `set_overlay`, which keep the session and
its input while the surface under it changes.

## 2. Who owns what

**The toolbar is core** (`crates/himark/src/toolbar.rs`), a member of
the window's [`Layers`] — it is window chrome, like the workbench
background. But the surfaces are plugins, and core must not name them
(the command-inversion rule). So dispatch inverts twice:

- **`OverlaySurfaces` registry** (a store singleton, like
  `Commands`): the prefix table above. An opener is a toggle's body:
  it builds the modal (unmounting widgets, snapshotting commands,
  scoping effects) and returns it; the app mounts it via
  `show_modal`.
- **`ToolbarButtons` registry**: plugins register `{glyph, command
  id, order, side}` at init. A button press performs the registered
  `DynamicCommand` by id through the existing `Commands` registry.
  Buttons appear only when their command was registered, so
  capability-gating carries over for free. `ToolbarButton.order`
  picks position within its side — registration order is
  capability-gate order, not display order. Glyphs are painted
  vectors — no icon assets exist and none get introduced.

**Modals have no inputs of their own.** `ModalView` has one hook:

```rust
/// The toolbar's query changed — refilter. Default: no-op (a modal
/// that keeps its own input, or has no query, ignores it).
fn set_query(&mut self, store: &mut Store, ui: &UiCtx, query: &str,
             fx: &mut Effects<'_, DynCommand>) {}
```

The peeker and the palette have no input wells; their `layout` starts
the list at the top of the modal area. Their keymaps
(Up/Down/Enter/Escape) are theirs — those belong to the surface, not
the input. **The search view keeps its input for the docked case**: a
pinned search panel lives in a pane where the toolbar doesn't drive
it; in modal mode its input hides and `set_query` feeds it. The pin
seeds the docked input — docking search rebuilds the panel's own
input holding the session's query, so the panel continues where the
toolbar left off. There is no `ModalInputChrome`; the toolbar well is
the one query chrome.

## 3. Routing and focus

The toolbar files requests through a plain slot
(`RequestSlot<ToolbarRequest>`, drained by the app after every
content command) because entity surgery — opening a modal, performing
a dynamic command — happens above the layers:

```rust
enum ToolbarRequest {
    /// The query changed (or the well was just focused): make the
    /// surface for this query's prefix be the open modal, and feed
    /// it the remainder. The app resolves the prefix through
    /// OverlaySurfaces, swaps via show_modal when the surface
    /// differs, then calls set_query.
    Query(String),
    /// A button: perform this registered command.
    Command(&'static str),
}
```

The mode logic — "which surface does this query mean, is it already
up, swap if not" — lives in ONE place, the app's drain (`app.rs`,
beside the `ModalRequest` drain), not in the toolbar and not in the
surfaces.

**Geometry and events.** Layout gives every other layer the area
*below* the toolbar band — the workbench, the drawer and the modal
each mount in a `below_layer` offset container sized `(width, height
- toolbar.height)`, so nothing can ever render under the toolbar and
it stays clickable during a session. `document_geometry` is
strip-blind (content-area coordinates); window-coordinate consumers
(`split_current`, test probes) add `toolbar.height` themselves.

Pointer routing is by geometry: a mouse-down in the toolbar band
lands in the toolbar (and flips the window's layer focus to it); a
modal blocks the pointer over its own surface; everything else falls
through by layer. A click on the well files `Query("")` (focus + open
the peeker); a click on a button files `Command(..)`.

**Keyboard is the semantic focus walk, not widget events.** No widget
tree is built for a keystroke: views implement
`View::focus_data(store, ui) -> FocusData`, and the window folds the
layers' `FocusData` along the focus path — commands (keymap entries)
collect along the walk, `on_key`/`on_text` handlers catch what the
maps don't, and the palette's command list IS the same walk. While a
toolbar session runs, the layer focus names the toolbar, so typing
reaches its input's editor through the walk; the modal's
Up/Down/Enter/Escape ride the modal's own keymap in the same fold.

The input itself stays what it is everywhere else: a raw
`EditorView::input` value editing itself — caret, selection and IME
for free.

## 4. Chrome

`ToolbarChrome` in `UiTheme` (`#[serde(default)]`, physical-px,
2x-tuned like every section — no constant in code):

- `height` — the strip; `document_geometry.top` is
  `max(clamped top, toolbar.height)` so panes never collide with it.
- `background`, `rule` (the bottom hairline).
- the center well: `well_width_ratio/min/max`, `well_height`,
  `well_radius`, `well_fill`, `well_fill_focused`, `title_size`,
  `title_color` (the unfocused panel name, dimmed and centered).
- buttons: `button_size`, `button_gap`, `button_inset`, `glyph_color`,
  `button_hover_fill`.

## 5. Deliberately not built

- Plugin-contributed surfaces beyond the three (the prefix table is
  the extension point — a `#`-symbols surface, `:`-goto-line).
- Per-pane breadcrumbs in the unfocused well; it shows one title.
