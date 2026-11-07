# Motion bento

A 1440×900 control center whose twelve tiles all move at once: a segmented scene picker, a
thermostat ring, light switches, mixer faders, an XY pad, a card deck, an activity feed, and a
bottom row of five tiles that reflow around whichever one has focus. A seeded chaos schedule taps
them (a few times a second, with bursts of six to twelve taps 30–60 ms apart), so nearly every
frame has springs in flight.

`chaos/` (`motion-bento-chaos`) holds everything that isn't Rune: the model, the schedule, the
look (colours, grid, spring tunings) and the frame meter. It has no Rune dependency, so another
toolkit can build the same window and play the same schedule.

```sh
cargo run -p motion-bento --profile perf -- --seed 7 --duration 30 --intensity 1.5 --hud
```

| Flag | Default | |
| --- | --- | --- |
| `--seed N` | 1 | Names one schedule, and every random choice its taps make. |
| `--duration S` | 20 | Seconds of chaos. The window quits a second later. Rune's headless runs read it too. |
| `--intensity X` | 1 | Scales the rate of taps: 2 is twice as many. |
| `--variant NAME` | `default` | Spring tuning: `default`, `bouncy` or `stiff`. |
| `--metrics PATH` | | Write the frame meter's JSON when the run ends. |
| `--hud` | off | Show the frame meter from the start. `H` toggles it. |
| `--stay` | off | Keep the window open after the schedule ends. |
| `--emit-script PATH` | | Write the schedule as a Rune headless script and exit. |

## Headless

Every action is a tap on a keyed element (`Action::key`), so the schedule is also a script:

```sh
motion-bento --seed 7 --duration 8 --emit-script chaos.json
motion-bento --seed 7 --duration 8 --script chaos.json --video bento.mp4 --size 1440x900
motion-bento --seed 7 --duration 8 --script chaos.json --tree end.json --at mark:end
```

With a script, the window plays its taps instead of the schedule, and both reach the same state
(`script_taps_reach_the_same_state_as_the_schedule`).

## The meter

Measured from `App::on_frame_encoded`, for drawn frames only:

- **work**: `update_ms + encode_ms`, from the frame's start (after its drawable came) until its
  commands were submitted: events, `poll`, the view, layout, paint, uploads and encode. CPU wall
  time, not GPU time.
- **interval**: time between frame starts. A display-paced window starts each frame on a refresh,
  so this is the gap between frames on screen.
- **late**: intervals over 1.5 refreshes (`FrameEncoded::refresh_hz`), and the refreshes they
  skipped. Gaps over 100 ms are the window resting, not late frames.
- **CPU** and **footprint**: process CPU time over the run's wall time, and the physical footprint,
  from `procwatch`.
