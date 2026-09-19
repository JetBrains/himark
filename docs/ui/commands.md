# Named commands

Every serious editor is extensible through some notion of a *command*:
a unit of user intent that keybindings, menus, palettes, macros and
plugins all speak. himark's commands are typed — per-view enums,
routed along the focus path — and carry **names**: stable string ids
that keymaps, the palette, menus and the host FFI resolve against one
surface. This doc surveys how the editors that got extensibility right
structured their command systems, distills the design dimensions, and
describes the system himark runs.

## 1. The typed substrate

The dispatch machinery underneath the names:

- **Typed payloads per view.** Every `View` declares its `Command` enum:
  `EditorCommand::InsertText`, `TableCommand::InsertRow(usize)`,
  `PeekerCommand`, `SearchCommand`… Payloads are real types with real
  fields; the compiler checks every construction site.
- **Focus-path routing.** Position-less events (keys, text) reach the
  focused path through the SEMANTIC focus walk: views implement
  `View::focus_data(store, ui)`, each parent picking its focused child
  by state ([UI.md](UI.md), Focus) — no widget tree is built for a
  keystroke. A command performs against the store in one transaction;
  effects defer work and *come home as commands* — the async story is
  solved and uniform.
- **Type erasure at the plugin boundaries.** A `PanelView` (search,
  the diff pane) and an inlay view (table, checkbox) speak
  `DynCommand = Box<dyn Any + Send>`; the owner routes the box down and
  the plugin downcasts. Typed payloads cross an erased boundary and
  stay typed on both sides.

Identity rides this substrate without duplicating it: a name is bound
to an already-typed command, never to a second dispatch path.

## 2. The survey

### Emacs — everything is a named function

Commands are interactive functions in one global namespace; keymaps are
first-class values layered per buffer (global → major mode → minor
modes → text properties); `M-x` invokes by name; `describe-key` inverts
the mapping. Anything can be rebound, advised, or hooked at runtime.
*Buys*: the ceiling on extensibility — whole applications live inside
it. *Costs*: no static structure at all; name collisions, minor-mode
keymap ordering as a way of life, and the interpreter as a permanent tax.

### Vim — the command grammar

Two systems: modal key mappings and Ex commands (`:names` with ranges
and args). The real invention is that normal mode is a *composable
grammar* — operator × motion × text object — so `d`, `i`, `p` multiply
instead of adding. Extensibility is mapping key sequences to key
sequences; Neovim adds a real API (`nvim_create_user_command`,
buffer-local maps, descriptions for discovery). *Buys*: a tiny set of
primitives composing into an editing language. *Costs*: modality, and
the grammar only composes over text motions — structural commands (our
tables) don't fit it.

### VS Code — the flat string registry

A command is a string id (`"editor.action.rename"`) mapped to a callback
taking JSON-ish args and returning a promise. Keybindings, menus and the
palette are *data* contributed in `package.json`; enablement and
visibility are **when-clause contexts** — a tiny boolean DSL over
context keys that anything may set. Dispatch is global; the context
system compensates for the missing focus routing. *Buys*: one uniform
surface serving palette, menus, keymaps and out-of-process extensions.
*Costs*: stringly-typed everything — args are unvalidated bags, ids are
typos waiting to happen, and when-clause soup becomes its own language.

### IntelliJ — actions pull their context

`AnAction` objects registered by id in `plugin.xml`, arranged into
`ActionGroup`s from which menus and toolbars are *generated*. An action
implements `update()` (enabled/visible) and `actionPerformed()`, both
reading a `DataContext` — typed keys resolved against the focused
component's ancestry. The action isn't routed to the target; it *pulls*
the target out of the focus-derived environment. Keymaps are data,
per-OS schemes, shortcuts attach to ids. *Buys*: one action serves every
surface with correct contextual enablement. *Costs*: `update()` runs
constantly and becomes a performance discipline of its own; XML
ceremony.

### Sublime Text — commands as data, macros for free

