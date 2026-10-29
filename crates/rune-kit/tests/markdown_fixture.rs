//! Golden element trees for a broad Markdown document: headings, wrapped paragraphs, lists,
//! task items, quotes, highlighted and plain code, a table, links, mermaid diagrams, and one
//! document still streaming in. Rendered headless with the OpenCode theme's fonts and tokens
//! in both appearances, then compared with `tests/golden/*.json`.
//!
//! Run: `cargo test -p rune-kit --features mermaid --test markdown_fixture`.
//! Accept an intended change by re-running with `RUNE_BLESS=1`, then review the golden diff.
//! On a mismatch the fresh tree is written next to the build output (path in the failure).
#![allow(clippy::disallowed_methods, clippy::expect_used, clippy::let_underscore_must_use, clippy::undocumented_unsafe_blocks, clippy::unwrap_used)]

use std::fmt::Write as _;
use std::path::PathBuf;

use rune::text::TextSystem;
use rune::{Cx, El, Env, Runtime, col};
use rune_kit::markdown::{Block, parse};
use rune_kit::syntax;
use rune_kit::{Markdown, MarkdownStyle};

const DOC: &str = r#"## What's happening

The server sends a heartbeat every **10 seconds**, but `stream.ts` only reconnects on an `error` event. When the machine sleeps, the TCP connection goes half-open: the *client* never sees an error, so it waits forever. See [the retry notes](https://example.com/retry) for the long version, which is deliberately wordy so that this paragraph has to wrap onto several lines at every width.

Two smaller problems make it worse:

- `backoff.ts` resets its delay on *every* message, so a flapping server is retried at full speed.
- The sidebar store ignores `session.updated` events that arrive before the first list fetch.
  - A nested point with `code` in it.

### The fix

Treat silence as failure. If no event or heartbeat arrives for 25 seconds, close the stream and reconnect with backoff:

```ts
const QUIET_MS = 25_000

function watchdog(stream: EventStream, reconnect: () => void) {
  let timer = setTimeout(fire, QUIET_MS) // restarted on every message
  function fire() {
    stream.close()
    reconnect()
  }
  stream.on("message", () => {
    clearTimeout(timer)
    timer = setTimeout(fire, QUIET_MS)
  })
}
```

```
plain text fence, no language
  keeps its   spacing
```

> Quoted advice: the reset must wait for a healthy minute.
> It spans two lines.

1. **Watchdog**: closes and reopens the stream after 25 s of silence.
2. **Backoff**: the delay only resets after a minute of healthy traffic.
3. **Sidebar**: early events are queued until the list loads.

- [x] watchdog added
- [ ] reconnect test
- [ ] docs

| Case | Delay | Resets |
| :--- | ---: | :---: |
| flapping | 1s, 2s, 4s | no |
| healthy minute | 1s | yes |
| capped | 30s | no |

---

```mermaid
graph TD
  A[Event arrives] --> B{Quiet for 25 s?}
  B -- yes --> C[Close stream]
  C --> D[Reconnect with backoff]
  B -- no --> A
```

```mermaid
sequenceDiagram
  participant C as Client
  participant S as Server
  C->>S: open stream
  S-->>C: heartbeat
  Note over C: 25 s of silence
  C->>S: reconnect
```

```mermaid
graph LR
  this is not valid
```

Done. All 14 tests in `src/net` pass.
"#;

const STREAMING: &str = "I'll add a test that drives the backoff with a fake clock.\n\n- a flapping server that fails right after each message;\n- a stream that stays healthy for a minute\n\n| Case | Delay |\n| --- | --- |\n| flapping | 1s |\n| healthy | 1";

const STREAMING_FENCE: &str = "Here is the test:\n\n```ts\nimport { expect, test } from \"bun:test\"\n\ntest(\"flapping keeps backing off\", () => {\n  const clock = fakeClock()\n  const delays = [1, 2, 3].map(() => b.fail())";

struct S {
    src: &'static str,
    dark: bool,
    streaming: bool,
}

fn view(s: &S, _: &mut Cx<S>) -> El {
    let kit = rune_theme_opencode::kit(s.dark);
    col().w_full().padding(16.0).child(
        Markdown::new(s.src).style(MarkdownStyle::from_kit(kit)).streaming(s.streaming).build(kit),
    )
}

fn render(src: &'static str, dark: bool, streaming: bool, width: f32) -> String {
    let text = TextSystem::new(rune_theme_opencode::FONTS, rune_theme_opencode::FAMILIES);
    let mut rt = Runtime::with_text(S { src, dark, streaming }, view, (width, 900.0), Env::new(dark, 2.0), text);
    rt.frame(0.0);
    rt.frame(1.0);
    let mut out = String::from("[\n");
    let els = rt.elements();
    for (i, e) in els.iter().enumerate() {
        let r = e.rect;
        let _ = write!(out, "{{\"path\":{:?},\"role\":{:?},\"rect\":[{:.2},{:.2},{:.2},{:.2}],\"fill\":{:?}", e.path, e.role, r[0], r[1], r[2], r[3], e.fill.hex());
        if let Some(k) = &e.key { let _ = write!(out, ",\"key\":{k:?}"); }
        if let Some(t) = &e.text { let _ = write!(out, ",\"text\":{t:?}"); }
        if let Some(c) = e.text_color { let _ = write!(out, ",\"color\":{:?}", c.hex()); }
        if e.opacity != 1.0 { let _ = write!(out, ",\"opacity\":{:.3}", e.opacity); }
        out.push('}');
        out.push_str(if i + 1 < els.len() { ",\n" } else { "\n" });
    }
    out.push_str("]\n");
    out
}

fn check(name: &str, actual: String) {
    check_ext(name, "json", actual);
}

fn check_ext(name: &str, ext: &str, actual: String) {
    let golden = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/golden").join(format!("{name}.{ext}"));
    if std::env::var_os("RUNE_BLESS").is_some_and(|v| v != "0") {
        std::fs::create_dir_all(golden.parent().unwrap()).unwrap();
        std::fs::write(&golden, &actual).unwrap();
        return;
    }
    let want = std::fs::read_to_string(&golden).unwrap_or_else(|_| panic!("missing golden {name}; run with RUNE_BLESS=1"));
    if want != actual {
        let fresh = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("{name}.{ext}"));
        std::fs::write(&fresh, &actual).unwrap();
        let line = want.lines().zip(actual.lines()).position(|(a, b)| a != b).unwrap_or(want.lines().count().min(actual.lines().count()));
        panic!(
            "golden differs: {name} (first difference at line {})\n  golden: {}\n  fresh:  {}\n  accept with RUNE_BLESS=1 after review",
            line + 1,
            golden.display(),
            fresh.display(),
        );
    }
}

