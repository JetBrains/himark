# Clipboard

Copy, cut and paste — without himark ever touching a pasteboard. The
clipboard is the most platform-entangled surface there is (NSPasteboard
and its change counts, Wayland selections, the web's permission-gated
`navigator.clipboard`, sandbox rules on all three), so the design is
the IME design (docs/ui/ime.md) applied again: **the HOST owns the
platform half and initiates every operation; himark answers through a
client interface reached down the focus path.** himark never binds
cmd-C — it could not serve it if it did.

## 1. The shape: a seat on the focus walk

The clipboard is a SEAT on the semantic focus walk — the `clipboard`
field of `FocusData` (docs/ui/UI.md, Focus) — not an event. Whatever
holds focus — an editor pane, the search field, a terminal, a list
row — installs a seat; nested widgets forward or override, and the
innermost seat wins. The engine has no "the focused editor" global,
and none is needed: the walk IS the resolution, exactly like IME and
key presses.

```rust
// imba::clipboard
/// What crosses the boundary. A struct, not a String: rich forms
/// (html/rtf beside the text, file lists) can grow without touching
/// the routing.
pub struct ClipboardContent {
    pub text: String,
}

pub trait ClipboardClient {
    /// The focused component's selected content — `None` when there
    /// is nothing to copy (no selection, or not a copy source): the
    /// host leaves the pasteboard alone.
    fn copy(&mut self) -> Option<ClipboardContent>;

    /// Copy plus removal. Components that cannot remove (a list row,
    /// a terminal) answer as `copy` does.
    fn cut(&mut self) -> Option<ClipboardContent>;

    /// Accept content at the focus. `false` = not a paste target.
    fn paste(&mut self, content: &ClipboardContent) -> bool;
}
```

The app asks through `Application::with_clipboard_client(window, f)`
(`frontend/himark/src/focus.rs`): it runs the focus walk, hands `f`
the innermost seat's client, and performs whatever command the visit
produced. Like the IME client, **mutations stash commands**: the
widget's client records what `cut` or `paste` means for it and
answers with an ordinary [`EditorCommand`]/plugin command — every
mutation lands through the same dispatch as typing, one undo entry,
change notifications and all.

## 2. Who answers, and with what

- **The editor** — `EditorClipboardClient`
  (`frontend/editor/src/editor_view.rs`), installed on the
  `EditorFocus::Text` arm of `EditorView::focus_data` and forwarded
  through the `Inlay` arm, so an embedded editor's seat surfaces
  through its host:
  - `copy`: the caret selections' text, non-empty selections joined
    with `"\n"` in caret order; `None` with no selection anywhere.
  - `cut`: the same text; the stashed command is
    `EditorCommand::DeleteSelections` (selection-only — `Backspace`
    would eat a character at collapsed carets in a mixed
    multi-caret); multi-caret stays one bulk edit.
  - `paste`: stashes `EditorCommand::Paste { text }`.
- **The terminal**: `PtyPaste` (`frontend/himark/src/terminal.rs`) —
  `paste` writes to the PTY session; `copy`/`cut` answer `None` (the
  grid has no selections).
- **Unified diff and list rows** forward the seat down to whatever
  editor or input is focused inside them.

`EditorCommand::Paste` exists BESIDE `InsertText` for one reason:
paste is plain text insertion and never a type-assist `Typed` consult
— a pasted `(` must not auto-close (docs/editor/type-assist.md). There is no
markdown-aware paste anywhere; the one place the two commands meet is
table cells, which treat `InsertText` and `Paste` alike.

## 3. The host halves

### Engine surface

```rust
impl Application {
    /// Hands the focused component's client to `f`; `None` when
    /// nothing on the focus path installed a seat.
    pub fn with_clipboard_client<R>(
        &mut self,
        window: WindowId,
        f: impl FnOnce(&mut dyn ClipboardClient) -> R,
    ) -> Option<R>;
}
```

### ABI (frontend-host), the `himark_substring` buffer convention

```c
// Host → himark, host-initiated (a menu item, cmd-C/X/V, a browser
// clipboard event). Copy/cut fill the caller's buffer with UTF-8 and
// return the full byte length (call with a null buffer to size, like
// himark_substring); 0 = nothing to copy — leave the pasteboard be.
size_t himark_copy (Engine, uint64_t window, char* out, size_t cap);
size_t himark_cut  (Engine, uint64_t window, char* out, size_t cap);
// True = consumed (repaint); false = no paste target at the focus.
bool   himark_paste(Engine, uint64_t window, const char* utf8, size_t len);
```

Cut is side-effecting under the two-call sized-string protocol, so it
runs ONLY on the sizing probe — the deletion happens once and the
text parks in `pending_cut`; the fill call drains the stash instead
of cutting again.

### Shells

- **macOS**: `copy(_:)`/`cut(_:)`/`paste(_:)` responder selectors on
  `HimarkView` (the Edit menu and cmd-C/X/V route there natively)
  call the ABI; copy/cut results go to `NSPasteboard`, paste reads
  `.string` off it. The engine never sees cmd-C as a key.
- **winit shell**: cmd/ctrl-C/X/V ARE plain keys there — the shell
  intercepts them before the key path and calls the same three entry
  points against a process-local pasteboard (a static `Mutex<Option<
  String>>`). An unconsumed ask FALLS THROUGH: copy on a terminal
  answers `None`, so ctrl-C still travels the plain key path and
  interrupts where ctrl is the primary modifier. The OS clipboard is
  a listed extension for this dev shell.

Tests need no host at all: `with_clipboard_client` is callable
directly — copy in, assert text; paste in, assert the document.

## 4. What deliberately does NOT exist

- **No engine-side clipboard storage.** The pasteboard is the host's;
  himark holds no shadow copy (no staleness, no sandbox surprises, no
  cross-app divergence).
- **No cmd-C in the engine keymap.** Host-initiated only — on macOS
  the key never reaches the engine anyway; binding it elsewhere would
  fork behavior per platform.
- **No format negotiation.** `ClipboardContent` carries plain text;
  html/rtf/file-list fields are additive when a producer and a
  consumer exist.
- **No dedicated clipboard event.** The seat rides the unified
  focus-data visit; there is no separate routed event to keep in sync
  with it.

**Listed extensions**: list-row label/path copy, terminal grid
selections + copy, bracketed paste, rich content forms,
paste-splitting across matching caret counts, the web shell's DOM
`copy`/`cut`/`paste` events.
