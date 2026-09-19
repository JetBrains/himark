# Widgets, mounting, and the peeker as a buffer switcher

Emacs semantics, modeled properly: a stateful panel's SUBSTANCE is a
session-family row ([app-state.md §2](app-state.md)) — a terminal's PTY in
`terminal::Terminals`, a search/references list in `himark::Lists`, a
diff pair in the diff registry's pane rows
(`OpenDocuments::pair_ref`), a chat in `higent::Chats` — and the
mounted panel is a cheap HANDLE over that row (the ChatPane recipe).
There is no shelf of displaced widgets: displacement just drops the
handle; the row survives in the store and rides its session.

## The model

- Displacement (`replace_focused_panel` / `close_pane` / a navigation
  displace) DROPS the plugin handle. Nothing dismantles: the family
  row — the PTY, the installed editors, the pair — lives on.
- `dismantle` is the DESTRUCTION hook — the explicit kill (cmd-w) and
  window teardown: it tears the row's installs down and removes it.
- Each handle names the row it fronts through
  `PanelView::family_row() -> Option<FamilyRow>`; the peeker uses it
  to skip rows a mounted widget already shows.
- A fresh handle over any row is minted by `himark::family_rows`
  (terminals, lists and chats directly; plugin-owned kinds — the diff
  pair — through the `RowMinters` catalog, registered at boot via
  `Application::register_row_minter`).
- Editor panes are NOT widgets: their substance is the document,
  already listed and previewed by the existing machinery; a displaced
  editor pane simply drops.

## The peeker

The buffer list = open documents (workspace finds included)
+ every widget: the MOUNTED panels (taken from their panes for the
session) and a freshly MINTED handle for every family row no pane
fronts — labeled by `title()` (handles read it off the store), one
subsequence filter over all of it.

**The preview is a real mount point.** Selecting a widget MOUNTS it
there — the value moves:

- a family row → its minted handle, laid out at preview size,
  displayed — resizes and all the fuzz (a terminal simply resizes;
  that is fine).
- mounted → UNMOUNTED from its pane (a blank placeholder takes the
  leaf), mounted into the preview, displayed the same way.

Preview commands flow normally to the mounted widget (no swallowing,
no per-type handling); its effects lift through the modal machinery
like the document preview's do. Moving the selection returns the
widget where it came from (pane or list); closing the peeker restores
everything.

## Picking

The previewed widget mounts into the FOCUSED pane:

- the panel previously in the focused pane DROPS — its family row
  survives (that is the point) and relists next time;
- if the picked widget was mounted in another pane, that pane keeps
  the blank placeholder;
- picking a widget already in the focused pane just dismisses.

Document picks are unchanged (show / adopt / open-by-location).

## Machinery

- `Window::unmount_all_widgets()` takes the mounted panes (blanks
  behind); the peeker appends `himark::mint_unfronted(store,
  &fronted)` — fresh handles for the rows no held widget fronts.
- `WidgetOrigin` remembers how each goes back at dismissal:
  `Pane(index)` remounts over its blank (pre-order `for_each_pane`
  positions, valid while the modal blocks pane surgery);
  `Family` just drops — the row lives in the store.
- `ModalRequest::SelectWidget` carrying the previewed widget home;
  peeker model: entries documents + widgets, preview = editor preview |
  mounted widget, commands routed via a `PeekerCommand::Widget` arm.
- Cleanup is `View::destroy` ([UI.md](UI.md)), which every dismissal path
  reaches — `release_widgets` clears only a WIDGET preview slot (the
  widgets are being taken); an EDITOR preview survives the hook so the
  destroy right after can retract it. The peeker reaps only its OWN
  fetches (`temp_docs`): a LISTED open document browsed through loses
  the preview editor and nothing more.

The DEFAULT rows are the RECENT PLACES (`RecentLocations`,
[navigation.md](navigation.md)), not the open documents — recency is remembered
separately from keeping documents around. Every location row resolves
one way: an open document previews and picks through the registry; a
released or never-opened one fetches (registered at display, adopted
on pick). Workspace finds list below, deduplicated against the
recents. Scratches carry scratch:// identities ([documents.md](../editor/documents.md)) —
they list and recur like any open, live through displacement, and die
only to cmd-w.

## Out of scope

- Cross-window widgets.
- Persisting the recents across sessions.
