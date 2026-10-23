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
