# Rune

A Rust GPU UI framework (wgpu, winit) for apps that feel like SwiftUI on iOS at 120 Hz on Mac, Windows, Linux, web and iOS. Views are plain Rust. Apps pin `dev` revs.

## Layout

- `crates/rune`: layout, runtime, renderer, windows, overlays, dev tooling. `crates/rune-kit`: components. `crates/rune-mermaid`: Mermaid diagrams (flowcharts, sequence and state diagrams). `crates/rune-motion`: springs and transitions. `crates/rune-fx`, `crates/rune-light`: effects. `crates/rune-ref`: reference renderer. `crates/procwatch`: an app's own memory and CPU (footprint, counting allocator, gauges, warnings, loopback JSON); libc only, no Rune dependency.
- `examples/`: apps and labs. `themes/`: themes. `site/`: the docs site (see `site/README.md`).
- `PLAN.md`: direction and milestones. `research/`: notes behind decisions. `demo/`: images the docs show.

## Commands

- `cargo test -p rune -p rune-kit --features rune-kit/markdown`
- `cargo check --workspace --all-targets`
- Tests run in debug. `--release` is incremental, for looking at an app. Benchmarks use `--profile perf`; the runtime bench scripts refuse other builds.

## Changes

- Work on `dev`. `production` deploys the site.
- `dev` is fast-forward-only: never force-push or rewrite it. Apps pin its revs, so a rewrite breaks every app's build on a clean fetch.
- For any change that can move an app's output or break its build, add `changelog/unreleased/<YYYY-MM-DD>-<slug>.md` with one top-level bullet, in the style of the `CHANGELOG.md` entries. `scripts/changelog-fold` folds them into `## dev`.
- Keep Rust files under 800 non-test lines (`#[cfg(test)]` code excluded); 1,500 is a hard cap. The point is to stop god files and edit collisions, not to chop a file that reads fine as one piece. Older files over the cap are listed in `scripts/file-size-exceptions.txt` with ceilings that only go down. Run `scripts/file-size.sh` to check.
- Put tests in a sibling `tests.rs` or `*_tests.rs`, and keep any test file or module under 1,000 lines.
- Before adding to a file over the cap, move the area you touch into its own module, in its own PR.
- A feature owns its state, keys and registrations so it plugs in with one line. New runtime state goes in the owning subsystem's struct, not a flat `Runtime` field.
- In `lib.rs`, keep `mod` and `pub use` lists one item per line, sorted.
- Apps build these types as literals, so adding a pub field breaks them: `AppSpec`, `Env`, `Elevation`, `PaletteSeed`, `OverlaySpec`, `TextStyle`, `RunOptions`, `ScrollSpec`, `Span`, `ByteFrame`, `SurfaceDraw`, `LayerSpec`. Add a builder method instead.

## Performance

App-side rules live in "Launching fast" in `crates/rune/README.md` and the "Launch fast" docs page. In examples and in Rune itself:

- Start slow work (network, indexing, status items) from `on_ready`, after the first frame. Land state that arrives just after launch with `tx.instant()`, so the first second stays still.
- `poll` returns true only when visible state changed, and a loop stops asking for frames when its work ends. An idle window draws nothing.
- Read sizes with `Cx::layout_size` or `Cx::layout_with`, not whole rects, so a scroll doesn't rebuild the view every frame. A handler that only moves a live value (scroll, zoom, a transform) calls `Tx::live_only()`.
- Build only mounted rows: `virtual_rows`, with row content built inside its closure; `memo` for rows that rarely change.
- Bound every cache in entries and in bytes, counting decoded pixels, not encoded size.
- Take numbers from `--profile perf`. Compare paired, alternating runs by median, keep outliers, and say what a headless number leaves out (GPU upload, presentation).
- A motion change passes `scripts/motion-audit` with its scenario added. Fix a finding in Rune rather than allowing it.

## This repo is public

Everything pushed stays public in history. Before committing, run `git diff --cached | rg -n '/Users/|/private/var/|\.ts\.net|ses_[A-Za-z0-9]{20}'` and keep these out:

- secrets, tokens and credential file contents;
- home folder and temp paths, machine names and tailnet hostnames. Write repo-relative paths, and use `host.example.ts.net` in tests;
- session IDs, chat or Slack links, and quotes from private sessions, chats or calls;
- teammates' names attached to their plans, opinions or internal discussions, and private repos or branches;
- quotes of, or citations to, Anomaly's internal guides and rules (Abattoir). Say what the code does and why on its own terms;
- agent handoff notes (`REVIEW-TO-PARENT.md` and similar), raw evidence, logs and benchmark dumps;
- generated videos and strips nothing links to. Commit the stills a doc shows.
