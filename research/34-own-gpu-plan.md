# 34. Should Rune own its GPU layer? Verdict, numbers and plan

Reader: someone deciding whether Rune should replace `wgpu` with its own GPU layer, or choosing
which layer to own on each platform. Re-run `overlay_hud_cost` ([30](30-overlay-gpu-cost.md)),
`upload_bench`, `frame_bench`, `layout_text_cost` ([31](31-layout-text-cost.md)) and
`scripts/motion-audit` at every gate below.

The question: from first principles, should Rune build and own its own GPU backend instead of
paying `wgpu`'s per-frame overhead or waiting on upstream?

**Verdict: yes on Apple (`macOS` and `iOS`), no everywhere else.** Build a ~2,800-line native
Metal backend inside `crates/rune/src/gpu/metal/` on `objc2-metal`, `objc2-quartz-core` and
build-time `naga` (all already in `Cargo.lock`), bridged into `wgpu` 30's `custom` backend
trait (`features = ["custom"]`) plus a direct internal fast path for Rune's own uploads and
present. Keep `wgpu` on Windows (DX12), Linux (Vulkan) and Web (WebGPU), and send three small
bug-fixed patches upstream to `gfx-rs/wgpu` (~120 lines, hand-written descriptions) so Windows
and Linux drop the redundant present submission when upstream ships them.

Why this split wins:

1. **Continuous 120 Hz frames spend 30% to 75% of process CPU in `wgpu` and driver dispatch.**
   That covers HUD overlays, `virtual_rows` scrolling, `gpu_springs` ticks and multi-pass blur
   or lighting frames. Even with both prototype `wgpu` patches from [30](30-overlay-gpu-cost.md),
   any frame that writes a uniform or instance buffer still commits 3 `MTLCommandBuffer`s plus
   a `MTLBlitCommandEncoder` and `unmap`/`map_async` round-trip, costing ~1,428
   kinstructions/frame against ~850 kinstructions/frame in hand-written Metal.
2. **Heavy window commits spend only 1% to 3.5% (~0.15 ms) in `wgpu`.** On `stack` (10.2 ms),
   `edit` (15.5 ms) and `md-resize` (4.35 ms), the commit, motion and paragraph-caching work in
   [32](32-engine-plan.md) must carry the win. Once [32](32-engine-plan.md) brings memoized
   commits and scrolling under 0.3 to 0.8 ms, `wgpu` becomes a major share of normal windows too.
3. **`wgpu::custom` keeps 100% of the public API compiling.** `Compositor`, `Frame`,
   `EffectHook`, `rune-fx`, `rune-light`, `rune-staging`, headless pixel tests and downstream
   apps keep their exact `wgpu::*` types on crates.io `wgpu ~30.0.1` without `[patch.crates-io]`
   (which Cargo ignores in git dependencies anyway).
4. **Owning Metal is small (~2.8 kLOC); owning Vulkan and DX12 is 5 to 7 times larger (~15 to
   20 kLOC) where driver and Wayland/X11 swapchain bugs hurt small teams.** Zed (`zed#46758`)
   dropped `blade` on Linux and returned to `wgpu` in January 2026 while keeping raw Metal on
   macOS; Warp ships the same native-Metal-plus-`wgpu` split; Flutter Impeller's Vulkan backend
   (~21 kLOC) is 3.5 times its Metal backend (~5.9 kLOC).

## 1. Where GPU CPU goes: HUD overlays vs normal windows

### Code checks and corrections to earlier notes

Checking `crates/rune`, `crates/rune-fx`, `crates/rune-light` and `wgpu` 30.0.1 source turned up
five facts that change the options:

1. **Normal macOS windows do not use `PresentWorker`.** Only overlays spawn the `rune present`
   worker (`crates/rune/src/overlay/frame.rs:57`). `crates/rune/src/window/draw.rs:269-275` runs
   `encoder.finish()`, `queue.submit` and `queue.present` synchronously on the AppKit main
   thread. Once Metal submit-and-present folds onto one `MTLCommandBuffer` (~35 µs instead of
   ~214 µs), overlays can drop `PresentWorker` too, saving its thread and two `mpsc` wakeups per
   frame (~14 to 25 ms per 10 s).
