//! Offscreen HDR render targets, resolve/blit shaders and async GPU frame timer.

use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};

use rune_fx::{FxOptions, FxRenderer, MaskPyramid};
use rune_light::LightRenderer;

pub(super) const HDR: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;

pub(super) struct Tex {
    pub(super) tex: wgpu::Texture,
    pub(super) view: wgpu::TextureView,
}

fn tex(device: &wgpu::Device, label: &str, size: (u32, u32), usage: wgpu::TextureUsages) -> Tex {
    let tex = device.create_texture(&wgpu::TextureDescriptor {
        label: Some(label),
        size: wgpu::Extent3d { width: size.0, height: size.1, depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: HDR,
        usage,
        view_formats: &[],
    });
    let view = tex.create_view(&Default::default());
    Tex { tex, view }
}

const SHADERS: &str = r#"
@group(0) @binding(0) var src: texture_2d<f32>;
@group(0) @binding(1) var<uniform> alpha: vec4f;

struct V { @builtin(position) pos: vec4f }

@vertex
fn vs(@builtin(vertex_index) vi: u32) -> V {
    let p = vec2f(f32((vi << 1u) & 2u), f32(vi & 2u));
    var o: V;
    o.pos = vec4f(p * 2.0 - 1.0, 0.0, 1.0);
    return o;
}

fn pcg(v: u32) -> u32 {
    let s = v * 747796405u + 2891336453u;
    let w = ((s >> ((s >> 28u) + 4u)) ^ s) * 277803737u;
    return (w >> 22u) ^ w;
}
fn u01(n: u32) -> f32 { return f32(pcg(n) >> 8u) * (1.0 / 16777216.0); }
fn to_srgb(c: vec3f) -> vec3f {
    let lo = c * 12.92;
    let hi = 1.055 * pow(max(c, vec3f(0.0)), vec3f(1.0 / 2.4)) - 0.055;
    return select(hi, lo, c <= vec3f(0.0031308));
}
fn from_srgb(c: vec3f) -> vec3f {
    let lo = c / 12.92;
    let hi = pow((max(c, vec3f(0.0)) + 0.055) / 1.055, vec3f(2.4));
    return select(hi, lo, c <= vec3f(0.04045));
}
fn dithered(pos: vec2f) -> vec3f {
    let ip = vec2u(pos);
    let c = textureLoad(src, vec2i(ip), 0);
    let n = u01(ip.x + pcg(ip.y)) + u01(ip.x * 7919u + pcg(ip.y + 104729u)) - 1.0;
    return clamp(to_srgb(clamp(c.rgb, vec3f(0.0), vec3f(1.0))) + vec3f(n / 255.0), vec3f(0.0), vec3f(1.0));
}

// Linear HDR scene -> 8-bit target with TPDF dither, once.
@fragment
fn fs_resolve_srgb(in: V) -> @location(0) vec4f { return vec4f(from_srgb(dithered(in.pos.xy)), 1.0); }
@fragment
fn fs_resolve_unorm(in: V) -> @location(0) vec4f { return vec4f(dithered(in.pos.xy), 1.0); }

// The lit stage (sRGB-encoded by rune-light's composite) into the linear scene.
@fragment
fn fs_blit(in: V) -> @location(0) vec4f {
    let c = textureLoad(src, vec2i(in.pos.xy), 0);
    return vec4f(from_srgb(c.rgb) * alpha.x, alpha.x);
}
"#;

pub(super) struct Fullscreen {
    pub(super) pipeline: wgpu::RenderPipeline,
    bgl: wgpu::BindGroupLayout,
}

fn fullscreen(device: &wgpu::Device, module: &wgpu::ShaderModule, entry: &str, format: wgpu::TextureFormat, blend: Option<wgpu::BlendState>) -> Fullscreen {
    let bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some(entry),
        entries: &[
            wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: false },
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 1,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None },
                count: None,
            },
        ],
    });
    let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor { label: Some(entry), bind_group_layouts: &[Some(&bgl)], immediate_size: 0 });
    let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some(entry),
        layout: Some(&layout),
        vertex: wgpu::VertexState { module, entry_point: Some("vs"), compilation_options: Default::default(), buffers: &[] },
        primitive: Default::default(),
        depth_stencil: None,
        multisample: Default::default(),
        fragment: Some(wgpu::FragmentState {
            module,
            entry_point: Some(entry),
            compilation_options: Default::default(),
            targets: &[Some(wgpu::ColorTargetState { format, blend, write_mask: wgpu::ColorWrites::ALL })],
        }),
        multiview_mask: None,
        cache: None,
    });
    Fullscreen { pipeline, bgl }
}

