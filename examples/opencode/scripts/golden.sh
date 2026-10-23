#!/bin/sh
# Renders the fixture scenes that contain markdown and writes element trees and snapshots
# to $1 (default: target/opencode-golden) and fails when a tree differs from
# examples/opencode/golden/*.json (copy over them to accept an intended change).
# Run from the repo root.
set -e
O=${1:-target/opencode-golden}
mkdir -p "$O"
cargo build --release -p opencode
B=${CARGO_TARGET_DIR:-target}/release/opencode
F="--fixture examples/opencode/assets/fixture.json"
for sc in session streaming; do
  for th in dark light; do
    $B $F --scene $sc --theme $th --tree "$O/$sc-$th-tree.json" --snap "$O/$sc-$th.png"
  done
done
for g in examples/opencode/golden/*-tree.json; do
  cmp "$g" "$O/$(basename "$g")" || { echo "golden differs: $g" >&2; exit 1; }
done
echo "opencode golden trees match"
