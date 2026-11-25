- Docs: `crates/rune/README.md` "Depending on Rune" now covers building from an internal git
  mirror or without network (git `insteadOf` with `git-fetch-with-cli`, the mirror URL in the
  manifest, or `cargo vendor`, and what each does to `Cargo.lock`; offline builds from a warm,
  stable `CARGO_HOME`) and a checklist for bumping Rune. New `scripts/check_rune_pins.py`, for apps
  to copy, checks that every Rune git line shares one URL and `rev`, that the app patches exactly
  Rune's `[patch.crates-io]` list at that rev, that `Cargo.lock` holds one Rune commit, and that
  the `wasm-bindgen` crate matches the CLI pin.
