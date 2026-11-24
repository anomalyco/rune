# Rune examples

Every package under `examples/`, grouped by what it teaches. All headless commands below run from the repository root.

In debug builds, apps built with `rune::run`, `App::run`, or `rune::run_windows` automatically include Rune's headless CLI (`--snap`, `--tree`, `--list-scenes`, `--list-windows`, `--script`, `--trace`, `--strip`, `--video`, `--html`). Release builds include it only when Rune's `tooling` feature is enabled.

## Basics and state

| Example | What it teaches | Main APIs | Platforms | Repo-root headless command |
| --- | --- | --- | --- | --- |
| [`hello`](hello) | Minimal single-state app with a spring-animated switch and accessible role/label | `rune::run`, `AppSpec`, `El::motion`, `El::on_press`, `tok` | Desktop, headless | `cargo run -p hello -- --snap target/hello.png --scene on` |
| [`todo`](todo) | Keyed list with add/toggle/remove, enter/exit presence animations, and a spring progress bar | `El::key`, `ScrollSpec`, `Spring`, `tok::Theme` | Desktop, headless | `cargo run -p todo -- --snap target/todo.png` |
| [`settings`](settings) | Full settings window: dark/light theme switch, accent picker, sliders, toggles, time scale, and macOS app-menu `MenuItem::about` / custom `⌘Q` confirmation | `AppSpec::scenes`, `MenuItem::about`, `El::on_drag`, `Phase`, `Spring`, `tok::Theme` | Desktop, headless (`hot`, `frame-profile` optional) | `cargo run -p settings -- --snap target/settings.png --scene dark` |
| [`playground`](playground) | Built-in story browser (`⌘⇧D`) with nested groups, variants, side-column controls, and an overlay story | `App::playground`, `Playground`, `Story`, `Story::overlay`, `Stage` | Desktop, headless | `cargo run -p playground -- --snap target/playground.png --playground` |
| [`knobs`](knobs) | Live-tuning a UI card with `rune-knobs` (`⌘⇧K` panel and playground side column) | `rune_knobs::{Group, Knob, Knobs, Preset}`, `App::playground` | Desktop, headless | `cargo run -p knobs -- --snap target/knobs.png --scene open` |

## Motion and live updates

| Example | What it teaches | Main APIs | Platforms | Repo-root headless command |
| --- | --- | --- | --- | --- |
| [`live-values`](live-values) | Updating opacity and offset at 60–120 Hz from background threads without rebuilding the view (`--bench` compares against `--rebuild`) | `Live<f32>`, `Live<[f32; 2]>`, `El::live_opacity`, `El::live_offset`, `Live::smooth` | Desktop, headless | `cargo run -p live-values -- --snap target/live-values.png --at 2` |
| [`matched`](matched) | Shared-element geometry flight between a 2×2 grid and a full-stage view, plus a `--scene mips` streaming/mipmapped `Pixels` comparison | `El::matched`, `Transition`, `Pixels::{replace, with_rgba}`, `ImageCache::rgba_into`, `SurfaceFit` | Desktop, headless | `cargo run -p matched -- --snap target/matched.png --scene one` (`--scene mips` for streaming vs mipmapped `Pixels`) |
| [`odometer`](odometer) | Rolling numeric digits and right-aligned usage headers using `rune-kit`'s `RollingText` | `rune_kit::{RollingText, Button, Kit}`, `El::width_leads`, `El::ride_trailing` | Desktop, headless | `cargo run -p odometer -- --snap target/odometer.png` |
| [`frosted-bar`](frosted-bar) | Long list scrolling behind an in-window blurred bottom bar | `El::backdrop_blur`, `El::scroll`, `AppSpec::scenes` | Desktop, headless | `cargo run -p frosted-bar -- --snap target/frosted-bar.png --theme dark` |

## Desktop, input, windows, and overlays

