import type { CSSProperties, ReactNode } from "react"
import { decisions, layers, milestones, principles, risks, type CodeRef, type Entry, type Milestone } from "./content"
import { Code } from "./Code"
import {
  ArchitectureDiagram,
  CrateMap,
  FrameDiagram,
  PipelineDiagram,
  StateLoopDiagram,
  TestDiagram,
  ThreadDiagram,
} from "./Diagram"
import { withBase } from "./base"
import { Wordmark } from "./brand/Wordmark"
import { REPO } from "./routes"
import { PageHead, SectionHead } from "./system/Frame"

export const num = (i: number) => String(i + 1).padStart(2, "0")

function Rule({ entry, number, children }: { entry: Entry; number?: string; children?: ReactNode }) {
  return (
    <article id={entry.slug} className="rule">
      <header className="rule-head">
        {number && (
          <a className="rule-num" href={`#${entry.slug}`} aria-label={`${entry.title}, ${number}`}>
            {number}
          </a>
        )}
        <h3 className="rule-title">{entry.title}</h3>
      </header>
      <div className="rule-body">
        <div className="rule-text">{entry.body}</div>
        {(entry.spec || entry.code) && (
          <dl className="rule-spec">
            {entry.spec?.map(s => (
              <div key={s.label}>
                <dt>{s.label}</dt>
                <dd className={s.mono ? "mono" : undefined}>
                  {s.values.map((v, i) => (
                    <span key={i}>{v}</span>
                  ))}
                </dd>
              </div>
            ))}
            {entry.code && (
              <div>
                <dt>Code</dt>
                <dd>
                  <CodeRefs refs={entry.code} />
                </dd>
              </div>
            )}
          </dl>
        )}
      </div>
      {children}
    </article>
  )
}

/** Files in this repo, crate (or folder) first so the names line up, each linked to GitHub. */
function CodeRefs({ refs }: { refs: CodeRef[] }) {
  return (
    <ul className="refs">
      {refs.map(({ path, note }) => {
        const parts = path.split("/")
        const inCrate = parts[0] === "crates" || parts[0] === "examples"
        const label = inCrate ? parts[1]! : parts[0]!
        const rest = parts.slice(inCrate ? 2 : 1)
        const folder = !/\.\w+$/.test(path)
        const name = (rest[0] === "src" ? rest.slice(1) : rest).join("/") + (folder ? "/" : "")
        return (
          <li key={path}>
            <a className="ref-link" href={`${REPO}/${folder ? "tree" : "blob"}/dev/${path}`} title={path}>
              <span className="ref-label">{label}</span>
              <span>
                <span className="ref-name">{name}</span>
                {note && <span className="ref-note">{note}</span>}
              </span>
            </a>
          </li>
        )
      })}
    </ul>
  )
}

export function Pager({ prev, next }: { prev?: [string, string]; next?: [string, string] }) {
  return (
    <nav className="doc-pager" aria-label="Pages">
      {prev ? <a href={withBase(prev[0])}>← {prev[1]}</a> : <span />}
      {next && <a href={withBase(next[0])}>{next[1]} →</a>}
    </nav>
  )
}

type Status = "works" | "partly" | "planned"

const STATUS: Record<Status, { label: string; dot: Milestone["status"] }> = {
  works: { label: "Works today", dot: "done" },
  partly: { label: "Partly", dot: "started" },
  planned: { label: "Planned", dot: "later" },
}

/** What Rune can build today, checked against the code; each row names where to see it. */
const buildable: { title: string; text: ReactNode; status: Status; meta: string; href?: string }[] = [
  {
    title: "Desktop apps on macOS",
    text: (
      <>
        Windows drawn entirely by Rune through winit and wgpu. The Settings window and a gallery of OpenCode's
        components run today.
      </>
    ),
    status: "works",
    meta: "examples/settings",
  },
  {
    title: "Windows and Linux",
    text: (
      <>
        The same wgpu backend, with Win32, X11 and Wayland code for overlays already in. Running and checking apps on
        both is the next milestone, with Hex's Settings as the first real screen.
      </>
    ),
    status: "partly",
    meta: "M2",
    href: "/roadmap#m2",
  },
  {
    title: "Background and menu-bar apps",
    text: (
      <>
        An app can start with no window, keep running after its window closes, reopen it with the same state, and on
        macOS run with no Dock icon.
      </>
    ),
    status: "works",
    meta: "examples/background",
  },
  {
    title: "Overlays and HUDs",
    text: (
      <>
        Transparent, click-through windows over every display, and HUD panels at a screen edge that never take focus,
        fed from any thread. Built for Rooms.
      </>
    ),
    status: "works",
    meta: "examples/overlay",
  },
  {
    title: "Video",
    text: (
      <>
        Decoded frames drawn as ordinary Rune nodes: NV12 IOSurfaces on macOS, uploaded planes elsewhere, fed by a
        background producer. The example uses synthetic frames.
      </>
    ),
    status: "works",
    meta: "examples/video-tiles",
  },
  {
    title: "Headless rendering",
    text: (
      <>
        Every app also runs with no window: PNG stills at a chosen moment, frame sequences, input scripts and motion
        traces, for tests, CI goldens and agents. A CPU reference renderer checks the GPU one.
      </>
    ),
    status: "works",
    meta: "crates/rune-ref",
  },
  {
    title: "Effects and lighting",
    text: (
      <>
        Glow, fire, electricity, distortion and motion blur (<code>rune-fx</code>), and soft shadows and highlights from
        a light you place (<code>rune-light</code>), drawn through the compositor hook.
      </>
    ),
    status: "works",
    meta: "examples/lab",
  },
  {
    title: "Web",
    text: (
      <>
        The same apps in a browser over WebGPU, in one canvas on the page's main thread. Experimental: Safari keyboard
        checks, accessibility and a WebGL fallback are still to come.
      </>
    ),
    status: "partly",
    meta: "examples/web-hello",
  },
  {
    title: "iOS, then Android",
    text: (
      <>
        An iOS shell with native text input and a view for embedding Rune in SwiftUI. Nothing runs on a phone yet.
      </>
    ),
    status: "planned",
    meta: "M6",
    href: "/roadmap#m6",
  },
]

