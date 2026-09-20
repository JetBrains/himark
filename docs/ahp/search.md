# Find surfaces

The workspace search panel that used to live here — occurrence rows
as live bounded editors over eagerly fetched documents — is retired.
Text search and LSP reference/implementation results are LOCATION
LISTS now: streamed over `ahp-locations:/…` channels
(docs/ahp/ahp-locations.md) into the Search dock tab and the go-to
peek, rendered from each location's own context with no document
fetched before navigation. The design and its history are
docs/ui/location-list.md; the wire truth is docs/ahp/ahp-locations.md
and docs/ahp/ahp-search.md.

What remains here is the one find surface that never left the
document: the in-document find bar.

## The in-document find bar

cmd-F drops a bar across the top of the focused editor pane — a
query input plus an occurrence count; shift-cmd-F opens the Search
dock tab. Enter / cmd-G walk forward, shift-Enter / cmd-shift-G back
— each step SELECTS the occurrence and reveals it through the
ordinary scroll machinery (`Document::reveal_selecting`). Escape
dismisses.

The bar lives on the PANE SLOT (`PaneSlot.find`), like navigation
history: it survives panel displacement — switching files keeps the
bar and re-scans the new document — and never touches `Panel`. Its
tints are one feature markup on the target document, `StyleId::Match`
ranges (docs/editor/markup.md), shown on the pane's editor only (two
panes over one document each carry their own bar); the producer
brings the change set (old ∪ new occurrence ranges) to every
`replace_markup` swap. Scanning is case-insensitive, the query as a
regex when it parses and its escaped literal otherwise, over
line-aligned 64KB windows, capped at 20k occurrences.

Focus follows the shield pattern: the bar wraps its input in
`focus_scope(focused)` and its keymap leaf is placed last (tried
first); a click into the pane text hands the keyboard back without
closing the bar, and cmd-F while open refocuses with the query
selected. Every slot command funnels through one sync, so query
edits, pane edits, and displacement all converge on fresh tints; the
walk restarts from the current occurrence after each re-scan.