| Example | What it teaches | Main APIs | Platforms | Repo-root headless command |
| --- | --- | --- | --- | --- |
| [`drag-out`](drag-out) | Dragging a file, app bundle, or text out of a Rune window into external apps | `DragItem`, `DragOutcome`, `El::on_drag_out`, `El::on_drag_start` | Desktop, headless | `cargo run -p drag-out -- --tree` |
| [`drop-files`](drop-files) | Accepting file drops on targeted drop zones and the whole window | `El::on_drop`, `El::on_drop_hover` | Desktop, headless | `cargo run -p drop-files -- --script examples/drop-files/scripts/drop.json --frames 90 --every 6 --cols 5 --strip target/drop-files.png` |
| [`rich-clipboard`](rich-clipboard) | Reading and writing plain text, sanitised HTML, and RTF on the OS clipboard, and inspecting the clipboard change counter | `ClipboardContent`, `TextEdit`, `text_input`, `Tx::copy`, `clipboard::revision` | Desktop, headless | `cargo run -p rich-clipboard -- --tree` |
| [`sketch`](sketch) | Freehand drawing canvas with a custom 2× PNG cursor and hot spot | `Cursor::Image`, `CursorImage::png`, `Shape`, `canvas`, `El::on_drag` | Desktop, headless | `cargo run -p sketch -- --snap target/sketch.png` |
| [`background`](background) | Windowless/background app lifetime controlled over stdin, plus HUD and clickable corner overlays | `App::windowless`, `App::overlay`, `AppHandle`, `OverlayHandle` | Desktop, headless (macOS `--accessory` option) | `cargo run -p background -- --snap target/background.png --scene ticked` |
| [`launcher`](launcher) | Multi-window notes app plus a `⌘Space` launcher panel, composable under `--desktop` | `rune::run_windows`, `WindowSpec`, `WindowOptions::launcher_panel`, `App::global_shortcut` | Desktop, headless | `cargo run -p launcher -- --window notes --snap target/launcher.png` |
| [`overlay`](overlay) | Control window plus one transparent, click-through overlay per display sharing a `Live` cursor | `rune::run_windows`, `WindowSpec`, `WindowOptions::overlay`, `WindowHandle`, `Live` | Desktop, headless (macOS `window_number`) | `cargo run -p overlay -- --window overlay-0 --snap target/overlay.png --at 2` |
| [`glass-card`](glass-card) | Spring-animated card over a live test pattern using macOS system backdrop material (`NSGlassEffectView` / `NSVisualEffectView`) | `rune::run_windows`, `El::system_material`, `SystemMaterial`, `MacWindowOptions` | macOS for live system material (fallback tint elsewhere) | No headless CLI mode — see [Notes on `glass-card` and `glass-panel`](#notes-on-glass-card-and-glass-panel) |
| [`glass-panel`](glass-panel) | Floating non-activating macOS overlay panels (`flight` handing off to `dock`) with continuous corners and system material | `rune::run_windows_with`, `RunOptions`, `App::windowless`, `App::overlay`, `SystemMaterial` | macOS for live system material and panel levels | No headless CLI mode — see [Notes on `glass-card` and `glass-panel`](#notes-on-glass-card-and-glass-panel) |

## Graphics, media, and compositing

| Example | What it teaches | Main APIs | Platforms | Repo-root headless command |
| --- | --- | --- | --- | --- |
| [`shapes`](shapes) | Vector strokes, filled polygons, Bézier paths, waveforms, and memoized canvases | `Shape`, `Path`, `LineCap`, `canvas`, `memo` | Desktop, headless | `cargo run -p shapes -- --snap target/shapes.png --at 1.5` |
| [`video-player`](video-player) | Playing video or audio files and URLs (defaults to an embedded test clip) with seek, mute, loop, and output-device controls | `Player`, `PlayerState`, `SurfaceFit`, `Waker` | Desktop, headless | `cargo run -p video-player -- --snap target/video-player.png --at 0.6` |
| [`video-tiles`](video-tiles) | Four live video surfaces fed from a background producer, plus a headless `--bench-native` mode | `NativeSurface`, `ByteFrame`, `SurfaceFit`, `native_surface::macos::PixelBuffer` | Desktop, headless (NV12 `IOSurface` on macOS; byte planes elsewhere) | `cargo run -p video-tiles -- --snap target/video-tiles.png --scale 2` |
| [`cursed-settings`](cursed-settings) | Settings window where controls trigger custom `rune-fx` and `rune-light` effects via a compositor | `rune::run_with`, `Compositor`, `rune-kit`, `rune-theme-opencode`, `rune-fx`, `rune-light` | Desktop, headless | `cargo run -p cursed-settings -- --snap target/cursed-settings.png --scene fire` |
| [`showreel`](showreel) | Clock-driven showcase scenes combining Rune UI and `rune-fx` for frame-exact `--video` captures | `App::compositor`, `AppSpec::scenes`, `rune-fx` | Desktop, headless | `cargo run -p showreel -- --scene deploy --snap target/showreel.png --at 1.0` |

## Components, scripting, and web

| Example | What it teaches | Main APIs | Platforms | Repo-root headless command |
| --- | --- | --- | --- | --- |
| [`kit-gallery`](kit-gallery) | Every `rune-kit` component story (OpenCode theme, `--kit neutral`, or `--kit palette`) | `rune-kit` (`markdown`, `mermaid`), `rune-theme-opencode`, `AppSpec::scenes` | Desktop, headless | `cargo run -p kit-gallery -- --scene ui-button--variants --snap target/kit-gallery.png --theme dark --size 720x300` |
| [`luau-panel`](luau-panel) | Sandboxed Luau UI script with `task.wait`, signals, 1,000-row `ui.virtualList`, and live file watch; includes a second `luau-bench` binary | `rune_luau::{Script, Sources, ScriptOptions}`, `Script::watch` | Desktop, headless, web (`scripts/web-build.sh luau-panel` with `WASI_SDK_PATH`); native builds need a host C++ compiler | `cargo run -p luau-panel --bin luau-panel -- --snap target/luau-panel.png` (benchmark binary: `cargo run -p luau-panel --bin luau-bench --profile perf`) |
| [`web-hello`](web-hello) | Responsive chat layout with safe-area insets, text input, clipboard copy, and wasm32 browser entry point | `Cx::safe_area`, `Cx::system_dark`, `text_input`, `ScrollSpec`, `rune::web::open_url` | Desktop, headless, web (`scripts/web-build.sh web-hello`) | `cargo run -p web-hello -- --snap target/web-hello.png` |
| [`web-showcase`](web-showcase) | Interactive switch, spring picker, and throwable puck built to mount on an existing webpage canvas | `AppSpec`, `Spring`, `El::on_drag`, `wasm_bindgen` | Desktop, headless, web (`scripts/web-build.sh web-showcase`) | `cargo run -p web-showcase -- --snap target/web-showcase.png --size 560x320 --scale 2` |
| [`fetch`](fetch) | Cross-target JSON fetch: `spawn_local` + `web_sys` `fetch` + `AppHandle::update_if` on web, `Latest` on native | `App::handle`, `App::init`, `App::on_ready`, `AppHandle::update_if`, `Latest` | Desktop, headless, web (`scripts/web-build.sh fetch`) | `cargo run -p fetch -- --snap target/fetch.png` |

## Labs and benchmarks

These packages are diagnostic labs, benchmarks, or site asset renderers rather than minimal app templates.

| Example | Kind | What it exercises | Main APIs | Platforms | Repo-root headless command |
| --- | --- | --- | --- | --- | --- |
| [`lab`](lab) | Lab | Interactive lighting, glass, and effects sandbox with live knobs for `rune-light` and `rune-fx` | `App::compositor`, `rune-light`, `rune-fx`, `rune_motion::{Animated, VelocityTracker}` | Desktop, headless | `cargo run -p lab -- --scene lamp --snap target/lab.png` |
| [`launch-floor`](launch-floor) | Benchmark | Measures minimum first-frame launch cost (`bare`, `shell`, `reads`, `LAUNCH_FLOOR=warm` cache prewarm, and `LAUNCH_FLOOR_MEMORY_POLICY`) | `App::on_ready`, `App::layout_seed`, `App::gpu_memory_policy`, `GpuMemoryPolicy`, `rune::set_gpu_memory_policy`, `rune::marks`, `rune::warm_gpu_cache` | Desktop, headless (debug builds include tooling; `LAUNCH_FLOOR=warm` opens no window) | `cargo run -p launch-floor -- --tree` |
| [`motion-bento`](motion-bento) | Benchmark / lab | 12-tile control center and `--load`/`--ramp` stress wall driven by a seeded chaos schedule; shares its Rune-free model and schedule crate [`motion-bento/chaos`](motion-bento/chaos) (`motion-bento-chaos`) | `Live`, `memo`, `rune-kit`, `motion_bento_chaos` | Desktop, headless | `cargo run -p motion-bento -- --snap target/motion-bento.png --at 1.0` |
| [`overlay-lab`](overlay-lab) | Lab | Multi-display overlay pill, full-screen area picker, regional hit testing, capture exclusion, plus extra native probe binaries `layer-filter-overlay` and `occluded-timer` | `App::overlay`, `OverlayOptions`, `OverlayInput`, `OverlayLevel`, `KeyPolicy` | Desktop, headless (capture exclusion and native status levels are macOS-specific) | `cargo run -p overlay-lab -- --scene pill --snap target/overlay-lab.png` |
| [`site-scenes`](site-scenes) | Asset renderer | Renders the home-page material stills (`metal`, `glass`, `light`) plus an opt-in `runtime` scene directly through `Gpu::headless`, `Offscreen`, and `Renderer::render_runtime_to_offscreen` | `Gpu::headless`, `Offscreen`, `Renderer::render_runtime_to_offscreen`, `write_png`, `rune-fx`, `rune-light` | Headless only (custom CLI, no `rune::run`) | `cargo run -p site-scenes -- --only runtime --out target/site-scenes` |

## Notes on `glass-card` and `glass-panel`

- **`glass-card`** (`examples/glass-card/src/main.rs`) builds two windows through `rune::run_windows` and passes explicit `App::args(vec![])` (for `pattern`) and `App::args(trace)` (for `card`). In `crates/rune/src/tooling.rs`, `WindowSpec::resolve_args` lets each app's explicit `App::args` replace the inherited process CLI flags, so headless flags are overridden and the example opens its native windows instead. Live `SystemMaterial` also requires a native macOS window; headless renders substitute a flat tint.
- **`glass-panel`** (`examples/glass-panel/src/main.rs`) runs a 1×1 `windowless()` host app and drives three non-activating macOS overlay panels (`backdrop`, `flight`, `dock`) from a background thread loop. Headless CLI targets only the 1×1 dummy host window and neither runs the overlay panels nor composites macOS system materials.
