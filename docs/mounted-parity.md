# Mounted parity: one road per behavior

2026-10-13. The desktop (islands) shell exposed a pattern: every
behavior the workbench window road did implicitly — scroll physics,
reveal application, focus moves, tree-follow — was re-implemented on
the mounted road as a hand-rolled twin, and each twin shipped with
its own bugs. This document is the inventory and the law for closing
them STRUCTURALLY: shared code, shells differing only in physics and
chrome.

## The law

A behavior lives in exactly ONE place:

- **Content views own NO scroll** (the descrollering law, now without
  exceptions). A view is content-sized under an infinite ceiling and
  asks for positioning via `EventResult::Handled { reveal, .. }`.
- **Scroll PHYSICS belong to the shell adapter**: `hikit::Scrolled`
  on the workbench pane road, the native scroller (NSScrollView /
  UIScrollView) on the mounted road. Both consume the SAME reveals.
- **Inner overlays keep their scroll** (completion popup, usages
  inlay): wheels are OFFERED to the engine first
  (`himark_frame_offer_scroll`); consumed means an inner scroller
  claimed, otherwise the shell's physics take the event.
- A command that can land windowless implements the mounted landing
  via the places vocabulary — never a parallel open road.

## Inventory (the 2026-10-12 bug list, mapped)

| Bug | Root | Shared mechanism |
| --- | --- | --- |
| Canvas scrolls with imba; typing scrolls it up | `Canvas.rows: ScrollView<CanvasRows>` — grandfathered out of descrollering | DONE — bare rows, reveals out; workbench wraps in `Scrolled` |
| Chat fights scrolling + tool-call expansion | tail-follow lived twice: dead engine `set_scroll_y` + per-host `pinsToBottom` twins | DONE — the list's standing FOLLOW MODE on the reveal road; only the hand releases it; host pins deleted on macOS + iOS |
| goto-usages inlay doesn't scroll | editor FocusGate ate positional events on unfocused inlays; the peek master had no scroller | DONE — positional events route by position; master in ScrollView, keys to the controller |
| Rail tree unscrollable until tab switch | host never re-applies measured content size after its own resize (fold → expand) | DONE — `DesktopScrollHost.layout` re-seeds from the last measure |
| Opened editor not revealed in file tree | tree-follow lives in workbench batch-tail only | DONE — `TreeCommand::Follow` is the one API; workbench pushes from batch tail, islands host from `land()` via `himark_tree_follow` |
| Empty glass slab at small window (screenshot) | macOS 26 mounts the titlebar GLASS platter over the content top under the default window style | DONE — `.windowStyle(.hiddenTitleBar)`; repro via `HIMARK_DEBUG_FRAME` |
| Wrapped canvas line paints across the gutter | STALE WRAP SPLITS: the inline face (pair-managed, skipped by the plain repair lane) never joined the pair repair either — after a rewrap, rows past the sync budget kept old-width splits (one clipped row + the tail at column 0) | DONE — the pair lane captures/lands the inline editor too; law test `a_wrapped_canvas_line_stays_off_the_gutter` |
| Tree selection / chat background / send button / composer paddings | design drift | DONE (db615a0d) — composer card + capsule Stop + islands selection well + ~23pt tree pitch + quiet refresh chip; screenshot-verified against mock crops |

## Presentation: one frame, or none

2026-10-13. A behavior living in one place is not enough if its
pixels and its geometry reach the screen in different frames. The
mounted road paints into a `CAMetalLayer` the shell is also moving,
so the same law applies in time:

- **A present is part of the transaction that moved the layer.**
  `presentsWithTransaction` is ALWAYS on (there is no "sync during a
  live scroll" mode to toggle); `skia_metal_end` commits, waits for
  scheduling, and presents on the calling thread. The band's new
  frame, its `drawableSize` and the paint that fills it are set
  inside ONE `CATransaction`, and a pass that painted nothing rolls
  the frame back rather than show old pixels in a new place.
- **An unpainted frame shows truth in the wrong amount, never a lie
  in the right one**: `contentsGravity = .topLeft`, because the
  default (`resize`) SCALES the stale drawable — the stretched text
  during a window drag.
- **Chrome animations move a clip, they do not re-propose content.**
  A SwiftUI spring on a bridge-backed view's own width re-lays the
  engine tree every frame; hold the content at one size and animate
  an outer frame that clips it (`RailIsland`).

Probe: `HIMARK_TRACE_SYNC` counts presents, those drifting from the
clip view's origin, deferred renders, rollbacks, and per-pass cost.
A scroll that went 65 uncoupled presents / 61 deferred before now
reports 0 uncoupled, 18 deferred, 0 drifted, 0 rolled back.

## Order

1. Descroller `canvas` (then the chat band) — kills four bugs at the
   root and finishes the library law.
2. Offer-road routing to inner scrollers.
3. Host re-measure on resize; engine-tail tree-follow.
4. Design pass with mock-crop comparisons.

Each step lands only after a probe run (synthetic clicks + screenshot
or trace) shows the behavior — not after it compiles.