Commands are Python classes surfaced by snake_case name, split by target
(`TextCommand` receives an `Edit` token that groups its mutations into
one undo step; `WindowCommand`; `ApplicationCommand`). Everything
invokes them as `name + args-dict`: keymap JSON, menu JSON, and —
crucially — **macros are just recorded lists of (name, args)**.
Key contexts are queried back into plugins (`on_query_context`).
*Buys*: because invocation is data, replay/record costs nothing.
*Costs*: stringly ids and args again; the `Edit` token exists precisely
because undo needed an explicit bracket.

### Zed / GPUI — typed actions on the focus path

An action is a Rust *type* (a struct, possibly with fields) registered
under its name. Dispatch walks the **focus path** through the element
tree; any node may handle `on_action::<T>`. Keymaps are JSON mapping
keystrokes to action names *with args deserialized into the typed struct
via serde*, scoped by key-context strings matched against the focus
path's context stack. *Buys*: the modern synthesis — typed payloads,
data keymaps, focus routing, palette from the registry. *Costs*: needs a
type↔name registry and serde on every action; contexts are still
strings.

### CodeMirror 6 / ProseMirror — commands as functions

CM6: a command is `(view) => bool` — returns whether it handled;
keymaps are prioritized extension lists; composition is first-true-wins
chains. ProseMirror adds the gem: a command is
`(state, dispatch?) => bool` — call it **without** `dispatch` and it
answers "would I apply?" without doing anything; with `dispatch` it
builds a transaction. Enablement and execution are the same code, never
drifting apart. *Buys*: minimal, pure, composable. *Costs*: no names, so
palettes/keymaps-as-data need a wrapper layer (which Obsidian built:
`addCommand {id, name, checkCallback, hotkeys}` — the PM dry-run trick
under a VS Code-shaped registry).

## 3. The dimensions

Reading the survey as a decision table:

| dimension | the options seen | who |
|---|---|---|
| identity | function name / string id / **type** / anonymous fn | Emacs / VS Code, Sublime / Zed / CM6 |
| payload | none / JSON bag / **typed struct** | Vim / VS Code / Zed |
| targeting | global + context keys / context pull / **focus path** | VS Code / IntelliJ / Zed, CM6, himark |
| enablement | when-DSL / `update()` / **dry-run convention** | VS Code / IntelliJ / ProseMirror |
| binding | code / **data keymap (id + args)** | Emacs, CM6 / everyone else |
| replay | impossible / **free (invocation is data)** | — / Sublime |
| undo bracket | explicit token / **transaction per dispatch** | Sublime / ProseMirror, himark |
| extension registration | runtime registry / static manifest / **compiled plugins** | Emacs / VS Code / himark, Zed |

Two convergent lessons stand out. First: every system that serves a
palette, menus and keymaps from one source made *invocation a piece of
data* — a name plus arguments. Second: the systems that stayed pleasant
kept *payloads typed* and let data cross into types at exactly one
boundary (Zed's serde'd actions; our own `DynCommand` downcasts). The
systems that let strings leak all the way through (VS Code, Sublime)
pay for it forever.

## 4. The constraints

What the command system must serve — plugins adding invocable commands
without touching core enums or the shells; keybindings as data owned
by himark (the web and iOS shells have no menus at all); a command
palette; one surface the shells query instead of defining — without
giving up:

- Typed payloads. The compiler proves every command construction; we
  are not trading that for JSON bags.
- Focus-path dispatch. It exists and it is *correct* — the focused
  table cell gets the key before the pane does. VS Code-style global
  dispatch + context keys would be a regression.
- One store transaction per dispatch, effects coming home as commands.
  The command system rides the existing `perform` spine, not a second
  one.
- Nothing per-frame, nothing O(doc) on the UI thread. The command
  surface is consulted on key events and palette opens — never during
  paint.

## 5. The system: offered ids on the focus path

Two kinds of named command, one resolution surface.

### 5.1 `PresentableCommand` — focus-scoped, offered by views

