//! FxRenderer: one pipeline per effect family (built ahead of time), one storage buffer of
//! instances, one of motion poses. `prepare` uploads and culls; `draw_layer` / `draw_all` encode
//! into the caller's render pass (target must not be the backdrop texture).

use crate::*;
use crate::binds::BindSet;
use wgpu::util::DeviceExt;

#[path = "renderer_prepare.rs"]
mod prepare;

const CLIP: &str = include_str!("../../rune/src/clip.wgsl");
const COMMON: &str = include_str!("shaders/common.wgsl");
const SOURCES: [(&str, &str); KINDS] = [
    ("glow", include_str!("shaders/glow.wgsl")),
    ("pulse", include_str!("shaders/pulse.wgsl")),
    ("electric", include_str!("shaders/electric.wgsl")),
    ("fire", include_str!("shaders/fire.wgsl")),
    ("distort", include_str!("shaders/distort.wgsl")),
    ("motion", include_str!("shaders/motion.wgsl")),
    ("extras", include_str!("shaders/extras.wgsl")),
    ("fill", include_str!("shaders/fill.wgsl")),
];

/// Everything one frame of effects needs.
pub struct FxFrame<'a> {
    pub viewport: [u32; 2],
    /// Device px per pt.
    pub scale: f32,
    pub layers: &'a [FxLayer],
    /// The target's content beneath the layers (for distortion and glass). Must not be the render
    /// target. Samples are linear by default (including hardware-decoded `*Srgb` views).
    /// For sRGB-encoded premultiplied samples, use `prepare_with_slots` with
    /// `FxSlot::backdrop_srgb(true)`. A frame prepared with `None` instead defaults per-slot
    /// copies (`set_layer_backdrop` / `blur_backdrop`) to the frame's blend encoding, matching
    /// `EffectSite::backdrop`, including `Rgba16Float` isolated layers.
    pub backdrop: Option<&'a wgpu::TextureView>,
    /// Source textures layers refer to by `Source::slot` (mask pyramids, node layers).
    pub sources: &'a [&'a wgpu::TextureView],
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FxStats {
    pub drawn: u32,
    pub culled_idle: u32,
    pub culled_offscreen: u32,
    /// Covered device pixels across drawn quads.
    pub pixels: u64,
    pub needs_backdrop: bool,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct FxOptions {
    /// Add TPDF dither in the effect shaders (set for 8-bit targets; leave off for 16f targets
    /// that dither once at the final resolve).
    pub dither: bool,
}

#[derive(Clone, Copy)]
struct Prepared {
    kind: usize,
    instance: u32,
    bind: usize,
    /// Drawn quad, device px (x0, y0, x1, y1).
    bounds: [f32; 4],
    /// Frosted lens: blur sigma in device px (0 = no backdrop blur).
    blur_px: f32,
    needs_backdrop: bool,
    backdrop_srgb: bool,
    blend: FxBlend,
}

/// Backdrop blur pyramid: half-resolution mip chains for the dual-filter down and up passes.
struct BlurPyramid {
    size: [u32; 2],
    down: Vec<wgpu::TextureView>,
    up: Vec<wgpu::TextureView>,
    /// Up chain level 0 (half resolution): what a frosted lens samples.
    result: wgpu::TextureView,
    /// Down pass into mip m + 1 (from mip m); the pass into mip 0 reads the backdrop.
    down_binds: Vec<wgpu::BindGroup>,
    /// Up pass into mip m from down mip m + 1 (the coarsest level used) or from up mip m + 1.
    up_binds: Vec<(wgpu::BindGroup, Option<wgpu::BindGroup>)>,
}

const BLUR_MIPS: u32 = 6;
const BLUR_SLOTS: usize = 64;

/// Group 1's resources: backdrop, source and blurred backdrop views, and the backdrop encoding.
pub(crate) type Fx1Key = ([wgpu::TextureView; 3], usize);

/// The index of a group binding `views` and encoding `enc`, made only when no group of this or last frame does.
pub(crate) fn fx1(
    binds: &mut BindSet<Fx1Key>,
    device: &wgpu::Device,
    bgl: &wgpu::BindGroupLayout,
    sampler: &wgpu::Sampler,
    encoding: &[wgpu::Buffer; 4],
    views: [&wgpu::TextureView; 3],
    enc: usize,
) -> usize {
    let is = |(v, e): &Fx1Key| *e == enc && v[0] == *views[0] && v[1] == *views[1] && v[2] == *views[2];
    binds.find(is, || {
        let bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("fx1"),
            layout: bgl,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(views[0]) },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(views[1]) },
                wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::Sampler(sampler) },
                wgpu::BindGroupEntry { binding: 3, resource: wgpu::BindingResource::TextureView(views[2]) },
                wgpu::BindGroupEntry { binding: 4, resource: encoding[enc].as_entire_binding() },
            ],
        });
        ((views.map(Clone::clone), enc), bg)
    })
}

