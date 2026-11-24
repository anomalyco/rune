# Rune architecture

This document maps how Rune works in code today. It focuses on stable crate boundaries, frame flow, state ownership, hosts, and rendering pipelines, and intentionally omits volatile counts (field, file, thread, or test totals).

For long-term direction and the planned two-thread app/render-thread architecture, see [`PLAN.md`](PLAN.md) (§3). Where today's implementation differs from that plan, this document describes what is actually built.

## Workspace crates and layering

All workspace members live in the root [`Cargo.toml`](Cargo.toml). Apps depend on `rune` alone; `crates/rune` re-exports the public APIs of the core stack beneath it.

### Zero-Rune-dependency base crates

- `crates/rune-motion`: analytic springs, tweens, decay, presence transitions, and `Scene`.
- `crates/rune-staging`: reused per-frame GPU upload staging chunks and GPU health polling.
- `crates/rune-taffy`: compact retained layout store (`CompactStyle`, `LayoutStore`) driving Taffy's Flexbox, Grid, and Block solvers.
- `crates/procwatch`: process footprint, per-thread CPU, counting allocator, byte gauges, and loopback JSON (depends on `libc` only).
- `crates/rune-highlights`: build-time-translated syntax highlighters (80 languages).
- `crates/rune-i18n`: compile-time-checked message catalogs, CLDR plurals/numbers/lists, and system locale discovery (excluded from default workspace members).
- `crates/rune-notify`: OS notifications (macOS `UNUserNotificationCenter`, Windows toast notifications; excluded from default workspace members).
- `crates/rune-register`: self-registration macros built on `linkme`.

### Core stack beneath `rune`

```text
rune-motion   rune-staging   rune-taffy
    │              │              │
    ▼              │              │
rune-core ─────────┼──► rune-gpu (also depends on rune-motion, rune-staging)
  │   │   │        │       │
  │   │   ▼        │       │
  │   │ rune-image │       │
  │   │     │      │       │
  │   ▼     ▼      │       │
  │  rune-text ◄───┘ (also depends on rune-motion)
  │     │
  ├──► rune-edit (depends on rune-core, rune-text)
  ├──► rune-spell (depends on rune-core)
  └──► rune-media (depends on rune-core, rune-gpu, rune-image, rune-staging)
                   │
                   ▼
             crates/rune (depends on all core crates above, plus optional procwatch)
```

- `crates/rune-core`: foundation primitives with no GPU or windowing dependencies—colours, design tokens, themes, vector shapes, state machines (`machine`), clocks, off-UI tasks (`task`), tracked reads, growth curves, live values (`Live`), latest-wins requests (`Latest`), launch marks, app directories, and preferences.
- `crates/rune-gpu`: GPU adapter selection, backend dispatch, device error logging, analytic GPU springs (`springs.rs`), and Rune's native Apple Metal backend (`metal-backend`).
- `crates/rune-image`: content-addressed image decoding, linear-light mip generation, shelf pixel atlas, optional SVG/AVIF codecs, and OS app icon rasterization.
- `crates/rune-spell`: platform spellchecking (Windows `ISpellChecker`, macOS `NSSpellChecker`, web Hunspell).
- `crates/rune-text`: text shaping and layout (Parley), font discovery, glyph rasterization (including CoreGraphics-matched font smoothing, DirectWrite on Windows, and Windfoil outlines), and icon rendering.
- `crates/rune-edit`: single-line and rich text editing (`TextEdit`), undo history, IME composition, key events, and HTML import/export.
- `crates/rune-media`: video and audio playback, zero-copy native surfaces (macOS `IOSurface`, Windows D3D shared textures, Linux dmabufs), and audio recording.
- `crates/rune`: element tree (`el`), retained runtime and commit/paint pipeline (`runtime`), GPU renderer and shaders (`render`, `shaders`), native window/overlay/web/embedded hosts (`window`, `overlay`, `platform`), accessibility (`a11y`), and headless agent tooling (`tooling`).

### Crates above and beside `rune`

