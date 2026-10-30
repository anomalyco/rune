# 32. Layout and text engines: verdicts and frame-cost plan

Reader: someone deciding whether Rune should build its own layout engine (replacing Taffy) or
text engine (replacing Parley), or choosing the next frame-time and memory wins in `rune`,
`rune-motion` and `rune-kit`. Re-run `layout_text_cost` ([31](31-layout-text-cost.md)) and
`scripts/motion-audit` before and after each phase below.

The question: from first principles, should Rune replace Taffy and Parley with purpose-built
engines, and how should it cut frame CPU, per-frame allocations and retained heap from here?

The short answer: **keep both Taffy and Parley.** Outside text shaping, Taffy is 5% to 16% of
a frame and Parley is 0.4% to 20% ([31](31-layout-text-cost.md)); replacing both with free,
zero-byte engines would leave a 5,283-node settings commit at 9.6 ms (still over the 8.3 ms
120 Hz budget) and a 50 KB Markdown resize at 2.96 ms. Where Parley dominates (98% of typing
in a 50 KB `text_input` at 15.5 ms, and 73% of cold-opening a 50 KB note at 18.3 ms), it is
spending 83% and 50% of the frame inside HarfRust OpenType shaping, which an own engine would
still run unless it shapes less text. Fixes inside Rune's own commit, motion, style and
caching layers can bring typing to ~1 ms, live resize to ~1.4 ms, and steady 5,283-node
commits to ~3.5 ms (< 0.8 ms with `memo`), while cutting per-node heap roughly in half. Two
focused layers *are* worth building from scratch now: a compact `BoxStyle` on `El` (replacing
inline `taffy::Style`) and a multi-paragraph composite `TextLayout` above `parley::Layout`.

---

## 1. Verdict on an own layout engine: keep Taffy, delete the mirror's waste

### Evidence

- **Solver CPU is tiny.** In `layout_text_cost` ([31](31-layout-text-cost.md)), Taffy's layout
  pass (`Phase::Layout`) takes **0.058 ms** for 5,283 nodes when one switch flips (`stack`)
  and **0.012 ms** when nothing changed (`stack-same`). Taffy's 5.6% sample share in `stack`
  (0.57 ms) is spent in `flatten`, comparing and dropping `taffy::Style` values whose CSS Grid
  `Vec` fields Rune never populates (2.6% alone in `Style::eq` and grid track comparisons).
- **Even a full cache-miss resize has a 0.71 ms ceiling.** Stepping a 50 KB Markdown window's
  width by 1 pt (`md-resize`, 1,268 nodes) invalidates every width constraint in Taffy. Taffy
  takes 15.6% of the frame (**0.71 ms**, 0.56 µs/node, 4,170 flex-line allocations). A magic
  zero-cost solver would take `md-resize` from 4.56 ms to 3.85 ms; skipping Rune's redundant
  view/flatten rebuild on resize saves twice as much (~1.5 ms) in ~120 lines.
- **Taffy is the smallest of the three per-node heap owners.** Live heap per node in `stack`
  is **2.1 KB** in Taffy, **2.4 KB** in Rune's flattened `Node`, and **3.5 KB** in
  `rune_motion`. An earlier branch (`t710-taffy-adapter`) replaced `TaffyTree` with a dense
  364 B/node store (−71% layout-tree heap on 500 rows) and left commit time flat, confirming
  that commit CPU sits in Rune's tree walk, string paths, style comparisons and `Scene`
  retargeting rather than Taffy's slot maps.
- **Clean seam, real CSS parity risk.** `El.style` is `pub(crate)` (`crates/rune/src/el.rs:782`)
  and `taffy` is never re-exported, touching 12 files in `crates/rune`. Rune uses flex
  (`row`/`col`, grow/shrink/basis, wrap, gap, min/max, padding/border/margin, `aspect_ratio`),
  `Position::Absolute` (under `Position::Relative` by default), and one grid helper,
  `equal_columns(n)` (`repeat(n, 1fr)` on row 1). Even on that subset, views and `rune-kit`'s
  components depend on CSS flexbox specifics: automatic minimum size (`min_h(0.0)` on flexing
  scroll views), `flex_shrink: 0.0` defaults, `max-content` horizontal scroll rewrites
  (`runtime/flatten.rs:312-334`), shrink negotiation under min/max bounds, and `flex_wrap`
  with gaps. Re-owning those (~2.5 to 3.5 kLOC plus differential fuzzing) carries real
  layout-regression risk for ≤ 0.71 ms of upside.

### Measurable trigger to revisit an own layout solver

Build a Rune-native single-pass layout solver over `Vec<Node>` only if, **after Phases 1 and 3
land**, one of these holds on `layout_text_cost` (`--profile perf`, paired medians):

1. `taffy::*` exceeds **30% of frame CPU and ≥ 1.0 ms** on a target 120 Hz workload (such as
   resizing or mutating a 5,000-node unvirtualized table or canvas).
2. Taffy's rootward dirty propagation (`mark_dirty` walking to the root on any leaf edit)
   spends **≥ 0.5 ms** on localized edits inside fixed-size or scrolling containers where
   Flutter-style relayout boundaries would stop at the container.
3. Multi-pass layout commits (`aspect_fit`, `fit_anchored`, or `virtual_list` height-bound
   retries) still run ≥ 2 full-tree solves on **> 5% of commits** in real traces after PR 4c.

---

## 2. Verdict on an own text engine: keep Parley + HarfRust, cache at paragraph granularity

### Evidence

