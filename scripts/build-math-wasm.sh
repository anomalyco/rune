#!/bin/sh
# Reproducibly builds `crates/rune-kit/assets/math/ratex-math.wasm.deflate`,
# `crates/rune-kit/assets/math/katex-fonts.pack.deflate`, and
# `crates/rune-kit/src/math/wasm_hash.rs` from pinned `tools/ratex-math-wasm/`.
# Pass `--check` to rebuild from source and assert byte-for-byte equality.
set -eu

cd "$(dirname "$0")/.."
exec cargo run --release --manifest-path tools/ratex-math-wasm/pack/Cargo.toml -- "$@"
