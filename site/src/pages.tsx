import type { ReactNode } from "react"
import { decisions, layers, milestones, principles, risks, type Entry, type Milestone } from "./content"
import { Code } from "./Code"
import { ArchitectureDiagram, CrateMap, FrameDiagram, TestDiagram, ThreadDiagram } from "./Diagram"
import { withBase } from "./base"
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
        {entry.spec && (
          <dl className="rule-spec">
            {entry.spec.map(s => (
              <div key={s.label}>
                <dt>{s.label}</dt>
                <dd className={s.mono ? "mono" : undefined}>
                  {s.values.map((v, i) => (
                    <span key={i}>{v}</span>
                  ))}
                </dd>
              </div>
            ))}
          </dl>
        )}
      </div>
      {children}
    </article>
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

const running = [
  {
    title: "Settings window",
    text: "Toggles, a segmented control, a slider, and theme and accent changes that animate on springs.",
    meta: "examples/settings",
  },
  {
    title: "Component gallery",
    text: "One scene per @opencode/ui story, rendered on the OpenCode theme and diffed against the web version.",
    meta: "examples/kit-gallery",
  },
  {
    title: "OpenCode Desktop",
    text: "The home and session pages, from a fixture or read-only from a live OpenCode server.",
    meta: "examples/opencode",
  },
  {
    title: "Effects",
    text: "Glow, pulses, fire, distortion and motion blur as GPU layers on any element's shape.",
    meta: "crates/rune-fx",
  },
  {
    title: "Headless renders",
    text: "Every example renders offscreen to a PNG at a chosen moment, for goldens and for agents.",
    meta: "--snap",
  },
]

export function HomeView() {
  return (
    <>
      <PageHead eyebrow={<><span>Rust UI framework</span><span>Early</span></>} title="Rune">
        <p>
          Rune is a Rust UI framework with its own GPU renderer, text, layout, spring animation and thin shells for each
          platform. It aims to feel like SwiftUI on a 120&nbsp;Hz iPhone on every platform, and to be written and checked by
          coding agents.
        </p>
        <p>
          <a href={withBase("/how-it-works")}>How it works</a> follows one frame through the code and shows real
          examples. The code and the full plan are at <a href={REPO}>anomalyco/rune</a>.
        </p>
      </PageHead>

      <section className="home-section">
        <SectionHead id="status" title="Status" aside="October 2026">
          <p>
            Rune is early, and its APIs will change. Today it runs on macOS through winit and wgpu, and draws each frame
            in one draw call.
          </p>
        </SectionHead>
        <ul className="index-list">
          {running.map(r => (
            <li key={r.title} className="index-row">
              <span className="index-title">{r.title}</span>
              <p className="index-text">{r.text}</p>
              <span className="index-meta">{r.meta}</span>
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
