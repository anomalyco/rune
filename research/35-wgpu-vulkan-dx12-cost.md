# 35. What wgpu costs a Rune frame on Vulkan and DX12

Reader: someone deciding whether Rune should own its Vulkan and DX12 backends, as
[34](34-own-gpu-plan.md) decided for Metal. Question: per frame, what does `wgpu` 30.0.1 cost on
Linux (Vulkan) and Windows (DX12) compared with the same frame written against the raw API, and how
much of that does the upstream present-skip patch already remove?

**Answer: a native backend would save about 70k instructions and 0.06 to 0.09 ms of CPU per window
frame, under 1.1% of a 120 Hz frame.** The present-skip patch takes about a quarter of the
instructions and, on Vulkan, more than half of the time, because it removes a whole
`vkQueueSubmit`. On a normal window frame, where Rune's own work dominates, `wgpu` and the driver
are about 10% of the renderer's instructions. That backs [34](34-own-gpu-plan.md): keep `wgpu` on
Vulkan and DX12 and land the patch upstream. A 15 to 20 kLOC backend buys less than 0.1 ms a frame.

All numbers come from software drivers (Mesa lavapipe, and vkd3d on top of it under Wine), so
driver instructions and every time below hold only for software drivers. API call counts, and the
instructions spent in `wgpu`, `wgpu-core` and `wgpu-hal`, don't depend on the GPU.

## Setup

- 8-vCPU AMD EPYC 9554P (Zen 4) VM, Ubuntu 24.04, no GPU. Mesa 25.2.8 lavapipe (Vulkan 1.4), Xvfb
  with no vsync, so frames run uncapped.
- DX12: the same probe binaries cross-compiled to `x86_64-pc-windows-gnu` and run under Wine 9.0,
  whose `d3d12.dll` is vkd3d over lavapipe. Every D3D12 call is translated to Vulkan, so DX12
  instructions include the translation.
- `wgpu` 30.0.1 with `vulkan` and `dx12`, as Rune builds it off Apple. "Patched" adds
  `research/overlay-cpu/wgpu-core-present-skip.patch`.
- Workloads:
  - `overlay_hud_cost` ([30](30-overlay-gpu-cost.md)): Rune's real overlay. Its compositor clears
    a 360×80 drawable in one pass, with `OVERLAY_WRITES=0` or `1` queue writes per frame.
  - `research/gpu-floor` (built from that directory, outside the workspace):
    - `wgpu_frame`: the same frame through bare `wgpu` (acquire, `create_view`, N 256-byte
      `Queue::write_buffer`s, one cleared pass, submit, present).
    - `vk_floor` (ash) and `dx12_floor` (`windows`): that frame with the fewest calls each API
      allows. `UPLOAD=mapped` writes the uniform into a persistently mapped ring with no API call.
      `UPLOAD=copy` stages it and records a copy plus one barrier, as a discrete GPU needs.
  - `upload_bench` (steady and scroll): a headless window-sized renderer frame
    (`Renderer::prepare`, `draw`, `submit`, with no surface), standing in for a normal window.
- Instructions per frame are for the drawing thread only. Lavapipe rasterizes on its own threads,
  and on real hardware that work is the GPU's, so those threads are left out.
  - Callgrind collects only inside the frame function, Rune's `Compositor::render` or Rune's
    renderer calls. `research/gpu-floor/cgsplit.py` then splits the counts by crate and
    library, charging `std` and allocator code to its caller.
  - `perf_event` instruction counters and `CLOCK_THREAD_CPUTIME_ID` give the user, kernel and CPU
    time numbers: medians of 5 alternating rounds of 4,000 frames.
- Barriers were classified with a logging build of `wgpu-hal`
  (`research/gpu-floor/wgpu-hal-log-barriers.patch`) on Vulkan and with vkd3d's trace log on
  DX12.

## Vulkan, per frame

