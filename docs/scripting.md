# Scripting

himark's compiled automation surface — enrichment passes, named
commands, panels — is powerful, typed, and sealed. The AI-workflow
shapes (docs/ahp/artifacts.md: plans, walkthroughs, session summaries,
documentation updates) want the opposite lifecycle: a user — or an
AGENT — writes a small program *in himark itself* and it orchestrates
documents and agent turns for THIS repo's conventions. Nobody
recompiles the editor to teach it "after every session, append the
walkthrough to docs/CHANGES.md".

The engine is a detail; the load-bearing fact is that **a script is
an effect-worker citizen**: every hard problem (sandboxing, async
agent turns, UI-thread discipline, cancellation) is one the effect
system already solved for enrichment (docs/editor/editor-enrichment.md) and
the host boundary (docs/ahp/host.md).

```js
export default async function walkthrough(himark) {
  const diff  = await himark.vcs.changes();
  const prose = await himark.agent.ask(
    `Narrate these changes as a walkthrough:\n${diff}`);
  await himark.docs.write("docs/session.md", prose);
  await himark.docs.show("docs/session.md");
}
```

## Requirements

1. **Every platform.** Native (macOS/winit) and the emscripten web
   shell. On web, binary size is a measured, fought-over constraint
   (docs/editor/lazy-languages.md) — the engine must be small or
   side-loadable.
2. **Full sandboxing.** A script gets NO ambient authority — no fs,
   no net, no process. Runaway scripts (loops, allocation bombs) must
   not take the app down or freeze the UI.
3. **Authored in himark.** Source lives in the workspace, edited in a
   himark pane. No external toolchain, no compile step — which rules
   out compile-to-WASM plugin schemes however good their sandboxes
   are.
4. **AI-workflow shaped.** Scripts orchestrate: read documents, call
   agent turns (minutes-long, async), transform markdown, write
   documents. And the AUTHORS are often agents — the language must be
   one models write fluently.

## The engine: quickjs-ng (JavaScript)

**QuickJS**, the maintained `quickjs-ng` fork, bound through
`rquickjs`, vendored and compiled with `cc` like hisitter and
mdparser compile their C. `frontend/hiscript` builds it natively AND
under `wasm32-unknown-emscripten` with the app's exact web recipe;
the bare engine builds with `--no-default-features`, so the
small-binary contract is pinned by its own build.

What decides it is requirement 4. Scripts here shuttle JSON — AHP
payloads, tool arguments — and await long agent turns; and they are
routinely WRITTEN by models:

- **JSON is native.** Lua needs a library and carries the
  `{}`-is-both-array-and-object ambiguity, which bites every agent
  round trip. JS just parses the protocol.
- **`async`/`await` is the turn story.** An agent ask suspends the
  script for minutes; await is the notation both humans and models
  write correctly on the first try.
- **LLM fluency.** Models produce working JS an order of magnitude
  more reliably than Rhai/Rune/Koto and meaningfully better than
  Lua. For a system whose scripts are agent-authored, this is a
  feature requirement, not a taste.
- **Types for free.** hijavascript/hitypescript grammars are
  registered, so scripts get highlighting, folding and outline;
  authoring is JS.

Sandbox knobs are built in: `JS_SetMemoryLimit` (per-runtime
allocation cap), `JS_SetMaxStackSize`, `JS_SetInterruptHandler` (the
deadline/cancellation hook). QuickJS proper has no fs/net/process —
those live in the OPTIONAL qjs-libc layer, which we simply do not
link: `std`/`os`/`require`/`process`/`fetch` are all undefined
inside a run, and the native test battery pins it (the memory cap
kills an allocation bomb; the interrupt handler kills `for(;;){}`;
promises drain through the job queue; no ambient authority exists).

### The road not taken

- **Lua 5.4** (~250 KB, mlua): the smallest engine, and coroutines
  map beautifully onto effect parking. Loses on JSON, on agent
  authorship, and its C error handling (longjmp) is the classic
  emscripten friction point. The runner-up.
- **Luau**: the best-designed sandboxed Lua (built for untrusted
  scripts, per-VM interrupts and memory caps). But it is C++ and
  BIGGER than QuickJS — it loses on both axes that would justify
  choosing Lua at all.
- **Rhai** (pure Rust): trivially portable, sandboxed by
  construction — and no coroutines. Orchestrating async turns means
  hand-rolled CPS. Disqualifying for requirement 4.
- **Rune** (pure Rust, native async): technically the elegant
  answer; ecosystem and model fluency near zero.
- **Piccolo** (pure-Rust STACKLESS Lua): suspendable at any
  instruction — an unusually good match for our cancellation lanes.
  Not production-ready.
- **Boa** (pure-Rust JS): would drop a C dep we do not need to drop;
  slower and larger than QuickJS.
- **Starlark**: hermetic by design, and deliberately not a general
  language (no while, bounded recursion). Wrong for workflows.
- **WASM plugins** (Extism/wasmi/wasmtime): the strongest sandbox
  and the wrong lifecycle — a compile step between "edit in himark"
  and "runs" breaks requirement 3, and JIT engines cannot execute
  inside the web shell.

