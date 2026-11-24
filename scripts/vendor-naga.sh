#!/usr/bin/env bash
# Re-makes vendor/naga: the naga crate exactly as published on crates.io at <version>, plus
# vendor/naga.patch (remaps `Statement::Emit` ranges in `back::pipeline_constants::process_function`
# from the per-expression index spans newly appended to `function.expressions` rather than
# `Range::new_from_bounds(new_pos[first], new_pos[last])`, so constant folding an expression to an
# earlier handle neither stretches an `Emit` backwards over earlier emits nor inverts it;
# gfx-rs/wgpu#7793). Run it when wgpu/naga is bumped, then build: a patch that no longer applies
# fails here, and the fix goes back in by hand. If upstream naga takes the fix, delete vendor/naga
# and its `[patch.crates-io]` entry instead.
# Usage: scripts/vendor-naga.sh <version>     e.g. scripts/vendor-naga.sh 30.0.1
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
version="${1:?usage: scripts/vendor-naga.sh <version>}"
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
curl -fsSL "https://static.crates.io/crates/naga/naga-$version.crate" -o "$tmp/crate.tar.gz"
tar -xzf "$tmp/crate.tar.gz" -C "$tmp"
rm -rf vendor/naga
mv "$tmp/naga-$version" vendor/naga
git apply --directory=vendor/naga vendor/naga.patch
echo "vendor/naga: naga $version + vendor/naga.patch"