| HUD frame, 0 writes (1 write) | wgpu today | wgpu patched | Native floor |
| --- | ---: | ---: | ---: |
| `vkQueueSubmit` | 2 (2) | 1 (1) | 1 (1) |
| Command buffers begun and ended | 5 (6) | 4 (5) | 1 (1) |
| `vkResetCommandPool` | 2 (3) | 1 (2) | 1 (1) |
| `vkCmdPipelineBarrier` calls | 4 (5) | 2 (3) | 0 (copy: 1) |
| Barriers, of which no-ops | 4, 2 (6, 3) | 2, 0 (4, 1) | 0 (copy: 1, 0) |
| Image views created and destroyed | 2 (2) | 2 (2) | 0 |
| Framebuffers created and destroyed | 1 (1) | 1 (1) | 0 |
| Buffers created, bound, destroyed | 0 (1) | 0 (1) | 0 |
| Host semaphore or fence calls | 2 (2) | 2 (2) | 2 (2) |
| Heap allocations, `wgpu` crates | 81 (91) | 60 (70) | 0 |
| Heap allocations, driver | 57 (67) | 44 (54) | 29 (29; copy 37) |
| Kinstructions, `wgpu` + `wgpu-core` + `wgpu-hal` | 53.7 (62.3) | 43.0 (50.0) | 0 (probe 0.2) |
| Kinstructions, driver (software) | 34.1 (38.3) | 26.4 (30.6) | 16.9 (17.0; copy 19.2) |
| Kinstructions, thread total (callgrind) | 88.5 (101.3) | 70.0 (81.3) | 17.1 (17.2; copy 19.5) |
| Kinstructions user + kernel (`perf`) | 106.6 (119.0) | 81.3 (92.1) | 30.5 (30.8; copy 33.7) |
| Thread CPU, µs (VM, software driver) | 126 (133) | 89 (92) | 67 (70; copy 74) |

Split of the 53.7 kinstructions on the `wgpu` side: `wgpu-core` 36.8, `wgpu-hal` 14.5, `wgpu`
2.4. Inclusive cost by entry point in Rune's overlay, including driver time: `Queue::submit` 32.4,
`Queue::present` 27.2 (8.2 patched), `CommandEncoder::finish` 16.5, `get_current_texture` 7.3,
`create_view` 3.3, `begin_render_pass` 2.3.

What the floor drops, and why each call is avoidable:

- **The second submit.** `Queue::present` submits a `PendingWrites` command buffer that holds only
  PRESENT→PRESENT barriers, because Vulkan's ordered texture usages are `INCLUSIVE` only
  (`wgpu-hal/src/vulkan/adapter.rs:3247`). `skip_barrier` therefore keeps the barrier
  (`wgpu-core/src/track/mod.rs:341-345`), and `vulkan/command.rs:220,268` skip
  `vkCmdPipelineBarrier` only when the list is empty. The patch removes this submit.
- **Two PRESENT→PRESENT image barriers.** Neither changes the layout (`PRESENT_SRC_KHR` both sides)
  nor orders anything, so both are no-ops.
- **Three empty command buffers.** `Transit`, `Pre Pass` and `Present` each pay a
  `vkBeginCommandBuffer`/`vkEndCommandBuffer` pair. The two barriers that matter,
  UNDEFINED→COLOR_ATTACHMENT and COLOR_ATTACHMENT→PRESENT_SRC, ride in two of them. The floor gets
  both transitions free from the render pass's `initialLayout` and `finalLayout`, with one external
  subpass dependency on the acquire semaphore (`vk_floor.rs`).
- **Two image views and a framebuffer every frame.** `get_current_texture` builds a clear view for
  the surface texture (`wgpu-core/src/present.rs:210-223`), and the app's `create_view` builds the
  second. The framebuffer cache lives in the command encoder and is drained on every `reset_all`
  (`vulkan/command.rs:184-187`), so the framebuffer for that frame's view is rebuilt every frame.
  The floor makes views and framebuffers once per swapchain image.
- **With a write:** each `Queue::write_buffer` creates, binds and destroys a staging `VkBuffer`
  through `gpu-allocator`, and records a copy into `PendingWrites`, the third command buffer
  reset. Its MAP_WRITE→COPY_SRC barrier is a no-op, because `vkQueueSubmit` already makes host
  writes visible (Vulkan spec, "Host Write Ordering Guarantees"). COPY_DST→COPY_DST orders the
  write after last frame's copy into the same buffer and is needed. The floor writes into a mapped
  ring. On a discrete GPU it records one `vkCmdCopyBuffer` and one buffer barrier, which costs
  2.3 kinstructions.

