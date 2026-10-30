//! Markdown.

use super::*;

const MARKDOWN: &str = "# Rune markdown\n\nParagraphs take **strong**, *emphasis*, `inline code` and [links](https://rune.dev). \
Links are styled now and become clickable with rich-text spans.\n\n## Lists\n\n- First item\n- Second item with `code`\n\n\
1. One\n2. Two\n\n### Quote and rule\n\n> Quoted text sits behind a rule.\n\n---\n\n#### A muted heading\n\n```rust\nfn main() {\n    println!(\"hello\");\n}\n```\n";

pub(super) fn markdown_basic(_: &G, k: &Kit, _: &mut Vec<El>) -> El {
    col().w(560.0).child(Markdown::new(MARKDOWN).on_link(|_: &mut G, _| {}).build(k))
}

/// Tables, diffs and diagrams: what an agent's answer about a code change looks like.
const WALKTHROUGH: &str = r#"## Retry with backoff

The client now retries failed requests with **exponential backoff**, capped at `max_delay`.

| Attempt | Delay | Jitter | Notes |
|:--------|------:|:------:|-------|
| 1 | 0 ms | none | first try |
| 2 | 200 ms | ±20% | after a `503` |
| 3 | 400 ms | ±20% | |
| 4 | 800 ms | ±20% | gives up after this one and reports the **last** error |

```diff
@@ -12,7 +12,9 @@ impl Client {
     pub async fn send(&self, req: Request) -> Result<Response> {
-        self.inner.send(req).await
+        retry(self.policy, || self.inner.send(req.clone()))
+            .await
+            .map_err(Error::from)
     }
```

```mermaid
flowchart LR
    A[Request] --> B{Status?}
    B -->|2xx| C([Done])
    B -->|503| D[Wait backoff]
    D --> E[(Retry budget)]
    E -.->|left| A
    E -->|spent| F[Report error]
```

```mermaid
sequenceDiagram
    autonumber
    actor U as User
    participant C as Client
    participant S as Server
    U->>+C: send(req)
    loop until success or budget spent
        C->>S: POST /items
        S--xC: 503 Unavailable
        Note right of S: overloaded
    end
    C-->>-U: Err(last error)
```
"#;

/// Subgraphs, every node shape, a left-to-right chart that fits, and an unsupported diagram.
const DIAGRAMS: &str = r#"```mermaid
graph TD
    subgraph client [Client]
        UI[Web app] --> Cache[(Local cache)]
    end
    subgraph edge [Edge]
        LB{{Load balancer}} --> W1[Worker 1] & W2[Worker 2]
    end
    UI ==> LB
    W1 --> DB[(Postgres)]
    W2 --> DB
    W2 -.-> Q>Queue]
```

```mermaid
flowchart LR
    a([Stadium]) --> b[[Subroutine]] --> c((Circle)) --o d{Diamond}
    d --x e[/Lean/] <--> f[/Trapezoid\]
```

| Narrow | Table |
|---|---|
| wraps | when its column runs out of room, like this long cell does |

```mermaid
pie title Pets
    "Dogs" : 386
    "Cats" : 85