/// Whole-frame GPU time from timestamps on the compositor's own passes, read back a couple of
/// frames later without stalling.
pub(super) struct Timer {
    qs: wgpu::QuerySet,
    resolve: wgpu::Buffer,
    read: wgpu::Buffer,
    /// 0 idle, 4 written, 1 copy recorded, 2 mapping, 3 mapped.
    state: Arc<AtomicU8>,
    period: f32,
    recording: bool,
}

impl Timer {
    fn new(device: &wgpu::Device, queue: &wgpu::Queue) -> Option<Timer> {
        if !device.features().contains(wgpu::Features::TIMESTAMP_QUERY) {
            return None;
        }
        let qs = device.create_query_set(&wgpu::QuerySetDescriptor { label: Some("lab ts"), ty: wgpu::QueryType::Timestamp, count: 2 });
        let buf = |label, usage| device.create_buffer(&wgpu::BufferDescriptor { label: Some(label), size: 16, usage, mapped_at_creation: false });
        let resolve = buf("lab ts resolve", wgpu::BufferUsages::QUERY_RESOLVE | wgpu::BufferUsages::COPY_SRC);
        let read = buf("lab ts read", wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ);
        Some(Timer { qs, resolve, read, state: Arc::new(AtomicU8::new(0)), period: queue.get_timestamp_period(), recording: false })
    }

    /// Collects a finished reading (ms) and decides whether this frame records one. Last frame's queries are
    /// resolved here, a command buffer later: Metal orders work by resource, so a resolve in the same encoder
    /// can run before the final pass's end-of-pass sample lands.
    pub(super) fn begin(&mut self, enc: &mut wgpu::CommandEncoder) -> Option<f32> {
        let mut out = None;
        match self.state.load(Ordering::Acquire) {
            4 => {
                enc.resolve_query_set(&self.qs, 0..2, &self.resolve, 0);
                enc.copy_buffer_to_buffer(&self.resolve, 0, &self.read, 0, 16);
                self.state.store(1, Ordering::Release);
            }
            1 => {
                self.state.store(2, Ordering::Release);
                let st = self.state.clone();
                self.read.slice(..).map_async(wgpu::MapMode::Read, move |r| st.store(if r.is_ok() { 3 } else { 0 }, Ordering::Release));
            }
            3 => {
                if let Ok(data) = self.read.slice(..).get_mapped_range() {
                    let t: &[u64] = bytemuck::cast_slice(&data);
                    out = (t[1] > t[0]).then(|| (t[1] - t[0]) as f32 * self.period / 1e6);
                }
                self.read.unmap();
                self.state.store(0, Ordering::Release);
            }
            _ => {}
        }
        self.recording = self.state.load(Ordering::Acquire) == 0;
        out
    }

    /// The query set when this frame records. The writes go on the compositor's own render passes: Metal may
    /// overlap a resource-free marker pass with the work around it, so empty marker passes read near zero.
    pub(super) fn query(&self) -> Option<wgpu::QuerySet> {
        self.recording.then(|| self.qs.clone())
    }

    pub(super) fn end(&mut self) {
        if self.recording {
            self.state.store(4, Ordering::Release);
        }
    }
}

