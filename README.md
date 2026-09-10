# Himark

[![Tests](https://github.com/JetBrains/himark/actions/workflows/tests.yml/badge.svg)](https://github.com/JetBrains/himark/actions/workflows/tests.yml)

## Desktop

Linux and Windows use the shared `himark-winit` host crate, built on `winit`
0.30.13 and `softbuffer`.

```sh
cargo run --release -p linux -- [file-or-directory ...]
cargo run --release -p windows -- [file-or-directory ...]
```

### Building on Windows

Prerequisites:

- Rust via [rustup](https://rustup.rs) with the default `x86_64-pc-windows-msvc`
  toolchain.
- Visual Studio 2022 (or Build Tools for Visual Studio 2022) **17.10 or newer**
  with the "Desktop development with C++" workload — MSVC toolset 14.40+ and a
  Windows 10/11 SDK.

The first build downloads prebuilt Skia binaries compiled against the MSVC STL
from toolset 14.40. Linking with an older toolset fails with unresolved
`__std_min_f`/`__std_max_f`/`__std_minmax_f` symbols; update the C++ toolset in
the Visual Studio Installer if you hit that.

The winit host opens dropped files and command-line paths recursively for
`.md`, `.markdown`, `.rs`, and `.txt` files. Shortcuts: `Ctrl+N` new scratch,
`Ctrl+P` peeker, `Ctrl+D` split, `Ctrl+L` document list, `Ctrl+F` search,
`Ctrl+W` close pane, `Ctrl+M` demo, `Ctrl+Shift+M` wall demo.

## Web

Build the static application with `apps/web/tools/build-web.sh`. An existing
complete lazy-grammar pack is reused; pass `--rebuild-grammars` to regenerate
and recompress it. The main WebAssembly module is emitted as
`web-<sha256>.wasm`, allowing it to be cached immutably while `index.html` and
the loader continue to revalidate. Lazy language modules and highlight queries
are fingerprinted the same way and resolved through a small `assets.tsv`
manifest.

Run `apps/web/tools/build-web.sh --check` to type-check the threaded
Emscripten application without linking or staging assets. This uses the same
toolchain and flags as the full build and also runs in CI; native workspace
checks only compile the web crate's stub entry point.

For a faster development loop, run `apps/web/tools/debug-web.sh`. It uses an
unoptimized incremental WebAssembly build, skips compression, reuses the
grammar pack when possible, and then starts `himark-agent-host` in HTTP mode at
`127.0.0.1:4312`. Pass `--http ADDRESS` to use a different bind address or
`--rebuild-grammars` to rebuild the grammar side modules without optimization.

When the app is hosted separately from
`himark-agent-host`, pass the host's tokened HTTP or WebSocket URL in the page
query:

```text
https://app.example/?agent_host=http%3A%2F%2F127.0.0.1%3A4312%2F%3Ftkn%3D...
```

The parameter overrides the URL injected when the agent host itself serves
the application.

With an agent host connected, use **Save** in the command palette or
`Cmd+S` / `Ctrl+S` to write an existing file back through that host. Save is
unavailable for scratches and demos: the web app has no Save As destination
picker. Revision-pinned views are not save targets either.

---

*Last edited: 2026-09-02*