2. **Both prototype `wgpu` patches still leave +310 kinstructions/frame on any buffer write, and
   $P + 1$ command buffers on $P$-pass frames.** In `wgpu-core` 30.0.1
   (`command/render.rs:2308-2315`), `encode_render_pass` calls `raw_encoder.close_if_open()`
   ahead of every pass so it can splice a barrier buffer before the pass (`close_and_swap` at
   `:2765`). Any `copy_buffer_to_buffer` recorded by `Staging::flush` (`Frame::write_buffer` or
   `Renderer::prepare`) opens a non-empty blit `MTLCommandBuffer` that `encode_render_pass`
   immediately closes. `wgpu-hal-metal-lazy-command-buffer.patch` only skips *empty* command
   buffers, so every frame with an upload still commits 3 `MTLCommandBuffer`s (blit, render,
   present) plus `unmap`, `map_buffer_on_submit` and channel recycling (+323 kinstructions in
   [30](30-overlay-gpu-cost.md)). Multi-pass frames (`backdrop_blur`, `layer_blur`, `rune-fx`,
   `rune-light`) commit $2P + 4$ command buffers unpatched and $P + 1$ patched (~2.3 MiB of
   transient `IOAccelerator` memory per command buffer, `wgpu#10494`).
3. **`wgpu-hal-metal-lazy-command-buffer.patch` has three bugs.** First, `begin_encoding`
   (`metal/command.rs:487`) increments `command_buffer_created_not_submitted`, and `end_encoding`
   clears `self.begun = false` while returning `CommandBuffer { raw: None }`; if that buffer is
   dropped unsubmitted, `CommandBuffer::drop` (`:1978`) returns early on `None` and leaks a slot
   toward the 2,048 `DeviceError::Lost` limit. Second, `Queue::submit` (`metal/mod.rs:755`)
   unconditionally sets `"(wgpu internal) Signal"` on the last non-empty command buffer, which
   with lazy buffers is the caller's render pass buffer, clobbering its label. Third, moving
   `relay.next_release_value.load(Acquire)` out of `begin_encoding` into lazy `cmd_buf()` races
   concurrent submissions under strict event sync.
4. **`wgpu` 30.0.1 already exposes `wgpu::custom` (`features = ["custom"]`).** Every `wgpu`
   handle dispatches over `Core | WebGPU | Custom` (`wgpu/src/dispatch.rs:728`), with public
   `Instance::from_custom`, `Texture::from_custom` and `as_custom::<T>()`. Both `Core` and
   `Custom` coexist in one binary (`RUNE_GPU_BACKEND=metal|wgpu`). Meanwhile, Rune's full
   surface uses two features a minimal quad renderer would miss: 2-target MRT (`gb0`/`gb1` in
   `crates/rune-light/src/render.rs:108`) and dynamic uniform offsets (`has_dynamic_offset: true`
   in `crates/rune-fx/src/renderer.rs:342`).
5. **`wgpu-hal` alone neither reaches 1 command buffer on Metal nor stays thin on Vulkan/DX12.**
   `hal::metal::Queue::present` (`metal/mod.rs:810`) always commits a separate `Present` command
   buffer, and `hal::metal::Queue::submit` (`:739`) always heap-allocates a `block2::RcBlock`
   completion handler (+81 kinstructions/frame combined in `metal_hud.swift`). On Vulkan and
   DX12, `wgpu-hal` (`lib.rs:48-52`) requires explicit caller-computed `transition_textures` and
   `transition_buffers` barriers, which would force Rune to rebuild `wgpu-core`'s state tracker
   to keep multi-pass effects and user `Compositor`s sound.

### Attribution: HUD overlay (120 Hz, `overlay_hud_cost`)

From [30](30-overlay-gpu-cost.md) (paired runs at 120 Hz, `proc_pid_rusage` retired instructions
and slow-mode process CPU):

