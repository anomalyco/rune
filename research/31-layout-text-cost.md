# 31. Where a frame goes: Taffy, Parley and Rune

Reader: someone deciding whether to replace Taffy or Parley with engines built for Rune, or
looking for the next big CPU or memory win in a frame. Re-run the bench below before and after
a change to layout, text or the commit path.

The question: how much of a real frame's CPU time, allocations and heap belong to Taffy, to
Parley and its font stack (Fontique, HarfRust, Swash, ICU), and to Rune's own code?

The short answer: outside of shaping, Rune's own code is most of every frame. Taffy is 5% to
16% of a frame and Parley 0.4% to 20%, so the most either replacement could save there is
under 0.9 ms a frame. Shaping is the one place Parley dominates: 73% of opening a long note and 98% of
typing in one. Both cases come from shaping text whose shape is already known, and a cache
above Parley could avoid that without replacing it.

## How it was measured

`crates/rune-kit/examples/layout_text_cost`, headless, no window:

```
CARGO_PROFILE_PERF_DEBUG=limited cargo build --profile perf -p rune-kit \
  --example layout_text_cost --features markdown,frame-profile
target/perf/examples/layout_text_cost                                  # timing
target/perf/examples/layout_text_cost --mode sample --repeat 3 --out s.txt   # CPU profile
target/perf/examples/layout_text_cost --mode heap --out h.txt          # allocation stacks
python3 scripts/cost-attribution.py symbolize s.txt --exe target/perf/examples/layout_text_cost > s.syms
python3 scripts/cost-attribution.py report s.txt s.syms --top 12
```

Each case starts on a fresh runtime, lets it settle, then times its frames:

| Case | What each frame does |
|---|---|
| `stack` | A settings-like column of 60 cards of switches, badges and buttons, not virtualized (5,283 layout nodes, 2,040 texts). Each frame flips one switch. |
| `stack-same` | The same column. Each frame re-runs the view with nothing changed. |
| `list` | 10,000 rows of three texts under `virtual_rows` (278 nodes mounted). Each frame scrolls two rows. |
| `edit` | One multi-line `text_input` holding a 50 KB note. Each frame types a character. |
| `md-resize` | A 50 KB Markdown note in a scroll view (1,268 nodes, 614 texts). Each frame steps the width 1 pt, 900 → 600 → 900, as a live resize does. |
| `md-same` | The same note. Each frame re-runs the view with nothing changed. |
| `md-open` | The first frame of a fresh runtime showing the note, so every text is shaped. |

The note is generated from fictional words, like the one in [28](28-resize-rewrap.md).

Three probes ran inside the example, since no system profiler was available on the build
machine:

- **Timing.** Each frame's thread CPU time comes from `rune/frame-profile`, split into its
  phases. Five runs alternated case order (forward, reverse, forward, …). The tables give the
  median of the five runs' medians and p95s. Most runs landed within 5% of that median. One
  run each of `stack-same` and `md-resize` was 19% and 24% slower, likely because of the other
  jobs on the machine.
- **CPU sampling.** A thread suspends the main thread every 200 µs, walks its frame pointers
  and resumes it. 208,000 samples over three repeats of every case. A sample belongs to the
  innermost frame that isn't std, `alloc`, `core`, hashbrown and the like. Time in `malloc`
  or `free` goes to *Allocator*, which also records who called it. In the tables, allocator
  time counts toward its caller's bucket. Symbols come from `atos -i`, so inlined frames
  count too.
- **Heap.** A counting global allocator records the stack of every allocation during the
  measured frames, plus what is still live once the case settles.

Machine: an M4 Pro Mac mini, `--profile perf`. It was running other build jobs at the time,
with a load average around 15.

Buckets:

- **Taffy:** `taffy::*`, including `Style` comparisons and drops in flatten. It doesn't
  include the text measure callbacks Taffy calls.
- **Parley:** `parley`, `parley_engine`, HarfRust, read-fonts and skrifa (shaping), ICU and
  bidi (text analysis), Fontique (font queries and fallback) and Swash (glyph raster). It also
  includes Rune's clone of a shaped `Layout` before re-breaking it (see below), which exists
  only because Parley breaks a layout in place.