/// A blur pass's inputs; the uniform slot is a dynamic offset.
fn blur_bind(device: &wgpu::Device, bgl: &wgpu::BindGroupLayout, src: &wgpu::TextureView, fine: &wgpu::TextureView, sampler: &wgpu::Sampler, uniforms: &wgpu::Buffer) -> wgpu::BindGroup {
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("fx blur"),
        layout: bgl,
        entries: &[
            wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(src) },
            wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(fine) },
            wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::Sampler(sampler) },
            wgpu::BindGroupEntry { binding: 3, resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding { buffer: uniforms, offset: 0, size: wgpu::BufferSize::new(16) }) },
        ],
    })
}

impl BlurPyramid {
    fn new(device: &wgpu::Device, viewport: [u32; 2], bgl: &wgpu::BindGroupLayout, dummy: &wgpu::TextureView, sampler: &wgpu::Sampler, uniforms: &wgpu::Buffer) -> BlurPyramid {
        let size = [viewport[0].div_ceil(2).max(1), viewport[1].div_ceil(2).max(1)];
        let mips = BLUR_MIPS.min(32 - size[0].max(size[1]).leading_zeros());
        let make = |label| {
            let t = device.create_texture(&wgpu::TextureDescriptor {
                label: Some(label),
                size: wgpu::Extent3d { width: size[0], height: size[1], depth_or_array_layers: 1 },
                mip_level_count: mips,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba16Float,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            });
            (0..mips)
                .map(|m| t.create_view(&wgpu::TextureViewDescriptor { base_mip_level: m, mip_level_count: Some(1), ..Default::default() }))
                .collect::<Vec<_>>()
        };
        let down = make("fx blur down");
        let up = make("fx blur up");
        let result = up[0].clone();
        let bind = |src, fine| blur_bind(device, bgl, src, fine, sampler, uniforms);
        let m = down.len();
        let down_binds = (1..m).map(|k| bind(&down[k - 1], dummy)).collect();
        let up_binds = (0..m - 1).map(|k| (bind(&down[k + 1], &down[k]), (k + 2 < m).then(|| bind(&up[k + 1], &down[k])))).collect();
        BlurPyramid { size: viewport, down, up, result, down_binds, up_binds }
    }
}

pub struct FxRenderer {
    pipelines: Vec<[wgpu::RenderPipeline; 2]>,
    format: wgpu::TextureFormat,
    layout: wgpu::PipelineLayout,
    modules: Vec<wgpu::ShaderModule>,
    /// Pipelines for other target formats (`add_format`), one pair per effect family.
    extra: Vec<(wgpu::TextureFormat, Vec<[wgpu::RenderPipeline; 2]>)>,
    srgb: Option<bool>,
    bgl0: wgpu::BindGroupLayout,
    bgl1: wgpu::BindGroupLayout,
    backdrop_encoding: [wgpu::Buffer; 4],
    bg0: Option<wgpu::BindGroup>,
    globals: wgpu::Buffer,
    insts: wgpu::Buffer,
    inst_cap: usize,
    xforms: wgpu::Buffer,
    xform_cap: usize,
    sampler: wgpu::Sampler,
    dummy: wgpu::TextureView,
    binds: BindSet<Fx1Key>,
    /// The blur pyramid's first pass per backdrop.
    blur_binds: BindSet<wgpu::TextureView>,
    prepared: Vec<Option<Prepared>>,
    scratch: Vec<InstRaw>,
    scratch_xf: Vec<[f32; 4]>,
    mip_pipeline: wgpu::RenderPipeline,
    mip_bgl: wgpu::BindGroupLayout,
    blur: Option<BlurPyramid>,
    blur_down: wgpu::RenderPipeline,
    blur_up: wgpu::RenderPipeline,
    blur_bgl: wgpu::BindGroupLayout,
    blur_uniforms: wgpu::Buffer,
    blur_slot: usize,
    viewport: [u32; 2],
    opts: FxOptions,
    sims: Option<crate::sim::Sims>,
    /// Upload staging, made by the first `prepare`, and the globals it last wrote.
    staging: Option<(rune_staging::Staging, [f32; 4])>,
}

