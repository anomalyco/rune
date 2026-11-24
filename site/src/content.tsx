import type { ReactNode } from "react"

export type Spec = { label: string; values: ReactNode[]; mono?: boolean }
/** A file or folder in this repo that implements an idea. The prerender fails if the path doesn't exist. */
export type CodeRef = { path: string; note?: string }
export type Entry = { slug: string; title: string; body: ReactNode; spec?: Spec[]; code?: CodeRef[] }

const at = (path: string, note?: string): CodeRef => ({ path, note })

export const principles: Entry[] = [
  {
    slug: "feel-first",
    title: "Feel is the product",
    body: (
      <>
        <p>
          Rune is judged first on how it feels in the hand: 120&nbsp;Hz, springs that can be interrupted at any point, and
          nothing that pops, jumps or is cut off by a hard edge. The bar is SwiftUI on a ProMotion iPhone.
        </p>
      </>
    ),
    spec: [
      { label: "Frame", values: ["8.33 ms at 120 Hz"], mono: true },
      { label: "Target", values: ["event → frame p50 ≤ 8.3 ms"], mono: true },
    ],
    code: [
      at("crates/rune-motion/src/spring.rs", "closed-form springs"),
      at("examples/settings/src/main.rs", "the smoke test"),
    ],
  },
  {
    slug: "motion-defaults",
    title: "Motion rules are runtime defaults",
    body: (
      <>
        <p>
          Kit's motion rules are what the framework does when the app says nothing. Nothing animates on first mount,
          drags apply 1:1, removed elements play their exit before they unmount, stationary edges never move, and
          reduced motion swaps travel for a short fade.
        </p>
        <p>
          Animated properties change only through spring tracks, so a one-frame jump can't be written. In tests, the
          motion-lint rules (pops, jumps, reversals, jitter, hard clips) are failures.
        </p>
      </>
    ),
    spec: [
      { label: "Spring", values: [<code>Spring::new(duration, bounce)</code>], mono: true },
      { label: "Default", values: ["bounce 0"], mono: true },
    ],
    code: [
      at("crates/rune-motion/src/tokens.rs", "the motion palette"),
      at("crates/rune-motion/src/transition.rs", "exits before unmount"),
    ],
  },
  {
    slug: "animation-off-app",
    title: "Animation never runs through app code",
    body: (
      <p>
        Views state targets and Rune's spring tracks animate toward them. Today one main thread runs every frame step
        (only plain macOS wgpu overlays present on a worker), while still skipping view, layout and diff while values
        animate. Moving animation and present onto a dedicated render thread over a command channel is Target, so a slow
        handler won't stall an animation that's already moving.
      </p>
    ),
    spec: [{ label: "Target", values: ["app work per animated frame: 0"], mono: true }],
    code: [
      at("crates/rune-motion/src/scene.rs", "spring tracks"),
      at("crates/rune/tests/integration/live.rs", "no view, layout or shaping"),
    ],
  },
  {
    slug: "native-behaviour",
    title: "Native behaviour, one look",
    body: (
      <>
        <p>
          Text input, accessibility, scroll momentum, clipboard and window management behave the way each platform
          expects. Controls, type, motion and materials come from the app's theme and look the same everywhere, Mac
          included.
        </p>
        <p>The core ships no brand of its own. OpenCode's theme is the first theme built on it and the main example.</p>
      </>
    ),
    spec: [
      { label: "Native", values: ["IME, accessibility, scrolling, clipboard, windows"] },
      { label: "Themed", values: ["controls, type, motion, materials"] },
    ],
    code: [
      at("crates/rune-edit/src/lib.rs", "text input and IME"),
      at("crates/rune/src/a11y/tree.rs", "the accessibility tree (AccessKit)"),
      at("crates/rune-core/src/theme.rs", "themes as data"),
      at("themes/opencode", "OpenCode's theme"),
    ],
  },
  {
    slug: "agents",
    title: "Written and checked by agents",
    body: (
      <>
        <p>
          Agents write most of Rune and most Rune apps, so they need to see and test their own work without a person.
          Every component has named scenes, every scene renders headless and deterministically, and CLI flags
          (<code>--tree</code>, <code>--trace</code>, <code>--snap</code>, <code>--script</code>,{" "}
          <code>--hot-control</code>) inspect and drive apps today (a live JSON inspector socket is Target).
        </p>
        <p>
          The code is optimised for performance, correctness and speed of change rather than for a human reading it.
          Comments state invariants for the next agent.
        </p>
      </>
    ),
    spec: [{ label: "Render", values: [<code>--snap out.png --at 0.25</code>], mono: true }],
    code: [
      at("crates/rune/src/tooling.rs", "--snap, --scene, --script, --trace"),
    ],
  },
  {
    slug: "one-way",
    title: "One way to do things",
    body: (
      <p>
        Views are plain Rust functions that return a keyed tree. There is no view macro language, no second way to
        hold state and no notify call to forget. Errors should name the fix, such as{" "}
        <code>Text has no key inside a dynamic list; add .key(id)</code>.
      </p>
    ),
    code: [at("crates/rune/src/el.rs", "the whole view API")],
  },
  {
    slug: "fast-and-reference",
    title: "A fast path and a reference path",
    body: (
      <p>
        The renderer has <code>crates/rune-ref</code>, springs are checked against ODE integration, and{" "}
        <code>rune-taffy</code> compares against upstream <code>TaffyTree</code> (run locally and on scheduled macOS
        CI). A full per-change differential reference suite across all four subsystems remains Target.
      </p>
    ),
    spec: [{ label: "Reference", values: ["CPU renderer, no GPU tricks"] }],
    code: [
      at("crates/rune-ref/src/shader.rs", "the CPU renderer"),
      at("crates/rune-ref/tests/differential.rs", "GPU against CPU"),
    ],
  },
  {
    slug: "observable",
    title: "Everything is observable",
    body: (
      <p>
        Today <code>--tree</code> (carrying the accessibility tree), <code>--trace</code>, <code>--snap</code> and{" "}
        <code>--script</code> inspect and drive apps, and Rewind records the last 60&nbsp;s of presented rects and
        opacity. A live JSON inspector socket, pixel-to-<code>file:line</code> provenance and deterministic session
        replay remain Target.
      </p>
    ),
    code: [
      at("crates/rune-motion/src/trace.rs", "motion traces"),
      at("crates/rune/src/tooling.rs", "--tree, --trace"),
    ],
  },
  {
    slug: "hot-reload",
    title: "Edits reach the screen with state intact",
    body: (
      <>
        <p>
          Today structural Rust edits hot-patch on macOS via <code>rune/hot</code> (~8&nbsp;s median for Settings;
          ~1&nbsp;s in the S1 minimal spike, goal 300&nbsp;ms) and <code>rune-luau</code> reloads Luau scripts. Live
          token and literal reload without rebuilding remains Target.
        </p>
        <p>
          Target for iPhone development: because a phone can't load new code, a split-process dev mode will run the
          render thread on the iPhone and the app side on the Mac over the same command channel.
        </p>
      </>
    ),
    spec: [
      {
        label: "Targets",
        values: ["tokens ≤ 50 ms (Target)", "literals ≤ 100 ms (Target)", "Rust: ~8 s Settings / ~1 s S1, goal 300 ms"],
        mono: true,
      },
    ],
    code: [at("spikes/hot-patch", "989 ms median")],
  },
  {
    slug: "budgets",
    title: "Performance budgets are gates",
    body: (
      <p>
        <code>FrameStats</code> counts commits, layout nodes, glyphs and shaped layouts, allocation tests enforce zero
        per-frame allocations, and once settled an app does no work at all. Exact instruction counters and a CI
        frame-budget gate remain Target.
      </p>
    ),
    spec: [
      { label: "At rest", values: ["0 ticks, 0 GPU submissions"], mono: true },
      { label: "Per frame", values: ["0 allocations"], mono: true },
    ],
    code: [at("crates/rune/tests/alloc.rs", "0 allocations a frame"), at("crates/rune-motion/tests/alloc.rs")],
  },
  {
    slug: "owned",
    title: "Owned, not wrapped",
    body: (
      <p>
        Rune depends on foundational crates and never on another UI framework. Today text fields hold a plain{" "}
        <code>String</code> with winit IME on desktops and a sliding-window hidden <code>&lt;textarea&gt;</code> on the
        web; borrowing GPUI's rope, line layout cache and direct macOS/iOS text input code under GPUI's Apache-2.0
        licence remains Target.
      </p>
    ),
    spec: [{ label: "Uses", values: ["wgpu, Taffy, Parley, AccessKit"] }],
    code: [at("crates/rune/Cargo.toml", "dependencies")],
  },
]

