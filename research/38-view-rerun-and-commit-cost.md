# 38. View re-run and commit cost

Follows [37](37-what-to-own-next.md) on `dev` after `43a1a469`, `154e4d33`, `9beba394`, `bdf0507d`,
`8d3d3f72`, `fc9a3d07`, `8a14dda8` and `622890c4` had already landed (`e6371c48`). Those commits
moved `memo`-reused slices on normal rebuilds, split cold fields out of `Node` and `Visual`, and
replaced Taffy's map storage with dense slot tables. Two costs remained for apps that don't wrap
every subtree in `memo`:

1. **Re-running the whole view and rebuilding static subtrees.** Scrolling an `El::virtual_rows`
   list (or filling its overscan) re-ran the app's `view` closure and rebuilt every mounted row's
   `El` tree even when app state hadn't changed. And `Markdown::render` parsed and built its entire
   block/inline `El` tree on every commit unless the caller wrapped it in `memo`, while leaving
   `reveal = 0.0` on static paragraphs so `apply` populated `reveals` and rejected `clean_apply_spans`.
2. **Copying and diffing unchanged commit state.** Resize commits (`relayout`) still cloned every
   `Node` (`Node::relaid_from`), adjacent clean `memo` spans stayed uncoalesced, truncated and
   line-clamped labels ran Parley binary-search shaping probes on every commit, non-memo `Node`
   paths allocated fresh `String`s every commit, and `apply` ran ~20 `Scene` / `AnimationStore`
   hash lookups on every non-memo node even when its layout, visuals and tracks were at rest.

## What landed

1. **Partial `virtual_rows` remount (`CommitWhy::VIRTUAL`).** When `!relayout && state_unchanged`
   and no pending reveal or layout/caret read invalidates the view, `remount_virtuals` moves
   non-list subtrees and staying rows straight out of `self.nodes`, calls the row closure only for
   newly entering indices, and falls back cleanly to a full `view` pass if a layout read of a
   culled or newly mounted row moves. `MemoCache` tags entries with `virtual_row` so view-level and
   staying-row memos survive partial commits while culled row memos evict immediately.
2. **Automatic `Markdown::render` subtree caching.** Pure-data `Markdown` renders (not streaming,
   no link/task/highlighter closures, no source-copy selection or Mermaid blocks) cache their built
   and flattened subtree in `MemoCache` by a 128-bit `(FxHasher, DefaultHasher)` digest and pass
   ordinal, preserving `el.key` (`None` when unkeyed, or `.key(..)` attached after `.render(..)`).
   Static `Markdown::new` defaults `reveal` to `-1.0` so non-streaming Markdown skips `reveals` and
   qualifies for `clean_apply_spans`.
3. **Move nodes on resize commits and coalesce clean apply spans.** `relayout` moves non-input,
   non-secure `Node`s out of `self.nodes` via `Node::moved_stub` (restoring them if a virtual
   window moves and forces a full `view` pass), and `clean_apply_spans` merges contiguous spans.
4. **Elision cache, in-place node path moves, and resting non-memo `apply` skip.** `TextSystem`
   caches `El::truncate` and `El::max_lines` cuts (`CachedElision`) with the same two-commit +
   `STALE_BYTES` eviction and `supersede` invalidation as shaped layouts (bypassed for `secure`
   text) and reuses probe buffers and the winning binary-search probe. Committing flatten passes
   move `old.path` `String`s directly from matching `self.nodes` slots (restored on retry or
   `discard_measured`, as opposed to [37](37-what-to-own-next.md)'s dropped string-pool spike, this
   also skips the `path_ids` hash lookup when `old_cursor` stays in step). Finally,
   `Scene::is_resting_at` checks presence, frame and active tracks in one slot lookup so `apply`
   skips unchanged resting non-memo nodes, and `commit_paint` short-circuits `!instant && from == Some(to)`
   before querying `AnimationStore`.

## Why arbitrary view helpers still need `memo`

Rust evaluates arguments eagerly: in `parent.child(helper(state, cx))`, `helper` has already run
and built its `El` tree before `parent.child` sees anything, and `&S` is a plain shared reference
with no field-read tracking. Skipping arbitrary `fn(&S, &mut Cx<S>) -> El` helpers without `memo`
would require either reactive state handles or a view macro/compiler rewrite. Automatic skipping in
plain Rust therefore covers the two boundaries Rune already owns: `virtual_rows` closures when `&S`
didn't change, and pure-data component builders (`Markdown::render`).

## Headless numbers, before → after

Built with `--profile perf` and `rune-kit/markdown,frame-profile`, headless at scale 2 on an idle
Apple Silicon worker. Paired over 7 alternating rounds (`A B`, `B A`, …) with outliers kept,
compared by median. Headless figures leave out GPU upload, Metal/wgpu submission, window-server
events and presentation.

### `layout_text_cost` (`e6371c48` → after)

