# Overlay GPU cost: Rune vs hand-written Metal

Fixture: `crates/rune/examples/overlay_hud_cost.rs`, a 360×80 overlay whose compositor clears
the drawable in one render pass at 120 Hz with an empty Rune view. The comparison is
`research/overlay-cpu/metal_hud.swift`, the same panel cleared by hand from a CVDisplayLink
callback with one command buffer per frame.

Metric: retired instructions per frame for the whole process (`proc_pid_rusage`), because the
test Mac moves between a fast and a slow CPU mode that changes CPU time by 2× but not
instruction counts. CPU percentages are from paired slow-mode runs. Runs alternate (3 or 4
rounds), 8 to 10 s each after a 2 s warmup, on a real window.

## Per frame

| | Rune | Rune with the wgpu patches | Metal |
| --- | ---: | ---: | ---: |
| Command buffers committed | 6 | 2 | 1 |
| Completion handlers | 2 | 1 | 0 |
| Render encoders, presents | 1, 1 | 1, 1 | 1, 1 |
| Heap allocations | 65.1 | 55.1 | – |
| Kinstructions | ~1,555 | ~1,118 | ~850 |
| Process CPU | ~19.5% | ~16.0% | ~11.2% |

Rune submits once per frame and, with no writes, its staging submits and recycles nothing.
The six command buffers are wgpu's:

1. `(wgpu internal) Transit`: wgpu-core's barrier buffer for the submitted command buffer. Empty on Metal.
2. `(wgpu internal) Pre Pass`: the pass's barriers. Empty on Metal.
3. The render pass.
4. The surface texture's transition to PRESENT at `Queue::submit`, which gets the fence signal and a completion handler. Empty on Metal.
5. `Queue::present` → `prepare_surface_texture_for_present` submits again with a second signal and handler. That's a PRESENT→PRESENT barrier: no backend lists PRESENT among the usages that skip barriers.
6. wgpu-hal's present, which presents the drawable from a command buffer of its own.

wgpu 30.0.1 is the latest release, and wgpu's main branch has the same code, so an upgrade
doesn't help. Two small patches do (`research/overlay-cpu/*.patch`, against 30.0.1):

- `wgpu-core-present-skip.patch`: when the texture needs no clear and every pending transition
  is from PRESENT, return without a submission. This affects every backend: Vulkan also leaves
  PRESENT out of its ordered usages.
- `wgpu-hal-metal-lazy-command-buffer.patch`: a Metal command encoder makes its MTLCommandBuffer
  on first use, so an encoding that records nothing commits nothing. The submit signals on the
  last non-empty buffer. This is a prototype. Before sending it upstream, `raw_command_buffer()`
  should make the buffer for `as_hal` callers, and the strict-event-sync wait value should be
  read at `begin_encoding`.

With the Metal HUD made to do what wgpu does, the gap is driver work per command buffer:

| Metal HUD shape | Kinstructions |
| --- | ---: |
| 1 command buffer, present on it | ~848 |
| Present from its own command buffer, 1 handler (the patched shape) | ~929 |
| 2 extra empty buffers, split present, 1 handler | ~1,105 |
| 4 extra empty buffers, split present, 2 handlers (wgpu's shape) | ~1,213 |

## By thread (8 s, both processes at once, ms)

| | Driver and dispatch threads | Present thread | Main | Display link | Total |
| --- | ---: | ---: | ---: | ---: | ---: |
| Rune, patched | 650 | 205 | 183 | 21 | 1,059 |
| Metal | 468 | – | 3 | 296 | 767 |

- Driver threads: `_submitAvailableCommandBuffers` (271 ms), completion notifications
  (~100 ms), drawable presents (~50 ms). This cost scales with command buffers and handlers.
- Present thread: wgpu-hal into Metal 134 ms (commits, present), wgpu-core 43 ms, Rune ~1 ms.
- Main thread, standalone 10 s profile, patched:
  - `acquire` 154 ms, of which `nextDrawable` 125 ms. Metal's display link spends the same on
    `nextDrawable`.
  - Rune's own work is about 105 ms: `frame::step` 46 (`step_motion` 34, the empty view's
    `build` 23), `about_to_wait` 22 (pointer observe 7), `render_frame` 18, `submit_and_present`
    11 (the hand-off send 9), `PresentWorker::wait` 5, occlusion check 3.
  - wgpu's `create_view` on the surface texture is 10 ms.