- **0-write HUD:** Rune today runs 1,555 kinstr/frame (19.5% CPU, 6 command buffers, 2
  handlers) vs 850 kinstr/frame (11.2% CPU, 1 command buffer, 0 handlers) for
  `metal_hud.swift`. The two `wgpu` patches cut 4 empty command buffers and the second present
  submit (-437 kinstr, to 1,118 kinstr / 16.0% CPU), leaving 268 kinstr (58% of the 8.3-point
  CPU gap). From the 8 s and 10 s thread profiles, that remaining 268 kinstr splits into three
  parts:
  - **~81 kinstr in `wgpu-hal` Metal:** separate `Present` command buffer plus 1 `RcBlock`
    completion handler and driver notification (`929k - 848k` measured on `metal_hud.swift`).
  - **~90 kinstr in `wgpu-core` and `wgpu-hal`:** per-frame `Surface` `Texture`/`TextureView`
    allocations and registry locks (`10 ms/10 s` in `create_view` alone), `ArcCommand` record
    and replay, `Tracker` `set_size`/drop (`43 ms/8 s` on the present thread), 55 heap
    allocations/frame, and duplicate `Fence::get_latest` `status()` polls.
  - **~97 kinstr in Rune's own overlay loop (being fixed separately):** `frame::step` on an
    unchanged empty view (`46 ms/10 s`), `about_to_wait` and pointer polling (`22 ms/10 s`),
    `PresentWorker` handoff/wait (`14 ms/10 s`), and duplicate `surface_occluded` layer walks.
- **1-write animating HUD (`OVERLAY_WRITES=1`, live meters/waveforms):** Rune today runs 1,881
  kinstr/frame (`Frame::write_buffer`) or 1,863 kinstr (`Queue::write_buffer`). Patched `wgpu`
  still lands around ~1,428 kinstr/frame because the blit copy splits out a 3rd command buffer
  and unmaps/remaps the staging chunk every frame. A unified-memory `MTLStorageModeShared` ring
  in a single `MTLCommandBuffer` writes 256 B with one `memcpy` and
  `setVertexBufferOffset`/`setFragmentBufferOffset` (< 2 kinstr).

### Attribution: normal window frames (`upload_bench`, `frame_bench`, [29], [31])

Running `upload_bench` and `frame_bench` (`--profile perf`, headless Metal) gives 54.3 µs CPU
to prepare, encode and submit a 2,000-instance steady frame, 83.0 µs when all 2,000 instances
scroll (256 KiB staged upload), 15.1 µs to record 10 `rune-light` GI passes (before `finish()`
replays them into 20+ command buffers), and 97.9 µs for 16 `rune-fx` layers with 4 backdrop
blurs. Adding main-thread surface acquire (excluding `nextDrawable`'s own wait, which native
Metal shares), `create_view`, `present`'s extra submit and background driver threads puts
avoidable `wgpu` overhead in a 1-pass window at ~0.14 to 0.20 ms/frame (~0.35 to 0.55 ms/frame
with 6 to 10 blur/effect passes):

| Window workload (120 Hz = 8.33 ms) | Rune CPU today ([29], [31]) | Avoidable `wgpu` + driver CPU | `wgpu` share today | Rune CPU after [32] | `wgpu` share after [32] |
|---|---:|---:|---:|---:|---:|
| `edit` (typing in 50 KB note) | 15.50 ms | ~0.16 ms | ~1.0% | ~1.00 ms | ~14% |
| `stack` (5,283 nodes, 1 flip) | 10.20 ms | ~0.16 ms | ~1.5% | ~3.8 ms (< 0.8 `memo`) | ~4% (~17% `memo`) |
| `md-resize` (50 KB width drag) | 4.35 ms | ~0.16 ms | ~3.5% | ~1.40 ms | ~10% |
| `md-same` (unchanged commit) | 1.98 ms | ~0.13 ms | ~6.2% | ~0.7 ms (< 0.25 `memo`) | ~16% (~34% `memo`) |
| `list` scroll (`virtual_rows`) | 0.15–0.31 ms | ~0.17 ms | **35–53%** | ~0.10–0.22 ms | **44–63%** |
| `gpu_springs` (`present_at` tick) | ~0.01 ms | ~0.17 ms | **> 90%** | ~0.01 ms | **> 90%** |
| Window + backdrop blur / `rune-fx` | 0.30–1.50 ms | ~0.35–0.50 ms | **20–60%** | 0.20–0.80 ms | **30–70%** |