export function HomeView() {
  return (
    <>
      <header id="home-hero" className="home-hero">
        <h1 className="masthead">
          <span className="sr-only">Rune</span>
          <Wordmark periodic />
        </h1>
        <p className="home-lede">
          <strong>
            Rune is a Rust UI framework with its own GPU renderer, text, layout, spring animation and thin shells for
            each platform.
          </strong>{" "}
          It aims to feel like SwiftUI on a 120&nbsp;Hz iPhone on every platform, and to be written and checked by coding
          agents. It's early, and its APIs will change.
        </p>
      </header>

      <section className="home-section">
        <SectionHead id="app" title="A Rune app">
          <p>
            State is a plain struct, the view is a function of it, and a handler changes it. This is{" "}
            <code>examples/hello</code> apart from its test, which is further down. The switch never animates itself: the view says where the knob is, and{" "}
            <code>.motion(motion::SNAPPY)</code> names the spring that moves it.
          </p>
        </SectionHead>
        <div className="app-demo">
          <Code id="hello" />
          <figure className="shot app-shot" style={{ "--shot-w": "320px" } as CSSProperties}>
            <img
              src={withBase("/shots/hello-on.png")}
              alt="A dark window with the words Lights on above a blue switch, turned on"
              width={320}
              height={200}
            />
            <figcaption>
              Headless: <code>--snap hello.png --scene on</code>
            </figcaption>
          </figure>
        </div>
      </section>

      <section className="home-section">
        <SectionHead id="frame" title="From your code to the screen" />
        <PipelineDiagram />
      </section>

      <section className="home-section">
        <SectionHead id="how" title="How do I…">
          <p>Each of these is cut from a Rune example or test when the site builds, so it compiles and runs.</p>
        </SectionHead>
        <div className="howto-list">
          <article className="howto">
            <h3 id="how-list">Keep a list in state</h3>
            <p>
              Build a row per item with a stable key. Rows added or removed after the first frame animate in and out on
              their own, and handlers capture the id they act on.
            </p>
            <div className="app-demo">
              <Code id="todo-list" />
              <figure className="shot app-shot" style={{ "--shot-w": "360px" } as CSSProperties}>
                <img
                  src={withBase("/shots/todo.png")}
                  alt="A dark todo list with a progress bar, three rows with round checkboxes, and an Add a todo link"
                  width={360}
                  height={240}
                  loading="lazy"
                />
                <figcaption>
                  Headless: <code>--snap todo.png</code>
                </figcaption>
              </figure>
            </div>
          </article>
          <article className="howto">
            <h3 id="how-spring">Animate on a spring</h3>
            <p>
              Set the value the state implies and name a spring with <code>Spring::new(duration, bounce)</code>. When
              a todo is ticked, the bar retargets from wherever it is, keeping its velocity.
            </p>
            <Code id="todo-spring" />
          </article>
          <article className="howto">
            <h3 id="how-overlay">Open an overlay on every display</h3>
            <p>
              <code>run_windows</code> builds one window per <code>WindowSpec</code>, and{" "}
              <code>WindowOptions::overlay</code> makes it transparent, borderless, on top and click-through. All the
              windows can share one <code>Live</code> value fed from a background thread.
            </p>
            <Code id="overlay-windows" />
          </article>
          <article className="howto">
            <h3 id="how-test">Test it without a window</h3>
            <p>
              A <code>Runtime</code> is the window's runtime with no window. The test taps the switch and steps the
              clock to check the knob travels and lands. It runs in <code>cargo test</code> with no GPU.
            </p>
            <Code id="hello-test" />
          </article>
        </div>
        <p className="home-more">
          <a href={withBase("/how-it-works")}>How it works</a> follows one frame through the code, with more examples.
        </p>
      </section>

      <section className="home-section">
        <SectionHead id="build" title="What you can build" aside="October 2026">
          <p>
            Native apps first, and anything that needs Rune's pixels without a window, since headless runs use the same
            code. The web build is experimental, and phones are planned.
          </p>
        </SectionHead>
        <ul className="index-list">
          {buildable.map(b => (
            <li key={b.title} className="index-row">
              <span className="index-title">{b.title}</span>
              <p className="index-text">{b.text}</p>
              <span className="index-meta build-meta">
                <span className="status" data-status={STATUS[b.status].dot}>
                  {STATUS[b.status].label}
                </span>
                {b.href ? (
                  <a href={withBase(b.href)}>{b.meta}</a>
                ) : (
                  <a href={`${REPO}/tree/dev/${b.meta}`}>{b.meta}</a>
                )}
              </span>
            </li>
          ))}
        </ul>
        <div className="shots">
          <figure className="shot">
            <img src={withBase("/shots/settings.png")} alt="A dark Settings window with theme, accent, toggle and slider rows" width={1240} height={1040} loading="lazy" />
            <figcaption>examples/settings, rendered headless</figcaption>
          </figure>
        </div>
      </section>

      <section className="home-section">
        <SectionHead id="next" title="Next">
          <p>
            The first real screen is Hex's Settings on Mac, Windows and Linux, which brings Rune's own platform shells
            and native text input. After that come OpenCode Desktop's main screens. That's the flagship test: it should
            look like the current app side by side and feel clearly better.
          </p>
        </SectionHead>
      </section>

      <Pager next={["/how-it-works", "How it works"]} />
    </>
  )
}