Rune's real overlay agrees with the probe: 105.3 kinstructions per frame, of which Rune is 9.9,
the `wgpu` side 51.5, the driver 39.8 and windowing 4.1. With one write it's 116.7 (58.9 `wgpu`,
43.7 driver). Patched, those drop to 84.5 and 97.9.

## DX12, per frame

Traced through vkd3d, counted between two presents. Thread numbers are for the whole Wine stack.

| HUD frame, 0 writes (1 write) | wgpu today | wgpu patched | Native floor |
| --- | ---: | ---: | ---: |
| `ExecuteCommandLists` | 2 (2) | 1 (1) | 1 (1) |
| Command lists reset and closed | 5 (6) | 4 (5) | 1 (1) |
| Command allocator resets | 2 (3) | 1 (2) | 1 (1) |
| `ResourceBarrier` calls, transitions | 2, 2 (3, 3) | 2, 2 (3, 3) | 2, 2 (copy: 2, 3) |
| No-op transitions | 0 (1) | 0 (1) | 0 |
| `Signal` on the frame fence | 2 | 1 | 1 |
| `CreateRenderTargetView` | 2 | 2 | 0 |
| Placed resource, `Map`/`Unmap` per write | 0 (1) | 0 (1) | 0 |
| Kinstructions user + kernel (`perf`) | 160.4 (200.7) | 137.9 (176.1) | 41.4 (41.7; copy 44.4) |
| Thread CPU, µs (Wine + vkd3d + lavapipe) | 130 (146) | 128 (133) | 61 (61; copy 64) |

- DX12 already sits at the barrier floor. `transition_textures` drops a transition whose states
  match (`wgpu-hal/src/dx12/command.rs:514-516`), so PRESENT→PRESENT never reaches the API, and
  DX12 orders COLOR_TARGET and DEPTH_STENCIL_WRITE (`dx12/adapter.rs:1397-1400`). The two
  PRESENT↔RENDER_TARGET transitions are required for a swapchain buffer, so the floor has them too.
- The one no-op is COMMON→COPY_SOURCE on the write's staging buffer: buffers are promoted
  implicitly from COMMON, and upload-heap resources stay in GENERIC_READ anyway.
- What's left is the same shape as on Vulkan: a second `ExecuteCommandLists` with its `Signal`
  (`dx12/mod.rs:1799-1802`), four extra command list Reset/Close pairs, two RTVs written per frame,
  and a placed staging resource per write.
- The per-crate split wasn't measured on DX12, because callgrind can't follow the Windows binary
  under Wine. `wgpu-core` is the same code on both backends, so its ~37 kinstructions carry over.
  The rest of the 119-kinstruction gap is `wgpu-hal`'s dx12 code plus vkd3d translating its extra
  calls, which real Windows doesn't pay. Treat the DX12 gap as an upper bound.
- The probe uses `wgpu`'s default HWND swapchain. Rune uses `DxgiFromVisual`, which differs only
  in how the swapchain is created, not in the per-frame calls above.

## Normal window frame

`upload_bench` measures a renderer frame with no surface: `prepare`, one pass with one draw, and
submit.

| Kinstructions per frame (Vulkan, callgrind) | steady | scroll |
| --- | ---: | ---: |
| Rune (`prepare`, `draw`) | 426.1 | 554.1 |
| `wgpu` + `wgpu-core` + `wgpu-hal` | 36.8 | 37.1 |
| Driver (software) | 11.8 | 12.3 |
| Total | 474.7 | 603.5 |
| `cpu_us` without valgrind | 56.9 | 72.1 |

Per frame: 1 `vkQueueSubmit`, 4 command buffers, 3 barriers, 1 copy, 1 draw, and 83 heap
allocations in the `wgpu` crates. A native backend would keep roughly the driver part and add a
few kinstructions of its own, so it saves about 35 kinstructions, 6 to 7% of the frame. A window
also pays the surface costs measured on the HUD (acquire, view, present, and the second submit
today), so a normal window saves about the HUD's figure plus a few µs.

## Projected savings

