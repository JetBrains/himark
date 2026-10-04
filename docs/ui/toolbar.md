# The Header Toolbar

A slim strip across the top of the window — the band
`document_geometry.top` reserves above the panes. Two things live on
it:

- **Buttons**, registered by features (`ToolbarButtons`): the files
  tree, the changes view, the comments panel and the history view on
  the right; the agents drawer and the table of contents
  ([toc.md](toc.md)) on the left; the chat composer button beside the
  title (`ToolbarSide::Well` — the side keeps the historical name). A
  press performs the registered command
  ([commands.md](commands.md)).
- **Center**: the **focused panel's title** (`Panel::title(store)` —
  the document name, or the plugin panel's name), plain text. It is
  not an input and takes no clicks.

With the toolbar naming the focused panel there are **no per-pane
headers**: `Panel::layout` mounts content over the whole pane and the
split dividers run full height. `Panel::title` remains — the toolbar
reads it.

## There is no omnibox

The toolbar used to host THE query input — a shared well the peeker,
the palette and the search modal typed through, with VSCode-style
`>`/`%` prefixes swapping surfaces mid-session. That is gone. Each
surface is now a standalone overlay that OWNS its input editor:

| surface | entry | input |
|---------|-------|-------|
| peeker (goto-file: documents, widgets, workspace finds) | cmd-P, `peeker.toggle` | its own, top of the panel |
| command palette | cmd-shift-P, `palette.toggle` | its own, top of the layer |
| text search | cmd-shift-F | the search dock's own input ([location-list.md](location-list.md)) |

The overlays are plain z-stacked layers in the window frame (the
modal slot under the toolbar band — [app-state.md](app-state.md)),
not `imba` popup overlays. Their toggle commands build the view and
call `Window::show_modal`; a standing modal is dismissed first.
Keyboard reaches them through the ordinary modal focus fold: the
overlay's `focus_data` folds its own keys (Escape, Enter-on-empty)
over the list controller's table over its input editor's seats.

## Mechanics

- `Toolbar` holds no state but a request slot. `ToolbarCommand`
  is `Button(index)`; a press files `ToolbarRequest::Command(id)`,
  drained by the app loop into the registered command.
- The title is centered across the full band; `ToolbarSide::Well`
  buttons hug the title's left edge; Left buttons respect the
  platform chrome clearance; Right buttons end flush with the band.
- The pressed button backdrop is `ToolbarChrome::pressed_fill`; the
  band's only other paint is `background` and the bottom `rule`
  hairline.
