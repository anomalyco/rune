# Dev panel handoff

Worktree: `/Users/kit/code/worktrees/rune/dev-panel`, branch `dev-panel`.
Base: `origin/dev` at `28ef718`. Local commits only; no push.

## What is implemented

- `App::dev_panel(Config)` opts a debug runtime into a loopback/Tailscale
  WebSocket. Release builds omit the module even with `--features dev`.
- Existing `AppSpec::scenes` feed the panel. Scene jumps rebuild the spec's
  initial state, apply the fixture, reconnect setup, and settle. This supports
  Rooms' no-op `list` fixture. Replay settles its source for one frame and runs
  the action on the next. A replacement scene cancels pending action side effects.
- `rune::tune!` exposes `Spring`, `f32` (optional bounds), and `Color` through
  knobs descriptors. The disabled macro expands to the default expression;
  const compilation verifies its key/range expressions are not resolved.
- The clock scales animation and poll/timer deadlines together. Pause, manual
  1/60-second steps, scaled host deadlines, and native idle handling are covered.
- Live spring changes re-commit and preserve analytic position/velocity when
  replacing a running view-declared spring. The ordinary unchanged-target fast
  path remains intact outside tuning commits.
- Layout-velocity vectors are an optional draw overlay. The existing lab's
  rune-fx Motion scene obeys `Runtime::motion_blur_enabled()`.
- `examples/dev-panel` has three states, two transitions, and four tunables.
  `scripts/dev-panel` is socket/page glue for the real knobs library, with no
  hand-written panel controls.

## Core landing surface

Local implementation commits, in order:

1. `48b8d98`: separate clock bridge and dev feature.
2. `71244d0`: dev protocol, tunable registry, macro and rune-motion retuning.
3. `a198905`: core integration hooks, isolated for the parent's landing queue.
4. `c376fab`: example, lab blur integration, protocol proof, knobs adapter and design.

Apply the implementation stack together: the backend and integration commits
reference each other, and the example commit supplies the updated lockfile.
The clock commit is independently usable.

Core integration consists of feature-gated fields/frame dispatch in
`runtime.rs`, attachment in `tooling.rs`/`app.rs`, and a six-line hidden-host
dev wake path in `window.rs`. `overlay.rs` has only a test fixture conversion
to `..AppSpec::new`. Retuning adds dev-only helpers in rune-motion store/scene.

## Proof

All cargo work ran through HiveMind, one job at a time, on the Mini after Pro
was full. No cargo ran on Studio.

- Four focused dev tests: clock mapping, timer subdivision/pause/step, registry
  isolation/unwind/CSS colour handling, and idle/replay cancellation.
- Real-WebSocket example test: schema, scene tree, settled source frame,
  transition, in-flight spring continuity and analytic inherited velocity,
  pause/step, atomic rejection, reset, and listener shutdown.
- `cargo check -p lab --features dev`.
- `cargo check -p rune-dev-panel --no-default-features`.
- `cargo check -p rune-dev-panel --release --features dev`.
- Worker-built example executed windowless on Studio. Its real socket changed
  the tree to Joining and changed the spring to 0.55 seconds. The process was
  stopped after the smoke. No window, GPU allocation, or screen capture.
- The latest build, including fresh-initial-state scene resets, passed that
  Studio smoke again. Independent Claude review of the complete diff from
  `28ef718` returned **No blockers**. `git diff --check` passed.

Reproduce the focused checks with HiveMind running
`bash scripts/dev-panel/check.sh`; `delta` runs dev tests/build, `build` only
builds. `bun scripts/dev-panel/smoke.ts /path/to/worker-built/rune-dev-panel`
checks the native windowless host.

## External dependency still being coordinated

The KNOBS session (`ses_ec91ede28ffeFpn6L7FZtGoka0`) has two queued asks for
`formatRust(descriptors, values, group?)` / `RemoteControls.rust()`. The adapter
uses `formatRust` in knobs' own `mountPage` Copy callback and excludes reserved
`rune.*` playback keys. At this writing that export is absent; the page cannot
bundle until KNOBS supplies it. No knobs files were edited. Browser/panel proof
and Rust-copy verification are consequently pending. No clip was captured.
The bounded ten-minute formatter watch expired. No listener, preview, review,
build or watch is left running. Generated binaries were removed after the smoke.

## Rooms integration notes

See `DESIGN.md` for the API and clock rationale. Forward a Rooms `dev` feature
to `rune/dev`; add `..AppSpec::new(...)` to its currently exhaustive spec
literal; attach `Config::transitions([...])`. Keep tune reads in the view's
motion/size/colour declarations for immediate in-flight retuning. Compositors
advertise `motion_blur: true` and read `rt.motion_blur_enabled()`.

Scene reset uses repeatable `AppSpec::init`, not `App::init`'s one-shot captured
closure. The scene picker records the last explicitly selected source fixture;
actions do not infer a destination scene from arbitrary app state. Velocity
vectors show local layout-track velocity at the node's presented centre.

The parent branch note remains untracked and is never committed. No AGENTS.md
was present in this worktree or the primary Rune checkout.
