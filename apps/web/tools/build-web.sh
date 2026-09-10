#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/../../.." && pwd)"
cd "$repo_root"

rebuild_grammars=false
cargo_command=build
while (($#)); do
    case "$1" in
        --check)
            cargo_command=check
            ;;
        --rebuild-grammars)
            rebuild_grammars=true
            ;;
        -h | --help)
            echo "usage: ${0##*/} [--check] [--rebuild-grammars]"
            echo "  --check  type-check the threaded web target without linking or staging assets"
            echo "  --rebuild-grammars  rebuild and recompress the lazy grammar pack"
            exit 0
            ;;
        *)
            echo "unknown argument: $1" >&2
            echo "usage: ${0##*/} [--check] [--rebuild-grammars]" >&2
            exit 2
            ;;
    esac
    shift
done

if [[ -z "${EMSDK:-}" ]]; then
    if command -v brew >/dev/null && brew --prefix emscripten >/dev/null 2>&1; then
        emscripten_prefix="$(brew --prefix emscripten)"
        mkdir -p target/emsdk/upstream
        ln -sfn "$emscripten_prefix/libexec" target/emsdk/upstream/emscripten
        export EMSDK="$PWD/target/emsdk"
    elif [[ -d /usr/lib/emscripten ]]; then
        emscripten_dir="target/emsdk/upstream/emscripten"
        if [[ -L "$emscripten_dir" ]]; then
            unlink "$emscripten_dir"
        fi
        mkdir -p "$emscripten_dir"
        for entry in /usr/lib/emscripten/*; do
            ln -sfn "$entry" "$emscripten_dir/${entry##*/}"
        done
        export EM_CACHE="${EM_CACHE:-${XDG_CACHE_HOME:-$HOME/.cache}/emscripten}"
        mkdir -p "$EM_CACHE"
        ln -sfn "$EM_CACHE" "$emscripten_dir/cache"
        export EMSDK="$PWD/target/emsdk"
    fi
fi
if [[ -x "${EMSDK:-}/upstream/emscripten/emcc" ]]; then
    export PATH="$EMSDK/upstream/emscripten:$PATH"
    emcc --version >/dev/null
fi

python_for_depot_tools=""
for candidate in /opt/homebrew/bin/python3.11 /opt/homebrew/bin/python3.10 /usr/bin/python3; do
    if [[ -x "$candidate" ]]; then
        python_for_depot_tools="$candidate"
        break
    fi
done
if [[ -n "$python_for_depot_tools" ]]; then
    mkdir -p target/web-tools
    ln -sfn "$python_for_depot_tools" target/web-tools/python
    ln -sfn "$python_for_depot_tools" target/web-tools/python3
    export PATH="$PWD/target/web-tools:$PATH"
fi

export FORCE_SKIA_BUILD=1
export GIT_CONFIG_COUNT=1
export GIT_CONFIG_KEY_0=core.fsmonitor
export GIT_CONFIG_VALUE_0=false
if [[ -d vendor/skia-bindings-0.99.0/skia ]]; then
    find vendor/skia-bindings-0.99.0/skia -name '*.lock' -delete
fi
export EMCC_CFLAGS="-pthread -fPIC"
export CXXFLAGS_wasm32_unknown_emscripten="-fPIC"
export HIMARK_STRIP_WASM_EXCEPTIONS=1
export CARGO_TARGET_WASM32_UNKNOWN_EMSCRIPTEN_LINKER="$repo_root/apps/web/tools/emscripten-linker.sh"
grammar_exports="_main,___stack_pointer,_malloc,_calloc,_realloc,_free,_abort,_memcpy,_memmove,_memset,_memcmp,_memchr"
grammar_exports+=",_strlen,_strcmp,_strncmp,_strcpy,_strncpy,_strchr,_strrchr,_strstr,_strcat,_strncat,_strtol,_strtoul"
grammar_exports+=",_iswspace,_iswalpha,_iswalnum,_iswdigit,_iswxdigit,_iswlower,_iswupper,_iswpunct,_towlower,_towupper"
grammar_exports+=",_isspace,_isalpha,_isalnum,_isdigit,_isxdigit,_islower,_isupper,_ispunct,_tolower,_toupper"
EMCC="${EMCC:-em++}" \
RUSTFLAGS="-C panic=abort \
    -C link-arg=-sMAIN_MODULE=2 \
    -C link-arg=-sALLOW_TABLE_GROWTH=1 \
    -C link-arg=-sFETCH \
    -C link-arg=-Wl,--export=__stack_pointer \
    -C link-arg=-sEXPORTED_FUNCTIONS=$grammar_exports \
    -C link-arg=-sUSE_WEBGL2=1 \
    -C link-arg=-sMIN_WEBGL_VERSION=2 \
    -C link-arg=-sMAX_WEBGL_VERSION=2 \
    -C link-arg=-sFULL_ES3=1 \
    -C link-arg=-sALLOW_MEMORY_GROWTH=1 \
    -C target-feature=+atomics,+bulk-memory \
    -C link-arg=-pthread \
    -C link-arg=-sPTHREAD_POOL_SIZE=4 \
    -C link-arg=-sDEFAULT_PTHREAD_STACK_SIZE=8388608 -C link-arg=-sSTACK_SIZE=8388608 -C link-arg=-sSTACK_OVERFLOW_CHECK=2 \
    -C link-arg=-sMAXIMUM_MEMORY=2147483648 -C link-arg=-lwebsocket.js -C link-arg=-sSUPPORT_LONGJMP=emscripten -C link-arg=-sINITIAL_MEMORY=134217728" \
    cargo +nightly "$cargo_command" --locked -Zbuild-std=std,panic_abort -p web --release \
    --target wasm32-unknown-emscripten