## 2. Comparison of options

HUD numbers show the projection after Rune's ~97 kinstr overlay-loop fixes (with pre-fix
numbers in parentheses for 0-write) so GPU layers are compared on equal footing against
`metal_hud.swift` (850 kinstr / 11.2% CPU).

| Option | HUD 0-write / 1-write (kinstr, slow CPU %) | Normal window CPU saved | LOC to write & own | Public API & feature impact | Startup & shaders | Key risks |
|---|---|---|---|---|---|---|
| **Today (`wgpu` 30.0.1)** | `1,458k` (`1,555k` raw) / `1,784k`; ~18.3% / ~22.3% | Baseline (0 ms) | 0 | None; 10 macOS `as_hal` sites depend on `wgpu-core` + `wgpu-hal`. | `uber.metallib` only (85 ms); `fx`/`light`/blur/iOS compile WGSL at runtime; `PipelineCache` no-op. | 6–24 CBs/frame; +310k/write; `get_current_texture` leaks `TextureId` on miss. |
| **(A) Patched `wgpu` fork + upstream PRs** | `~1,021k` (`1,118k` raw) / **`~1,331k`**; ~13.5% / ~17.5% | ~0.06 ms (1-pass); ~0.15 ms (blur, empty CBs only) | ~150 LOC patch + fork rebases | **Cargo ignores `[patch]` in git deps**: every app must copy `[patch.crates-io]` or face split `wgpu` versions. | Unchanged (`Passthrough` still lacks `sized_bindings` & `override`; no `MTLBinaryArchive`). | Upstream Metal review is slow (`#5738` open since 2024); never fixes 1-write (+310k) or $P$-pass CB split. |
| **(B) macOS `CAMetalLayer` bypass + `from_hal`** | `~1,195k` (`~1,292k` raw) / `~1,505k`; ~15.4% / ~19.3% | ~0.04 ms | ~250 LOC in `crates/rune` | Zero breakage, but still links `wgpu-hal` + `wgpu-core`. | Unchanged. | Strictly worse than (A): `Transit` + `Pre Pass` + split `Present` CBs (4–5 CBs/frame) and per-frame `TextureId` churn stay. |
| **(C) `wgpu-hal` directly on Metal, DX12, Vulkan (Web on `wgpu`)** | `~935k` / `~960k` (2 CBs, 1 `RcBlock` handler); ~12.3% / ~12.6% | ~0.11 ms (1-pass); ~0.25 ms (multi-pass) | **~5,500–7,500 LOC** (incl. Vulkan/DX12 barrier tracker & fence ring) | Must wrap in `wgpu::custom` or break `Compositor`/`Frame`/`fx`/`light`. | Still no `MTLBinaryArchive`; `hal` passthrough still lacks `sized_bindings` unless forked. | **High:** `wgpu-hal` is semver-unstable with undocumented `unsafe` contracts; missing Vulkan/DX12 barriers corrupt user `Compositor`s. |
| **(D) Own Metal (`macOS`/`iOS`) via `wgpu::custom` + internal fast path; `wgpu` elsewhere + 3 upstream PRs** *(Recommended)* | **`~855–880k`** (`~955k` pre-overlay-fix) / **`~860–890k`**; **~11.3–11.6%** (1 CB, 0 handlers) | **~0.14–0.19 ms** (1-pass, −35–50% scroll CPU); **~0.35–0.48 ms** (multi-pass) | **~2,800 LOC** (−350 LOC `as_hal` deleted); 0 new Cargo deps | **Zero breakage:** `Compositor`, `Frame`, `fx`, `light`, video `IOSurface`, headless & web compile unchanged; runtime `RUNE_GPU_BACKEND=wgpu` toggle. | Build-time Naga WGSL→MSL→`.metallib` for **all** shaders (`macOS` + `iOS`) + on-disk `MTLBinaryArchive` (< 15 ms warm). | **Low:** scoped to Apple's stable Metal 2.3 API; `wgpu` stays as A/B oracle and Linux/Windows/Web workhorse. |
| **(E) Own backends everywhere (Metal, DX12, Vulkan; Web on `wgpu`)** | Same as (D) on Apple (`~855–880k`) | Same as (D) on Apple; ~0.12–0.20 ms on Win/Linux | **~17,000–23,000 LOC** + DXC/SPIR-V build & allocator stack | Same `wgpu::custom` bridge, but must emulate full WebGPU surface for `Compositor` on 3 APIs. | Offline `.metallib` + DXIL + SPIR-V + `VkPipelineCache`. | **Very high:** Wayland/X11/NVIDIA swapchain hangs (`zed#46758`), D3D12 descriptor heaps/DirectComposition, Lavapipe CI quirks. |

