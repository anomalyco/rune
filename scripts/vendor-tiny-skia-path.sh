#!/usr/bin/env bash
# Re-makes vendor/tiny-skia-path: the tiny-skia-path crate exactly as published on crates.io at
# <version>, plus vendor/tiny-skia-path.patch, which puts back how Skia itself strokes and splits
# conics where tiny-skia's port has drifted (Rune's icons are covered as Chromium's Skia covers
# them: crates/rune/src/text/area.rs):
#   - a conic becomes quads a quarter of a *device* pixel from it (the stroker passes its
#     resolution scale), and one quad when that's close enough, not at least two;
#   - the stroker keeps each tangent as a vector, measures its distance to the tangent line as
#     Skia's `pt_to_tangent_line` does, and, out of recursion, emits a line rather than giving up;
#   - `set_point_length` multiplies in double and rounds once, as `SkPoint::setLength` does;
#   - `PathBuilder::conic_to`, `set_conic_tolerance` and `AutoConicToQuads` are public.
#   - `set_keep_conics` retains weighted conic segments for SVG arcs; the stroker fits the two
#     contours against the conic itself instead of fitting against quadratic approximations.
# Run it when tiny-skia-path is bumped, then build: a patch that no longer applies fails here,
# and the changes go back in by hand.
# Usage: scripts/vendor-tiny-skia-path.sh <version>     e.g. scripts/vendor-tiny-skia-path.sh 0.11.4
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
version="${1:?usage: scripts/vendor-tiny-skia-path.sh <version>}"
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
curl -fsSL "https://static.crates.io/crates/tiny-skia-path/tiny-skia-path-$version.crate" -o "$tmp/crate.tar.gz"
tar -xzf "$tmp/crate.tar.gz" -C "$tmp"
rm -rf vendor/tiny-skia-path
mv "$tmp/tiny-skia-path-$version" vendor/tiny-skia-path
git apply --directory=vendor/tiny-skia-path vendor/tiny-skia-path.patch
echo "vendor/tiny-skia-path: tiny-skia-path $version + vendor/tiny-skia-path.patch"
