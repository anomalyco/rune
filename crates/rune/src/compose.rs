//! Compositor hook: an app takes over drawing each frame (lighting, effect layers) while Rune
//! keeps layout, animation, input, the window and the headless tooling. `rune::run` never
//! builds one, so the default path is unchanged; `rune::run_with` or `App::compositor` attaches
//! one.

use crate::render::{Instance, Kind, Renderer};
use crate::runtime::Runtime;

/// Draws a frame in place of Rune's default.
///
/// To paint custom content at `El::effect` slots, draw the frame with `Renderer::draw_with` and
/// paint in its hook: it runs at each slot in painter order, inside the slot's isolated layers,
/// so the slot's opacity (an exit fade included), clip and transform already apply. Keep
/// resources per `EffectSite::mark`, read the slot's `EffectSite::data`, and drop a mark's
/// resources once a frame passes without it:
///
/// ```
/// # use std::collections::{HashMap, HashSet};
/// # use rune::{Compositor, Frame, render::EffectSite};
/// # struct App;
/// # struct Wave;
/// # impl Wave {
/// #     fn new(_: &wgpu::Device) -> Self { Self }
/// #     fn draw(&mut self, _: &wgpu::Device, _: &wgpu::Queue, _: &mut wgpu::RenderPass<'_>, _: &EffectSite<'_>, _: [f32; 4]) {}
/// # }
/// struct Hud { waves: HashMap<u32, Wave>, seen: HashSet<u32> }
///
/// impl Compositor<App> for Hud {
///     fn render(&mut self, f: &mut Frame<'_, App>) {
///         let (device, queue, clear) = (f.device, f.queue, f.rt.clear_color());
///         f.ui.prepare(device, queue, &mut f.rt.text.atlas, &f.rt.instances, f.size);
///         self.seen.clear();
///         f.ui.draw_with(f.encoder, f.target, clear, &mut |pass, site| {
///             self.seen.insert(site.mark);
///             let wave = self.waves.entry(site.mark).or_insert_with(|| Wave::new(device));
///             wave.draw(device, queue, pass, site, site.data);
///         });
///         self.waves.retain(|mark, _| self.seen.contains(mark));
///     }
/// }
/// ```
pub trait Compositor<S: 'static>: 'static {
    /// Device features to request when the adapter has them (e.g. `TIMESTAMP_QUERY`).
    /// `IMMEDIATES` also raises `max_immediate_size` to the adapter's (4 KB on Metal): per-draw
    /// values set with `RenderPass::set_immediates` travel inside the pass, where a uniform
    /// written through the queue or `Frame::write_buffer` costs the frame another command buffer.
    fn features(&self) -> wgpu::Features {
        wgpu::Features::empty()
    }

    /// Draws the frame into `f.target`. Runs after `Runtime::frame` built the instances, in the
    /// window and headless alike. Nothing has been recorded yet; `El::pixels` images are uploaded.
    /// `f.device` can change between frames (an overlay shown again on another GPU, or a lost
    /// device recreated after [`Compositor::release_gpu`]): keep resources made on it with
    /// `f.device_key()`, and make them again when it changes. Never compare `wgpu::Device`s for
    /// this: wgpu compares them by id, and every Rune GPU has its own instance, whose first device
    /// has the same id as the last one's (hex#982).
    fn render(&mut self, f: &mut Frame<'_, S>);

    /// Drops GPU resources owned by the compositor (textures, buffers, pipelines, bind groups,
    /// effect/light renderers) right after Rune drops its `wgpu::Device` and before recreating a
    /// replacement device.
    ///
    /// On Windows D3D12, a removed `ID3D12Device` stays the adapter's sole process-wide device
    /// while any resource made on it remains alive, causing `D3D12CreateDevice` on that adapter
    /// to fail until those resources are dropped. The default does nothing so existing compositors
    /// keep building without changes.
    fn release_gpu(&mut self) {}

    /// True while something the compositor draws moves on its own: the window keeps its display
    /// link running. False lets it go idle (no frames at rest) and rest: a due `wake_after`
    /// runs `poll` alone and draws only when that poll changed something.
    fn animating(&self) -> bool {
        false
    }

    /// Whether the last frame drew anything of the compositor's own, besides `Frame::draw_ui`.
    /// False lets the window treat it as Rune's default frame: a due timer whose frame would
    /// build the same instances draws nothing, as in a window with no compositor. A compositor
    /// that only sometimes draws (Office pages in their slots) says false while it doesn't.
    fn draws_own(&self) -> bool {
        true
    }
    /// Printed after a headless run (e.g. GPU timings).
    fn summary(&mut self, _device: &wgpu::Device) -> Option<String> {
        None
    }
}

/// One frame handed to a compositor.
pub struct Frame<'a, S: 'static> {
    pub device: &'a wgpu::Device,
    pub queue: &'a wgpu::Queue,
    pub encoder: &'a mut wgpu::CommandEncoder,
    /// The window surface or the offscreen target.
    pub target: &'a wgpu::TextureView,
    pub format: wgpu::TextureFormat,
    /// Target size in device pixels.
    pub size: (u32, u32),
    /// Real seconds since the previous frame (0 on the first).
    pub dt: f64,
    /// CPU milliseconds `Runtime::frame` took this frame (handlers, view, layout, animation).
    pub update_ms: f64,
    /// Instances, `marks`, the clock (`t`), the pointer, element rects and app state.
    pub rt: &'a mut Runtime<S>,
    /// Rune's own renderer: `prepare_lists`, `encode_layers`, then `draw_range` draw complete
    /// painter scopes into any target (`add_format` for non-target formats). Layer begin/end
    /// records must remain balanced; do not partition a scope by primitive kind.
    pub ui: &'a mut Renderer,
}

/// Which device a frame draws on, unique for the process: a new device never has an earlier
/// key, as a `wgpu::Device` can have an earlier id. Keep it beside resources made with
/// `Frame::device`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct DeviceKey(u64);

impl DeviceKey {
    pub(crate) fn next() -> DeviceKey {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        DeviceKey(NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed))
    }
}

