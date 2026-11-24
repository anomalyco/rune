# Live values

Sixteen speaking rings at 60 Hz, four remote cursors and a rotating dial needle at
120 Hz, plus wrapped (`.max_lines(2)`) and truncated (`.truncate()`) `LiveText`
readouts. The window uses a background producer; headless runs use the same scripted
values at virtual time. Updates stop after four seconds, then the window sleeps.
`--scene smooth` adds an interruptible 160 ms, zero-bounce spring to the cursors and
needle.

```rust
let level = rune::Live::new(0.0_f32);
let cursor = rune::Live::new([0.0_f32, 0.0]);
let angle = rune::Live::new(0.0_f32);
let caption = rune::LiveText::new("Angle 0.00 rad");
// Clone into a producer thread; no view invalidation or explicit Waker needed.
level.set(0.8);
cursor.set([120.0, 64.0]);
angle.set(std::f32::consts::FRAC_PI_4);
caption.set("Angle 0.79 rad (wrapped live caption)");
let ring = rune::el().live_opacity(&level);
let pointer = rune::el().live_offset(cursor.smooth(rune::Spring::new(0.16, 0.0)));
let needle = rune::el().origin(0.5, 1.0).live_rotation(&angle);
let label = rune::text("Placeholder").size(120.0, 40.0).max_lines(2).live_text(&caption);
```

```sh
cargo build --release -p live-values
target/release/live-values --snap live.png --at 2
target/release/live-values --scene smooth --trace live.jsonl --frames 840 --fps 120

cargo build --profile perf -p live-values
target/perf/live-values --bench
```

`--rebuild` selects the reference poll/view/layout path. `--bench` compares both
paths with one warmup and seven measured runs, reporting median CPU frame time.
It includes synthetic producer work but excludes wgpu/driver work. Wake/idle
counters model 120 Hz consumption; they are not OS power or native presentation
measurements. Ordinary headless `--frames` deliberately forces frames even at rest.

Bindings override the corresponding authored property, preserving layout. Offset
is additive in parent points; scale and rotation pivot about `El::origin` (default
centre), with rotation in clockwise radians. Fill is linear
RGBA. Bindings on removed nodes disconnect while the last pose exits. Shared
sources independently wake each subscribed, visible window. First mount lands
immediately; smoothing obeys Rune's reduced-motion policy.

`.live_rect(&Live<[f32; 4]>)` controls `[x, y, width, height]` in parent points
without reflowing siblings or rewrapping text. This also supports a meter's fill
width. `.live_fill(&Live<Color>)` and `.live_scale(&Live<f32>)` use the same path.

Values use coherent 128-bit atomic snapshots, including concurrent producers.
Desktop CPUs must support lock-free 128-bit atomics (`Live::new` checks). `set`
ignores NaN and clamps ±inf, so audio callbacks never panic. Publication and CPU presentation allocate nothing after mounting;
platform wake callbacks are not hard-real-time guarantees. Subscription metadata
is one cell per source/runtime pair, reused across view rebuilds and freed when
the last source handle dies. Keep sources scoped to their app/window lifetime.