```
"#;

pub(super) fn markdown_diagrams(_: &G, k: &Kit, _: &mut Vec<El>) -> El {
    col().w(520.0).child(Markdown::new(DIAGRAMS).build(k))
}

/// What the lenient parser reads: an unclosed `[/…` label is a plain rectangle, a line that doesn't
/// read is skipped; plus a class diagram and sequence `box` groups with a `rect` band.
const LENIENT: &str = r#"```mermaid
flowchart LR
    A[List + count] --> B[/items split view]
    B --> C[Details]
    C --> D[Pause / Resume / Delete]
    C -- --> D
```

```mermaid
flowchart LR
    a[setting.some_name] --> b[run_* + /go]
    c[(store -> row)] --> d[Item.Worker]
    d --> e[/api/v2/item]
```

```mermaid
classDiagram
    direction LR
    class Shape {
        <<trait>>
        +area() f32
    }
    class Circle {
        +radius f32
    }
    class Canvas {
        +shapes Vec~Shape~
        +draw()
    }
    Shape <|.. Circle
    Canvas o-- "many" Shape : holds
```

```mermaid
sequenceDiagram
    box Front
        actor U as User
        participant A as App
    end
    box Back
        participant S as Server
    end
    U->>A: open list
    rect rgb(80, 140, 220)
        A->>+S: fetch items
        S-->>-A: 20 items
    end
    A-->>U: show list
```
"#;

pub(super) fn markdown_lenient(_: &G, k: &Kit, _: &mut Vec<El>) -> El {
    let style = MarkdownStyle::from_kit(k).diagram_max_width(560.0);
    col().w(560.0).child(Markdown::new(LENIENT).style(style).build(k))
}

/// Every Mermaid v11 flowchart shape, four to a row.
const SHAPES: &str = r#"```mermaid
flowchart LR
    a1[rect] ~~~ a2(rounded) ~~~ a3([stadium]) ~~~ a4[[subproc]]
    b1[(cyl)] ~~~ b2((circle)) ~~~ b3(((dbl-circ))) ~~~ b4>odd]
    c1{diam} ~~~ c2{{hex}} ~~~ c3[/lean-r/] ~~~ c4[\lean-l\]
    d1[/trap-b\] ~~~ d2[\trap-t/] ~~~ d3@{ shape: bang, label: "bang" } ~~~ d4@{ shape: cloud, label: "cloud" }
    e1@{ shape: notch-rect, label: "card" } ~~~ e2@{ shape: hourglass, label: "collate" } ~~~ e3@{ shape: bolt, label: "com-link" } ~~~ e4@{ shape: delay, label: "delay" }
    f1@{ shape: brace, label: "comment" } ~~~ f2@{ shape: brace-r, label: "brace-r" } ~~~ f3@{ shape: braces, label: "braces" } ~~~ f4@{ shape: text, label: "text" }
    g1@{ shape: h-cyl, label: "h-cyl" } ~~~ g2@{ shape: lin-cyl, label: "lin-cyl" } ~~~ g3@{ shape: curv-trap, label: "display" } ~~~ g4@{ shape: div-rect, label: "div-rect" }
    h1@{ shape: doc, label: "doc" } ~~~ h2@{ shape: lin-doc, label: "lin-doc" } ~~~ h3@{ shape: docs, label: "docs" } ~~~ h4@{ shape: tag-doc, label: "tag-doc" }
    i1@{ shape: tri, label: "extract" } ~~~ i2@{ shape: flip-tri, label: "manual file" } ~~~ i3@{ shape: win-pane, label: "win-pane" } ~~~ i4@{ shape: lin-rect, label: "lin-rect" }
    j1@{ shape: notch-pent, label: "loop limit" } ~~~ j2@{ shape: sl-rect, label: "manual input" } ~~~ j3@{ shape: st-rect, label: "processes" } ~~~ j4@{ shape: flag, label: "paper tape" }
    k1@{ shape: bow-rect, label: "stored data" } ~~~ k2@{ shape: tag-rect, label: "tag-rect" } ~~~ k3@{ shape: sm-circ } ~~~ k4@{ shape: f-circ }
    l1@{ shape: fr-circ } ~~~ l2@{ shape: cross-circ } ~~~ l3@{ shape: fork } ~~~ l4@{ shape: icon, form: circle, label: "icon" }
```
"#;

pub(super) fn markdown_mermaid_shapes(_: &G, k: &Kit, _: &mut Vec<El>) -> El {
    let style = MarkdownStyle::from_kit(k).diagram_max_width(560.0);
    col().w(560.0).child(Markdown::new(SHAPES).style(style).build(k))
}

/// Sequence participant types, `create` and `destroy`, numbering from 10; and a state diagram with
/// notes, concurrent regions, a fork and join and a choice.
const FLOWS: &str = r#"```mermaid
sequenceDiagram
    autonumber 10
    participant UI@{ "type": "boundary" }
    participant Ctl@{ "type": "control" }
    participant Db@{ "type": "database" }
    participant Q@{ "type": "queue" }
    UI->>Ctl: submit
    Ctl->>Db: save
    create participant Job
    Ctl->>Job: start
    Job->>Q: enqueue
    destroy Job
    Job-->>Ctl: done
    Note over UI,Ctl: saved
```

```mermaid
stateDiagram-v2
    [*] --> Ready
    state Ready {
        [*] --> Idle
        Idle --> Typing : key
        --
        [*] --> Offline
        Offline --> Online : connect
    }
    Ready --> split
    state split <<fork>>
    split --> Save
    split --> Sync
    state merge <<join>>
    Save --> merge
    Sync --> merge
    merge --> ok
    state ok <<choice>>
    ok --> [*] : fine
    ok --> Ready : retry
    note right of Save : writes to disk
