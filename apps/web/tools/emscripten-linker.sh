#!/usr/bin/env bash
set -euo pipefail

if [[ -n "${HIMARK_LOG_LINK_ARGS:-}" ]]; then
    printf '%s\n' "$@" > "$HIMARK_LOG_LINK_ARGS"
fi
args=()
for arg in "$@"; do
    if [[ "${HIMARK_STRIP_WASM_EXCEPTIONS:-}" == "1" && "$arg" == "-fwasm-exceptions" ]]; then
        continue
    else
        args+=("$arg")
    fi
done

exec "${EMCC:-emcc}" "${args[@]}"
