# Rune: plan

*Research notes behind every claim are in [`research/`](research/). Numbers marked "target" are commitments to
measure against, not results.*

## The short version

Rune is a Rust UI framework with its own renderer, text, layout, state,
animation and platform shells, built to feel like SwiftUI-on-iOS at 120 Hz and to be written and checked by agents.

**Direction:** Rune apps carry **their own design language on every
platform, Mac included**, rather than imitating each OS. Rune itself bakes in no brand: it ships a token-driven theming
system, and OpenCode's theme is the first theme built on it and the reference example. The flagship test is
**recreating OpenCode Desktop** in Rune and raising its bar, which makes Windows and Linux
first-class from the start. Native stays where it's behaviour, not looks: text input/IME, accessibility, window
management, clipboard, scroll momentum. A Rust component library with the same names as `@opencode/ui`, gallery
and screenshot tests, and Hex Settings as the first real screen fold in here instead of becoming a second stack on
Slint.

| Layer | Pick | Instead of |
|---|---|---|
| Language | Rust (2024), thin `objc2`/Swift shims on Apple | Zig, C++, Swift, Mojo |
| GPU | wgpu on every platform first (one backend ships Mac, Windows, Linux and web); a raw Metal backend later only where a measured need shows up (live-resize sync, IOSurface video), via wgpu-hal escape hatches first | Raw Metal first, Vulkan/D3D12 by hand |
| Renderer | One instanced "uber-quad" SDF pass + glyph atlas + Slug for big/animated text and icons + `vello_gpu` sparse strips for complex vectors; soft-edge masks in-shader on every clip | Full compute Vello, Skia, MSAA path textures |
| Look | Unbranded core with a token-driven theming system (semantic tokens, themes with light/dark, runtime switching, codegen); the same look on every platform. OpenCode's theme (from `@opencode/ui`) is the first theme and the main example, kept outside the core | A theme baked into the framework; per-OS native look; Liquid Glass as the base (it's an optional Apple extra) |
| Text | Parley + HarfRust + skrifa + fontique, GPUI-style two-tier line cache, kitlangton/gpui glyph dilation | CoreText-only, cosmic-text |
| Text editing / IME | Zed's `sum_tree` + rope; each platform's own text protocol, implemented directly (`NSTextInputClient`, `UITextInput`, …) | winit's IME, a hidden native text field |
| Layout | Taffy (flex, grid, block) on Rune's own node arena, plus a sum-tree height index for huge lists | Own layout engine, SwiftUI-style proposals |
| State | Keyed retained tree rebuilt from plain view functions; typed models; reads subscribe; handlers queued with `&mut` | Signals, GPUI entities, immediate mode |
| Animation | Render thread owns the presented tree; every property is an analytic spring track (duration + bounce, SwiftUI's formula) that retargets with velocity | Animating through view re-renders on the main thread |
| Accessibility | AccessKit, and the same tree is the agent inspector's tree | Own a11y bridges |
| Shells | winit to start (the prototype runs on it); own thin shells per platform (borrowing from Zed's Apache-2.0 `gpui_macos`/`gpui_windows`/`gpui_linux`/`gpui_ios`/`gpui_web`) as soon as text input lands, because winit's IME is incomplete | winit forever, SDL3 |
| Hot reload | Tokens, knobs and live literals with no rebuild (≤ 100 ms), subsecond hot-patching for structural Rust edits on Mac (~1 s measured, goal 300 ms), **split-process dev mode** for the iPhone (renderer on the phone, app code on the Mac) | A view DSL interpreter, code loading on the device |
| Platform order | macOS + Windows + Linux together (one wgpu backend) → web (docs site, demos) → iOS → Android | |

**Top three risks:** text input and text quality (IME on three desktops, then iOS `UITextInput`), matching
OpenCode Desktop's Chromium-rendered look closely enough (text above all), and sheer scope.

**First milestones:** Rune's theming system plus an OpenCode theme generated from `@opencode/ui` as its first
consumer, and a component gallery styled by it; Hex's Settings as the first real screen on Mac, Windows and Linux; then OpenCode Desktop's main screens.
A docs/marketing site comes first to explain it.

---

## 1. Why build this at all

Kit has rebuilt the same small surfaces three times in ten days:
the Scrawl pill went GPUI → hand-written Metal → SwiftUI helper process, the launcher app went GPUI → SwiftUI with a
Core Animation glass morph, and Hex's native branch stalled because Hex also ships on Windows and Linux. Each time,
the reasons were the same:

1. **No backdrop or glass.** GPUI draws one forward pass into one layer; Liquid Glass only exists in the window server.
2. **Animation waited behind the main thread.** The Metal pill dropped 85 frames on finish until it got its own
   display-link thread.
3. **Windows and focus were wrong for HUDs and launchers** (late focus events, Dock policy, no non-activating panels,
   per-frame window resizes).
4. **Text looked thin and colours were off,** and unseen glyphs stalled the first frame by ~13 ms.
5. **No springs or layout animation,** so every app hand-rolled them.
6. **Upstream was frozen** at 0.2.2 for Kit's apps, so every fix lived in a fork.

Upstream GPUI has since moved a lot: Zed's tree at `5447804` has `gpui_ios`, `gpui_web`, a wgpu backend,
`spring.rs`, AccessKit and subsecond hot reload (verified in a clone). It still redraws the whole window every dirty
frame, has no layer compositing or group opacity, runs animations by re-rendering views on the main thread, and
panics on re-entrant entity borrows ([01](research/01-gpui-zed-warp.md)). Those are the exact things that sent Kit
to Apple frameworks, and they're architectural. **Verdict: don't fork GPUI; start fresh and lift its best parts**
(rope/sum_tree, BoundsTree, line layout cache, the macOS/iOS IME code, the web textarea bridge, the shader fixes in
Kit's fork) under its Apache-2.0 licence.

## 2. Tenets

1. **Feel is the product.** 120 Hz, interruptible springs, nothing pops, jumps or hard-clips. Kit's motion rules
   are runtime defaults, not discipline.
2. **Animation never runs through app code.** Views state targets; the render thread animates.
3. **Everything observable.** Every frame, node, layout rect, spring and timing can be queried, traced and replayed.
   The agent's view of the app is the same tree as accessibility's.
4. **One way to do things.** Small explicit APIs, no macro DSL for views, errors that say how to fix.
5. **Native behaviour, one look.** Text input, accessibility, scroll momentum, clipboard and window behaviour are
   native per platform. Controls, type, motion and materials come from the app's theme, the same on every platform,
   Mac included. The framework ships no brand of its own.
6. **Owned, not wrapped.** Depend on foundational crates (Taffy, Parley, AccessKit), never on another UI framework.
7. **Optimise for performance, correctness, speed of change and prototyping; not for human readability**.
   Agents write and maintain the code, so:
   - **Fast path plus reference path.** Each hard subsystem (layout, text, renderer, springs) has a slow, obviously
     correct reference implementation (CPU, no tricks) and a fast one (SoA, SIMD, `unsafe` where measured, specialised,
     duplicated). Every change differential-tests the fast one against the reference on generated inputs. Correctness
     comes from the reference and the oracles, not from the fast code being easy to read.
   - **Data-oriented everything:** flat arenas indexed by ids, struct-of-arrays, no `Rc`/`RefCell`, zero allocation
     per frame (enforced by a counting allocator in tests), precomputed tables, pipelines built ahead of time.
   - **Specialise by codegen, not by generics.** Generated, repetitive, per-component code is fine; deep trait towers
     and proc-macro DSLs are not, because they slow compiles and bury errors. Debug builds stay type-erased and fast
     to compile; release builds specialise.
   - **Edits are safe because checks are instant:** invariants as `debug_assert`s everywhere, scenes, goldens,
     motion/layout lint and exact perf counters run on every change, so an agent sees breakage in seconds.
   - **Comments state invariants and intent for the next agent,** not prose for a human reader.
   - **Perf is measured by counts** (instructions, instances, glyphs, layout nodes, allocations) so gates are exact
     and bisectable; wall-clock numbers come from reserved hardware.
   - **Prototypes are cheap and throwaway:** live literals, knobs, scenes and hot reload make trying ten variants
     faster than debating one.

## 3. Architecture

```
                 ┌──────────────────────── dev only: same protocol over USB/Wi-Fi ────────────────────────┐
                 │                                                                                         │
┌────────────────┴─────────────────────────────┐      commands: tree diff,        ┌──────────────────────┴─────────────────────────┐
│ APP SIDE  (platform main thread by default)  │      layout rects, targets,      │ RENDER THREAD  (display-link paced)            │
│                                              │      springs, text runs          │                                                │
│  models (typed state, keyed by stable ids)   │ ───────────────────────────────▶ │  presented tree (what's on screen right now)   │
│  view fns  →  keyed El tree  →  diff         │                                  │  property tracks: analytic springs, retarget   │
│  Taffy layout at the *target* state          │ ◀─────────────────────────────── │  presence (exiting nodes), layout edge tracks  │
│  Parley shaping, line cache                  │      events: hits, gestures,      │  scroll physics, drag bindings (1:1, local)    │
│  handlers: queued, get &mut model            │      settles, presented-frame ids│  scene build → instance buffers               │
│  AccessKit tree updates                      │                                  │  encode (Metal / wgpu) → present              │
└──────────────┬───────────────────────────────┘                                  └──────────────┬─────────────────────────────────┘
               │ platform calls                                                                    │ CATransaction: Metal drawable +
┌──────────────┴───────────────────────────────┐                                  ┌───────────────┴────────────────────────────────┐
│ SHELL (per platform)                         │                                  │ NATIVE LAYERS (Apple)                          │
│  windows/panels, menus, clipboard, DnD,      │                                  │  NSGlassEffectView / UIGlassEffect underlays,  │
│  IME protocol, a11y adapter, lifecycle       │                                  │  AVPlayerLayer, MapKit; masks driven per frame │
└──────────────────────────────────────────────┘                                  └────────────────────────────────────────────────┘
        background pool: image decode, big-paragraph shaping, vector flattening, file IO
        inspector socket (dev + test builds): tree, rects, a11y, source locations, springs, timings, traces, input injection
```

The one boundary that matters is the **command channel between the app side and the render thread**. It carries
tree diffs, target layout rects, shaped text runs and spring targets one way, and hits, gesture results, settles and
presented-frame ids the other. Three things fall out of making it a real, serializable protocol:

- **Animations survive a busy app.** When one value animates, the app side does no work at all: no view re-run, no
  layout, no diff ([06](research/06-reactivity-animation.md)).
- **The iPhone hot-reload path.** In dev, the channel can run over USB: the renderer runs on the phone and the app
  side runs on the Mac, where subsecond hot-patches it (§5).
- **Agents and tests get the same stream.** Recording the channel gives deterministic replay, frame traces and an
  inspector for free (§6).

## 4. Each layer

### 4.1 Language: Rust

Rust is the only option with production text, layout and accessibility crates (Taffy, Parley, HarfRust, fontations,
AccessKit), no GC, reach to every target including wasm, and a working hot-patcher (subsecond). Agents write it well;
the borrow checker is the main cost, which the state model below is designed around (no `RefCell` re-entry, no
borrows held across async). Zig lacks the ecosystem and a dependable incremental patch path on Mach-O; Swift is great
on Apple and weak everywhere else; C++ gives up safety and agent reliability for nothing; Mojo isn't a UI language
yet ([04 §1](research/04-language-gpu-renderer.md)). Swift is used only for thin host pieces Apple exposes no other
way (SwiftUI-only glass morphing, the `RuneView` representable).

### 4.2 GPU API: wgpu everywhere, raw Metal on Apple only where measured

wgpu runs on every platform, since the desktop trio ships together. Zed's own move from blade to wgpu in 2026 is
the evidence that it's good enough ([04 §2](research/04-language-gpu-renderer.md),
[01](research/01-gpui-zed-warp.md)). A raw Metal (`objc2-metal`) backend behind a thin internal trait is a later
optimization, added only where measurements show wgpu falls short: precompiled `.metallib` (no shader compile at
launch), `IOSurface` zero-copy video, framebuffer fetch for cheap group opacity on Apple silicon, and exact control
of `CAMetalLayer` presentation (`presentsWithTransaction`, drawable count). Rejected: hand-written Vulkan/D3D12 (Zed
just deleted theirs).

### 4.3 Renderer

A typical frame is one render pass. Six primitives, all instanced, all clipped by a GPU clip table that gives
rounded clips **and soft gradient edges per side for free**, so "no hard edges" costs nothing extra
([04 §3](research/04-language-gpu-renderer.md)):

| Primitive | Does |
|---|---|
| `UberQuad` | rounded/squircle rects, borders, analytic Gaussian shadows (inner/outer), Oklab gradients with dithering, surface material |
| `AtlasSprite` | body text from a greyscale glyph atlas, images, emoji |
| `Slug` | big, rotating or scaling text and icons straight from Bézier curves (patent dedicated to the public domain on 2026-03-17; reference shaders MIT/Apache) |
| `SparseStrips` | complex vector art via `vello_gpu` (Linebender moved compute Vello to `research/` in 2026) |
| `Backdrop` | in-app blur/saturate of what's already drawn (dual Kawase, scissored), with Scrawl's peak-alpha boost for legible blurred text |
| `NativeSurface` | zero-copy video and camera frames |

Plus: offscreen layers only when needed (group opacity with blur, cached complex subtrees), dirty-rect scissoring
for idle-ish frames, linear-light blending, Display P3, and dithering on every gradient and shadow (Kit's GPUI fork (`kitlangton/gpui`)
fixes, now partly upstream).

**Frame pacing and latency.** `CADisplayLink` with `preferredFrameRateRange` up to 120 while anything animates, zero
ticks at rest. Discrete input (key, click) draws immediately instead of waiting for the next tick, with at most two
drawables in flight. Animation time is the display link's *target* timestamp, so a late frame still lands on the
right pose. `CAMetalDisplayLink` is suspect: one 2026 measurement shows windowed macOS scheduling it 3–5 refreshes
ahead ([04 §4](research/04-language-gpu-renderer.md)); spike S3 checks this before we rely on either.

### 4.4 Compositing with native layers (Apple)

*Optional now that Rune carries OpenCode's look on every platform; kept for Kit's Mac surfaces (M4).*

Rune can't draw Liquid Glass: the shaders that sample behind the window live in the window server. So the Apple
shell composes **native underlays and overlays** with Rune's Metal layer: a system glass view sits behind a
transparent region of the Metal layer, and its shape is driven by a mask layer that the render thread updates in the
same `CATransaction` as the drawable (`presentsWithTransaction`). This is how Scrawl's Metal pill already hosts
`NSGlassEffectView`, and the launcher's CA glass recipe is the fallback material backend
([11 §2](research/11-platform-shells.md)). Popovers, HUDs and pills use a
fixed, transparent, max-size surface with shape hit-testing, never per-frame window resizes.

Two caveats: glass *morphing* between shapes is SwiftUI-only on iOS, and NSViews are main-thread objects. Spike S2
measures whether a glass underlay can track a Rune-animated shape at 120 Hz with the main thread blocked.

### 4.5 Text

- **Fonts and shaping:** HarfRust (pure-Rust HarfBuzz port), skrifa/fontations (hinting, variable fonts, COLRv1),
  fontique (system fonts and fallback), plus SF's tracking tables on Apple.
- **Layout:** Parley for bidi, line breaking, rich text and inline boxes, wrapped in GPUI's two-tier cache (shaped
  runs separate from wrap points), so resizing a container never re-shapes text
  ([05](research/05-text-ime-a11y-layout.md)). Overflow is a soft fade mask by default, never an ellipsis.
- **Rasterizing:** greyscale atlas, 4× horizontal subpixel positioning, and the kitlangton/gpui luminance glyph dilation
  that fixed "thin text" by matching macOS font smoothing. Text under a scale animation switches to Slug outlines so
  springs never re-rasterize glyphs every frame.
- **Quality bar:** a side-by-side diff against CoreText on the same strings (spike S5). If Rune text doesn't match
  native on a Retina Mac, nothing else matters.

### 4.6 Text editing and IME

Zed's `sum_tree` and rope for storage (O(log n) UTF-8↔UTF-16 offsets, large files). Each platform's real protocol,
implemented directly: `NSTextInputClient` (with `doCommandBySelector:` for Kit's key bindings), `UITextInput` plus
`UITextInteraction` on iOS (selection handles, loupe, autocorrect, dictation, marked text), `InputConnection` on
Android, TSF on Windows, `text-input-v3` on Wayland, and a sliding-window hidden `<textarea>` on the web (Zed's
`gpui_web` approach) with `EditContext` where available. This is the single riskiest area; Flutter's iOS text shim is
3,400 lines and still feels off ([03](research/03-flutter-apple-rive-figma.md)).

### 4.7 Layout

Taffy (flexbox, grid, block) running over Rune's own node arena through its traversal traits, so there's one tree,
not two. Agents already know flexbox. Huge lists use a sum tree of row heights (cached or estimated) and build views
only for visible rows plus overscan. Layout runs once, at the *target* state; layout animation is the render thread's
job (§4.9). Rejected: an own engine (no gain worth the cost), SwiftUI-style proposals (agents know it less well, and
it's harder to make incremental).

### 4.8 State

A keyed retained tree rebuilt from plain view functions ([06 §1](research/06-reactivity-animation.md)):

```rust
fn pill(cx: &mut Cx, rec: &Model<Recorder>) -> El {
    let r = cx.read(rec);                  // reading subscribes this view; no notify() to forget
    row().key("pill")
        .padding(tok::space::S2).gap(tok::space::S1)
        .shape(Shape::Capsule)
        .material(Material::Glass)
        .motion(tok::motion::MORPH)        // layout + shape changes animate on this spring
        .child(dot().key("dot").fill(tok::color::RECORD).present(r.recording))  // exits animate, then unmount
        .child(text(r.elapsed()).key("time").style(tok::type_::MONO_CAPTION))
        .on_press(rec, |r, _| r.stop())    // queued between frames, gets &mut Recorder
}
```

- App state lives in typed `Model<T>`s in a store, outside any view, keyed by stable ids. That's what lets state
  survive hot patches.
- Views return a type-erased `El` (fast compiles, small generics surface). Keys are explicit.
- Handlers never run inside render, so there's no double-borrow panic.
- High-frequency values (audio meters, scroll offset, drag position) never go through models; they bind straight to
  render properties.

Rejected: fine-grained signals (state lost on reload, reads outside tracking silently do nothing), GPUI's entities
(re-render on the main thread for animation, re-entrancy panics), immediate mode (rebuilds everything every frame;
fine for tools, wrong for feel).

### 4.9 Animation, a first-class primitive

- **Springs** are `Spring::new(duration, bounce)` with ω = 2π/duration and ζ = 1 − bounce, exactly SwiftUI's
  parameterisation (the launcher's convention differs by ~20%; Rune standardises). Default tokens come from
  Kit's motion presets.
- **Every property is a track** of analytic segments `{t0, x0, v0, target, spring}`: position and velocity are exact
  at any time, so retargeting samples `(x, v)` at the presentation time and starts a new segment. This is
  Psychopomp's model generalised.
- **Layout animation** animates each *edge* of a node whose frame changed, one spring per axis. A stationary edge
  gets no track so it can't jiggle; width can lead with a faster X spring; shapes, borders and shadows are drawn at
  the presented size, so there's no scale distortion. Both edges snap to 1/64 px.
- **Presence:** removing a keyed node moves it to *exiting*; it leaves flow, plays its exit and unmounts only once its
  opacity has settled at 0. A key that returns mid-exit revives from where it is.
- **Gestures:** drags apply 1:1 on the render thread (no animation on Kit's own hand); release hands velocity to a
  spring. Scroll physics are per platform (iOS: 0.998/ms deceleration, 0.55 rubber band; macOS: the OS's momentum
  phases).
- **Kit's rules as defaults:** nothing animates on first mount, direct manipulation is instant, reduced motion swaps
  travel for a short fade per property class, blur only on things entering or leaving in place, soft masks on every
  clip.
- **One clock** with time scale (slow-mo), pause and manual stepping, shared by tests, traces and the inspector. The
  render thread emits motion-trace JSONL natively, with exact "this was animated on purpose" flags.

## 5. Hot reload

Goal: edit to pixels in under 300 ms with state kept, on the Mac and on a connected iPhone. Today that's reachable
for tokens, knobs, literals and shaders; structural Rust edits measure about a second.

Code loading on a physical iPhone is a dead end for now. Subsecond explicitly excludes physical iOS devices; JIT and
loading code signed after install aren't available without a debugger; InjectionNext's device support has an
unresolved 2026 failure report; Kit's HotReloadKit is simulator- and Mac-only for the same reasons
([07](research/07-hot-reload.md)). So Rune layers it:

Views stay plain Rust; there is no separate view language.

| Layer | What changes | How | Target (edit → presented frame) |
|---|---|---|---|
| 0. Tokens and knobs | colours, spacing, type, springs, any `Tunable` | token file watched + knobs remote protocol over WebSocket; render thread re-resolves live, in-flight springs retarget | ≤ 50 ms Mac, ≤ 100 ms iPhone |
| 1. Live literals | any edit that only changes literals in view code (a number, colour, string, spring, enum variant) | Xcode-Previews-style: in dev builds a macro gives each literal in a view function a `file:line:col` id; the watcher re-parses the saved file and, if only literals changed, pushes the new values down the token channel. No compile. | ≤ 100 ms Mac and iPhone |
| 2. Shaders | Metal/WGSL | rebuild the one pipeline at runtime in dev | ≤ 150 ms |
| 3. Rust on the Mac | structural edits: view functions, handlers, layout, logic | subsecond hot-patching through `dx` (the same path upstream GPUI now uses); models live outside patched code, so state stays | **measured ~1 s** (989 ms median, S1); goal ≤ 300 ms |
| 4. Rust on the iPhone | the same edits | **split-process dev mode** (below) | layer 3 + one USB round trip *(spike S4)* |
| 5. Everything else | struct layout changes, new deps | full rebuild; models restored from a snapshot keyed by id, incompatible ones reset | ≤ 2 s Mac incremental (1.85 s measured for a tiny crate); device install as fast as Xcode allows |

**What spike S1 found** ([08](research/08-spike-hot-patch.md)). Subsecond 0.7.10 hot-patches a plain Rust binary
with no Dioxus UI, keeping main-owned state across ten edits: 989 ms median, 1,150 ms p95 from save to new output
(826 ms to patch applied; the CLI's own patch time was 687 ms median), on an M2 Max build worker at background
priority. A normal incremental rebuild was 1.85 s, and cold 71 s. It also found traps Rune's integration has to
close: a single-reference closure capture silently kept running old code (use `HotFn::current(f).call(args)` at
explicit boundaries), statics and thread locals reset on patch, cached `Box<dyn Trait>` objects keep their old
methods, and adding a struct field isn't detected. So the ≤ 300 ms target holds for layers 0–2, which cover
most tuning edits; structural Rust edits are about a second. Getting those under 300 ms is M1 work: measure at
foreground priority on the Mac, keep each app's views in a tiny leaf crate, use the fastest linker, and try the
cranelift backend for dev builds. Patch boundaries are Rune's view functions and handlers, not arbitrary code; a
type change in a model triggers the snapshot-and-restore path instead of a silent patch. Layer 3 ships as the
opt-in `hot` feature and `scripts/rune-dev` (see "Live Rust editing" in [`crates/rune/README.md`](crates/rune/README.md)).

**Split-process dev mode.** In dev builds the iPhone runs a generic Rune "player": shell, renderer, render thread,
animation, text rasterizing, scroll and drag physics. The app side (models, views, layout, shaping) runs on the Mac
as an ordinary process that subsecond can patch, and talks to the phone over the same command channel the two
threads already use (§3), across USB. Because animation, scrolling and drags run on the phone, feel is local; only
discrete state changes cross the cable (~1–3 ms each way over USB, to be measured). Platform services the app calls
(haptics, camera, keyboard, permissions) go through Rune's capability interfaces, which are injected anyway for
deterministic tests, and in dev mode are proxied to the phone. React Native's old remote JS debugging ran app code on
the desktop the same way; the difference here is that the protocol is the one Rune ships in production between
threads, not a debug-only bridge. Release builds are a single native binary with no player and no socket.

What it can't do: measure real on-device app-side performance (use a native build for that), or exercise iOS-only
code paths in the app side without the capability proxy. Both are acceptable for a dev loop.

State rules (from React Fast Refresh and Flutter): model state survives any patch whose type is unchanged; view-local
state keyed by explicit string keys survives; a changed struct layout resets only the affected models (or migrates
via a declared function); each reload is applied atomically and acknowledged by its first presented frame id, which
is what the latency numbers measure.

## 6. Agent tooling

Agents need to build UI, look at it, and prove it works without a person. Built in from the first commit
([09](research/09-agent-native-and-perf-targets.md)):

- **Scenes.** Every component registers named scenes (states with fixture data and fake services), like
  `scrawl lab pill tap-then-area`. Scenes render in the real app, offscreen in tests, and in a gallery.
- **Headless, deterministic rendering.** Offscreen Metal (and wgpu) with pinned fonts, fixed clock, seeded randomness
  and a recorded environment manifest. Two pixel lanes: exact hashes on a pinned worker, and per-platform baselines
  with explicit tolerances. Failures save expected/actual/diff plus the tree.
- **Inspector protocol** (versioned JSON over a socket, CLI and GUI are clients): query nodes by role/key/text,
  layout rects, styles, a11y roles, the source `file:line` of the view that made each node, live springs and their
  tokens, per-phase timings, and input injection with exact timestamps.
- **Time control:** pause, step, scale, seek. `settle` with a deadline that lists what's still pending, instead of
  "wait until idle".
- **Traces:** motion-trace JSONL from the render thread (no recorder needed), the internal layout linter's rules ported to the layout
  tree, Perfetto/Tracy spans per phase, presented-frame ids.
- **Tuning:** every `Tunable` publishes knobs `ControlDescriptor`s; the knobs panel drives Mac or iPhone; "copy as
  code" emits Rust constants.
- **APIs agents can't misuse:** no notify to forget, no borrows across `await`, explicit keys, errors with fixes
  ("`Text` has no key inside a dynamic list; add `.key(id)`"), `llms.txt`, and examples that are also tests.

The CLI, sketched:

```
mk run [--device iphone] [--split]      # app with hot reload; --split = split-process dev mode on the phone
mk scenes / mk snap <scene> [--at 0.25s] # list scenes; render one to PNG + tree JSON, deterministic
mk inspect <query>                       # e.g. mk inspect 'role=button text~"Stop"' → rects, source, springs
mk trace <scene> --script tap.json       # motion-trace JSONL + frame timings; mk lint runs motion/layout rules
mk tune                                  # open the knobs panel for the running app
mk bench <workload>                      # the perf workloads in §8, with machine manifest
```

## 6b. Guarantees and built-in motion debugging

Things few UI libraries promise, which owning the whole stack and a deterministic clock make possible. Each one is
checked automatically, not by eye.

**Motion you can prove is smooth (Kit's motion toolkit, built in).** Everything Kit's motion tooling does today
with external recorders becomes part of the runtime, because Rune owns every presented value on every frame:

- The motion linter's rules (pop-in/out, opacity/size/shape snaps, jumps, reversals, jitter, clip snaps, overlaps,
  flicker, hitches, fade cutoffs, stop-dead, velocity snaps) run live in debug builds, every frame, with the node, its
  spring token and the source `file:line` in the finding; in tests they are hard failures.
- Animated properties can only change through tracks, so a one-frame jump can't be written; a node can't unmount at
  opacity > 0; persistent items can't blur; stationary edges are two-edge snapped to 1/64 px.
- A motion-inspector overlay (an idea from a private iOS client): live presented rects, velocities, active springs and their
  tokens, intent flags, hitch markers, drawn over the app; toggled by a key or the inspector.
- Render-time slow motion and frame stepping (true time scale, not stretched video), plus `rune trace` → motion-trace
  JSONL, frame strips and slowed before/after clips produced from deterministic
  replays, so a fix is shown with evidence before Kit looks.
- Every hitch is charged to the named interval and node that caused it (HitchProbe-style), with the phase
  (view, layout, shaping, encode, GPU) that ran long.

**Other guarantees, in rough order of how soon:**

| Guarantee | How it's checked |
|---|---|
| **Instant first frame.** The first frame is rendered at build time (per appearance and common window size) and shown the moment the process starts; the live tree takes over invisibly once ready. Target: pixels in ≤ 50 ms from process start on a cold launch. | Launch timestamps in CI; a golden diff between the pre-rendered and the first live frame must be zero (no visible handoff). |
| **No dropped frames by budget.** Each scene's cost (instances, glyphs, layout nodes, shaping, allocations) is counted exactly; CI fails if a scene exceeds the 8.33 ms budget model for the reference device. | Exact counters, bisectable; wall-clock confirmation on reserved hardware. |
| **Zero cost at rest.** No ticks, no GPU submissions, no wake-ups once settled. | Counters over a 10 s idle sample. |
| **Real contrast.** Text contrast is checked against the actual composited pixels behind each glyph (after blur and glass), in every theme. | Per-scene accessibility pass. |
| **No clipped text, any locale or size.** Every scene is fuzzed across window sizes, text scale, long German, RTL, CJK, light/dark; fails on overflow, overlap, hard clips, tap targets < 44 pt, off-spine alignment (layout-linter rules). | Scene fuzzing in CI. |
| **Every pixel traces to code.** Any pixel → node → `file:line` → commit; screenshot diffs explain themselves. | Inspector query; PR visual diffs. |
| **Bug reports are replays.** Sessions record inputs and timing (KB, not video) and replay to identical pixels; Scrawl attaches one. | Replay determinism test on every change. |
| **Same pixels on every platform** for the canonical lane, from a CPU reference renderer that the GPU path is diffed against. | Differential renderer tests. |

## 6c. Research round 2: tricks to use

Six more notes ([12](research/12-research-rendering.md)–[17](research/17-research-perf-architecture.md)), aimed
at cutting-edge work that suits agent-maintained code. Web search was down for most of these agents, so many sources
were recalled rather than re-read; each note marks which. Cite-check before relying on a number.

**Adopt now**

- **Sample every animation at the display's target presentation time**, never "now". Frame-time *variance* hurts
  perceived smoothness more than average fps (12 ms variation is noticeable, 4 ms barely); with analytic springs a
  late frame is a time gap, not a velocity jump ([15](research/15-research-motion-perception.md)).
- **Smoothness metrics from the frame trace:** position error against the ideal analytic value, hitch-time ratio,
  cadence deviation, velocity continuity at retarget, plus a jitter fuzzer in CI ([15]).
- **Late-latched input on the render thread** with Android's resampling constants, the impulse velocity estimator
  (least-squares as reference), tuned on a recorded gesture corpus. Hand springs measured velocity, never predicted
  ([13](research/13-research-latency-startup.md), [15]).
- **Text under motion:** snap y, ¼-px x phases while moving, end the track at the analytic settle time to avoid
  shimmer; spatial springs may overshoot, opacity/colour springs never do ([15]).
- **CPU reference renderer:** `crates/rune-ref` evaluates Rune's own instance formulas on the CPU, the exact lane
  with hash goldens; GPU lanes get per-channel tolerances, because the WGSL spec allows FMA fusion and
  reassociation, so GPU bit-exactness can't be promised ([24](research/24-reference-renderer.md),
  [12](research/12-research-rendering.md), [14](research/14-research-determinism-verification.md)).
- **Differential fuzzing of fast vs reference** from one structured scene generator (Vello's
  `cpu_gpu_differential` fuzz target is the model), plus metamorphic and model-based tests from the same generator
  ([14]).
- **App-level record/replay** (inputs, clock, RNG, a state hash per frame, periodic keyframes for seeking); `rr`
  doesn't run on macOS ([14]). The command channel becomes a byte SPSC ring that doubles as the replay log
  ([17](research/17-research-perf-architecture.md)).
- **Exact CI gates:** instruction counts (gungraun) and allocation counts on Linux, shuttle for the app↔render
  thread handoff, Miri on every PR ([14]).
- **Pixel provenance:** the CPU renderer also writes a node-id buffer, so pixel diffs blame a node and its
  `file:line`; a semantic diff (tree, rects, springs) runs before the pixel diff ([14]).
- **Per-node dirty classes** (Unreal Slate's model) with early cutoff: one measured case went 34 ms → 0.35 ms for
  10k widgets ([17]). A **per-frame arena** built into `El`'s lifetime, with a counting-allocator test ([17]).
- **Blur and shadows:** Jimenez-style pyramid backdrop blur with a fractional radius (an animated radius never
  steps), capped radius with upscale; closed-form blurred rounded rects and shadows; Oklab with deterministic dither
  ([12]).
- **Startup:** precompiled shaders / Metal binary archive and parallel init so device, fonts and pipelines stay off
  the first-frame path; the first-frame layer of §6b with a bit-identical handoff gate; order-file/PGO layout trained
  on replayed command streams ([13], [17]).
- **Compile speed:** no debug info for dependencies (22–30% faster clean builds in one measured project); skip
  Cranelift on Apple for now (unwinding/ABI gaps) ([17]).
- **Agents:** one component catalog generated from the Rust definitions (schema, `llms.txt`, validator, prompt),
  props that accept only theme tokens, a single `rune check` (build → render scene → layout/motion lint → tree
  diff → fixes keyed to source), a compact text tree with stable ids derived from keys ("tap n17"), and a small
  Rune-Bench of 30–50 scenes measuring first-try correctness ([16](research/16-research-agent-generative-ui.md)).

**Spike**

- **Springs evaluated on the GPU.** Two notes reached this independently ([12], [17]): instances reference
  transform/clip/effect property nodes (Chromium's property trees) whose analytic spring tracks a shader evaluates,
  so an animating frame costs the CPU about one uniform write regardless of how many things move. Verified against
  the CPU evaluator. Spiked as opt-in translation/opacity tracks in `crates/rune/src/gpu_springs.rs`.
- Liquid-glass lens in our own shader (SDF height field, refraction, dispersion, smooth-union morphing) ([12]).
- Word-level FLIP when text reflows during a resize ([15]); Slug for scale-animated text ([12]).
- Runtime-generated UI: an A2UI-shaped protocol (flat ids, separate data model, patches) on the command channel, with
  validate-then-commit, budgets and capability grants ([16]).
- Perceptual diffs (NVIDIA FLIP) and Kani on core invariants ([14]).

**Effects: motion blur and friends.** Research [15] said motion blur isn't needed at 120 Hz,
but Kit wants it as an effect, and Rune can do it better than anyone because every track is analytic: true
**shutter motion blur** samples a node's exact position/scale/opacity at several instants inside the frame's shutter
interval (sample count scaled by on-screen speed, zero cost at rest) and accumulates them, instead of guessing from
velocity; a cheap directional blur along the analytic velocity is the fallback for big surfaces. Shutter angle is a
motion token and a knob, so Kit tunes it live (and can say where it belongs: flings, page/camera moves, flights like
the timeline's send flight). Kit's "persistent items never blur" rule is about Gaussian blur during reorders;
motion blur is a separate, opt-in effect that Kit judges by eye. Same family, all spikes: liquid-glass lensing, glow
that cools rather than cuts, feathered streaming-text reveal (from Desktop's timeline-motion branch), soft edge masks
everywhere.

**Skip:** GPU-driven indirect draws and partial present for now ([12]); synthetic motion blur at 120 Hz (maybe for
flings only) ([15]); process snapshot/restore for GPU state ([13]).

**Open:** whether wgpu exposes enough Metal present control and framebuffer fetch, or the Apple shell must own
`CAMetalLayer` and a raw Metal backend sooner ([12], [13]).

## 7. The design language

Rune ships a design system, not just primitives:

- **Theming is a framework feature; no theme is baked in.** Rune defines a semantic token schema (background
  layers, text, icon, border, accent, status colours; space, radius, type scale, elevation, materials, motion) and
  components read only semantic tokens. A theme supplies values for light and dark, can be switched at runtime (every
  colour retargets on a spring), can be derived from seed colours, and can be loaded from data (for hot reload and
  user themes) or generated as Rust constants. Colour in OKLCH/P3. A lint fails on raw style literals in app code.
  The core ships one plain neutral default so examples render.
- **OpenCode's theme is the first consumer and the main example,** in its own crate outside the core
  (`themes/opencode` or an example): generated from `@opencode/ui`'s theme (`packages/ui/src/theme`:
  `desktop-theme.schema.json`, 36 themes with seed colours; the `--v2-*` semantic tokens in `styles/tokens/`), the
  same source that feeds Tailwind on the web. The pipeline's output should be a Rune theme,
  not changes to Rune. Motion tokens default to Kit's motion presets.
- **Components are generic and themable.** The OpenCode example kit can mirror `@opencode/ui`'s names and variants
  (card, list, popover, context-menu, hover-card, text-field, collapsible, scroll-view, …) so screens port from the
  Solid app by name, and its screenshot tests diff each component against its web story.
- **Materials:** in-app blur and tinted surfaces everywhere; system Liquid Glass on Apple only as an optional extra
  via underlays.
- **Motion:** Kit's palette (instant, snappy, default, calm, smooth, morph, playful, slow) as named tokens with
  enter/exit pairs; transitions as primitives (morph one surface, width leads, soft exits).
- **Components** grown from dogfooding, not designed up front: capsule/pill, chips, list rows with selection, search
  field, menus and command palette, tray/popover morph, toast/HUD, sheet, segmented control, keycap. Each with scenes,
  a11y, reduced motion and knobs.

## 8. Performance targets

Reference devices get pinned before any number becomes a gate (a 120 Hz Apple-silicon Mac and a ProMotion iPhone,
never a developer's desk display). CI runs deterministic correctness on every change and performance on reserved hardware
([09 §C](research/09-agent-native-and-perf-targets.md)).

| Metric | Target |
|---|---|
| Mac hello app, fresh process → first presented frame | p50 ≤ 150 ms, p95 ≤ 200 ms live; ≤ 50 ms with the pre-rendered first frame (§6b) |
| iPhone hello, app entry → first present | p95 ≤ 250 ms |
| Event → presented frame (software, 120 Hz) | p50 ≤ 8.3 ms, p95 ≤ 16.7 ms; app-side dispatch → submit p95 ≤ 4 ms |
| Physical key → photon (Mac, photodiode rig) | p95 ≤ 35 ms, compared against native AppKit on the same rig |
| App-side work while one value animates | zero (no view, layout or diff) |
| 1,000 spring-animated quads at 120 Hz, Mac and iPhone | CPU p95 ≤ 4 ms, GPU p95 ≤ 4 ms, < 0.1% missed presents |
| Effects scene (Scrawl and launcher glass morph, blur, masks, shadows) | same budget |
| 1M-row list (fixed and variable height) | list CPU p95 ≤ 1 ms / 2 ms, first viewport ≤ 100 ms, ≤ 200 realized rows |
| 100 MB text file (Mac) | first usable viewport p95 ≤ 250 ms, peak extra memory ≤ 2.5× file size |
| At rest | zero animation ticks and zero GPU submissions |
| Idle memory | Mac ≤ 50 MiB RSS, iPhone ≤ 40 MiB footprint |
| Binary | Mac stripped arm64 hello ≤ 5 MiB |
| Reload: tokens / live literals / structural Rust edit on Mac / same on iPhone split | ≤ 50 ms / ≤ 100 ms / ≤ 300 ms goal (≈ 1 s measured) / Mac + USB |
| Incremental compile after a view edit | ≤ 2 s on the build worker |

## 9. Risks

1. **Text input and text quality.** Desktop IME on three platforms comes first now; iOS `UITextInput` (selection handles, loupe, autocorrect, dictation, marked text)
   is where custom-rendered frameworks most visibly fail, and text that's a shade thinner than CoreText reads as
   cheap. Mitigation: implement the native protocols directly from day one of each platform, borrow Zed's macOS/iOS
   code, and gate on a CoreText side-by-side diff.
2. **Matching OpenCode Desktop's look.** The bar is a Chromium-rendered app (the reason Slint with Skia was
   considered). Text rendering on Windows/Linux, shadows, gradients and borders must be indistinguishable from the web kit
   or better. Mitigation: per-component screenshot diffs against the `@opencode/ui` stories, rendered by headless
   Chromium beside Rune's headless renderer. (Native glass sync is now an optional-extra risk, spike S2.)
3. **Scope.** Renderer, text, IME, a11y, six shells, tooling: this is years of work for a human team. Mitigation:
   agents write it, but only inside the verification harness (scenes, goldens, traces, perf gates) built first;
   ship one real surface at a time and delete the old one each time; depend on Taffy/Parley/AccessKit instead of
   rewriting them.
4. **Accessibility** on every platform (AccessKit's iOS and Android adapters are young). Use the a11y tree as the
   inspector tree so it's exercised by every agent test.
5. **Platform feel drift** (scroll, menus, focus rings change each OS release). Keep those native or measured against
   native, as Flutter does with its scroll-physics overlay test.
6. **Split-process dev mode** is new. If S4 fails, fall back to subsecond in the simulator plus token/knobs reload on
   the device, and fast full installs.

## 10. Milestones

**M0 — Spikes (1–2 weeks).** Each answers one question with a number.

| Spike | Question | Where |
|---|---|---|
| S1 hot patch | edit → patched function running, with state, via subsecond in a non-Dioxus Rust binary | **done:** works; 989 ms median, 1,150 ms p95 ([08](research/08-spike-hot-patch.md)) |
| S1b fast patch | same edit at foreground priority, views in a leaf crate, fastest linker, cranelift: can it reach 300 ms? | build Mac |
| S2 glass sync | can an `NSGlassEffectView` underlay track a render-thread-animated shape at 120 Hz with the main thread blocked, no edge glitches? | GUI test Mac, recorded off the desk display |
| S3 pacing | `CADisplayLink` vs `CAMetalDisplayLink` latency and stability; immediate draw on input; event → present numbers | **probe built:** `CVDisplayLink` and `CADisplayLink` both tick at 120 Hz; Rune paces one submission per tick ([native-pacing](scripts/native-pacing/README.md)) |
| S4 split mode | renderer on an iPhone, app side on the Mac over USB: round-trip latency and edit → pixels | build Mac + an iPhone |
| S5 text | Parley + atlas + dilation vs CoreText, pixel diff on 200 strings at 1× and 2× | **done:** raster and blend calibrated against CoreText ([23](research/23-text-quality.md)) |

**M0.5 — Rendering smoke test (now).** Before any of the theming or component work: one small window (a settings
panel) that proves the basics render correctly. Rounded rects, borders, shadows, soft edges, crisp text that isn't
thin, correct Retina scale, a few springs, clicks. Checked by headless `--snap` renders and frame sequences, then
by Kit on a real window.

**M1 — Theming, gallery, docs site (now).** Rune's token
schema and theme system, with the OpenCode theme generated from `@opencode/ui` as its first consumer; core tree, Taffy, springs and presence, wgpu renderer (`UberQuad` + atlas text with
dilation), winit window, scenes, `--snap` headless goldens, inspector, motion traces, knobs. A gallery of the
first components with the same names as the web kit, each diffed against its web story. A docs/marketing site that
explains Rune with headless-rendered stills first and live WebGPU demos once the web build works. The settings demo
being built now is the seed.

**M2 — Hex Settings on Mac, Windows and Linux.** The first real screen. Forms, toggles, keycap recorder, lists,
text field (own shells start here: `NSTextInputClient`, TSF, `text-input-v3`), AccessKit, CI goldens on all three.
Replaces GPUI for that screen in Hex.

**M3 — OpenCode Desktop's main screens.** Sidebar and session list, the streaming transcript (markdown, code blocks,
tool pills), composer, diff view, command palette, popovers and menus, at 120 Hz with Kit's motion. Done when it looks
like OpenCode Desktop side by side and feels clearly better, on all three desktops. This is the flagship test.
It runs against **real data**: a live mode connects to the OpenCode server (by default `127.0.0.1:4096`, with basic auth
read from local config and never printed), lists real projects and
sessions, renders real transcripts, follows the event stream as turns arrive, and can send a prompt when Kit types one
in the composer. Not every feature, just enough to use it. Rules: agents building and testing it only use read-only
GETs and never send prompts or create sessions on the live server (never `GET /api/experimental/session/stats`);
goldens and committed fixtures are synthetic, never real session content; screenshots of real sessions stay local.

**M4 — Kit's Mac surfaces.** Scrawl pill (zero motion-linter findings, no more dropped frames than the Metal pill, Kit
picks it blind), the launcher's tray and panel, Scrawl review/Settings. Optional Liquid Glass underlays here (spike S2).

**M5 — Web.** The same app in the browser over WebGPU (docs site demos, knobs, gpu-gallery).

**M6 — iOS.** iOS shell, `UITextInput`, `RuneView` for embedding in SwiftUI, split-process dev mode; first surface
embedded in the private iOS client. **M7 — Android.**

## 11. Where Rune fits at Anomaly

One native stack for every Anomaly product, themed with OpenCode's design language (as a theme, not baked in). OpenCode Desktop is the flagship
test: recreate it and raise the bar, so every app built on Rune inherits that quality, the way Zed built GPUI for Zed
(but with the compositing, animation and text Zed's lacks). Hex Settings is the first real screen, then Hex, Scrawl
and the launcher, then the private iOS client one embedded surface at a time. Psychopomp should share Rune's spring, timeline and text
primitives rather than duplicate them. Because agents write Rune apps against scenes, an inspector and
deterministic goldens, it fits "build infinite software": every new app starts with the design language, motion and
verification harness already in place.

## 12. Open questions

1. App side on the platform main thread (default) or its own thread? Needs S3's latency numbers.
2. How far to lean on Core Animation for simple transform/opacity animations (the launcher's "promote to render server") vs
   always animating on our own render thread.
3. Glass morphing on iOS needs SwiftUI: host a tiny SwiftUI glass container per morph, or accept static glass shapes
   with our own morph on top?
4. Does Rune eventually *replace* OpenCode Desktop's Electron app, or ship beside it? M3 decides with evidence.
5. Text on Windows/Linux: grayscale everywhere, or ClearType-style subpixel where the web kit has it?
