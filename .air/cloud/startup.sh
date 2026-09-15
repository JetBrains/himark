#!/usr/bin/env bash
# Environment bootstrap for the himark workspace.
#
# The image is a bare Ubuntu 24.04 with no C toolchain, no headers and no
# passwordless sudo, so everything is installed into $HOME:
#
#   ~/.air-sysroot        Ubuntu .deb packages unpacked as a userspace sysroot
#                         (gcc/g++, libc headers, clang/libclang, pkg-config,
#                         fontconfig, freetype, wayland, xkbcommon, zlib, fonts)
#   ~/.air-toolchain      compiler wrappers that point gcc/clang at that sysroot
#   ~/.air-fontconfig     a fonts.conf that points fontconfig at the sysroot's
#                         font directories instead of the empty /usr/share/fonts
#   ~/.cargo ~/.rustup    rustup with the stable toolchain
#   ~/.air-himark-env.sh  the environment, sourced from ~/.profile and ~/.bashrc
#
# Modes (AIR_STARTUP_MODE): "warmup" bakes the snapshot, so it does the slow
# cacheable work -- unpack the sysroot, install Rust, fetch the crates, prime
# the font cache and run a full `cargo build` -- and then blocks in
# `healthcheck`. "task" boots from that snapshot, so it only re-asserts the
# environment and starts the agent host in the background before returning.

set -euo pipefail

log() { printf '[air-setup] %s\n' "$*"; }
die() { printf '[air-setup] ERROR: %s\n' "$*" >&2; exit 1; }

if [ "${AIR_STARTUP_MODE:-}" = warmup ]; then WARMUP=1; else WARMUP=; fi

REPO_ROOT=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/../.." && pwd)
SYSROOT="$HOME/.air-sysroot"
TOOLCHAIN_BIN="$HOME/.air-toolchain/bin"
FONTCONFIG_DIR="$HOME/.air-fontconfig"
ENV_FILE="$HOME/.air-himark-env.sh"
APT_DIR="$HOME/.air-apt"
SYSROOT_STAMP="$SYSROOT/.air-complete"
HOST_LOG="$HOME/.air-agent-host.log"
HOST_BIN="$REPO_ROOT/target/debug/himark-agent-host"
SHOT_DIR="${HIMARK_SHOT:-/tmp/himark-shots}"
HTTP_BIND="${HIMARK_HTTP_BIND:-0.0.0.0:4312}"
HTTP_PORT="${HTTP_BIND##*:}"

# Everything the README's "Prerequisites" and the Linux CI job install with
# apt-get, plus the fontconfig tools and a few font families: the image ships
# no fonts at all and the editor panics ("a system typeface") without one.
# apt resolves the full dependency closure for these.
APT_PACKAGES=(
    gcc g++ make pkg-config libc6-dev
    clang libclang-dev
    libfontconfig-dev libfreetype-dev libwayland-dev libxkbcommon-dev
    zlib1g-dev
    fontconfig fonts-dejavu-core fonts-liberation2 fonts-noto-color-emoji
)

# --------------------------------------------------------------------------
# userspace sysroot
# --------------------------------------------------------------------------

