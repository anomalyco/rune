# rune-cli

First-class cross-platform CLI (`rune`) for creating, checking, and running Rune applications without repository scripts, Bash, or Python.

## Install

```sh
cargo install --git https://github.com/anomalyco/rune --rev <dev-rev> rune-cli
```

Or from a local Rune checkout:

```sh
cargo install --path crates/rune-cli
```

## Commands

- `rune new <name>` — scaffolds a working Rune app with a pinned `rune` dependency, matching `[patch.crates-io]` entries, `[features] hot = ["rune/hot"]`, `[profile.rune-hot]`, `rune.toml`, a live-patchable `src/view.rs`, and a headless spring test. Pass `--path <rune-dir>` to target a local Rune checkout, or `--rev <sha>` / `--git <url>` to customize the pin.
- `rune dev` — runs your app in development mode:
  - On macOS and Linux with Dioxus CLI `0.7.10` (`dx`) installed and `[features] hot` present, runs guarded live function-body hot patching (with the AST contract guard embedded directly in `rune`, so no external guard binary or Python/Bash script is needed) and automatically rebuilds and relaunches on structural/state edits.
  - When `dx` `0.7.10` is not installed — or by default on Windows, where upstream `dx` `0.7.10` dylib patching is experimental — automatically falls back to a native watch-and-restart `cargo run` loop. Pass `--mode hot` to require `dx` hot patching or `--mode restart` to always use watch-and-restart.
- `rune check` — verifies Rune git URL, revision, `[patch.crates-io]` list, and `Cargo.lock` alignment (plus optional `--wasm-bindgen-cli`), then runs `cargo check` (or `--pins-only`).
- `rune doctor` — inspects `rustc`, `cargo`, platform prerequisites (Windows GNU `dlltool`, Linux `libxkbcommon`/`libwayland-client`), `dx` `0.7.10` status, and the current project's config and pins.

## Project configuration

Defaults can be placed in `rune.toml`:

```toml
[dev]
patch = ["src/view.rs", "src/ui/**/*.rs"]
features = []
bin = "my-app"
```

Or in `Cargo.toml` under `[package.metadata.rune.dev]`. Precedence is: built-in defaults < `Cargo.toml` metadata < `rune.toml` < CLI flags.