| Case | Commit p50 → after (ms) | Commit p95 → after (ms) | View p50 → after (ms) | Apply p50 → after (ms) | Allocs p50 → after |
|---|---:|---:|---:|---:|---:|
| `md-same` | 0.851 → 0.167 (−80.4%) | 0.952 → 0.186 (−80.5%) | 0.359 → 0.021 (−94.1%) | 0.201 → 0.038 (−81.0%) | 5,687 → 10 (−99.8%) |
| `memo-chat` | 0.549 → 0.340 (−38.0%) | 0.621 → 0.385 (−38.0%) | 0.072 → 0.071 (−1.7%) | 0.241 → 0.086 (−64.3%) | 1,047 → 713 (−31.9%) |
| `md-beside:commit` | 1.218 → 0.818 (−32.9%) | 1.418 → 0.984 (−30.6%) | 0.227 → 0.030 (−86.7%) | 0.223 → 0.111 (−50.3%) | 4,637 → 1,788 (−61.4%) |
| `list:commit` | 0.540 → 0.375 (−30.6%) | 0.599 → 0.419 (−30.1%) | 0.073 → 0.006 (−91.8%) | 0.140 → 0.089 (−36.0%) | 2,048 → 853 (−58.3%) |
| `md-virtual:commit` | 0.855 → 0.620 (−27.5%) | 1.046 → 0.793 (−24.2%) | 0.151 → 0.045 (−70.2%) | 0.136 → 0.069 (−49.4%) | 2,916 → 1,228 (−57.9%) |
| `stack` | 3.962 → 3.254 (−17.9%) | 4.735 → 4.011 (−15.3%) | 1.355 → 1.352 (−0.2%) | 1.278 → 0.788 (−38.4%) | 25,877 → 20,594 (−20.4%) |
| `tree` | 3.216 → 2.700 (−16.0%) | 3.728 → 3.244 (−13.0%) | 0.920 → 0.913 (−0.7%) | 1.163 → 0.733 (−37.0%) | 16,704 → 11,740 (−29.7%) |
| `stack-same` | 3.651 → 3.186 (−12.7%) | 4.365 → 3.861 (−11.5%) | 1.340 → 1.344 (+0.3%) | 1.035 → 0.773 (−25.4%) | 25,806 → 20,523 (−20.5%) |
| `vlist-resize` | 0.452 → 0.432 (−4.5%) | 0.498 → 0.480 (−3.6%) | 0.000 → 0.000 | 0.072 → 0.060 (−16.8%) | 2,236 → 1,747 (−21.9%) |
| `md-resize` | 1.822 → 1.742 (−4.4%) | 1.979 → 1.903 (−3.8%) | 0.000 → 0.000 | 0.265 → 0.230 (−13.2%) | 8,335 → 6,308 (−24.3%) |
| `md-open` | 17.674 → 17.543 (−0.7%) | 18.371 → 18.571 (+1.1%) | 0.456 → 0.481 (+5.5%) | 0.131 → 0.117 (−10.6%) | 47,253 → 48,266 (+2.1%) |
| `edit` | 18.980 → 19.009 (+0.2%) | 19.421 → 19.410 (−0.1%) | 0.007 → 0.007 (0.0%) | 0.053 → 0.052 (−1.7%) | 1,153 → 1,150 (−0.3%) |

### `scroll_bench` (`e6371c48` → after)

| Workload | Frame CPU p50 / p95 (ms) | Commit CPU p50 / p95 (ms) | View p50 / p95 (ms) | Apply p50 / p95 (ms) | Allocs p50 / p95 |
|---|---:|---:|---:|---:|---:|
| `card` | 2.010 → 1.268 (−36.9%) / 2.289 → 1.417 | 1.491 → 0.760 (−49.0%) / 1.700 → 0.853 | 0.513 → 0.301 (−41.3%) / 0.607 → 0.339 | 0.486 → 0.167 (−65.6%) / 0.551 → 0.196 | 9,327 → 3,504 (−62.4%) / 9,380 → 3,557 |
| `growth` (commits every 12th frame) | 0.478 → 0.481 / 2.315 → 2.047 (−11.6%) | 0.000 / 1.846 → 1.585 (−14.1%) | 0.000 / 0.356 → 0.355 | 0.000 / 0.595 → 0.395 (−33.6%) | 6 / 10,272 → 8,239 (−19.8%) |
| `list`, `markdown`, `prose`, `sticky` (non-committing scroll) | within ±1.2% | 0.000 → 0.000 | 0.000 → 0.000 | 0.000 → 0.000 | unchanged |

## Tried and dropped

- **Moving `Node`s on `!same_path` `memo` reuse.** When a reused `memo` subtree shifts to a new
  parent path (`!same_path`), `old.id != new_id`. Replacing `old_nodes[flat.start + k]` with
  `Node::moved_stub()` causes `apply`'s retirement loop (`if old.is_moved_stub() { forget(old); continue; }`)
  to skip `Scene::remove(old.id)` whenever no sibling took `old`'s former path, dropping exit
  transitions on the old slot. Kept `Node` moves strictly to `same_path` `memo` reuse, partial
  `virtual_rows` remount, and `relayout`, where `old.id == new_id` is in `self.node_index`.
- **Keeping `n.text_layout` attached across `relayout`'s `solve`.** Clearing `n.text_layout = None`
  before `solve` drops the node's `Rc<Layout<Ink>>` so `TextSystem`'s cache holds the sole
  strong reference (`Rc::strong_count == 1`), letting width changes re-break shaped runs in place.
  Fixed-size wrapped text (`w_full().h(..)`) skips Taffy's `measure_leaf`, so keeping `n.text_layout`
  across `solve` leaves `strong_count == 2` at `lay_out_text` and forces a clone of the shaped runs
  on resize (`resizing_rebreaks_kept_texts_in_place`). Dropped.
- **Two-step `get_key_value` lookup in `MemoCache` to avoid `Rc<str>` construction on nested hits.**
  Doing `get_key_value(name)` followed by `get_mut(&existing_id)` added a second hash lookup on
  every top-level `memo` hit and regressed `memo-chat` `view_ms` by ~11%. Storing `id: Rc<str>` on
  `Entry` itself kept a single `get_mut(name)` lookup while still avoiding `Rc<str>` allocation
  when nested.