pub(super) struct Res {
    pub(super) size: (u32, u32),
    pub(super) format: wgpu::TextureFormat,
    pub(super) scene: Tex,
    pub(super) backdrop: Tex,
    pub(super) node: Tex,
    pub(super) base: Tex,
    pub(super) text: Tex,
    pub(super) lit: Tex,
    pub(super) pyramid: MaskPyramid,
    pub(super) light: Option<LightRenderer>,
    pub(super) fx: FxRenderer,
    pub(super) resolve: Fullscreen,
    pub(super) blit: Fullscreen,
    pub(super) resolve_bg: wgpu::BindGroup,
    pub(super) blit_bg: wgpu::BindGroup,
    pub(super) blit_alpha: wgpu::Buffer,
    pub(super) timer: Option<Timer>,
}

impl Res {
    pub(super) fn new(device: &wgpu::Device, queue: &wgpu::Queue, size: (u32, u32), format: wgpu::TextureFormat, ui: &mut rune::render::Renderer) -> Res {
        use wgpu::TextureUsages as U;
        ui.add_format(device, HDR);
        let rt = U::RENDER_ATTACHMENT | U::TEXTURE_BINDING;
        let scene = tex(device, "lab scene", size, rt | U::COPY_SRC);
        let backdrop = tex(device, "lab backdrop", size, U::TEXTURE_BINDING | U::COPY_DST);
        let node = tex(device, "lab node layer", size, rt);
        let base = tex(device, "lab base", size, rt);
        let text = tex(device, "lab text", size, rt);
        let lit = tex(device, "lab lit", size, rt);
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("lab"), source: wgpu::ShaderSource::Wgsl(SHADERS.into()) });
        let resolve = fullscreen(device, &module, if format.is_srgb() { "fs_resolve_srgb" } else { "fs_resolve_unorm" }, format, None);
        let blit = fullscreen(device, &module, "fs_blit", HDR, Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING));
        let blit_alpha = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("lab blit alpha"),
            size: 16,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let bind = |f: &Fullscreen, v: &wgpu::TextureView| {
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: None,
                layout: &f.bgl,
                entries: &[
                    wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(v) },
                    wgpu::BindGroupEntry { binding: 1, resource: blit_alpha.as_entire_binding() },
                ],
            })
        };
        let resolve_bg = bind(&resolve, &scene.view);
        let blit_bg = bind(&blit, &lit.view);
        Res {
            size,
            format,
            pyramid: MaskPyramid::new(device, size.0, size.1),
            light: None,
            fx: {
                let mut fx = FxRenderer::new(device, HDR, FxOptions { dither: false });
                fx.set_fire_sim(device, true);
                fx
            },
            timer: Timer::new(device, queue),
            scene,
            backdrop,
            node,
            base,
            text,
            lit,
            resolve,
            blit,
            resolve_bg,
            blit_bg,
            blit_alpha,
        }
    }
}

pub(super) fn pass<'e>(enc: &'e mut wgpu::CommandEncoder, view: &'e wgpu::TextureView, clear: Option<[f32; 4]>) -> wgpu::RenderPass<'e> {
    pass_ts(enc, view, clear, None)
}

pub(super) fn pass_ts<'e>(
    enc: &'e mut wgpu::CommandEncoder,
    view: &'e wgpu::TextureView,
    clear: Option<[f32; 4]>,
    timestamp_writes: Option<wgpu::RenderPassTimestampWrites<'e>>,
) -> wgpu::RenderPass<'e> {
    let load = clear.map_or(wgpu::LoadOp::Load, |c| wgpu::LoadOp::Clear(wgpu::Color { r: c[0] as f64, g: c[1] as f64, b: c[2] as f64, a: c[3] as f64 }));
    enc.begin_render_pass(&wgpu::RenderPassDescriptor {
        label: Some("lab"),
        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
            view,
            depth_slice: None,
            resolve_target: None,
            ops: wgpu::Operations { load, store: wgpu::StoreOp::Store },
        })],
        depth_stencil_attachment: None,
        timestamp_writes,
        occlusion_query_set: None,
        multiview_mask: None,
    })
}
