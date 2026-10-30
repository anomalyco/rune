import { withBase } from "./base"
import { PageHead, SectionHead } from "./system/Frame"
import { Pager, Sample } from "./pages"

const blockingBefore = `// Before: a socket round trip on every keystroke, on the main thread.
.on_input_change(|s: &mut Search, _| {
    s.hits = index.search(&s.query.text); // blocks the window for the whole round trip
})`

const blockingAfter = `// After: init builds it once; the handler asks, poll collects.
results: Latest::default().debounce(Duration::from_millis(60)),   // init
s.results.set_waker(waker);                                       // setup

.on_input_change(|s: &mut Search, _| {
    let q = s.query.text.clone();
    s.results.request(q, move |q| index.search(&q));              // runs on a worker
})
fn poll(s: &mut Search, _: f64, _: &mut Tx) -> bool {
    let Some(hits) = s.results.take() else { return false };       // newest answer only
    s.hits = hits;
    true
}`

const listBefore = `// Before: every message is built each commit, then virtualized.
col().key("rows").growth(GrowthSpec::default())
    .virtualize(72.0)
    .children(s.messages.iter().map(|m| message(m)))`

const listAfter = `// After: only the rows in the mounted window are built.
col().key("rows").growth(GrowthSpec::default()).virtual_rows(
    s.messages.len(),
    72.0,
    |s: &Chat, i| s.messages[i].id.as_str().into(),
    |s: &Chat, i| {
        let m = &s.messages[i];
        memo(&m.id, m.revision, || message(m))
    },
)`

const memoBefore = `// Before: the version is the whole document, hashed on every frame.
memo("reader", s.markdown.clone(), || render_markdown(&s.markdown))`

const memoAfter = `// After: an id and a counter the edit bumps.
memo("reader", (s.note_id, s.edits), || render_markdown(&s.markdown))`

const loopBefore = `// Before: asks for a frame every 16 ms for as long as the window is open.
fn poll(s: &mut App, _: f64, tx: &mut Tx) -> bool {
    tx.wake_after(Duration::from_millis(16));
    s.tick();
    true
}`

const loopAfter = `// After: the loop runs while something moves, then stops.
fn poll(s: &mut App, _: f64, tx: &mut Tx) -> bool {
    if !s.recording { return false }
    tx.wake_after(Duration::from_millis(16));
    s.tick();
    true
}`

const pollBefore = `// Before: always "changed", so every wake draws.
fn poll(s: &mut App, _: f64, _: &mut Tx) -> bool {
    s.reload_from_disk();
    true
}`

const pollAfter = `// After: cheap when nothing happened, true only on a change.
fn poll(s: &mut App, _: f64, _: &mut Tx) -> bool {
    let Some(next) = s.updates.take() else { return false };
    s.apply(next);
    true
}`

const rectBefore = `// Before: the view re-runs every frame the feed scrolls.
let height = cx.layout("feed").map(|r| r[3]);`

const rectAfter = `// After: it re-runs when the height changes.
let height = cx.layout_size("feed").map(|[_, h]| h);`

const liveBefore = `// Before: each scroll or zoom event re-runs the view.
.on_magnify(|s: &mut Canvas, e, _| {
    s.camera.zoom_about(e.x, e.y, 1.0 + e.delta);
    true
})`

const liveAfter = `// After: the handler moves Live values and skips the view.
.on_magnify(|s: &mut Canvas, e, tx| {
    s.camera.zoom_about(e.x, e.y, 1.0 + e.delta); // sets Live values only
    tx.live_only();
    true
})`

const cacheBefore = `// Before: one entry per size ever asked for.
thread_local! { static SIZES: RefCell<HashMap<String, [f32; 2]>> = Default::default(); }`

const cacheAfter = `// After: a bound in entries; evict when it is passed.
const MAX: usize = 256;
fn remember(key: String, size: [f32; 2]) {
    SIZES.with_borrow_mut(|m| {
        if m.len() >= MAX { m.clear() }
        m.insert(key, size);
    })
}`

