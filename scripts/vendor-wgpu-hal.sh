#!/usr/bin/env bash
# Re-makes vendor/wgpu-hal: the wgpu-hal crate exactly as published on crates.io at <version>, plus
# vendor/wgpu-hal.patch (`dx12::Instance::expose_adapter`, which `gpu/pick.rs` uses to hand wgpu the one
# adapter it picked through DXGI; FXC loaded on first use; `Surface::set_present_rects`, partial presents; a device-wide pool of D3D12 command allocators, each with its lists). Run it when wgpu is bumped, then build: a patch
# that no longer applies fails here, and the function goes back in by hand. If upstream wgpu takes
# the function, delete vendor/ and the `[patch.crates-io]` entries instead.
# `--regenerate` goes the other way: it writes vendor/wgpu-hal.patch from vendor/wgpu-hal/src against
# the published <version>, then checks that the patch applied to that version gives the vendored
# source byte for byte. Edit the vendored source, then regenerate; two branches that both change the
# patch merge their source and regenerate instead of merging the patch's hunks.
# Usage: scripts/vendor-wgpu-hal.sh <version>                 e.g. scripts/vendor-wgpu-hal.sh 30.0.1
#        scripts/vendor-wgpu-hal.sh --regenerate <version>
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
regenerate=false
if [ "${1:-}" = "--regenerate" ]; then regenerate=true; shift; fi
version="${1:?usage: scripts/vendor-wgpu-hal.sh [--regenerate] <version>}"
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
curl -fsSL "https://static.crates.io/crates/wgpu-hal/wgpu-hal-$version.crate" -o "$tmp/crate.tar.gz"
tar -xzf "$tmp/crate.tar.gz" -C "$tmp"
if $regenerate; then
  # The patch is against the version vendored: another version would pass its own round trip.
  vendored="$(sed -n 's/^version = "\(.*\)"$/\1/p' vendor/wgpu-hal/Cargo.toml | head -n 1)"
  [ "$vendored" = "$version" ] || { echo "vendor/wgpu-hal is wgpu-hal $vendored, not $version" >&2; exit 1; }
  # A checkout made before .gitattributes kept these bytes can still hold converted copies: the
  # patch would be a whole-file one that passes its own check. `git checkout -- vendor/wgpu-hal`.
  converted="$(git ls-files --eol -- vendor/wgpu-hal/src | awk '{ i = substr($1, 3); w = substr($2, 3); if (i != w && w != "") print $NF }')"
  [ -z "$converted" ] || { echo "the checkout's line endings differ from the index's (git checkout -- vendor/wgpu-hal):" >&2; echo "$converted" >&2; exit 1; }
  mkdir "$tmp/a" "$tmp/b"
  cp -R "$tmp/wgpu-hal-$version/src" "$tmp/a/src"
  cp -R vendor/wgpu-hal/src "$tmp/b/src"
  # Git's options pinned, whatever the user's config says: no line-ending conversion, no external
  # diff or text conversion, the default algorithm and context, plain a/ b/ paths.
  git_=(env GIT_CONFIG_GLOBAL=/dev/null GIT_CONFIG_NOSYSTEM=1 git -c core.attributesFile=/dev/null -c core.autocrlf=false -c core.safecrlf=false -c core.eol=lf -c diff.noprefix=false -c diff.mnemonicPrefix=false -c diff.relative=false -c diff.algorithm=myers -c diff.indentHeuristic=true)
  # Exit 1 means the trees differ, which is the point; above 1 is an error.
  status=0
  (cd "$tmp" && "${git_[@]}" diff --no-index --no-color --no-renames --no-ext-diff --no-textconv --abbrev=8 --unified=3 --src-prefix= --dst-prefix= a/src b/src) > "$tmp/new.patch" || status=$?
  [ "$status" -le 1 ] || exit "$status"
  mkdir "$tmp/check"
  cp -R "$tmp/wgpu-hal-$version/src" "$tmp/check/src"
  (cd "$tmp/check" && "${git_[@]}" apply "$tmp/new.patch")
  diff -r "$tmp/check/src" vendor/wgpu-hal/src
  mv "$tmp/new.patch" vendor/wgpu-hal.patch
  echo "vendor/wgpu-hal.patch: vendor/wgpu-hal/src against wgpu-hal $version"
  exit 0
fi
rm -rf vendor/wgpu-hal
mv "$tmp/wgpu-hal-$version" vendor/wgpu-hal
git apply --directory=vendor/wgpu-hal vendor/wgpu-hal.patch
echo "vendor/wgpu-hal: wgpu-hal $version + vendor/wgpu-hal.patch"
