#!/usr/bin/env bash
# Regenerates the CoreText reference sheets that `rune::text::pixel_compare` (its test and
# `--example coretext_compare`) compares Rune's text with. macOS only. Rune's layout of each
# case goes to scripts/coretext-reference.swift, which draws it with CoreText, font smoothing on
# and off; layout.txt records that layout so a change to it marks the sheets stale.
#   crates/rune/tests/coretext/     Inter, committed.
#   crates/rune/tests/coretext/sf/  San Francisco from this Mac's system font; never committed
#                                   (Apple's font can't be redistributed). Skipped without SFNS.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
cargo run -q -p rune --example coretext_compare -- export "$tmp"
for set in inter sf; do
  [[ -d "$tmp/$set" ]] || continue
  for scale in 1 2; do
    swift scripts/coretext-reference.swift "$tmp/$set/cases-${scale}x.json" "$tmp/$set" >/dev/null
  done
done
cargo run -q -p rune --example coretext_compare -- pack "$tmp"
du -ch crates/rune/tests/coretext/*.png | tail -1
