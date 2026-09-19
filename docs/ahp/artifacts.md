# Markdown documents: collaborating with agents

The position this document cashes in: chat is mainly an input,
floating over the surface; the main interaction with AI happens in
markdown — plans and designs in, walkthroughs and
paper-trail out. The load-bearing choice is that there is no artifact
machinery at all: **a shared document is an ordinary markdown FILE in
the session**, and every collaboration mechanism below is a standing
seam that works on any file. The agent reads and writes with its
ORDINARY tools — no new tool vocabulary, no provider-specific API —
and a terminal CLI in the same session sees the same files (the
CLI-interop rule, docs/ahp/agent-host.md). Persistence is free: the paper
trail IS the files. A dedicated protocol surface (a standalone
`openDocument` channel with no backing uri) was rejected for exactly
this reason: it would be invisible to the agent's tools and to the
terminal, and would need its own persistence story.

Three mechanisms carry the whole story:

## Co-editing rides documents@1

A registered, non-synthetic document gets a document channel at
registration (`DocsyncHook`, docs/editor/documents.md; the loop itself is
docs/editor/rebase.md). The host mirrors the file and is its reload
authority: the agent rewrites the file on disk with a tool, the host
diffs the disk state into the live channel as its own operations
(`backend/agent-host` — `watch_mirror`/`reload_mirror`, a three-way
hunk diff), and the user's in-flight edits rebase over them. No lock,
no turn-taking, no conflict dialog: the user edits a plan WHILE the
agent streams into it, and both converge. Saves flow through the same
channel (the `storeDocument` request — docs/ahp/files.md), so a save
never masquerades as a foreign edit to the host's watcher.

## Feedback rides annotations

A comment card on a markdown document is the standing comment card
(docs/ahp/comments.md): anchored to a range, synced over the session's
annotations channel, visible to every client. Sending it to the agent
is the standing delivery leg — `comments.send` groups cards into one
turn per session and the host expands them into the prompt as a
`<review-comments>` block. The review loop — select a passage,
object, send — costs no vocabulary beyond what every file already
has.

## Embeds are fence-ADDRESSED editors, not rendered snippets

Markdown grows ONE small extension: a file path after the language in
a fence's info string. In himark the fence auto-replaces with an
Instead widget (`InlayMode::Instead`, built by the enrichment pass —
docs/editor/editor-enrichment.md, `himarkdown::fence_embed`) that IS a real
editor over the referenced location — registered through the ordinary
register-at-display door, deduped by `by_location`, so it is THE same
document a pane showing that file holds, with the same syntax colors
and the same diff stripes. No diff vocabulary exists in the fence
because none is needed: diffs are already the editor's. The fence
BODY is the excerpt a foreign renderer (GitHub, a terminal) shows;
himark shows the live file instead:

````markdown
``` c++ path-to/some/file.cpp
void foo ()
```
````

A line fragment on the path (`file.cpp#L10-40`) scopes the embed to
an excerpt; a bare path embeds the whole file. The path resolves
relative to the embedding document's own location. A resolved range
is a LIVE ANCHOR, not a number: at load the embed mints a
fragment-set interval on the target document (shifting anchors,
edit-door maintained — docs/editor/markup.md), so the shown window follows
the target's edits in place; the written fragment is only the cold
spelling, resolved at (re)load. A fence naming a missing or
unreachable file keeps its plain body.

## The boundary

Markdown links are decorated (`StyleId::Link`) but carry no
navigation; embeds are addressed by path and line fragment only —
outline-anchor fragments and write-back healing of drifted `#L…`
spellings are not built; and no convention layer (well-known file
names, provider instructions about what to write where) exists in the
host or client. What agents and users share is exactly what the
session's filesystem, the document channels, the annotations channel
and the fence embeds provide — every one of them a mechanism over
plain files, none of them artifact-specific.
