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
| Canvas scrolls with imba; typing scrolls it up | `Canvas.rows: ScrollView<CanvasRows>` — grandfathered out of descrollering | Descroller the canvas: bare rows, reveals out; workbench wraps in `Scrolled` |
| Chat fights scrolling + tool-call expansion | transcript band's anchor corrections ride its own scroll assumptions | Same descrollering; corrections ride reveals, host-applied; user gesture supersedes |
| goto-usages inlay doesn't scroll | offer road doesn't reach the inlay's inner ScrollView | One offer road; verify inner dispatch, fix routing not symptoms |
| Rail tree unscrollable until tab switch | host never re-applies measured content size after its own resize (fold → expand) | `DesktopScrollHost.layout` re-seeds from the last measure |
| Opened editor not revealed in file tree | tree-follow lives in workbench batch-tail only | follow runs in the engine batch tail over mounted trees too |
| Empty glass slab at small window (screenshot) | go-to/palette sheet renders contentless at squeezed heights | reproduce at that size; sheet needs a floor on minted content |
| Wrapped canvas line paints across the gutter | canvas editor width vs gutter inset at narrow panes | fix in the one editor band layout, not per-shell |
| Tree selection / chat background / send button / composer paddings | design drift | measured pass against the mocks, self-verified by screenshot |

## Order

1. Descroller `canvas` (then the chat band) — kills four bugs at the
   root and finishes the library law.
2. Offer-road routing to inner scrollers.
3. Host re-measure on resize; engine-tail tree-follow.
4. Design pass with mock-crop comparisons.

Each step lands only after a probe run (synthetic clicks + screenshot
or trace) shows the behavior — not after it compiles.
