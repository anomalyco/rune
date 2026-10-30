# 37. What to own next

Reader: someone deciding whether Rune should replace another dependency (Taffy, Parley, winit,
an image decoder) or put the next week into its own runtime. This note re-runs
[33](33-real-bottlenecks.md)'s four real-app workloads on current `dev` and ranks the parts
Rune could own by what they cost now.

The short answer: there's nothing left worth owning. The dependencies Rune could replace now
cost little: Taffy is 1–4% of busy time outside a live resize, and 0.06–0.09 ms per committing
frame in an agent client's chat. Shaping is at most 4.7%, and image decode and Markdown parsing
are near zero. What costs now is Rune's own runtime:

1. **Copying `Node`s in commit.** Memo reuse clones every reused node, each about 1.7 KB with
   its `Visual`, on every commit, and apply then drops the old copies. That is 15–20% of commit
   time in the agent client and most of the commit time that isn't view, layout, text or apply.
2. **Per-frame scans and ancestor walks** in the paint walk and frame step: `subtree_exiting`
   walks up to the root for each hit-testable node, and `highlight_target_in`, `Keyed::new` and
   `layout_of` scan every node. On the board's long card these cost 15–20% of busy time. A
   small spike removed most of it: long-card scroll p50 −17%.
3. **The paint walk itself** (`Runtime::build`). It is 25–62% of a scroll frame. Scroll frames
   are already 0.5–0.9 ms at p50, so making it cheaper saves power but doesn't fix missed frames.

The one interaction still missing the frame budget is scrolling the board's long card: 82
frames over 8.3 ms, with p95 15 ms. Item 2 is its first fix.

## How it was measured

- **Builds:** "Then" is the exact tree [33](33-real-bottlenecks.md) measured: Rune `c83aa06b`
  with each app's code from that day. "Now" is Rune `dev` at `506f5421`, which matches
  `29df6816` apart from three commits that only move files, with each app's current code.
  So "now" includes app-side changes as well. The biggest is the team chat's timeline, which
  now builds only its mounted rows; that explains much of its type and resize gains.
- **Rune changes in between:**
  - glyph quad replay (`f560da33`);
  - memo reuse of flattened slices and Taffy subtrees (`049a0b13`, `417da565`);
  - the resize fast path that now admits virtual lists (`d7cdefde`).
- **Machine and builds:** `--profile perf` with `rune/frame-profile`, headless at scale 2, on a
  12-core M2 Max shared with other jobs (load 5–13). Each side has its own target dir.
- **Runs:** each app ran 6 paired rounds after a warmup. Each round ran every scenario on both
  sides, alternating which side went first and the scenario order. Numbers are medians with
  outliers kept. A 1 kHz main-thread `sample` ran 3 times per scenario per side, 90 s each.
- **Then figures:** the "then" column is this run's own paired figures. They differ from
  [33](33-real-bottlenecks.md)'s because that run's machine was busier (load 6–20).
- **What headless leaves out:** GPU work, presentation, winit and window-server event delivery.
  A Metal backend is landing separately, so GPU submission is out of scope here.

## Frame CPU per interaction, then → now

Thread-CPU ms per drawn frame: p50 / p95 / p99 / max, then frames over 8.3 ms.

