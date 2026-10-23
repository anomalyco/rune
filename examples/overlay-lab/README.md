# Overlay lab

`cargo run -p overlay-lab` opens a transparent pill on the display under the pointer.
Only its chips intercept clicks; the background and hover label pass through. iPhone/Paste
change its contents inside a fixed maximum canvas. Area/Window/Screen open one picker per
display. Drag selects an area; Escape closes all pickers on platforms with overlay keys.

Headless stills use the same views, selected as the ordinary `lab` host:

```
cargo run -p overlay-lab -- --scene pill --snap pill.png
cargo run -p overlay-lab -- --scene expanded --snap expanded.png
cargo run -p overlay-lab -- --scene picker --size 960x600 --snap picker.png
```

Set `OVERLAY_LAB_SECONDS=15` for a bounded desktop probe with panel-ID/capture-request
logging. `OVERLAY_LAB_PICKER=1` opens the picker immediately. Run desktop probes in Pro's
logged-in GUI session, at normal priority. Do not launch/capture Studio's desktop.
`OVERLAY_LAB_UNDERLAY=1` runs a separate normal-window process that logs clicks, for
cross-process pass-through/focus checks. `OVERLAY_LAB_INCLUDE_IN_CAPTURE=1` disables the
capture request while grounding input coordinates; omit it for the exclusion check.

Linux capture exclusion is unavailable. Wayland only observes pointer movement inside its
input region, and the compositor chooses an output for UnderPointer. It cannot follow the
global pointer. OnDemand permits click-to-focus on layer-shell v4; Windows/X11 do not offer
nonactivating key focus. Capture requests on macOS must be supplemented with an explicit
ScreenCaptureKit window-ID filter when the OS capture path ignores NSWindow sharingType.
Wayland keeps all three levels on the compositor's overlay layer; it has no higher layer and
does not guarantee ordering among peers. Native status/status+1 levels are macOS-specific.

## Scrawl integration

- The lab prewarms hidden hosts and logs `on_created` with the actual macOS CGWindowID.
  `window_number()` is available after that callback and stays stable through hide/show.
  Host creation happens on the event loop, not synchronously in `App::overlay`.
- Use `set_input(OverlayInput::All)` for selection and `set_input(OverlayInput::None)` for
  pass-through ink/glow on the same host. Mode changes wait for an owned drag's release.
  `Regions` uses `.hit()` geometry; `Legacy` restores the spec's `interactive` behavior.
- `set_follow_pointer(bool)` enables/disables following independently of placement/input.
  Scrawl can keep it disabled and supply its own `Rect` and `set_display` requests.
- `Display::native_id()` maps to `DisplayTarget::Native`: macOS CGDirectDisplayID, Windows
  full-width HMONITOR, X11 RandR CRTC, Wayland wl_output registry name. X11 output IDs can
  also be targeted with `NativeDisplayId::X11Output`. `native_bounds()` is global top-left
  (macOS/Wayland points, Windows/X11 pixels); `scale` converts local lengths.
- The pill uses `AboveOverlays` (macOS 27), above the picker's `AboveStatus` (26). Both use
  CanJoinAllSpaces, FullScreenAuxiliary and Stationary. Wayland ordering remains compositor-owned.
- Desktop hide retains the native window, GPU surface and renderer. With `prewarm`, macOS
  keeps the panel ordered in at alpha zero with input disabled and a frame rendered into its
  actual surface. An unchanged show only restores input and alpha; hidden changes render
  before reveal. Hidden hosts have no pointer observers or draw loop. Wayland recreates its
  layer after hide.
- macOS Regions mode samples native pointer position/buttons at up to 120 Hz while visible.
  Monitors wake the loop; native local downs establish ownership through delivery of Up.
  None/All and hidden modes stop the polling deadline. Unchanged hover does not draw.
- Release measurements on a shared Pro: 30 retained re-shows reached the alpha setter in
  **0.369 ms median / 0.776 ms p95**. This is **not a presentation timestamp**. Alpha-only
  reveal reuses a drawable; Metal does not timestamp its compositor reveal. Opt-in diagnostic
  redraws reached positive `presentedTime` at **49.728 ms median / 75.907 ms p95**.
  **Sub-16 ms visible presentation remains unproven.**
- Ten-second idle intervals measured **0.044% CPU / 104 interrupt wakes/s** for a visible
  Regions pill with the pointer elsewhere and about **0.002% CPU** hidden. Neither interval
  produced drawable callbacks. All/None stopped polling and also produced no drawable callbacks.

