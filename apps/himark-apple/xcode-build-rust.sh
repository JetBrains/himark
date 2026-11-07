#!/usr/bin/env bash
# Copyright © 2026 JetBrains s.r.o.
# SPDX-License-Identifier: Apache-2.0
#
# The Xcode PRE-BUILD phase (project.yml seats it on both targets):
# build the Rust staticlib for the SDK being built, so ⌘R alone is
# enough after a Rust change — linking a stale libhimark_api.a was
# the silent failure this kills. `./build.sh` stays the road for the
# from-scratch setup, the xcconfig generation and the macOS bundle's
# extras (agent-host, the web site).
set -euo pipefail
cd "$(dirname "$0")"
ROOT="$(cd ../.. && pwd)"

# Xcode's phase environment is minimal and exports an SDKROOT that
# would poison cargo's HOST-side build scripts (an iOS SDK for a
# macOS build script). Reach the usual toolchains (rustup, homebrew's
# go for libtailscale), then pin SDKROOT to the macOS SDK rather
# than unsetting it: Xcode's PATH resolves `cc` to the toolchain's
# own clang, which — unlike the /usr/bin xcrun shim — has no default
# sysroot, so every cc-rs build script (tree-sitter, ring, quickjs…)
# would die on `TargetConditionals.h not found`. cc-rs and rustc both
# ignore a MacOSX.platform SDKROOT for iOS targets and ask xcrun.
export PATH="$HOME/.cargo/bin:/opt/homebrew/bin:/usr/local/bin:/usr/local/go/bin:$PATH"
unset SDKROOT
export SDKROOT="$(xcrun --sdk macosx --show-sdk-path)"

case "${PLATFORM_NAME:-macosx}" in
  macosx)
    case "$(uname -m)" in
      arm64) RUST_TARGET="aarch64-apple-darwin" ;;
      x86_64) RUST_TARGET="x86_64-apple-darwin" ;;
      *) echo "error: unsupported host architecture $(uname -m)" >&2; exit 1 ;;
    esac
    ;;
  iphoneos)
    RUST_TARGET="aarch64-apple-ios"
    export BINDGEN_EXTRA_CLANG_ARGS_aarch64_apple_ios="--sysroot $(xcrun --sdk iphoneos --show-sdk-path)"
    ;;
  iphonesimulator)
    RUST_TARGET="aarch64-apple-ios-sim"
    export BINDGEN_EXTRA_CLANG_ARGS_aarch64_apple_ios_sim="--sysroot $(xcrun --sdk iphonesimulator --show-sdk-path) --target=arm64-apple-ios-simulator"
    ;;
  *)
    echo "error: unsupported platform '${PLATFORM_NAME:-}'" >&2
    exit 1
    ;;
esac

command -v cargo >/dev/null || {
  echo "error: cargo is not on PATH — install rustup (https://rustup.rs)" >&2
  exit 1
}
rustup target list --installed | grep -qx "$RUST_TARGET" || {
  echo "==> rustup target add $RUST_TARGET"
  rustup target add "$RUST_TARGET"
}

# libtailscale's Go core under the cc crate's -Wall -Wextra: Go
# compiles its own runtime/cgo with -Werror (see build.sh).
export CGO_CFLAGS="${CGO_CFLAGS:+$CGO_CFLAGS }-Wno-sign-compare"

echo "==> cargo build -p himark-capi --release --target $RUST_TARGET"
( cd "$ROOT" && cargo build -p himark-capi --release --target "$RUST_TARGET" )

# The FFI header the bridging header includes — keep it the build's.
cp "$ROOT/frontend/himark-ffi/include/himark.h" ./himark.h
cp "$ROOT/frontend/himark-capi/include/himark-shell.h" ./himark-shell.h
cp "$ROOT/frontend/hitailnet/include/himark-tailnet.h" ./himark-tailnet.h
cp "$ROOT/frontend/hiahp/include/himark-ahp.h" ./himark-ahp.h

# The skia-bindings out-dir rotates with dependency bumps, and the
# xcconfig's link paths were evaluated BEFORE this phase ran: when
# they no longer match the build on disk, fail LOUDLY with the remedy
# instead of letting the link fail cryptically (or worse, succeed
# against a stale skia).
# (The reader DRAINS stdin — an early break would SIGPIPE cargo and
# pipefail would read that as the build failing.)
SKIA="$(cd "$ROOT" && cargo build -p himark-capi --release --target "$RUST_TARGET" \
  --message-format=json 2>/dev/null | python3 -c "
import sys, json
out = None
for line in sys.stdin:
    try: m = json.loads(line)
    except Exception: continue
    if out is None and m.get('reason') == 'build-script-executed' \
            and 'skia-bindings' in m.get('package_id', ''):
        out = m['out_dir'] + '/skia'
print(out or '')
")"
[ -n "$SKIA" ] || { echo "error: skia-bindings out dir not found in cargo's output" >&2; exit 1; }
case "${LIBRARY_SEARCH_PATHS:-}" in
  *"$SKIA"*) ;;
  *)
    echo "error: the generated xcconfig's link paths are STALE — skia now lives at" >&2
    echo "       $SKIA" >&2
    echo "       run ./build.sh (gen covers macOS+iOS) and build again." >&2
    exit 1
    ;;
esac