- Above `rune`: `crates/rune-kit` (themable components, optional `markdown`, `mermaid`, `math`, and `syntax` features), `crates/rune-mermaid` (native Mermaid flowcharts, sequence, and state diagrams), `crates/rune-knobs` (live tuning knobs and panel), `crates/rune-luau` (sandboxed Luau UI scripting via `mlua`; requires a host C++ compiler and is excluded from default workspace members), `themes/opencode` (`rune-theme-opencode`), and `crates/rune-ref` (scalar CPU reference renderer for differential testing; optional `gpu` feature, excluded from default workspace members).
- Beside `rune`: `crates/rune-fx` (special-effects passes; depends on `rune-motion`, `rune-staging`), `crates/rune-light` (real-time SDF lighting and glass; depends on `rune-staging`), `crates/rune-mac` (macOS window-frame helpers; depends on `rune-notify`), and `crates/rune-perf` (bounded local performance recording; depends on `procwatch`).
- Vendored patched crates under `vendor/`: `wgpu-hal`, `naga`, `winit`, `sctk-adwaita`, `parley_engine`, `accesskit_windows`, `accesskit_macos`, and `accesskit_atspi_common` are patched via `[patch.crates-io]` in the root `Cargo.toml`; `tiny-skia-path` is a direct path dependency of `crates/rune-text`.

## Single-thread frame model and background work

Unlike the two-thread target in [`PLAN.md`](PLAN.md) §3, Rune has no separate render thread today. Input dispatch, `poll`, view functions, Taffy layout, animation sampling, and instance painting all run synchronously on the caller's thread (the platform UI/main thread in windowed apps, or the calling thread in headless runs and tests).

Views still never run just to animate. A view returns target styles and structure on commits; between commits, retained tracks, scroll physics, growth stacks, live bindings, and reveals advance without calling the view function.

Slow or blocking work stays off the UI thread through dedicated mechanisms:

- **Shared task pool and service threads** (`crates/rune-core/src/task.rs`): `rune::task::spawn` runs work on a shared worker pool on native targets (and on JS Promise Integration fibers on `wasm32`); `rune::task::serve` runs long-lived services on dedicated threads.
- **Latest-wins requests** (`crates/rune-core/src/latest.rs`): `rune::Latest` runs debounced queries (such as search) on a bounded shared worker pool, drops stale results, and wakes the UI thread through a `Waker` so `poll` delivers only the latest answer.
- **Image decoding** (`crates/rune-image/src/lib.rs`): image decode jobs run on `rune_core::task::spawn` rather than on the UI thread.
- **Media pipelines** (`crates/rune-media/src/`): video/audio decoding, audio output, and native-surface completion run on background threads.
- **GPU prewarming** (`crates/rune/src/render/prewarm.rs`, `crates/rune/src/render/warm.rs`): `prewarm_gpu` creates the GPU device and compiles pipelines on a background thread while the OS window loop starts up.

## One frame: OS event to present

In a native app, `crates/rune/src/window/event_loop.rs` owns the `winit` event loop and routes OS events, timer wakes, and `Waker` notifications into each host. Each drawn frame runs through `RuntimeCore::frame_split` and `RuntimeCore::frame_step` (`crates/rune/src/runtime/frame.rs`):

1. **Clock and catch-up**: `frame_step` updates the logical/wall clock and steps any already-running motion forward by `catch_up` (`catch_up_motion`) so handlers and commits retarget from the pose at the frame's timestamp rather than dumping idle time onto new animations.
2. **Wakes, timers, `poll`, and queued input**: due wake deadlines, hover delays, press repeat, edge scroll, autoscroll, touch holds, and spellcheck ticks fire; then the app's `poll` callback runs, followed by queued input and pointer/keyboard/IME/drop actions (`run_action`). If a routed key, IME, or file-drop event arrives after state changed, an intermediate commit runs first so focus and hit targets reflect the new tree.
3. **Commit (`Measurement` boundary into `apply`)**: if app state, layout reads, viewport size, or virtual-list windows changed, `RuntimeCore::measure` (`crates/rune/src/runtime/commit/measure.rs`) constructs a `Measurement` capability struct that runs the view, flattens `El` nodes into the retained node array (`commit/flatten.rs`), shapes and measures text (`commit/text_layout.rs`), runs Taffy layout at the target state, and finishes into a `Measured` value. `RuntimeCore::apply` (`crates/rune/src/runtime/commit/apply.rs`) accepts only `Measured` (never raw `El` trees) and reconciles `rune_motion::Scene`, growth stacks, streaming reveals, scroll viewports, live bindings, and exiting nodes.
4. **Motion and paint (`step_motion` → `build`)**: `RuntimeCore::step_motion` (`crates/rune/src/runtime/frame/motion.rs`) advances `rune_motion::Scene`, growth stacks (`step_growth`), scroll momentum (`step_scroll`), reveals, and text flights, then runs `self.build()` (`crates/rune/src/runtime/paint/build/`) to emit the flat `Vec<Instance>` stream and hit-test lists. Every `Instance` is a 128-byte `#[repr(C)]` `Pod` record in device pixels (`crates/rune/src/render/instance.rs`).
5. **GPU render and present**: `crates/rune/src/render/` uploads dirty glyph-atlas pages, image-atlas regions, and instance/uniform buffers via `rune-staging`, encodes any isolated layers, backdrop blurs, native video surfaces, or custom `Compositor` passes, submits the command buffer, and presents to the swap chain (or reads back pixels in headless mode).

