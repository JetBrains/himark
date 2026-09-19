# Keymap: chords to command ids

Shortcuts are DATA: a JSON file mapping key chords to command ids —
the same ids `PresentableCommand` and `DynamicCommand` already carry —
consulted by the engine exactly when the focus chain did NOT consume a
key. The alternative is what every multi-shell editor grows by
default: an accelerator table per shell, menu keyEquivalents on macOS,
and duplicated chord arms inside the editor's own key handler — three
tables, three drift surfaces, zero user control. One table, one
namespace.

## 1. The one-namespace insight

The dispatch surface already exists. `palette_commands(store, ui,
window)` (frontend/himark/src/commands.rs) aggregates the focus path's
`PresentableCommand`s — each with a stable `&'static str` id,
collected by the semantic focus walk ([UI.md](UI.md), Focus) — plus the
`Commands` registry of `DynamicCommand`s, and
`AppExt::perform_registered(window, id)` (app_ext.rs) resolves an id
against that surface and performs it. The keymap adds nothing to
command identity: it is a chord → id table, resolved through the SAME
lookup the palette pick uses.

This buys contextual binding for free: `cmd-z` maps to `"editor.undo"`,
and the id resolves only when an editor on the focus path OFFERS it.
No focus predicates in the keymap, no per-surface tables — offering is
the context, exactly as it is for the palette.

## 2. The file

`frontend/himark/assets/keymap.json`, embedded via `include_str!` —
the theme recipe verbatim (embedded default, serde parse to
`Result<_, String>`, no disk read). A flat object; a representative
slice:

```json
{
    "cmd-p": "peeker.toggle",
    "cmd-shift-p": "palette.toggle",
    "cmd-z": "editor.undo",
    "cmd-w": "workbench.close",
    "cmd-shift-w": "workbench.close-pane",
    "cmd-f": "find.open",
    "cmd-shift-f": "search.open",
    "cmd-t": "toc.toggle",
    "cmd-e": "files.tree",
    "cmd-[": "navigation.back",
    "cmd-]": "navigation.forward",
    "alt-left": "editor.move-word-left",
    "alt-shift-left": "editor.select-word-left",
    "backspace": "editor.backspace",
    "enter": "editor.newline",
    "escape": "editor.collapse-carets"
}
```

The full map also carries the editor's entire key surface — movements,
deletion, indent, caret ops — because the editor owns no key table of
its own (§6).

**Chord grammar**: dash-separated, case-insensitive on parse; zero or
more modifiers (`cmd`, `ctrl`, `alt`, `shift`, any order) then ONE
key. Keys: a single character (stored lowercased — shift is always the
explicit modifier), or a name: `enter`, `tab`, `escape`, `backspace`,
`delete`, `space`, `up`, `down`, `left`, `right`, `home`, `end`,
`pageup`, `pagedown`, `f1`…`f12`. Unknown key or modifier → the parse
error names the entry. Duplicate chords: serde's map keeps the last —
accepted, not detected.

## 3. The value

`frontend/himark/src/keymap.rs` — himark owns it (the table maps to
COMMAND IDS, a workbench concept; imba stays policy-free, its `Key`
and `Modifiers` are just the coordinates):

```rust
/// One chord, normalized: Char keys lowercased (shells disagree on
/// case under shift), shift always in `mods`.
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct Chord { key: imba::event::Key, mods: imba::event::Modifiers }

pub struct Keymap { bindings: HashMap<Chord, Arc<str>> }

impl Keymap {
    pub fn embedded() -> Self;                       // include_str! + expect
    pub fn from_json(&str) -> Result<Self, String>;  // the theme shape
}
```

Store singleton `Keymaps` beside `Themes`: `Keymaps::binding_of(store,
key, mods)` answers the bound id, falling back to `embedded()`;
`Keymaps::set` installs a replacement map — the override door. No
worker mirror — the keymap is UI-thread-only.

The table also runs in reverse: `Chord::display` formats a chord back
to its glyph form (`⌘⇧T`), and `shortcuts_by_id` hands the palette its
shortcut column — the display never drifts from the binding because it
IS the binding.

## 4. Dispatch: the focus chain first, the keymap after

One hook, in the app's `dispatch_event`
(frontend/himark/src/app.rs) — keyboard events route through the
SEMANTIC focus chain (`focus::window_focus_data`, a state walk over
the views; no widget tree is built for a keystroke), and the keymap
sees only what that chain declined:

```
Event::KeyDown ignored by the focus chain (Ignored, or Reveal-only)
  → Keymaps::binding_of(store, key, normalized mods)?     // O(1)
  → the focus chain's offered commands .find(|p| p.id == id)
      else Commands registry .find(id)                     // the palette's resolution
  → perform_batch(command)                                 // consumed, repaint
```

- **Ad-hoc handling stays first**, by construction: the keymap only
  sees what the focus chain declined. The terminal keeps eating raw
  keys, modals keep their Escape, speed-search keeps its arrows —
  every claimant's `focus_data` answers before the table is consulted.
- An id nothing currently offers → the chord stays unconsumed. A
  binding to a command that gates on context (editor-only,
  host-capability-only) simply goes quiet outside it.
- Cost: the `HashMap` probe is per-ignored-KeyDown; a chord HIT walks
  the focus state once more to collect the offered commands. Plain
  typing arrives as `TextInput`, never touching this path.
- `Key::Char` lookups normalize to lowercase, matching the stored
  form; a performed binding reports consumed like any performed
  command.

The `Ignored`/`Handled` distinction is load-bearing here: the keymap
fallback fires on `Ignored` alone, and the macOS shell runs its
text-input path only for keys the engine did not consume. A layer that
blanket-converts declined events to `Handled` would silence both — the
keymap and plain typing in every overlay input.

## 5. The shells own no shortcut tables

- **winit**: no editor-command accelerators. `shortcut_command` covers
  only host-level affordances (the demo documents, the instructions
  page); every editor chord flows to `key_down_at` and resolves
  through the keymap. The cmd/ctrl-C/X/V clipboard interception stays:
  that is pasteboard integration, not a command shortcut.
- **macOS**: the menu keeps its NSMenuItem keyEquivalents — same ids,
  same result, and the menu is the native discoverability surface.
  They shadow the keymap for the chords they claim (the menu fires
  before keyDown); the keymap covers everything else. `keyDown` itself
  is uniform — no modifier-conditional routing: unless composition is
  active (marked text owns the keyboard), EVERY key is offered to the
  engine first as key + modifier bits (AppKit's function-key scalars
  and control ASCII translate to HIMARK_KEY_*, printable scalars pass
  as themselves); only unconsumed keys fall to `interpretKeyEvents`,
  whose NSTextInputClient callbacks type text and whose selector table
  serves the composition path.
- **iOS / web**: hardware-keyboard chords flow to `key_down` and
  resolve against the same table — no per-shell work at all.

## 6. The editor owns no key table either

There is no editor `key_command` arm: every key the editor answers —
arrows, Backspace, Enter, Tab, Home/End, Escape, the movement set — is
a `PresentableCommand` on the focus path (`caret_surface` in
frontend/editor/src/editor_view.rs), movements as `editor.move-*` /
`editor.select-*` pairs: a chord is exact, so a selecting motion is
its own id and the payload carries the shift. The default chords live
in keymap.json beside the command chords — palette-visible,
rebindable, and open to emacs-style alternatives (`ctrl-a` →
`editor.move-line-start` is one user line). Escape collapses carets by
conditional OFFERING, not conditional handling:
`editor.collapse-carets` is on the list only while multiple carets or
a selection exist, so the chord stays unconsumed otherwise and bubbles
outward.