The idle path filters outside-canvas motion and skips unchanged hover redraws. Windows hooks
run on a separate message-loop thread; X11 observes XI2 raw motion instead of polling. The
shared Wayland socket wakes its private queue; legacy noninteractive HUDs bind no seat and
reuse their empty input region.

## Native measurement harness (macOS)

Ordinary windows can opt into the same native switches without changing exhaustive
`WindowOptions` literals:

```rust
WindowSpec::from_app("capsule", app).options(
    WindowOptions { transparent: true, borderless: true, ..Default::default() }
        .macos(MacWindowOptions::default()
            .shadow(false).level(27)
            .collection_behavior(1 | 16 | 64 | 256)
            .exclude_from_capture(true))
)
```

The mask above is CanJoinAllSpaces | Stationary | IgnoresCycle | FullScreenAuxiliary.
These settings apply at each native creation; other platforms ignore them. They do not
change an ordinary window into a nonactivating panel. Runtime `set_always_on_top` overrides
the configured native level, including after recreation. `OVERLAY_LAB_UNDERLAY=1` plus
`OVERLAY_LAB_NATIVE_WINDOW=1` exercises native options; `OVERLAY_PROBE_NATIVE_OPTIONS=1`
logs native readbacks when the diagnostic library is loaded.

`native/` contains diagnostic tools, loaded only into the lab process. Build through
HiveMind on a worker, then copy the products into a fresh temporary directory on the
authorized GUI host:

```sh
cargo build --release -p overlay-lab -j 2
xcrun clang -dynamiclib -fobjc-arc -framework AppKit -framework QuartzCore -framework Metal \
  examples/overlay-lab/native/presentation-probe.m -o presentation-probe.dylib
xcrun swiftc -parse-as-library examples/overlay-lab/native/capture-probe.swift -o capture-probe
```

Put `overlay-lab`, `presentation-probe.dylib` and `capture-probe` in that directory. Hold
the host's GUI lock (`/tmp/pro-gui.lock`, `fcntl.LOCK_EX` on Pro), then run
`python3 examples/overlay-lab/native/run-native-probe.py <directory>` at normal priority.
Never run it on Studio's screen.

`OVERLAY_LAB_CONTROL` names a unique Unix socket and starts the pill prewarmed but hidden.
Newline-terminated `show`, `hide`, `status`, `none`, `all`, `regions`, and `quit` commands are accepted. The driver
records the request's `CACurrentMediaTime` immediately before `handle.show()` and the
drawable's actual `presentedTime`, not queue submission or callback arrival time. Zero
timestamps are reported separately, with a two-second deadline for a positive timestamp.
It checks WindowServer identity before 31 shows (initial plus 30 re-shows). Set
`OVERLAY_PROBE_RETAINED=1` for alpha-zero hosts; the old order-out baseline omits it.
`OVERLAY_PROBE_REDRAW=1` adds diagnostic redraws until a positive timestamp. Report this
endpoint separately from the alpha-only reveal; it includes work absent in normal showing.
`results.json` and `presentation.log` are the evidence; the lab has a bounded shutdown timer.

`run-idle-probe.py` samples process CPU and interrupt wake counters over ten-second intervals,
without redraw assistance, and counts drawable callbacks. `--modes` measures All/None/Regions.
`run-scrawl-probe.py <installed-binary> <directory>` runs 30 `lab pill tap-then-screen`
cycles using the same diagnostic library without redraw assistance. It measures native
already-visible `orderFrontRegardless` entry to positive presentation, not the beginning of
the coordinator request. Pro's installed Metal pill measured 81.833 ms median / 101.002 ms
p95; its optimization profile is unknown, so this is not a controlled release-to-release comparison.
`run-input-probe.py` drives an independent underlay and twenty raw-motion enter/click/leave/click
cycles. Build `mouse-path.swift`, copy it to a stable path and ad-hoc sign once before the
user grants Accessibility. Do not replace the granted binary. The runner defaults to
`~/tmp/rune-overlay-probe/mouse-path`; `OVERLAY_MOUSE_HELPER` overrides it. Set
`RUNE_OVERLAY_TRACE_INPUT=1` to log regions, monitor callbacks, polls, and native input flips.

The capture helper uses ScreenCaptureKit with and without the exact CGWindowID in
`excludingWindows`. It checks permission first and exits 77 if unavailable. Capture exclusion
is **pending Scrawl integration**: Scrawl.app will verify its CGWindowID exclusion filter
using its existing Screen Recording permission. The probe does not change TCC.

For click-through checks, use an independent underlay process. A coordinate-only synthetic
click does not establish that native mouse-move observation ran first. Check actual pointer
movement and both directions across the region boundary, and verify underlay/overlay logs.
