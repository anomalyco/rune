- **`scripts/rune-dev` no longer writes into a read-only Rune checkout and honours the job count.**
  An app's run (`--manifest-path`) built into the Rune checkout's `target/live-edit-build`, which
  fails when Rune is a Cargo git dependency, and always ran cargo with 2 jobs. Its build now goes
  under `$XDG_CACHE_HOME/rune-dev` (or `~/.cache/rune-dev`), beside its source mirror;
  `RUNE_DEV_TARGET_DIR` picks another directory. `RUNE_DEV_JOBS`, else an exported
  `CARGO_BUILD_JOBS`, sets the job count, 2 when neither is set. Examples keep building in
  `target/live-edit-build`.
