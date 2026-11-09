#!/usr/bin/env bash
# Run from the app's Cargo workspace. Produces a static, HTTPS-hostable folder.
set -euo pipefail
usage="usage: web-build.sh PACKAGE [--example NAME] [--out DIR] [--opt] [--dev] [--watch] [--no-html] [--names]"
package=${1:?$usage}
shift
example=
out=target/web
opt=false
dev=false
watch=false
html=true
names=false
while (($#)); do
    case "$1" in
        --example) example=${2:?--example needs a name}; shift 2 ;;
        --out) out=${2:?--out needs a directory}; shift 2 ;;
        --opt) opt=true; shift ;;
        --dev) dev=true; shift ;;
        --watch) watch=true; shift ;;
        --no-html) html=false; shift ;;
        --names) names=true; shift ;;
        -h|--help) echo "$usage"; exit 0 ;;
        *) echo "unknown option: $1" >&2; echo "$usage" >&2; exit 2 ;;
    esac
done
# Build with the toolchain's own cargo rather than a `cargo` wrapper on PATH, which may run
# the build on another machine and leave no wasm here. Set CARGO to choose a cargo binary.
cargo=${CARGO:-$(rustup which cargo 2>/dev/null || command -v cargo)}
# Calling the toolchain's cargo directly skips rustup's proxy, so pin the toolchain for the
# rustc it runs too: build scripts compile outside the repo, where rust-toolchain.toml can't reach.
if [[ -z ${RUSTUP_TOOLCHAIN:-} ]] && toolchain=$(rustup show active-toolchain 2>/dev/null); then
    export RUSTUP_TOOLCHAIN=${toolchain%% *}
fi
# C and C++ dependencies (rune-luau's Luau) build for the web with WASI SDK's clang against WASI
# libc, using WebAssembly exception handling. Set WASI_SDK_PATH to an unpacked WASI SDK 33 or
# newer; rune-luau's build script links the SDK's libc and C++ runtime from it.
if [[ -n ${WASI_SDK_PATH:-} ]]; then
    exe=
    [[ -f $WASI_SDK_PATH/bin/clang.exe ]] && exe=.exe
    export CC_wasm32_unknown_unknown="$WASI_SDK_PATH/bin/clang$exe" \
        CXX_wasm32_unknown_unknown="$WASI_SDK_PATH/bin/clang++$exe" \
        AR_wasm32_unknown_unknown="$WASI_SDK_PATH/bin/llvm-ar$exe" \
        CXXSTDLIB_wasm32_unknown_unknown=c++ \
        CFLAGS_wasm32_unknown_unknown=--target=wasm32-wasip1 \
        LUAU_CXXFLAGS="--target=wasm32-wasip1 -fexceptions -fwasm-exceptions -mllvm -wasm-use-legacy-eh=false"