| Workload / interaction | Then | Now |
|---|---|---|
| Team chat / scroll | 0.55 / 0.96 / 1.15 / 16.5, 3 over | 0.46 / 0.75 / 0.98 / 3.21, 0 over |
| Team chat / type | 0.65 / 13.0 / 16.4 / 22.5, 84.5 over | 0.39 / 2.36 / 3.07 / 3.43, 0 over |
| Team chat / resize | 17.6 / 30.9 / 39.2 / 43.1, 239 over | 2.79 / 5.40 / 5.78 / 6.59, 0 over |
| Notes / open | 4.14 p50, 16.6 max, 2 over | 3.08 p50, 14.5 max, 2 over |
| Notes / cursor | 5.73 / 11.4 / 12.5, 23.5 over | 2.68 / 4.70 / 6.02, 0 over |
| Notes / scroll | 0.83 / 1.18 / 1.29 / 1.62 | 0.63 / 0.84 / 0.91 / 0.99 |
| Notes / resize | 6.67 / 9.81 / 11.5 / 12.8, 21 over | 1.80 / 3.33 / 4.69 / 6.10, 0 over |
| Agent client / scroll | 0.66 / 1.77 / 2.75 / 4.55 | 0.56 / 1.36 / 2.11 / 4.20 |
| Agent client / stream | 0.72 / 2.06 / 2.47 / 3.03 | 0.82 / 1.59 / 1.97 / 2.38 |
| Agent client / type | 1.53 / 1.77 / 2.15 / 2.15 | 0.88 / 1.15 / 1.27 / 1.40 |
| Agent client / resize | 2.01 / 2.68 / 3.08 / 3.73 | 1.23 / 1.72 / 1.91 / 2.10 |
| Board / scroll | 1.29 / 1.70 / 7.91 / 13.0, 2 over | 0.25 / 0.48 / 3.29 / 3.49, 0 over |
| Board / open cards | 1.83 / 11.1 / 17.7 / 30.1, 108 over | 0.39 / 0.71 / 3.66 / 14.7, 1 over |
| Board / long-card scroll | 3.33 / 21.5 / 23.3 / 24.3, 81.5 over | 2.34 / 15.2 / 19.6 / 20.4, 82 over |
| Board / diagram card, scroll | 2.41 / 12.4 / 13.7 / 14.7, 59 over | 0.91 / 4.38 / 5.45 / 6.67, 0 over |
| Board / diagram card, open | 98.3 max | 55.8 max |

The notes palette still blocks on its daemon socket: its mean update wall time went from
231 ms to 188 ms, while its CPU time is small. That one is app-side.

**An agent client's chat** was measured in a separate run on a bigger real transcript: `dev`
`29df6816`, headless release, medians of 3. The figures are CPU ms per active frame, before the
memo work → after.

| Interaction | p50 | p95 | Allocated per frame, p50 |
|---|---|---|---|
| Typing | 3.92 → 2.15 | 4.63 → 2.71 | 5,072 → 2,446 KB |
| New message | 3.91 → 2.79 | 4.57 → 3.53 | 5,811 → 3,744 KB |

## Launch, then → now

Headless, from process start to the first frame read back, and the first frame's CPU.

| Workload | Process to pixels | First frame CPU |
|---|---|---|
| Team chat | 65 → 50 ms | 31.1 → 17.5 ms |
| Notes | 59 → 75 ms | 16.1 → 16.2 ms |
| Agent client | 69 → 75 ms | 17.4 → 19.8 ms |
| Board | 168 → 111 ms | 67.8 → 25.1 ms |

The notes and agent client launches stay within run-to-run spread; the agent client's extra
time is in app state loading (24 → 27 ms).

## Where the main thread goes, then → now

These are inclusive shares of main-thread busy time: self samples whose stack contains the
owner. Owners nest (glyph emission is inside the paint walk), so a row doesn't sum. A share can
rise only because the frame around it got cheaper.

| Workload / interaction | Paint walk | Glyph emit | Flatten | Apply | Taffy | Shaping | Markdown | Alloc |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| Team chat / scroll | 52 → 48 | 8 → 11 | 1 → 0 | 1 → 1 | 1 → 1 | 1 → 1 | 3 → 0 | 6 → 6 |
| Team chat / type | 20 → 36 | 3 → 7 | 5 → 7 | 3 → 7 | 1 → 2 | 0 → 1 | 16 → 3 | 13 → 6 |
| Team chat / resize | 4 → 14 | 1 → 4 | 6 → 14 | 3 → 9 | 3 → 12 | 0 → 0 | 22 → 7 | 16 → 11 |
| Notes / scroll | 72 → 62 | 16 → 16 | 0 → 0 | 0 → 0 | 2 → 4 | 2 → 2 | 0 → 1 | 1 → 2 |
| Notes / cursor | 44 → 46 | 10 → 10 | 12 → 8 | 6 → 6 | 2 → 2 | 1 → 1 | 0 → 0 | 7 → 3 |
| Notes / resize | 20 → 29 | 4 → 7 | 13 → 0 | 13 → 15 | 26 → 25 | 1 → 1 | 0 → 0 | 8 → 3 |
| Agent client / scroll | 46 → 26 | 12 → 10 | 2 → 2 | 2 → 2 | 4 → 4 | 2 → 2 | 1 → 0 | 4 → 7 |
| Agent client / stream | 27 → 24 | 5 → 6 | 15 → 12 | 12 → 10 | 6 → 9 | 2 → 3 | 3 → 5 | 9 → 7 |
| Agent client / type | 46 → 30 | 10 → 8 | 7 → 4 | 4 → 5 | 4 → 4 | 2 → 3 | 0 → 0 | 5 → 8 |
| Agent client / resize | 20 → 19 | 3 → 7 | 19 → 14 | 9 → 9 | 11 → 14 | 1 → 1 | 0 → 0 | 11 → 7 |
| Board / open cards | 13 → 25 | 1 → 4 | 5 → 3 | 3 → 0 | 1 → 1 | 1 → 2 | 0 → 0 | 9 → 6 |
| Board / long-card scroll | 20 → 29 | 1 → 1 | 9 → 10 | 7 → 7 | 2 → 2 | 1 → 1 | 1 → 0 | 7 → 5 |
| Board / diagram card | 15 → 33 | 1 → 4 | 5 → 4 | 3 → 1 | 1 → 1 | 1 → 2 | 0 → 0 | 7 → 4 |

