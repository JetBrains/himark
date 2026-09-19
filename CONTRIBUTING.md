# Contributing to himark

Contributions are welcome. By submitting a pull request you agree to the
[JetBrains Contributor License Agreement](https://www.jetbrains.com/agreements/cla/);
you will be asked to sign it when you open your first PR.

Design documentation lives in [docs/](docs/) — start from the
[README](README.md#documentation). House rules for the code itself are in
[docs/code-style.md](docs/code-style.md).

## Prerequisites on every platform

- **Rust** via [rustup](https://rustup.rs). The stable toolchain is enough for
  the native builds; the web build additionally needs nightly (see below).
- **A C/C++ toolchain.** Several crates compile C sources (tree-sitter
  grammars, QuickJS).
- **libclang.** The QuickJS bindings are generated at build time with
  `bindgen`, which loads libclang. On macOS the Xcode Command Line Tools
  provide it, on Linux install `libclang-dev`, on Windows install LLVM and set
  `LIBCLANG_PATH`.
- **Network access on the first build.** Cargo fetches crates, and the
  vendored `skia-bindings` crate downloads a prebuilt Skia archive from the
  `rust-skia/skia-binaries` GitHub releases for your target. Only the web
  build compiles Skia from source.
- **git**, used by the Apple and web build scripts.

The workspace is large. A cold `cargo build` takes several minutes; later
builds are incremental. Dependencies are compiled with optimizations even in
the dev profile so that debug builds of the editor stay usable.

## Build and test

```sh
cargo build                       # the default members: engine, plugins, backend
cargo test                        # the default test suite
cargo clippy --workspace --all-targets --locked
cargo fmt --all -- --check
```

CI runs the tests with [cargo-nextest](https://nexte.st):

```sh
cargo install cargo-nextest
cargo nextest run --workspace --exclude linux --locked --all-targets --profile ci   # macOS
cargo nextest run --workspace --exclude macos --locked --all-targets --profile ci   # Linux
```

A few tests are marked `#[ignore]` because they need a real Claude login or
produce screenshots. Run them explicitly with `cargo test -- --ignored` when
you want them.

`cargo build` skips the platform shells. Build those explicitly with
`cargo build -p linux`, `cargo build -p windows`, or the platform scripts
described below.

## The agent host

Every shell talks to `himark-agent-host`, the backend binary built from
`backend/agent-host`. You rarely start it by hand: when a shell launches it
looks for a running host and, if there is none, spawns one. The lookup order
for the binary is `HIMARK_AGENT_HOST_BIN`, then a `himark-agent-host` file
next to the shell's own executable, then `PATH`. So for a development setup
it is enough to build it into the same target directory as the shell:

```sh
cargo build -p agent-host
```

The host writes a lockfile and a Unix socket under `~/.himark/agent-host/`
(override the directory with `HIMARK_HOST_HOME`). If a shell finds a live host
that was built from a different binary it terminates that host and starts its
own, so rebuilding always takes effect.

To run it manually:

```sh
cargo run -p agent-host                                  # unix socket only
cargo run -p agent-host -- --http 127.0.0.1:4312         # also serve the web app over HTTP
cargo run -p agent-host -- --socket /tmp/himark.sock     # custom socket path
cargo run -p agent-host -- --http 0.0.0.0:4312 --web-root target/web
```

The host runs in the foreground and logs to `~/Library/Logs/Himark/` on every
platform (override with `HIMARK_LOG_DIR`). Set `HIHOST_TRACE=1` to mirror the
log to stderr and `RUST_LOG` to change the filter. If a host is already
running the command exits immediately with status 0.

### External tools the host looks for

The host spawns these tools on demand and searches `PATH` plus the usual
install locations. Each can be pinned with an environment variable:

| Tool | Used for | Override |
|---|---|---|
| `claude` (Claude Code CLI) | Claude agent sessions, using your existing `claude` login | `HIMARK_CLAUDE_BIN` |
| `codex` (Codex CLI) | Codex agent sessions | `HIMARK_CODEX_BIN` |
| `rust-analyzer` | Code intelligence for Rust files | `HIMARK_RUST_ANALYZER` |

None of them are required to build or to edit markdown. Without them the
corresponding features are simply unavailable.

## macOS

The macOS app is an AppKit application that links the engine as a static
library and renders through Metal.

### Prerequisites

- Xcode 15 or newer, with the Command Line Tools installed
  (`xcode-select --install`). The generated project targets macOS 14.
- [XcodeGen](https://github.com/yonaskolb/XcodeGen): `brew install xcodegen`.
- The Rust target for your machine (`aarch64-apple-darwin` or
  `x86_64-apple-darwin`). The build script adds it if missing.

### Build and run

```sh
cd apps/himark-apple
./build.sh mac            # build the engine, the host, the app; then launch it
```

`build.sh mac` does the following:

1. Copies the engine's C header from `frontend/frontend-host/include/himark.h`.
2. Sparse-clones the pinned Skia headers into `target/skia-headers/` on the
   first run. The Objective-C++ Metal bridge needs them; the prebuilt Skia
   archive only carries libraries.
3. Runs `cargo build -p frontend-host --release` for the host architecture and
   reads the exact Skia library directory from cargo's build-script output
   into `Generated-macOS.xcconfig`, so the app links the same Skia the
   engine was built with.
4. Generates `himark.xcodeproj` with XcodeGen and builds the `himark-macOS`
   scheme in Release into `apps/himark-apple/build/`.
5. Builds `himark-agent-host` and copies it into the app bundle next to the
   main executable. If `target/web` exists (see the web section) it is
   bundled too, so the host can serve the browser app.
6. Ad-hoc signs the bundle and opens it.

To work in Xcode instead, generate the project and open it:

```sh
./build.sh gen            # also builds the iOS targets; see below
open himark.xcodeproj     # pick the himark-macOS scheme and press Cmd+R
```

Re-run `./build.sh gen` after changing the Rust side; Xcode links the static
library that cargo produced and does not rebuild it.

Open files with **File > Open** (`Cmd+O`). On macOS you can also run the
`winit` shell described in the Linux section (`cargo run -p linux`) as a
quick way to test the engine without Xcode.

## iOS

The iOS target shares the engine, the Metal bridge, and the Xcode project with
macOS. The iOS view renders, handles touch and pan, and takes basic typing.
Full `UITextInput` support is not implemented yet.

### Prerequisites

Everything from the macOS section, plus the iOS Rust targets:

```sh
rustup target add aarch64-apple-ios aarch64-apple-ios-sim
```

### Build and run

```sh
cd apps/himark-apple
./build.sh ios            # or ./build.sh gen: builds device + simulator libraries
open himark.xcodeproj     # pick the himark-iOS scheme and a simulator, press Cmd+R
```

The script builds `frontend-host` for `aarch64-apple-ios` and
`aarch64-apple-ios-sim`, writes both library paths into
`Generated-iOS.xcconfig` with SDK qualifiers, and regenerates the project.
The first iOS build is slow because cargo downloads and prepares the iOS
Skia archive. The script points `bindgen` at the iOS SDK sysroots through
`BINDGEN_EXTRA_CLANG_ARGS_*`, so the QuickJS bindings generate without extra
setup. Running on a physical device needs a signing team set in Xcode.

## Linux

Linux uses the shared `himark-winit` shell (`winit` 0.30 with `softbuffer`),
which supports both Wayland and X11.

### Prerequisites

On Debian or Ubuntu:

```sh
sudo apt-get install -y clang libclang-dev pkg-config \
    libfontconfig1-dev libfreetype6-dev libwayland-dev libxkbcommon-dev
```

Other distributions need the equivalents: a C++ compiler, libclang,
pkg-config, fontconfig, FreeType, Wayland, and xkbcommon development headers.

### Build and run

```sh
cargo build -p agent-host                             # the backend, once
cargo run --release -p linux -- [file-or-directory ...]
```

Command-line paths and files dropped onto the window open recursively;
`.md`, `.markdown`, `.rs`, and `.txt` files are picked up. The shell draws
its own menu bar inside the window. Use `Ctrl+N` for a new document,
`Ctrl+P` for the file peeker, `Ctrl+Shift+P` for the command palette,
`Ctrl+D` to split, `Ctrl+F` to find, `Ctrl+Shift+F` to search in files,
`Ctrl+W` to close, and `Ctrl+M` / `Ctrl+Shift+M` to open the demo documents.

## Windows

Windows uses the same `winit` shell.

### Prerequisites

- Rust via rustup with the default `x86_64-pc-windows-msvc` toolchain.
- Visual Studio 2022 or Build Tools for Visual Studio 2022, **17.10 or
  newer**, with the "Desktop development with C++" workload. The prebuilt
  Skia binaries are compiled against the MSVC STL from toolset 14.40; linking
  with an older toolset fails with unresolved `__std_min_f`, `__std_max_f`,
  and `__std_minmax_f` symbols. Update the C++ toolset in the Visual Studio
  Installer if you hit that.
- LLVM, for libclang. Install it from the LLVM releases or with
  `winget install LLVM.LLVM`, then set `LIBCLANG_PATH` to its `bin`
  directory.

### Build and run

```sh
cargo run --release -p windows -- [file-or-directory ...]
```

Shortcuts are the same as on Linux. The Windows build is not covered by CI,
and the agent host currently depends on Unix-only APIs (pseudo-terminals and
Unix sockets), so on Windows the shell runs without a backend and the
features that route through it are unavailable.

## Web

The web app is the engine compiled to WebAssembly with Emscripten, running
in a worker pool with WebGL2 rendering. It connects to an agent host over a
WebSocket.

### Prerequisites

- Rust **nightly** with the `rust-src` component and the
  `wasm32-unknown-emscripten` target. The build uses `-Zbuild-std`.

  ```sh
  rustup toolchain install nightly --component rust-src --target wasm32-unknown-emscripten
  ```

- **Emscripten 6.0.7**, the version CI pins. Either install the
  [emsdk](https://github.com/emscripten-core/emsdk) and `source emsdk_env.sh`
  so that `EMSDK` is set, or `brew install emscripten` on macOS; the scripts
  detect the Homebrew install and a distro install under `/usr/lib/emscripten`.
- **Tools for building Skia from source**, which the web target does: `git`,
  `ninja`, `clang`, and Python 3.10 or 3.11 for Skia's `depot_tools`. The
  scripts look for `/opt/homebrew/bin/python3.11`, `python3.10`, then
  `/usr/bin/python3`. On Ubuntu: `apt-get install clang libclang-dev ninja-build pkg-config`.
- Optional: `brotli` for `.br` artifacts next to the `.gz` ones.

The first web build compiles Skia for Emscripten and takes a long time.

### Release build

```sh
apps/web/tools/build-web.sh                    # full build into target/web
apps/web/tools/build-web.sh --rebuild-grammars # also rebuild the lazy grammar pack
apps/web/tools/build-web.sh --check            # type-check only, as CI does
```

The output in `target/web` is a static site: `index.html`, the loader
`web.js`, the worker script, the fonts, and `web-<sha256>.wasm`. The main
module is content-addressed so it can be cached forever while `index.html`
revalidates. Language grammars compile to separate side modules under
`target/web/grammars/` and load on first use; a complete existing pack is
reused unless you pass `--rebuild-grammars`. Everything is also written as
`.gz` (and `.br` when brotli is installed), so a server that serves
precompressed files can use them directly.

The page needs cross-origin isolation for threads. A bundled service worker
(`coi-serviceworker.js`) adds the required headers when the server does not,
so any static file host works, including GitHub Pages.

### Development loop

```sh
apps/web/tools/debug-web.sh                    # unoptimized incremental build, then serve
apps/web/tools/debug-web.sh --http 0.0.0.0:8080
apps/web/tools/debug-web.sh --rebuild-grammars
```

The debug script builds into a separate target directory
(`target/web-debug-build`, override with `HIMARK_WEB_DEBUG_TARGET_DIR`), skips
compression, stages the page into `target/web`, and then starts
`himark-agent-host` in HTTP mode on `127.0.0.1:4312` (or `HIMARK_HTTP_BIND`).
The host prints a URL with a one-time token; open it in the browser:

```
[agent-host] browser url: http://localhost:4312/?tkn=...
```

The host serves the page from `--web-root`, `HIMARK_WEB_ROOT`, or a `web`
directory found next to or above its own binary, which is how `target/web`
is found in a development checkout and `Contents/Resources/web` in the
macOS bundle.

### Hosting the page elsewhere

When the static page is served by another server, tell it where the agent
host is with a query parameter carrying the host's tokened URL:

```
https://app.example/?agent_host=http%3A%2F%2F127.0.0.1%3A4312%2F%3Ftkn%3D...
```

The parameter overrides the URL the host injects when it serves the page
itself. With a host connected, **Save** in the command palette or `Cmd+S` /
`Ctrl+S` writes an existing file back through it. Scratches, demos, and
revision-pinned views have no save target in the browser.

## Releases

The `Build` workflow (`.github/workflows/build.yml`) builds packages for
every platform and attaches them to a **draft** GitHub release. Publish it by
pushing a version tag:

```sh
git tag v0.2.0
git push origin v0.2.0
```

The version in the package names is the tag without the leading `v`; it is
also written into the macOS bundle's `CFBundleShortVersionString`. Review
the draft on the Releases page and publish it when the notes look right.
Every push to `main`, and a manual run from the Actions tab, builds the
same packages and leaves them as workflow artifacts without creating a
release.

The packages:

| Package | Contents |
|---|---|
| `himark-<v>-macos-arm64.zip` | `himark-macOS.app` with the agent host and the web page bundled |
| `himark-<v>-linux-{x86_64,aarch64}.tar.gz` | `himark` (the `winit` shell) and `himark-agent-host` |
| `himark-<v>-windows-{x86_64,aarch64}.zip` | `himark.exe` (no agent host on Windows yet) |
| `himark-<v>-web.tar.gz` | the static site from `apps/web/tools/build-web.sh` |
| `SHA256SUMS` | checksums of the above |

The macOS app is only ad-hoc signed, so Gatekeeper blocks it on first
launch. Either right-click the app and choose **Open**, or clear the
quarantine flag after unzipping:

```sh
xattr -dr com.apple.quarantine himark-macOS.app
```

Signing with a Developer ID and notarizing are not wired up yet; the
`macos` job is the place to add them.

## Environment variables

| Variable | Effect |
|---|---|
| `HIMARK_AGENT_HOST_BIN` | Path of the host binary a shell should autostart |
| `HIMARK_HOST_HOME` | Directory for the host lockfile and socket (default `~/.himark/agent-host`) |
| `HIMARK_HOST_AUTOSTART` | `1` or a binary path: lets the AHP client itself spawn a host when discovery finds none |
| `HIMARK_AHP_URL` | Connect to an agent host at this URL instead of discovering one |
| `HIMARK_WEB_ROOT` | Directory the host serves as the web app |
| `HIMARK_HTTP_BIND` | Bind address for `debug-web.sh` (default `127.0.0.1:4312`) |
| `HIMARK_CLAUDE_BIN`, `HIMARK_CODEX_BIN`, `HIMARK_RUST_ANALYZER` | Pin the external tools |
| `HIMARK_LOG_DIR` | Log directory (default `~/Library/Logs/Himark`) |
| `HIHOST_TRACE` | Mirror host logs to stderr |
| `RUST_LOG` | Log filter for host and app logs |

## Troubleshooting

- **"no himark agent host running"**: the shell could not find or start the
  backend. Build it with `cargo build -p agent-host` so it sits next to the
  shell binary, or start it by hand and try again.
- **"a live host from another build holds ~/.himark/agent-host"**: a host
  from an older build was running; the shell replaces it automatically. If a
  stale lockfile blocks startup, stop the old process and delete
  `~/.himark/agent-host/host.lock`.
- **Skia download fails**: the first build needs access to GitHub releases.
  Set `SKIA_LIBRARY_SEARCH_PATH` to a directory with prebuilt Skia libraries
  to build offline, or `FORCE_SKIA_BUILD=1` to compile Skia from source.
- **Linker errors mentioning `__std_min_f` on Windows**: update the MSVC
  toolset to 14.40 or newer.
- **iOS build fails in `bindgen` with `stdio.h not found`**: run through
  `build.sh`, which sets the SDK sysroots, rather than invoking cargo directly.
- **Web build cannot find `emcc`**: set `EMSDK` to your emsdk checkout, or
  install Emscripten with Homebrew.
