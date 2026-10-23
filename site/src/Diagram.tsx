import type { CSSProperties, ReactNode } from "react"

/** The architecture in one figure: the app side and the render thread, the command channel between them, and the
 *  platform shell under both. Plain HTML so the text stays crisp and the layout can stack on a phone. */
export function ArchitectureDiagram() {
  return (
    <figure className="arch" aria-labelledby="arch-caption">
      <div className="arch-grid">
        <section className="arch-box" data-area="app">
          <p className="arch-kicker">Main thread</p>
          <h3>App side</h3>
          <ul>
            <li>Models: typed state, keyed by stable ids</li>
            <li>View functions → keyed tree → diff</li>
            <li>Taffy layout at the target state</li>
            <li>Parley shaping and line cache</li>
            <li>Handlers, queued between frames</li>
            <li>Accessibility tree updates</li>
          </ul>
        </section>

        <div className="arch-channel" data-area="channel">
          <p className="arch-channel-label">Command channel</p>
          <div className="arch-flow" data-dir="fwd">
            <span className="arch-arrow" aria-hidden="true" />
            <span className="arch-flow-text">tree diffs, target rects, text runs, spring targets</span>
          </div>
          <div className="arch-flow" data-dir="back">
            <span className="arch-arrow" aria-hidden="true" />
            <span className="arch-flow-text">hits, gestures, settles, frame ids</span>
          </div>
        </div>

        <section className="arch-box" data-area="render">
          <p className="arch-kicker">Paced by the display</p>
          <h3>Render thread</h3>
          <ul>
            <li>Presented tree: what's on screen now</li>
            <li>Spring tracks that retarget with velocity</li>
            <li>Exiting nodes, edge-by-edge layout motion</li>
            <li>Scroll physics, drags applied 1:1</li>
          </ul>
          <div className="arch-inner">
            <p className="arch-inner-title">Renderer · wgpu</p>
            <ul>
              <li>One instanced pass: rects, shadows, glyphs, vectors</li>
              <li>Soft-edge clip on every instance</li>
            </ul>
          </div>
        </section>

        <section className="arch-box arch-shell" data-area="shell">
          <div>
            <p className="arch-kicker">Per platform</p>
            <h3>Shell</h3>
            <p className="arch-line">Windows and panels, menus, clipboard, text input protocol, accessibility adapter, lifecycle</p>
          </div>
          <div data-optional>
            <p className="arch-kicker">Apple, optional</p>
            <h3>Native layers</h3>
            <p className="arch-line">System glass and video behind Rune's layer, masked by the render thread each frame</p>
          </div>
        </section>
      </div>
      <figcaption id="arch-caption">
        The app side states targets and the render thread animates toward them. Everything that crosses between them
        goes through the command channel.
      </figcaption>
    </figure>
  )
}

type Step = { name: string; does: string; at: string }

const everyFrameBefore: Step[] = [
  { name: "Input", does: "The window queues clicks, keys and scrolls; poll() reads outside data.", at: "window.rs" },
  { name: "Handlers", does: "Queued handlers run with &mut access to the app's state.", at: "runtime.rs" },
]
const onChange: Step[] = [
  { name: "View", does: "The view function returns a keyed El tree, matched to the last one by key.", at: "fn view(&S, &mut Cx<S>) -> El" },
  { name: "Layout", does: "Taffy lays the tree out at its target state; Parley shapes the text.", at: "Taffy, Parley" },
  { name: "Targets", does: "Each changed rect, colour or opacity retargets its spring.", at: "rune-motion Scene" },
]
const everyFrameAfter: Step[] = [
  { name: "Springs", does: "Every track is sampled at the frame's time; scroll and reveals step too.", at: "rune-motion" },
  { name: "Draw", does: "Rects, shadows and glyphs become instances in one wgpu pipeline.", at: "render.rs" },
]

function Steps({ steps, from }: { steps: Step[]; from: number }) {
  return steps.map((s, i) => (
    <li key={s.name} className="dg-step">
      <span className="dg-num">{String(from + i).padStart(2, "0")}</span>
      <span className="dg-name">{s.name}</span>
      <span className="dg-does">{s.does}</span>
      <code className="dg-at">{s.at}</code>
    </li>
  ))
}

