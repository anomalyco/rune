import { Suspense, useState, type ReactNode } from "react"
import { Wordmark } from "./brand/Wordmark"
import { Code } from "./Code"
import { DOCS, REPO } from "./meta"
import { Picture } from "./Picture"
import { LivePanel } from "./sections/LivePanel"
import { Pipeline } from "./sections/Pipeline"
import { SpringDemo } from "./sections/SpringDemo"
import { withBase } from "./base"

const CARGO = `rune = { git = "${REPO}", rev = "${__RUNE_REV__}" }`

export function App({ notFound = false }: { notFound?: boolean }) {
  return (
    <>
      <a className="skip" href="#main">
        Skip to content
      </a>
      <header className="topbar">
        <a className="brand" href={withBase("/")} aria-label="Rune home">
          <Wordmark />
        </a>
        <nav className="topnav" aria-label="Rune">
          <a href={DOCS}>Guide</a>
          <a href={REPO}>GitHub</a>
        </nav>
      </header>
      <main id="main">{notFound ? <NotFound /> : <Home />}</main>
      <footer className="footer">
        <span>Rune is open source under the MIT license.</span>
        <span>
          Made by <a href="https://anoma.ly">Anomaly</a>.
        </span>
      </footer>
    </>
  )
}

// Each section is its own Suspense boundary, so hydration yields between sections instead of taking one long task.
function Section({ id, title, children, wide = false }: { id: string; title: string; children: ReactNode; wide?: boolean }) {
  return (
    <Suspense>
      <section className={wide ? "section is-wide" : "section"} id={id} aria-labelledby={`${id}-h`} data-reveal>
        <h2 id={`${id}-h`}>{title}</h2>
        {children}
      </section>
    </Suspense>
  )
}

function Home() {
  return (
    <>
      <section className="hero" aria-labelledby="hero-h">
        <div className="hero-text">
          <div className="masthead">
            <span className="sr-only">Rune</span>
            <Wordmark periodic />
          </div>
          <h1 id="hero-h">A Rust UI framework that draws every frame itself, at 120&nbsp;Hz.</h1>
          <p className="lede">
            Rune brings its own GPU renderer, text, layout and springs. Views are plain Rust, a small app's first window
            opens in about 300&nbsp;ms, and the same code runs on macOS, Windows, Linux, iOS and the web.
          </p>
          <div className="actions">
            <a className="button is-primary" href={REPO}>
              <GitHubMark />
              View on GitHub
            </a>
            <a className="button" href={DOCS}>
              Read the guide
            </a>
          </div>
        </div>
        <LivePanel />
      </section>

      <Section id="views" title="A view is a plain Rust function of your state.">
        <p className="intro">
          There is no markup and no macro language. <code>view</code> reads the state and returns elements; a handler changes
          the state and Rune draws what follows. Give an element <code>.motion()</code> and it moves to its new layout on a
          spring.
        </p>
        <div className="split">
          <Code id="hello" />
          <figure className="shot">
            <Picture name="hello" alt="A dark window reading “Lights on” above a blue switch that is on" sizes="320px" />
            <figcaption>
              What it draws, rendered headless with <code>--snap</code>
            </figcaption>
          </figure>
        </div>
      </Section>

      <Section id="springs" title="Every animation is a spring that keeps its speed when you change your mind.">
        <p className="intro">
          Springs take SwiftUI's numbers, a duration and a bounce, and are solved exactly, so a retarget starts from where
          the value is and how fast it's going. Nothing animates on the first frame, a finger moves things directly, and
          reduced motion swaps travel for a short fade.
        </p>
        <SpringDemo />
        <Code id="retarget" />
      </Section>

      <Section id="frame" title="Every frame takes the same short path, from your input to one draw call." wide>
        <p className="intro">
          Input changes your state. Rune calls your view, diffs it by key against the last one, lays it out with Taffy, moves
          what changed on springs and draws the whole window as one batch of instanced quads. At 120&nbsp;Hz all of it has
          8.33&nbsp;ms, and an idle window draws nothing.
        </p>
        <Pipeline />
      </Section>

      <Section id="renderer" title="Metal, glass and light are drawn by the same renderer." wide>
        <p className="intro">They're GPU shaders, drawn live. These stills come straight out of Rune.</p>
        <div className="gallery">
          <Shot name="metal" alt="A ball of liquid chrome on black, reflecting a cool studio light" caption="Liquid metal" sizes="(max-width: 760px) calc(100vw - 48px), 740px" hero />
          <Shot name="glass" alt="A frosted glass tile over a blue disc, sharp beside the glass and soft through it" caption="Frosted glass" />
          <Shot name="light" alt="Nine raised keys under one soft light, the middle one lit blue" caption="Raised keys under one light" />
        </div>
      </Section>

      <Section id="opencode" title="OpenCode's desktop app is the test Rune is built against." wide>
        <p className="intro">
          Rune's first big job is rebuilding OpenCode's desktop app and raising its bar: long streaming sessions, markdown
          and code, window chrome and scroll momentum. These screens are drawn by Rune.
        </p>
        <div className="screens">
          <figure className="screen is-main">
            <Picture name="opencode-session" alt="OpenCode drawn by Rune: a session with a sidebar of projects and a conversation with code" sizes="(max-width: 1168px) calc(100vw - 48px), 1120px" />
          </figure>
          <figure className="screen">
            <Picture name="opencode-home" alt="OpenCode drawn by Rune: the home screen with a prompt field" sizes="(max-width: 760px) calc(100vw - 48px), 548px" />
          </figure>
          <figure className="screen">
            <Picture name="opencode-streaming" alt="OpenCode drawn by Rune: a reply streaming in" sizes="(max-width: 760px) calc(100vw - 48px), 548px" />
          </figure>
        </div>
      </Section>

      <Section id="agents" title="An agent can build, run and check a Rune app without a screen.">
        <p className="intro">
          Tests drive the real runtime: tap an element by key, step the clock, read where things are. Any app renders headless
          to a PNG with <code>--snap</code>, to a strip of frames with <code>--frames</code>, and prints its element tree with{" "}
          <code>--tree</code>.
        </p>
        <Code id="hello-test" />
        <ul className="facts">
          <li>
            <strong>A reference renderer.</strong> <code>rune-ref</code> draws the same scenes on the CPU, and golden tests
            hold the GPU within two levels per channel of it.
          </li>
          <li>
            <strong>A motion audit.</strong> Example apps run scripted scenarios headless at 120&nbsp;fps, and a checker flags
            snaps, pops, jumps and retargets that lose their speed.
          </li>
          <li>
            <strong>Over 2,600 tests</strong> across the framework, its components and its motion.
          </li>
        </ul>
      </Section>

      <Section id="platforms" title="The same code runs on macOS, Windows, Linux, iOS and the web." wide>
        <ul className="platforms">
          <Platform name="macOS" note="Rune's own Metal backend" />
          <Platform name="Windows" note="wgpu; windows stay hidden until their first frame" />
          <Platform name="Linux" note="wgpu on X11 and Wayland" />
          <Platform name="iOS" note="Embedded in UIKit or SwiftUI" />
          <Platform name="Web" note="WebGPU on one canvas, experimental" />
        </ul>
      </Section>

      <section className="cta" aria-labelledby="cta-h" data-reveal>
        <h2 id="cta-h">Pin a commit and start with one view.</h2>
        <p className="intro">
          Rune isn't on crates.io yet. Add it from Git at a commit, then start from <code>examples/hello</code>.
        </p>
        <CopyLine text={CARGO} />
        <div className="actions">
          <a className="button is-primary" href={DOCS}>
            Read the guide
          </a>
          <a className="button" href={`${REPO}/tree/dev/examples`}>
            Browse the examples
          </a>
        </div>
      </section>
    </>
  )
}