export const layers: Entry[] = [
  {
    slug: "tree",
    title: "Core tree and state",
    body: (
      <>
        <p>
          A view is a plain Rust function that returns an <code>El</code> tree with explicit keys. App state lives in
          one plain Rust state value outside any view. Handlers are queued and run between frames with <code>&mut</code>{" "}
          access to the state, then the view re-runs and the new tree is diffed against the old one by key.
        </p>
        <p>
          The toggle below, from the settings example, states where the knob should be and animates nothing itself.
          The <code>.motion(motion::SNAPPY)</code> token tells Rune's spring tracks how to get there (on the main thread
          today; dedicated render thread is Target).
        </p>
      </>
    ),
    spec: [
      { label: "Uses", values: ["keyed retained tree, one plain Rust state value"] },
      { label: "Not", values: ["signals, GPUI entities, immediate mode"] },
    ],
    code: [at("crates/rune/src/el.rs", "El, keys, handlers"), at("crates/rune/src/runtime.rs", "queue, view, diff")],
  },
  {
    slug: "layout",
    title: "Layout",
    body: (
      <p>
        Taffy's solver runs over <code>rune_taffy::LayoutStore</code> beside Rune's node tree (one unified tree is
        Target). Layout runs once, at the target state; moving between layouts is driven by Rune's spring tracks (on the
        main thread today; dedicated render thread is Target). Long lists build only the visible rows plus overscan and
        keep the rest as estimated heights.
      </p>
    ),
    spec: [
      { label: "Uses", values: ["Taffy"] },
      { label: "Not", values: ["own engine, SwiftUI-style proposals"] },
    ],
    code: [
      at("crates/rune/src/runtime.rs", "Taffy via LayoutStore"),
      at("crates/rune/src/el/virtual_list.rs", "visible rows only"),
    ],
  },
  {
    slug: "text",
    title: "Text",
    body: (
      <p>
        Parley handles shaping, bidi and line breaking, with shaped runs cached apart from wrap points so resizing never
        re-shapes. On macOS and Mac browsers, default <code>FontSmoothing::On</code> uses a CoreGraphics pipeline port
        with ppem-dependent subpixel steps and outline dilation; elsewhere, default <code>WindfoilChromium</code> fills
        outline masks on quarter-pixel pens with Skia text gamma, hinted small CJK and off-macOS vertical hints.
        Overflow is cut only when requested: <code>El::truncate_fade</code> fades it out, while <code>El::truncate</code>{" "}
        and <code>El::max_lines</code> end in an ellipsis.
      </p>
    ),
    spec: [
      { label: "Uses", values: ["Parley, swash, skrifa"] },
      { label: "Not", values: ["CoreText only, cosmic-text"] },
    ],
    code: [
      at("crates/rune-text/src/lib.rs", "Parley, swash, the atlas"),
      at("research/23-text-quality.md", "against CoreText"),
    ],
  },
  {
    slug: "render-thread",
    title: "Spring tracks (Target: render thread)",
    body: (
      <>
        <p>
          Today one main thread owns the presented tree and samples every frame step (skipping view, layout and diff
          while animating); moving this onto a dedicated render thread over a command channel is Target. Every animated
          property is a track of analytic spring segments, so position and velocity are exact at any time and a retarget
          starts the new spring from where the value really is.
        </p>
        <p>
          Layout animates per edge, so an edge that isn't moving gets no track. Removed nodes leave the flow, play their
          exit and unmount at opacity 0. Drags and scrolling are applied here, 1:1, and one clock with time scale and
          stepping is shared by tests and traces.
        </p>
      </>
    ),
    spec: [
      { label: "Spring", values: ["ω = 2π / duration", "ζ = 1 − bounce"], mono: true },
      { label: "Snap", values: ["both edges to 1/64 px"], mono: true },
    ],
    code: [
      at("crates/rune-motion/src/spring.rs"),
      at("crates/rune-motion/src/scene.rs", "today on the main thread"),
      at("crates/rune-motion/src/transition.rs", "presence"),
    ],
  },
  {
    slug: "renderer",
    title: "Renderer",
    body: (
      <>
        <p>
          One instanced pipeline (with pinned single-kind fast paths) draws rounded rects, borders, analytic shadows,
          glyphs and vector strokes and fills, so a typical plain frame is one render pass with a draw call per
          atlas-page batch (or pinned kind run), plus passes when isolated layers, blurs or GPU mask fills are used.
          Every instance carries a clip with soft gradient edges, which makes "no hard edges" free. Blending is in sRGB
          by default (matching CSS and Core Animation); <code>App::srgb_blend(false)</code> switches to linear light.
        </p>
        <p>
          Scale-animating text draws straight from GPU outlines in outline-drawn modes, and in-app blur runs through{" "}
          <code>El::backdrop_blur</code>; curve-drawn icons and Vello sparse strips remain Target.
        </p>
      </>
    ),
    spec: [
      { label: "Uses", values: ["native Metal on macOS, wgpu on Windows/Linux/web/iOS"] },
      { label: "Not", values: ["Skia, compute Vello, hand-written Vulkan/D3D12"] },
    ],
    code: [
      at("crates/rune/src/render.rs"),
      at("crates/rune/src/shaders/shader.wgsl", "one uber-quad pipeline"),
      at("crates/rune-core/src/shape.rs", "strokes and fills"),
    ],
  },
  {
    slug: "shells",
    title: "Platform shells",
    body: (
      <p>
        A shell owns windows and panels, menus, clipboard, drag and drop, the text input protocol, the accessibility
        adapter and the app lifecycle. Rune runs on winit today (with winit IME on desktops and a sliding-window hidden{" "}
        <code>&lt;textarea&gt;</code> on the web). Moving to thin shells per platform that implement{" "}
        <code>NSTextInputClient</code>, TSF, <code>text-input-v3</code> and <code>UITextInput</code> directly remains
        Target.
      </p>
    ),
    spec: [
      { label: "Uses", values: ["winit, then own shells; AccessKit"] },
      { label: "Not", values: ["winit's IME, SDL3"] },
    ],
    code: [
      at("crates/rune/src/window.rs", "winit today"),
      at("crates/rune/src/platform/macos.rs"),
      at("crates/rune/src/platform/win32.rs", "overlays"),
      at("crates/rune/src/platform/layer_shell.rs", "Wayland overlays"),
      at("crates/rune/src/a11y/mod.rs", "UI Automation, NSAccessibility and AT-SPI adapters"),
    ],
  },
  {
    slug: "native-layers",
    title: "Native layers on Apple",
    body: (
      <p>
        Liquid Glass samples what's behind the window, so only the window server can draw it. For Mac surfaces that want
        it, the Apple shell places system glass views behind transparent regions of Rune's layer and updates their masks
        in the same transaction as the frame (on the main thread today; render thread is Target). This is optional;
        every platform gets Rune's own blur and materials.
      </p>
    ),
    spec: [{ label: "Uses", values: ["NSGlassEffectView, CATransaction"] }],
  },
  {
    slug: "tooling",
    title: "Agent tooling",
    body: (
      <p>
        Scenes are named states with fixture data. They render in the app, offscreen in tests and in a gallery, with
        pinned fonts and a fixed clock. Today CLI flags (<code>--tree</code>, <code>--trace</code>, <code>--snap</code>,{" "}
        <code>--script</code>, <code>--hot-control</code>) inspect trees, emit motion traces from the runtime and drive
        scripted input; a live versioned JSON inspector socket remains Target.
      </p>
    ),
    code: [
      at("crates/rune/src/tooling.rs", "scenes and flags"),
      at("crates/rune-motion/src/trace.rs", "motion traces"),
    ],
  },
  {
    slug: "playground",
    title: "Playground",
    body: (
      <p>
        A storybook built into the window, on ⌘⇧D. The app's scenes and its own stories, in a tree of groups nested by
        their paths, show one at a time on a plain stage, live and interactive, with their triggers and controls beside
        them. The keyboard is in one column at a time, the list, the stage or the controls, so a story's own keys work on
        its stage. Each story has its own state, so the real one is never touched. R replays a story from the start, its
        timed steps running on the logical clock, the same every time.
      </p>
    ),
    code: [at("crates/rune/src/playground.rs", "stories, triggers and replay")],
  },
]