Parley's glyph iterators, which [33](33-real-bottlenecks.md) found at 15–26% of a scroll frame,
are now 0–1.4%. What glyph emission still costs is Rune's own replay
(`glyphs_in_band_cached`, `glyph_or_image`). Image decode and winit don't appear in any sample:
the workloads load no images, and headless runs bypass winit's event loop.

## The biggest costs now

| Cost | Where | Share |
|---|---|---|
| `Runtime::build`, the paint walk's own lines | every scroll | 13–21% self |
| `highlight_target_in`, a full node scan with a `format!` for each highlight | board long card, diagram card | 6–11% |
| `Scene::phase`, from `subtree_exiting` walking to the root for each hit-testable node | board long card | 5% (+ about 7% inlined into the paint walk) |
| Node copies in flatten: `memmove`, `Node::clone`, `Visual::clone` | agent client stream, type | 6% of busy time, about 60% of flatten |
| `window_tree`: virtual-row builders, `El` moves, a SipHash `HashSet`, linear path finds | agent client stream | 7% |
| `Keyed::new`, a per-frame map of every node's key | board | 3–4.5% |
| `glyphs_in_band_cached` | scroll | 4–9% |
| `Renderer::prepare`, `pack_fade` | scroll | 3–12% (GPU work, out of scope) |
| Taffy `compute_flexbox_layout` | resize | 4.6–5.5% self |
| `__recvfrom` on the daemon socket | notes | 3–7% (app-side) |

In the board runs, 1.7–5.5% goes to the frame log being echoed to stderr as JSON. That is the
harness's own overhead, not the app's.

## What commit spends, besides view, layout, text and apply

In the agent client's chat, about 1.2–1.7 ms of each commit is none of view, layout, text or
apply. Sampling the same workload here (agent client, type and stream) shows which functions
own it. The figures are shares of the samples under `Runtime::commit`:

| Owner | Type | Stream |
|---|---:|---:|
| Flatten: copying reused nodes (`try_reuse_memo` → `Node::clone`, `Vec<Node>::push`) | 14.6 | 14.0 |
| Flatten: the rest (`Style` comparison, path strings) | 7.9 | 9.9 |
| `window_tree`: building virtual rows (app code that the view timer misses), moving `El`s, a SipHash set of mounted keys, a linear search for the scroll node | 7.3 | 17.2 |
| Apply: dropping the previous commit's nodes | 2.4 | 4.2 |
| Layout reads, `measure_virtuals`, `release_kept_layouts`, place | 9.7 | 10.7 |
| *For scale:* view 15.9 / 14.0, Taffy 17.7 / 15.2, the rest of apply 24.4 / 14.9 | | |

Across commit, `memmove` is 17–20% of samples and allocation 7–9%. Most of that `memmove` is
`Node`s being moved or cloned: each is about 1.7 KB, around 100 fields plus a 71-field
`Visual`.

