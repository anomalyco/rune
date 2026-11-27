#[cfg(not(target_arch = "wasm32"))]
pub(super) mod completion;
mod frame;

use super::*;
use rune_core::Blend;
use std::collections::HashMap;

/// The video pass's shader; on Windows its DX12 bytecode ships precompiled (`dxbc.rs`).
pub(super) const WGSL: &str = include_str!("video.wgsl");

fn blend_state(blend: Blend) -> wgpu::BlendState {
    match blend {
        Blend::SourceOver => wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING,
        Blend::PlusLighter => {
            let add = wgpu::BlendComponent {
                src_factor: wgpu::BlendFactor::One,
                dst_factor: wgpu::BlendFactor::One,
                operation: wgpu::BlendOperation::Add,
            };
            wgpu::BlendState { color: add, alpha: add }
        }
    }
}

/// A renderer whose pipeline compiles on a thread of its own (`SurfaceRenderer::build`).
#[doc(hidden)]
#[cfg_attr(target_arch = "wasm32", allow(dead_code))]
pub struct Building {
    format: wgpu::TextureFormat,
    layout: wgpu::BindGroupLayout,
    /// The compiled pipeline, once sent; None when the thread couldn't start.
    built: Option<std::sync::mpsc::Receiver<wgpu::RenderPipeline>>,
    /// The punch pipeline (`SurfaceRenderer::set_punched`), compiled next on the same thread;
    /// the renderer takes the receiver.
    punch: std::sync::Mutex<Option<std::sync::mpsc::Receiver<wgpu::RenderPipeline>>>,
}

pub(crate) struct Imported {
    frame: Arc<Frame>,
    textures: [wgpu::Texture; 2],
    /// Whether `textures` come from a bounded pool or decoder ring that may be returned again,
    /// so caching their bind group in `Prepared::binds` avoids per-frame reallocation without
    /// retaining one-shot textures.
    reusable: bool,
    /// Keeps a pooled copy target reserved while this `Imported` is live (in `cache`,
    /// `Prepared`, or an in-flight `SurfaceRenderer::submit` callback).
    _lease: Option<Arc<()>>,
}
struct Prepared {
    drawn: std::sync::atomic::AtomicBool,
    before: u32,
    bind: wgpu::BindGroup,
    uniform: wgpu::Buffer,
    imported: Arc<Imported>,
    surface: NativeSurface,
    /// Bind groups this draw made for other textures, newest last: a decoder cycles a few, so
    /// a new picture in one it drew before binds with no allocation.
    binds: Vec<([wgpu::Texture; 2], wgpu::BindGroup)>,
    /// Its place is punched out of the window's pixels, for the clip's own layer underneath.
    punch: bool,
    /// It was drawn inside an isolated layer, where a punch can't reach the window.
    isolated: std::sync::atomic::AtomicBool,
}

/// Clears a video's place by its coverage, colour and alpha alike: the window's pixels there go
/// transparent and the clip's own layer underneath shows (`video::windows::layer`).
#[cfg(target_os = "windows")]
const PUNCH: wgpu::BlendState = wgpu::BlendState { color: CLEAR, alpha: CLEAR };
#[cfg(target_os = "windows")]
const CLEAR: wgpu::BlendComponent = wgpu::BlendComponent { src_factor: wgpu::BlendFactor::Zero, dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha, operation: wgpu::BlendOperation::Add };

// Metal has a small limit on live counter sample buffers. Keep one query set and lease
// disjoint pairs until their deferred resolve/readback completes, rather than allocating
// a query set for every queued frame. Timing may be omitted; rendering never waits.
#[cfg(not(target_arch = "wasm32"))]
const TIMING_SLOTS: u32 = 8;
struct TimingPool {
    queries: wgpu::QuerySet,
    free: std::sync::Mutex<Vec<u32>>,
}
impl TimingPool {
    fn take(self: &Arc<Self>) -> Option<Arc<TimingSlot>> {
        self.free.lock().unwrap().pop().map(|start| Arc::new(TimingSlot { pool: self.clone(), start }))
    }
}
struct TimingSlot {
    pool: Arc<TimingPool>,
    start: u32,
}
impl Drop for TimingSlot {
    fn drop(&mut self) {
        self.pool.free.lock().unwrap().push(self.start);
    }
}

