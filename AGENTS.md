# Rune

A Rust GPU UI framework (wgpu, winit) for apps that feel like SwiftUI on iOS at 120 Hz on Mac, Windows, Linux, web and iOS. Views are plain Rust. Apps pin `dev` revs.

## Layout

- `crates/rune`: layout, runtime, renderer, windows, overlays, dev tooling. `crates/rune-kit`: components. `crates/rune-motion`: springs and transitions. `crates/rune-fx`, `crates/rune-light`: effects. `crates/rune-ref`: reference renderer. `crates/procwatch`: an app's own memory and CPU (footprint, counting allocator, gauges, warnings, loopback JSON); libc only, no Rune dependency.
- `examples/`: apps and labs. `themes/`: themes. `site/`: the docs site (see `site/README.md`).
- `PLAN.md`: direction and milestones. `research/`: notes behind decisions. `demo/`: images the docs show.

## Commands

- `cargo test -p rune -p rune-kit --features rune-kit/markdown`
- `cargo check --workspace --all-targets`
- Tests run in debug. `--release` is incremental, for looking at an app. Benchmarks use `--profile perf`; the runtime bench scripts refuse other builds.

## Changes

- Work on `dev`. `production` deploys the site.
- `dev` is fast-forward-only: never force-push or rewrite it. Apps pin its revs, so a rewrite breaks every app's build on a clean fetch.
- Add a `CHANGELOG.md` `## dev` entry, newest first, for any change that can move an app's output or break its build.
- Apps build these types as literals, so adding a pub field breaks them: `AppSpec`, `Env`, `Elevation`, `PaletteSeed`, `OverlaySpec`, `TextStyle`, `RunOptions`, `ScrollSpec`, `Span`, `ByteFrame`, `SurfaceDraw`, `LayerSpec`. Add a builder method instead.

## This repo is public

Everything pushed stays public in history. Before committing, run `git diff --cached | rg -n '/Users/|/private/var/|\.ts\.net|ses_[A-Za-z0-9]{20}'` and keep these out:

- secrets, tokens and credential file contents;
- home folder and temp paths, machine names and tailnet hostnames. Write repo-relative paths, and use `host.example.ts.net` in tests;
- session IDs, chat or Slack links, and quotes from private sessions, chats or calls;
- teammates' names attached to their plans, opinions or internal discussions, and private repos or branches;
- quotes of, or citations to, Anomaly's internal guides and rules (Abattoir). Say what the code does and why on its own terms;
- agent handoff notes (`REVIEW-TO-PARENT.md` and similar), raw evidence, logs and benchmark dumps;
- generated videos and strips nothing links to. Commit the stills a doc shows.