if [[ "$cargo_command" == check ]]; then
    exit 0
fi

out_dir="target/web"
bin_dir="target/wasm32-unknown-emscripten/release"
grammar_dir="$out_dir/grammars"
mkdir -p "$out_dir"

sha256_file() {
    if command -v sha256sum >/dev/null 2>&1; then
        sha256sum "$1" | awk '{print $1}'
    elif command -v shasum >/dev/null 2>&1; then
        shasum -a 256 "$1" | awk '{print $1}'
    else
        echo "sha256sum or shasum is required to fingerprint web.wasm" >&2
        return 1
    fi
}

wasm_source="$bin_dir/web.wasm"
wasm_hash="$(sha256_file "$wasm_source")"
wasm_name="web-$wasm_hash.wasm"

grammar_pack_complete() {
    [[ -s "$grammar_dir/manifest.tsv" && -s "$grammar_dir/assets.tsv" ]] || return 1
    [[ -f "$grammar_dir/build-profile" && "$(<"$grammar_dir/build-profile")" == release ]] || return 1
    local module wasm query
    local count=0
    while IFS=$'\t' read -r module wasm query || [[ -n "$module" ]]; do
        [[ -n "$module" ]] || continue
        [[ -s "$grammar_dir/$wasm" && -f "$grammar_dir/$query" ]] || return 1
        ((count += 1))
    done < "$grammar_dir/assets.tsv"
    ((count > 0))
}

grammars_built=false
if [[ "$rebuild_grammars" == true ]] || ! grammar_pack_complete; then
    bash apps/web/tools/build-grammars.sh
    grammars_built=true
else
    echo "reusing grammar pack in $grammar_dir (pass --rebuild-grammars to regenerate it)"
fi

find "$out_dir" -maxdepth 1 -type f -name 'web-*.wasm*' -delete
rm -f "$out_dir/web.wasm" "$out_dir/web.wasm.gz" "$out_dir/web.wasm.br"

if ! grep -qF 'web.wasm' "$bin_dir/web.js"; then
    echo "generated web.js no longer contains the expected web.wasm reference" >&2
    exit 1
fi
sed "s|web\.wasm|$wasm_name|g" apps/web/index.html > "$out_dir/index.html"
cp apps/web/assets/JetBrainsMono-Regular.woff2 "$out_dir/JetBrainsMono-Regular.woff2"
sed "s|web\.wasm|$wasm_name|g" "$bin_dir/web.js" > "$out_dir/web.js"
cp "$wasm_source" "$out_dir/$wasm_name"
if [[ -f "$bin_dir/web.data" ]]; then
    cp "$bin_dir/web.data" "$out_dir/web.data"
fi
for worker in "$bin_dir"/web.worker.*js; do
    [[ -f "$worker" ]] && cp "$worker" "$out_dir/"
done
cp apps/web/coi-serviceworker.js "$out_dir/coi-serviceworker.js" 2>/dev/null || true

brotli_available=false
if command -v brotli >/dev/null 2>&1; then
    brotli_available=true
fi
compress_artifacts() {
    local directory="$1"
    local artifact
    for artifact in \
        "$directory"/*.wasm \
        "$directory"/*.js \
        "$directory"/*.scm \
        "$directory"/*.data \
        "$directory"/*.woff2; do
        [[ -f "$artifact" ]] || continue
        gzip -9 -kf "$artifact"
        if [[ "$brotli_available" == true ]]; then
            brotli -q 11 -f -k "$artifact"
        fi
    done
}

compress_artifacts "$out_dir"
if [[ "$grammars_built" == true ]]; then
    compress_artifacts "$grammar_dir"
fi

wasm_sizes="$(du -h "$out_dir/$wasm_name" | cut -f1 | tr -d ' ') wasm, $(du -h "$out_dir/$wasm_name.gz" | cut -f1 | tr -d ' ') gz"
if [[ -f "$out_dir/$wasm_name.br" ]]; then
    wasm_sizes+=", $(du -h "$out_dir/$wasm_name.br" | cut -f1 | tr -d ' ') br"
fi
echo "web page in $out_dir ($wasm_name; $wasm_sizes)"