fi
meta=$("$cargo" metadata --format-version 1 --no-deps)
selection=$(python3 -c '
import json,sys
m=json.load(sys.stdin); p=next((p for p in m["packages"] if p["name"]==sys.argv[1]),None)
if p is None: sys.exit("package is not a workspace member")
targets=[t for t in p["targets"] if "example" in t["kind"] and t["name"]==sys.argv[2]] if sys.argv[2] else [t for t in p["targets"] if "cdylib" in t["crate_types"]]
if not targets and not sys.argv[2]: targets=[t for t in p["targets"] if "bin" in t["kind"]]
if len(targets)!=1: sys.exit("select one cdylib, binary, or --example target")
t=targets[0]
print(m["workspace_root"]); print(m["target_directory"]); print(t["name"])
print("example" if sys.argv[2] else "lib" if "cdylib" in t["crate_types"] else "bin")
' "$package" "$example" <<< "$meta")
root=$(sed -n '1p' <<< "$selection")
target=$(sed -n '2p' <<< "$selection")
name=$(sed -n '3p' <<< "$selection")
kind=$(sed -n '4p' <<< "$selection")
if $dev; then
    # The debug profile without debug info, with dependencies at opt-level 1: an edit rebuilds
    # only this package, unoptimized, while Rune and the rest stay fast at run time.
    profile=web-dev
    build=(--profile web-dev --config 'profile.web-dev.inherits="dev"' --config 'profile.web-dev.debug=0'
        --config 'profile.web-dev.package."*".opt-level=1')
    # `"*"` covers only non-members, so name the workspace members this package depends on.
    while IFS= read -r member; do
        [[ -n $member ]] && build+=(--config "profile.web-dev.package.\"$member\".opt-level=1")
    done < <(python3 -c '
import json,sys
m=json.load(sys.stdin); by={p["name"]:p for p in m["packages"]}
todo=[sys.argv[1]]; seen=set()
while todo:
    for d in by[todo.pop()]["dependencies"]:
        if d.get("path") and d["name"] in by and d["name"] not in seen and d["kind"] is None:
            seen.add(d["name"]); todo.append(d["name"])
print("\n".join(sorted(seen - {sys.argv[1]})))
' "$package" <<< "$meta")
    opt=false
else
    profile=release
    build=(--release)
    # Defaults for size; set any of these to override, e.g. CARGO_PROFILE_RELEASE_LTO=fat for a smaller, slower build.
    export CARGO_PROFILE_RELEASE_OPT_LEVEL=${CARGO_PROFILE_RELEASE_OPT_LEVEL:-s} \
        CARGO_PROFILE_RELEASE_LTO=${CARGO_PROFILE_RELEASE_LTO:-thin} CARGO_PROFILE_RELEASE_CODEGEN_UNITS=1 \
        CARGO_PROFILE_RELEASE_DEBUG=0 CARGO_PROFILE_RELEASE_STRIP=debuginfo
    # Panic messages carry source paths; keep the builder's home folder out of a wasm meant for the public.
    # rustc applies the last matching prefix, so the narrower cargo home comes after the home folder.
    export RUSTFLAGS="${RUSTFLAGS:-} --remap-path-prefix=$HOME=/home --remap-path-prefix=${CARGO_HOME:-$HOME/.cargo}=/cargo"
fi
args=(--lib)
artifact="$target/wasm32-unknown-unknown/$profile/${name//-/_}.wasm"
if [[ $kind == example ]]; then args=(--example "$name"); artifact="$target/wasm32-unknown-unknown/$profile/examples/$name.wasm"; fi
if [[ $kind == bin ]]; then args=(--bin "$name"); artifact="$target/wasm32-unknown-unknown/$profile/$name.wasm"; fi
version=$(python3 -c 'import sys,tomllib; print(next(p["version"] for p in tomllib.load(open(sys.argv[1],"rb"))["package"] if p["name"]=="wasm-bindgen"))' "$root/Cargo.lock")
if ! command -v wasm-bindgen >/dev/null || [[ $(wasm-bindgen --version) != "wasm-bindgen $version" ]]; then
    echo "Install the matching CLI: cargo install wasm-bindgen-cli --version $version --locked" >&2
    exit 2
fi
marker='<!-- generated by rune web-build.sh: delete this line to keep your own edits -->'

write_html() {
    local reload='' import="import init from './app.js';" load="await init();"
    if $watch; then
        import=
        reload='<script src="reload.js"></script>'
        load="const v = Date.now(); const { default: init } = await import('./app.js?v=' + v); await init({ module_or_path: './app_bg.wasm?v=' + v });"
    fi
    cat > "$out/index.html" <<HTML
<!doctype html>
$marker
<html lang="en">
<meta charset="utf-8">
<meta name="viewport" content="width=device-width,initial-scale=1,maximum-scale=1,user-scalable=no,viewport-fit=cover">
<meta name="color-scheme" content="light dark">
<title>Rune</title>
<style>html,body{margin:0;width:100%;height:100%;overflow:hidden;overscroll-behavior:none;touch-action:none}#status{font:16px system-ui;padding:24px}</style>
<div id="status">Loading…</div>
$reload
<script type="module">
$import
try {
    if (!navigator.gpu) throw new Error('WebGPU requires a supported browser and HTTPS.');
    $load
    document.querySelector('#status').remove();
} catch (error) {
    document.querySelector('#status').textContent = String(error);
    console.error(error);
}
</script>
</html>
HTML
}

build_once() {
    local start=$SECONDS
    "$cargo" build -p "$package" --target wasm32-unknown-unknown "${build[@]}" "${args[@]}" || return 1
    if [[ ! -f $artifact ]]; then
        echo "cargo finished but $artifact is missing. If cargo ran on another machine, set CARGO to a local cargo binary." >&2
        return 1
    fi
    # Build into a staging folder beside the output, then rename each file into place, so a
    # page loading mid-build never gets app.js and app_bg.wasm from different builds.
    local next="$out/.next"
    rm -rf "$next"
    mkdir -p "$next"
    # A release build drops the function-name section (over 40% of an unoptimized wasm); a `--dev`
    # build, or `--names` for profiling, keeps it so panics and profiles name their functions.
    local bindgen=(--target web --out-dir "$next" --out-name app)
    $dev || $names || bindgen+=(--remove-name-section)
    wasm-bindgen "${bindgen[@]}" "$artifact" || return 1
    if $opt; then
        local keep=()
        $names && keep=(-g)
        wasm-opt -Oz ${keep[@]+"${keep[@]}"} --enable-bulk-memory --enable-nontrapping-float-to-int --enable-sign-ext --enable-reference-types --enable-exception-handling \
            "$next/app_bg.wasm" -o "$next/app_bg.opt.wasm" || return 1
        mv "$next/app_bg.opt.wasm" "$next/app_bg.wasm"
    fi
    # Rune's default fonts embed Inter cut to Latin, Greek and Cyrillic text and fetch the full faces from beside
    # app.js once the first frame is up: copy those of the Rune this build embedded the cut from.
    local cut
    cut=$(tr ' ' '\n' < "${artifact%.wasm}.d" 2>/dev/null | grep -m1 '/assets/web/first/Inter-Regular\.ttf$' || true)
    if [[ -n $cut ]]; then cp "${cut%/first/*}"/Inter-*.ttf "$next/"; fi
    local entry
    for entry in "$next"/*; do
        if [[ -d $entry ]]; then
            rm -rf "${out:?}/$(basename "$entry")"
        fi
        mv -f "$entry" "$out/"
    done
    rmdir "$next"
    if $html; then
        if [[ ! -e $out/index.html ]] || grep -qF "$marker" "$out/index.html"; then
            write_html
        else
            echo "Kept $out/index.html: it has no generated-by marker."
        fi
    fi
    if $watch; then
        cat > "$out/reload.js" <<'JS'
// Reloads the page when web-build.sh --watch finishes a build.
(() => {
    const url = new URL('.stamp', document.currentScript?.src ?? location.href);
    let last;
    setInterval(async () => {
        try {
            const stamp = await (await fetch(url, { cache: 'no-store' })).text();
            if (last !== undefined && stamp !== last) location.reload();
            last = stamp;
        } catch {}
    }, 500);
})();
JS
        echo "$(date +%s)-$RANDOM" > "$out/.stamp"
    else
        rm -f "$out/reload.js" "$out/.stamp"
    fi
    $dev || python3 -c 'import gzip,pathlib,sys; p=pathlib.Path(sys.argv[1]); b=p.read_bytes(); print(f"{p}: {len(b):,} bytes raw, {len(gzip.compress(b,mtime=0)):,} bytes gzip")' "$out/app_bg.wasm"
    echo "Built $profile in $((SECONDS - start)) s. Static folder: $out"
}

if ! $watch; then
    build_once
    exit
fi
echo "Watching. Serve the folder with: python3 -m http.server 8000 -d $out (then open http://localhost:8000)"
# Poll the workspace members' files, skipping build output, and return once one changed after
# the build started (so an edit made during a build is not lost) or one appeared or went away.
wait_for_change() {
    META=$meta python3 - "$out" "$1" <<'PY'
import json, os, sys, time
out, since = os.path.realpath(sys.argv[1]), float(sys.argv[2])
m = json.loads(os.environ["META"])
root = m["workspace_root"]
dirs = {os.path.dirname(p["manifest_path"]) for p in m["packages"]}
def scan():
    seen = {}
    paths = [os.path.join(root, "Cargo.toml"), os.path.join(root, "Cargo.lock")]
    for top in dirs:
        for here, subdirs, files in os.walk(top):
            subdirs[:] = [d for d in subdirs if d not in ("target", "node_modules") and not d.startswith(".")
                          and os.path.realpath(os.path.join(here, d)) != out]
            paths += [os.path.join(here, f) for f in files]
    for path in paths:
        try: seen[path] = os.stat(path).st_mtime
        except OSError: pass
    return seen
first = scan()
while True:
    now = scan()
    if now.keys() != first.keys() or max(now.values(), default=0) >= since: break
    time.sleep(0.3)
PY
}
while true; do
    started=$(python3 -c 'import time; print(time.time())')
    build_once || echo "Build failed; waiting for the next change." >&2
    wait_for_change "$started"
done
