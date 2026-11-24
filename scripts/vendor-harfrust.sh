#!/usr/bin/env bash
# Re-makes vendor/harfrust: the harfrust crate exactly as published on crates.io at <version>, plus
# vendor/harfrust.patch (`DEFAULT_SHAPER` is a `static`, not a `const`). The planner swaps any
# shaper but the default one for `DUMBER_SHAPER`, which never zeroes a mark's advance, when a font
# shapes with `morx`, and it tells the default one apart by its address. Each use of a `const`
# may sit at an address of its own, so a run whose script is Common (every emoji) was planned
# with `DUMBER_SHAPER`, and Apple Color Emoji's two-glyph sequences (a couple with a heart, a
# handshake in two skin tones) took two advances where HarfBuzz and Core Text give one. Run it
# when parley's harfrust is bumped, then build: a patch that no longer applies fails here, and
# the fix goes back in by hand. If upstream harfrust takes the fix, delete vendor/harfrust and
# its `[patch.crates-io]` entry instead.
# Usage: scripts/vendor-harfrust.sh <version>     e.g. scripts/vendor-harfrust.sh 0.12.0
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
version="${1:?usage: scripts/vendor-harfrust.sh <version>}"
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
curl -fsSL "https://static.crates.io/crates/harfrust/harfrust-$version.crate" -o "$tmp/crate.tar.gz"
tar -xzf "$tmp/crate.tar.gz" -C "$tmp"
rm -rf vendor/harfrust
mv "$tmp/harfrust-$version" vendor/harfrust
git apply --directory=vendor/harfrust vendor/harfrust.patch
echo "vendor/harfrust: harfrust $version + vendor/harfrust.patch"
