# Native presentation pacing

Timestamp-only probe on a GUI Mac. No screen capture. It measures `nextDrawable` entry to
positive Metal `presentedTime`, using the overlay team's steady-state endpoint.
Three batches per mode, two-second warmup, then three three-second intervals;
the summary is the median of nine interval medians.

Build with `cargo build --profile perf -j 2 -p rune --example pacing_overlay --example pacing`
on the Mac that runs the probe, and run jobs one at a time. Pass the
`target/perf/examples/` executable path; the runners refuse binaries from any
other profile.

```sh
python3 scripts/native-pacing/run.py <out> <pacing_overlay> \
  --scrawl <path to a reference app executable>
```

The runner checks it is on the expected GUI host, holds a GUI lock file, bounds
app lifetimes, and removes its instrumentation dylib. `--modes native` selects only
the continuous Rune workload; the default tests injected redraws and native
animation. The reference app is optional and its order alternates between batches.

Rune uses a 680×160-point overlay. Native mode animates opacity continuously;
injected mode uses a static view with an AppKit 60-Hz redraw timer. The reference
app runs on a different-sized surface.
Results include executable hashes, raw callback logs, drawable waits, interval
medians, acquisition gaps, and counts of excluded zero presentation timestamps.
These excluded callbacks are not a dropped-frame count. A timeout is a failed
run and cannot serve as a complete performance baseline.

The `pacing` example separately exercises state commits, idle, hide/show and
shutdown. Run it under the same GUI lock with `--native --trace <file>`, then
check the file with `python3 scripts/check-native-pacing.py <file>`.

## Isolated refresh-rate experiments

Use a fresh output directory for each experiment:

```sh
# Same production executable; one change at a time, in this order.
python3 scripts/native-pacing/run.py <out-ca> <pacing_overlay> --modes native --display-link ca
python3 scripts/native-pacing/run.py <out-two> <pacing_overlay> --modes native --drawables 2
python3 scripts/native-pacing/run.py <out-unsynced> <pacing_overlay> --modes native --display-sync 0
```

`clock-probe.m` interposes CV callback registration/start/stop. Its default mode
counts real CV callbacks. CA mode supplies the same Rune callback from an NSView
`CADisplayLink` with `CAFrameRateRangeMake(80, 120, 120)` on macOS 14+. This adapter
selects the fixture's visible panel and supports this single-surface workload;
it is not a general replacement for Rune's native display-link implementation.
Layer overrides affect Rune only. An optional reference app keeps its settings.

For the admission-guard comparison, apply `admission-probe.patch` in a disposable
checkout, generate a new source manifest, and rebuild the example. Run `--guard production` then `--guard no-half` on that same executable.
The latter removes only elapsed-half-refresh admission, retaining stale-target
and duplicate-target rejection. The runner requires matching diagnostic output.
Counters are printed after stopping the link, not in the callback. Revert the
patch and rebuild when finished. Never ship this patch; it exists to reproduce
the experiment and intentionally leaves the original unit-test helper unused.

`acquisition_hz` uses `nextDrawable` entry times. `presentation_hz` uses distinct,
positive `presentedTime` timestamps for frames acquired in the nine-second sample.
Callback Hz and guard counts cover the full app run, including warmup. Guard
reason counters can overlap; they are not an exhaustive partition of callbacks
because pending callbacks coalesce. The summary includes the selected overrides
and binary hash. Raw logs retain the actual layer settings.

With display sync disabled, positive presentation timestamps do not establish
tear-free 120-Hz scanout. Apple's API explicitly allows tearing in this mode.

## Findings

With the default synced layer, the tested overlay presents at about 60 Hz on a
120-Hz display, although CVDisplayLink and NSView CADisplayLink both deliver
about 120 callbacks/s. Two drawables lower latency but keep 60-Hz presentation.
Disabling display sync raises positive presentations above 100/s, which does not
prove tear-free output. Removing the half-refresh admission guard moves waiting
into `nextDrawable` without raising presentation frequency, and breaks the
half-refresh bound: keep the guard. The evidence points to synced drawable
backpressure, not callback rate.

`refresh-probe.m` then found the cause of the 60-Hz cap: Low Power Mode holds a
ProMotion panel at 60 Hz while both display links and `NSScreen` still report
120 Hz. `crates/rune/src/display_link.rs` paces submissions at that cap.
