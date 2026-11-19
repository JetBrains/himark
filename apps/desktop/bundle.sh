#!/usr/bin/env bash
# Copyright © 2026 JetBrains s.r.o.
# SPDX-License-Identifier: Apache-2.0
#
# The macOS application bundle: the desktop binary, the agent host
# beside it (the app starts the one next to its own executable) and
# the web site under Resources/web (the host serves it from there).
# Launches the bundle unless CI is set.
set -euo pipefail
cd "$(dirname "$0")"
ROOT="$(cd ../.. && pwd)"

case "$(uname -m)" in
  arm64) RUST_TARGET="aarch64-apple-darwin" ;;
  x86_64) RUST_TARGET="x86_64-apple-darwin" ;;
  *)
    echo "unsupported macOS architecture: $(uname -m)" >&2
    exit 1
    ;;
esac
# The machine's native architecture, even when the active Rust
# toolchain runs under Rosetta.
rustup target list --installed | grep -qx "$RUST_TARGET" || rustup target add "$RUST_TARGET"

# libtailscale's Go core: the cc-crate CC string carries -Wall -Wextra,
# and Go compiles its own runtime/cgo with -Werror.
export CGO_CFLAGS="${CGO_CFLAGS:+$CGO_CFLAGS }-Wno-sign-compare"

echo "==> cargo build -p himark-desktop -p agent-host --release --target $RUST_TARGET"
( cd "$ROOT" && cargo build -p himark-desktop -p agent-host --release --target "$RUST_TARGET" )

BIN="$ROOT/target/$RUST_TARGET/release"
APP="$ROOT/target/bundle/himark-macOS.app"
rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp "$BIN/desktop" "$APP/Contents/MacOS/himark-macOS"
cp "$BIN/himark-agent-host" "$APP/Contents/MacOS/"
sed -e "s/@HIMARK_VERSION@/${HIMARK_VERSION:-0.1}/" \
    -e "s/@HIMARK_BUILD_NUMBER@/${HIMARK_BUILD_NUMBER:-1}/" \
    Info.plist > "$APP/Contents/Info.plist"

if [ -d "$ROOT/target/web" ]; then
  cp -R "$ROOT/target/web" "$APP/Contents/Resources/web"
else
  echo "note: no target/web — build the page with apps/web/tools/build-web.sh first"
fi

codesign --force --sign - "$APP/Contents/MacOS/himark-agent-host" 2>/dev/null || true
codesign --force --sign - "$APP" 2>/dev/null || true

if [ -n "${CI:-}" ]; then
  echo "built $APP"
else
  open "$APP"
fi