/** One frame, top to bottom, as `Runtime::frame` and the window's redraw run it today. */
export function FrameDiagram() {
  return (
    <figure className="dg" aria-labelledby="frame-caption">
      <ol className="dg-flow">
        <Steps steps={everyFrameBefore} from={1} />
        <li className="dg-group">
          <p className="dg-group-label">Only when state changed</p>
          <ol className="dg-flow">
            <Steps steps={onChange} from={3} />
          </ol>
        </li>
        <Steps steps={everyFrameAfter} from={6} />
      </ol>
      <figcaption id="frame-caption">
        Steps 3–5 run only after a handler or <code>poll()</code> changes state. A frame that is only animating runs
        1, 2, 6 and 7, and a settled window doesn't draw at all.
      </figcaption>
    </figure>
  )
}

/** Who runs what: one thread today, and the split PLAN.md §3 describes. */
export function ThreadDiagram() {
  return (
    <figure className="dg" aria-labelledby="threads-caption">
      <div className="dg-pair">
        <section className="dg-panel">
          <p className="dg-kicker">Today</p>
          <div className="dg-box">
            <h3>Main thread, winit event loop</h3>
            <ul>
              <li>Input and handlers</li>
              <li>View, Taffy and Parley, on a change</li>
              <li>Springs: the Runtime's Scene, sampled each frame</li>
              <li>wgpu encode and present</li>
            </ul>
          </div>
          <div className="dg-feeds">
            <div className="dg-box dg-small">
              <h3>Your threads</h3>
              <p><code>Live&lt;T&gt;::set</code> writes an atomic and wakes the window, without re-running the view</p>
            </div>
            <div className="dg-box dg-small">
              <h3>rune-images</h3>
              <p>Decodes images, then wakes the window</p>
            </div>
          </div>
        </section>
        <section className="dg-panel" data-planned>
          <p className="dg-kicker">Planned</p>
          <div className="dg-box">
            <h3>App side, main thread</h3>
            <ul>
              <li>Handlers, view, diff</li>
              <li>Taffy and Parley</li>
            </ul>
          </div>
          <p className="dg-link">
            <span className="dg-arrow" aria-hidden="true" />
            Command channel: diffs, rects, spring targets down; hits and settles back
          </p>
          <div className="dg-box">
            <h3>Render thread, paced by the display</h3>
            <ul>
              <li>Owns the springs, scroll and drags</li>
              <li>wgpu encode and present</li>
            </ul>
          </div>
        </section>
      </div>
      <figcaption id="threads-caption">
        Today one thread runs every step. Animation still skips app code, because the view only re-runs when state
        changes. The plan moves springs and drawing to a render thread, so a slow handler can't stall an animation
        that's already moving.
      </figcaption>
    </figure>
  )
}

type Crate = { name: string; does: string }

const crateRows: { label: string; crates: Crate[] }[] = [
  {
    label: "Apps",
    crates: [{ name: "examples/*", does: "settings, opencode, kit-gallery, live-values, shapes and others" }],
  },
  {
    label: "Components",
    crates: [
      { name: "rune-kit", does: "Button, Switch, Select, Menu, TextField…, reading only semantic tokens" },
      { name: "themes/opencode", does: "OpenCode's tokens, fonts and icons as a rune-kit theme" },
    ],
  },
  {
    label: "Framework",
    crates: [{ name: "rune", does: "El tree, runtime, layout, text, wgpu renderer, window, headless tooling" }],
  },
  {
    label: "Foundations",
    crates: [
      { name: "rune-motion", does: "Springs, tracks, presence; no dependencies" },
      { name: "wgpu · winit · Taffy · Parley", does: "GPU, windows, layout, text" },
    ],
  },
]

const crateSide: Crate[] = [
  { name: "rune-ref", does: "CPU reference renderer and golden hashes" },
  { name: "rune-fx", does: "Effect layers: glow, fire, motion blur" },
  { name: "rune-light", does: "Lighting: soft shadows, god rays, GI" },
]

/** The workspace: each row builds on the rows below it; the side column is optional. */
export function CrateMap() {
  return (
    <figure className="dg" aria-labelledby="crates-caption">
      <div className="dg-crates">
        <ol className="dg-layers">
          {crateRows.map(row => (
            <li key={row.label} className="dg-layer">
              <p className="dg-kicker">{row.label}</p>
              <div className="dg-layer-row">
                {row.crates.map(c => (
                  <div key={c.name} className="dg-box dg-small">
                    <h3><code>{c.name}</code></h3>
                    <p>{c.does}</p>
                  </div>
                ))}
              </div>
            </li>
          ))}
        </ol>
        <aside className="dg-side">
          <p className="dg-kicker">Beside the core</p>
          {crateSide.map(c => (
            <div key={c.name} className="dg-box dg-small">
              <h3><code>{c.name}</code></h3>
              <p>{c.does}</p>
            </div>
          ))}
        </aside>
      </div>
      <figcaption id="crates-caption">
        Each row depends only on rows below it. Apps never need the crates beside the core.
      </figcaption>
    </figure>
  )
}

