<!-- tallow -->
# Rune

A Rust GPU UI framework (wgpu, winit) for apps that feel like SwiftUI on iOS at 120 Hz on Mac, Windows, Linux, web and iOS. Views are plain Rust. Apps pin `dev` revs.

## Layout

- Core crates: `crates/rune`: layout, runtime, renderer, windows, overlays, dev tooling. `crates/rune-core`: foundation primitives (colour, tokens, themes, shapes, state machines, clocks, tasks, tracked reads, growth, live values, latest-wins requests, marks, dirs, prefs; no GPU or windowing). `crates/rune-gpu`: GPU adapter selection, backend dispatch, device error logging, native Apple Metal backend and analytic GPU springs. `crates/rune-image`: content-addressed image decoding, linear-light mips, pixel buffers, SVG/AVIF codecs, OS app icons and placement math. `crates/rune-spell`: platform spellchecking (Windows `ISpellChecker`, macOS `NSSpellChecker`, Linux and web Hunspell). `crates/rune-media`: video and audio playback, zero-copy native surfaces (IOSurface, D3D shared textures, dmabufs), and audio recording. `crates/rune-text`: text shaping, layout, font discovery, glyph rasterization and icons. `crates/rune-edit`: the text editing model (`TextEdit`, keys and modifiers, IME composition, undo history, rich text and its HTML form). `crates/rune-motion`: springs and transitions.
- Layering: `rune-motion`, `rune-staging` and `rune-taffy` have no Rune dependencies; `rune-core` depends on `rune-motion`; `rune-gpu` depends on `rune-motion` (and `rune-staging` under `metal-backend`); `rune-image` and `rune-spell` depend on `rune-core`; `rune-text` depends on `rune-core`, `rune-image` and `rune-motion`; `rune-edit` depends on `rune-core` and `rune-text`; `rune-media` depends on `rune-core`, `rune-gpu`, `rune-image` and `rune-staging`; `crates/rune` depends on all of them (plus optional `procwatch`) and re-exports their public APIs so apps depend on `rune` only. Above `rune` sit `rune-kit` (also `rune-motion` and optional `rune-highlights`, `rune-mermaid`), `rune-mermaid`, `rune-knobs`, `rune-luau` and optional `rune-ref`; beside it sit `rune-fx` (`rune-motion`, `rune-staging`), `rune-light` (`rune-staging`), `rune-mac` (`rune-notify`) and `rune-perf` (`procwatch`).
- Companion crates: `crates/rune-kit`: components. `crates/rune-mermaid`: 17 Mermaid diagram types (flowcharts, sequence, state, class, ER, gantt, pie, mindmap, timeline, user journey, git graph, quadrant, XY chart, sankey, block, requirement and C4). `crates/rune-i18n`: message catalogs generated from JSON, CLDR plurals and numbers, the system's languages. `crates/rune-fx`, `crates/rune-light`: effects. `crates/rune-ref`: reference renderer. `crates/rune-register`: distributed registration slices for scenes, stories and knobs. `crates/procwatch`: an app's own memory and CPU (footprint, counting allocator, gauges, warnings, loopback JSON); libc only, no Rune dependency.
- `examples/`: apps and labs. `themes/`: themes. `site/`: the docs site (see `site/README.md`).
- `vendor/winit`: winit as published plus `vendor/winit.patch` (Windows: no TaskbarList COM call when a window is made, and system DLLs loaded with `LOAD_LIBRARY_SEARCH_SYSTEM32`; and `WindowExtWayland::content_corner_radius`, the radius a client-side frame leaves the content's bottom corners to; Wayland window icons through `xdg-toplevel-icon-v1`, backported from rust-windowing/winit#4288; Wayland `wayland_wm_state` configure getters/wake plus X11 `PropertyNotify` `take_wm_state_changed`; and Wayland `current_monitor` skipping destroyed outputs + deduplicating `WinitState.monitors` (`new_output` upsert / `output_destroyed` `retain`) + waking `AboutToWait` on output destroy/enter/leave), through `[patch.crates-io]`. When winit is bumped, run `scripts/vendor-winit.sh <new version>` (Git Bash on Windows) and update the `[patch.crates-io]` rev in apps that build Rune.
- `vendor/sctk-adwaita`: sctk-adwaita as published plus `vendor/sctk-adwaita.patch` (the client-side frame's one-point border follows the window's rounded bottom corners, and `content_corner_radius` says what radius the content leaves them), through `[patch.crates-io]`. When it is bumped, run `scripts/vendor-sctk-adwaita.sh <new version>` (Git Bash on Windows) and update the `[patch.crates-io]` rev in apps that build Rune.
- `vendor/tiny-skia-path`: tiny-skia-path as published plus `vendor/tiny-skia-path.patch` (Skia's own conic splitting and stroker steps, so icons stroke as Chromium's Skia strokes them), a plain path dependency of `crates/rune-text`, not a patch (resvg keeps the published one). When it is bumped, run `scripts/vendor-tiny-skia-path.sh <new version>` (Git Bash on Windows).
- `vendor/wgpu-hal`: wgpu-hal as published plus `vendor/wgpu-hal.patch` (`dx12::Instance::expose_adapter`, FXC loaded on first compile, Windows system DLLs loaded with `LOAD_LIBRARY_SEARCH_SYSTEM32`, a Metal device lost when a command buffer fails, with the fence of gfx-rs/wgpu#10570 and #10875, Linux `vulkan::Instance::with_direct_drivers` for exclusive `VK_LUNARG_direct_driver_loading`, GLES lazy shader compile-status checks plus opt-in `KHR_parallel_shader_compile` deferred program links, and Vulkan `SwapchainAcquireSemaphore`/`SwapchainPresentSemaphores` `Drop`-owned destruction so swapchain teardown after submit-time `Device::Lost` never panics while `wgpu-core`'s `Surface::drop` still holds an unpresented `acquired_texture`, and process-wide serialization of raw D3D12 `CreatePipelineState`/`CreateGraphicsPipelineState`/`CreateComputePipelineState` calls against the unlocked `PassRegistry` `DenseMap` race in `dxilconv.dll`), through `[patch.crates-io]`. When wgpu is bumped, run `scripts/vendor-wgpu-hal.sh <new wgpu-hal version>` (Git Bash on Windows) and update the `[patch.crates-io]` rev in apps that build Rune.
- `vendor/naga`: naga as published plus `vendor/naga.patch` (`back::pipeline_constants::process_function` remaps `Statement::Emit` ranges from each original expression's newly appended index range instead of `Range::new_from_bounds(new_pos[first], new_pos[last])`, fixing `ExpressionAlreadyInScope` and inverted ranges when constant folding reuses an earlier handle; gfx-rs/wgpu#7793), through `[patch.crates-io]`. When wgpu/naga is bumped, run `scripts/vendor-naga.sh <new naga version>` (Git Bash on Windows) and update the `[patch.crates-io]` rev in apps that build Rune.
- `vendor/accesskit_atspi_common`: accesskit_atspi_common as published plus `vendor/accesskit_atspi_common.patch` (reads an item node's own `size_of_set` on item roles before falling back to `size_of_set_from_container`, matching `vendor/accesskit_windows.patch`), through `[patch.crates-io]`. When AccessKit is bumped, run `scripts/vendor-accesskit-atspi-common.sh <new version>` (Git Bash on Windows) and update the `[patch.crates-io]` rev in apps that build Rune.
- `ARCHITECTURE.md`: how Rune works today (crate layering, single-thread frame flow, state ownership, hosts, renderer). `PLAN.md`: direction and milestones. `research/`: notes behind decisions. `demo/`: images the docs show.

## Commands

- `cargo test -p rune -p rune-mermaid -p rune-kit --features rune-kit/markdown,rune-kit/mermaid`
- `cargo check --workspace --all-targets`
- `bash scripts/check-local.sh` runs opt-in local checks (file sizes, folder shape, public-repo hygiene, `cargo check`, `cargo clippy --lib --bins`; supports `--wasm`, `--hot`, `--features`, `--all`, `--tests`).
- Cargo aliases in `.cargo/config.toml`: `cargo lint`, `cargo lint-all`, `cargo check-wasm`, `cargo check-webgl`, `cargo check-hot`, `cargo check-gpu-tests`, `cargo check-win-material`, `cargo check-linux-noop`, and `cargo test-fast`.
- Workspace-wide commands (`cargo check --workspace --all-targets`, `cargo test --workspace`, `cargo lint-all`, `scripts/check-local.sh --all`) build `crates/rune-luau` and `examples/luau-panel`, which compile Luau's C++ sources via `mlua` (`mlua-sys`/`luau0-src`/`cc`) and require a host C++ compiler (`c++`/`g++`/`clang++`, or Visual Studio C++ build tools on Windows); default-member commands (`cargo check`, `cargo test`, `cargo lint`, `scripts/check-local.sh`) and per-package commands (`-p rune ...`, `cargo test-fast`) skip `rune-luau` and need no C++ compiler.
- Web checks: `cargo check-wasm` and `cargo check-webgl` (or `scripts/check-local.sh --wasm`) treat warnings as warnings for everyday development.
- Strict web checks: `cargo lint-wasm` and `cargo lint-webgl` (or `scripts/check-local.sh --strict`) deny warnings in `rune`.
- Tests that count GPU work or need wgpu's no-op backend are behind features so plain builds don't compile those backends: add `--features rune/test-gpu-counters,rune-gpu/test-gpu-counters` (and `rune/test-gpu-noop` on Linux) to run them. `rune-ref`, `rune-i18n` and `rune-luau` are not default members: test them with `-p`.
- On a GPU-less Linux host (such as a container without Vulkan), opt into skipping real-GPU tests and running non-pixel GPU tests on wgpu's no-op backend with `RUNE_ALLOW_NO_GPU=1 cargo test -p rune -p rune-mermaid -p rune-kit --features rune-kit/markdown,rune-kit/mermaid,rune/test-gpu-noop --no-fail-fast` (unset by default so missing adapters still fail on hosts with a GPU).
- Tests run in debug. `--release` is incremental, for looking at an app. Benchmarks use `--profile perf`; the runtime bench scripts refuse other builds.
- CI's macOS tests (`.github/workflows/macos-tests.yml`) run on a schedule and on `workflow_dispatch`, not per pull request or push: the owner's decision on 2026-11-12. Don't add those triggers back; run the workflow by hand when a change needs it.

## Changes

- Work on `dev`. `production` deploys the site.
- `dev` is fast-forward-only: never force-push or rewrite it. Apps pin its revs, so a rewrite breaks every app's build on a clean fetch.
- For any change that can move an app's output or break its build, add `changelog/unreleased/<YYYY-MM-DD>-<slug>.md` with one top-level bullet, in the style of the `CHANGELOG.md` entries. `scripts/changelog-fold` folds them into `## dev`.
- Keep Rust files under 800 non-test lines (`#[cfg(test)]` code excluded); 1,500 is a hard cap. The point is to stop god files and edit collisions, not to chop a file that reads fine as one piece. Older files over the cap are listed in `scripts/file-size-exceptions.txt` with ceilings that only go down. Run `scripts/file-size.sh` to check.
- Keep folders shaped by capability: `x.rs` or `x/` beside two or more `x_*.rs` (or three or more `x_*.rs` alone) means move them into `x/`; a folder over 40 source files warns and over 60 is an error (tests aren't counted; `examples/`, `benches/` and `bin/` are skipped). `scripts/folder-shape.sh` runs inside `scripts/file-size.sh`; old violations are in `scripts/folder-shape-exceptions.txt` with ceilings that only go down.
- Put tests in a sibling `tests.rs` or `*_tests.rs`, and keep any test file or module under 1,000 lines.
- Before adding to a file over the cap, move the area you touch into its own module, in its own PR.
- A feature owns its state, keys and registrations so it plugs in with one line. New runtime state goes in the owning subsystem's struct, not a flat `Runtime` field.
- In `lib.rs`, keep `mod` and `pub use` lists one item per line, sorted.
- Apps build these types as literals, so adding a pub field breaks them: `AppSpec`, `Env`, `Elevation`, `PaletteSeed`, `OverlaySpec`, `TextStyle`, `RunOptions`, `ScrollSpec`, `Span`, `ByteFrame`, `SurfaceDraw`, `LayerSpec`. Add a builder method instead.

## State

- Behaviour with modes or phases (hover delays, drags, IME composition, scroll modes, requests and retries) is an explicit state machine: one `enum State` whose variants carry only the data valid in them, and one `fn step(state: &mut State, event: Event) -> Effects<Effect>` (`rune::machine`) that returns effects instead of performing them. No clusters of booleans, `Option`s and timestamps whose combinations encode the mode.
- Timers are events: a timed variant stores its `Attempt` and due time on `Tx::clock`/the runtime clock, the caller turns its effect into `Tx::wake_at`, and `poll` (or the runtime's frame) feeds the timer back into `step`. An answer whose attempt the state no longer holds changes nothing.

## Performance

App-side rules live in "Launching fast" in `crates/rune/README.md` and the "Launch fast" docs page. In examples and in Rune itself:

- Start slow work (network, indexing, status items) from `on_ready`, after the first frame. Land state that arrives just after launch with `tx.instant()`, so the first second stays still.
- `poll` returns true only when visible state changed, and a loop stops asking for frames when its work ends. An idle window draws nothing.
- Read sizes with `Cx::layout_size` or `Cx::layout_with`, not whole rects, so a scroll doesn't rebuild the view every frame. A handler that only moves a live value (scroll, zoom, a transform) calls `Tx::live_only()`.
- Build only mounted rows: `virtual_rows`, with row content built inside its closure; `memo` for rows that rarely change.
- Bound every cache in entries and in bytes, counting decoded pixels, not encoded size.
- Take numbers from `--profile perf`. Compare paired, alternating runs by median, keep outliers, and say what a headless number leaves out (GPU upload, presentation).
- A motion change passes `scripts/motion-audit` with its scenario added. Fix a finding in Rune rather than allowing it.
- No file I/O in the frame loop: never read, stat, write, sync or rename a file on the UI thread during frames, input or animation. On Windows each open and `MoveFileExW` rename runs antivirus filter callbacks and can fail with a sharing violation.
- No background rewrite churn: diagnostics and logs (`rune-perf`, traces) keep one open handle and write a summary on an incident or at shutdown, never on a repeating timer while idle.
- Keep persistence APIs free of `std::fs` and `PathBuf` so `wasm32` targets can back them with browser storage.

## This repo is public

Everything pushed stays public in history. Before committing, run `git diff --cached | rg -n '/Users/|/private/var/|\.ts\.net|ses_[A-Za-z0-9]{20}'` and keep these out:

- secrets, tokens and credential file contents;
- home folder and temp paths, machine names and tailnet hostnames. Write repo-relative paths, and use `host.example.ts.net` in tests;
- session IDs, chat or Slack links, and quotes from private sessions, chats or calls;
- teammates' names attached to their plans, opinions or internal discussions, and private repos or branches;
- quotes of, or citations to, internal guides and rules. Say what the code does and why on its own terms;
- agent handoff notes (`REVIEW-TO-PARENT.md` and similar), raw evidence, logs and benchmark dumps;
- generated videos and strips nothing links to. Commit the stills a doc shows.