## How a run works

**The runner is a trampoline over the sync engine**
(`hiscript::run_script`): a fresh VM per run evaluates the module,
calls `default(himark)`, pumps the job queue to quiescence, reads
the root promise, and answers the parked asks — capability READS
park on injected world closures awaited between context scopes;
WRITES are intents resolved instantly and carried in the outcome. A
FAILED run commits nothing. The compute deadline re-arms at every
park: compute-between-awaits is what the budget bounds, so a script
may wait on an agent for an hour while a spinning one dies.

**A run is a value; state lives in documents.** The VM is created
per run and dropped at the end. No script-global state survives
between runs — nothing for one run to leak into the next, and the
sandbox resets to zero every time. A workflow that needs memory
writes it where himark keeps everything else: in a document.

**A script run is an effect on a lane.** A script never executes on
the UI thread. `RunScriptEffect`'s handler owns the QuickJS runtime
and drives the script on the serial effect worker (QuickJS contexts
are `!Send`; so are our effect futures — the same model). One lane
per script identity (`ScriptLanes`), the relaunch idiom: launching a
script that is already running supersedes it, and the interrupt
handler polls the lane's token — a cancelled script dies at the next
instruction boundary, no cooperation required.

**Capabilities are effects; no ambient authority.** The script's
world is ONE injected object; every method that touches the outside
parks on an effect (`EffectCaller` — the same seam enrichment's
fence embed uses). This is the actual sandbox: the engine's caps
stop runaway compute, and the capability object is the entire
reachable world.

**Writes land through the doors.** A script never mutates a document
from the worker. Writes are captured as intents, diffed against the
launch snapshot ON THE WORKER, and the landing commits them through
`Document::edit` on the UI thread — undo grouping, change-sink
notifications, reparse scheduling and markup transforms all come
free BECAUSE the script cannot bypass them. An open target takes the
operation revision-guarded — typing mid-run discards that write,
logged (the watch-merge discipline); an unopened target stores
through the host. A show whose target is a store-through defers to
the store's own landing — never open what the disk does not hold
yet.

## The surface

Invocation is `script.run` — a `DynamicEditorCommand` offered on the
focused `.js` document (the two-phase payload road, the file-save
pattern). The launch captures the source plus O(1) snapshots of
every open located document; the worker resolves relative paths
against the SCRIPT's directory (the fence-embed convention) and
answers reads from snapshots first, host fetch second. `ScriptRuns`
records each outcome (the log, the error, what committed).

The capability object:

- `himark.log(...)` — collected into the run record.
- `himark.docs.read(path)` — the text, `null` for absence.
- `himark.docs.write(path, text)` — whole-text write intent (append
  is read-concat-write).
- `himark.docs.show(path)` — an open-in-pane intent, committed at
  the landing through the standard open road with its dedup.
- `himark.agent.ask(prompt)` — the agent capability, captured AT
  LAUNCH: the focused window's current session, when it names a real
  one. The run creates a FRESH chat there (the user's own chat stays
  clean), sends the turn, and folds the stream (markdown parts +
  deltas → the completed answer). A rejected ask is CATCHABLE
  (try/catch); an agentless run's ask rejects naming the missing
  session; an uncaught rejection fails the run — which commits
  nothing.
- `himark.vcs.changes()` — the Changes panel's launch-time snapshot
  (status letter, folder-relative path, counts per file), `null`
  when no vcs state stands. The heavy diffing stays with the AGENT,
  which can read files itself.

The trust model needs no consent dialog: the command names its
source file, and invoking it on that file IS the consent — the
capability object is constructed only for a run the user (or an
agent acting with the user's standing) asked for by name.

## Limits

- **Memory**: per-run allocation cap (64 MB default).
- **Deadline**: a wall-clock budget on COMPUTE between awaits (2 s
  default) — a parked script costs nothing; a spinning one dies.
- **Cancellation**: the interrupt handler polls the lane token.

## Layering

Everything above the engine is hiscript's `plugin` half (feature
`plugin`, default-on): hiscript sits ABOVE himark like every plugin,
registering its command and effect handler through `frontend-host`;
himark carries no scripting code. The bare engine
(`--no-default-features`) is what a web shell would side-load.

## What deliberately does not exist

- **No script registry or discovery.** There is no
  `.himark/scripts` scan and no script-exported commands; the one
  entry point is `script.run` on a focused script file. Automatic
  hooks (change sinks, script enrichment passes) fire WITHOUT an
  invocation, so the call-is-consent gate does not cover them — they
  need their own consent story before they exist.
- **No error markup on the script document** — a failed run lands in
  `ScriptRuns` and the log.
- **No chat disposal** — the chats a run creates accumulate in the
  session's chat list; no model/effort pick and no attachments on
  `agent.ask`.
- **No web side-loading** — the engine builds under emscripten and
  the recipe is pinned by `tools/spike-web.sh`, but the web shell
  does not ship it as a fetched side module.