impl<S: 'static> Frame<'_, S> {
    /// The device `device` is, as a key that changes whenever the frame moves to another one.
    pub fn device_key(&self) -> DeviceKey {
        self.ui.device_key()
    }

    /// The `wgpu::Instance` that created this frame's device, when the renderer belongs to a Rune
    /// host.
    ///
    /// Returns `Some` across all Rune-managed hosts: Windows (DX12 wgpu), Linux (Vulkan wgpu),
    /// macOS wgpu (`GpuBackend::Wgpu`), macOS and iOS native Metal (`GpuBackend::Metal`, wrapped
    /// through wgpu's custom backend), and browser WebGPU and WebGL2 (`wasm32`). Returns `None`
    /// when `Frame::ui` is a standalone [`Renderer::new`] or [`Renderer::try_new`] with no
    /// attached GPU.
    ///
    /// On macOS, Rune's default native Metal backend implements the wgpu subset Rune uses; an
    /// embedded renderer that needs render bundles, indirect draws, occlusion queries, or other
    /// wgpu features should opt into `App::gpu_backend(GpuBackend::Wgpu)` and
    /// `prewarm_gpu_on(GpuBackend::Wgpu)`.
    ///
    /// ```no_run
    /// # use rune::compose::{Compositor, DeviceKey, Frame};
    /// # struct Engine;
    /// # impl Engine {
    /// #     fn new(_: wgpu::Instance, _: wgpu::Adapter, _: wgpu::Device, _: wgpu::Queue) -> Self { Self }
    /// # }
    /// struct Embedded {
    ///     key: Option<DeviceKey>,
    ///     engine: Option<Engine>,
    /// }
    ///
    /// impl Compositor<()> for Embedded {
    ///     fn render(&mut self, f: &mut Frame<'_, ()>) {
    ///         if self.key != Some(f.device_key())
    ///             && let (Some(inst), Some(adap)) = (f.instance(), f.adapter())
    ///         {
    ///             self.engine = Some(Engine::new(
    ///                 inst.clone(),
    ///                 adap.clone(),
    ///                 f.device.clone(),
    ///                 f.queue.clone(),
    ///             ));
    ///             f.restore_device_callbacks();
    ///             self.key = Some(f.device_key());
    ///         }
    ///         f.draw_ui();
    ///     }
    ///
    ///     fn release_gpu(&mut self) {
    ///         self.engine = None;
    ///         self.key = None;
    ///     }
    /// }
    /// ```
    pub fn instance(&self) -> Option<&wgpu::Instance> {
        self.ui.gpu_instance()
    }

