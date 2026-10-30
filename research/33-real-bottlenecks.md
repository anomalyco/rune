# 33. Real-app bottlenecks, checked against the engine plan

Reader: someone choosing the next frame-time work in `rune`, `rune-kit` or `rune-motion`, or
checking whether [32](32-engine-plan.md)'s ranked plan matches what real apps spend. Re-run the
harness below before and after a change that should move one of these numbers.

The question: in four real Rune apps, during the interactions people actually do, which costs
push frames over the 8.3 ms budget at 120 Hz, and does [32](32-engine-plan.md) cover them?

The short answer: the frames that miss the budget come from rebuilding too much, not from
Taffy or Parley. In every interaction measured here, shaping is 0% to 3.5% of the main
thread's busy time and Taffy 0% to 7.4%. The worst frames re-run a whole view and flatten its
tree after a small change: a keystroke, a step of a live resize, a scroll that mounts new rows,
or opening a card. In two apps the view runs every row's builder, Markdown included, because
the list is built in full and then virtualized. [32](32-engine-plan.md)'s flatten, `memo` and
motion items (#2, #3, #7, #8) hold up on real data, but its two largest projected savings don't
fire here: the resize fast path (#4) excludes any tree with a virtual list, which every measured
app has, and none of the measured interactions types into a long field, where the per-paragraph
layout (#1) would pay off. Three costs the plan doesn't mention: paint re-emits every visible
glyph each frame (21% to 37% of a scroll frame), a virtual window shift re-runs the whole view,
and a blocking read on the main thread looks like a fast frame unless wall time is logged
next to CPU time.

## How it was measured

- Rune `c83aa06b` (`dev`), with each app's `rune` dependencies patched to that tree and built
  at `--profile perf` with `rune/frame-profile`. Apps were left unchanged apart from bench-only
  seeding switches (a longer seeded conversation, a fixed reply to stream, a card to open).
- Headless and offscreen at scale 2, on a 12-core M2 Max worker that other jobs were also
  using (load average 6 to 20 during the runs). Each scenario ran once as warmup and then five
  more times, with scenario order alternating between runs. Numbers are medians across runs
  with outliers kept.
- `RUNE_FRAME_LOG=FILE` (new here) writes one JSON line per frame: wall and thread-CPU time,
  phase CPU (view, commit, layout, text, paint, apply), allocations, commits and texts
  shaped. A closing line records memory. `RUNE_HEADLESS_WAIT=SECS` (also new) holds a headless
  run before its first frame so a sampling profiler can attach in time to see launch.
- `scripts/real-frames.py` runs the scenarios, summarizes them per interaction window, and
  buckets a 1 kHz macOS `sample` of the main thread by self time.
- **What headless leaves out:** GPU execution, presentation, display timing and the window
  server. Headless also draws every frame. The tables count only frames a real window would
  draw: ones where input arrived, a commit ran or something animated.

Workloads, described generically:

| Workload | Data | Interactions |
|---|---|---|
| Agent client | A real 1,200-message transcript (22 MB), plus a 7.2k-character Markdown reply streamed in | launch, scroll (5 swipes), stream, type in the composer, live resize |
| Task board | A real board snapshot: 895 tasks, some cards with long notes, one with a Mermaid diagram | board scroll, open 12 cards in turn, scroll a long card, open and scroll the diagram card |
| Team chat | A seeded conversation repeated 40 times, one day apart | launch with arrivals, scroll, type, live resize |
| Notes reader | 601 synthetic notes, one of them 200 KB | launch, open a note, step the block cursor, search palette, scroll, live resize |

## Frame CPU per interaction

Thread-CPU ms per drawn frame, from the median run. "Over" is the number of frames above
8.3 ms out of `n`.

| Workload / interaction | n | p50 | p95 | p99 | max | over |
|---|---:|---:|---:|---:|---:|---:|
| Agent client / scroll | 544 | 1.16 | 3.93 | 5.66 | 11.7 | 2 |
| Agent client / stream | 1980 | 1.14 | 5.52 | 7.01 | 9.45 | 2 |
| Agent client / type | 96 | 3.44 | 5.51 | 6.50 | 6.50 | 0 |
| Agent client / resize | 266 | 4.87 | 8.59 | 9.62 | 10.6 | 18 |
| Task board / scroll | 178 | 1.93 | 2.47 | 3.33 | 9.90 | 1 |
| Task board / open cards | 324 | 3.21 | 25.5 | 43.1 | 47.4 | 106 |
| Task board / long card scroll | 361 | 5.47 | 31.1 | 32.9 | 33.9 | 82 |
| Task board / diagram card scroll | 435 | 3.59 | 17.2 | 18.9 | 22.0 | 60 |
| Team chat / scroll | 736 | 0.56 | 1.18 | 1.48 | 15.0 | 3 |
| Team chat / type | 659 | 0.51 | 12.1 | 22.7 | 24.9 | 92 |
| Team chat / resize | 336 | 18.4 | 39.7 | 42.8 | 44.6 | 241 |
| Notes reader / step cursor | 96 | 6.42 | 13.8 | 15.0 | 15.0 | 30 |
| Notes reader / palette search | 22 | 2.88 | 7.64 | 9.62 | 9.62 | 1 |
| Notes reader / scroll | 267 | 1.22 | 1.81 | 2.16 | 2.64 | 0 |
| Notes reader / resize | 242 | 7.71 | 13.7 | 15.3 | 15.5 | 102 |

An earlier chat resize run on the same machine measured p50 13.4 ms with 228 frames over.
Palette search's CPU looks fine, but its wall time doesn't: see #2 below.

Launch, headless (process start to first frame read back) and the first frame's CPU:

| Workload | Process to pixels | First frame CPU | What the first frame spends |
|---|---:|---:|---|
| Agent client | 298 to 347 ms | 28 to 33 ms | before it, 63 to 193 ms loading the transcript; layout 13 to 16, text 12 to 15 (102 texts shaped) |
| Task board | 201 to 293 ms | 86 to 172 ms | shaping 27 to 38, layout 13 to 47, view 20 to 44 |
| Team chat | 68 to 112 ms | 34 to 45 ms | view 26 (every row built), paint 9 |
| Notes reader | 59 to 98 ms | 13 to 26 ms | view 5, layout 3, text 2 |

## Where the main thread goes

Share of busy main-thread samples (waits left out), by the self-time owner. "paint" is the
per-frame draw-list walk (`Runtime::build`, `Phase::Paint`). "glyphs" is Parley's laid-out-glyph
iterators, almost all of it called from that walk. "rtext" is Rune's text code: `push_glyph`,
`text_hash`, Markdown building. "app" and "os" include app code: when an app function's own
frames sit in system calls, they land in "os".

| Interaction | paint | glyphs | rtext | view | flatten | taffy | shape | motion | alloc | app | os |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| Agent client / scroll | 13.1 | 25.3 | 10.0 | 8.5 | 1.5 | 1.1 | 2.1 | 1.8 | 4.6 | 2.5 | 18.3 |
| Agent client / stream | 10.4 | 8.9 | 8.7 | 21.6 | 11.0 | 2.9 | 2.1 | 5.0 | 8.6 | 3.3 | 11.3 |
| Agent client / type | 17.2 | 18.7 | 8.7 | 12.6 | 5.2 | 0.9 | 2.8 | 3.3 | 6.1 | 2.8 | 14.3 |
| Agent client / resize | 7.8 | 8.2 | 7.6 | 21.0 | 12.8 | 6.2 | 0.6 | 6.6 | 8.1 | 2.8 | 11.7 |
| Task board / scroll | 13.6 | 2.3 | 1.5 | 10.4 | 1.9 | 0.0 | 0.0 | 12.5 | 6.3 | 22.7 | 22.7 |
| Task board / open cards | 8.2 | 1.9 | 2.8 | 9.5 | 3.6 | 0.4 | 1.0 | 3.6 | 11.6 | 26.8 | 23.3 |
| Task board / long card | 13.3 | 0.9 | 3.2 | 19.2 | 5.5 | 0.6 | 0.7 | 8.8 | 9.6 | 16.3 | 17.4 |
| Team chat / scroll | 19.6 | 16.0 | 8.2 | 14.9 | 0.5 | 0.3 | 0.5 | 3.5 | 6.5 | 2.2 | 17.7 |
| Team chat / type | 8.9 | 5.9 | 11.3 | 19.2 | 2.6 | 0.2 | 0.2 | 2.5 | 11.5 | 12.6 | 21.4 |
| Team chat / resize | 1.6 | 1.0 | 13.3 | 21.3 | 4.0 | 1.6 | 0.0 | 1.2 | 14.6 | 17.8 | 22.6 |
| Notes reader / step cursor | 13.1 | 15.1 | 13.0 | 16.4 | 7.4 | 0.4 | 0.4 | 8.4 | 7.3 | 2.4 | 9.7 |
| Notes reader / scroll | 21.8 | 26.1 | 12.1 | 6.7 | 0.4 | 0.0 | 1.9 | 5.8 | 2.2 | 1.3 | 10.8 |
| Notes reader / resize | 7.5 | 4.6 | 16.8 | 18.5 | 7.5 | 7.4 | 0.4 | 8.5 | 10.2 | 3.8 | 10.7 |

Glyph rasterization and atlas work is at most 0.9% everywhere. Parley line breaking is at
most 3.4%.

## The ten biggest costs

Ranked by how often and how far each one takes frames past 8.3 ms. "Covered" refers to the
numbered items in [32](32-engine-plan.md) §4.

1. **Team chat: a keystroke or a resize step rebuilds every message row.** A keystroke
   commit takes 14.8 ms of CPU, 83% of it in view (12.3 ms). A resize step takes 18 to 20 ms
   (view 16.8 ms), and 228 to 241 of 336 resize frames miss the budget. The view builds every
   message, Markdown included, and then calls `El::virtualize`, which drops the offscreen rows
   in `populate_virtual_children`'s non-`RowSource` branch
   (`crates/rune/src/runtime/measure.rs:164-189`). The hot code is the app's row builder,
   `rune_kit::markdown` (`markdown.rs:1556-1580`) and `el::text` (`el.rs:970-1006`).
   *Not covered.* #4 excludes virtual lists, #8 helps only if the app memoizes rows, and
   nothing makes `virtualize` build lazily. The `virtual_rows` API already avoids this.
2. **Notes reader: each keystroke in the search palette blocks for 260 to 540 ms.** Mean wall
   time per update is 263 ms (p95 538 ms, worst 658 ms) while CPU stays at 3 ms. 90% of
   main-thread samples sit in `recvfrom`: the key handler sends a search request to a local
   daemon over a Unix socket and waits for the answer. This one cause accounts for the whole
   hitch. *Not covered*, since the cost is in the app. Rune has nothing that reports a frame
   whose wall time far exceeds its CPU time.
3. **Task board: opening a card or scrolling a long one re-runs the whole view.** Spike frames
   cost 18 ms (open) and 28 ms (long card). That is view 9 to 10 ms (36% to 50%),
   flatten and bookkeeping 3.6 to 6.5 ms, apply 1.3 to 4.2 ms and the app's poll 2 ms. 106 of
   324 and 82 of 361 frames miss. During a scroll, a virtual window or layout-read change sets
   off a full commit (`runtime.rs:2285-2303` → `commit` → `measure`). That commit rebuilds the
   card's note Markdown along with everything else. *Partly covered:* #2 (apply) and #3, #7
   and #8 (flatten). Nothing in the plan limits a window shift to building the newly mounted
   rows.
4. **Task board launch: the first frame takes 86 to 172 ms of CPU, and opening the diagram
   card 136 ms.** 27 to 38 ms of that is shaping text seen for the first time. Layout takes 13
   to 47 ms and view 20 to 44 ms over 1,350 to 4,800 layout nodes. This is a one-time spike,
   almost all of it first-use shaping and layout of everything mounted. *Partly covered:* #9
   (resolved-style cache, −1.5 to −2.5 ms) and #10 (virtualized Markdown). The real cost is
   5 to 9× the plan's 18 ms `md-open` baseline because the whole board mounts.
5. **Live resize of virtualized views re-runs view and flatten on every step.** Notes reader:
   7.6 ms per step (view 1.4, layout 2.1, text 0.5, apply 1.3, flatten about 1.2, paint 0.7),
   102 of 242 frames over. Agent client: p50 4.9 ms, 18 of 266 over. `text_hash` alone is 8% of
   the notes reader's resize (about 0.6 ms) and 4.4% of the agent client's; Taffy is 6% to 7%.
   *Covered in idea only.* #4's fast path would skip about 1.4 to 2 ms here, but its gate
   (`virtuals.is_empty()`) rejects all four apps. #5's projected saving holds: 0.2 to 0.6 ms.
6. **A small state change commits the whole tree.** Stepping the notes reader's block cursor
   costs 6.4 ms with 25k allocations, and 30 of 96 frames miss. An agent client keystroke
   costs 3.4 ms, and a streamed chunk 4.2 ms. The costs are in `flatten` (5% to 13%;
   `flatten.rs:222` moves each `Node` in, `:340` compares Taffy styles), `El::clone` of `memo`
   hits (2% to 7%, `expand_memo`), `apply` (3% to 6%, `runtime.rs:4449`), `Keyed::new` (2% to
   3%, `layout_read.rs:585`, a fresh map of every keyed node on each commit) and
   `capture_keeps` (2.8%, `sticky.rs:439`). *Mostly covered* by #3, #7 and #8 (flatten,
   clones) and #2 (apply). `Keyed::new` and `capture_keeps` are not.
7. **Paint re-emits every visible glyph every frame.** On scroll, paint takes 0.4 to 1.2 ms a
   frame. Parley glyph iteration plus `push_glyph` (`runtime.rs:6959-6994`, `text.rs:2153`,
   Parley `run.rs:335-359`) is 21% to 37% of a scroll frame, and the per-node walk
   (`runtime.rs:5734`) another 13% to 22%. That makes it the floor under every scroll and
   animation frame, and it grows with window size. *Not covered*, except #7's culling of
   offscreen subtrees. Onscreen text has no cached quads.
8. **Task board: the app's poll costs 1.0 to 1.8 ms every frame, 98% of the time with no
   change.** It makes 8k to 13k allocations a frame, half of a board-scroll frame. The cost is
   in the app's per-frame status checks. *Not covered* (app code).
9. **Motion upkeep: `Scene::present_dirty` and `Scene::phase`.** These take 2% to 12.5% of
   busy time, most on board scroll (about 0.24 ms a frame). Apply takes 4.2 ms on the long
   card's spike frames. *Covered* by #2. Its −2.3 ms holds only for the largest tree (the
   board's 4,800 nodes). The other apps' trees are smaller and would save 0.1 to 0.5 ms.
10. **First frames in the chat and agent client: 28 to 45 ms of CPU.** In the chat, the view
    takes 26 ms because every row is built (as in #1), and paint takes 9 ms rasterizing glyphs
    for the first time. In the agent client, layout takes 13 ms and text 12 ms for 102 texts.
    *Partly covered* by #9 and #10.

## Checking [32](32-engine-plan.md) against real data

| # | Plan item | Real data |
|---|---|---|
| 1 | Per-paragraph `TextLayout` | None of the measured interactions types into a long field: the agent client's composer is short and the notes reader has no editor. Shaping is ≤ 3.5% everywhere, so −14 ms doesn't show up here. Still worth doing for long-field editing, but it isn't what these apps hit today. |
| 2 | Lazy motion tracks, no-op dirty | Holds. Motion is 2% to 12.5%, and apply is 0.6 to 4.2 ms on commit frames. Saves 0.1 to 0.5 ms on small trees and more on the board. |
| 3 | `has_virtual` bit and path table | Holds. `node_path` 2.2% and memo deep clones 2% to 7% match root cause 1. |
| 4 | Resize fast path | Doesn't fire on any measured app: all four have a virtual list. Its saving (~1.4 to 2 ms a step) is real only if the gate admits virtual lists whose window doesn't change. |
| 5 | One `shape_key` hash | Holds: `text_hash` is 2% to 8%, so 0.2 to 0.6 ms. |
| 6 | In-place re-break | Not visible. `Layout::clone` isn't among the top functions of any profile. |
| 7 | Compact `BoxStyle`, `Cold<T>`, culling | Holds for flatten (`:222` Node moves and `:340` style compares are its top lines). Culling is the plan's only paint change. |
| 8 | Flattened `memo` reuse | The best fit for #3 and #6, but only where apps memoize. The chat app doesn't. |
| 9 | Resolved-style cache | Small next to the board's 27 to 38 ms first-frame shaping. |
| 10 | Virtualize Markdown | Applies to card notes and transcript messages at first mount, not in steady state. |
| 11 | Single-pass `aspect_fit` | Not seen. |
| 12 | Word shape cache | No trigger: shaping is ≤ 3.5% after launch. |

What the plan misses:

- **Lazy virtual lists.** `El::virtualize` accepts a fully built list. Rune could warn when a
  virtualized list builds far more rows than it mounts, or make lazy building the default
  path.
- **Window shifts that build only the new rows.** When a virtual window or a layout read
  moves, Rune re-runs the whole view (#3, and the agent client's roughly 59 commits per scroll run, at 3.9 ms each).
- **A glyph quad cache in paint.** Keep each text node's quads keyed by layout, scale and
  colour, and translate them when the node moves (#7).
- **Per-commit bookkeeping:** `layout_read::Keyed::new` and `sticky::capture_keeps` (#6).
- **Stall detection.** The frame log's wall-versus-CPU split is what exposed #2. A debug
  warning when an update's wall time is many times its CPU time would catch blocking I/O on
  the main thread.

## Memory

App heap, where the app counts it: task board 50 to 100 MB live heap, notes reader 9 to
21 MB. Malloc regions from `footprint`: agent client while streaming 177 MB, chat 86 MB.

Headless footprint numbers can't be compared across runs. The headless loop submits a frame
each time without polling the device, so GPU work and its buffers pile up while the run
renders frames faster than real time. `IOAccelerator (graphics)` reached 155 to 784 MB over
1,800 to 8,300 regions and grew with the number of frames rendered, not with the
interaction. An idle notes launch reached 967 MB after 3 s, with heap at 9 MB. That explains
why the same chat scenario measured 0.25 GB in one run and 1.6 GB in another. A real window
paces itself on the surface, so these numbers say nothing about real windows. Compare heap,
not footprint, until the headless loop polls the device every frame.

## Caveats

- The worker was shared. Wall times include preemption: the agent client's wall p95 is 15 to
  43 ms against a CPU p95 of 4 to 9 ms. The CPU numbers are the ones to compare.
- Headless profiling misses GPU and presentation costs. A HUD overlay app was left out
  because its costs are in overlay presentation, which headless doesn't model and which is
  being changed separately.
- The workloads' data is real, apart from the notes reader's synthetic fixture and the chat's
  repeated seed.

## Reproduce

Build an app at `--profile perf` with `rune/frame-profile`, write a scenario config (format in
`scripts/real-frames.py`'s docstring), then:

```
scripts/real-frames.py run app.json --out out/app --runs 5 --sample --sample-secs 90
scripts/real-frames.py summary app.json --out out/app --table
scripts/real-frames.py profile out/app/scroll.sample.txt --top 30
```