export type Decision = { area: string; chose: string; rejected: string; why: string }

export const decisions: Decision[] = [
  {
    area: "Language",
    chose: "Rust",
    rejected: "Zig, C++, Swift",
    why: "The only option with production text, layout and accessibility crates, no garbage collector, wasm, and a working hot-patcher.",
  },
  {
    area: "GPU",
    chose: "Native Metal on macOS, wgpu elsewhere",
    rejected: "Hand-written Vulkan and D3D12",
    why: "Rune's native Metal backend is the default on macOS (plugged into wgpu's API); wgpu ships Windows, Linux, web and by default iOS.",
  },
  {
    area: "Renderer",
    chose: "One instanced uber-quad pass, glyph atlas / GPU outlines",
    rejected: "Skia, compute Vello",
    why: "A typical plain frame is one render pass (one draw call per atlas-page batch or pinned kind run), and inline soft clip edges cost nothing extra.",
  },
  {
    area: "Look",
    chose: "Token-driven themes",
    rejected: "Per-OS look, a built-in brand",
    why: "Apps carry their own design language on every platform. OpenCode's is one theme, kept outside the core.",
  },
  {
    area: "Text",
    chose: "Parley, swash, skrifa",
    rejected: "CoreText only, cosmic-text",
    why: "Platform-matched defaults (CoreGraphics pipeline port on macOS/Mac browsers, WindfoilChromium GPU outlines elsewhere), and resizing never re-shapes.",
  },
  {
    area: "Layout",
    chose: "Taffy via rune_taffy::LayoutStore",
    rejected: "An own engine, SwiftUI-style proposals",
    why: "Agents already know flexbox; LayoutStore sits beside Rune's node tree today (one unified tree is Target).",
  },
  {
    area: "State",
    chose: "Keyed tree from plain functions",
    rejected: "Signals, GPUI entities, immediate mode",
    why: "One plain Rust state value outside views survives hot patches, and handlers never run inside render, so borrows can't collide.",
  },
  {
    area: "Animation",
    chose: "Analytic spring tracks (Target: render thread)",
    rejected: "Re-rendering views each frame",
    why: "Today springs sample on the main thread while skipping view/layout/diff, and retargets keep velocity; a dedicated render thread is Target.",
  },
  {
    area: "Accessibility",
    chose: "AccessKit",
    rejected: "Own bridges",
    why: "The same tree powers --tree dumps today (and the Target live inspector), so agent tests exercise it.",
  },
  {
    area: "Hot reload",
    chose: "Subsecond patches on macOS + rune-luau (Target: live values, split iPhone)",
    rejected: "Mandatory view DSL, loading code on iOS devices",
    why: "Core views stay plain Rust with opt-in rune/hot (~8 s Settings) and optional rune-luau; live token/literal reload and split iPhone dev mode are Target.",
  },
]