export function HowItWorksView() {
  return (
    <>
      <PageHead eyebrow={<span>How it works</span>} title="How Rune works">
        <p>
          You write a function from your app's state to a keyed tree of elements. Rune lays the tree out, animates
          every change on springs and draws the frame with wgpu.
        </p>
        <p>
          This page follows one frame through the code, then shows the code you write. Every example is cut from
          Rune's source when the site builds and links to the lines it came from.
        </p>
      </PageHead>

      <section className="doc-section">
        <SectionHead id="frame" title="A frame">
          <p>
            Each frame runs these steps in order, in <code>Runtime::frame</code> and the window's redraw. The view
            function only runs when state changed, so animating costs your code nothing.
          </p>
        </SectionHead>
        <FrameDiagram />
      </section>

      <section className="doc-section">
        <SectionHead id="app" title="A window and a view">
          <p>
            An app is a state type, a view function and an <code>AppSpec</code>. <code>rune::run</code> opens the
            window, or renders headless when given a flag like <code>--snap</code>. Scenes are named starting states
            for those renders. This is the <a href={withBase("/")}>Settings window</a> from the overview.
          </p>
        </SectionHead>
        <div className="code-stack">
          <Code id="app-cargo" />
          <Code id="app-main" />
        </div>
      </section>

      <section className="doc-section">
        <SectionHead id="state" title="State changes in handlers">
          <p>
            A handler is a plain function that gets <code>&amp;mut</code> state. Handlers are queued and run between
            frames, then the view re-runs and the new tree is matched to the old one by key. The toggle never animates
            anything itself. It says where the knob should be, and <code>.motion(motion::SNAPPY)</code> names the
            spring that gets it there.
          </p>
        </SectionHead>
        <StateLoopDiagram />
        <div className="code-stack">
          <Code id="toggle" />
          <Code id="motion-tokens" />
        </div>
      </section>

      <section className="doc-section">
        <SectionHead id="springs" title="Springs keep their velocity">
          <p>
            <code>Spring::new(duration, bounce)</code> uses SwiftUI's numbers: ω&nbsp;=&nbsp;2π&nbsp;/&nbsp;duration
            and ζ&nbsp;=&nbsp;1&nbsp;−&nbsp;bounce. Each animated property is a track of closed-form spring segments, so
            its position and velocity are exact at any time. When the target changes mid-flight, the new segment
            starts from that position and velocity, so the value turns smoothly instead of jumping or stopping. This
            test checks it.
          </p>
        </SectionHead>
        <Code id="retarget" />
      </section>

      <section className="doc-section">
        <SectionHead id="live" title="Values that change every frame skip the view">
          <p>
            Audio levels, remote cursors and drag positions change too often to go through state.
            A <code>Live&lt;T&gt;</code> binds straight to a property, and any thread can set it. The test checks that
            setting four of them runs no view, layout or text shaping.
          </p>
        </SectionHead>
        <Code id="live" />
      </section>

      <section className="doc-section">
        <SectionHead id="threads" title="Threads">
          <p>
            Springs belong to the runtime's <code>Scene</code>, which today lives on the main thread with everything
            else. The <a href={withBase("/architecture")}>architecture</a> moves it to a render thread.
          </p>
        </SectionHead>
        <ThreadDiagram />
      </section>

      <section className="doc-section">
        <SectionHead id="testing" title="Tests run without a window">
          <p>
            A test builds a <code>Runtime</code> from a state and a view, steps it frame by frame and reads back what
            it laid out. The same binaries take flags that render PNGs, play input scripts and write motion traces.
          </p>
        </SectionHead>
        <TestDiagram />
        <div className="code-stack">
          <Code id="test-segmented" />
          <Code id="tap-script" />
          <Code id="snap-commands" />
        </div>
        <div className="prose prose-after">
          <p>
            The renderer is checked against a reference. <code>rune-ref</code> draws the same instances on the CPU
            with strict float rules, and a hash of each generated scene is pinned in <code>goldens.json</code>. A
            differential test then holds the GPU to within 2 levels per channel of the CPU image.
          </p>
        </div>
        <Code id="golden-hashes" />
      </section>

      <section className="doc-section">
        <SectionHead id="crates" title="Where the code lives">
          <p>
            The workspace is a few crates. <code>rune</code> holds most of the framework and builds on these
            foundations.
          </p>
        </SectionHead>
        <CrateMap />
        <Code id="rune-deps" />
      </section>

      <Pager prev={["/", "Overview"]} next={["/launch-fast", "Launch fast"]} />
    </>
  )
}

