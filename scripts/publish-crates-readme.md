# rune-ui

An early development build of **Rune**, a GPU-rendered UI framework for Rust (macOS, Windows, Linux, and the web) with its own renderer, text shaping, layout, springs, and headless tooling.

> **Not affiliated with the Rune programming language.** The `rune` crate on crates.io is the Rune scripting language; this crate is the Rune UI framework from [anomalyco/rune](https://github.com/anomalyco/rune).

## Development snapshot notice

This is a pre-release snapshot (`{{VERSION}}`, commit `{{REV}}`). Public APIs change without notice. Pin the exact version with `=`.

**The supported way to use Rune today is still pinning a git revision of `dev`** (see [the repository README](https://github.com/anomalyco/rune)). That gives you daily fixes and keeps Rune and its patches on the same revision.

## Building with `rune-ui`

Because Cargo does not inherit `[patch.crates-io]` from dependencies, add `rune-ui` and the matching patch entries at commit `{{REV}}` to your workspace root `Cargo.toml` (`winit` and `sctk-adwaita` must come from the same revision):

```toml
[dependencies]
rune-ui = "={{VERSION}}"

[patch.crates-io]
wgpu-hal = { git = "https://github.com/anomalyco/rune", rev = "{{REV}}" }
naga = { git = "https://github.com/anomalyco/rune", rev = "{{REV}}" }
winit = { git = "https://github.com/anomalyco/rune", rev = "{{REV}}" }
sctk-adwaita = { git = "https://github.com/anomalyco/rune", rev = "{{REV}}" }
parley_engine = { git = "https://github.com/anomalyco/rune", rev = "{{REV}}" }
accesskit_windows = { git = "https://github.com/anomalyco/rune", rev = "{{REV}}" }
accesskit_atspi_common = { git = "https://github.com/anomalyco/rune", rev = "{{REV}}" }
accesskit_macos = { git = "https://github.com/anomalyco/rune", rev = "{{REV}}" }
```

The library target is named `rune`, so Rust code imports `use rune::...;` directly:

```rust
use rune::{AppSpec, Cx, El, col, text};

fn view(_: &(), _: &mut Cx<()>) -> El {
    col().w_full().h_full().center().child(text("Hello, Rune"))
}

fn main() {
    rune::run(AppSpec::new("Hello", (400.0, 240.0), || (), view));
}
```

On Linux, install `libxkbcommon-dev` and `libwayland-dev` (or distro equivalents). On Windows, the MSVC toolchain (`x86_64-pc-windows-msvc`) is recommended.

- Repository: <https://github.com/anomalyco/rune>
- Framework guide: <https://github.com/anomalyco/rune/blob/{{REV}}/crates/rune/README.md>
- Site: <https://anoma.ly/team/rune>
- License: MIT