## 3. Internal seam and public API preservation

Rune keeps `wgpu` types at the public boundary (`Gpu`, `Renderer`, `Compositor`, `Frame`,
`EffectHook`) by enabling `wgpu = { version = "~30.0.1", features = ["custom", ...] }` on
`macOS` and `iOS` only (Windows, Linux and Web compile without `custom`, preserving `wgpu`'s
single-variant devirtualization there). Across 60 draws and 10 bind groups, `custom`'s vtable
dispatch adds ~75 calls (~0.1 µs, ~0.4 kinstr). Internally, Rune's window loop, overlay loop,
`Uploads` and `native_surface` downcast once to `MetalQueue` / `MetalDevice` / `MetalSurface`
to bypass staging blits, `PresentWorker`, and `as_hal`:

```rust
// crates/rune/src/gpu/mod.rs: internal fast-path seam alongside wgpu::custom
pub(crate) trait BackendHooks {
    fn write_shared_ring(&self, bytes: &[u8], align: u64) -> Option<(wgpu::Buffer, u64)>;
    fn submit_and_present(&self, encoder: wgpu::CommandEncoder, frame: wgpu::SurfaceTexture);
    fn uses_present_worker(&self) -> bool; // false on Metal (1-CB commit is ~35 us)
}

// crates/rune/src/gpu/metal/command.rs: 1 MTLCommandBuffer across all blit/compute/render passes
enum ActiveEncoder {
    None,
    Render(Retained<ProtocolObject<dyn MTLRenderCommandEncoder>>, RenderStateCache),
    Compute(Retained<ProtocolObject<dyn MTLComputeCommandEncoder>>, ComputeStateCache),
    Blit(Retained<ProtocolObject<dyn MTLBlitCommandEncoder>>),
}

pub(crate) struct MetalEncoder {
    cmd_buf: Mutex<Option<Retained<ProtocolObject<dyn MTLCommandBuffer>>>>,
    active: Mutex<ActiveEncoder>,
}

impl wgpu::custom::CommandEncoderInterface for MetalEncoder {
    fn begin_render_pass(&self, desc: &wgpu::RenderPassDescriptor<'_>) -> DispatchRenderPass {
        let cb = self.ensure_cmd_buf();
        self.end_active_encoder(); // closes Blit/Compute without closing MTLCommandBuffer
        DispatchRenderPass::custom(MetalRenderPass::open(&cb, desc))
    }
    // begin_compute_pass and copy_* switch ActiveEncoder on the same MTLCommandBuffer.
}

impl MetalQueue {
    /// Window and overlay path: 1 command buffer, presentDrawable on it, MTLSharedEvent fence,
    /// and 0 RcBlock completion handlers unless map_async / on_submitted_work_done registered one.
    pub(crate) fn submit_presenting(&self, encoder: wgpu::CommandEncoder, frame: MetalDrawable) -> u64 {
        let (cmd_buf, callbacks) = self.finish_custom(encoder);
        let ticket = self.next_ticket.fetch_add(1, Ordering::Relaxed) + 1;
        cmd_buf.encodeSignalEvent_value(&self.shared_event, ticket); // GPU counter, no ObjC block
        if !callbacks.is_empty() { self.arm_event_listener(ticket, callbacks); }
        if !frame.presents_with_transaction {
            cmd_buf.presentDrawable(&frame.drawable);
            cmd_buf.commit();
        } else {
            cmd_buf.commit();
            cmd_buf.waitUntilScheduled();
            frame.drawable.present();
        }
        self.retire_completed(self.shared_event.signaledValue());
        ticket
    }
}
```

