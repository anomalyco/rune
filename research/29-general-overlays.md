# General overlays

Design, 22 October 2026, including Scrawl's review. One `AnyHost` per overlay, sharing the event loop and GPU.
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
    follow_pointer: false, // Scrawl supplies a display-local Rect.
    input: OverlayInput::Regions,
    exclude_from_capture: true,
    prewarm: true,
    ..Default::default()
}).on_created(|host| { /* add host.window_number to the capture filter */ }), state);
// Chip geometry, including live/spring transforms and clipping, drives native input.
el().hit().on_press(/* ... */)
```

Options: `placement: Option<OverlayPlacement>` (`None` uses legacy edge fields),
`OverlayPlacement::{Edge { edge, margin, align, inset }, Rect { x, y, width, height },
FullScreen}`, `display: DisplayTarget::{Primary, UnderPointer, Id(DisplayId), Native(NativeDisplayId)}`,
`follow_pointer`, `level: OverlayLevel::{Status, AboveStatus, AboveOverlays}`, `exclude_from_capture`,
`keys: KeyPolicy::{None, OnDemand}`, `input: OverlayInput::{Legacy, None, Regions, All}`, `prewarm`.
Rect coordinates are **logical points from the selected display's top-left**. Keep native
desktop origins in their original coordinate space. IDs are opaque, session-scoped output
identities. `native_id()` and `DisplayTarget::Native` map CGDirectDisplayID, full-width
HMONITOR, RandR CRTC/output and wl_output registry names. `Display::native_bounds()` returns
global top-left native bounds (points on macOS/Wayland, pixels on Windows/X11); `scale`
converts local lengths. A disconnected ID
falls back to primary; UnderPointer resolves on show, then on movement only if following.
FullScreen means full bounds, not the work area. Legacy Windows edge placement retains
its taskbar inset. `OverlayHandle` gets placement/display/key-focus requests and the native
window number for capture filtering. `OnDemand` permits explicit `request_keys()`;
showing a pill never requests keyboard focus. `set_follow_pointer` is runtime-settable.
`set_input(None/Regions/All)` reuses a picker as a pass-through ink/glow host; a change waits
for its owned drag to release. `AboveOverlays` puts the pill above an `AboveStatus` picker.

`El::hit()` marks a rectangular input region without adding a handler. In Regions mode
unmarked pixels pass through; Legacy preserves `interactive` exactly. `on_hover` and a
new `on_pointer_move` use overlay-local points even over passing-through content, without
turning observational elements into click targets. A press retains pointer ownership
until release, so a drag leaving a chip cannot strand the gesture. Hidden hosts have no
monitors or polling deadlines. Desktop hide retains the native window and GPU surface;
Wayland destroys its layer. `prewarm` creates hidden hosts and warms layout/glyphs/pipelines,
then calls `on_created` before first show. On macOS it renders the actual surface and leaves
the panel ordered in at alpha zero with input disabled. Show restores alpha/input; hidden
content changes render before reveal. macOS CGWindowID stays stable through hide/show.
Release measurements on Pro preserved the ID over 31 shows. Thirty re-shows reached the alpha
setter in 0.593 ms median / 2.964 ms p95. This is not visible-presentation latency: Metal
does not timestamp compositor revelation of retained pixels. Diagnostic redraws reached
positive presentation in 49.154 ms median / 76.666 ms p95. Sub-16 ms presentation is unproven.
No native window exists before the loop runs.

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

Layout, native windows and region publication run on the event thread (AppKit main thread).
Windows hook/X11 reader threads own observation. Handles enqueue requests and wake the existing
loop. Native callbacks record pointer/key data and wake the host; they must not re-enter
Runtime. Global observation reports movement, not other apps' clicks. Outside-canvas
movement is filtered by the Windows/X11 observers; unchanged hover does not redraw.
macOS visible Regions mode samples native position/buttons on a 120 Hz deadline; monitors
are extra wake sources. Local downs establish ownership, held through delivery of Up.
The deadline stops in hidden/None/All modes. A ten-second visible idle sample measured
0.044% CPU, 104 interrupt wakes/s and zero drawable callbacks; hidden cost was about 0.002%.
Hold no application mutex while calling native APIs or client callbacks. Drop monitors/subclasses before their
views, and GPU surfaces before their native backing.

## Native implementation

| Backend | Placement, input, keys and capture |
| --- | --- |
| macOS | Real NSPanel with winit view/delegate routing; levels 25/26/27 and CanJoinAllSpaces, FullScreenAuxiliary, Stationary. Native display IDs and top-row-inclusive pointer containment; gaps preserve the current screen. `sharingType=None` plus stable CGWindowID. A view `hitTest:` alone cannot pass events across processes: observation changes `ignoresMouseEvents` only at hit boundaries. Key focus is explicit, never app activation. |
| Windows | Topmost `WS_EX_NOACTIVATE` window, display affinity, `WM_NCHITTEST` and layered `WS_EX_TRANSPARENT`. The shared observation hook has its own message-loop thread: store positions and wake; change styles on the event thread. Nonactivating key focus is unsupported. Levels order Rune overlays. |
| X11 | Override-redirect windows, SHAPE input and XI2 raw-motion observation on a private reader connection; no idle polling. Implicit button grabs survive region changes. Capture exclusion and nonactivating key focus are unsupported. |
| Wayland | Bind explicit outputs; Primary/UnderPointer leave the choice to the compositor. Bind devices only as needed; cache regions and commit changes with buffers. winit socket readiness drives the private queue. OnDemand keys require v4. No global hover/follow or capture exclusion; compositor orders peers on the Overlay layer. |

## Proof and risks

Headless tests cover negative display origins, mixed DPI, disconnect fallback, rect/edge
placement, moving/transformed/clipped hit regions and release outside a region. Overlay lab
offers pill/picker views as ordinary headless `--snap` scenes too. Compile macOS, cargo-xwin
clippy, and Linux through HiveMind, one job at a time. Check actual cross-app clicks, hover,
drag retention, focus and ScreenCaptureKit/screencapture on Pro or mini only.

Main risks: clicks between polling samples (raw-motion boundary proof is permission-blocked);
moved winit-view geometry/delegate/IME
assumptions (launcher research documents these); mixed-DPI output migration; private Wayland
queue starvation; protocol-dependent keyboard focus. Modern macOS capture paths may ignore
sharingType, so Scrawl's exclusion filter remains necessary and must be tested. Native
exclusion is a request with platform capability/error reporting, not a security guarantee.