install_sysroot() {
    if [ -f "$SYSROOT_STAMP" ]; then
        log "sysroot already present at $SYSROOT ($(du -sh "$SYSROOT" | cut -f1))"
        return
    fi

    log "resolving the apt dependency closure for: ${APT_PACKAGES[*]}"
    rm -rf "$APT_DIR"
    mkdir -p "$APT_DIR/state/lists/partial" "$APT_DIR/cache/archives/partial" "$APT_DIR/debs"
    : > "$APT_DIR/state/status"
    cat > "$APT_DIR/apt.conf" <<EOF
Dir::State "$APT_DIR/state";
Dir::State::status "$APT_DIR/state/status";
Dir::Cache "$APT_DIR/cache";
Dir::Cache::archives "$APT_DIR/cache/archives";
Acquire::Languages "none";
EOF
    export APT_CONFIG="$APT_DIR/apt.conf"

    # apt still tries to tidy the system archive dir it cannot write; that
    # warning is harmless, so only a real failure should stop us here.
    apt-get -qq update
    apt-get install -y --no-install-recommends --print-uris "${APT_PACKAGES[@]}" \
        | sed -n "s/^'\(http[^']*\)'.*/\1/p" > "$APT_DIR/urls.txt"
    local count
    count=$(wc -l < "$APT_DIR/urls.txt")
    [ "$count" -gt 0 ] || die "apt resolved no download URLs; is archive.ubuntu.com reachable?"
    log "downloading $count .deb packages"
    (cd "$APT_DIR/debs" && xargs -n1 -P8 curl -sSfLO --retry 3 < "$APT_DIR/urls.txt")

    log "unpacking into $SYSROOT"
    rm -rf "$SYSROOT"
    mkdir -p "$SYSROOT"
    local deb
    for deb in "$APT_DIR"/debs/*.deb; do
        dpkg-deb -x "$deb" "$SYSROOT"
    done

    # Reproduce Ubuntu's usr-merge layout: glibc's linker scripts refer to
    # /lib/x86_64-linux-gnu/libc.so.6 and /lib64/ld-linux-x86-64.so.2, and ld
    # resolves those relative to --sysroot.
    ln -sfnT usr/lib "$SYSROOT/lib"
    ln -sfnT usr/lib64 "$SYSROOT/lib64"
    ln -sfnT usr/bin "$SYSROOT/bin"
    ln -sfnT usr/sbin "$SYSROOT/sbin"

    # dpkg-deb keeps absolute symlink targets, which would escape the sysroot
    # (fontconfig's conf.d and clang's resource dir rely on them).
    local link target
    while IFS= read -r link; do
        target=$(readlink "$link")
        case "$target" in /*) ln -sfn "$SYSROOT$target" "$link" ;; esac
    done < <(find "$SYSROOT" -type l)

    log "sysroot unpacked ($(du -sh "$SYSROOT" | cut -f1), $(find "$SYSROOT" -xtype l | wc -l) dangling links)"
    rm -rf "$APT_DIR/debs" "$APT_DIR/cache"
    touch "$SYSROOT_STAMP"
}

# gcc/clang are relocatable but still look for headers and crt files under the
# compiled-in /usr prefix, so every invocation needs --sysroot. Rust invokes
# the linker as plain `cc` and the `cc` crate uses `cc`/`c++`, so the wrappers
# have to own those names and come first on PATH.
install_compiler_wrappers() {
    mkdir -p "$TOOLCHAIN_BIN"
    local name real
    for name in gcc cc g++ c++ clang clang++; do
        case "$name" in
            cc) real=gcc ;;
            c++) real=g++ ;;
            *) real=$name ;;
        esac
        cat > "$TOOLCHAIN_BIN/$name" <<EOF
#!/bin/sh
exec "$SYSROOT/usr/bin/$real" --sysroot="$SYSROOT" "\$@"
EOF
        chmod +x "$TOOLCHAIN_BIN/$name"
    done
    log "compiler wrappers installed in $TOOLCHAIN_BIN"
}

# The sysroot's own fonts.conf lists absolute font directories (/usr/share/
# fonts), which are empty in this image. Point fontconfig at the sysroot's
# directories instead, or Skia finds no typeface and the editor panics.
install_fontconfig() {
    mkdir -p "$FONTCONFIG_DIR" "$HOME/.cache/fontconfig"
    cat > "$FONTCONFIG_DIR/fonts.conf" <<EOF
<?xml version="1.0"?>
<!DOCTYPE fontconfig SYSTEM "urn:fontconfig:fonts.dtd">
<!-- Generated by .air/cloud/startup.sh -->
<fontconfig>
  <dir>$SYSROOT/usr/share/fonts</dir>
  <dir>$SYSROOT/usr/local/share/fonts</dir>
  <dir>/usr/share/fonts</dir>
  <dir prefix="xdg">fonts</dir>
  <dir>~/.fonts</dir>
  <cachedir>$HOME/.cache/fontconfig</cachedir>
  <include ignore_missing="yes">$SYSROOT/etc/fonts/conf.d</include>
</fontconfig>
EOF
    log "wrote $FONTCONFIG_DIR/fonts.conf"
}

# --------------------------------------------------------------------------
# rust
# --------------------------------------------------------------------------

install_rust() {
    if [ -x "$HOME/.cargo/bin/rustup" ]; then
        log "rustup already present ($("$HOME/.cargo/bin/rustc" --version))"
        return
    fi
    log "installing rustup with the stable toolchain"
    curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs \
        | sh -s -- -y --no-modify-path --profile minimal \
            --default-toolchain stable -c rustfmt -c clippy -c rust-src
    log "installed $("$HOME/.cargo/bin/rustc" --version)"
}

# CI runs the suite with cargo-nextest, so make it available too. The
# prebuilt tarball takes seconds; building it from crates.io is the fallback.
install_nextest() {
    if [ -x "$HOME/.cargo/bin/cargo-nextest" ]; then
        log "cargo-nextest already present"
        return
    fi
    log "installing cargo-nextest"
    if curl -sSfL --retry 3 https://get.nexte.st/latest/linux \
        | tar -xzf - -C "$HOME/.cargo/bin" 2>/dev/null; then
        log "cargo-nextest installed from the prebuilt tarball"
    elif cargo install cargo-nextest --locked; then
        log "cargo-nextest built from crates.io"
    else
        log "WARNING: could not install cargo-nextest; use plain 'cargo test'"
    fi
}

# --------------------------------------------------------------------------
# environment
# --------------------------------------------------------------------------

# The launch runs this script as a child process, so exports made here die
# with it. Write them to a file and source that from the login shell and from
# ~/.bashrc, which is what the agent's shells actually read.
write_env_file() {
    cat > "$ENV_FILE" <<EOF
# Generated by .air/cloud/startup.sh -- himark toolchain environment.
export HIMARK_SYSROOT="$SYSROOT"
export PATH="$TOOLCHAIN_BIN:\$HOME/.cargo/bin:$SYSROOT/usr/bin:\$PATH"

# gcc/ld and the \`cc\` crate: headers come from --sysroot (see the wrappers),
# libraries and crt objects from LIBRARY_PATH.
export CC="$TOOLCHAIN_BIN/cc"
export CXX="$TOOLCHAIN_BIN/c++"
export LIBRARY_PATH="$SYSROOT/usr/lib/x86_64-linux-gnu:$SYSROOT/usr/lib\${LIBRARY_PATH:+:\$LIBRARY_PATH}"
export LD_LIBRARY_PATH="$SYSROOT/usr/lib/x86_64-linux-gnu:$SYSROOT/usr/lib\${LD_LIBRARY_PATH:+:\$LD_LIBRARY_PATH}"

# pkg-config: the .pc files carry absolute /usr paths, PKG_CONFIG_SYSROOT_DIR
# rewrites them into the sysroot.
export PKG_CONFIG_PATH="$SYSROOT/usr/lib/x86_64-linux-gnu/pkgconfig:$SYSROOT/usr/share/pkgconfig"
export PKG_CONFIG_SYSROOT_DIR="$SYSROOT"

# bindgen (rquickjs, tree-sitter) loads libclang at build time.
export LIBCLANG_PATH="$SYSROOT/usr/lib/llvm-18/lib"
export BINDGEN_EXTRA_CLANG_ARGS="--sysroot=$SYSROOT"

# Skia enumerates fonts through fontconfig at runtime.
export FONTCONFIG_FILE="$FONTCONFIG_DIR/fonts.conf"
export FONTCONFIG_PATH="$FONTCONFIG_DIR"
EOF
    log "wrote $ENV_FILE"

    local line="[ -f \"$ENV_FILE\" ] && . \"$ENV_FILE\"  # himark-air-env"
    # A login shell reads only the first of these that exists, so writing to
    # ~/.profile while the image ships a ~/.bash_profile would do nothing.
    local rc="$HOME/.profile" candidate target
    for candidate in "$HOME/.bash_profile" "$HOME/.bash_login" "$HOME/.profile"; do
        if [ -f "$candidate" ]; then rc="$candidate"; break; fi
    done
    for target in "$rc" "$HOME/.bashrc"; do
        [ -f "$target" ] || touch "$target"
        if ! grep -qF 'himark-air-env' "$target"; then
            printf '\n%s\n' "$line" >> "$target"
            log "hooked $ENV_FILE into $target"
        fi
    done
}

# --------------------------------------------------------------------------
# build and run
# --------------------------------------------------------------------------

prime_font_cache() {
    log "priming the fontconfig cache"
    fc-cache -f >/dev/null 2>&1 || log "WARNING: fc-cache failed"
    log "fontconfig sees $(fc-list | wc -l) fonts; default sans is $(fc-match sans)"
}

prime_build_caches() {
    cd "$REPO_ROOT"
    log "fetching crates (cargo fetch --locked)"
    cargo fetch --locked

    # Cargo's first build also downloads the prebuilt Skia archive for this
    # target from the rust-skia/skia-binaries releases.
    log "building the default members (engine, plugins, backend); this is the slow one"
    cargo build --locked

    # Not default members: the winit shell the README tells you to run, and
    # hiscript, whose rquickjs dependency is what exercises bindgen/libclang.
    log "building the linux shell and hiscript"
    cargo build --locked -p linux -p hiscript

    # Deliberately NOT `--all-targets`: test binaries link Skia statically per
    # crate and a full --all-targets build overflows this 30G volume. The
    # healthcheck below primes the test profile for a representative subset.
    log "disk after the build: $(df -h "$REPO_ROOT" | awk 'NR==2 {print $4" free"}')"
}

start_agent_host() {
    if [ ! -x "$HOST_BIN" ]; then
        log "no agent host binary at $HOST_BIN yet; skipping start"
        return
    fi
    if pgrep -f 'himark-agent-host --http' >/dev/null 2>&1; then
        log "agent host already running"
        return
    fi
    local web_root_args=()
    if [ -d "$REPO_ROOT/target/web" ]; then
        web_root_args=(--web-root "$REPO_ROOT/target/web")
        log "serving the web app from target/web"
    else
        log "target/web is not built; the agent host serves AHP/WebSocket only"
        log "  (build it with apps/web/tools/build-web.sh -- see the README's Web section)"
    fi
    log "starting the agent host on $HTTP_BIND (log: $HOST_LOG)"
    : > "$HOST_LOG"
    ( cd "$REPO_ROOT" && nohup "$HOST_BIN" --http "$HTTP_BIND" "${web_root_args[@]}" \
        >> "$HOST_LOG" 2>&1 & echo $! > "$HOME/.air-agent-host.pid" )
}

# --------------------------------------------------------------------------
# healthcheck
# --------------------------------------------------------------------------
# Asserts what a real task on this repository needs, not just that the install
# succeeded:
#   1. the toolchain compiles, links and runs C++ against the userspace
#      sysroot, and pkg-config resolves the native libraries;
#   2. the agent host -- the backend every shell talks to -- answers an
#      authenticated HTTP request on its port and rejects an unauthenticated
#      one;
#   3. the engine's own tests pass;
#   4. the editor actually renders: a headless Skia screenshot test produces a
#      PNG, which only works when Skia, tree-sitter and fontconfig all work.
# Polls for readiness without a deadline; the launch owns the timeout. Any
# failure returns non-zero, which fails startup.

healthcheck() {
    # shellcheck source=/dev/null
    . "$ENV_FILE"
    cd "$REPO_ROOT"

    log "healthcheck: toolchain"
    rustc --version || return 1
    cargo --version || return 1
    cc --version | head -1 || return 1

    local probe
    probe=$(mktemp -d)
    cat > "$probe/probe.cc" <<'EOF'
#include <cstdio>
#include <string>
#include <fontconfig/fontconfig.h>
#include <xkbcommon/xkbcommon.h>
int main() {
    std::string ok = "ok";
    FcConfig* config = FcInitLoadConfigAndFonts();
    if (!config) { std::printf("fontconfig failed to load\n"); return 1; }
    FcFontSet* fonts = FcConfigGetFonts(config, FcSetSystem);
    int count = fonts ? fonts->nfont : 0;
    std::printf("fontconfig %d, %d fonts, xkbcommon %s, c++ %s\n", FcGetVersion(),
                count, xkb_keysym_get_name ? "linked" : "missing", ok.c_str());
    return count > 0 ? 0 : 1;
}
EOF
    log "healthcheck: compiling and running a C++ probe against the sysroot"
    # shellcheck disable=SC2046
    "$CXX" "$probe/probe.cc" $(pkg-config --cflags --libs fontconfig xkbcommon) \
        -o "$probe/probe" || { rm -rf "$probe"; return 1; }
    "$probe/probe" || { log "the probe found no usable fonts"; rm -rf "$probe"; return 1; }
    rm -rf "$probe"

    log "healthcheck: pkg-config sees the native libraries"
    pkg-config --modversion fontconfig freetype2 xkbcommon wayland-client zlib || return 1

    [ -x "$HOST_BIN" ] || { log "agent host binary missing at $HOST_BIN"; return 1; }

    log "healthcheck: waiting for the agent host to answer on port $HTTP_PORT"
    local token code waited=0
    while :; do
        token=$(sed -n 's/.*[?&]tkn=\([0-9a-f][0-9a-f]*\).*/\1/p' "$HOST_LOG" 2>/dev/null | tail -1)
        if [ -n "$token" ]; then
            code=$(curl -s -o /dev/null -m 5 --noproxy '*' -w '%{http_code}' \
                "http://127.0.0.1:$HTTP_PORT/?tkn=$token" || true)
            # 200 once target/web is built, 404 ("no web root configured")
            # otherwise; both mean the router answered an authenticated request.
            case "$code" in
                200 | 404)
                    log "agent host answered HTTP $code for an authenticated request"
                    break
                    ;;
            esac
        fi
        if ! pgrep -f 'himark-agent-host --http' >/dev/null 2>&1; then
            log "the agent host is not running; its log follows"
            tail -40 "$HOST_LOG" 2>/dev/null || true
            return 1
        fi
        waited=$((waited + 3))
        log "  still waiting for the agent host (${waited}s, token=$([ -n "$token" ] && echo found || echo pending), last code=${code:-none})"
        sleep 3
    done

    code=$(curl -s -o /dev/null -m 5 --noproxy '*' -w '%{http_code}' \
        "http://127.0.0.1:$HTTP_PORT/" || true)
    if [ "$code" != 403 ]; then
        log "expected HTTP 403 for a tokenless request, got ${code:-none}"
        return 1
    fi
    log "healthcheck: a tokenless request is correctly rejected with 403"

    log "healthcheck: running the engine's tests (rope, text, intervals, documents)"
    cargo test --locked --lib -p rope -p text -p intervals -p documents || return 1

    log "healthcheck: rendering the editor headlessly into $SHOT_DIR"
    rm -f "$SHOT_DIR/rust-split.png"
    HIMARK_SHOT="$SHOT_DIR" cargo test --locked -p demo --lib -- \
        --ignored --exact tests::dump_rust_split_screenshot || return 1
    local shot="$SHOT_DIR/rust-split.png" size
    [ -f "$shot" ] || { log "no screenshot at $shot"; return 1; }
    size=$(wc -c < "$shot")
    # A blank frame compresses to a few KB; a rendered split view is ~150K.
    if [ "$size" -lt 20000 ]; then
        log "the screenshot at $shot is only ${size}B, so nothing was drawn"
        return 1
    fi
    log "healthcheck: the editor rendered $shot (${size}B)"

    log "healthcheck: OK ($(df -h "$REPO_ROOT" | awk 'NR==2 {print $4" free on the workspace volume"}'))"
}

# --------------------------------------------------------------------------

main() {
    log "AIR_STARTUP_MODE=${AIR_STARTUP_MODE:-unset} repo=$REPO_ROOT"
    log "disk: $(df -h "$REPO_ROOT" | awk 'NR==2 {print $4" free"}'), cpus: $(nproc)"

    install_sysroot
    install_compiler_wrappers
    install_fontconfig
    install_rust
    write_env_file

    # shellcheck source=/dev/null
    . "$ENV_FILE"
    mkdir -p "$SHOT_DIR"

    if [ -n "$WARMUP" ]; then
        prime_font_cache
        install_nextest
        prime_build_caches
        start_agent_host
        log "warmup: blocking on healthcheck"
        healthcheck || die "healthcheck failed"
        log "warmup complete"
    else
        # Boot from the snapshot: refresh crates for whatever lockfile this
        # branch carries and bring the backend up, both without blocking.
        ( cd "$REPO_ROOT" && nohup cargo fetch --locked >> "$HOME/.air-cargo-fetch.log" 2>&1 & ) || true
        start_agent_host
        log "task startup done; agent host log: $HOST_LOG"
    fi
}

main "$@"