/** How a test drives the same runtime the window uses, and what it can check. */
export function TestDiagram() {
  return (
    <figure className="dg" aria-labelledby="tests-caption">
      <div className="dg-test">
        <div className="dg-box dg-small" data-area="in">
          <h3>Start</h3>
          <p>A state, often a named scene, and an optional <code>--script</code> of taps at exact times</p>
        </div>
        <span className="dg-arrow" data-area="a1" aria-hidden="true" />
        <div className="dg-box dg-small" data-area="rt">
          <h3><code>Runtime</code></h3>
          <p>The window's runtime, stepped by <code>frame(dt)</code> on a virtual clock, with no window</p>
        </div>
        <span className="dg-arrow" data-area="a2" aria-hidden="true" />
        <ul className="dg-outs" data-area="out">
          <li className="dg-box dg-small">
            <h3><code>elements()</code></h3>
            <p>Rects, text and opacity for <code>assert!</code> in <code>cargo test</code>, no GPU</p>
          </li>
          <li className="dg-box dg-small">
            <h3><code>--snap</code>, <code>--frames</code></h3>
            <p>Offscreen wgpu to PNG, compared byte for byte with pinned images</p>
          </li>
          <li className="dg-box dg-small">
            <h3><code>--trace</code></h3>
            <p>Per-frame rects and opacity as JSONL; motion-lint fails pops, jumps and jitter</p>
          </li>
        </ul>
      </div>
      <figcaption id="tests-caption">
        Tests and agents run the same code path as the window, minus the window. Time only moves when the test steps it,
        so a run repeats exactly.
      </figcaption>
    </figure>
  )
}

type Link = { name: ReactNode; does: ReactNode }

/** Boxes left to right with an arrow between each (top to bottom on a phone), and an optional line back from the end
 *  to the start for a loop. */
function Chain({ id, links, back, caption }: { id: string; links: Link[]; back?: ReactNode; caption: ReactNode }) {
  return (
    <figure className="dg" aria-labelledby={`${id}-caption`}>
      <ol className="dg-chain" style={{ "--links": links.length } as CSSProperties}>
        {links.map((l, i) => (
          <li key={i} className="dg-box dg-small">
            <h3>{l.name}</h3>
            <p>{l.does}</p>
          </li>
        ))}
      </ol>
      {back && (
        <p className="dg-back">
          <span className="dg-back-line" aria-hidden="true" />
          <span className="dg-back-label">{back}</span>
        </p>
      )}
      <figcaption id={`${id}-caption`}>{caption}</figcaption>
    </figure>
  )
}

/** From the code you write to pixels, for one frame. */
export function PipelineDiagram() {
  return (
    <Chain
      id="pipeline"
      links={[
        { name: "Your state", does: <>A plain struct, changed only by handlers with <code>&amp;mut</code> access</> },
        { name: <code>view()</code>, does: <>Returns a keyed <code>El</code> tree: what should be on screen</> },
        { name: "Layout", does: "Taffy places every node at its target; Parley shapes the text" },
        { name: "Springs", does: "Each moved rect, colour or opacity animates from where it is now" },
        { name: "GPU", does: "Rects, shadows and glyphs as instances, usually one wgpu draw call" },
      ]}
      back="A tap runs a handler, the state changes, and the view runs again"
      caption={
        <>
          Your code runs in the first two boxes. Rune does the rest, and while something animates it repeats only the
          last two, so your view isn't called again until the state changes.
        </>
      }
    />
  )
}

/** One state change, from the event to the spring targets. */
export function StateLoopDiagram() {
  return (
    <Chain
      id="state-loop"
      links={[
        { name: "Event", does: "A tap, key or poll() result is queued by the window" },
        { name: "Handler", does: <><code>|s: &amp;mut State|</code> runs between frames</> },
        { name: <code>view(&amp;State)</code>, does: "Builds the whole tree again from the new state" },
        { name: "Diff by key", does: "Nodes with the same key are the same node, so they move instead of popping" },
        { name: "Targets", does: <>Changed properties retarget their springs, using the node's <code>.motion()</code></> },
      ]}
      caption={
        <>
          A view never animates anything itself. It says where things are, and the diff turns the difference into
          spring targets.
        </>
      }
    />
  )
}
