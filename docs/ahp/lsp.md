# LSP: code intelligence, client side

himark speaks go-to-definition, find-references, completion and
hover without the engine ever speaking LSP. The protocol lives on
the other side of the wire: the agent host supervises one language
server per workspace root and forwards stateless requests verbatim
(docs/ahp/ahp-lsp.md); text truth reaches the servers through the
document channels (docs/ahp/ahp-documents.md), never through the client.
The engine stays LSP-blind the way it is git-blind: it launches
effects carrying locations and positions, and answers come back as
locations and markdown.

The load-bearing decisions:

1. **The client sends no text.** There is no client-side
   didOpen/didChange stream — the host feeds its servers from the
   document channels it already owns, so the server an ask reaches
   has seen exactly the text the user is looking at. An ask carries
   a location and a position, nothing else.
2. **Positions cross the boundary as line/col** (`LineCol` — col in
   bytes within the line; the wire negotiates utf-8). The rope
   answers line/col ↔ byte in log time; the engine converts at its
   own edges — caret → line/col when asking, answer → bytes at
   install/reveal, where the target's text is in hand. No position
   arithmetic exists anywhere between.
3. **One results component.** The "collection of locations grouped
   by document, previewed as bounded row editors" machinery is
   `himark::location_list::LocationList` — search ships on it and
   the references panel is its second consumer. No second copy of
   the install/fetch/temp-document discipline exists.

## The asks

Two commands in `plugins/hicode` — **editor-scoped**, not global:
`code.definition` ("Go to Definition") and `code.references` ("Find
References"), `DynamicEditorCommand`s offered by every LOCATED view
(`EditorView.location` is joined from the registry record at gather;
scratch and temp documents offer nothing — the server has no name
for them). The focus walk puts the focused editor's entries in front
of the palette (docs/ui/UI.md, Focus).

The flow rides ordinary routing: the pick performs AT the editor —
document id, location, caret in hand — and launches ONE composed
effect, `CodeNavigationEffect`. Its handler (holding an
`EffectCaller`) does the whole worker half: call the ask
(`FindDefinitionEffect` / `FindReferencesEffect` — answered by the
seat route, which wraps the position into the wire's
`textDocument/definition` / `references` envelope and converts the
answers back under the folder's authority), and when a panel is
coming (more than one target), prefetch-and-build every never-opened
target document before landing one command. The landing's rule is
the feature's whole UX:

- **exactly one target → navigate.** The open road carries an
  optional TARGET (`Range<LineCol>`) end to end — the already-open
  path and the fetch path alike — resolved to bytes against the
  opened document's rope at mount; the caret lands on the range and
  the scroll-to-caret reveal converges even while background repairs
  move the geometry.
- **more than one → the references panel**, fully populated at the
  landing: targets grouped by location (the LIVE registry wins — a
  document opened since the ask shows as itself, edits included;
  prefetched builds register under their locations), spans converted
  against each document's rope, exact marks plus context lines.
- zero / `None` → nothing lands. `None` means "no server could
  answer"; an empty vec means the server answered "nothing".

Both asks are ask-answer: fire-and-forget, never on a lane — every
asker is owed its answer.

## The references panel

A thin face over the shared component: a `PanelView` hosting
`ScrollView<LocationList>` under a title band. Rows are live bounded
editors over the real documents (open ones: the document itself;
unopened ones: registered previews) — so "find references, fix them
in place" works exactly like editing in search results. The panel
rows live in the session's `LocationLists` family; a displaced panel
survives as its family row and the peeker re-mints a handle
(docs/ui/app-state.md, docs/ui/peeker.md).

`LocationList` itself is feature-blind and scroll-free by design (a
view holding its own scroll does not compose — the owning panel
wraps it). It owns the whole install discipline: tint markup via the
feature-markup doors, fragments and bounded row editors, the
unopened-document fetch/build pipeline registering at display, the
generation guard (landings from an older set drop), the display
budget with its note, and the clone rule — a clone carries its
install bookkeeping. Its `SpanSource` seam is why the wire carries
no offsets to go stale: whoever installs a fetched document derives
the spans against the text actually built (search re-scans with its
matcher; references converts its line/col answers).

## Completion and hover

- **Completion** (`himark/src/completion.rs`) rides the same seat
  road: the pane's completion controller launches its effect on a
  LANE (a newer ask supersedes), the route sends
  `textDocument/completion` and converges the wire's shapes into the
  popup's rows. The popup is pane chrome on the focus walk — its
  keys sit OVER the editor's while it is up.
- **Hover** (`himark/src/hover.rs`) is pointer-driven: the editor
  core hears per-move, occlusion-honest in/out through the HitTest
  broadcast (containers hand the true point only to the topmost
  child under it via `Widget::blocks_pointer` — a pane under a
  drawer or modal hears a MISS, which IS its leave). The pane's
  hover controller resolves the WORD under the point — the natural
  debounce: a move within one word re-asks nothing, a new word
  supersedes the lane. A new word ARMS rather than asks: the ask
  fires only after a rest interval (the tooltip accumulate-on-ticks
  shape, so the armed pane keeps the animation clock alive), so a
  pointer sweeping across text pops nothing in its wake. The route
  converges `MarkupContent` / `MarkedString` / arrays to MARKDOWN (a
  `{language, value}` becomes a fenced block), and the landing
  mounts a display-only `HoverView` card above the word — a value
  editor over a real markdown document (the chat-cell recipe:
  glance-sized, synchronous full layout, no lane owed), so code
  spans, fences and emphasis render exactly as they do everywhere
  else. Any other activity dismisses it. Hover is offered by
  located, non-synthetic documents only.

## What deliberately does not exist client-side

- **No LSP process, no protocol structs, no position re-encoding** —
  the deleted-by-design half. The host owns server lifecycle,
  capabilities, and text sync; the client's four methods
  (completion, hover, definition, references) are the entire ask
  surface it sends today.
- **No diagnostics surface.** The host streams diagnostics on their
  own channel (docs/ahp/ahp-lsp.md); nothing in the client subscribes or
  renders them — the gutter/underline surface is absent, not hidden.
- **The engine's change-notification seam is uninstalled.** A
  generic "a located document's text changed" effect
  (`DocumentChangeEffect` behind the `ChangeObserver` gate) exists
  in the documents crate, but no distribution installs it: with text
  truth on the document channels there is nothing for the client to
  notify. It remains the seam a non-channel consumer would gate on,
  and launches nothing until one exists.