- **Outside shaping, Parley is ≤ 0.6 ms.** On steady commits (`stack`, `md-same`), the entire
  text stack (Parley, `parley_engine`, HarfRust, Fontique, ICU4X, Swash) is 0.4% to 2.2% of
  the frame (≤ 0.05 ms). On `md-resize` (4.56 ms), Parley's bucket is 19.6% (0.89 ms), but a
  third of that (6.6%, 0.30 ms, 3,540 allocations) is Rune's own `Layout::clone` before
  re-breaking; Parley's actual line breaking of 395 wrapped texts takes **0.35 ms** (0.88 µs
  per paragraph).
- **Typing is slow because Rune hashes and reshapes the whole 50 KB field per keystroke.**
  Typing one character in a 50 KB `text_input` (`edit`) takes **15.5 ms** (97.8% Parley, of
  which 82.8% is HarfRust shaping and 8.6% is ICU/bidi analysis) and holds **13.8 MB** of
  Parley heap (7.8 MB across two whole-note `ShapedText` buffers plus a 4.3 MB HarfRust
  `UnicodeBuffer` high-water mark). Splitting the same 50 KB note into keyed blocks
  ([27](27-long-note-editing.md) `--blocks`) already drops typing to **0.82 ms** (reshaping
  one block takes 0.03 to 0.6 ms). Any custom text engine that shapes a 50 KB buffer as one
  unit would still spend ~12 ms in HarfRust and fail 120 Hz.
- **Cold open is bounded by unique-word shaping and unvirtualized offscreen blocks.** `md-open`
  shapes 404 texts in **18.3 ms** (9.2 ms HarfRust, 1.15 ms ICU/bidi, 0.82 ms Fontique, 0.81 ms
  Swash). Note that `layout_text_cost`'s generator draws from a 50-word list (`main.rs:43-47`:
  772 distinct tokens across 7,201 words, an 89% repeat rate), whereas real prose and code
  repeat ~40% to 55% of words. Meanwhile, only 32 of the 614 texts in `md-open` are on screen
  ([28](28-resize-rewrap.md)); virtualizing Markdown blocks avoids shaping ~90% of offscreen
  blocks on open without touching the shaper.
- **High parity cost for replacing Parley.** Owning text below `parley` means re-implementing
  UAX #9 bidi reordering and visual-order cursor movement (`Cursor`/`Selection`), UAX #14/29
  line and grapheme segmentation, fallback itemization across ZWJ emoji and combining marks,
  IME preedit caret/underline geometry, and CoreText vertical-metric parity
  (`core_text_tests.rs`). Switching to per-OS native shapers (CoreText/DirectWrite) would give
  up Rune's cross-platform layout determinism and still leave web/Linux needing a Rust stack.

### Measurable trigger to revisit a lower text seam (`parley_engine` breaker)

Never build an own OpenType shaper, bidi engine or OS-divergent shaper. Revisit replacing
`parley::Layout` with Rune's own line breaker and word/run shape cache directly over
`parley_engine 0.12` (`Analyzer` + `Shaper` + `FontSelector`) only if, **after Phases 1, 2
and 4 land**:

1. Cold open of **unvirtualized** UI or document views (≥ 300 uncached texts on screen at once)
   still spends **≥ 6.0 ms in HarfRust**, **and** the Phase 0 spike on real English/code files
   shows **≥ 50% word-cache hits and ≥ 2.0× net wall-clock speedup** after per-word overhead
   and `unsafe-to-break` fallbacks; **or**
2. Workarounds around `parley::Layout` (inline-box `cursor_from_point`, `cursor_geometry`
   `inline_min_coord`, `2n+1` pill re-breaks, space-widened tabs) need > 300 new lines of
   wrapper code and upstream fixes in Parley are blocked.

---

## 3. Five root causes verified in the code

Checking the attribution numbers in [31](31-layout-text-cost.md) against `crates/rune` and
`crates/rune-motion` turned up five specific mechanisms that explain the big buckets:

1. **`window_tree` walks every node and expands every `memo` even when no `virtualize` exists
   (`crates/rune/src/runtime.rs:3543-3645`, `el.rs:1613-1618`).**
   `window_tree` only mutates elements with `virtual_estimate.is_some()`, yet `build_tree`
   runs it unconditionally over all 5,283 nodes of `stack` and 1,268 nodes of `md-*`. Worse,
   line 3544 calls `el.expand_memo()` while `MemoCache` still holds a second `Rc<Shared>`
   reference, forcing `Rc::unwrap_or_clone` to **deep-clone every cached memo subtree's
   `Vec<El>`** before `flatten` consumes it. `window_tree` and `flatten` each call
   `node_path` (`format!`), and `flatten.rs:229` calls `path.clone()`, accounting for all **4
   `node_path` allocations per node** (21,130 of 45,657 allocations in `stack`).