Taffy's share of samples is larger than the frame profile's layout p50 (0.07 ms here,
0.06–0.09 ms in the agent client's chat) because samples weight by time. The commits that
rewrap streamed text pay for layout; the median commit doesn't. Layout's mean is 0.13–0.16 ms.

## Verdicts

Upper bounds are the measured share a change could remove at most, as a percentage of
main-thread busy time unless marked otherwise.

| # | Candidate | Upper bound | Size | Risk | Verdict |
|---|---|---|---|---|---|
| 1 | (b) Smaller `Node`, starting with memo reuse moving nodes instead of cloning them | 15–20% of commit (0.35–0.6 ms per commit in the agent client's chat), plus paint-walk stride | Medium for the clone fix; large for a hot/cold split | Medium: apply's diffing and measure retries read the old nodes | **Own it**, in Rune. Stop cloning reused spans first, then split `Node` and `Visual` into hot and cold parts |
| 2 | (i) Per-frame scans and ancestor walks: `subtree_exiting`, `highlight_target_in`, `layout_of`, `Keyed::new`, `in_scope` | 15–20% on the board's long card, under 2% elsewhere | Small | Low | **Fix in Rune.** Spiked below |
| 3 | (i) `window_tree`: per-commit sets, string paths, linear finds; time row builders as view | 7–17% of commit | Small | Low | **Fix in Rune** |
| 4 | (i) Retained paint per subtree, translating cached spans on scroll | 25–62% of a scroll frame | Large | High: fades, clips, sticky headers, hover | **Later.** Scroll already fits the budget, so the gain is power |
| 5 | (c) Per-frame arena | Allocation is 2–11% of busy time, 7–9% of commit; an arena reaches only frame-lifetime allocations | Medium–large | Medium | **Don't bother.** Items 1 and 3 remove most of the churn |
| 6 | (d) Own flex and stack layout instead of Taffy | 1–4% outside resize; 8–10% self (≤ 25% with measure callbacks) on notes resize | Large | High | **Don't bother.** If resize needs it, skip clean subtrees within Taffy |
| 7 | (e) Own shaper and a shaped-word cache | ≤ 4.7% everywhere | Large | High | **Don't bother** |
| 8 | (f) Markdown parse and build in rune-kit | 0–7% now (was up to 22%) | Medium | Low | **Don't bother.** Memo already covers it |
| 9 | (a) Flat glyph runs for text paint | Landed (`f560da33`); Parley iteration 15–26% → 0–1.4% | Done | — | **Done** |
| 10 | (g) Own the macOS run loop and use CADisplayLink | Not visible headless; a windowed profile is needed | Large | High | **Don't own it for CPU.** Pacing or latency could justify it, but only with a windowed measurement |
| 11 | (h) Image decode | 0% in these workloads | — | — | **Don't bother** |

## Spike: per-frame scans (#2)

The spike made three changes on a scratch tree that won't land:

- One forward pass per frame marks the subtrees that are exiting, and `subtree_exiting` reads
  that mark during the frame step.
- A node reads its clock only when it has a loop track.
- `highlight_target_in` checks whether a node has highlights before testing scope, and matches
  keys without building a path string.

Both sides were built from the same tree, `dev` with each app's current code, with only the
patch differing. The board ran 6 paired rounds. Other jobs pushed the machine's load to 80–115
during the run, so absolute times are inflated. The spike was faster in every paired round of
every scenario.

| Board interaction | Base p50 | Spike p50 | Frames over 8.3 ms | Median paired ratio |
|---|---:|---:|---:|---:|
| Long-card scroll | 9.81 | 8.17 | 221 → 178 | 0.83 |
| Diagram card scroll | 3.13 | 2.93 | 19 → 20.5 | 0.93 |
| Board scroll | 1.07 | 1.03 | 1 → 2 | 0.96 |
| Open cards | 1.59 | 1.52 | 13 → 12.5 | 0.96 |

In the long-card samples, `highlight_target_in` fell from 11.7% to 4.7%, and from 6.1% to
1.5% on the diagram card. Its remaining cost comes from `layout_of`, which calls it once for
each layout read, so the next step is an index from highlight key to node built once per
commit. `Keyed::new` can be cached per commit the same way. Re-run on a quiet machine before
quoting the size of the gain.

No spike was run for #1, because a safe version has to keep the old nodes readable through
measure retries and apply's diffing, which makes it more than a scratch patch. Its upper bound
comes from the commit samples above.
