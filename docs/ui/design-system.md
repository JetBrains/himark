# The design system: size carries rank

himark's chrome is TYPOGRAPHIC. No glass, no pills, no boxes; a word
before an icon; rank by size and weight, never by fill; hairlines
separate, whitespace groups. Everything a surface draws comes from
two tables in the theme (the `ink` and `type_ramp` parts —
`theme.part(InkChrome::KEY)` / `theme.part(TypeChrome::KEY)`,
frontend/editor/src/theme.rs, values in
frontend/editor/assets/theme.json and theme-light.json) through the
roles in `hikit::ui`; a bare number or color in a view is a smell.

All metrics are PHYSICAL pixels for 2x displays: the design's 17px
title is `34.0` in the theme.

## Faces

- **Bricolage Grotesque** — one variable family (opsz 12–96, wdth
  75–100, wght 200–800) for every chrome size; bundled
  (frontend/editor/assets), registered with both text roads at boot,
  instanced per rung by `hikit::fonts::chrome_font(ui, size, weight, width)`.
  A font instanced at a weight/width SHAPES at it: `imba::layout::TextShaper`
  reads the axes off the typeface.
- **JetBrains Mono** — identifiers, keys, the margin
  (`hikit::fonts::mono_font`). The editor's own code face is
  untouched.

## The ramp (`type_ramp`, each a `TypeRole { size, weight, width, tracking, line_height }`)

| role | px / wght | where |
|------|-----------|-------|
| `query` | 88 / 300 | the palette's and the jump sheet's input |
| `query_small` | 60 / 300 | the Find tab's input |
| `headline` | 64 / 560, wdth 88 | the commit you're reading |
| `display` | 48 / 560 | a header's display-rank word (`ColumnHeader::display_button`) — no default caller since the tabs replaced the Sessions action |
| `prompt` | 44 / 520 | your messages; the composer |
| `title` | 34 / 640 | the file, the session, the active tab |
| `subtitle` | 28 / 400 | the title's quiet companion |
| `reading` | 30 / 400, lh 1.6 | the agent's prose, lists |
| `row` / `row_strong` | 28 / 400 · 560 | tree and list rows; a directory |
| `brief` | 26 / 400 | paths, metadata, counts |
| `label` | 22 / 600, +2.2 tracking, CAPS | group heads, field labels |
| `margin` | 22 mono | tool calls, keys, line numbers |

`hikit::ui::{query, query_small, headline, display, prompt, title,
subtitle, reading, row, row_strong, brief, caps, margin}` resolve a
role to a `TextStyle` (font, ink, tracking); `hikit::ui::text(&style, …)`
makes the label.

## The ink (`ink`)

| name | dark | use |
|------|------|-----|
| `ground` | #0D0D0F | the page |
| `rule` / `rule_strong` | #222226 / #2E2E33 | hairlines; the composer's baseline |
| `ink` / `ink_bright` | #ECEAE4 / #F2F0EB | primary; the current item |
| `ink_2` | #B5B2AB | secondary |
| `ink_3` | #8A8781 | labels, metadata |
| `ink_4` | #5F5D59 | the quietest: zero counts, hints |
| `live` | #F2B544 | the caret, the here-node, matched letters |
| `added` / `removed` | #7FD08A / #F07A6B | diff counts |
| `hover` / `selected` / `here` | #17171B / #1A1A1F / #16161A | row washes |
| `sheet` / `sheet_border` | #151518 / #2A2A2F | the one sheet (palette, jump) |
| `mono_dim` | #5D5D66 | line numbers |
| `declaration` | #5CC8FF | the current declaration name |

## The frame

Three columns on the ground, hairlines between: the chat (text edge
`window.chat_text_edge`, 176), the tree (`code_text_edge`, 160 — the
code column), the side column (`side_text_edge`, 100; `side_width`,
720). Every column has a `hikit::header::ColumnHeader`
([toolbar.md](toolbar.md)): its words on one baseline, a hairline
under. State is typographic: a directory is `name/`, an expanded
node's children hang under it, the current row is a flat wash, the
here commit's rail node fills LIVE.