```
"#;

pub(super) fn markdown_mermaid_flows(_: &G, k: &Kit, _: &mut Vec<El>) -> El {
    let style = MarkdownStyle::from_kit(k).diagram_max_width(560.0);
    col().w(560.0).child(Markdown::new(FLOWS).style(style).build(k))
}

/// A table without an outline, with a dimmed header and figures that line up, and a diff whose
/// line text is tinted too.
const TABLE_STYLES: &str = "| Field | Compared |\n|---|---|\n| title | yes |\n| updatedAt | yes |\n| draft cursor | no |\n\n| Build | Files | Time | Size |\n|:--|--:|--:|--:|\n| debug | 1,204 | 41.8 s | 118.4 MB |\n| release | 1,204 | 71.0 s | 31.1 MB |\n| perf | 1,211 | 69.6 s | 33.7 MB |\n\n```diff\n@@ -3,3 +3,3 @@ fields:\n   title: compared\n-  draft: compared\n+  draft: ignored\n```\n";

pub(super) fn markdown_table_styles(_: &G, k: &Kit, _: &mut Vec<El>) -> El {
    let style = MarkdownStyle::from_kit(k)
        .table(k.c.border_muted, rune::Color::TRANSPARENT)
        .table_outline(None)
        .table_header_text(k.c.text_muted)
        .diff_text_tint(0.5);
    col().w(560.0).child(Markdown::new(TABLE_STYLES).style(style).build(k))
}

/// Fenced code in each language family the highlighter knows.
const CODE: &str = r#"```tsx
// Loads a user, then renders their card.
export async function load(id: string): Promise<User | null> {
  const res = await fetch(`/api/users/${id}`, { method: "GET" });
  return res.ok ? res.json() : null;
}
const Card = ({ user }: Props) => <div className="card">{user.name}</div>;
```

```rust
#[derive(Debug, Clone)]
pub struct Retry<'a> { name: &'a str, attempts: u32 }

fn backoff(n: u32) -> Duration {
    Duration::from_millis(200 * 2u64.pow(n)) // capped by the caller
}
```

```python
@dataclass
class Job:
    name: str
    tries: int = 3  # per run

    def label(self) -> str:
        return f"{self.name} ×{self.tries}" if self.tries > 1 else self.name
```

```go
func (s *Server) Serve(ctx context.Context) error {
	for job := range s.queue {
		if err := job.Run(ctx); err != nil {
			return fmt.Errorf("job %q: %w", job.Name, err)
		}
	}
	return nil
}
```

```sh
# Build, then ship the release.
FEATURES=markdown cargo build --release && echo "done in $SECONDS s"
if [ -n "$TAG" ]; then gh release create "$TAG" ./target/*.tar.gz; fi
```

```sql
SELECT u.id, count(*) AS posts
FROM users u JOIN posts p ON p.user_id = u.id
WHERE u.created_at > '2026-01-01' AND p.draft = false
GROUP BY u.id ORDER BY posts DESC LIMIT 10;
```

```yaml
name: ci
on: [push, pull_request]
jobs:
  test:
    runs-on: macos-15 # Apple silicon
    timeout-minutes: 30
    steps:
      - run: cargo test --workspace
```
"#;

pub(super) fn markdown_code(_: &G, k: &Kit, _: &mut Vec<El>) -> El {
    col().w(620.0).child(Markdown::new(CODE).build(k))
}

pub(super) fn markdown_walkthrough(_: &G, k: &Kit, _: &mut Vec<El>) -> El {
    col().w(680.0).child(Markdown::new(WALKTHROUGH).on_link(|_: &mut G, _| {}).build(k))
}

pub(super) fn markdown_streaming(s: &G, k: &Kit, _: &mut Vec<El>) -> El {
    let n: usize = s.picked("md-stream", "60").parse().unwrap_or(60);
    let end = MARKDOWN.char_indices().map(|(i, _)| i).nth(n).unwrap_or(MARKDOWN.len());
    col().w(560.0).gap(12.0)
        .child(
            Button::new("Stream more")
                .on_press(move |s: &mut G| {
                    s.pick.insert("md-stream".into(), (n + 40).to_string());
                })
                .build(k)
                .key("stream-more"),
        )
        .child(Markdown::new(&MARKDOWN[..end]).reveal(0.18).streaming(end < MARKDOWN.len()).build(k))
}
