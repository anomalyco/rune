# General overlays

Proposal, 22 October 2026. One `AnyHost` per overlay, sharing Rune's event loop and GPU.
The pill uses a **fixed maximum transparent canvas**; springs move its contents and hit
regions. This avoids native resize/surface reconfiguration on every animation tick.
The picker uses one full-display host per display; Scrawl shares state between them.

## API and compatibility

Keep `OverlaySpec { name, size, edge, margin, align, inset, interactive, view }` intact.
Adding fields breaks existing exhaustive Rust literals even with `Default`; Hex uses those.
`OverlaySpec::options(OverlayOptions)` returns a configured spec accepted by `App::overlay`
through `Into<ConfiguredOverlay<T>>`. Bare specs convert with legacy defaults. Add a
`OverlaySpec::new(name, size, view)` constructor for new callers.

```rust
let pill = app.overlay(spec.options(OverlayOptions {
    display: DisplayTarget::UnderPointer,
    follow_pointer: true,
    input: OverlayInput::Regions,
    exclude_from_capture: true,
    ..Default::default()
}), state);
// Chip geometry, including live/spring transforms and clipping, drives native input.
el().hit().on_press(/* ... */)
```

Options: `placement: Option<OverlayPlacement>` (`None` uses legacy edge fields),
`OverlayPlacement::{Edge { edge, margin, align, inset }, Rect { x, y, width, height },
FullScreen}`, `display: DisplayTarget::{Primary, UnderPointer, Id(DisplayId)}`,
`follow_pointer`, `level: OverlayLevel::{Status, AboveStatus}`, `exclude_from_capture`,
`keys: KeyPolicy::{None, OnDemand}`, `input: OverlayInput::{Legacy, Regions}`.
Rect coordinates are **logical points from the selected display's top-left**; native
display bounds are physical desktop pixels. IDs are opaque, session-scoped native output
identities, exposed by `Display`, never names or enumeration indices. A disconnected ID
falls back to primary; UnderPointer resolves on show, then on movement only if following.
FullScreen means full bounds, not the work area. Legacy Windows edge placement retains
its taskbar inset. `OverlayHandle` gets placement/display/key-focus requests and the native
window number for capture filtering. `OnDemand` permits explicit `request_keys()`;
showing a pill never requests keyboard focus.

`El::hit()` marks a rectangular input region without adding a handler. In Regions mode
unmarked pixels pass through; Legacy preserves `interactive` exactly. `on_hover` and a
new `on_pointer_move` use overlay-local points even over passing-through content, without
turning observational elements into click targets. A press retains pointer ownership
until release, so a drag leaving a chip cannot strand the gesture. Hidden hosts have no
monitors, polling deadlines, or input regions.

## Layout, input and threads

Runtime emits marked regions from the **same final animated geometry** used for drawing,
after parent transforms, scroll and ancestor clip intersection, excluding exits and fully
transparent nodes. Native hit tests read an immutable published snapshot, never run the
view or acquire application-state locks. Publish after layout and before presenting that
frame; never use last frame's regions as next frame's layout input. Re-evaluate stationary
pointer hit state whenever regions move. OS presentation and native region changes are
not universally atomic: this prevents an extra Rune lag frame, not compositor-level
atomicity. Wayland regions and buffers share a surface commit. Conservative rectangular
regions are explicit; curved/path-perfect hit testing is a later API.

All layout, native windows, monitor installation/removal and region publication run on the
event thread (AppKit main thread). Handles only enqueue requests and wake the existing
loop. Native callbacks record pointer/key data and wake the host; they must not re-enter
Runtime. Global observation never consumes another app's events. Hold no mutex while
calling AppKit/Win32 or invoking client callbacks. Drop monitors/subclasses before their
views, and GPU surfaces before their native backing.

## Native implementation

| Backend | Placement, input, keys and capture |
| --- | --- |
| macOS | Extend Dax's real NSPanel owner. Resolve NSScreen by native display ID; convert top-left points to AppKit bottom-left coordinates. Levels 25/26. Reuse launcher-spike's initialized key-capable nonactivating subclass (`canBecomeKeyWindow`, never main), retaining winit view/delegate routing. `sharingType = None` requests capture exclusion; also expose panel CGWindowID for Scrawl's explicit ScreenCaptureKit exclusion filter. A view `hitTest:` returning nil alone does **not** pass events to other processes: use local/global mouse observation to update `ignoresMouseEvents` against the published regions, retaining interception during drags. Key focus is requested explicitly, never app activation. |
| Windows | Monitor bounds/DPI plus topmost `WS_EX_NOACTIVATE` tool window. `SetWindowDisplayAffinity(WDA_EXCLUDEFROMCAPTURE)` with reported failure. `WM_NCHITTEST` reads the snapshot, but `HTTRANSPARENT` alone forwards only within the same thread: cross-process pass-through also needs layered `WS_EX_TRANSPARENT` toggling and global pointer observation. Windows cannot promise macOS-style nonactivating keyboard focus; report unsupported rather than silently activate. Levels provide relative Rune overlay ordering, not status-bar semantics. |
| X11 | Override-redirect windows on selected RandR outputs; input SHAPE rectangles in window-local device pixels. Observe root pointer position (bounded timer while visible if no global motion source), retain button grabs through release. No portable capture exclusion or nonactivating keyboard focus. Report unsupported capabilities. |
| Wayland | Layer-shell surface bound to each selected `wl_output`; recreate on output migration. Anchors/margins encode placement; full-screen respects compositor configure. Commit `set_input_region` with drawn buffer; add private-queue seat/pointer/keyboard dispatch and event-loop wake integration. `OnDemand` keyboard interactivity where protocol version permits. No unrestricted global pointer coordinates/hover through an empty region, global pointer-follow, or portable capture exclusion: expose these limits, never claim success. |

## Proof and risks

Headless tests cover negative display origins, mixed DPI, disconnect fallback, rect/edge
placement, moving/transformed/clipped hit regions and release outside a region. Overlay lab
offers pill/picker views as ordinary headless `--snap` scenes too. Compile macOS, cargo-xwin
clippy, and Linux through HiveMind, one job at a time. Check actual cross-app clicks, hover,
drag retention, focus and ScreenCaptureKit/screencapture on Pro or mini only.

Main risks: global-monitor ordering near a click; moved winit-view geometry/delegate/IME
assumptions (launcher research documents these); mixed-DPI output migration; private Wayland
queue starvation; protocol-dependent keyboard focus. Modern macOS capture paths may ignore
sharingType, so Scrawl's exclusion filter remains necessary and must be tested. Native
exclusion is a request with platform capability/error reporting, not a security guarantee.