```rust
pub struct PresentableCommand<Command> {
    /// Stable, namespaced: "editor.undo", "workbench.split-pane",
    /// "find.next". The only string in the system.
    pub id: &'static str,
    /// What the palette shows: "Collapse to Single Caret".
    pub name: String,
    /// The TYPED payload, already constructed. No serde, no args
    /// bag: the view built the real command when it offered it.
    pub command: Command,
}
```

Views contribute these in `View::focus_data` — the same state walk
that routes keys. Each node on the focus path appends its commands on
the way down, `.map(...)`-wrapping them into the parent's command type
exactly as commands nest everywhere else. The editor offers its whole
key surface this way (`caret_surface` in
frontend/editor/src/editor_view.rs: undo/redo, the movement set as
`editor.move-*`/`editor.select-*` pairs, indent, deletion); panels,
drawers and overlays offer theirs.

**Offering IS enablement** — the ProseMirror dry-run lesson, inverted.
A command that cannot apply right now is simply not on the list:
`editor.collapse-carets` appears only while multiple carets or a
selection exist; a closed speed-search contributes no input commands.
There is no `update()` loop and no when-clause DSL — the focus walk is
the context, and it runs only when asked (a key the tree declined, a
palette open), never per frame.

Location-gated editor commands ride the same answer: the
`EditorCommands` registry (frontend/editor/src/dynamic.rs) holds
`DynamicEditorCommand`s with an `offers_at(&ResourceLocation)`
predicate, and the editor's `focus_data` folds the passing ones into
its offered list.

### 5.2 `DynamicCommand` — window-scoped, registered

```rust
pub trait DynamicCommand: Send + Sync {
    fn id(&self) -> &'static str;
    fn name(&self) -> String;
    fn perform(&self, app: &mut Application, store: &mut Store,
               window: crate::WindowId, fx: &mut crate::AppFx<'_>);
}
```

The `Commands` store singleton (frontend/himark/src/commands.rs) holds
them; core and plugins register at startup, alongside the panels and
languages they already register. These are the workbench verbs —
split, close, toggle a drawer, open search — that need a window, not a
focus context.

### 5.3 One resolution surface

`palette_commands(store, ui, window)` concatenates the focus walk's
offered commands with the registry — that Vec IS the command surface.
`AppExt::perform_registered(window, id)` resolves an id against it and
performs the hit; every id consumer goes through the same lookup:

- **The palette** (frontend/plugins/palette) lists the surface,
  subsequence-filters it, shows each entry's chord
  (`Keymaps::shortcuts_by_id`), and performs the pick.
- **The keymap** ([keymap.md](keymap.md)) maps chords to ids and fires only
  when the focus chain declined the key — an id nothing offers leaves
  the chord unconsumed.
- **The shells** call `himark_perform_command(engine, window, id)` —
  one FFI door; macOS menu items are one-selector wrappers over ids,
  not bespoke exported functions.

Because a `PresentableCommand` carries its payload pre-built, the
instant an id resolves, everything is a typed enum performing through
the ordinary `perform` spine — same store transaction, same effects,
same routing as a click.

### 5.4 What we deliberately do not build

- **Runtime command definition.** The command SURFACE stays compiled
  Rust plugins — names give runtime *invocation*, not runtime
  *definition*; Emacs's ceiling is not worth Emacs's floor for this
  product. (Workflow scripting exists — [scripting.md](../scripting.md) — but a
  script is invoked through one built-in command and orchestrates
  through capabilities; it does not register commands of its own.)
- **A when-clause DSL.** Offering along the focus path is the
  enablement model; a plugin that needs "enabled when the caret is in
  a fenced block" writes a Rust predicate reading the store,
  type-checked like everything else.
- **Global dispatch.** A focus-scoped id without a claimant does
  nothing; we never grow VS Code's "every command reachable from
  everywhere, guarded by context keys" surface.
- **String payloads internally.** Ids exist at the keymap/palette/FFI
  boundary only; no args bags cross into the views — a command that
  wants a parameter is offered pre-parameterized or registered as a
  distinct id.
