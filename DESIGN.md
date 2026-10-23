# Rune dev controls

## Rooms call site

```rust
let spec = rune::AppSpec {
    scenes: &[("lobby", lobby), ("joining", joining), ("room", room)],
    ..rune::AppSpec::new("Rooms", (960., 640.), init, view)
};
let app = rune::App::new(spec);
#[cfg(all(feature = "dev", debug_assertions))]
let app = app.dev_panel(rune::dev::Config::default().transitions([
    rune::dev::Transition::new("Join", "lobby", join),
    rune::dev::Transition::new("Leave", "room", leave),
]));
app.run();

fn view(state: &Rooms, cx: &mut rune::Cx<Rooms>) -> rune::El {
    let spring = rune::tune!("rooms.join.spring", rune::Spring::new(0.35, 0.0));
    let width = rune::tune!("rooms.card.width", 320.0_f32);
    let radius = rune::tune!("rooms.card.radius", 16.0_f32);
    let fill = rune::tune!("rooms.card.fill", rune::oklch(0.6, 0.12, 240.));
    // ordinary Rune view using those values
}
```

Rooms forwards its `dev` feature to `rune/dev`. The macro is selected in Rune,
not expanded into consumer-side feature checks. Without dev, or in a release
build, it expands to the default expression alone (including const contexts).
With dev compiled but no runtime panel opt-in, no listener or registry exists.
The default remains at its use site; explicit names survive code movement and
copy-as-code. An optional numeric bounds form supplies min/max/step.

The dev configuration lives on `App`, set only by `App::dev_panel`, so
`AppSpec` gains no field: Rooms, Hex and Deadbolt keep their exhaustive
`AppSpec { .. }` literals (a compile test in `examples/dev-panel` spells one).
Rooms' `list`, `room`, `live-list`, and `live-echo` registrations can be used
directly as transition sources; the names above just keep the API sketch short.

A frame-scoped thread-local registry makes the macro available to both view
helpers and transition handlers without threading a context through all of
Rooms. It restores the previous scope on unwind and isolates multiple runtimes.
Calls outside an attached runtime return their default. Registration is lazy;
controls encountered in earlier states remain available when states change.

## States and transitions

The panel reads `AppSpec::scenes` directly. No second scene registry. A state
selection rebuilds `AppSpec::init`, invokes the same scene function as tooling,
reconnects `AppSpec::setup`, and commits with settle. This matters for Rooms'
existing `("list", |_| {})` scene: it is a patch of the initial state, not a
reset function by itself. `App::init`'s one-shot captured initializer is not
replayed; repeatable fixtures belong in `AppSpec::init` and `scenes`.
A transition names its source scene and a `fn(&mut S, &mut Tx)`. Replay commits
and settles that source for one frame, then runs its action on the next frame.
The event-loop waker requests that second frame even if paused or at rest.
Selecting a new state cancels an outstanding replay.

## Wire protocol and UI

Rune owns a loopback WebSocket listener, default port 4793 (port 0 announces a
free port). Explicit Tailscale IP binding is allowed; wildcard/public binding
is rejected. The listener exists only inside an opted-in debug runtime, with
bounded messages/queues and owned shutdown. Socket threads enqueue commands
and wake the app; model/view mutation stays on the app thread.

Server `state` messages contain `descriptors`, complete `values`, and readouts.
Client messages: `{type:"set", key, value}` (`null` restores default),
`{type:"setBatch", changes:[{key,value}]}`, `{type:"reset", key?, group?}`,
`{type:"action", key}`, and `{type:"get"}`. Invalid requests return `error`.
Descriptors are knobs `ControlDescriptor`s, not a new Rune panel schema:

| Rune value | knobs descriptor |
| --- | --- |
| Scene | choice (`rune.state`) |
| Transition | action (`rune.transition.<name>`) |
| Scale | number, 0.1–1 (`rune.scale`) |
| Pause | toggle (`rune.paused`) |
| Step | action (`rune.step`), one 1/60 logical second |
| Spring | spring, `{duration,bounce}` |
| f32 | number, optional min/max/step |
| Color | color, CSS colour string |
| Frame time | watch |

The tiny Bun page adapter only connects the socket and calls
`createRemoteControls` + knobs `mountPage`. All controls, layout, history and
copy UI belong to knobs. Rust copy formatting is a requested knobs extension;
Rune will not fork its panel. The page can proxy the socket on the same origin
for a tailnet HTTPS preview.

## Clock hook (separate core commit)

Keep the existing production clocks untouched. An optional dev clock maps
host elapsed time to a monotonic logical clock before the runtime frame.
Scale/pause apply to both incoming animation dt and the app clock used by
`poll`/`Tx::wake_after`. As in a native shell, the logical clock follows host
wall time while animations step by the frame's own (capped) dt, so a spring
committed after idle never jumps by the idle time. Stored timer deadlines
remain logical. The host-facing
`wake_deadline()` converts remaining logical time back to wall time using the
current scale; paused clocks expose no deadline. Headless deadline subdivision
uses the internal logical deadlines. Manual step advances the same clock by
1/60 second while remaining paused. Existing view `Cx::time_scale` remains an
additional animation-only multiplier. Scale changes never multiply an absolute
timestamp or stretch spring parameters, so no pose discontinuity is introduced.

Tunables dirty the view. During a tuning commit, active same-target spring
tracks accept changed spring parameters and inherit their sampled velocity.
Ordinary commits retain the existing same-target trajectory fast path.

## Motion debugging and proof

Layout velocity vectors use Scene's analytic local-frame velocities, drawn at
each moving node's presented centre without changing the app tree. Motion blur
is an opt-in compositor effect in rune-fx, not a core Rune pass:
`Runtime::motion_blur_enabled()` is demonstrated in the existing lab's Motion
scene (`lab --features dev`). The small three-state example uses core rendering.

The example has lobby/joining/room scenes, Join/Leave transitions, and four
tunables. A headless protocol test verifies descriptors, scene tree, the settled
source frame, action frame, changed spring trajectory, pause/step, scaled timer
deadlines and validation. A socket smoke test exercises the same dispatcher.
All cargo runs use one HiveMind worker job at a time, never Studio cargo.

Commands first bring existing motion/timers to their arrival timestamp, then
commit the change with zero elapsed time. This prevents an idle interval from
being applied to a newly replayed transition. The dev clock is a global
multiplier; an app's explicit `Cx::time_scale` still affects animation alone.

## Repository notes

Read BRANCH-FROM-PARENT.md and PLAN.md §5. No AGENTS.md exists in this worktree
or the primary Rune checkout at task start. Parent branch notes are untracked
and will not be committed. Work stays local on dev-panel.