/// Standalone pipeline, also used by Rune's renderer. A frame must be submitted through `submit`
/// for timing notifications; texture lifetimes are safe even with ordinary wgpu submission.
pub struct SurfaceRenderer {
    /// The queue timestamp resolves go to, held here so their callbacks hold it weakly.
    #[cfg_attr(target_arch = "wasm32", allow(dead_code))]
    timing_queue: std::sync::OnceLock<Arc<wgpu::Queue>>,
    #[cfg(not(target_arch = "wasm32"))]
    pub(super) completion: completion::CompletionWorker,
    /// Timestamp resolves a completion callback couldn't submit yet (`completion::Parked`).
    #[cfg(not(target_arch = "wasm32"))]
    resolves: Arc<completion::Parked<completion::Resolve>>,
    #[cfg(target_os = "linux")]
    dmabuf: Option<linux::Importer>,
    #[cfg_attr(target_arch = "wasm32", allow(dead_code))]
    device: wgpu::Device,
    timing_pool: Option<Arc<TimingPool>>,
    queries: Option<Arc<TimingSlot>>,
    queries_used: std::sync::atomic::AtomicBool,
    pipeline: wgpu::RenderPipeline,
    format: wgpu::TextureFormat,
    extra: Vec<(wgpu::TextureFormat, wgpu::RenderPipeline)>,
    additive: Option<(wgpu::TextureFormat, wgpu::RenderPipeline)>,
    layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    cache: HashMap<usize, Arc<Imported>>,
    draws: Vec<Prepared>,
    /// Surfaces whose place is punched out (by key), and the pipeline that does it: the video's
    /// coverage, opacity and clip, writing `dst * (1 - coverage)` to colour and alpha alike.
    punched: Vec<usize>,
    punch: Option<wgpu::RenderPipeline>,
    #[cfg_attr(target_arch = "wasm32", allow(dead_code))]
    punch_building: Option<std::sync::mpsc::Receiver<wgpu::RenderPipeline>>,
    srgb_blend: bool,
    /// The owning `Renderer`'s uploads, when it has one; queue writes otherwise.
    staging: Option<rune_staging::Staging>,
    #[cfg(target_os = "macos")]
    importer: Option<macos::Importer>,
    #[cfg(target_os = "windows")]
    shared: Option<windows::Importer>,
}
impl Drop for SurfaceRenderer {
    fn drop(&mut self) {
        #[cfg(not(target_arch = "wasm32"))]
        {
            // Release the queue before joining so any in-flight completion callback won't submit
            // another timestamp resolve to a renderer that is going away.
            self.timing_queue.take();
            self.completion.stop();
        }
        #[cfg(target_os = "linux")]
        if let Some(dmabuf) = &mut self.dmabuf {
            dmabuf.stop();
        }
    }
}

impl SurfaceRenderer {
    #[cfg(target_arch = "wasm32")]
    pub fn new(_: &wgpu::Device, _: wgpu::TextureFormat) -> Self {
        panic!("native surfaces are unsupported on web")
    }
    #[cfg(target_arch = "wasm32")]
    #[doc(hidden)]
    pub fn build(_: &wgpu::Device, _: wgpu::TextureFormat, _: Box<dyn FnOnce() + Send>) -> Building {
        panic!("native surfaces are unsupported on web")
    }
    #[cfg(target_arch = "wasm32")]
    #[doc(hidden)]
    pub fn finish(_: &wgpu::Device, _: &Building) -> Option<Self> {
        None
    }