## State ownership

### Type-erased `RuntimeCore` inside `Runtime<S>`

`Runtime<S>` (`crates/rune/src/runtime.rs`) is generic over the app's state type `S`, but almost all runtime logic compiles once as non-generic methods on `RuntimeCore`. Both structs are `#[repr(C)]` and generated from the same `runtime_core_fields!` macro, ending their shared prefix at a zero-sized 16-byte-aligned `CoreEnd` marker. Compile-time `offset_of!` assertions prove that every prefix field sits at the same offset in both structs and that the `S`-dependent tail (`state`, `poll`, `on_key`, `hooks`) begins at or after `size_of::<RuntimeCore>()`. `Runtime::split` and `Runtime::split_mut` project disjoint borrows of `RuntimeCore` and the `S`-dependent tail.

### Capability-owned subsystem structs

Inside `RuntimeCore`, runtime state lives in capability-owned subsystem structs rather than flat fields:

- `tree` (`TreeState`): retained node array, id/key lookup tables, and per-commit capability flags.
- `hit` (`HitState`): per-frame hit-test lists built in paint order.
- `paint` (`PaintState`): paint scratch buffers, reuse cache, isolated layer caches, text quads, and native material draw lists.
- `motion` (`MotionState`): growth stacks, streaming word reveals, text flights, animation loops, and rewind capture.
- `scroll` (`ScrollTables`): scroll viewports, wheel axis locking, scroll requests, and sticky headers.
- `frame` (`FrameState`): action queue, pending `Tx`, wake deadlines, live-value latch, tracing, and hot-reload state.
- `commit` (`CommitState`): Taffy layout engine, memo cache, virtual list windows, and layout/visibility callback tracking.
- `pointer` (`PointerState`): pointer position, capture, hover/tooltip delays, touch, autoscroll, and file/OS drags.
- `kbd` (`KeyboardState`): keyboard focus, modal focus/scope, type-select, held keys, and modifier tracking.
- `edit` (`TextInputState`): selectable text, links, caret, IME, spellcheck, and clipboard state.
- `host` (`HostState`): window/host settings, zoom, titlebar chrome, window requests, popups, dialogs, AccessKit adapter state, and web page metadata.

Alongside these subsystems, `RuntimeCore` directly holds `scene` (`rune_motion::Scene`), `text` (`TextSystem`), `images` (`ImageAtlas`), `instances`, `marks`, `surfaces`, and frame clock/environment scalars.

### Explicit state machines

Modal and timed behaviours use `rune::machine` (`crates/rune-core/src/machine.rs`): one `enum State` carrying only the data valid in each phase, and one pure `step(state, event) -> Effects<Effect>` function whose caller executes the returned effects. Timed states and async replies carry an `Attempt` fencing token issued by `Seq` so stale timers or out-of-order responses are ignored. Examples across `crates/rune/src/` include focus (`runtime/input/focus_machine.rs`), hover delays (`runtime/input/hover_delay.rs`), press repeat (`runtime/input/press_repeat.rs`), scroll modes (`runtime/scroll/mode.rs`), selection phases (`runtime/selection/phase.rs`), overlay presence (`overlay/presence.rs`), shader variant selection (`render/variant.rs`), and surface recovery (`window/surface/backoff.rs`, `platform/web_math.rs`).

## Hosts

Every host drives a `Runtime<S>` (or multiple runtimes) through the same commit/paint pipeline:

