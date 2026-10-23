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
Wayland keeps both levels on the compositor's overlay layer; it has no higher layer and
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
- Desktop hide retains the native window, GPU surface and renderer. Hidden hosts have no
  pointer observers or draw loop; warm startup renders offscreen once. Wayland recreates its
  layer after hide. **Under 16 ms to first presented frame on Pro remains unmeasured.**

The idle path filters outside-canvas motion and skips unchanged hover redraws. Windows hooks
run on a separate message-loop thread; X11 observes XI2 raw motion instead of polling. The
shared Wayland socket wakes its private queue; legacy noninteractive HUDs bind no seat and
reuse their empty input region.
