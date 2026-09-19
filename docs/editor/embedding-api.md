# The himark embedding API

himark is a headless editor engine. It owns text, layout, editing, and
rendering *into a canvas it is handed* — nothing else. The **host** owns the
platform: the window, the GPU surface, the event loop, menus, files, IME.
The two meet at one contract: **the host hands himark a skia canvas, the
viewport, and events; himark paints and reports what changed.**

There is exactly one such contract, exposed as one C ABI
(`frontend/frontend-host` — its lib is `himark_api`; the generated header
is `include/himark.h`). The DESIGN story of the host — what it is, and how
deferred work flows across the boundary as effects and external executors —
is `docs/ahp/host.md`; this document is the concrete surface. Every shell is a
consumer of it: the Swift macOS/iOS app (`apps/himark-apple`) drives the C
ABI; the Rust shells (winit desktop, web) call the same contract with
native types instead of over FFI. Neither width is privileged; the C ABI is
the canonical documented surface.

## The principle: himark is handed the world, it owns none of it

- **Handed a canvas.** himark never creates a window or a GPU surface. Each
  frame the host gives it a skia canvas (backed by whatever the host
  renders to — a Metal drawable, a GL framebuffer, a raster bitmap) and
  himark paints the current editor state into it.
- **Handed a viewport.** Size and backing scale come from the host every
  frame; himark holds no notion of "the window."
- **Handed events.** Keys, text, mouse, scroll, resize, and the animation
  tick arrive from the host; himark turns them into edits and returns
  whether the frame needs repainting.
- **Owns editing; the host owns the threads.** himark spawns nothing and
  touches no OS service. It *queues* its deferred work (reparse, repair —
  one in-flight per lane) and *wakes* the host to run it; the host runs it
  on a thread it owns (`run_pending`) and hands each result back. Wasm — no
  threads at all — runs it inline. himark stays a pure library.

Everything platform-shaped — file dialogs, menus, IME composition, window
management, the display link — lives in the host, where the OS already does
it well.

## The contract, language-neutral

An opaque **engine** handle, driven single-threaded from the host's main
loop:

```
create()  -> Engine
destroy(Engine)

// render: paint current state into a canvas the host owns (see handoff)
draw(Engine, canvas, width, height, scale)

// events: each returns whether a repaint is now needed
key_down(Engine, key, mods)        -> needs_redraw
text_input(Engine, utf8, len)      -> needs_redraw     // committed text
mouse_down(Engine, x, y, button)   -> needs_redraw
scroll(Engine, x, y, dx, dy)       -> needs_redraw
resize(Engine, width, height, scale) -> needs_redraw
animation_tick(Engine, now_ms)     -> needs_redraw     // the animation clock

// files: the host reads bytes (or a path) and hands them in
open(Engine, name, utf8_bytes, len)

// deferred work — the host owns every thread; himark spawns none:
//   effect_wake fires when there is background work; the host schedules
//   run_pending on a worker thread it owns. run_pending runs the queued
//   work and posts results, then fires wake so the host schedules drain on
//   main; drain applies the finished results.
set_wake(Engine, wake: fn(ctx), ctx)           // "apply on main"
set_effect_wake(Engine, wake: fn(ctx), ctx)    // "run work off-main"
run_pending(Engine)                            // on a host worker thread
drain(Engine)                      -> needs_redraw
```

`needs_redraw` is the whole scheduling protocol: the host repaints when a
call returns true and pauses its display link when the animation tick comes
back false (exactly the idle story the animation design relies on —
docs/ui/animation.md).

Beside the core loop, the same header carries the surfaces documented
elsewhere: the platform callback table and capability flags
(docs/ahp/host.md), the IME/text-input protocol with its UTF-16 boundary
(docs/ui/ime.md), and `himark_perform_command` — the one FFI door menus and
shell shortcuts resolve through (docs/ui/commands.md).

## The skia handoff

**The host owns skia; himark is handed a canvas and paints into it.** himark
never creates a GPU context, never touches Metal, never manages a surface,
never presents — those belong to whoever owns the GPU and the window, which
is the host.

The Swift app links skia (through its C/C++ API), owns the whole rendering
setup, and drives it:

```c
// himark's only render entry point: paint current state into a canvas the
// host created and owns. himark borrows it for the call, nothing more.
void himark_draw(Engine, SkCanvas*, w, h, scale);
```

The host's rendering code:

```
// setup, once: create + connect the layer, make a skia surface bound to it
layer   = CAMetalLayer(); view.layer = layer
surface = SkSurface::MakeFromCAMetalLayer(context, layer, …)   // once, not per frame

// each display-link tick
canvas = surface.getCanvas()
himark_draw(engine, canvas, w, h, scale)
context.flushAndSubmit(); layer.present()
```

Surface creation, the drawable cycle, flush, present, timing — all
host-side. himark sees only the `SkCanvas*` for the duration of one
`draw` call.

- **Rust shells.** Already hold a skia `Canvas` and call the engine's
  `draw(canvas)` directly — the exact same contract, native types, no FFI.
- **Raster / tests.** A bitmap-backed canvas into the same `draw` — the
  headless-test path.

**The one build constraint:** the host and himark must use **one shared
skia**, so the `SkCanvas*` the host creates is a valid pointer for himark's
`skia-safe` to wrap. Two skia copies would cross pointers between
instances. This is a linking concern of the shell target, not a design
fork.

## Threading

himark spawns no threads and touches no OS scheduling service; the host owns
every thread. There are two thread contexts, and the ABI keeps them apart:

- **Main thread** — every `*(Engine, …)` call except `run_pending`: create,
  destroy, draw, events, IME, `drain`. These touch the (non-`Send`) app
  state.
- **A host worker thread** — `run_pending` only. himark *queues* deferred
  work and fires `effect_wake`; the host schedules `run_pending` on a
  thread it owns (a `DispatchQueue` in Swift, a `std::thread` in the native
  shells, inline on wasm — no thread). `run_pending` runs the queued work
  and posts each result, then fires `wake` so the host schedules `drain` on
  main.

The two contexts share only a small `Sync` block (the result inbox and the
wake slots); the app state is reached solely from the main thread, so no
himark UI state crosses threads. The host must stop its worker before
`destroy`. On wasm, with no threads, the host calls `run_pending` inline —
the same queue, drained on the main thread.

## The frame loop lives in the host

The host owns the display link. Per tick:

```
now = host clock
if animation_tick(engine, now) || pending_redraw:
    drawable = layer.nextDrawable()
    draw(engine, drawable's canvas, w, h, scale)
    present(drawable)
pause the link when animation_tick returned false and nothing is pending
```

Coalescing and present timing are shell code — `CADisplayLink` and
`nextDrawable` are first-class in Swift, winit's equivalents in the Rust
shells.

## What himark deliberately does not do

- **No window, no GPU context creation, no event loop.** The host's job.
- **No file I/O policy.** Content moves through the effect seam and the
  agent host ([Design.md](../Design.md)); the engine never touches the
  filesystem itself — sandbox-clean and testable.
- **No menus, no dialogs, no IME composition UI.** Native, host-side; menus
  resolve to command ids through the one FFI door.
- **No global state.** Everything hangs off the engine handle, so multiple
  editors / windows are just multiple handles.

## Why this is the embedding API

A host holds an engine, drives it with a canvas and events, and reads back
"what changed." The Swift app is the first non-Rust consumer, which is
exactly what validates the ABI — a foreign host driving it cleanly is the
proof the surface is honest.