export type Milestone = { id: string; title: string; status: "done" | "now" | "started" | "next" | "later"; body: ReactNode }

export const milestones: Milestone[] = [
  {
    id: "m0",
    title: "Spikes",
    status: "now",
    body: (
      <p>
        Short experiments that each answer one question with a number: hot-patch speed, glass underlay sync, frame
        pacing, the iPhone split mode, and text quality against CoreText. Hot patching is done, at 989&nbsp;ms median from
        save to new output.
      </p>
    ),
  },
  {
    id: "m0-5",
    title: "Rendering smoke test",
    status: "now",
    body: (
      <p>
        A small settings window that proves the basics: rounded rects, borders, shadows, soft edges, crisp text, Retina
        scale, springs and clicks, checked by headless renders and then on a real window.
      </p>
    ),
  },
  {
    id: "m1",
    title: "Theming, gallery and this site",
    status: "now",
    body: (
      <p>
        The token schema and theme system, with OpenCode's theme generated from <code>@opencode/ui</code> as its first
        consumer. A gallery of components with the same names as the web kit, each diffed against its web story.
      </p>
    ),
  },
  {
    id: "m2",
    title: "Hex Settings",
    status: "next",
    body: (
      <p>
        The first real screen, on Mac, Windows and Linux: forms, toggles, a keycap recorder, lists and a text field. Own
        shells start here, along with AccessKit and goldens on all three platforms.
      </p>
    ),
  },
  {
    id: "m3",
    title: "OpenCode Desktop",
    status: "started",
    body: (
      <p>
        The main screens: sidebar and sessions, the streaming transcript, composer, diff view, command palette and
        menus, against real data. Done when it looks like OpenCode Desktop side by side and feels clearly better on all
        three desktops.
      </p>
    ),
  },
  {
    id: "m4",
    title: "Kit's Mac surfaces",
    status: "later",
    body: <p>An app's pill, tray and launcher, and their settings windows, with optional Liquid Glass.</p>,
  },
  {
    id: "m5",
    title: "Web",
    status: "started",
    body: (
      <p>
        The same apps in the browser over WebGPU, starting with live demos on this site. An experimental target builds
        and runs today (<code>WEB.md</code>, <code>examples/web-hello</code>).
      </p>
    ),
  },
  {
    id: "m6",
    title: "iOS, then Android",
    status: "later",
    body: (
      <p>
        An iOS shell with <code>UITextInput</code>, a view for embedding Rune in SwiftUI, and the split-process dev mode.
        The first surface ships inside OpenCode iOS. Android follows.
      </p>
    ),
  },
]

export const risks: Entry[] = [
  {
    slug: "risk-text",
    title: "Text input and text quality",
    body: (
      <p>
        Input methods on three desktops, then iOS selection, autocorrect and dictation, are where custom-rendered
        frameworks most visibly fail. Rune implements each platform's protocol directly and gates text rendering on a
        pixel diff against CoreText.
      </p>
    ),
  },
  {
    slug: "risk-look",
    title: "Matching OpenCode Desktop",
    body: (
      <p>
        The bar is a Chromium-rendered app. Each component is diffed against its <code>@opencode/ui</code> story,
        rendered by headless Chromium beside Rune's headless renderer.
      </p>
    ),
  },
  {
    slug: "risk-scope",
    title: "Scope",
    body: (
      <p>
        A renderer, text, input methods, accessibility, six shells and tooling is years of work for a human team. Agents
        write it inside a verification harness that was built first, and Rune ships one real surface at a time.
      </p>
    ),
  },
]