function Pair({ before, after }: { before: string; after: string }) {
  return (
    <div className="code-stack">
      <Sample>{before}</Sample>
      <Sample>{after}</Sample>
    </div>
  )
}

export function FastAppsView() {
  return (
    <>
      <PageHead eyebrow={<span>Fast apps</span>} title="Fast apps">
        <p>
          A Rune app feels fast when its main thread is free, its first frame is small, its lists build only what is on
          screen and an idle window draws nothing. Each rule below is one way a slow app breaks it, with a before and
          after. Where Rune can see the mistake, debug builds print a warning once per call site on stderr;{" "}
          <code>RUNE_PERF_WARN=0</code> silences them, and release builds contain none of it.
        </p>
        <p>
          Getting the first window up fast is its own page: <a href={withBase("/launch-fast")}>Launch fast</a>, which
          has <code>rune::prewarm_gpu()</code> and the measured numbers.
        </p>
      </PageHead>

      <section className="doc-section">
        <SectionHead id="main-thread" title="The main thread never waits">
          <p>
            A socket, an HTTP request, a database query, a spawned process or a large file read does not belong in{" "}
            <code>view</code>, a handler, <code>poll</code> or <code>init</code>. The window stops for the whole wait.
          </p>
        </SectionHead>
        <div className="prose">
          <p>
            Debug builds warn when a handler, <code>poll</code> or <code>init</code> spends over 8 ms waiting rather
            than computing: "blocked on the main thread". Do the work on a thread and bring the answer back through{" "}
            <code>poll</code>. For a search box or any request where only the newest answer matters, use{" "}
            <code>rune::Latest</code>: it supersedes older requests, debounces, and wakes the window once per answer.
          </p>
        </div>
        <Pair before={blockingBefore} after={blockingAfter} />
      </section>

      <section className="doc-section">
        <SectionHead id="first-frame" title="The first frame comes first">
          <p>
            <code>init</code> builds what the first frame shows from memory or a small file. Everything else, such as
            status items, network and indexing, starts in <code>on_ready</code>, after the first frame. The details are
            on <a href={withBase("/launch-fast#defer")}>Launch fast</a>.
          </p>
        </SectionHead>
      </section>

      <section className="doc-section">
        <SectionHead id="lists" title="Long lists build only what is on screen">
          <p>
            <code>virtualize</code> mounts only the visible rows, but it takes rows you have already built, so a
            thousand-message list still builds a thousand messages on every commit.{" "}
            <code>virtual_rows</code> takes a count and a closure and builds only the mounted window, with row content
            built inside the closure.
          </p>
        </SectionHead>
        <div className="prose">
          <p>
            Debug builds warn when a <code>virtualize</code> list is handed over 200 rows and more than four times what
            it mounts, and name the call site. Use <code>virtualize</code> for short lists only. Handlers in a row find
            their item by key or id, never by index.
          </p>
        </div>
        <Pair before={listBefore} after={listAfter} />
      </section>

      <section className="doc-section">
        <SectionHead id="memo" title="Memo whole panels, keyed small">
          <p>
            <code>memo(id, version, build)</code> reuses a subtree until its version changes. A hit no longer deep
            clones the subtree, and layout and apply are reused for unchanged subtrees, so wrapping a whole panel pays
            off, not only the rows of a list.
          </p>
        </SectionHead>
        <div className="prose">
          <p>
            The version is hashed every frame, so keep it small: an id plus an edit counter that the edit bumps. Never
            key a memo by a large value such as a Markdown source. Hashing or comparing it costs what the memo saves.
            Anything the closure reads that can change belongs in the version, except <code>Cx::layout</code> reads,
            which Rune tracks.
          </p>
        </div>
        <Pair before={memoBefore} after={memoAfter} />
      </section>

      <section className="doc-section">
        <SectionHead id="idle" title="An idle window draws nothing">
          <p>
            <code>poll</code> returns true only when visible state changed, and a <code>wake_after</code> loop has an
            end condition. A wake on a resting window runs <code>poll</code> first and draws only if it returns true.
          </p>
        </SectionHead>
        <Pair before={loopBefore} after={loopAfter} />
        <div className="prose prose-after">
          <p>
            Check it with <code>procwatch</code>: an app with its window shown and untouched should sit near zero CPU.
          </p>
        </div>
      </section>

      <section className="doc-section">
        <SectionHead id="poll" title="poll is cheap, and honest">
          <p>
            <code>poll</code> runs on every frame the window is awake. On a frame where nothing happened it should cost
            microseconds and allocate nothing: drain a channel, compare a generation counter or an mtime. Never reload,
            parse or scan in it.
          </p>
        </SectionHead>
        <Pair before={pollBefore} after={pollAfter} />
      </section>

      <section className="doc-section">
        <SectionHead id="reads" title="Read the size, not the rect">
          <p>
            A view that calls <code>Cx::layout</code> re-runs when the rect moves past half a point, and inside a scroll
            view the rect moves every frame. If you use only the size, read <code>Cx::layout_size</code>; for anything
            else, <code>Cx::layout_with</code> re-runs the view only when your answer changes.
          </p>
        </SectionHead>
        <Pair before={rectBefore} after={rectAfter} />
      </section>

      <section className="doc-section">
        <SectionHead id="live-only" title="Scroll and zoom handlers skip the view">
          <p>
            A handler that only moves a <code>Live</code> value, such as a scroll position, a zoom or a transform, calls{" "}
            <code>Tx::live_only()</code>, so the frame doesn't commit state and re-run the view. Debug builds check for
            a handler that said <code>live_only</code> but changed what the view reads.
          </p>
        </SectionHead>
        <Pair before={liveBefore} after={liveAfter} />
      </section>

      <section className="doc-section">
        <SectionHead id="caches" title="Every cache has a bound">
          <p>
            A cache bounds its entries and its bytes, counting decoded pixels rather than encoded size, so a long
            session can't grow it without end. A <code>static</code> map that only ever inserts is a leak with a
            lookup. Evict by count, by age or by byte budget, and test the bound.
          </p>
        </SectionHead>
        <Pair before={cacheBefore} after={cacheAfter} />
      </section>

      <section className="doc-section">
        <SectionHead id="audit" title="Audit a repo in seconds">
          <p>
            <code>scripts/app-audit &lt;repo&gt;</code> greps an app's Rust files for the patterns above and prints one
            line per finding with <code>file:line</code>.
          </p>
        </SectionHead>
        <div className="prose">
          <ul>
            <li>
              <code>.virtualize(</code> next to a built <code>.children(</code>, <code>.child(</code> or push loop.
            </li>
            <li>
              <code>Command::new</code>, <code>UnixStream</code>, <code>TcpStream</code>,{" "}
              <code>reqwest::blocking</code>, <code>rusqlite</code> and <code>read_to_string</code> in a file that
              defines <code>view</code>, <code>poll</code>, <code>init</code> or a handler.
            </li>
            <li>
              A <code>wake_after</code> of 33 ms or less with no <code>if</code>, <code>match</code> or{" "}
              <code>return</code> just before it.
            </li>
            <li>
              <code>layout(..)</code> read only for <code>[2]</code> or <code>[3]</code>.
            </li>
            <li>
              A <code>static</code> map in a file with no <code>retain</code>, <code>remove</code> or{" "}
              <code>clear</code>.
            </li>
          </ul>
          <p>
            It is crude: a finding is a place to look, not a verdict. <code>--strict</code> exits 1 when there are
            findings. The warnings above catch more, while the app runs; the audit also covers code a run didn't reach.
          </p>
        </div>
      </section>

      <Pager prev={["/launch-fast", "Launch fast"]} next={["/architecture", "Architecture"]} />
    </>
  )
}