- **Rune:** the `rune` crate and `rune_motion`.
- **View:** `rune_kit`, Markdown and the bench's own view code.
- **Instr.:** the frame-profile clock. Only builds with that feature pay for it.

## Results

### CPU time per frame

The ms columns are thread CPU, median (p95). The bucket shares come from the samples. The
last three columns are the frame with that bucket's share removed: the most a free Taffy, a
free Parley, or both could save.

| Case | ms | Rune | View | Taffy | Parley | Instr. | Taffy = 0 | Parley = 0 | Both = 0 |
|---|---|---|---|---|---|---|---|---|---|
| stack | 10.2 (13.6) | 86.8% | 7.2% | 5.6% | 0.4% | 0.0% | 9.6 | 10.2 | 9.6 |
| stack-same | 9.9 (11.7) | 86.6% | 7.8% | 5.1% | 0.5% | 0.1% | 9.4 | 9.9 | 9.4 |
| list (mean) | 0.44 | 72.7% | 2.9% | 5.3% | 18.2% | 0.5% | 0.41 | 0.36 | 0.33 |
| edit | 15.5 (17.1) | 1.6% | – | 0.1% | 97.8% | 0.1% | 15.5 | 0.34 | 0.33 |
| md-resize | 4.56 (5.87) | 53.7% | 6.4% | 15.6% | 19.6% | 4.5% | 3.85 | 3.67 | 2.96 |
| md-same | 1.98 (2.49) | 80.1% | 12.9% | 4.8% | 2.2% | 0.1% | 1.88 | 1.94 | 1.84 |
| md-open (one frame) | 18.3 (21.3) | 19.1% | 1.5% | 5.0% | 72.8% | 1.2% | 17.4 | 5.0 | 4.1 |

`list` frames come in two kinds. Most mount nothing new (median 0.15 ms). Every few frames,
new rows mount and get shaped (p95 1.16 ms). The samples cover all frames, so the shares are
for the mean.

The allocator takes 11% of `stack`, 12% of `md-resize`, 10% of `md-same` and under 6%
elsewhere. Most of that is Rune's: 9.4 of the 11 points in `stack`. In `md-resize` it splits
4.9 points Rune, 5.2 Parley and the layout clone, 1.6 Taffy.

The Parley column broken down:

| Case | Shaping (HarfRust) | Analysis (ICU, bidi) | Fontique | Line breaking | Layout building and glyph iteration | Clone before re-break | Swash raster |
|---|---|---|---|---|---|---|---|
| edit | 82.8% | 8.6% | 2.5% | 2.5% | 1.3% | – | – |
| md-open | 50.2% | 6.3% | 4.5% | 2.5% | 3.0% | – | 4.4% |
| md-resize | 0.5% | – | – | 7.6% | 2.4% | 3.8% + 2.8% allocator | – |
| list | 7.8% | 0.7% | 1.0% | 0.2% | 8.0% | – | – |

In `list`, "layout building" is mostly paint walking Parley's lines and glyph runs to place
glyphs.

What the time-mode phases say:

| Case, ms | View | Commit | Layout | of which Text | Apply | Paint |
|---|---|---|---|---|---|---|
| stack | 2.41 | 8.54 | 0.058 | 0 | 2.92 | 1.09 |
| stack-same | 2.42 | 8.37 | 0.012 | 0 | 2.84 | 1.06 |
| edit | 0.01 | 15.36 | 15.25 | 15.21 | 0.06 | 0.12 |
| md-resize | 0.64 | 4.20 | 1.99 | 0.79 | 0.82 | 0.26 |
| md-same | 0.57 | 1.67 | 0.002 | 0 | 0.40 | 0.24 |
| md-open | 0.64 | 16.93 | 14.92 | 13.25 | 0.43 | 1.26 |

Commit includes view, layout and apply. Layout includes text.

### Allocations per frame

Calls to the allocator per frame, by the owner of the allocating code, and bytes allocated
per frame:

| Case | Total | Rune | View | Taffy | Parley | Bytes |
|---|---|---|---|---|---|---|
| stack | 45,657 | 39,293 (86%) | 6,290 | 74 | 0 | 30 MB |
| stack-same | 45,586 | 39,293 (86%) | 6,290 | 3 | 0 | 30 MB |
| list (mean) | 1,350 | 1,153 | 91 | 41 | 65 | 1.4 MB |
| edit | 1,176 | 87 | 1 | 14 | 1,074 (91%) | 3.8 MB |
| md-resize | 21,198 | 8,503 (40%) | 2,641 | 4,956 (23%) | 5,099 (24%) | 12 MB |
| md-same | 10,744 | 8,100 | 2,641 | 3 | 0 | 7 MB |
| md-open | 31,264 | 14,270 | 2,641 | 5,669 | 8,684 | 35 MB |

The biggest single sources:

- `flatten::node_path`: 4 allocations per node per frame. That's 21,130 of the 45,657 in
  `stack`, and 5,070 in each Markdown case.
- `El::child` growing children `Vec`s: 4,742 allocations but 16 MB a frame in `stack`. An `El`
  is large, so a few children take kilobytes, and each doubling copies them.
- `flatten` itself (one per node), `El::key`, and the label `String`s.
- Taffy's flexbox: `generate_anonymous_flex_items` and the `FlexLine` vectors allocate on every
  layout of a flex container. That's 4,170 allocations per frame in `md-resize`.
- Parley's line breaker clones `BreakerState`, which owns a vector, once per line: 1,030 of
  `edit`'s 1,176 allocations.
- The layout clone before re-breaking copies seven vectors per text: 3,540 allocations and
  2.2 MB per frame in `md-resize`.

### Heap

What is live once a case settles, by owner (MB = 10⁶ bytes):

| Case | Live | Rune | Taffy | Parley | Other |
|---|---|---|---|---|---|
| stack (5,283 nodes, 2,040 texts) | 57.9 MB | 41.3 | 11.0 | 5.4 | 0.2 |
| list (278 nodes, 165 texts, 10,000 rows) | 11.4 MB | 8.0 | 0.7 | 0.8 | 1.9 (row data) |
| edit (1 field, 50 KB) | 19.2 MB | 5.3 | 0.02 | 13.8 | 0.1 |
| md-resize (1,268 nodes, 614 texts) | 20.8 MB | 13.3 | 2.8 | 3.9 | 0.9 (Markdown) |

Per node and per text:

| Owner | stack | md-resize |
|---|---|---|
| Taffy's node (`Style`, layout cache) | 2.1 KB / node | 2.2 KB / node |
| Rune's flattened node | 2.4 KB / node | 2.5 KB / node |
| `rune_motion` store and scene | 3.5 KB / node | 3.3 KB / node |
| Parley layout | 2.6 KB / text (short labels) | 6.3 KB / text, 76 bytes per byte of text |

One 50 KB field holds 13.8 MB of Parley, about 270 bytes per byte of text. 7.8 MB of that is
`ShapedText`'s reserved runs, clusters and glyphs, and 4.3 MB is HarfRust's buffer. That buffer
grew to fit the whole text and stays that size. The glyph atlas's CPU copy is a fixed 4.2 MB
per runtime and counts toward Rune in every row.

## Text: how much is re-shaping text that was shaped before

- **Resize (`md-resize`): none.** Since [28](28-resize-rewrap.md), a width change re-breaks
  395 texts a frame and shapes none. The rewrap costs 0.8 ms of text time, which is Parley's
  line breaking, cloning the shaped layout before breaking it (7.1% with its allocations), and
  hashing (below). A word-shape cache would save nothing here.
- **Typing (`edit`): almost all of it.** Each keystroke shapes the whole 50 KB note as one
  text: 15.2 ms, 83% of it in HarfRust. The note has 7,201 words and 772 distinct ones, and
  one word changed. Splitting the field into paragraphs ([27](27-long-note-editing.md))
  removes nearly all of it, with no word cache needed.
- **Opening (`md-open`): most of it, if the cache is per word.** The 404 texts shaped hold the
  same 7,201 words, of which 772 are distinct. A cache keyed by (word, style, font) would
  shape 11% of them and could cut the 9.2 ms of HarfRust in this frame by up to about 8 ms.
  That is an upper bound. The note's generator draws from a small vocabulary, so real prose
  repeats words less. Shaping a word alone also loses context across word boundaries, which
  some scripts and fonts need, so a cache like this needs a fallback for them. The list's
  10,000 rows have 149,438 words and 10,050 distinct ones (6.7%).