/** A gallery tile. The `hero` tile spans two rows beside the others. */
function Shot({
  name,
  alt,
  caption,
  sizes = "(max-width: 760px) calc(100vw - 48px), 360px",
  hero = false,
}: { name: Parameters<typeof Picture>[0]["name"]; alt: string; caption: string; sizes?: string; hero?: boolean }) {
  return (
    <figure className={hero ? "shot is-hero" : "shot"}>
      <div className="shot-frame">
        <Picture name={name} alt={alt} sizes={sizes} />
      </div>
      <figcaption>{caption}</figcaption>
    </figure>
  )
}

function Platform({ name, note }: { name: string; note: string }) {
  return (
    <li className="platform">
      <span className="platform-name">{name}</span>
      <span className="platform-note">{note}</span>
    </li>
  )
}

function CopyLine({ text }: { text: string }) {
  const [copied, setCopied] = useState(false)
  return (
    <div className="copyline">
      <pre className="code" tabIndex={0}>
        <code>
          <span className="tk-f">rune</span> <span className="tk-p">=</span> <span className="tk-p">{"{"}</span> git{" "}
          <span className="tk-p">=</span> <span className="tk-s">"{REPO}"</span>
          <span className="tk-p">,</span> rev <span className="tk-p">=</span> <span className="tk-s">"{__RUNE_REV__}"</span>{" "}
          <span className="tk-p">{"}"}</span>
        </code>
      </pre>
      <button
        type="button"
        className="copy"
        onClick={() =>
          void navigator.clipboard?.writeText(text).then(() => {
            setCopied(true)
            setTimeout(() => setCopied(false), 1600)
          })
        }
      >
        {copied ? "Copied" : "Copy"}
      </button>
    </div>
  )
}

function NotFound() {
  return (
    <section className="notfound">
      <h1>There's no page here.</h1>
      <p className="lede">
        Try <a href={withBase("/")}>Rune's home page</a> or the <a href={REPO}>code on GitHub</a>.
      </p>
    </section>
  )
}

function GitHubMark() {
  return (
    <svg viewBox="0 0 16 16" width="16" height="16" aria-hidden="true" focusable="false">
      <path
        fill="currentColor"
        d="M8 0C3.58 0 0 3.58 0 8c0 3.54 2.29 6.53 5.47 7.59.4.07.55-.17.55-.38 0-.19-.01-.82-.01-1.49-2.01.37-2.53-.49-2.69-.94-.09-.23-.48-.94-.82-1.13-.28-.15-.68-.52-.01-.53.63-.01 1.08.58 1.23.82.72 1.21 1.87.87 2.33.66.07-.52.28-.87.51-1.07-1.78-.2-3.64-.89-3.64-3.95 0-.87.31-1.59.82-2.15-.08-.2-.36-1.02.08-2.12 0 0 .67-.21 2.2.82.64-.18 1.32-.27 2-.27s1.36.09 2 .27c1.53-1.04 2.2-.82 2.2-.82.44 1.1.16 1.92.08 2.12.51.56.82 1.27.82 2.15 0 3.07-1.87 3.75-3.65 3.95.29.25.54.73.54 1.48 0 1.07-.01 1.93-.01 2.2 0 .21.15.46.55.38A8.01 8.01 0 0 0 16 8c0-4.42-3.58-8-8-8"
      />
    </svg>
  )
}