How every existing subsystem keeps working without signature changes:

- **`Compositor` and `Frame` (`crates/rune/src/compose.rs`):** `Frame` still hands callers
  `&wgpu::Device`, `&wgpu::Queue`, `&mut wgpu::CommandEncoder` and `&wgpu::TextureView`. App
  compositors (`overlay_hud_cost`, `lab`, `cursed-settings`, `rune-fx`, `rune-light`) record
  straight into `MetalEncoder`'s single `MTLCommandBuffer` through `wgpu::custom`.
- **Uploads (`Uploads` and `rune-staging`):** On Apple unified memory, `Renderer` and
  `Frame::write_buffer` allocate from a triple-buffered `MTLStorageModeShared` ring retired by
  `shared_event.signaledValue()`, copying bytes directly into `MTLBuffer.contents()` and binding
  offsets with zero `MTLBlitCommandEncoder`s and zero `map_async` callbacks. Glyph and image
  atlas row updates record `copyFromBuffer:toTexture:` on the frame's `MTLCommandBuffer` ahead
  of the first render pass without closing the command buffer.
- **Video (`native_surface/macos.rs`) and AppKit (`macos.rs`, `present_time.rs`,
  `system_material`):** `MetalDevice::import_cv_pixel_buffer` wraps `CVMetalTextureGetTexture`
  directly via `wgpu::Texture::from_custom` (retaining `CVMetalTexture` + `PixelBuffer` in the
  custom texture's `Drop`), and `MetalSurface::layer()` exposes `&CAMetalLayer` without
  `wgpu-hal` or `wgpu-core`.
- **Headless tests & CI:** `Gpu::headless()` uses `MetalInstance` on macOS (1 `MTLCommandBuffer`
  + `MTLBlitCommandEncoder` readback to a shared `MTLBuffer`) and `wgpu` Vulkan/Lavapipe on
  Linux CI. Setting `RUNE_GPU_BACKEND=wgpu` switches macOS back to `wgpu-core` for differential
  pixel and timing checks in the same binary.

## 4. Phased plan and measurable gates

### Parallel track: upstream three small fixes to `gfx-rs/wgpu` (~120 LOC, non-blocking)

Fix the three bugs in `wgpu-hal-metal-lazy-command-buffer.patch` (decrement the counter when
dropping an unsubmitted `raw: None` `CommandBuffer`, preserve user command buffer labels on
signal, latch `relay.next_release_value` in `begin_encoding`), pair it with
`wgpu-core-present-skip.patch` and a `fid` cleanup on failed `surface_get_current_texture`
(`wgpu-core/src/present.rs:449`), and submit PRs with hand-written descriptions per `wgpu`'s
contribution policy. Do not block Rune on upstream review or carry a `[patch.crates-io]` fork;
once merged into a `wgpu` point release, Windows DX12 and Linux Vulkan gain single-submit
presentation automatically.

### Phase 0: First spike (≤ 450 LOC, ≤ 1 day), single-CB `wgpu::custom` Metal + Naga MSL

The two riskiest assumptions are (1) that `wgpu::custom` over `objc2-metal` + `MTLSharedEvent`
runs `overlay_hud_cost` unchanged with 1 command buffer, 0 completion handlers and 0 blit
encoders (eliminating both the empty-CB tax and the +310 kinstr upload tax), and (2) that
standalone `wgpu::naga` (`wgsl` → `process_overrides` → `msl`) compiles every WGSL shader in
`rune`, `rune-fx` and `rune-light` (`_mslBufferSizes`, `override`s, atomics, MRT) cleanly.

1. **Build `crates/rune/src/gpu/metal_spike.rs` (~380 LOC, gated on `RUNE_GPU_BACKEND=metal`):**
   implement minimal `InstanceInterface`, `SurfaceInterface` (`CAMetalLayer::nextDrawable`),
   `DeviceInterface`, `BufferInterface` (`MTLStorageModeShared` direct write),
   `CommandEncoderInterface`, `RenderPassInterface`, and `MetalQueue::submit_presenting`
   (`presentDrawable` + `MTLSharedEvent`, 0 `RcBlock`s).
2. **Add a standalone Naga→MSL compile test (~60 LOC):** translate all 29 WGSL shaders (with
   `layer_blur` override specializations and `rune-light`/`gpu_springs` storage-buffer size
   slots) and compile each via `MTLDevice::newLibraryWithSource`.
3. **Exact measurement (`--profile perf`, real HUD window on a dedicated test Mac, 4 alternating
   10 s rounds after 2 s warmup, `OVERLAY_WRITES=0` and `OVERLAY_WRITES=1`):**
   - **Go gate:** `OVERLAY_COUNT_METAL=1` reports `command_buffers=1.00`, `presents=1.00`,
     `completed_handlers=0.00`, `blit_encoders=0.00` on both `WRITES=0` and `WRITES=1`;
     `WRITES=0` lands within ≤ +40 kinstr/frame (≤ +0.5 CPU percentage points) of
     `metal_hud.swift` once paired with the overlay-loop fixes (or ≤ 960 kinstr before them,
     ≥ 595 kinstr below today's 1,555k); `WRITES=1` adds ≤ 15 kinstr/frame over `WRITES=0`
     (down from +310–323 kinstr today); all 29 WGSL shaders compile to `MTLLibrary`.
   - **Kill / pivot gate:** if `wgpu::custom` blocks a required `wgpu` handle operation or adds
     `> 25 kinstr/frame` of dispatch overhead, keep the Metal backend and swap the `wgpu::custom`
     shim for an internal `gpu::Encoder` enum (`Metal | Wgpu`). If single-CB Metal itself fails
     to beat patched `wgpu` by `≥ 180 kinstr` on `WRITES=0` and `≥ 400 kinstr` on `WRITES=1`,
     abandon owning Metal and ship only the upstream `wgpu` patches.

### Phase 1: Core Metal backend, unified-memory upload ring and window/overlay integration (~1,500 LOC)

- Implement full 2D textures, sRGB/linear views, mip views, samplers, MRT render passes (up to 4
  color attachments), dynamic uniform offsets, state deduplication, blit copies (`buf↔tex`,
  `tex↔tex`, headless `tex→buf` readback), and `MTLSharedEvent` deferred-free resource
  retirement (`Vec<(u64, Vec<Retained<AnyObject>>)>`).
- Wire `Uploads` and `Frame::write_buffer` on Metal to a bounded `MTLStorageModeShared` ring (no
  staging copy or `map_async`), encode dirty glyph/image atlas blits onto the frame's single
  `MTLCommandBuffer` without closing it, fold `submit + present` in `window/draw.rs` and
  `overlay/frame.rs`, and skip spawning `PresentWorker` when running on `MetalQueue`.
- **Gate:** `cargo test -p rune -p rune-kit --features rune-kit/markdown` green on
  `RUNE_GPU_BACKEND=metal`; `scripts/check-reference.sh` (`rune-ref`) and CoreText golden tests
  match `wgpu` pixel-for-pixel; `scripts/motion-audit` passes with a HUD/scroll scenario added;
  `upload_bench --profile perf` drops `steady` `54 µs → ≤ 15 µs` and `scroll` `83 µs → ≤ 22 µs`
  with `0` buffer allocations/frame; `bench-ui.py --frame-cpu --scroll` + window present trace
  commits `1.00` command buffer/frame and cuts total main+driver scroll CPU by `≥ 120 µs/frame`.

### Phase 2: Compute, timestamps, `rune-fx`/`rune-light`, `IOSurface` video and `as_hal` removal (~900 LOC, −350 LOC deleted)

- Add `MTLComputePipelineState`, `MTLComputeCommandEncoder` (switching cleanly inside the same
  `MTLCommandBuffer`), `_mslBufferSizes` sideband `setVertexBytes`/`setFragmentBytes`/`setBytes`,
  and `MTLCounterSampleBuffer` timestamp queries (`resolve_query_set`).
- Move `macos.rs`, `present_time.rs`, `system_material/macos.rs` and `native_surface/macos.rs`
  (`CVMetalTextureCache` zero-copy BGRA/NV12 import) onto `MetalSurface` / `MetalDevice`
  directly; make `RUNE_GPU_BACKEND=metal` the default on `macOS` and `iOS` and gate `wgpu`'s
  `metal` feature + `wgpu-hal`/`wgpu-core` behind an optional `wgpu-metal-fallback` dev/debug
  feature.
- **Gate:** `rune-fx` and `rune-light` test suites and `video_gui` pass (`live_planes == 0` on
  drop); `frame_bench --profile perf` (including `finish + submit`) commits 1.00
  `MTLCommandBuffer`/frame across all 10+ GI and glass passes (down from 16–24), cutting
  multi-pass submit+driver CPU by `≥ 60%`; `layout_text_cost` shows 0% regression;
  `scripts/file-size.sh` passes with every new file under 800 non-test lines.

### Phase 3: Build-time `.metallib` bundle for all shaders + `MTLBinaryArchive` cache (~450 LOC)

- Extend `scripts/precompile-metal-shaders.sh` to run Naga (`wgsl` → `process_overrides` →
  `msl`) and `xcrun metal` for `macos`, `ios` and `ios-simulator` across all library shaders
  (`uber` + `gpu_springs`, `backdrop_blur`, `layer_blur`'s 5 specializations, `video`,
  `rune-fx`'s 13 modules, `rune-light`'s 7 modules), embedding `.metallib` bytecode plus compact
  `sized_bindings` tables (falling back to runtime Naga WGSL only for custom app shaders).
- Add a bounded on-disk `MTLBinaryArchive` keyed by `(OS build, GPU name, ABI hash)` so pipeline
  creation hits cached GPU ISA after first launch.
- **Gate:** cold pipeline init for `Renderer` + `FxRenderer` + `LightRenderer` drops `≥ 3×`
  (`≤ 80 ms` cold AIR, `≤ 15 ms` warm archive); `scripts/bench-startup.py` `first-pixels` median
  beats or matches baseline; `.metallib` freshness and pixel-equivalence tests pass across
  `rune`, `rune-fx` and `rune-light`.

## 5. Triggers to expand or abandon

- **Abandon / roll back to `wgpu` + upstream patches** if: (a) Phase 0 misses its kill gate,
  (b) `crates/rune/src/gpu/metal/` exceeds 3,500 non-test LOC, or (c) Metal backend parity
  causes `> 2` pixel or pacing regressions in a quarter once Phase 2 ships (`RUNE_GPU_BACKEND=wgpu`
  provides an immediate fallback before `wgpu-metal-fallback` is ever removed).
- **Expand to an owned Windows DX12 backend (~3.5–4.5 kLOC)** only if, after upstream
  `present-skip` ships and [32](32-engine-plan.md) Phases 1–3 land: (a) a real Windows 120 Hz
  HUD or scrolling app spends ≥ 25% of frame CPU and ≥ 0.35 ms/frame inside
  `wgpu_core`/`wgpu_hal::dx12`, and (b) every library shader precompiles offline to signed DXIL
  so Rune never ships `dxcompiler.dll` (~20 MB) at runtime.
- **Never own WebGPU** (`wgpu` on `wasm32` already compiles straight to browser WebGPU bindings
  without `wgpu-core`), and **do not own Linux Vulkan** unless `wgpu-core` exceeds 0.50 ms/frame
  (> 30%) on a 120 Hz Wayland layer-shell workload after `present-skip` and a targeted upstream
  `wgpu-hal/vulkan` patch cannot fix it.
