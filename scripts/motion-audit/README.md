# Motion audit

Per-frame traces of every element in the example apps, and a checker that flags motion a user
would notice: snaps, pops, neighbours that jump, exits that vanish before they play, overshoot,
motion that never settles, and work spent on things nobody can see.

```sh
scripts/motion-audit/run.sh                        # record all scenarios into target/motion-audit
python3 scripts/motion-audit/analyze.py target/motion-audit/*.jsonl [--quiet] [--json out.json]
python3 scripts/motion-audit/track.py target/motion-audit/oc-send.jsonl composer/editor 80 120
python3 scripts/motion-audit/test_analyze.py        # analyze.py on synthetic traces
```

`run.sh` records each scenario headless at 120 fps on the virtual clock with
`--trace-all --trace`. `--trace-all` adds unkeyed elements (named by path) to the usual
motion-lint JSONL, and every trace now says why each frame animated (`quality.why`) and which
tracks it sampled (`tk`: `[element id or path, property, active]`; inactive means settling).

`analyze.py` reports, per trace:

| Finding | Meaning |
| --- | --- |
| `invisible-work` | tracks sampled on an element that is transparent, off-window, clipped or occluded |
| `idle-frames`, `never-settles` | frames that animate with no visible change; motion still running at the end |
| `pop-in`, `pop-out` | an element appears or disappears at full opacity and size with no transition |
| `appear-then-jump` | mounts in one place, then moves there in a frame |
| `jump`, `step`, `sibling-jump` | a one-frame change in position or size, on its own or next to something animating, seen on both frames (a loop wrapping while transparent or clipped is not one) |
| `overshoot`, `jiggle` | passes its target, or reverses direction more than once |
| `latency`, `hitch` | input with no motion for several frames; a frame over budget |
| `cut-exit` | unmounts partway through its fade: nothing leaves above opacity 0 |
| `reversal`, `dog-leg` | one motion heads the wrong way first or goes out and back; a route much longer than its chord |
| `stall` | one motion's speed drops for a frame and picks up again, as a retarget that lost part of its velocity does |

An app's scenario can say what each input should do with `"expect"` on its script event:

| Expect | Checks | Finding |
| --- | --- | --- |
| `instant` | everything in `scope` (an element name part) lands within a frame or two of the input; pops and jumps there are intended | `not-instant` |
| `follow` | a drag's `element` moves 1:1 with the pointer while it moves (it may stop at a limit) | `lags-hand` |
| `smooth` | after the input, `element` travels over several frames, not in one | `snap` |
| `leaves` | `element` is what leaves, not a sibling (index keys remove the last row) | `wrong-exit` |

```sh
python3 scripts/motion-audit/analyze.py --script s.json trace.jsonl --allow allow.json --fail
```

`--allow` takes a JSON list of `{"trace", "kind" (or a list), "element" (a substring),
"reason"}` rules for intended behaviour; every rule needs a reason, and a rule for a trace
the run recorded that matched nothing is listed as unused. `--fail` exits non-zero when a finding isn't allowed.

A motion that hides what it moves (a fade to 0, a collapse, a slide out) runs on for a few
frames below what anyone can see, down to its settle threshold. Up to 250 ms of that tail,
ending as its track settles, isn't `invisible-work`. A loop (a spinner) runs for as long as
it's shown, so frames animating only loops aren't `idle-frames` or `never-settles`.

Elements that draw nothing and hold nothing (hit and hover zones, spacers) carry the trace's
blank flag (64). Nobody can see them come, go or move, so they don't pop, jump or count as
drawn.

`track.py` prints one element's frames (rect, opacity, flags, sampled tracks) to confirm a
finding. Add scenarios as script JSON under `scenarios/` and a `run` line in `run.sh`.

## An app's own motion gate (`rune::motion_gate`)

The scenarios above are Rune's examples, recorded as traces and read by `analyze.py`. An app
gates its own motion inside `cargo test`, on its real view and state, with `rune::motion_gate`:

- `Run` wraps the app's `Runtime`: `idle` settles it unrecorded, `step`, `play` and `act` record
  frames on the virtual clock (an `act` marks an input, so motion it retargets isn't held to one
  path), and `quiet` marks where the cell's motion should end.
- `check` lists what a person would see go wrong in a `Recording`: `pop-in`, `pop-out`,
  `cut-exit`, `jump`, `offset-jump`, `appear-then-jump`, `jerk`, `reversal`, `overshoot`,
  `jiggle`, `text-swap`, `fade-snap`, `flicker`, `load-motion`, `never-settles`, `loop-jump`,
  `wakes-after-end`, `lands-unseen`, `remount`, `unpinned` and `twice`. Thresholds and meanings are the `RULES` table. Elements are tracked by tree
  path, so a keyed row keeps its identity however the rows around it change.
- `gate(cells, ALLOW)` runs the app's cells (a name and a closure that records one) in parallel,
  checks them, and fails the test on any finding its allowlist doesn't cover. Each allowlist row
  is `(kind, element contains, cell contains, why)`, and needs its reason.

The app keeps its scenarios, its circumstance matrix (cell names like `send/pinned/long/wide`)
and its allowlist; Rune keeps the checker and the runner. The module docs show a whole test.
`gate` runs one:

```sh
scripts/motion-audit/gate -- -p my-app motion_gate                 # every cell
scripts/motion-audit/gate send/pinned -- -p my-app motion_gate     # cells whose name contains it
scripts/motion-audit/gate --allowed -- -p my-app motion_gate       # allowed findings too
scripts/motion-audit/gate --dump 'send/pinned/long/wide|composer|120|160' -- -p my-app motion_gate
scripts/motion-audit/gate --sheet 'send/pinned/long/wide|134|composer' -- -p my-app motion_gate
```

`--dump` writes one cell's tracks frame by frame (rect, visible rect, opacity, scale, flags);
`--sheet` writes a PNG strip of the frames around one, every element outlined as strongly as
its opacity and the named one in red. Both are evidence for reading a finding; the findings
are the gate. `MOTION_GATE_OUT` names the report (default `$TMPDIR/motion-gate`), and
`MOTION_GATE_WRAP` runs the test under a command prefix.

## Tracing the real app (`motion-trace.py`)

The gate runs headless on the virtual clock, so it says nothing about how long a real frame
takes. For that, an app can write its own trace in the installed build, one JSON line per key,
span and frame (the format is at the top of `motion-trace.py`), and read it:

```sh
python3 scripts/motion-audit/motion-trace.py trace.jsonl --key Enter --frames 40 --budget 16.7
```

For each matching key it prints how many frames passed before a watched element moved, then
each frame's update and render cost, marking the ones over budget, with the spans over 1 ms in
them.
