# The Column Headers

There is no toolbar band. Every column draws its OWN header — a
`hikit::header::ColumnHeader`, 104px tall over the ground with a
hairline under it — and every header puts its words on ONE baseline
(`ToolbarChrome::title_baseline`), whatever each one's largest word.
Rank is size and weight, never a fill or an icon (docs/Design.md; the
type ramp in [design-system.md](design-system.md)):

| column | leading | title (34/640) | subtitle (28, ink 3) | trailing words |
|--------|---------|----------------|----------------------|----------------|
| the chat | the tabs **Sessions · Chat** | the session's title (omitted while the Sessions drawer stands) | `in <folder>` (`PanelView::subtitle`) | — |
| the tree, leftmost (no chat column) | the tabs **Sessions · Chat** | the focused file's name | its directory, ` / ` between segments | **Contents** (quiet rank); the side column's tabs, dim, while the dock is closed |
| the tree, beside the chat | — | the focused file's name | its directory | **Contents**; the dim tabs while the dock is closed |
| the side column (dock) | the tabs, the active one in the title rank | — | — | — |

The LEFTMOST column — the chat's, or the tree's while the chat hides —
opens with two tabs right after the semaphore: **Sessions** (the
drawer, `agent.toggle-agents`) and **Chat** (`chat.composer`); the
standing one is in the title rank, the other dim, and a title that
follows them keeps `ToolbarChrome::tabs_title_gap` off. The Sessions
drawer has no header of its own: it covers the chat column EXACTLY,
under that header (`Workbench::chat_column_rect`), with the Sessions
tab active; pressing Chat rolls it away. There is no hide-chat word —
the chat column hides by the window narrowing or `MaximizeTree`.

The leftmost header insets its title past the platform semaphore
(`ChromeClearance`, written by the shell) and never short of
`ToolbarChrome::lead_inset`; the tree's text edge is
`WindowChrome::code_text_edge`, the side column's
`WindowChrome::side_text_edge`.

## Registered buttons are words

Features still register `ToolbarButton`s (`ToolbarButtons`), now with
a `label`: the side `Left` names the chat column's session actions,
`Right` the side column's TABS (History · Files · Find · Changes ·
Comments — `ToolbarButtons::tabs`), `Well` the chat's own word. A
press mints `WorkbenchCommand::RunCommand(id)` (the registry's run
road) from a column header, or `DockCommand::HeaderButton(index)`
from the dock's own header; the window-level `Toolbar` lays an empty
strip and keeps only its request slot.

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
