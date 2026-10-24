# The Toolbar Band

One STATIC band, `ToolbarChrome::height` (104px) tall, stands over
the whole window; the body's columns lay under it. The band has
three sections, each a `hikit::header::ColumnHeader` — words on ONE
baseline (`ToolbarChrome::title_baseline`) over the ground, a
hairline under — and a vertical rule between neighbours. Rank is
size and weight, never a fill or an icon (docs/Design.md; the type
ramp in [design-system.md](design-system.md)):

| section | over | leading | title (34/640) | subtitle (28, ink 3) | trailing words |
|---------|------|---------|----------------|----------------------|----------------|
| the chat's | the chat column | the tabs **Sessions · Chat** | the session's title (while the chat column stands and no drawer covers it) | `in <folder>` (`PanelView::subtitle`) | — |
| the main area's | the split tree | — | the focused file's name | its directory, ` / ` between segments | **Contents** (quiet rank), when the focused panel offers a drawer (`Panel::offers_drawer`) |
| the side column's | the dock | the tabs **Files · Changes · Find · History · Comments**, the open dock's in the title rank | — | — | — |

Panels come and go; the band stays. A section is as wide as the
column under it and never narrower than its own words: the chat
section keeps the width of `Sessions · Chat` while the chat column is
gone, the side section the width of its tabs while the dock is
closed, and the main section takes what is left between them. The
rules between sections stand at the SAME x as the body's splitters —
inside the chat column's last pixels, on the dock's first — because
the band and the body lay against the one derivation,
`workbench::columns::Columns` (`Workbench::columns`): the dock's
revealed width from the right, the chat's from the left, the main
area between. The dock's slide and the chat's slide move the band's
rules and the body's splitters together.

**Sessions** opens the drawer (`agent.toggle-agents`) over the chat
column — exactly over it, under the band (`Columns::drawer`) — and
stands in the title rank while it does; the chat's name hides under
it. **Chat** TOGGLES the chat column: standing, a press rolls the
column away (`ToolbarCommand::RollAwayChat` → `MaximizeTree`, the
dock's slide mirrored); otherwise it runs `chat.composer`, which
fronts the chat — beside the tree when both fit, over the main area
when the tree is vacant or the window is too narrow for both. Rolled
away, the chat hides whatever the room.

The chat section's first word starts past the platform semaphore
(`ChromeClearance`, written by the shell) and never short of
`ToolbarChrome::lead_inset`; the main section's text edge is
`WindowChrome::code_text_edge` from its rule, the side section's
`WindowChrome::side_text_edge`.

## The body

Under the band, up to three columns: the chat column, the split
tree, the dock. The chat column lays at its full width and SLIDES —
mid-slide it stands past the window's left edge and the tree takes
the rest from where the column shows; the dock does the same from
the right. A draggable splitter stands between the chat and the
tree (`ChatResize`, a share of the room left of the dock) and on the
dock's edge (`DockCommand::Resize`); each is a hairline the band's
rule continues.

## Registered buttons are words

Features register `ToolbarButton`s (`ToolbarButtons`) with a `label`:
the side `Left` names the chat section's tabs (Sessions), `Right` the
side column's tabs (`ToolbarButtons::tabs`), `Well` the chat's own
word. The registry's `drawer_button` (Contents) is the main
section's. A press mints `ToolbarCommand::Word(id)`, which the band
files as `ToolbarRequest::Command(id)` for the shell's run road.

## There is no omnibox

Each query surface is a standalone overlay that OWNS its input editor:

| surface | entry | input |
|---------|-------|-------|
| jump to file (the peeker: documents, widgets, workspace finds) | cmd-P, `peeker.toggle` | its own, top of the sheet |
| command palette | cmd-shift-P, `palette.toggle` | its own, top of the sheet |
| find | cmd-shift-F | the Find tab's own input ([location-list.md](location-list.md)) |

The overlays are plain z-stacked layers in the window frame (the
side and modal slots now span the whole window —
[app-state.md](app-state.md)), not `imba` popup overlays. Their
toggle commands build the view and call `Window::show_modal`; a
standing modal is dismissed first. Keyboard reaches them through the
ordinary modal focus fold.
