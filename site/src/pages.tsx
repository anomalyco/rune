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

function Pager({ prev, next }: { prev?: [string, string]; next?: [string, string] }) {
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
        Windows drawn entirely by Rune through winit and wgpu. The Settings window, a gallery of OpenCode's components
        and OpenCode Desktop's home and session pages (from a fixture, or read-only from a live server) run today.
      </>
    ),
    status: "works",
    meta: "examples/opencode",
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
    text: <>The same apps in a browser over WebGPU, starting with live demos on this site. There's no web build yet.</>,
    status: "planned",
    meta: "M5",
    href: "/roadmap#m5",
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
            code. The web and phones are planned, not built.
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
          <figure className="shot">
            <img src={withBase("/shots/opencode-session.png")} alt="OpenCode Desktop's session page: a project sidebar, a transcript with code, and the composer" width={1600} height={1025} loading="lazy" />
            <figcaption>examples/opencode, session page from the fixture</figcaption>
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

      <Pager prev={["/", "Overview"]} next={["/architecture", "Architecture"]} />
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

      <Pager prev={["/how-it-works", "How it works"]} next={["/principles", "Principles"]} />
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
