# IME and marked text

Text input on macOS is not "key → character." Dead keys, accent
composition, emoji, and every CJK input method compose through
**marked text**: a provisional, inline preedit string the user edits before
committing. The host's `NSView` implements `NSTextInputClient`; himark
serves that protocol across the embedding ABI. This is the one place the
event surface grows past what the Rust shells needed, so it is designed
with the ABI, not bolted on.

## The macOS reality (`NSTextInputClient`)

A `keyDown` does not become text directly. The view calls
`interpretKeyEvents(_:)`, which hands the event to the input context / active
input method, which then calls back into the view's `NSTextInputClient`:

| callback | meaning |
| --- | --- |
| `setMarkedText(_:selectedRange:replacementRange:)` | set/replace the composing text; `selectedRange` is the cursor *within* it |
| `insertText(_:replacementRange:)` | commit final text (ends composition) |
| `unmarkText()` | commit whatever is marked, clear composition |
| `hasMarkedText()` / `markedRange()` | is there composition, and where |
| `selectedRange()` | current selection/caret |
| `firstRect(forCharacterRange:actualRange:)` | screen rect of a range — where the candidate window docks |
| `characterIndex(for:)` | a screen point → character index |
| `attributedSubstring(forProposedRange:actualRange:)` | text content for a range |
| `doCommand(by:)` | editing commands (arrows, delete) routed from keys |

himark answers all of these for the **focused editor** — the engine knows
which seat has keyboard focus, so IME calls carry no editor id, only the
window.

## The contract

Two directions across the ABI. Host → himark drives composition; himark →
host answers the queries the input method needs (chiefly to place its
candidate window):

```c
// host -> himark: composition
bool himark_set_marked_text(engine, window, utf8, len, selected, replacement);
bool himark_unmark_text(engine, window);
// commit is the existing text-input entry: it replaces any marked range
void himark_text_input(engine, window, utf8, len);
// arrows / delete during composition are the existing movement commands

// himark -> host: queries
bool      himark_has_text_focus(engine, window);   // whether to engage the input context
bool      himark_marked_range(engine, window, &range);
bool      himark_selected_range(engine, window, &range);
bool      himark_set_selected_range(engine, window, range);
bool      himark_document_length(engine, window, &len);
bool      himark_first_rect(engine, window, range, &rect); // window coords
int64_t   himark_char_index_at(engine, window, x, y);
uintptr_t himark_substring(engine, window, range, out, cap); // two-pass
uintptr_t himark_selection_rects(engine, window, range, out, cap);
bool      himark_reveal_selection(engine, window);
```

### Indices are UTF-16 here, deliberately

Everywhere else himark speaks UTF-8 byte offsets — its native coordinate.
`NSTextInputClient` speaks `NSRange` in **UTF-16 code units**. Rather than
push that conversion onto Swift (which would need the text to do it), the
IME calls take and return **UTF-16 indices**, and himark converts to its
byte offsets internally — it owns the text and the mapping. This is a
scoped exception, limited to the IME surface, so the Swift side stays a
direct transcription of the platform protocol.

### Coordinates

himark lays out the whole UI into the canvas, scroll included, so it knows
the caret's position in **window/canvas space**. `first_rect` and
`char_index_at` speak window coordinates; the host applies the one affine it
owns (view → screen) for the candidate window. himark never needs to know
the window's screen position.

## The ask road: name the seat, then recognize it

The IME is the one focus question that needs GEOMETRY, so it is
answered in two steps that share one identity ([UI.md](UI.md), Focus):

1. the semantic focus walk (`View::focus_data`) names the focused
   text seat — an opaque `SeatKey` the owning editor minted;
2. `with_ime_client` builds ONE bounded widget frame and folds
   `Widget::layout_data(target)` over it: containers are dumb folds
   that translate and clip like paint, and the editor leaf answers
   its `ImeSeat` (caret origin, clip, and the ask closure) iff its
   own key IS the target.