    /// The `wgpu::Adapter` that created this frame's device, when the renderer belongs to a Rune
    /// host.
    ///
    /// Returns `Some` across Windows (DX12 wgpu), Linux (Vulkan wgpu), macOS wgpu
    /// (`GpuBackend::Wgpu`), macOS and iOS native Metal (`GpuBackend::Metal`, wrapped through
    /// wgpu's custom backend), and browser WebGPU and WebGL2 (`wasm32`). Returns `None` when
    /// `Frame::ui` is a standalone [`Renderer::new`] or [`Renderer::try_new`] with no attached
    /// GPU.
    ///
    /// On macOS, embedded engines needing render bundles, indirect draws, or occlusion queries
    /// should opt into `App::gpu_backend(GpuBackend::Wgpu)` and
    /// `prewarm_gpu_on(GpuBackend::Wgpu)`.
    ///
    /// ```no_run
    /// # use rune::compose::Frame;
    /// # fn inspect<S: 'static>(f: &Frame<'_, S>) -> Option<wgpu::AdapterInfo> {
    /// f.adapter().map(wgpu::Adapter::get_info)
    /// # }
    /// ```
    pub fn adapter(&self) -> Option<&wgpu::Adapter> {
        self.ui.gpu_adapter()
    }

    /// Re-installs Rune's device-lost and uncaptured-error callbacks on `self.device`.
    ///
    /// Call this right after initializing an embedded renderer that overwrites wgpu's single-slot
    /// `Device::set_device_lost_callback` or `Device::on_uncaptured_error` handlers so Rune keeps
    /// owning window recovery and bounded error logging:
    ///
    /// - On native wgpu (Windows DX12, Linux Vulkan, macOS `GpuBackend::Wgpu`): re-registers
    ///   Rune's device-lost callback against the same `Gpu::is_lost` flag, and re-registers
    ///   Rune's bounded uncaptured-error logger with the same deduplication state on surface
    ///   devices (headless devices keep wgpu's validation panic).
    /// - On macOS/iOS native Metal (`GpuBackend::Metal`): re-registers Rune's device-lost
    ///   callback against the same `Gpu::is_lost` flag (`on_uncaptured_error` is a no-op on the
    ///   native Metal backend).
    /// - On browser WebGPU (`wasm32`): clears any foreign JS `device.onuncapturederror` handler
    ///   while leaving Rune's `addEventListener('uncapturederror', ...)` and `device.lost.then`
    ///   watchers intact without adding duplicates.
    /// - On browser WebGL2 (`wasm32`): re-registers Rune's `console.error` uncaptured-error
    ///   logger (keeping the same deduplication log) and device-lost callback.
    /// - On a standalone [`Renderer`] with no attached GPU: no-op.
    ///
    /// ```no_run
    /// # use rune::compose::Frame;
    /// # fn attach_external<S: 'static>(f: &mut Frame<'_, S>, init_engine: impl FnOnce(&wgpu::Instance, &wgpu::Adapter, &wgpu::Device, &wgpu::Queue)) {
    /// if let (Some(inst), Some(adap)) = (f.instance(), f.adapter()) {
    ///     init_engine(inst, adap, f.device, f.queue);
    ///     f.restore_device_callbacks();
    /// }
    /// # }
    /// ```
    pub fn restore_device_callbacks(&self) {
        self.ui.restore_device_callbacks(self.device);
    }

    /// True when the frame is sRGB-blended (`App::srgb_blend`): `target` is a non-sRGB view and
    /// premultiplied colours, in `ui` passes and in layers, are sRGB-encoded. False: linear
    /// premultiplied values, encoded by the `*Srgb` target on write. Build effect pipelines and
    /// encode their output to match (`FxRenderer::set_srgb_blend` in rune-fx).
    pub fn srgb_blend(&self) -> bool {
        self.ui.srgb_blend()
    }

    /// Packed explicit scroll-fade stops for this frame (`FxRenderer::set_fade_stops` in rune-fx
    /// or `clip_alpha_stops` in `CLIP_WGSL`).
    pub fn fade_stops(&self) -> [f32; 2] {
        self.rt.fade_stops()
    }

    /// `queue.write_buffer` without its per-call staging buffer: `data` goes through Rune's
    /// reused staging memory and lands in `buffer` at `offset` ahead of the commands recorded
    /// into `encoder` after this call. `offset` and `data.len()` are multiples of 4.
    pub fn write_buffer(&mut self, buffer: &wgpu::Buffer, offset: u64, data: &[u8]) {
        self.ui.write_buffer(self.device, self.queue, self.encoder, buffer, offset, data);
    }