The 120 Hz frame is 8.33 ms. "Measured" means CPU time on the VM with software drivers.
"Projected" assumes the `wgpu` side and the extra API calls run at 2 to 4 instructions per ns on a
desktop core.

| Backend, HUD frame | Today → native | Patched → native | Today → patched |
| --- | ---: | ---: | ---: |
| Vulkan, kinstructions (callgrind) | −71 (−84 with a write) | −53 (−64) | −18.5 (−20) |
| Vulkan, measured | −0.059 ms, 0.71% (−0.063, 0.76%) | −0.022 ms, 0.27% | −0.037 ms, 0.45% |
| Vulkan, projected on hardware | −0.02 to −0.04 ms, 0.2 to 0.5% | −0.015 to −0.03 ms | −0.005 to −0.01 ms, plus one submit's kernel time |
| DX12, kinstructions (`perf`, includes vkd3d) | −119 (−159) | −96 (−134) | −23 (−25) |
| DX12, measured under Wine | −0.070 ms, 0.83% (−0.085, 1.0%) | −0.067 ms, 0.81% (−0.072) | ~0 (−0.013 with a write) |
| Normal window, kinstructions | ≈ HUD figure + 35 | ≈ HUD figure + 35 | as HUD |

For scale, [30](30-overlay-gpu-cost.md) measured a 705-kinstruction gap per frame between Rune
and hand-written Metal, process-wide. The Vulkan gap here is about 10 times smaller in
instructions. A large part of the Metal gap is driver work per command buffer, which lavapipe
mostly hands to its own threads, so real Vulkan and DX12 drivers may cost more per extra submit
than these numbers show.

## What paid hardware would settle

- The real cost of each extra `vkQueueSubmit`/`ExecuteCommandLists`, empty command buffer,
  per-frame view and framebuffer, and staging buffer on NVIDIA, AMD and Intel drivers. A real
  submit goes to the kernel, and lavapipe's doesn't. That's the biggest unknown in the projection.
- DX12 with Microsoft's runtime instead of vkd3d, and Rune's `DxgiFromVisual` swapchain under DWM
  with real vsync and frame latency waits.
- Cheapest option: one AWS `g4dn.xlarge` (NVIDIA T4) in us-east-1 at $0.526/h on Linux, about
  $0.71/h on Windows (license included), or about $0.30/h spot. One hour each, about $1.25 on
  demand, runs every probe here.
- A free middle step is a `windows-latest` GitHub Actions job on WARP, which is Microsoft's runtime
  over a software rasterizer. Pushing a workflow file needs a token with the `workflow` scope, so it
  wasn't tried here.

## Cheaper than a backend

1. Upstream `wgpu-core-present-skip.patch` (one fewer submit, two no-op barriers, a command buffer
   and 20 allocations per frame on every backend).
2. In `wgpu-hal` Vulkan, keep the framebuffer cache across `reset_all`, or use imageless
   framebuffers, and skip same-layout image barriers as DX12 does.
3. In `wgpu-core`, reuse the surface texture's clear view across frames for the same swapchain
   image.
4. In Rune, keep writing through its own staging ring rather than `Queue::write_buffer`, which
   creates a staging buffer per call on Vulkan and a placed resource per call on DX12.

## Reproduce

From `research/gpu-floor` on Linux with Mesa lavapipe and an X server (`DISPLAY`):

```sh
cargo build --release
WRITES=0 ./target/release/wgpu_frame      # BACKEND=vulkan|dx12
WRITES=0 ./target/release/vk_floor        # UPLOAD=mapped|copy
cargo build --release --target x86_64-pc-windows-gnu
BACKEND=dx12 WINE_PERF=1 wine64 target/x86_64-pc-windows-gnu/release/wgpu_frame.exe
WINE_PERF=1 wine64 target/x86_64-pc-windows-gnu/release/dx12_floor.exe
```

For the patched rows, add
`--config 'patch.crates-io.wgpu-core.path="<wgpu-core 30.0.1 with the present-skip patch>"'`.
Callgrind per frame:
`valgrind --tool=callgrind --collect-atstart=no '--toggle-collect=*Draw>::frame' --separate-threads=yes ...`,
then `cgsplit.py <main-thread profile> --frames N`.