const INST_SIZE: usize = std::mem::size_of::<InstRaw>();

impl FxRenderer {
    pub fn new(device: &wgpu::Device, format: wgpu::TextureFormat, opts: FxOptions) -> FxRenderer {
        let entry = |binding, visibility, ty| wgpu::BindGroupLayoutEntry { binding, visibility, ty, count: None };
        let vf = wgpu::ShaderStages::VERTEX_FRAGMENT;
        let fr = wgpu::ShaderStages::FRAGMENT;
        let storage = wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Storage { read_only: true },
            has_dynamic_offset: false,
            min_binding_size: None,
        };
        let bgl0 = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("fx0"),
            entries: &[
                entry(
                    0,
                    vf,
                    wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                ),
                entry(1, vf, storage),
                entry(2, fr, storage),
            ],
        });
        let tex = wgpu::BindingType::Texture {
            sample_type: wgpu::TextureSampleType::Float { filterable: true },
            view_dimension: wgpu::TextureViewDimension::D2,
            multisampled: false,
        };
        let bgl1 = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("fx1"),
            entries: &[
                entry(0, fr, tex),
                entry(1, fr, tex),
                entry(2, fr, wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering)),
                entry(3, fr, tex),
                entry(4, fr, wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: wgpu::BufferSize::new(16) }),
            ],
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("fx"),
            bind_group_layouts: &[Some(&bgl0), Some(&bgl1)],
            immediate_size: 0,
        });
        let backdrop_encoding = [(0.0f32, 0.0f32), (1.0, 0.0), (0.0, 1.0), (1.0, 1.0)].map(|(encoded, present)| {
            device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("fx backdrop encoding"),
                contents: bytemuck::cast_slice(&[encoded, present, 0.0, 0.0]),
                usage: wgpu::BufferUsages::UNIFORM,
            })
        });
        let modules: Vec<wgpu::ShaderModule> = SOURCES
            .iter()
            .map(|(name, src)| device.create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some(name),
                source: wgpu::ShaderSource::Wgsl(format!("{CLIP}\n{COMMON}\n{src}").into()),
            }))
            .collect();
        let pipelines = Self::pipelines(device, &layout, &modules, format);
        let globals = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("fx globals"),
            size: 16,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("fx linear"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Linear,
            ..Default::default()
        });
        let dummy = device
            .create_texture(&wgpu::TextureDescriptor {
                label: Some("fx dummy"),
                size: wgpu::Extent3d { width: 1, height: 1, depth_or_array_layers: 1 },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba8Unorm,
                usage: wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            })
            .create_view(&Default::default());

        // Mask pyramid downsampler (Rgba16Float levels).
        let mip_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("fx mip"),
            entries: &[entry(0, fr, tex), entry(1, fr, wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering))],
        });
        let mip_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("fx mip"),
            bind_group_layouts: &[Some(&mip_bgl)],
            immediate_size: 0,
        });
        let mip_module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("fx mip"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shaders/mip.wgsl").into()),
        });
        let mip_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("fx mip"),
            layout: Some(&mip_layout),
            vertex: wgpu::VertexState {
                module: &mip_module,
                entry_point: Some("vs"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            primitive: Default::default(),
            depth_stencil: None,
            multisample: Default::default(),
            fragment: Some(wgpu::FragmentState {
                module: &mip_module,
                entry_point: Some("fs"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: MaskPyramid::FORMAT,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: None,
        });

        // Backdrop blur pyramid (frosted glass).
        let blur_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("fx blur"),
            entries: &[
                entry(0, fr, tex),
                entry(1, fr, tex),
                entry(2, fr, wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering)),
                entry(3, fr, wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: true, min_binding_size: wgpu::BufferSize::new(16) }),
            ],
        });
        let blur_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("fx blur"),
            bind_group_layouts: &[Some(&blur_bgl)],
            immediate_size: 0,
        });
        let blur_module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("fx blur"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shaders/blur.wgsl").into()),
        });
        let blur_pipe = |entry: &str| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(entry),
                layout: Some(&blur_layout),
                vertex: wgpu::VertexState { module: &blur_module, entry_point: Some("vs"), compilation_options: Default::default(), buffers: &[] },
                primitive: Default::default(),
                depth_stencil: None,
                multisample: Default::default(),
                fragment: Some(wgpu::FragmentState {
                    module: &blur_module,
                    entry_point: Some(entry),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState { format: wgpu::TextureFormat::Rgba16Float, blend: None, write_mask: wgpu::ColorWrites::ALL })],
                }),
                multiview_mask: None,
                cache: None,
            })
        };
        let blur_down = blur_pipe("fs_down");
        let blur_up = blur_pipe("fs_up");
        let blur_uniforms = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("fx blur params"),
            size: (BLUR_SLOTS * 256) as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let inst_cap = 64;
        let xform_cap = 4096;
        FxRenderer {
            pipelines,
            format,
            layout,
            modules,
            extra: Vec::new(),
            srgb: None,
            bgl0,
            bgl1,
            backdrop_encoding,
            bg0: None,
            globals,
            insts: Self::buffer(device, "fx insts", inst_cap * INST_SIZE),
            inst_cap,
            xforms: Self::buffer(device, "fx xforms", xform_cap * 16),
            xform_cap,
            sampler,
            dummy,
            binds: BindSet::new(),
            blur_binds: BindSet::new(),
            prepared: Vec::new(),
            scratch: Vec::new(),
            scratch_xf: Vec::new(),
            mip_pipeline,
            mip_bgl,
            blur: None,
            blur_down,
            blur_up,
            blur_bgl,
            blur_uniforms,
            blur_slot: 0,
            viewport: [0, 0],
            opts,
            sims: None,
            staging: None,
        }
    }

    fn pipelines(device: &wgpu::Device, layout: &wgpu::PipelineLayout, modules: &[wgpu::ShaderModule], format: wgpu::TextureFormat) -> Vec<[wgpu::RenderPipeline; 2]> {
        let blends = [
            wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING,
            wgpu::BlendState {
                color: wgpu::BlendComponent {
                    src_factor: wgpu::BlendFactor::One,
                    dst_factor: wgpu::BlendFactor::OneMinusSrc,
                    operation: wgpu::BlendOperation::Add,
                },
                alpha: wgpu::BlendComponent::OVER,
            },
        ];
        modules
            .iter()
            .zip(SOURCES)
            .map(|(module, (name, _))| {
                blends.map(|blend| {
                    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                        label: Some(name),
                        layout: Some(layout),
                        vertex: wgpu::VertexState { module, entry_point: Some("vs"), compilation_options: Default::default(), buffers: &[] },
                        primitive: wgpu::PrimitiveState { topology: wgpu::PrimitiveTopology::TriangleStrip, ..Default::default() },
                        depth_stencil: None,
                        multisample: Default::default(),
                        fragment: Some(wgpu::FragmentState {
                            module,
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
                })
            })
            .collect()
    }

    /// Builds the effect pipelines for another target format, once, so one renderer serves a
    /// window target and the `Rgba16Float` target of an isolated layer alike (`draw_layer_in`).
    pub fn add_format(&mut self, device: &wgpu::Device, format: wgpu::TextureFormat) {
        if format != self.format && !self.extra.iter().any(|(f, _)| *f == format) {
            let p = Self::pipelines(device, &self.layout, &self.modules, format);
            self.extra.push((format, p));
        }
    }

    /// Match the frame's blend mode (`rune::Frame::srgb_blend`): when true, the premultiplied
    /// output is sRGB-encoded for a non-sRGB target. Per-slot copies default to that same
    /// encoding when prepared without `FxFrame::backdrop`; explicit frame inputs retain their
    /// linear-samples contract. `FxSlot::backdrop_srgb` overrides either convention.
    /// Takes effect at the next `prepare`.
    pub fn set_srgb_blend(&mut self, on: bool) {
        self.srgb = Some(on);
    }

    fn set_format(&mut self, device: &wgpu::Device, format: wgpu::TextureFormat) {
        if format == self.format {
            return;
        }
        if let Some((f, p)) = self.extra.iter_mut().find(|(f, _)| *f == format) {
            std::mem::swap(&mut self.pipelines, p);
            *f = self.format;
        } else {
            let p = Self::pipelines(device, &self.layout, &self.modules, format);
            let old = std::mem::replace(&mut self.pipelines, p);
            self.extra.push((self.format, old));
        }
        self.format = format;
    }

    fn buffer(device: &wgpu::Device, label: &str, size: usize) -> wgpu::Buffer {
        device.create_buffer(&wgpu::BufferDescriptor {
            label: Some(label),
            size: size.max(256) as u64,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        })
    }

    /// Simulate fire and smoke (styles `Top` and `Heat`) as small fluid grids instead of noise.
    /// Off by default; when on, call `simulate` every frame after `prepare` and before drawing.
    pub fn set_fire_sim(&mut self, device: &wgpu::Device, on: bool) {
        match (on, self.sims.is_some()) {
            (true, false) => self.sims = Some(crate::sim::Sims::new(device)),
            (false, true) => self.sims = None,
            _ => {}
        }
    }

    /// Whether `simulate` has steps to record this frame (false when every fire is frozen or culled).
    pub fn simulate_pending(&self) -> bool {
        self.sims.as_ref().is_some_and(|s| s.pending())
    }

    /// Record this frame's fire simulation steps (fixed 1/60 s steps) as one compute pass. Call
    /// after `prepare`, outside any render pass, before drawing the fire layers. Returns false and
    /// records nothing when no step is due.
    pub fn simulate(&mut self, queue: &wgpu::Queue, encoder: &mut wgpu::CommandEncoder, timestamps: Option<wgpu::ComputePassTimestampWrites<'_>>) -> bool {
        self.sims.as_mut().is_some_and(|s| s.record(self.staging.as_ref().map(|(s, _)| s), queue, encoder, timestamps))
    }

    /// The group-1 bind group `layer` was prepared with; `None` when it was culled. For tests.
    #[doc(hidden)]
    pub fn layer_bind_group(&self, layer: usize) -> Option<&wgpu::BindGroup> {
        let p = self.prepared.get(layer).copied().flatten()?;
        Some(self.binds.get(p.bind))
    }

    /// Draw one prepared layer (no-op when it was culled).
    pub fn draw_layer(&self, pass: &mut wgpu::RenderPass<'_>, layer: usize) {
        self.draw_layer_in(pass, self.format, [0.0; 2], layer);
    }

    /// `draw_layer` into a pass whose target has `format` (the renderer's own, or one from
    /// `add_format`) and whose top-left pixel sits at `origin` in window device pixels: zero for
    /// the frame target, `rune::render::EffectSite::origin` inside an isolated layer. Effects are
    /// shaded in window coordinates; the viewport is shifted so they land in that target, so they
    /// take the layer's opacity, blur and clip when it composites. Resets the viewport after.
    pub fn draw_layer_in(&self, pass: &mut wgpu::RenderPass<'_>, format: wgpu::TextureFormat, origin: [f32; 2], layer: usize) {
        let Some(Some(p)) = self.prepared.get(layer) else { return };
        let pipelines = if format == self.format { &self.pipelines }
            else { &self.extra.iter().find(|(f, _)| *f == format).expect("FxRenderer::add_format first").1 };
        let shifted = origin != [0.0; 2];
        if shifted {
            pass.set_viewport(-origin[0], -origin[1], self.viewport[0] as f32, self.viewport[1] as f32, 0.0, 1.0);
        }
        let blend_idx = match p.blend {
            FxBlend::Screen => 1,
            FxBlend::Normal | FxBlend::Multiply | FxBlend::Overlay | FxBlend::SoftLight => 0,
        };
        pass.set_pipeline(&pipelines[p.kind][blend_idx]);
        pass.set_bind_group(0, self.bg0.as_ref().unwrap(), &[]);
        pass.set_bind_group(1, self.binds.get(p.bind), &[]);
        pass.draw(0..4, p.instance..p.instance + 1);
        if shifted {
            pass.set_viewport(0.0, 0.0, self.viewport[0] as f32, self.viewport[1] as f32, 0.0, 1.0);
        }
    }

    /// Draw every prepared layer with `place` in order.
    pub fn draw_all(&self, pass: &mut wgpu::RenderPass<'_>, layers: &[FxLayer], place: Option<Place>) {
        for (i, l) in layers.iter().enumerate() {
            if place.is_none_or(|p| p == l.place) {
                self.draw_layer(pass, i);
            }
        }
    }

    /// Bind a per-slot `backdrop` (`EffectSite::backdrop`) to `layer`. `blur_backdrop` calls this
    /// automatically, so call it directly only when drawing a backdrop layer without `blur_backdrop`.
    /// Uses the encoding chosen at `prepare` (`FxSlot::backdrop_srgb`); when prepared without
    /// a frame backdrop, the default expects `EffectSite::backdrop` in the pass's own format.
    pub fn set_layer_backdrop(&mut self, device: &wgpu::Device, backdrop: &wgpu::TextureView, layer: usize) {
        let Some(Some(p)) = self.prepared.get(layer).copied() else { return };
        if !p.needs_backdrop { return; }
        let blurred = if p.blur_px > 0.0 {
            self.blur.as_ref().map_or(&self.dummy, |b| &b.result)
        } else {
            &self.dummy
        };
        let enc = 2 | usize::from(p.backdrop_srgb);
        let bind = fx1(&mut self.binds, device, &self.bgl1, &self.sampler, &self.backdrop_encoding, [backdrop, &self.dummy, blurred], enc);
        self.prepared[layer].as_mut().unwrap().bind = bind;
    }

    /// Bind `backdrop` to `layer` and blur it for frosted glass (`LensParams::frost` or
    /// `FlutedGlassParams::frost` above 0), scissored to the layer.
    ///
    /// Call it after copying the backdrop for layer `layer` (for instance from the `EffectPhase::Backdrop`
    /// callback of `Renderer::draw_with_backdrop` / `Renderer::encode_layers_with_backdrop` with
    /// `EffectSite::backdrop`) and before drawing that layer, outside any render pass. Returns
    /// false (after still binding `backdrop` when the layer reads it) for non-frosted layers; a
    /// frosted lens drawn without it falls back to a small in-shader blur.
    pub fn blur_backdrop(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, encoder: &mut wgpu::CommandEncoder, backdrop: &wgpu::TextureView, layer: usize) -> bool {
        let staging = self.staging(device, queue);
        self.set_layer_backdrop(device, backdrop, layer);
        let Some(Some(p)) = self.prepared.get(layer) else { return false };
        let (Some(pyr), true) = (self.blur.as_ref(), p.blur_px > 0.0) else { return false };
        let mips = pyr.down.len();
        if mips < 2 || self.blur_slot + mips > BLUR_SLOTS {
            return false;
        }
        // Pyramid level (half-res mip) whose blur matches the sigma; the fraction crossfades levels.
        let level = (p.blur_px / 2.0).max(1.0).log2().clamp(0.0, (mips - 1) as f32);
        let n = (level.ceil() as usize).clamp(1, mips - 1);
        let frac = (level - (n - 1) as f32).clamp(0.0, 1.0);
        let margin = p.blur_px * 3.0 + 8.0;
        let [vw, vh] = self.viewport;
        let region = [(p.bounds[0] - margin).max(0.0), (p.bounds[1] - margin).max(0.0), (p.bounds[2] + margin).min(vw as f32), (p.bounds[3] + margin).min(vh as f32)];
        let base = self.blur_slot;
        self.blur_slot += mips;
        let mut params = [0u8; BLUR_MIPS as usize * 256];
        for k in 0..mips {
            let w: f32 = if k + 1 == n { frac } else { 1.0 };
            params[k * 256..k * 256 + 4].copy_from_slice(&w.to_le_bytes());
        }
        staging.write_buffer(&self.blur_uniforms, (base * 256) as u64, &params[..mips * 256]);
        // Tell the lens shader the pyramid is ready (xf.x), and how much of it to use (xf.y).
        let xf = [1.0f32, (p.blur_px / 3.0).min(1.0), 0.0, 0.0];
        let off = p.instance as usize * INST_SIZE + std::mem::offset_of!(InstRaw, xf);
        staging.write_buffer(&self.insts, off as u64, bytemuck::cast_slice(&xf));
        staging.flush(encoder);
        let scissor = |m: usize| {
            let k = (2u32 << m) as f32;
            let w = (vw.div_ceil(2) >> m).max(1);
            let h = (vh.div_ceil(2) >> m).max(1);
            let x0 = ((region[0] / k).floor() as u32).saturating_sub(2).min(w - 1);
            let y0 = ((region[1] / k).floor() as u32).saturating_sub(2).min(h - 1);
            let x1 = ((region[2] / k).ceil() as u32 + 2).min(w);
            let y1 = ((region[3] / k).ceil() as u32 + 2).min(h);
            (x0, y0, x1.max(x0 + 1) - x0, y1.max(y0 + 1) - y0)
        };
        let first = self.blur_binds.find(|k| k == backdrop, || {
            (backdrop.clone(), blur_bind(device, &self.blur_bgl, backdrop, &self.dummy, &self.sampler, &self.blur_uniforms))
        });
        let first = self.blur_binds.get(first);
        let mut run = |pipe: &wgpu::RenderPipeline, dst: &wgpu::TextureView, bg: &wgpu::BindGroup, slot: usize, m: usize| {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("fx blur"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: dst,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations { load: wgpu::LoadOp::Load, store: wgpu::StoreOp::Store },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            let (x, y, w, h) = scissor(m);
            pass.set_scissor_rect(x, y, w, h);
            pass.set_pipeline(pipe);
            pass.set_bind_group(0, bg, &[(slot * 256) as u32]);
            pass.draw(0..3, 0..1);
        };
        for m in 0..=n {
            let bg = if m == 0 { first } else { &pyr.down_binds[m - 1] };
            run(&self.blur_down, &pyr.down[m], bg, base + m, m);
        }
        for m in (0..n).rev() {
            let (from_down, from_up) = &pyr.up_binds[m];
            let bg = if m + 1 == n { from_down } else { from_up.as_ref().unwrap() };
            run(&self.blur_up, &pyr.up[m], bg, base + m, m);
        }
        true
    }

    /// Fill mips 1.. of a mask pyramid from its level 0 (render the mask into `pyramid.level(0)` first).
    pub fn build_mips(&self, device: &wgpu::Device, encoder: &mut wgpu::CommandEncoder, pyramid: &MaskPyramid) {
        let binds = pyramid.binds.get_or_init(|| {
            (1..pyramid.mips)
                .map(|lvl| {
                    device.create_bind_group(&wgpu::BindGroupDescriptor {
                        label: Some("fx mip"),
                        layout: &self.mip_bgl,
                        entries: &[
                            wgpu::BindGroupEntry {
                                binding: 0,
                                resource: wgpu::BindingResource::TextureView(&pyramid.levels[(lvl - 1) as usize]),
                            },
                            wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(&self.sampler) },
                        ],
                    })
                })
                .collect()
        });
        for (lvl, bg) in (1..pyramid.mips).zip(binds) {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("fx mip"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &pyramid.levels[lvl as usize],
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT), store: wgpu::StoreOp::Store },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_pipeline(&self.mip_pipeline);
            pass.set_bind_group(0, bg, &[]);
            pass.draw(0..3, 0..1);
        }
    }
}

/// A mask (text, icon) and its blur pyramid, for `GlowStyle::Mask` and mask shimmer.
pub struct MaskPyramid {
    pub texture: wgpu::Texture,
    /// All mips: bind this as the layer source.
    pub view: wgpu::TextureView,
    /// One view per mip level (level 0 is the render target for the mask itself).
    pub levels: Vec<wgpu::TextureView>,
    pub mips: u32,
    pub size: [u32; 2],
    binds: std::sync::OnceLock<Vec<wgpu::BindGroup>>,
}

impl MaskPyramid {
    pub const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;

    pub fn new(device: &wgpu::Device, width: u32, height: u32) -> MaskPyramid {
        let mips = (32 - width.max(height).leading_zeros()).clamp(1, 7);
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("fx mask pyramid"),
            size: wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
            mip_level_count: mips,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: Self::FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let view = texture.create_view(&Default::default());
        let levels = (0..mips)
            .map(|m| {
                texture.create_view(&wgpu::TextureViewDescriptor {
                    base_mip_level: m,
                    mip_level_count: Some(1),
                    ..Default::default()
                })
            })
            .collect();
        MaskPyramid { texture, view, levels, mips, size: [width, height], binds: std::sync::OnceLock::new() }
    }
}