    /// Creates a native surface renderer targeting `format`.
    ///
    /// Blending defaults to sRGB-encoded (`srgb_blend() == true`) for 8-bit non-sRGB targets
    /// (`Rgba8Unorm` and `Bgra8Unorm`), and to linear light (`srgb_blend() == false`) for
    /// `*Srgb` (`Rgba8UnormSrgb`, `Bgra8UnormSrgb`) and floating-point HDR (`Rgba16Float`)
    /// targets. Override with [`Self::set_srgb_blend`].
    ///
    /// ```no_run
    /// # fn demo(device: &wgpu::Device) {
    /// use rune_media::native_surface::SurfaceRenderer;
    ///
    /// let srgb = SurfaceRenderer::new(device, wgpu::TextureFormat::Rgba8Unorm);
    /// assert!(srgb.srgb_blend());
    /// let linear = SurfaceRenderer::new(device, wgpu::TextureFormat::Rgba8UnormSrgb);
    /// assert!(!linear.srgb_blend());
    /// # }
    /// ```
    #[cfg(not(target_arch = "wasm32"))]
    pub fn new(device: &wgpu::Device, format: wgpu::TextureFormat) -> Self {
        let layout = Self::bind_layout(device);
        let pipeline = Self::pipeline(device, &layout, format, Blend::SourceOver);
        Self::with_pipeline(device, format, layout, pipeline)
    }

    /// Starts compiling the renderer's pipeline on a thread of its own, and runs `done` once
    /// it's ready. The shader compiler takes hundreds of milliseconds (FXC on Windows), which
    /// on the UI thread froze a window's input and held the first picture back.
    #[cfg(not(target_arch = "wasm32"))]
    #[doc(hidden)]
    pub fn build(device: &wgpu::Device, format: wgpu::TextureFormat, done: Box<dyn FnOnce() + Send>) -> Building {
        let layout = Self::bind_layout(device);
        let (sender, built) = std::sync::mpsc::sync_channel(1);
        let (punch_sender, punch) = std::sync::mpsc::sync_channel(1);
        let (device_for, layout_for) = (device.clone(), layout.clone());
        let spawned = std::thread::Builder::new().name("rune-video-pipeline".into()).spawn(move || {
            sender.send(Self::pipeline(&device_for, &layout_for, format, Blend::SourceOver)).ok();
            done();
            // A clip's layer (Windows) needs the punch; it compiles once the pictures can show.
            #[cfg(target_os = "windows")]
            punch_sender.send(Self::pipeline_with(&device_for, &layout_for, format, PUNCH)).ok();
            #[cfg(not(target_os = "windows"))]
            drop(punch_sender);
        });
        let ok = spawned.is_ok();
        Building { format, layout, built: ok.then_some(built), punch: std::sync::Mutex::new(ok.then_some(punch)) }
    }

