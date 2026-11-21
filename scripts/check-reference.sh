#!/bin/sh
# Invoke on a build worker through the build queue. GPU is headless but still needs an adapter.
set -eu
export RUSTC_WRAPPER=
export RUSTFLAGS="${RUSTFLAGS:-} -C llvm-args=-fp-contract=off"
cargo test --locked -p rune-ref --test reference --test alloc
cargo test --locked -p rune --test alloc
if [ "${1:-}" = "--gpu" ]; then
    cargo test --locked -p rune-ref --features gpu --test differential -- --test-threads=1 --nocapture
    cargo test --locked -p rune-ref --features gpu --test fx_blend -- --test-threads=1
fi
