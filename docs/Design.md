# himark: the design

himark is a rich-text editor and agent workbench: markdown-first
editing with the performance discipline of a code editor, plus
first-class hosting of coding-agent sessions — chat, working-tree
diffs, version control, terminals, comments, and language
intelligence, all against the workspaces the agents mutate
([agents.md](ahp/agents.md), [agent-host.md](ahp/agent-host.md)). Markdown is the
founding document type, not the boundary: fenced code, embedded
languages, tables, diagrams and source files ride the same document
machinery.

The load-bearing decisions, up front:

1. **Logarithmic complexity on the UI thread.** Every per-frame
   question — where is byte N on screen, what row is at y, what does
   this viewport show — is answered by a seek over measured
   persistent trees, never by a scan. This is what lets one code
   path serve a gigabyte document, a million-row list, and a deep
   tree with the same latency.
2. **One document value.** Text, markup, diffs, parse state and
   every editor projection live in a single persistent value with a
   single edit door — consistency by construction
   ([document.md](editor/document.md)).
3. **The frontend renders; the backend owns the world.** Files,
   search, terminals, agents, git and language servers live in the
   agent host, reached over one protocol — AHP — for local and
   remote alike.
4. **One engine, thin shells.** A headless Rust engine renders
   through Skia into a canvas it is handed; each platform contributes
   only windowing, input and pixels.

## Logarithmic on the UI thread

The render pipeline is immediate-mode ([UI.md](ui/UI.md)): every frame
queries the data structures and emits Skia draw commands. The
React/Compose-style incremental pipeline is deliberately discarded —
if the frame cannot be produced within budget from the non-retained
state, positional caching does not fix the problem, it hides it
until the first uncached frame. **Any** frame must render inside the
target budget, so nothing may rely on memoization for amortization.
The data structures must have everything ready.

"Ready" means measured, persistent trees
([rope-and-intervals.md](editor/rope-and-intervals.md), [sumtree.md](editor/sumtree.md)): ropes
whose child edges cache additive metrics (bytes, lines, heights,
row counts), interval sets that shift under edits, sum trees for the
non-additive summaries. Rendering cost is **`View · log(Doc)`** —
proportional to what is visible, logarithmic in what exists. The
same substrate serves every large surface, not just the editor:

- the **editor** — text bytes ↔ lines ↔ pixels by seek
  ([document.md](editor/document.md));
- **lists and trees** — imba's `ListView` is a rope of rows with
  tree structure as data ([list-view.md](ui/list-view.md),
  [list-tree.md](ui/list-tree.md)), so the changes tree, search results, the
  diff canvas and the file tree scroll million-row content at
  editor latency;
- **diffs** — an `Operation` is itself a rope, so composing a
  keystroke into a standing diff and mapping offsets across it are
  O(log n) ([diff.md](editor/diff.md)).

Work that cannot fit the frame is organized into three loops with
different latency contracts:

- **The render loop** (vsync, pointer, typing, scroll, selection)
  runs at `View · log(Doc)`. Typing splices the affected layout
  element locally and marks damage — the render loop never runs a
  rewrite it cannot afford.
- **The layout loop** repairs damaged layout regions — linear in the
  damage, never in the document. The viewport's damage repairs
  synchronously under a small budget, anchored so the view does not
  jump; the rest repairs in background effects under a larger
  budget, landing guarded ([document.md §layout](editor/document.md),
  [viewport-preservation.md](editor/viewport-preservation.md)).
- **The background loop** — parsing, diff computation, enrichment,
  file I/O — runs as effects ([effects.md](ui/effects.md)). A late result
  REBASES over the edits it missed (the edit log is the universal
  catch-up — [rebase.md](editor/rebase.md)) rather than being discarded on a
  race with the user; where rebase is impossible, a fresher launch
  supersedes per lane, driven by stored cancellation tokens.

Each frame has a strict budget split between the loops. Where
threads exist, layout repair and background work run off-thread; the
web build without threads runs the same dispatch inline between
frames.