    /// Iterates every `El::effect` slot in this frame's laid-out instances, in painter order.
    ///
    /// Call this before opening compositor passes or calling `Renderer::prepare` to find slots by
    /// [`EffectSlot::mark`], size offscreen targets from [`EffectSlot::rect`] and
    /// [`EffectSlot::scale`], or read [`EffectSlot::data`] in the same frame layout ran. Does not
    /// require `Runtime::track_marks`.
    ///
    /// This borrows `&Frame`. To encode into `f.encoder` while iterating without cloning, call
    /// [`Runtime::effect_slots`] on `f.rt` directly (borrowing only `f.rt`).
    pub fn effect_slots(&self) -> impl Iterator<Item = EffectSlot<'_>> {
        self.rt.effect_slots()
    }

    /// Finds the first `El::effect` slot in this frame tagged `mark`, in painter order.
    ///
    /// Convenience over [`Self::effect_slots`] when each mark identifies one slot. If multiple
    /// slots can share a mark (such as an outgoing slot playing its exit transition beside its
    /// replacement), iterate [`Self::effect_slots`] or [`Runtime::effect_slots`] instead.
    pub fn effect_slot(&self, mark: u32) -> Option<EffectSlot<'_>> {
        self.rt.effect_slot(mark)
    }

    /// Imports `surface`'s current frame onto this frame's GPU device as `wgpu::Texture` plane(s),
    /// reusing Rune's per-device importer and texture cache across frames (zero-copy
    /// `CVMetalTextureCache` on macOS on both [`GpuBackend::Metal`](crate::GpuBackend::Metal) and
    /// [`GpuBackend::Wgpu`](crate::GpuBackend::Wgpu), pooled copy on Windows shared textures and
    /// Linux dmabufs, in-place NV12 planes for Windows hardware-decoded video, and texture upload
    /// for [`ByteFrame`](crate::native_surface::ByteFrame)).
    ///
    /// Returns `Ok(None)` when `surface` holds no frame, `Ok(Some(imported))` with the plane
    /// texture(s) and colour/orientation metadata when a frame is present, or `Err` if importing
    /// failed (the same error is recorded on
    /// [`NativeSurface::last_error`](crate::NativeSurface::last_error)).
    ///
    /// Repeated calls for an unchanged frame return the cached texture(s) without re-importing or
    /// re-copying. Importer state belongs to this frame's device (`self.device_key()`) and drops
    /// automatically on device loss. When `surface` lives inside `self.rt.state`, either clone the
    /// cheap `NativeSurface` handle or call `f.ui.surface_texture(f.device, f.queue, &f.rt.state.surface)`.
    ///
    /// Samples the surface's already-pushed frame mailbox; it does not pull a
    /// [`Player`](crate::Player)-backed surface unless that surface is also drawn in the view tree.
    ///
    /// ```no_run
    /// # use rune::compose::{Compositor, DeviceKey, Frame};
    /// # use rune::NativeSurface;
    /// struct Scene {
    ///     surface: NativeSurface,
    ///     device_key: Option<DeviceKey>,
    ///     revision: Option<u64>,
    ///     view: Option<wgpu::TextureView>,
    /// }
    ///
    /// impl Compositor<()> for Scene {
    ///     fn render(&mut self, f: &mut Frame<'_, ()>) {
    ///         if let Ok(Some(imported)) = f.surface_texture(&self.surface) {
    ///             if self.device_key != Some(f.device_key()) || self.revision != Some(imported.revision()) {
    ///                 self.view = Some(imported.texture().create_view(&Default::default()));
    ///                 self.device_key = Some(f.device_key());
    ///                 self.revision = Some(imported.revision());
    ///             }
    ///         }
    ///         f.draw_ui();
    ///     }
    ///
    ///     fn release_gpu(&mut self) {
    ///         self.view = None;
    ///         self.device_key = None;
    ///         self.revision = None;
    ///     }
    /// }
    /// ```
    pub fn surface_texture(
        &mut self,
        surface: &crate::NativeSurface,
    ) -> Result<Option<crate::native_surface::ImportedSurface>, String> {
        self.ui.surface_texture(self.device, self.queue, surface)
    }

    /// The default frame: every instance into the target, cleared to the window colour.
    pub fn draw_ui(&mut self) {
        let clear = self.rt.clear_color();
        self.ui.set_fade_stops(self.rt.fade_stops());
        self.ui.prepare_ui(self.device, self.queue, &mut self.rt.text.atlas, &self.rt.instances, self.size);
        self.ui.draw(self.encoder, self.target, clear);
    }
}

