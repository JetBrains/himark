# Lazy languages

The grammar problem, measured: the web binary was ~97 MB
stripped, and ~95 MB of it was the 49 statically linked tree-sitter
grammars — ~90 MB of parse-table DATA alone. On native the tables are
demand-paged (near-free), but every shell still paid 49 eager
highlights-query compilations at startup.

## The registry

`SyntaxLanguages` entries are lazy (`register_lazy`): a name list, a
`OnceLock` residency slot, and a loader. Entries ride `Arc`s, so
registry clones share residency. Three asks with three contracts:

- `knows(name)` — the ROUTING check (does this extension open as
  code?). Never loads.
- `get(name)` — the RESIDENT language. Never loads. UI-thread callers
  (type assist, the chat diff-side router) use this: an unloaded
  language simply answers `None` there.
- `ensure(name)` — loads through the entry's loader, first ask only; a
  `None` answer (failed fetch) is NOT cached, the next parse retries.
  WORKER-ROAD ONLY: `parse_syntax` calls it, and every parse site (the
  reparse lane, the document build effects) runs on workers. A
  per-entry mutex keeps two documents racing one cold language from
  loading it twice.

Eager `register` remains for languages in hand (markdown, tests).

## The plugins

Every lang plugin registers through `hisitter::register_grammar!`,
which cfg-splits per target:

- NATIVE: the grammar crate stays statically linked; the loader defers
  `TreeSitterLanguage::new` — the query COMPILATION, the real startup
  cost — to the first parse of that language.
- EMSCRIPTEN: the `language:`/`highlights:` expressions are cfg-erased
  and the grammar crate sits under
  `[target.'cfg(not(target_os = "emscripten"))'.dependencies]` — the
  grammar never enters the binary. The loader is
  `hisitter::fetch_side_grammar`.

The macro also records a `SideGrammar` descriptor (module name, dlsym
symbol, crate name, parser dir, and — natively — the highlights text
as a closure), which is what the build reads.

## The wasm side modules

`apps/web/tools/build-grammars.sh` (run by `build-web.sh` when the pack is
missing or `--rebuild-grammars` is passed):

1. `cargo run -p grammar-pack -- target/web/grammars` — a NATIVE bin
   that registers every plugin and writes `<module>.scm` from each
   registration's own highlights closure (single source of truth — the
   served query can never drift from what native compiles) plus
   `manifest.tsv`.
2. For each manifest row, the vendored crate sources (resolved through
   Cargo.lock) compile with
   `emcc -O2 -pthread -sSIDE_MODULE=1`. The build hashes each side module and
   query, stages them as `<module>-<sha256>.wasm` and
   `<module>-<sha256>.scm`, and writes their stable-module-to-filename mapping
   to `assets.tsv`. A `build-profile` marker prevents a release build from
   reusing an unoptimized pack produced by `debug-web.sh`.

The main module links with `-sMAIN_MODULE=2 -sALLOW_TABLE_GROWTH=1`
and an `EXPORTED_FUNCTIONS` list naming the libc surface scanners
import (mem*/str*/isw*/malloc family). A symbol missing from the list
surfaces BY NAME in the dlopen error — extend the list in
build-web.sh, don't guess.

At runtime, `fetch_side_grammar` (worker-only; it refuses the main
browser thread) fetches and caches `grammars/assets.tsv`, resolves the stable
module name, then fetches its fingerprinted `.wasm` + `.scm` relative to the
page (the `himark_tkn` cookie carries auth) through the
`side_fetch.c` shim — `emscripten_fetch` in `SYNCHRONOUS` mode, the
one blocking fetch that needs no ASYNCIFY and is allowed exactly on
worker threads (`-sFETCH` at link; the wget family is asyncify-only
and ABORTS the runtime from a pthread — a pthread abort taught this
rule) —
writes MEMFS, `dlopen`s, `dlsym`s the `tree_sitter_*` symbol, and
builds the `Language` through `LanguageFn::from_raw`.

## What the user sees

First open of a language: the build effect blocks its worker on one
fetch (~100 KB–11 MB per grammar, one time) and the document opens
fully parsed — no re-root machinery, no plain-text flash. Every later
open is resident. A fenced code block in markdown loads its language
the same way, inside the reparse worker. If the fetch fails the
document opens plain and the next parse retries.

Serving: the grammar pack lives beside the web root
(`target/web/grammars/`), rides the same static file road
(`agent-host` httpServe), and is copied into the mac app bundle with
the rest of `target/web`.

## Compression

The main module is staged as `web-<sha256>.wasm`, and both `index.html` and the
Emscripten loader are rewritten to request that content-addressed name. Lazy
grammar modules and queries use the same naming scheme through `assets.tsv`.
The agent host gives fingerprinted `.wasm` and `.scm` assets a one-year
immutable cache lifetime; unhashed manifests and loaders continue to revalidate.

The build compresses every big artifact, including the web font, into
a `.gz` sibling and, when the `brotli` command is available, a `.br` sibling
(build-web.sh's last step). A complete grammar pack in
`target/web/grammars` is reused without rebuilding or recompressing it;
pass `--rebuild-grammars` to explicitly regenerate both the originals and
their compressed siblings. `serve_static` negotiates those files from
`Accept-Encoding`, always preferring Brotli when available, and sends
the sibling verbatim with the matching `Content-Encoding`. Parse tables
compress ~4–10x, so the wasm goes over the wire at a fraction of its
size. `index.html` never rides this road (it is rewritten per request
with the injected AHP URL), and clients accepting neither encoding get
the originals.