2. **`Scene::record_track` marks every node `dirty = true` on no-op `animate` calls, and
   `apply` eagerly allocates 4 to 8 `AnimationStore` tracks on static nodes
   (`crates/rune-motion/src/scene.rs:870-940, 1065-1085`, `store.rs:466, 940`,
   `crates/rune/src/runtime.rs:3400, 4200-4298`).**
   - Each lane in `AnimationStore` takes ~108 B (~160 B with `Vec` capacity; `Layer` is 64 B).
     On commit 1, `apply` calls `set_or_anim(P_FILL, Color::TRANSPARENT)` on all 5,283 nodes
     and `P_TEXT` on all 2,040 text/icon nodes, even though `Runtime::color` (`runtime.rs:3401`)
     ignores `AnimationStore` whenever `!is_animating`. On commit 2, `Scene::animate` calls
     `ensure_prop` *before* checking `target == base`, creating 4-lane `RADIUS` and 1-lane
     `OPACITY` tracks on every static box (`Scene::insert` also creates `PRESENCE` on
     `Mount::Initial`). Together, static nodes carry ~14 unused lanes and ~5 tracks, explaining
     `rune_motion`'s **3.4 to 3.5 KB/node** (18.5 MB in `stack`).
   - `AnimationStore::animate` runs `Color::write_lanes` (`3 × powf(2.4)` + `3 × cbrt()` for
     OKLab) *before* checking whether the target changed (~10,000+ conversions/frame in `stack`).
   - For the ~4,750 offscreen nodes in `stack`, `quiet = out_of_sight` calls `Scene::set` →
     `set_raw`, which never checks if lanes already sit at `values` and pushes 9,500
     `TrackId`s into `set_touched`. For onscreen nodes, `record_track` sets `dirty = true`
     even when `animate_raw` was a no-op, forcing `Scene::present_dirty` to recompute all
     5,283 `Presented` structs on every commit, including `stack-same` and `md-same`.
3. **`Cx<S>` has no `viewport` getter, and `Runtime` already computes `state_unchanged`
   (`crates/rune/src/runtime.rs:270-386, 2299`).**
   A view function can only depend on layout geometry through `Cx::caret` (`caret_watch`),
   `Cx::layout*` (`layout_seen`), or `virtuals`. During `md-resize`, `state_unchanged` is
   already `true`, `scale` is unchanged, and `virtuals` is empty, so re-running `view`
   (0.64 ms) and `flatten` (~0.85 ms, 8,503 allocations) rebuilds the exact same `Node` and
   Taffy trees every 1 pt step.
4. **`Layout::clone` during resize (7.1% of `md-resize`, 3,540 allocs, 2.2 MB/frame) comes
   from `Rc` aliasing with `self.nodes` and stale widths in `self.layouts`
   (`crates/rune/src/text.rs:1629-1639`, `runtime.rs:3711-3722`).**
   `parley::Layout` stores shaped runs and broken lines in one struct (`LayoutData`). Cloning
   happens because `self.nodes` (from the prior commit) and `self.layouts[prev_width]` both
   hold `Rc<Layout<Ink>>` (`strong_count == 2`) while `compute_layout` runs. Clearing
   `old.text_layout` on surviving (non-exiting) nodes before `compute_layout` and moving an
   entry with `used < g` from `prev_key` to `key` drops `Rc::strong_count` to 1, letting
   `Rc::make_mut` re-break `&mut Layout<Ink>` in place with zero allocations.
5. **Hidden Parley setup work in `layout_blocks_inner` (`crates/rune/src/text.rs:1602, 1763,
   1855-1875`).**
   `lcx.ranged_builder` resolves `TextStyle::default()` (triggering an unused Fontique query
   for `"sans-serif"`) and splits overlapping style ranges, even though `el.rs:1013`
   (`flatten_spans`) already flattens spans into contiguous non-overlapping runs suited to
   `lcx.style_run_builder`. And when a fallback font's `trak` table differs from the primary
   font's, line 1874 rebuilds and reshapes the entire layout a second time.

---

## 4. Ranked improvements

Baselines from [31](31-layout-text-cost.md) (50 KB, `--profile perf`, M4 Pro): `edit` 15.5 ms
(19.2 MB heap), `md-open` 18.3 ms, `stack` 10.2 ms (45,657 allocs, 57.9 MB heap), `stack-same`
9.9 ms, `md-resize` 4.56 ms / 4.35 ms without `frame-profile` (21,198 allocs, 20.8 MB heap),
`md-same` 1.98 ms, `list` 0.44 ms mean (0.15 ms median). Savings note where items overlap on
the same workload.