## The document

`Document` is the engine's central value — text rope, two-lane
markup entries, the parse hierarchy with nested language mounts, the
edit log and undo, tracked diffs, and the editors themselves: each
editor a projection (layout at a width, carets, viewport) stored
inside the document so one edit moves the substance and every
projection atomically. The full anatomy and its rationale:
[document.md](editor/document.md); the storage story (identity, lifetime,
who holds what): [documents.md](editor/documents.md).

Layout deserves its one-paragraph summary here because the budgets
above depend on its shape: a `DocumentLayout` is a vertical rope of
horizontal elements — one per soft line or collapsed run — measured
by bytes and pixels, with damage kept beside it as an interval set.
Repair is a partial rewrite: it starts at a safepoint before the
damage, is quantized (stoppable at any point, resumable against
newer state), and owns alignment against the old layout. A window
resize damages everything — and still only the viewport repairs
before the next frame; the rest heals behind it while the viewport
stays pinned.

## The client and the agent host

The frontend communicates with the outside world exclusively through
effects, and the world-touching effect handlers are implemented in
exactly one place — the AHP client — by speaking **AHP** to the
backend: the **agent host**, where FS, search, terminals, agents,
git and the language servers live.

**The protocol is the only road.** An effect handler has exactly one
implementation, for local and remote alike; the `local` authority is
a name — the local backend's filesystem session — and routes over
the protocol like everything else. Three dependency rules keep the
line real:

- no frontend crate links the backend — it is reached only over the
  wire (tests serve the same protocol in-process over a socketpair:
  the same road, not a shortcut);
- the backend knows the protocol types and the text substrate, never
  the engine or plugins;
- plugins declare effects and register UI; they never implement
  world-touching handlers.

**Every workspace is a session.** Opening a local folder creates (or
rebinds) a session for it on the local agent host; the folder then
carries a session authority, and routing resolves an authority to a
seat with no fallback arm. Search is the seat's search
([ahp-search.md](ahp/ahp-search.md)); terminals are the seat's; watches, change
sets and stripes ride the protocol's channels ([ahp-ext.md](ahp/ahp-ext.md));
the language servers run on the backend, fed by the document
channels ([ahp-lsp.md](ahp/ahp-lsp.md)).

The backend must therefore always be reachable: it is its own binary
bundled beside the app, discovered through a lockfile, autostarted
when none is live. A frontend without a backend is a frontend
without workspaces — scratch documents still work; folders don't.
That is the honest contract.

Why this line, and why so absolute:

- **One protocol, one truth.** Every capability crossing it is
  written down in a spec (docs/ahp/) and testable at the wire — no
  behavior hides in an in-process shortcut the remote case would
  re-implement differently.
- **The remote story is free.** A remote workspace is the same
  backend over another transport ([remote.md](ahp/remote.md)); with one road
  there is no "remote-only" code path to keep honest.
- **The shells shrink.** No shell can be wrong about a file, a
  search, or a pty.

## Multiplatform

One engine crate tree in Rust, headless: it owns text, layout,
editing, and rendering *into a canvas it is handed*
([embedding-api.md](editor/embedding-api.md)). Each platform is a thin shell over
the same engine:

- **macOS / iOS** — an AppKit/UIKit app linking the engine as a
  static library, rendering through Metal;
- **Linux / Windows** — a shared `winit` shell;
- **web** — the same engine compiled to WebAssembly, rendering
  WebGL2 in a worker pool, connected to an agent host over a
  WebSocket.

The shell keeps only what is genuinely the platform's: windowing,
input/IME, clipboard, the file-picker dialogs (they answer
locations, never content), fonts, and the surface to draw on.
Capability flags on the shell's callback surface signal what the
host brings; the bodies do nothing. Everything behavioral — every
document, every feature, every workspace operation — is the same
code on every platform, which is why the engine's test suite drives
the product deterministically without any shell at all.
