#!/usr/bin/env bash
set -euo pipefail
# Run through HiveMind, never as a local Studio build.
if [[ "${1:-all}" != build ]]; then
  cargo test -j 2 -p rune --features dev --lib dev
  cargo test -j 2 -p rune-dev-panel --features dev --test protocol --test app_spec_literal
fi
if [[ "${1:-all}" == all ]]; then
  cargo check -j 2 -p lab --features dev
  cargo check -j 2 -p rune-dev-panel --no-default-features
  cargo check -j 2 -p rune-dev-panel --release --features dev
fi
cargo build -j 2 -p rune-dev-panel --features dev
mkdir -p captures/dev-panel
cp "${CARGO_TARGET_DIR:-target}/debug/rune-dev-panel" captures/dev-panel/
cp Cargo.lock captures/dev-panel/