export function Sample({ lang = "Rust", children }: { lang?: string; children: string }) {
  return (
    <figure className="code-block">
      <figcaption className="code-head">
        <span>{lang}</span>
      </figcaption>
      <pre className="code">
        <code>{children}</code>
      </pre>
    </figure>
  )
}

const launchSetup = `let app = App::new(AppSpec::new("Notes", (520.0, 320.0), State::default, view))
    .accessory()
    .keep_running();
let handle = app.handle();
app.on_ready(move |_window| {
    handle.update(|state, tx| {
        // First frame is on screen: now the menu bar item, indexing, network.
        state.start_background_work(tx);
    });
})
.run();`

const launchHidden = `// Keep the window; hide it instead of closing it.
let app = App::new(spec).keep_running().windowless();
let handle = app.handle();

// On the shortcut, the tray click or a Dock reopen:
handle.show();   // or handle.open() to also activate the app
// When the user dismisses it:
handle.hide();`

const launchNumbers: { row: string; before: string; after: string }[] = [
  { row: "Cold shader cache, content on screen", before: "599", after: "312" },
  { row: "Cold shader cache, window on screen", before: "422", after: "277" },
  { row: "Warm shader cache, content on screen", before: "466", after: "299" },
  { row: "Cold shader cache, Renderer::new alone", before: "232", after: "85" },
]