The fold recognizes the key; it never re-decides focus — the walk and
the fold cannot diverge because they share the one value. The seat is
a closure that builds its `ImeClient` on the stack, runs the host's
visitor against it, and answers the stashed mutation as an ordinary
command — call-locality: a client never outlives an ask, and this
build-to-ask fires only while composing. The clipboard and the
focused location ride the walk alone (no geometry, no build —
[clipboard.md](clipboard.md)).

## Where marked text lives

Preedit is a REAL document edit: `setMarkedText` replaces the marked
range through the ordinary edit door, and the marked RANGE is
per-editor transient state (`Editor::marked`) on the one composing
editor. Reusing the edit/layout/render machinery is what makes
composition free everywhere text works; the consequences are handled
where they bite:

- **undo**: composition steps COALESCE — while any editor of the
  document is composing, consecutive edits merge into one undo entry,
  so undo never walks through half-typed preedit; undo/redo are
  refused mid-composition (the IME owns the buffer until commit).
- **shared documents**: a document shown in several editors shows the
  preedit in siblings too — the accepted trade-off of preedit-as-edit.
  (The alternative — a per-editor overlay laid out only in the
  composing editor — would need the editor's layout to include text
  the document does not contain; the ABI would not change if that
  ever becomes worth its cost.)

The composing editor draws the marked text inline, **underlined** (the
standard preedit affordance), with the composition caret placed by the
IME's `selectedRange`. Focus can be inside an inlay editor (a table
cell); the marked state lives on whichever editor is focused —
top-level or inlay — because the seat rides the focus walk either way.

## iOS: the same seat carries `UITextInput`

macOS asks the seat for composition; iOS asks it for more. UIKit hands
the keyboard, dictation, autocorrect and the selection gestures only
to a view implementing `UITextInput`, so the iOS shell implements it
(`Sources/iOS/HimarkTextInput.swift`) against this same ABI, and the
canvas keeps its own rendering and model. The protocol needed answers
`NSTextInputClient` never asks for, and they are on the seat:

- `set_selected_range` — a mutation like any other: it collapses to
  one selecting caret and runs the caret-line unhide, but never arms
  the reveal (a grab-handle drag must not fight the pane's scroll).
  Multicaret is the engine's own; the platform protocols carry one
  range.
- `selection_rects` — the query twin of the paint pass's per-line
  selection walk, kept a QUERY by two budgets: clipped to the
  editor's last painted band plus a screen of slack, and capped at
  256 rows — a select-all over a 100k-line document must not shape
  every line synchronously, and the host only draws what shows.
- `document_length`, `reveal_selection` (the keyboard rose — bring
  the caret back through the standing scroll-to-caret road).

The FFI mirrors the house conventions: UTF-16 ranges in,
physical-pixel rects out, the two-pass array convention for the rect
list. The Swift side is a transcription: positions clamp against
`document_length`, geometry divides by the layer's scale with **no
y-flip** (UIKit is top-left like the engine, unlike AppKit), and
every rect answered to UIKit is checked FINITE — a `.null`/infinite
caret rect handed to the interaction machinery is a crash, so absence
answers absence, never infinity. The keyboard toolbar rides
`keyboardLayoutGuide`, so the editor shrinks when the keyboard rises.

Selection GESTURES ride `UITextInteraction(for: .editable)`, with a
hit-test arbitration (`interactionShouldBegin`) deciding whether a
touch is text's or the app's: the view's own tap stays the universal
click road (tree rows, gutter, pane focus) and tells the
`inputDelegate` when the engine moved the caret behind UIKit's back.
This seam is the surface's bounding constraint, worth stating
plainly: `UITextInteraction` assumes ONE text control owning ONE
document whose content, selection and geometry hold still between its
callbacks, while himark has many text surfaces, engine-owned focus
that retargets underneath UIKit, and virtualized layout that
legitimately has no geometry answer at times. The budgets, the
finite-rect rule and the arbitration keep the contract satisfiable;
anything UIKit asks uninvited beyond it is answered with the honest
fallback rather than a guess.

Autocorrect and system spell checking stay off: they are wrong in
code panes, and turning them on wants a language gate (the focused
document's syntax) that does not exist on this boundary. Dictation
and predictive text ride the protocol regardless.