- The app's own work, which Metal does on its display link, is not in these runs.

## Writes

A frame that writes a buffer costs about 310 kinstructions more, whether it uses `Queue::write_buffer`
or `Frame::write_buffer`. Either way wgpu-core records the copy in a command buffer of its own
(`PendingWrites`, or the encoder's own buffer ahead of the pass), with a blit encoder.

| Writes per frame | Queue | `Frame::write_buffer` |
| --- | ---: | ---: |
| 0 | ~1,558 | – |
| 1 | ~1,863 | ~1,881 |
| 4 | ~1,966 | ~1,918 |

Staging maps its chunk once per frame however many writes share it, so each extra write costs
~12 kinstructions against ~35 for a queue write.

## Rune's own frame

Each change stacks on the one before. The fixture also prints the main thread's instructions
(`thread_selfcounts`), which isolate Rune's share from the driver threads. Medians of 6
alternating rounds, 10 s each, no writes, unpatched wgpu:

| Build | Main kinstructions | Main µs | Process kinstructions | Present thread ms/s |
| --- | ---: | ---: | ---: | ---: |
| Before | 161.9 | 149 | 1,395 | 17.0 |
| Claim the display link's frame before waiting on the present thread | 153.3 | 134 | 1,398 | 16.4 |
| Skip the pointer sample when nothing reads it | 148.3 | 127 | 1,388 | 15.5 |
| Read occlusion once a turn | 144.2 | 118 | 1,384 | 15.3 |
| Leave a still runtime alone while only the compositor animates | 139.2 | 102 | 1,371 | 14.9 |
| Skip the staging when nothing is staged | 140.8 | 109 | 1,375 | 13.7 |

- The main thread does 13% fewer instructions. The process total moves by about as much as the
  run-to-run spread (±50 kinstructions), because driver threads dominate it.
- A Time Profiler run on the main thread finds `frame_step` and `step_motion` (14 of 117 samples
  before) gone once the runtime rests. What's left is `acquire`, mostly `nextDrawable`.
- Allocations stay at 65.1 a frame: they are wgpu's and Metal's, not Rune's.

## Writes with immediates

A compositor that asks for `Features::IMMEDIATES` gets the adapter's immediate size (4 KB on
Metal), so it can hand a frame's uniforms to `RenderPass::set_immediates` instead of writing a
buffer. Three writes a frame, same rounds:

| Path | Process kinstructions | Main kinstructions | Allocations | Command buffers, blit encoders |
| --- | ---: | ---: | ---: | ---: |
| `Queue::write_buffer`, before | 1,708 | 277 | 82.1 | 7, 1 |
| `Queue::write_buffer`, after | 1,696 | 258 | 82.1 | 7, 1 |
| `Frame::write_buffer`, before | 1,679 | 166 | 85.1 | 7, 1 |
| `Frame::write_buffer`, after | 1,650 | 144 | 85.2 | 7, 1 |
| Immediates | 1,405 | 148 | 71.1 | 6, 0 |

Immediates cost what a frame with no writes costs: no copy, so no command buffer and blit
encoder for it. A uniform ring in `MAPPABLE_PRIMARY_BUFFERS` memory was not tried. It would also
skip the copy, but it needs its own fencing per frame in flight, and it isn't available on every
backend.

## What's left

- Ship the wgpu fix. A `[patch]` in Rune's workspace doesn't reach apps that pin Rune. A git
  dependency on a wgpu fork would, but every app that names `wgpu` itself would have to follow
  it. Upstreaming both patches is the clean route.
- Without wgpu changes, Rune could acquire drawables from the CAMetalLayer itself, wrap each
  texture with `create_texture_from_hal`, and present on its own command buffer after the frame's.
  That would remove buffers 4 to 6 and one handler, about 110 kinstructions of driver work by the
  Metal HUD's measure plus wgpu-core's present path. In exchange, Rune would bypass wgpu's
  surface on macOS.
- Rune's main thread is now about 10% of the process's instructions, most of it `acquire`. The
  rest of the cost is wgpu's command buffers and Metal's driver threads.