| Rank | Change | Expected CPU & alloc/heap impact | Size | Risk |
|---:|---|---|---|---|
| **1** | **Per-paragraph composite `TextLayout` + range-delta undo** (`text/composite.rs`, `edit/history.rs`, ~12 `Layout<Ink>` readers): split multi-paragraph text at `\n` into independently cached `Rc<parley::Layout<Ink>>` slices under a `(byte_start, y_top, line_start)` prefix index; store `TextEdit` undo steps as byte-range diffs. | **`edit`: −14.3 to −14.7 ms** (`15.5 → ~0.8–1.2 ms`); **−9 to −10 MB heap** in `edit` (caps HarfRust buffer to 1 paragraph, drops dual 50 KB `ShapedText` copies); `edit` allocs `1,176 → < 80`; keeps 100/100 undo steps at 200 KB. | ~900 LOC | Medium (UAX #9 bidi is paragraph-scoped so `\n` split is exact; work is routing readers cleanly). |
| **2** | **Lazy `Scene`/`AnimationStore` tracks, no-op `dirty` fix, and sRGB pre-check** (`rune-motion/src/scene.rs`, `store.rs`, `runtime/apply.rs`): leave `dirty = false` on unchanged `animate`; no-op `set_raw` when settled at target; create `PRESENCE`/`RADIUS`/`OPACITY`/`P_FILL`/`P_TEXT` tracks only when values differ from rest; compare `Color` floats before OKLab `powf`/`cbrt`. | **`stack` / `stack-same`: −2.3 to −2.8 ms** (`apply` `2.92 → ~0.4 ms`, removes 8% `set_raw`+OKLab and 5,283-node `present_dirty`). **`md-resize`: −0.55 ms**. **`md-same`: −0.32 ms**. Heap: **−14 to −16 MB in `stack`** (`3.5 → ~0.5 KB/node`), **−3.5 MB in `md-resize`**. | ~260 LOC | Low (`scripts/motion-audit` + retarget velocity tests gate exact behavior). |
| **3** | **`has_virtual` subtree bit + retained `(NodeId, Segment)` `PathTable` (`Rc<str>`)** (`runtime/path_table.rs`, `flatten.rs`, `el.rs`, `memo.rs`): propagate `has_virtual: bool` on `El`/`Shared` to skip `window_tree` (and stop pre-flatten `memo` deep-clones) when no virtual list is under a node; intern `(parent_id, key_or_index) → (NodeId, Rc<str>, LayoutEntry)` across commits. | **`stack` / `stack-same`: −1.8 to −2.4 ms**. **`md-resize` / `md-same`: −0.35 to −0.50 ms**. **`md-open`: −0.5 ms**. Allocs: **−21,130/frame in `stack`**, **−5,070/frame in `md-*`**, while keeping `&str` deref for all ~100 `.path` readers. | ~300 LOC | Low (`--tree` and `key_diagnostics` verify identical paths). |
| **4** | **Viewport-relayout fast path (skip `view` + `flatten` when `state_unchanged`)** (`runtime.rs`): on `relayout` with `state_unchanged && scale_unchanged && virtuals.is_empty()`, update `root_id`'s `style.size` on `self.layout` and run `compute_layout` + `place` + text re-break on `self.nodes`; fall back to `measure_view` only if `layout_reads_moved()` or `caret_moved()` fires. | **`md-resize`: −1.35 to −1.55 ms** (skips 0.64 ms `view` and ~0.85 ms `flatten`; overlaps with #3 on `md-resize`). Allocs: **−11,140/frame** (−5 MB/frame) in `md-resize`. | ~120 LOC | Low (`Cx` exposes no viewport; read/caret fallback preserves reactive views). |
| **5** | **Single `shape_key` hash per `TextSpec` with fast non-SipHash mixer** (`text/hash.rs`, `el.rs`, `flatten.rs`, `text_layout.rs`): compute `shape_key: u64` once on `TextSpec` (invalidated only by `apply_case`/`conceal`) and pass it through Taffy `measure` (~4×/text) and final text layout instead of SipHashing ~6×/frame. | **`md-resize`: −0.38 to −0.50 ms** (11.2% → < 0.5%). **`stack` / `stack-same`: −0.40 to −0.45 ms** (4.4%). **`md-open`: −0.25 ms**. | ~100 LOC | Very low. |
| **6** | **In-place `Layout` re-break via `Rc::make_mut` (remove `Layout::clone`)** (`text.rs`, `runtime.rs`): drop `old.text_layout` on non-exiting nodes before `compute_layout` and move `used < g` cache entries from `prev_key` to `key` so `Rc::make_mut` re-breaks in place with zero copies. | **`md-resize`: −0.28 to −0.33 ms** (7.1% of frame). Allocs: **−3,540/frame and −2.2 MB/frame** in `md-resize`; halves Parley resize cache memory. | ~80 LOC | Low (`rewrap_tests` and `secure_text` `strong_count` tests cover edge cases). |
| **7** | **Compact `BoxStyle` (~88 B `Copy`) + `Cold<T>` split on `El`/`Node` + `subtree_end` paint culling** (`el/style.rs`, `runtime/node.rs`, `runtime/paint.rs`): replace inline `taffy::Style` on `El` with `BoxStyle` (converting to `taffy::Style` only on style change, dropping unused `content_size`/`block_layout` features); box cold handler/scroll/motion/visual groups via `Cold<T>`; store `subtree_end: u32` on `Node` to skip offscreen scroll subtrees in O(1) during `paint`. | **`stack` / `stack-same`: −2.0 to −2.8 ms** (kills Taffy's 0.57 ms `Style` compare/drop in `flatten`, cuts 16 MB/frame `El::child` realloc copies ~5×, drops `paint` `1.09 → ~0.3 ms` on 88% offscreen cards). **`md-same`: −0.4 to −0.6 ms**. Heap: **−9 to −11 MB in `stack`** (`Node` `2.4 → ~0.6 KB/node`). | ~600 LOC | Low–Medium (mechanical layout/cold refactor; `BoxStyle` covers 100% of `El` builders). |
| **8** | **Flattened subtree reuse for `memo`** (`memo.rs`, `flatten.rs`, `runtime.rs`): once #3 stops `window_tree` from expanding memos, reuse the prior commit's flattened `Node` slice (rebasing parent/context indices) and Taffy `NodeId` subtree on a `memo` hit, skipping `flatten` and clean interior `apply`. | **Memoized `stack` / real app views: `9.9 ms → < 0.8 ms`** on 1-card flip; **`md-same --memo`: `1.98 → < 0.25 ms`**. Scales commit cost with changed subtrees instead of mounted nodes. | ~350 LOC | Medium (must replay `Cx` reads and rebase Taffy leaf context indices accurately). |
| **9** | **`ResolvedStyle` cache + `style_run_builder` + single-build `trak`** (`text/resolved.rs`, `text.rs`): cache family stacks, features, variations and `trak` by `(TextStyle, collection_gen)`; switch `ranged_builder` to `style_run_builder`; reuse `LayoutContext` on secure text; pre-resolve fallback `trak` / update run spacing instead of double-shaping on mismatch. | **`md-open`: −1.5 to −2.5 ms** (and −40% to −50% on mixed-script/emoji text that currently double-shapes). **`list` mounting p95: −15%**. Allocs: **−3,500+ in `md-open`**. | ~280 LOC | Low (`core_text_tests.rs` verifies metrics and tracking). |
| **10** | **Fix `El::virtualize` caret/reveal + virtualize `rune_kit::Markdown` blocks** (`virtual_list.rs`, `runtime/caret.rs`, `rune-kit/src/markdown.rs`): fix virtualized `ElementInfo::caret` and `scroll_into_view` headless typing ([27](27-long-note-editing.md)); virtualize top-level Markdown blocks with re-break height estimates while keeping `markdown/find.rs` and `virtual_selection` working. | **`md-open` (50 KB): −11 to −13 ms** (`18.3 → ~4.5–6.0 ms`, and bounds 200 KB open at ~6 ms instead of 70 ms). **200 KB `md-resize`: `23.2 → < 2.0 ms`**. Heap: **−12 MB** on 50 KB Markdown. | ~450 LOC | Medium (offscreen height estimates until lazily broken; selection/find integration). |
| **11** | **Single-pass `aspect_fit` leaf measure & subtree-scoped `fit_anchored`** (`runtime/flatten.rs`, `runtime/anchor.rs`): compute `aspect_fit` inside Taffy's leaf measure closure where childless; run overflowing `anchored_fit` re-layout on `overlay_id` instead of `root_id` (an `above` overlay never moves siblings). | **−30% to −66% of layout CPU** on media grids and overflowing popovers/menus (1 root solve instead of 2 or 3). | ~130 LOC | Low. |
| **12** | **(Conditional, gated on Spike & Trigger) Word/run shape cache over `parley_engine`** (`text/`): cache shaped runs for space-delimited words without cross-space GSUB/GPOS, falling back to full runs for complex scripts. | **`md-open` (only if unvirtualized): −3 to −4.5 ms on real prose** (up to −8 ms on 50-word bench). Unneeded for notes once #10 virtualizes `md-open`. | 3–4.5 kLOC | High (see §5). |

### Projected scorecard after Phases 1 to 4 (keeping Taffy and Parley)

| Case (50 KB) | Today (ms) | Free Taffy + Parley today (ms) | Projected Phases 1–4 (ms) | Live heap today → projected |
|---|---:|---:|---:|---|
| `edit` | 15.5 | 0.33 *(if shaping = 0)* | **~0.9–1.2** | 19.2 MB → **~9.5 MB** |
| `md-open` | 18.3 | 4.1 | **~15.0** *(unvirt.)* / **~4.5–6.0** *(virt.)* | 20.8 MB → **~14 MB** *(~7 MB virt.)* |
| `md-resize` | 4.56 *(4.35 prod)* | 2.96 | **~1.3–1.6** | 20.8 MB → **~14 MB** |
| `md-same` | 1.98 | 1.84 | **~0.6–0.8** *(< 0.25 with `memo`)* | 20.8 MB → **~14 MB** |
| `stack` | 10.2 | 9.6 | **~3.5–4.5** *(< 0.8 with card `memo`)* | 57.9 MB → **~23–26 MB** |
| `stack-same` | 9.9 | 9.4 | **~3.2–4.0** *(< 0.5 with card `memo`)* | 57.9 MB → **~23–26 MB** |
| `list` (mean) | 0.44 *(0.15 med)* | 0.33 | **~0.28–0.33** *(0.10 med)* | 11.4 MB → **~9.5 MB** |

---

## 5. Where a from-scratch layer is the right choice

### Build from scratch now

1. **`BoxStyle` (~88 B, `Copy`) in `crates/rune/src/el/style.rs` (~350 LOC, zero parity risk).**
   `El` currently embeds `taffy::Style` (~400+ B with nine CSS Grid `Vec`s Rune leaves empty),
   paying to construct, move in `Vec<El>`, compare (`Style::eq`) and drop it on every node of
   every commit. A purpose-built `BoxStyle` stores only Rune's actual box/flex/anchor/columns
   fields in ~88 B, compares as plain words in `flatten`, and converts to `taffy::Style` only
   when a node's style changed. It eliminates Taffy's 5.6% `stack` overhead today and cleanly
   decouples `El` from Taffy if a native solver is ever warranted later.
2. **`ParagraphComposite` multi-paragraph layout in `crates/rune/src/text/composite.rs` (~800 LOC,
   low-to-medium parity risk).**
   `parley::Layout` is a single-block layout buffer by design. Building a thin composite over
   `Vec<ParagraphSlice>` (each holding an `Rc<parley::Layout<Ink>>`, cached by paragraph
   `shape_key` and width, indexed by binary search on byte, y and line offsets) solves long-note
   editing (`15.5 → ~1 ms`) and halves Parley's editor memory while keeping Parley's exact
   shaping, bidi and line-breaking behavior within every paragraph.

### Keep gated behind §1 and §2 triggers (do not build before Phase 4)

3. **Native single-pass flex + anchor solver over `Vec<Node>` and `BoxStyle` (~2.5 to 3.5 kLOC
   solver + ~1.2 kLOC differential fuzzer against Taffy; medium-to-high parity risk).**
   If a real workload ever trips the §1 trigger, a native solver wins by operating directly on
   `Vec<Node>` rather than by running faster flex arithmetic: it writes straight into
   `Node.local/target/inset` with one reusable scratch arena (zero per-container `FlexLine`
   allocations), enforces relayout boundaries at scroll viewports and fixed-size boxes so leaf
   edits stay O(subtree), and folds `aspect_fit`, `equal_columns(n)` and `anchored_fit` into
   single-pass sizing rules. Parity risk sits in CSS automatic minimum sizes (`min_h(0.0)`),
   `max-content` horizontal scroll sizing, shrink negotiation under min/max clamps, and
   `flex_wrap` + `gap`; promotion must require `±0.01 pt` agreement with Taffy on generated
   `El` trees and every `rune-kit` story.
4. **Custom line breaker + shaped-word/run cache over `parley_engine 0.12` (~3 to 4.5 kLOC +
   ~1.5 kLOC RTL/CJK/emoji/CoreText suite; high parity risk).**
   If unvirtualized cold-open shaping trips the §2 trigger, dropping `parley::Layout` for
   `parley_engine::{Analyzer, Shaper, FontSelector}` separates immutable shaped runs from
   lightweight `Vec<LineSlice>` line breaking, turns pills, real tab stops, block indents and
   optical `trak` into first-class breaker inputs (deleting five wrapper layers), and enables a
   Blink-style space-delimited word cache. Parity risk is high: `parley_engine` 0.12 notes that
   line-edge reshaping (`unsafe-to-break`) is still in progress, Blink disables word caching
   whenever a font has GSUB/GPOS rules touching space, complex scripts still shape per run, and
   Rune would take over bidi visual-order `Cursor`/`Selection` movement and CoreText vertical
   metrics parity.

---

## 6. Phases, measurable gates, and the first spike

Several target files (`runtime.rs`, `el.rs`, `text.rs`, `edit.rs`, `markdown.rs`) sit in
`scripts/file-size-exceptions.txt`. Per `AGENTS.md`, any phase touching one extracts that area
into a sibling module first and lowers its exception ceiling in `scripts/file-size.sh`.

### First spike (do this first; ~180 LOC, throwaway, half a day)

The two riskiest assumptions in this plan are (a) that stacking per-`\n` `parley::Layout<Ink>`
slices matches a single multi-paragraph `parley::Layout<Ink>` to the pixel across indents,
pills, RTL and Retina quantization (before refactoring ~12 reader call sites), and (b) whether
a word-shape cache still wins on *real* prose and code once `layout_text_cost`'s 50-word
synthetic vocabulary is replaced.

1. **Paragraph-stacking equivalence + `edit` timing (~100 LOC test/bench):** Split `edit`'s
   50 KB string at `\n` into cached `(shape_key, width) → Rc<Layout<Ink>>` slices and stack
   their line metrics. Check bit-exact `(glyph.id, x, y)` and line `[block_min_coord,
   block_max_coord]` parity against `layout_blocks_inner` across `rewrap_tests.rs` (spans, all
   4 alignments, RTL, CJK, emoji, math, indented `Block`s, pills, trailing `\n`, scale 1x/2x),
   and time `layout_text_cost --case edit`.
   - **Gate:** 0 px discrepancy (or exact quantization rule pinned down) **and** `edit` median
     `cpu_ms ≤ 1.2 ms`, live Parley heap `≤ 5.0 MB`.
2. **Real-corpus word-cache ceiling (~80 LOC bench):** Run three real ~50 KB inputs (`PLAN.md`,
   `CHANGELOG.md`, `crates/rune/src/runtime.rs`) through `WORDS` token counting and a throwaway
   per-word `parley_engine::Shaper` cache vs Parley paragraph shaping.
   - **Gate:** If net wall-clock speedup on real Markdown/code is `< 2.0×`, **kill Option 5.4**
     (`parley_engine` word-cache breaker) outright and rely on Phase 4b Markdown virtualization
     for cold open.

### Phase 1: Commit, resize and motion fast paths (4 PRs, ~550 LOC)

- **PR 1a (`text/hash.rs`, `flatten.rs`, `text_layout.rs`):** Precompute `shape_key: u64` once
  on `TextSpec` with a fast mixer (`Fx`) and pass it through `measure` and final layout.
  - *Gate:* `md-resize` `cpu_ms` drops `≥ 0.35 ms` (`text_hash` sample `< 1%`, down from 11.2%);
    `stack` drops `≥ 0.35 ms`.
- **PR 1b (`text.rs`, `runtime.rs`):** Clear `old.text_layout` on surviving nodes before
  `compute_layout` and move `used < g` cache entries across widths so `Rc::make_mut` re-breaks
  in place without `Layout::clone`.
  - *Gate:* `md-resize` `allocs` drops `≥ 3,400/frame` (`21,198 → ≤ 17,700`) and `cpu_ms` drops
    `≥ 0.25 ms`; `rewrap_tests` and `secure_text` `Rc::strong_count` tests pass.
- **PR 1c (`rune-motion/src/scene.rs`, `store.rs`, `runtime/apply.rs` extracted from
  `runtime.rs`):** Leave `Scene` node `dirty = false` on unchanged `animate`; no-op `set_raw`
  when settled at target; allocate `PRESENCE`/`RADIUS`/`OPACITY`/`P_FILL`/`P_TEXT` tracks
  lazily on first change/animation; compare `Color` sRGB floats before OKLab conversion.
  - *Gate:* `stack` and `stack-same` `apply_ms` `2.92 → ≤ 0.60 ms` (`cpu_ms ≤ 7.6 ms`), `stack`
    live heap `57.9 → ≤ 43 MB` (`rune_motion ≤ 0.7 KB/node`), `scripts/motion-audit` clean.
- **PR 1d (`runtime.rs`):** Relayout-only fast path on `set_viewport` when `state_unchanged &&
  scale_unchanged && virtuals.is_empty()`, with fallback to `measure_view` if `layout_reads` or
  `caret_watch` moved.
  - *Gate:* `md-resize` `cpu_ms ≤ 1.7 ms`, `view_ms = 0`, `allocs ≤ 9,500/frame`.

### Phase 2: Multi-paragraph `TextLayout` and delta undo (3 PRs, ~950 LOC)

- **PR 2a (`text/composite.rs` extracted from `text.rs`, `edit.rs`, `links.rs`,
  `selectable.rs`):** Route all `Layout<Ink>` readers through a single-layout `TextLayout`
  wrapper first and lower `text.rs`/`edit.rs` ceilings.
  - *Gate:* Zero `layout_text_cost` regression; full text/IME/selection test suite green;
    `scripts/file-size.sh` passes with lowered ceilings.
- **PR 2b (`text/composite.rs`):** Enable per-`\n` paragraph slicing, per-paragraph shape/width
  caching, and binary-searched line/byte/y lookup inside `TextLayout`.
  - *Gate:* `edit` (50 KB) `cpu_ms` `15.5 → ≤ 1.2 ms` (p95 `≤ 1.6 ms`), `shaped = 1` per
    keystroke, `edit` live heap `19.2 → ≤ 10.5 MB`; `long_note` 200 KB typing `45.7 → ≤ 1.8 ms`.
- **PR 2c (`edit/history.rs` extracted from `edit.rs`):** Replace full-string undo snapshots
  with `(range, removed, inserted_len)` + mark/block deltas.
  - *Gate:* `long_note` retains 100/100 undo steps at 50 KB and 200 KB within the 1 MB cap.

### Phase 3: Path table, compact `BoxStyle`/`Node`, and flattened `memo` (4 PRs, ~1,250 LOC)

- **PR 3a (`runtime/path_table.rs`, `flatten.rs`, `el.rs`, `memo.rs`):** Add `has_virtual: bool`
  on `El`/`Shared` to skip `window_tree` when false; replace per-frame `node_path` formatting
  with a retained `(NodeId, Segment) → (NodeId, Rc<str>, LayoutEntry)` `PathTable`.
  - *Gate:* `stack` `allocs` drops `≥ 20,000/frame` (`≤ 25,000`), `md-same` `allocs ≤ 5,700`,
    `stack` `cpu_ms` drops `≥ 1.5 ms`.
- **PR 3b (`el/style.rs` extracted from `el.rs`, `flatten.rs`, `Cargo.toml`):** Replace inline
  `taffy::Style` on `El` with 88 B `Copy` `BoxStyle`; materialize `taffy::Style` only on style
  change; drop unused `content_size` (and `block_layout` if unused).
  - *Gate:* `taffy::*` sample share in `stack` `< 0.5%` (down from 5.6%); `El::child` allocated
    bytes/frame drop `≥ 50%`.
- **PR 3c (`el.rs`, `runtime/node.rs`, `runtime/paint.rs`):** Group cold handler/scroll/motion/
  visual fields into `Cold<T>` on `El` and `Node`; add `subtree_end: u32` on `Node` and skip
  offscreen subtrees inside scroll clips during `paint`.
  - *Gate:* `stack` live heap `≤ 30 MB` (`Node ≤ 0.7 KB/node`), `paint_ms` `1.09 → ≤ 0.35 ms`,
    un-memoized `stack` `cpu_ms ≤ 4.5 ms`.
- **PR 3d (`memo.rs`, `flatten.rs`, `runtime.rs`):** Reuse flattened `Node` slices and retained
  Taffy `NodeId` subtrees on `memo` hits, skipping `flatten` and clean interior `apply`.
  - *Gate:* Per-card `memo` on `stack` reaches `cpu_ms ≤ 0.8 ms` on a switch flip; `md-same`
    with `memo` reaches `≤ 0.30 ms`.

### Phase 4: Cold-open scaling and single-pass layout cleanups (3 PRs, ~850 LOC)

- **PR 4a (`text/resolved.rs`, `text.rs`):** Add `ResolvedStyle` cache, switch `ranged_builder`
  to `style_run_builder`, reuse `LayoutContext` (cleared) on secure text, and eliminate the
  `trak` mismatch double build.
  - *Gate:* `md-open` `cpu_ms` drops `≥ 1.5 ms` and `allocs` drops `≥ 3,500`; mixed-font `trak`
    strings shape once (`shaped = 1`).
- **PR 4b (`virtual_list.rs`, `runtime/caret.rs`, `rune-kit/src/markdown/`):** Fix virtualized
  field caret reporting and `scroll_into_view` typing; virtualize `Markdown` top-level blocks
  above a block threshold while preserving `markdown/find.rs` and `virtual_selection`.
  - *Gate:* `md-open` `cpu_ms` `18.3 → ≤ 6.0 ms` at 50 KB and `≤ 6.5 ms` at 200 KB (down from
    70 ms); 200 KB `md-resize` `23.2 → ≤ 2.0 ms`.
- **PR 4c (`runtime/flatten.rs`, `runtime/anchor.rs`):** Move childless `aspect_fit` into
  Taffy's measure callback and scope `fit_anchored` re-layout to `overlay_id`.
  - *Gate:* `aspect_fit` media-grid and overflowing `anchored_fit` menu scenes run 1 root
    `compute_layout` pass instead of 2 or 3 (`Phase::Layout` `−40%+` on those scenes).

### Phase 5: Re-measure and check §1 / §2 triggers

Re-run `layout_text_cost` across `--mode time`, `--mode sample` and `--mode heap`. Start the
native flex solver (§5.3) or `parley_engine` line breaker (§5.4) only if a workload still trips
the §1 or §2 trigger.

## Spike results

Throwaway code on branch `spike/engine`: `research/engine-spike/paragraph_stack.rs`, compiled as
a test module of `rune` (it needs `layout_blocks_inner` and `test_alloc`). Timings use
`--profile perf` with `debug-assertions` on for the `rune` crate only, because its lib tests need
them. Parley and HarfRust build at plain perf settings. Modes alternate order on every keystroke
or round, and the numbers are medians.

### Q1: one cached layout per `\n` paragraph, stacked

**Equivalence.** The matrix is every `rewrap_tests` case plus Arabic followed by digits. Each case
is joined as `[case, plain, empty, case]`, `[plain, case]`, and both of those again with a
trailing `\n`. Every join runs at 1x and 2x, at widths 1200, 433.5, 210 and unbounded: 384 runs
per mode. A run is exact when every line box, run, glyph and pill box matches within 1e-3 px.

| Stacking rule | Exact runs |
| --- | --- |
| Each paragraph laid out alone, tops summed | 264 / 384 |
| + every paragraph forced to the text's base direction | 318 / 384 |
| + line tops rounded from the running total, unbounded width aligned to the widest paragraph | **384 / 384** |

The base rule is that each paragraph keeps its `\n`, and every paragraph but the last drops the
empty line after it. With that rule, trailing newlines, empty lines and their heights never
caused a mismatch. Three things did:

1. **Base direction.** Parley sets one base level per layout, from the first strong character.
   A paragraph laid out alone takes its own: a Latin line after Arabic goes LTR instead of RTL,
   which moves its alignment and run order. This affected RTL, RTL justified, Arabic then
   digits, end and centred. Fix: pass the whole text's direction to each paragraph's builder.
   Bidi reordering never crosses a `\n`, so nothing else leaked between paragraphs.
2. **Line-top rounding.** Parley rounds each line's top from the unrounded sum of the line
   heights above it. A fractional line height (monospace tabs at 32.5 px, pill lines at 1x)
   shifts the lines after it by 0.5 px when a paragraph restarts from a rounded top. Fix: round
   each line top from the running unrounded total, not the paragraph's own top.
3. **Unbounded width with centre, end or RTL alignment.** One layout aligns every line in its
   widest line; a paragraph alone aligns in its own widest line. Fix: shift each paragraph by
   (widest paragraph − its width) × the alignment share. When the widest paragraph changes, the
   other paragraphs move but nothing is reshaped.

**Typing.** The `edit` workload is a 50 KB note in 695 paragraphs, with one character typed mid-note
per keystroke at 1392 px width and 2x scale. Each mode ran 480 keystrokes:

| Mode | cpu ms median | p95 | Parley heap after open | after 480 keys |
| --- | --- | --- | --- | --- |
| whole (one layout) | 15.2 | 16.7 | 5.39 MB | 5.46 MB |
| naive (every paragraph through the cache) | 0.23 | 0.31 | 4.03 MB | 7.21 MB |
| incremental (only the edited paragraph) | **0.10** | **0.14** | **4.03 MB** | 7.22 MB |

Across three runs, incremental stayed between 0.10 and 0.18 ms median and between 0.14 and
0.26 ms p95. That is about 150× faster than one layout, and the gate is 1.2 ms. The live paragraph
layouts take 4.03 MB. Heap here means live growth from opening, which is not how research 31
counted its 13.8 MB.

The growth to 7.2 MB comes from the cache's stale copies of the edited paragraph. Clearing the
cache frees 3.18 MB while the live paragraphs stay. The stale budget (`STALE_TEXT`, 64 KB) is
charged in text bytes plus 64 per entry, but a 605-byte paragraph's layout holds about 31 KB.
So about 100 superseded copies fit, and the total settles near 64 KB × ~50.

**Verdict: go.** Stacking matches exactly with the three rules above, and typing is 0.1 ms. The
heap gate holds only if the layout cache charges stale entries by layout heap, or an edited
paragraph's old layout is dropped instead of kept as stale.

### Q2: per-word `parley_engine` shaper cache versus Parley paragraph shaping

The first ~50 KB of each file, cut at a newline. Inter 15 pt for the Markdown files and IBM Plex
Mono 13 pt for `runtime.rs`, at 2x and 1392 px. The cache is a word-keyed map of shaped glyphs;
a miss shapes the word alone. A greedy breaker splits lines at spaces. Each row is the median of
20 rounds, with a cold cache every round.

| File | tokens | distinct | hit rate | Parley ms | cache ms | + analysis | speedup | with analysis | warm cache |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| PLAN.md | 7,480 | 3,136 | 58.1% | 14.36 | 7.68 | 9.21 | 1.87× | **1.56×** | 8.9× |
| CHANGELOG.md | 7,231 | 2,980 | 58.8% | 14.20 | 8.26 | 9.73 | 1.72× | **1.46×** | 9.0× |
| runtime.rs | 6,252 | 2,661 | 57.4% | 6.30 | 2.69 | 4.02 | 2.34× | **1.57×** | 4.1× |

On real text only 57–59% of words repeat within 50 KB, against 89% for the synthetic note's 772
distinct words. "+ analysis" adds the per-paragraph `Analyzer` pass that any real breaker needs
for break opportunities, bidi and script, and with it the cache wins only 1.5×. "Warm cache"
means every word was already shaped, as on a re-wrap. That case is not the cache's to win: Parley
re-breaks an existing layout without reshaping, and per-paragraph caching already makes typing
0.1 ms.

The cache side is also an upper bound. It uses one font with no fallback, a fixed advance for
spaces, no kerning or ligatures across spaces, and breaks only at `' '`, not UAX #14.

**Verdict: kill** the custom breaker. A cold cache wins 1.5×, below the 2× bar, and the case
where it wins big (a warm cache) is already cheap with Parley plus per-paragraph layouts.