    /// The renderer `building` made, once its pipeline is ready; a build whose thread couldn't
    /// start compiles here.
    #[cfg(not(target_arch = "wasm32"))]
    #[doc(hidden)]
    pub fn finish(device: &wgpu::Device, building: &Building) -> Option<Self> {
        let pipeline = match &building.built {
            Some(built) => built.try_recv().ok()?,
            None => Self::pipeline(device, &building.layout, building.format, Blend::SourceOver),
        };
        let mut renderer = Self::with_pipeline(device, building.format, building.layout.clone(), pipeline);
        // The punch arrives on this later.
        renderer.punch_building = building.punch.lock().unwrap().take();
        Some(renderer)
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn with_pipeline(device: &wgpu::Device, format: wgpu::TextureFormat, layout: wgpu::BindGroupLayout, pipeline: wgpu::RenderPipeline) -> Self {
        let timing_pool = device.features().contains(wgpu::Features::TIMESTAMP_QUERY).then(|| {
            Arc::new(TimingPool {
                queries: device.create_query_set(&wgpu::QuerySetDescriptor {
                    label: Some("video frame timestamps"),
                    ty: wgpu::QueryType::Timestamp,
                    count: TIMING_SLOTS * 2,
                }),
                free: std::sync::Mutex::new((0..TIMING_SLOTS).map(|i| i * 2).collect()),
            })
        });
        let resolves = Arc::new(completion::Parked::default());
        let completion = completion::CompletionWorker::spawn(device.clone(), resolves.clone());
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("video linear"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        Self {
            timing_queue: std::sync::OnceLock::new(),
            completion,
            resolves,
            #[cfg(target_os = "linux")]
            dmabuf: None,
            device: device.clone(),
            timing_pool,
            queries: None,
            queries_used: std::sync::atomic::AtomicBool::new(false),
            pipeline,
            format,
            extra: Vec::new(),
            additive: None,
            layout,
            sampler,
            cache: HashMap::new(),
            draws: Vec::new(),
            punched: Vec::new(),
            punch: None,
            punch_building: None,
            srgb_blend: matches!(format, wgpu::TextureFormat::Bgra8Unorm | wgpu::TextureFormat::Rgba8Unorm),
            staging: None,
            #[cfg(target_os = "macos")]
            importer: None,
            #[cfg(target_os = "windows")]
            shared: None,
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn bind_layout(device: &wgpu::Device) -> wgpu::BindGroupLayout {
        device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("video"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                texture_entry(1),
                texture_entry(2),
                wgpu::BindGroupLayoutEntry {
                    binding: 3,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        })
    }

    /// Overrides whether video frames are output in sRGB-encoded space for blending (`true` by
    /// default for `Rgba8Unorm` and `Bgra8Unorm`; `false` by default for `*Srgb` and
    /// floating-point targets).
    ///
    /// ```no_run
    /// # fn demo(device: &wgpu::Device) {
    /// use rune_media::native_surface::SurfaceRenderer;
    ///
    /// let mut renderer = SurfaceRenderer::new(device, wgpu::TextureFormat::Rgba8UnormSrgb);
    /// renderer.set_srgb_blend(false);
    /// assert!(!renderer.srgb_blend());
    /// # }
    /// ```
    pub fn set_srgb_blend(&mut self, enabled: bool) {
        self.srgb_blend = enabled;
    }

    /// True when video frames are output in sRGB-encoded space for non-sRGB targets; false when
    /// output in linear light for `*Srgb` or floating-point targets.
    pub fn srgb_blend(&self) -> bool {
        self.srgb_blend
    }

    #[doc(hidden)]
    pub fn set_staging(&mut self, staging: rune_staging::Staging) {
        self.staging = Some(staging);
    }

    fn pipeline(device: &wgpu::Device, layout: &wgpu::BindGroupLayout, format: wgpu::TextureFormat, blend: Blend) -> wgpu::RenderPipeline {
        Self::pipeline_with(device, layout, format, blend_state(blend))
    }

    fn pipeline_with(device: &wgpu::Device, layout: &wgpu::BindGroupLayout, format: wgpu::TextureFormat, blend: wgpu::BlendState) -> wgpu::RenderPipeline {
        #[cfg(target_os = "windows")]
        if let Some(pipeline) = super::dxbc::pipeline(device, |vertex, fragment| Self::make_pipeline(device, layout, format, blend, vertex, fragment)) {
            return pipeline;
        }
        Self::wgsl_pipeline(device, layout, format, blend)
    }

    fn wgsl_pipeline(device: &wgpu::Device, layout: &wgpu::BindGroupLayout, format: wgpu::TextureFormat, blend: wgpu::BlendState) -> wgpu::RenderPipeline {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("video"),
            source: wgpu::ShaderSource::Wgsl(WGSL.into()),
        });
        Self::make_pipeline(device, layout, format, blend, &shader, &shader)
    }

    /// The pipeline from WGSL whatever the precompiled bytecode is, for `regen_dxbc` and its
    /// freshness test, which take the HLSL wgpu logs while it compiles.
    #[cfg(target_os = "windows")]
    #[doc(hidden)]
    pub fn compile_wgsl_now(device: &wgpu::Device, format: wgpu::TextureFormat) {
        let layout = Self::bind_layout(device);
        drop(Self::wgsl_pipeline(device, &layout, format, blend_state(Blend::SourceOver)));
    }

    fn make_pipeline(
        device: &wgpu::Device,
        layout: &wgpu::BindGroupLayout,
        format: wgpu::TextureFormat,
        blend: wgpu::BlendState,
        vertex: &wgpu::ShaderModule,
        fragment: &wgpu::ShaderModule,
    ) -> wgpu::RenderPipeline {
        let pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("video"),
            bind_group_layouts: &[Some(layout)],
            immediate_size: 0,
        });
        device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("video"),
            layout: Some(&pl),
            vertex: wgpu::VertexState {
                module: vertex,
                entry_point: Some("vs"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            primitive: wgpu::PrimitiveState { topology: wgpu::PrimitiveTopology::TriangleStrip, ..Default::default() },
            depth_stencil: None,
            multisample: Default::default(),
            fragment: Some(wgpu::FragmentState {
                module: fragment,
                entry_point: Some("fs"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: Some(blend),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: None,
        })
    }

    #[doc(hidden)]
    pub fn add_format(&mut self, device: &wgpu::Device, format: wgpu::TextureFormat) {
        if format != self.format && !self.extra.iter().any(|(f, _)| *f == format) {
            self.extra.push((format, Self::pipeline(device, &self.layout, format, Blend::SourceOver)));
        }
    }

    #[doc(hidden)]
    pub fn add_plus_lighter(&mut self, device: &wgpu::Device, format: wgpu::TextureFormat) {
        if self.additive.as_ref().is_none_or(|(f, _)| *f != format) {
            self.additive = Some((format, Self::pipeline(device, &self.layout, format, Blend::PlusLighter)));
        }
    }

    #[cfg(all(test, target_os = "macos"))]
    pub(super) fn live_planes(&self) -> usize {
        self.importer.as_ref().map_or(0, |i| i.live_planes.load(std::sync::atomic::Ordering::SeqCst))
    }

    #[cfg(test)]
    pub(super) fn kept_bind_counts(&self) -> Vec<usize> {
        self.draws.iter().map(|d| d.binds.len()).collect()
    }

    #[cfg(all(test, target_os = "windows"))]
    pub(super) fn windows_importer(&self) -> Option<&windows::Importer> {
        self.shared.as_ref()
    }

    #[cfg(target_os = "windows")]
    #[doc(hidden)]
    /// Punches these surfaces' places out of the window's pixels from the next `prepare` on,
    /// once the punch pipeline has compiled (`punch_ready`).
    pub fn set_punched(&mut self, keys: Vec<usize>) {
        self.punched = keys;
    }

    /// The punch pipeline has compiled. A renderer made without `build` starts compiling it here,
    /// on a thread of its own as `build` does.
    #[cfg(target_os = "windows")]
    #[doc(hidden)]
    pub fn punch_ready(&mut self) -> bool {
        if self.punch.is_some() {
            return true;
        }
        if self.punch_building.is_none() {
            let (sender, receiver) = std::sync::mpsc::sync_channel(1);
            let (device, layout, format) = (self.device.clone(), self.layout.clone(), self.format);
            let spawned = std::thread::Builder::new()
                .name("rune-video-punch".into())
                .spawn(move || drop(sender.send(Self::pipeline_with(&device, &layout, format, PUNCH))));
            if spawned.is_err() {
                return false;
            }
            self.punch_building = Some(receiver);
        }
        if let Some(p) = self.punch_building.as_ref().and_then(|b| b.try_recv().ok()) {
            self.punch = Some(p);
            self.punch_building = None;
        }
        self.punch.is_some()
    }
}

pub(super) fn descriptor(width: u32, height: u32, format: wgpu::TextureFormat) -> wgpu::TextureDescriptor<'static> {
    wgpu::TextureDescriptor {
        label: Some("video plane"),
        size: wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::TEXTURE_BINDING,
        view_formats: &[],
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn texture_entry(binding: u32) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Texture {
            sample_type: wgpu::TextureSampleType::Float { filterable: true },
            view_dimension: wgpu::TextureViewDimension::D2,
            multisampled: false,
        },
        count: None,
    }
}