- **Native windows** (`crates/rune/src/window/`, `crates/rune/src/platform/`): `winit` windows with platform shells for IME (`platform/ime/`), AccessKit accessibility (`crates/rune/src/a11y/`, enabled by default via the `accessibility` feature), native menus (`native_menu/`, `popup_menu/`), system tray (`tray/`), global shortcuts (`shortcuts/`), clipboard/drag-and-drop (`clipboard/`, `drag_out/`), and macOS system backdrop materials (`system_material/`, backed by `NSGlassEffectView` on macOS 26+ and `NSVisualEffectView` earlier, synchronized with Core Animation transactions).
- **Overlays and panels** (`crates/rune/src/overlay.rs`, `crates/rune/src/overlay/`): transparent always-on-top surfaces with their own state and `Runtime`, updated from any thread via `OverlayHandle`. Backed by non-activating `NSPanel` on macOS, `wlr-layer-shell` on Wayland (with an XWayland fallback where layer-shell is absent), override-redirect windows on X11, and `WS_EX_NOACTIVATE` topmost tool windows on Windows.
- **Embedded Apple layers** (`crates/rune/src/platform/embed.rs`, `embed_frame.rs`): `LayerHost<S>` attaches a `Runtime` directly to a caller-supplied `CAMetalLayer` (used for iOS and embedded Apple views).
- **Custom `Compositor`** (`crates/rune/src/compose.rs`): apps attach a `Compositor<S>` via `App::compositor` or `rune::run_with` to take over frame encoding (for example, drawing `rune-fx` or `rune-light` passes and custom `El::effect` slots) while keeping Rune's layout, animation, input, windows, and headless tooling.
- **Web canvas** (`crates/rune/src/platform/web.rs`, `crates/rune/src/runtime/web/`): `wasm32` host drawing to an HTML `<canvas>` via WebGPU (or WebGL2 with the `webgl` feature), with a DOM accessibility mirror (`web/a11y`), a sliding-window hidden `<textarea>` IME bridge (`web/runtime.rs`), and static `--html` first-frame export (`web/export`).
- **Headless tooling** (`crates/rune/src/tooling.rs`, `crates/rune/src/tooling/`): drives the same `Runtime` and `Renderer` offscreen on a fixed virtual clock for `--snap`, `--tree`, `--script`, `--trace`, `--strip`, `--video`, `--html`, and `--desktop`. Always compiled in debug builds; opt-in via the `tooling` feature in release builds.

## Renderer, backends, and shaders

### GPU backends

`crates/rune-gpu` dispatches GPU work across platforms:

- **wgpu**: D3D12 on Windows (with WARP software fallback), Vulkan on Linux, Metal on Apple, and WebGPU (or optional WebGL2) on `wasm32`.
- **Native Apple Metal backend** (`metal-backend` feature, default on in `rune` and `rune-gpu`): plugs Rune's native `objc2-metal` implementation into wgpu's `custom` backend on Apple targets. macOS apps use it by default (opt out with `App::gpu_backend(GpuBackend::Wgpu)` or `RUNE_GPU_BACKEND=wgpu`); iOS apps default to wgpu unless they choose `GpuBackend::Metal`.
- **Analytic GPU springs** (`crates/rune-gpu/src/springs.rs`): opt-in retained translation/opacity tracks evaluated on the GPU while layout, hit testing, and presence stay CPU-owned.

### Shaders and uber-quad variants

Most UI elements draw through one instanced uber-quad pipeline consuming 128-byte `Instance` records (`crates/rune/src/render/instance.rs`): rounded rects, borders, analytic Gaussian outer/inset shadows, vertical/angled/radial gradients, dashed borders, vector segments and triangles, RGBA images, glyph-atlas or outline text, and layer composites.

`crates/rune/src/render/variant.rs` manages uber-shader selection as an explicit state machine (`Uber`, plus `Lcd`):

- **`Classic`**: `crates/rune/src/shaders/shader.wgsl` alone (glyphs sampled from the R8 glyph atlas). Ships precompiled bytecode (`uber.metallib`, `uber_native.metallib` on macOS; DXBC on Windows).
- **`Windfoil`**: `shader.wgsl` joined with `crates/rune/src/shaders/outline.wgsl` (plus binding 4 outline texture), evaluating glyph coverage directly from banded quadratic outlines. Ships precompiled bytecode (`uber_windfoil.metallib`, `uber_windfoil_native.metallib`, DXBC) where supported.
- **`Lcd`**: a dual-source-blending subpixel variant of `Windfoil` (`dual_wgsl()`), built on demand when subpixel text (`FontSmoothing::WindfoilRgb` or `ClearType`) is drawn on a supporting device.

Companion shaders in `crates/rune/src/shaders/`:

- `clip.wgsl`: shared rectangular, rounded, Apple continuous-corner, and soft-edge clip math prepended into the uber shader and compositor shaders.
- `backdrop_blur.wgsl` and `backdrop_composite.wgsl`: dual-filter downsample/upsample pyramid and rounded/clipped composite for `El::backdrop_blur`.
- `layer_blur.wgsl`: blur pass for isolated layers.
- `surface_alpha.wgsl`: premultiplied-to-straight alpha conversion.
