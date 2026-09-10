#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/../../.."

SYSROOT="$(em-config CACHE)/sysroot"
EMCC="em++" EMCC_CFLAGS="-pthread -fPIC" \
BINDGEN_EXTRA_CLANG_ARGS_wasm32_unknown_emscripten="--sysroot=$SYSROOT -fvisibility=default" \
RUSTFLAGS="-C panic=abort \
    -C link-arg=-sMAIN_MODULE=2 \
    -C link-arg=-sALLOW_TABLE_GROWTH=1 \
    -C link-arg=-sALLOW_MEMORY_GROWTH=1 \
    -C target-feature=+atomics,+bulk-memory \
    -C link-arg=-pthread \
    -C link-arg=-sPTHREAD_POOL_SIZE=2 \
    -C link-arg=-sDEFAULT_PTHREAD_STACK_SIZE=8388608 -C link-arg=-sSTACK_SIZE=8388608 \
    -C link-arg=-sINITIAL_MEMORY=134217728" \
cargo +nightly build -Zbuild-std=std,panic_abort -p hiscript \
    --no-default-features \
    --bin hiscript-spike --target wasm32-unknown-emscripten --release

node target/wasm32-unknown-emscripten/release/deps/hiscript_spike.js