## Layout: how much is re-solving unchanged subtrees

- **When one switch flips (`stack`), almost none.** Taffy spends 0.058 ms laying out 5,283
  nodes, and 0.012 ms when nothing changed. Its cache already skips the clean subtrees. In
  `stack`, Taffy's 5.6% is spent in flatten, not in layout: comparing and dropping `Style`
  values to decide whether a node's style changed. Most of that goes to the grid fields Rune
  never sets (`Style::eq` 1.0%, the grid track and placement comparisons 1.6%).
- **During a resize (`md-resize`), there are no unchanged subtrees to skip in Taffy's
  terms.** Every node's available width changes, so every cache entry misses. Taffy's own
  work is 15.6% of the frame (0.71 ms, about 0.56 µs per node), including about 4,170
  allocations for flex items and lines. Some subtrees end up the same size anyway (code blocks
  and short lines that fit at both widths). Counting those would need instrumentation inside
  the runtime, which this pass didn't add. Skipping all of Taffy's work is still at most 0.71
  ms.
- **Measure calls.** `text_hash` runs once per text in flatten, once in `measure_view`, and on
  each Taffy measure call. Its time in each place puts Taffy's measure calls at about four per
  text per frame during a resize. Each call hashes the whole text with SipHash before it
  checks the cache.

## The biggest costs

Rune's own code:

1. **Re-running and re-committing a view that didn't change.** `stack-same` costs 9.9 ms a
   frame for 5,283 nodes: 1.9 µs, 8.6 allocations and 5.7 KB allocated per node. The time
   splits into apply (27%), flatten (15%), paint (11%) and view building (10%, plus 6% in kit
   components and 7% in the allocator). The allocations are `node_path` (4 per node) and
   children `Vec`s of large `El`s. In `stack`, `rune_motion`'s `AnimationStore::set_raw` and
   its OKLab conversion take 8%.
2. **Hashing whole texts on every measure.** `text_hash` (SipHash over the full text, its
   spans and style) is 11.2% of `md-resize`, about 0.5 ms, and 4.4% of `stack`. Flatten
   already computes the same hash. Passing it into measure would remove most of the cost.
3. **Cloning a shaped layout to re-break it:** 7.1% of `md-resize`, with 3,540 allocations a
   frame.
4. **Heap per node:** about 2.5 KB in the flattened node and 3.3 to 3.5 KB in the motion store
   and scene, against Taffy's 2.1 KB.
5. Instrumentation: frame-profile's clock costs 4.5% of `md-resize`, because a timing scope
   opens on every re-break. Builds without the feature don't pay it, so a production
   `md-resize` frame is closer to 4.35 ms than 4.56 ms.

Dependencies:

1. **HarfRust shaping** when a text is shaped: 50% of `md-open` and 83% of `edit`. A new
   engine would still have to shape. What would make this cheaper is shaping less (paragraphs
   for editing, maybe words for opening), and that works on top of Parley.
2. **Taffy during a resize:** 15.6%, 0.71 ms, and a quarter of the frame's allocations.
   Elsewhere it's 5% or less, mostly `Style` comparison in flatten.
3. **Parley memory:** about 270 bytes of heap per byte of text in one large field, and 76 in
   a Markdown note.
4. **Parley's per-line `BreakerState` clone:** 1,030 allocations per keystroke at 50 KB.

## What headless leaves out

- The GPU side: encoding, glyph and image uploads, submit, and waiting for presentation. Glyph
  rasterizing on the CPU (Swash) is included. [30](30-overlay-gpu-cost.md) and
  [29](29-scroll-frame-cpu.md) cover the GPU side.
- Delivery of real window events: winit, IME and the OS text input services, and the resize
  handshake with the window server.
- Other threads. Rune does all of this work on the main thread, so a single-thread profile is
  complete for these phases, but it says nothing about contention with other threads.
- Cache warmth in a real app. Each case runs on a fresh runtime in a process that has already
  run the earlier cases. A real app's caches may be colder (after other windows) or warmer.
- Startup. The system font list is built once per process, before any timed frame.
  `md-open` does include font matching and fallback queries (Fontique, 4.5%).
- The probes' own cost. The sampler stops the main thread for a few microseconds per sample,
  and heap mode makes every allocation slower. Timing runs used neither probe.