/// One `El::effect` slot present in a frame's laid-out instances ([`Frame::effect_slots`],
/// [`Frame::effect_slot`], [`Runtime::effect_slots`], [`Runtime::effect_slot`]).
///
/// Use this before opening compositor passes or calling `Renderer::prepare` to size offscreen
/// targets (for example an MSAA + depth 3D scene) from [`Self::rect`] and [`Self::scale`] in the
/// same frame the slot is laid out. When drawing the slot inside Rune's pass via
/// `Renderer::draw_with`, [`EffectSite`](crate::render::EffectSite) supplies the pass's target
/// `format` (`Frame::format` on the frame target, `Rgba16Float` inside an isolated layer),
/// `origin`, `size`, `in_layer` and `backdrop`.
///
/// ```no_run
/// # use rune::{Compositor, Frame, render::EffectSite};
/// # const VIEWPORT: u32 = 1;
/// # struct App;
/// # struct Scene;
/// # impl Scene {
/// #     fn render_offscreen(&mut self, _: &wgpu::Device, _: &wgpu::Queue, _: &mut wgpu::CommandEncoder, _: [f32; 4], _: f32) {}
/// #     fn blit(&mut self, _: &mut wgpu::RenderPass<'_>, _: &EffectSite<'_>) {}
/// # }
/// impl Compositor<App> for Scene {
///     fn render(&mut self, f: &mut Frame<'_, App>) {
///         if let Some(slot) = f.effect_slot(VIEWPORT) {
///             let (rect, scale) = (slot.rect, slot.scale);
///             self.render_offscreen(f.device, f.queue, f.encoder, rect, scale);
///         }
///         let clear = f.rt.clear_color();
///         f.ui.prepare(f.device, f.queue, &mut f.rt.text.atlas, &f.rt.instances, f.size);
///         f.ui.draw_with(f.encoder, f.target, clear, &mut |pass, site| {
///             if site.mark == VIEWPORT {
///                 self.blit(pass, site);
///             }
///         });
///     }
/// }
/// ```
#[non_exhaustive]
#[derive(Clone, Copy, Debug)]
pub struct EffectSlot<'a> {
    /// The slot's position in `Runtime::instances` (matches `EffectSite::index`).
    pub index: u32,
    /// The slot's `KIND_EFFECT` record: `rect`, `radii`, `clip` and `fade`.
    pub instance: &'a Instance,
    /// The slot's `El::effect` tag (`Instance::effect_mark`, matches `EffectSite::mark`).
    pub mark: u32,
    /// The slot's `El::effect_data` payload, bit for bit (matches `EffectSite::data`).
    pub data: [f32; 4],
    /// The slot's device-pixel rectangle `[x, y, width, height]` (`instance.rect`).
    pub rect: [f32; 4],
    /// Device pixels per point at this slot (`instance.params[3]`, matches `EffectSite::scale`).
    pub scale: f32,
    /// Bleed past `rect` in device pixels (`El::effect_bleed`, `instance.params[2]`).
    pub bleed: f32,
    /// Whether the slot requested a backdrop copy (`El::effect_backdrop(true)`).
    pub backdrop: bool,
}

impl<S: 'static> Runtime<S> {
    /// Iterates every `El::effect` slot in `self.instances`, in painter order.
    ///
    /// Decodes each slot's tag from its `KIND_EFFECT` record (`Instance::effect_mark`), so it
    /// works even when `Runtime::track_marks` is false. Inside `Compositor::render`, calling
    /// `f.rt.effect_slots()` borrows only `f.rt`, leaving `f.device`, `f.queue`, `f.encoder` and
    /// `f.ui` available for encoding compositor passes in the loop.
    pub fn effect_slots(&self) -> impl Iterator<Item = EffectSlot<'_>> {
        self.instances.iter().enumerate().filter_map(|(index, instance)| {
            let mark = instance.effect_mark()?;
            Some(EffectSlot {
                index: index as u32,
                instance,
                mark,
                data: instance.uv,
                rect: instance.rect,
                scale: instance.params[3],
                bleed: instance.params[2],
                backdrop: instance.params[0] != 0.0,
            })
        })
    }

    /// Finds the first `El::effect` slot in `self.instances` tagged `mark`, in painter order.
    pub fn effect_slot(&self, mark: u32) -> Option<EffectSlot<'_>> {
        self.effect_slots().find(|slot| slot.mark == mark)
    }
}

pub fn is_glyph(i: &Instance) -> bool {
    i.kind() == Some(Kind::Glyph)
}

#[cfg(all(test, not(target_arch = "wasm32")))]
#[path = "compose_tests.rs"]
mod tests;