export function LaunchFastView() {
  return (
    <>
      <PageHead eyebrow={<span>Launch fast</span>} title="Launch fast">
        <p>
          A Rune app can put its first window, with its content drawn, on screen about 300 ms after{" "}
          <code>open</code> on an Apple silicon Mac, including the first launch after an install or update. Four
          things get it there: start the GPU on a thread, ship the shader precompiled, keep the main thread free until
          the first frame, and reuse a window instead of rebuilding it.
        </p>
        <p>
          Two of them need one line or nothing from the app. The rest is about what the app leaves out of its first
          frame. The numbers are at the end. Keeping the app fast after launch is on{" "}
          <a href={withBase("/fast-apps")}>Fast apps</a>.
        </p>
      </PageHead>

      <section className="doc-section">
        <SectionHead id="prewarm" title="Call prewarm_gpu first in main">
          <p>
            A first frame needs a GPU adapter, a device and Rune's render pipeline, and making them is most of a cold
            launch. <code>rune::prewarm_gpu()</code> starts that work on a thread.
          </p>
        </SectionHead>
        <Sample>{`fn main() {
    rune::prewarm_gpu();
    // ... build the App and run it
}`}</Sample>
        <div className="prose prose-after">
          <p>
            On macOS the thread makes the wgpu instance, adapter and device, then builds the uber pipeline, loading the
            precompiled shader described below. The first window or overlay opened at launch takes the result, and
            waits if the thread is still working. Rune's run functions call it too, but only once they start the event
            loop, so calling it yourself in the first line of <code>main</code> gains the time your own setup takes
            before that.
          </p>
          <ul>
            <li>Calling it twice does nothing the second time.</li>
            <li>
              If the app opens no window at launch, the result is dropped and nothing stays in memory. A{" "}
              <code>windowless</code> app that opens its window later makes the device then, as it always did.
            </li>
            <li>
              If the first window needs GPU features the prewarmed device lacks, or the adapter can't present to its
              surface, the window makes a new device on the same instance. The pipeline is built for the usual surface
              format; a window with another one builds its own.
            </li>
            <li>On other platforms it does nothing, because the window's surface chooses the adapter there.</li>
          </ul>
        </div>
      </section>

      <section className="doc-section">
        <SectionHead id="metallib" title="Precompiled Metal shaders">
          <p>
            Without help, a launch translates the uber shader's WGSL to Metal source and has Metal compile it. Rune
            commits the compiled library instead, as <code>crates/rune/src/uber.metallib</code> (about 33 KB), and on
            macOS loads it through wgpu's passthrough shader support. This skips WGSL parsing, validation, translation
            and Metal's compile on every launch. <code>Renderer::new</code> on a cold shader cache goes from 232 ms to
            85 ms, and from 3.4 ms to 0.6 ms on a warm one.
          </p>
        </SectionHead>
        <div className="prose">
          <p>
            Apps need nothing and don't need Xcode: the library is a file in the crate. Rune falls back to the WGSL
            path, which is slower but identical on screen, when:
          </p>
          <ul>
            <li>the adapter lacks <code>PASSTHROUGH_SHADERS</code>;</li>
            <li>
              <code>RUNE_DISABLE_METALLIB=1</code> is set, which is also how to compare the two paths;
            </li>
            <li>the committed library's fingerprint doesn't match the shader the build generates;</li>
            <li>Metal rejects the library or the pipeline made from it.</li>
          </ul>
          <h3>Changing the shader</h3>
          <p>
            Edit <code>clip.wgsl</code> or <code>shader.wgsl</code>, then run{" "}
            <code>cargo test -p rune --lib precompiled_uber_metallib_is_fresh</code> on a Mac. The test compares the
            library and the shader's fingerprints, and its failure message says what to do:
          </p>
          <ul>
            <li>
              <strong>"MSL is byte-identical; no Xcode or metallib regeneration needed"</strong> means only the
              fingerprint changed, for example after a comment edit or a descriptor change that generates the same
              Metal. Paste the three <code>pub(super) const</code> lines it prints into{" "}
              <code>crates/rune/src/uber_metallib_hash.rs</code>.
            </li>
            <li>
              <strong>"generated MSL or Metal compile flags changed"</strong> or{" "}
              <strong>"does not match recorded hash"</strong> means the library is stale. Run{" "}
              <code>scripts/precompile-metal-shaders.sh</code>. It needs Xcode's Metal toolchain (
              <code>xcrun -sdk macosx metal</code>), dumps the generated MSL, compiles it and rewrites both{" "}
              <code>uber.metallib</code> and <code>uber_metallib_hash.rs</code>. Commit both files.
            </li>
          </ul>
          <p>
            On other platforms the same test can only say the fingerprint changed, and tells you to run it on macOS for
            the exact lines.
          </p>
          <h3>What it doesn't cover</h3>
          <p>
            Only the uber pipeline is precompiled. The effect, lighting and blur shaders still compile from WGSL the
            first time they're used, so an app that draws them on its first frame still pays that on a cold cache.
            iOS isn't covered. Metal binary archives could cover the rest, but they would need support in wgpu, which it doesn't have.
          </p>
        </div>
      </section>

      <section className="doc-section">
        <SectionHead id="first-frame" title="The first frame is drawn while the window is created">
          <p>
            On macOS a visible window draws its first frame during creation, before the window server reports it on
            screen. Launcher windows and panels, which start hidden, are left out.
          </p>
        </SectionHead>
        <div className="prose">
          <p>
            There is nothing to turn on. Before this, a window opened empty and drew on a later display-link tick; now
            content reaches the screen about 30 ms sooner on an idle Mac, and about 150 ms sooner on a busy one, where
            the window server is slow to say a new window is visible.
          </p>
          <p>
            It has one consequence for the app. The first view build and layout now run inside window creation, right
            after the window-created callbacks (<code>App::on_window</code>, <code>WindowSpec::on_created</code>). The
            window can't appear until they return, so the state's initial value, the first <code>view</code> call and
            those callbacks should be cheap. Anything slow added there delays the whole window, not just its content.
          </p>
        </div>
      </section>

      <section className="doc-section">
        <SectionHead id="defer" title="Start everything else after the first frame">
          <p>
            The menu bar item, status item, HUDs, secondary windows, network requests and file indexing are all work
            the user doesn't need to see the first window. Start them from <code>on_ready</code>.
          </p>
        </SectionHead>
        <div className="prose">
          <p>
            <code>App::on_ready</code> (and <code>WindowSpec::on_ready</code>) runs on the event thread once per native
            window creation, after the first frame is ready. For a launcher it runs while the window is still hidden,
            once its cached frame has finished on the GPU, which is the place to register a global shortcut. A window
            that isn't on screen (covered, occluded, hidden or on another Space) doesn't wait to draw: it runs at the
            first frame skipped for that, or a second after the window opens. It isn't called by headless tooling. The callback gets a <code>WindowHandle</code>, not the state, so capture an{" "}
            <code>AppHandle</code> and hand the work to <code>update</code>, which runs on the UI thread with{" "}
            <code>&amp;mut S</code> and a <code>Tx</code>.
          </p>
        </div>
        <Sample>{launchSetup}</Sample>
        <div className="prose prose-after">
          <p>
            Rune has no separate "after first frame" hook for state today; this is how to do it. Updates run inside the
            event loop even while no window shows, so an update may do main-thread native work such as creating a
            status item. For work that doesn't need the main thread, spawn a thread from the update and report back
            with <code>AppHandle::update</code> or <code>AppHandle::wake</code> and <code>AppSpec::poll</code>.
          </p>
          <p>
            A launcher can wait for readiness. With <code>RUNE_READY_DIR</code> set, the process writes{" "}
            <code>$RUNE_READY_DIR/&lt;bundle id&gt;.&lt;pid&gt;</code> right after its first <code>on_ready</code>{" "}
            returns: one line of JSON with <code>pid</code>, <code>bundle_id</code>, <code>ready_ms</code> (since the
            process started) and <code>rune</code> (its version). It removes the file when it exits normally; a
            process killed outright leaves it, so treat one whose PID isn't running as stale.
          </p>
        </div>
      </section>

      <section className="doc-section">
        <SectionHead id="still" title="Keep the first second still">
          <p>
            Rune animates nothing in the first second after content shows, so every frame drawn then is the app's.
            The usual cause is state that arrives a few milliseconds after launch, such as a system setting or a
            first network answer, and animates in.
          </p>
        </SectionHead>
        <div className="prose">
          <p>
            Land that first answer with <code>tx.instant()</code>, or don't animate it. A settings row whose switch
            faded in over a placeholder, and reflowed the row, kept its window drawing at 120 Hz for 0.7 s: about 85
            frames in the first second instead of 4 to 8.
          </p>
          <p>
            It costs memory too. A window that draws back to back while it comes on screen makes macOS allocate an
            extra window-sized drawable and keep it; that row cost 14 MB of settled footprint (91 MB instead of 77).
            Rune now holds a new window to two drawables until it first rests, or for its first second, so launch
            frames that wait on the window server or a shader compile no longer cause it.
          </p>
        </div>
      </section>

      <section className="doc-section">
        <SectionHead id="idle" title="Draw nothing when idle">
          <p>
            A window at rest should present no frames. Rune stops its display link when nothing moves, so what keeps
            an idle window drawing is almost always the app's own timers, polls or compositor.
          </p>
        </SectionHead>
        <div className="prose">
          <ul>
            <li>
              <strong>Return false from <code>poll</code> when nothing changed.</strong> A due{" "}
              <code>wake_after</code> on a resting window runs <code>poll</code> first and builds, lays out and
              draws only if it returns true. A poll that always returns true draws a frame on every tick.
            </li>
            <li>
              <strong>A compositor reports <code>animating()</code> only while it moves.</strong> It defaults to false.
              A window with a still compositor rests and gates its timers on <code>poll</code> like any other; one
              that always reports true draws every display-link tick.
            </li>
          </ul>
          <p>
            A settings window that checked a system setting four times a second went from 77 presents per 20 s idle to
            none, its CPU from 2.45% to 0.85%, and its footprint from 77 MB to 70 MB.
          </p>
        </div>
      </section>

      <section className="doc-section">
        <SectionHead id="reuse" title="Reuse a hidden window">
          <p>
            Showing a window that already exists costs a show. Rebuilding it costs a launch: the native window, the
            surface, the first view and layout. An app that comes and goes, such as a launcher, a menu bar panel or a
            settings window, should hide its window and show it again.
          </p>
        </SectionHead>
        <div className="prose">
          <ul>
            <li>
              <code>App::keep_running</code> keeps the app, its state and its event loop alive when the window closes,
              and <code>windowless</code> starts with no window. On macOS, Windows and X11 closing hides the window and
              keeps its host; <code>AppHandle::open</code>, <code>show</code> and <code>activate</code> bring it back
              with the same state, settled on its first frame. Wayland has no hidden toplevel, so it destroys and
              recreates the window.
            </li>
            <li>
              <code>AppHandle::hide</code> hides it without running the close callback. <code>show</code> shows it
              without taking focus; <code>open</code> also restores a minimized window and activates the app.{" "}
              <code>WindowHandle::show</code>, <code>hide</code> and <code>set_visible</code> do the same from window
              callbacks.
            </li>
            <li>
              <code>WindowOptions::launcher_panel()</code> makes a hidden, retained, nonactivating panel on macOS. It
              prepares its first frame at alpha zero while hidden, keeps rendering changes into that cached frame, and
              reveals the cached pixels on show, so a re-summon draws nothing new.
            </li>
            <li>
              A hidden window runs updates, polls and timers as usual but skips view and layout, and the frame that
              shows it builds once with the latest state.
            </li>
            <li>
              Keep the GPU too. <code>release_gpu_when_closed</code> frees it for an app that idles for hours, but the
              next open then pays for a device again.
            </li>
          </ul>
        </div>
        <Sample>{launchHidden}</Sample>
      </section>

      <section className="doc-section">
        <SectionHead id="measure" title="Measure it">
          <p>
            Launch time moves with the machine's load and Metal's cache, so a single run says little. This is the
            method behind the numbers below.
          </p>
        </SectionHead>
        <div className="prose">
          <ul>
            <li>
              <strong>Launch like a user does.</strong> Build the app into a bundle with its own bundle ID and start it
              with <code>open</code>, from a script that takes the timestamp just before. Stop it between runs. Use a
              Mac with nothing else running, and a release or <code>perf</code> build.
            </li>
            <li>
              <strong>Interleave the builds.</strong> Alternate base and changed builds inside each round, 5 to 10
              rounds, and compare medians and the 10th to 90th percentile spread. Load drifts over minutes; interleaving
              puts it on both sides.
            </li>
            <li>
              <strong>Time two moments.</strong> The window is visible when the process's window shows up on screen,
              which you can poll for in the window list. Content is visible at its first presented frame, which you can
              timestamp in the app or read from a screen recording of the window's rectangle. They differ by the time the
              app takes to draw after the window exists, which is what this page reduces.
            </li>
            <li>
              <strong>Run cold and warm.</strong> A cold launch is the first after an install or update, when Metal
              compiles shaders. To get one, delete the app's Metal shader caches, which sit in the user cache directory
              (<code>getconf DARWIN_USER_CACHE_DIR</code>) under the bundle ID: <code>com.apple.metal/&lt;bundle
              id&gt;</code>, and <code>&lt;bundle id&gt;/com.apple.metal</code>,{" "}
              <code>&lt;bundle id&gt;/com.apple.metalfe</code> and <code>&lt;bundle id&gt;/com.apple.gpuarchiver</code>.
              A warm launch follows an unmeasured one that fills the cache.
            </li>
            <li>
              <strong>Read settled memory last.</strong> Record the process's physical footprint a few seconds after
              launch. The <code>procwatch</code> crate reads it and counts allocations. A change that moves work earlier
              can raise memory at the window and leave it equal once settled.
            </li>
            <li>
              <strong>Count frames in the first second.</strong> After content shows, a still app draws a handful. Many
              more means something animates at launch.
            </li>
            <li>
              <strong>Measure idle too.</strong> Leave the window shown and untouched for 20 s, and count redraws and
              presents, CPU, and settled RSS and footprint. Count <code>poll</code> calls and the ones that return true
              separately from frames, so you can tell whether the app or the toolkit asked for each frame.
            </li>
          </ul>
          <p>
            In the repo, <code>--native --trace run.jsonl</code> writes wall-clock submission timestamps per frame for a
            real window, <code>surface_stats</code> reports presents and failed drawable acquisitions, and the{" "}
            <code>frame-profile</code> feature times view, layout, text and paint phases in headless runs. Run the app
            with <code>RUNE_DISABLE_METALLIB=1</code> to see what the precompiled library saves on your machine.
          </p>
        </div>
      </section>

      <section className="doc-section">
        <SectionHead id="numbers" title="Measured">
          <p>
            Median milliseconds from <code>open</code> for a small app, on an Apple silicon laptop that was under heavy
            load, 5 interleaved rounds (10 for <code>Renderer::new</code>). The absolute times are high because the machine
            was busy. The differences are what matter.
          </p>
        </SectionHead>
        <div className="table-wrap">
          <table className="decisions">
            <thead>
              <tr>
                <th scope="col">Launch</th>
                <th scope="col">Before</th>
                <th scope="col">After</th>
                <th scope="col">Saved</th>
              </tr>
            </thead>
            <tbody>
              {launchNumbers.map(n => (
                <tr key={n.row}>
                  <th scope="row">{n.row}</th>
                  <td data-label="Before">{n.before} ms</td>
                  <td data-label="After">{n.after} ms</td>
                  <td data-label="Saved">{Number(n.before) - Number(n.after)} ms</td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
        <div className="prose prose-after">
          <p>
            The first three rows are the first-frame and GPU prewarm changes together, in a hello-world app; a settings
            window gave 308 ms cold and 291 ms warm. After the change, content lands 30 to 40 ms after the window; before,
            160 to 190 ms after. The precompiled shader helps most on a cold cache (the last row); a warm cache already
            holds Metal's result, so warm launches gain little from it. Settled memory was the same, within 1 MB, in
            every case.
          </p>
        </div>
      </section>

      <Pager prev={["/how-it-works", "How it works"]} next={["/fast-apps", "Fast apps"]} />
    </>
  )
}

export function PrinciplesView() {
  return (
    <>
      <PageHead eyebrow={<><span>Principles</span><span>{principles.length}</span></>} title="Principles">
        <p>
          What Rune optimises for, most important first. Where it can, the framework enforces each one so that apps get
          it by default.
        </p>
      </PageHead>
      <div className="rules">
        {principles.map((p, i) => (
          <Rule key={p.slug} entry={p} number={num(i)} />
        ))}
      </div>
      <Pager prev={["/architecture", "Architecture"]} next={["/roadmap", "Roadmap"]} />
    </>
  )
}

export function ArchitectureView() {
  return (
    <>
      <PageHead eyebrow={<span>Architecture</span>} title="How Rune is built">
        <p>
          The design splits a Rune app across two threads. The app side turns state into a keyed tree and lays it out
          at its target state. The render thread owns what's on screen and animates toward those targets. Everything
          between them goes through one command channel.
        </p>
        <p>
          This page is the target from <a href={`${REPO}/blob/dev/PLAN.md`}>PLAN.md</a>. Today both sides run on one
          thread, in the order shown in <a href={withBase("/how-it-works#frame")}>How it works</a>.
        </p>
      </PageHead>

      <section className="doc-section">
        <SectionHead id="overview" title="Overview" />
        <ArchitectureDiagram />
      </section>

      <section className="doc-section">
        <SectionHead id="channel" title="The command channel" />
        <div className="prose">
          <p>
            The channel carries tree diffs, target layout rects, shaped text runs and spring targets to the render
            thread, and hits, gesture results, settles and presented-frame ids back. It's a real, serialisable protocol
            rather than shared memory, and three things follow from that.
          </p>
          <ul>
            <li>
              <strong>Animations survive a busy app.</strong> When a value animates, the app side does no view re-run,
              layout or diff.
            </li>
            <li>
              <strong>Hot reload reaches the iPhone.</strong> In development the channel runs over USB, with the render
              thread on the phone and the app side on the Mac, where it can be hot-patched.
            </li>
            <li>
              <strong>Agents and tests read the same stream.</strong> Recording the channel gives deterministic replay,
              frame traces and an inspector.
            </li>
          </ul>
        </div>
      </section>

      <section className="doc-section">
        <SectionHead id="layers" title="Layers" aside={`01–${num(layers.length - 1)}`} />
        <div className="rules">
          {layers.map((l, i) => (
            <Rule key={l.slug} entry={l} number={num(i)}>
              {l.slug === "tree" && <Code id="toggle-only" />}
            </Rule>
          ))}
        </div>
      </section>

      <div id="choices">
      <section className="doc-section">
        <SectionHead id="decisions" title="Decisions">
          <p>What Rune chose for each layer, what it turned down, and why.</p>
        </SectionHead>
        <div className="table-wrap">
          <table className="decisions">
            <thead>
              <tr>
                <th scope="col">Area</th>
                <th scope="col">Chose</th>
                <th scope="col">Instead of</th>
                <th scope="col">Why</th>
              </tr>
            </thead>
            <tbody>
              {decisions.map(d => (
                <tr key={d.area}>
                  <th scope="row">{d.area}</th>
                  <td data-label="Chose">{d.chose}</td>
                  <td className="rejected" data-label="Instead of">{d.rejected}</td>
                  <td>{d.why}</td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      </section>

      <section className="doc-section">
        <SectionHead id="gpui" title="Why not fork GPUI" />
        <div className="prose">
          <p>
            Zed's GPUI is the closest existing framework, and it has gained a wgpu backend, iOS and web shells, springs,
            AccessKit and hot reload. It still redraws the whole window on every dirty frame, has no layer compositing
            or group opacity, and runs animations by re-rendering views on the main thread. Those are architectural, and
            they're the reasons Kit's apps kept leaving it for Apple frameworks.
          </p>
          <p>
            So Rune starts fresh and borrows GPUI's best parts under its Apache-2.0 licence: the rope and sum tree, the
            line layout cache, the macOS and iOS text input code and the web textarea bridge.
          </p>
        </div>
      </section>
      </div>

      <Pager prev={["/fast-apps", "Fast apps"]} next={["/principles", "Principles"]} />
    </>
  )
}

const STATUS_LABEL: Record<Milestone["status"], string> = {
  done: "Done",
  now: "Now",
  started: "Started",
  next: "Next",
  later: "Later",
}

export function RoadmapView() {
  return (
    <>
      <PageHead eyebrow={<span>Roadmap</span>} title="Roadmap">
        <p>
          Rune ships one real surface at a time, and each one replaces the code it was built for. The order puts the
          three desktops together first, because one wgpu backend covers them.
        </p>
      </PageHead>

      <section className="doc-section">
        <SectionHead id="milestones" title="Milestones" />
        <ol className="index-list milestones">
          {milestones.map(m => (
            <li key={m.id} id={m.id} className="index-row">
              <span className="index-title">
                <span className="milestone-id">{m.id.toUpperCase().replace("-", ".")}</span>
                {m.title}
              </span>
              <div className="index-text">{m.body}</div>
              <span className="index-meta">
                <span className="status" data-status={m.status}>
                  {STATUS_LABEL[m.status]}
                </span>
              </span>
            </li>
          ))}
        </ol>
      </section>

      <section className="doc-section">
        <SectionHead id="risks" title="Risks" />
        <div className="rules">
          {risks.map(r => (
            <Rule key={r.slug} entry={r} />
          ))}
        </div>
      </section>

      <Pager prev={["/principles", "Principles"]} />
    </>
  )
}

export function NotFoundView() {
  return (
    <PageHead title="Not found">
      <p>
        That page doesn't exist. Go to the <a href={withBase("/")}>overview</a>.
      </p>
    </PageHead>
  )
}