#[test]
fn session_dark() { check("session-dark", render(DOC, true, false, 720.0)); }

#[test]
fn session_light() { check("session-light", render(DOC, false, false, 720.0)); }

#[test]
fn narrow_dark() { check("narrow-dark", render(DOC, true, false, 340.0)); }

#[test]
fn narrow_light() { check("narrow-light", render(DOC, false, false, 340.0)); }

#[test]
fn streaming_table_dark() { check("streaming-table-dark", render(STREAMING, true, true, 720.0)); }

#[test]
fn streaming_table_light() { check("streaming-table-light", render(STREAMING, false, true, 720.0)); }

#[test]
fn streaming_fence_dark() { check("streaming-fence-dark", render(STREAMING_FENCE, true, true, 720.0)); }

#[test]
fn streaming_fence_light() { check("streaming-fence-light", render(STREAMING_FENCE, false, true, 720.0)); }

/// What the element tree can't show: each block's inline spans (emphasis, code, links) and the
/// built-in highlighter's tokens for the first code fence.
#[test]
fn parsed_blocks_and_highlights() {
    let blocks = parse(DOC);
    let mut out = format!("{blocks:#?}\n");
    if let Some(Block::Code(code)) = blocks.iter().find(|b| matches!(b, Block::Code(_))) {
        let _ = writeln!(out, "-- highlights (ts)");
        for (range, paint) in syntax::builtin(Some("ts"), code) {
            let _ = writeln!(out, "{range:?} {:?} {paint:?}", &code[range.clone()]);
        }
    }
    check_ext("blocks", "txt", out);
}
