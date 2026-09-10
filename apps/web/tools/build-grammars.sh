#!/usr/bin/env bash
set -euo pipefail

debug=false
if [[ "${1:-}" == "--debug" ]]; then
    debug=true
    shift
fi
if (($#)); then
    echo "usage: ${0##*/} [--debug]" >&2
    exit 2
fi

out="target/web/grammars"
mkdir -p "$out"
find "$out" -maxdepth 1 -type f \( \
    -name '*.wasm' -o -name '*.wasm.gz' -o -name '*.wasm.br' -o \
    -name '*.scm' -o -name '*.scm.gz' -o -name '*.scm.br' \
\) -delete
rm -f "$out/assets.tsv" "$out/assets.tsv.tmp" "$out/build-profile"

if [[ "$debug" == true ]]; then
    cargo \
        --config 'profile.dev.package."*".opt-level=0' \
        --config 'profile.dev.package.documents.opt-level=0' \
        --config 'profile.dev.package.himark.opt-level=0' \
        --config 'profile.dev.package.editor.opt-level=0' \
        --config 'profile.dev.package.himarkdown.opt-level=0' \
        run -p grammar-pack -- "$out"
else
    cargo run --release -p grammar-pack -- "$out"
fi

registry="$HOME/.cargo/registry/src"

sha256_file() {
    if command -v sha256sum >/dev/null 2>&1; then
        sha256sum "$1" | awk '{print $1}'
    elif command -v shasum >/dev/null 2>&1; then
        shasum -a 256 "$1" | awk '{print $1}'
    else
        echo "sha256sum or shasum is required to fingerprint grammar assets" >&2
        return 1
    fi
}

emcc_flags=(
    -fno-exceptions
    -pthread
    -sSIDE_MODULE=1
)
if [[ "$debug" == true ]]; then
    emcc_flags=(-O0 "${emcc_flags[@]}")
else
    emcc_flags=(-O2 -DNDEBUG "${emcc_flags[@]}")
fi

assets="$out/assets.tsv.tmp"
: > "$assets"
while IFS=$'\t' read -r module symbol crate parser_dir; do
    version=$(awk -v crate="$crate" '
        $0 == "name = \"" crate "\"" { grab = 1; next }
        grab && /^version = / { gsub(/"/, "", $3); print $3; exit }
    ' Cargo.lock)
    if [[ -z "$version" ]]; then
        echo "!! $module: crate $crate not in Cargo.lock" >&2
        exit 1
    fi
    src=""
    for candidate in "$registry"/*/"$crate-$version/$parser_dir"; do
        [[ -d "$candidate" ]] && src="$candidate" && break
    done
    if [[ -z "$src" ]]; then
        echo "!! $module: sources for $crate-$version/$parser_dir not vendored (cargo fetch first)" >&2
        exit 1
    fi
    sources=("$src/parser.c")
    [[ -f "$src/scanner.c" ]] && sources+=("$src/scanner.c")
    wasm="$out/$module.wasm"
    query="$out/$module.scm"
    "${EMCC:-emcc}" "${emcc_flags[@]}" -I "$src" "${sources[@]}" -o "$wasm"

    wasm_name="$module-$(sha256_file "$wasm").wasm"
    query_name="$module-$(sha256_file "$query").scm"
    mv "$wasm" "$out/$wasm_name"
    mv "$query" "$out/$query_name"
    printf '%s\t%s\t%s\n' "$module" "$wasm_name" "$query_name" >> "$assets"
    echo "  $wasm_name ($(du -h "$out/$wasm_name" | cut -f1 | tr -d ' ')) + $query_name"
done < "$out/manifest.tsv"
mv "$assets" "$out/assets.tsv"
if [[ "$debug" == true ]]; then
    printf 'debug\n' > "$out/build-profile"
else
    printf 'release\n' > "$out/build-profile"
fi

echo "side modules in $out"
