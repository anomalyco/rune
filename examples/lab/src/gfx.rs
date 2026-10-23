//! The compositor: Rune's instances (tagged by `El::mark`) become rune-light occluders and anchor
//! rune-fx layers. One HDR frame, one resolve into the window or the offscreen target.
//!
//! Per frame: partition the instances (lit canvases → base/text/prims, a moving card → its node
//! layer, masks → the mask pyramid, everything else → the main list), light the lit canvases,
//! then draw the main list into the HDR scene with each effect layer inserted under or over the
//! node it belongs to (copying the backdrop before distortion), and resolve with dither.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};
use std::time::Instant;

use rune::compose::{Compositor, Frame, is_glyph};
use rune::render::{Instance, KIND_GRADIENT, KIND_LAYER, KIND_LAYER_END, KIND_RECT};
use rune_fx::{
    AuroraParams, BlurMode, DistortParams, ElectricParams, ElectricStyle, FireParams, Fx, FxFrame, FxLayer, FxOptions,
    FxRenderer, GlowParams, GlowStyle, GrainParams, Heat, LensParams, MaskPyramid, MotionBlurParams, MotionSamples, Pose,
    PulseParams, PulseStyle, Rect, Shape, ShimmerParams, SparkleParams, rgb,
};
use rune_light::{Gi, Light, LightRenderer, Prim, Scene as LightScene, Settings, TextPolicy};
use rune_motion::{Animated, Spring, Vec2};

use crate::knobs::{ALL, FX_COLORS, LAMP_COLORS, Scene, lin};
use crate::stage::{self, mark_material, mark_node, mark_scene, mat, node};
use crate::{Grab, Lab, ShotKind};

const HDR: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;
const SRC_MASK: u32 = 0;
const SRC_NODE: u32 = 1;

// ---------------------------------------------------------------- GPU resources

struct Tex {
    tex: wgpu::Texture,
    view: wgpu::TextureView,
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

struct Fullscreen {
    pipeline: wgpu::RenderPipeline,
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

/// Whole-frame GPU time from two compute timestamp markers (render-pass timestamps aren't ordered
/// on tile GPUs), read back a frame or two later without stalling.
struct Timer {
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
    fn begin(&mut self, enc: &mut wgpu::CommandEncoder) -> Option<f32> {
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
    fn query(&self) -> Option<wgpu::QuerySet> {
        self.recording.then(|| self.qs.clone())
    }

    fn end(&mut self) {
        if self.recording {
            self.state.store(4, Ordering::Release);
        }
    }
}

struct Res {
    size: (u32, u32),
    format: wgpu::TextureFormat,
    scene: Tex,
    backdrop: Tex,
    node: Tex,
    base: Tex,
    text: Tex,
    lit: Tex,
    pyramid: MaskPyramid,
    light: Option<LightRenderer>,
    fx: FxRenderer,
    resolve: Fullscreen,
    blit: Fullscreen,
    resolve_bg: wgpu::BindGroup,
    blit_bg: wgpu::BindGroup,
    blit_alpha: wgpu::Buffer,
    timer: Option<Timer>,
}

impl Res {
    fn new(device: &wgpu::Device, queue: &wgpu::Queue, size: (u32, u32), format: wgpu::TextureFormat, ui: &mut rune::render::Renderer) -> Res {
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
            fx: FxRenderer::new(device, HDR, FxOptions { dither: false }),
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

fn pass<'e>(enc: &'e mut wgpu::CommandEncoder, view: &'e wgpu::TextureView, clear: Option<[f32; 4]>) -> wgpu::RenderPass<'e> {
    pass_ts(enc, view, clear, None)
}

fn pass_ts<'e>(
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

// ---------------------------------------------------------------- live state the compositor owns

/// A light that follows the pointer 1:1 while it's over the stage and cools when it leaves.
struct Follow {
    pos: [f32; 2],
    k: Animated<f32>,
    inside: Option<bool>,
}

impl Follow {
    fn new(rest: [f32; 2]) -> Follow {
        Follow { pos: rest, k: Animated::new(1.0), inside: None }
    }

    fn update(&mut self, p: Option<[f32; 2]>, now: f64) -> ([f32; 2], f32) {
        match p {
            Some(p) => {
                self.pos = p;
                if self.inside != Some(true) {
                    self.k.animate_to(1.0, Spring::new(0.3, 0.0), now);
                    self.inside = Some(true);
                }
            }
            None if self.inside == Some(true) => {
                self.k.animate_to(0.0, Spring::new(0.45, 0.0), now);
                self.inside = Some(false);
            }
            None => {}
        }
        (self.pos, self.k.sample(now).clamp(0.0, 1.0))
    }

    fn moving(&self, now: f64) -> bool {
        !self.k.is_settled(now)
    }
}

/// A light switched by state: it warms up fast and cools slowly.
struct Switch {
    on: Option<bool>,
    heat: Heat,
}

impl Switch {
    fn new(attack: f32, cool: f32) -> Switch {
        Switch { on: None, heat: Heat::new(attack, cool) }
    }

    fn at(&mut self, on: bool, t: f32) -> f32 {
        if self.on != Some(on) {
            // The first sight is the resting state, not a warm-up.
            self.heat.set(if self.on.is_none() { -10.0 } else { t }, on);
            self.on = Some(on);
        }
        self.heat.at(t)
    }

    fn moving(&self, t: f32) -> bool {
        self.heat.is_active(t) && self.on == Some(false) || self.on == Some(true) && self.heat.at(t) < 0.999
    }
}

struct Live {
    torch: Follow,
    keylight: Follow,
    sun: Follow,
    neon: Switch,
    charged: Switch,
    rec: Switch,
    upload: Switch,
    droplet: Option<Animated<Vec2>>,
}

impl Default for Live {
    fn default() -> Self {
        Live {
            torch: Follow::new([470.0, 250.0]),
            keylight: Follow::new([150.0, 230.0]),
            sun: Follow::new(stage::GODRAY_REST),
            neon: Switch::new(0.12, 0.32),
            charged: Switch::new(0.15, 0.5),
            rec: Switch::new(0.12, 0.6),
            upload: Switch::new(0.12, 0.6),
            droplet: None,
        }
    }
}

// ---------------------------------------------------------------- the compositor

#[derive(Clone, Copy)]
enum Anchor {
    Under(u8),
    Over(u8),
    /// Over everything in the canvas.
    Top,
}

struct Lay {
    layer: FxLayer,
    scene: Scene,
    anchor: Anchor,
}

/// One canvas on screen this frame.
struct Cv<'a> {
    s: &'a Lab,
    sc: Scene,
    t: f32,
    now: f64,
    scale: f32,
    origin: [f32; 2],
    /// Canvas opacity (presence): lights and effects scale with it.
    a: f32,
    pointer: Option<[f32; 2]>,
    reduced: bool,
    viewport: Rect,
}

impl Cv<'_> {
    fn k(&self, id: &str) -> f32 {
        self.s.k(self.sc, id)
    }
    fn on(&self, id: &str) -> bool {
        self.s.on(self.sc, id)
    }
    /// Canvas points → window points.
    fn w(&self, p: [f32; 2]) -> [f32; 2] {
        [self.origin[0] + p[0], self.origin[1] + p[1]]
    }
    /// Canvas points → device pixels.
    fn px(&self, p: [f32; 2]) -> [f32; 2] {
        let q = self.w(p);
        [q[0] * self.scale, q[1] * self.scale]
    }
    fn rounded(&self, r: Rect, radius: f32) -> Shape {
        Shape::rounded(self.origin[0] + r.x, self.origin[1] + r.y, r.w, r.h, radius)
    }
    /// The whole viewport; the stage scissor clips it, so full-stage effects don't stop at the canvas edge.
    fn everywhere(&self) -> Shape {
        let v = self.viewport;
        Shape::rounded(v.x, v.y, v.w, v.h, 0.0)
    }
    fn capsule(&self, r: Rect) -> Shape {
        Shape::capsule(self.origin[0] + r.x, self.origin[1] + r.y, r.w, r.h)
    }
    fn circle(&self, c: [f32; 3]) -> Shape {
        let q = self.w([c[0], c[1]]);
        Shape::circle(q[0], q[1], c[2])
    }
    fn swatch(&self) -> u32 {
        FX_COLORS[(self.k("color") as usize).min(FX_COLORS.len() - 1)]
    }
}

/// Per-scene material inputs for prims.
#[derive(Default)]
struct PrimCx {
    keys: Vec<f32>,
    bevel: f32,
    gloss: f32,
    rec: f32,
    status: f32,
    progress: f32,
}

#[derive(Default)]
pub struct LabGfx {
    res: Option<Res>,
    live: Live,
    animating: bool,
    view_pending: bool,
    cpu: (f64, u32),
    gpu: (f64, u32),
    last_push: Option<f64>,
    shown: Option<(i32, Option<i32>, bool)>,
    last_pointer: Option<(f32, f32)>,
    total_gpu: (f64, u32),
}

fn srgbf(r: f32, g: f32, b: f32) -> [f32; 3] {
    let f = rune::color::srgb_decode;
    [f(r), f(g), f(b)]
}

impl Compositor<Lab> for LabGfx {
    fn features(&self) -> wgpu::Features {
        wgpu::Features::TIMESTAMP_QUERY
    }

    fn animating(&self) -> bool {
        self.animating || self.view_pending
    }

    fn summary(&mut self, _device: &wgpu::Device) -> Option<String> {
        let (sum, n) = self.total_gpu;
        Some(if n > 0 {
            format!("lab: gpu frame mean {:.3} ms over {n} timed frames (timestamp markers around the whole compositor)", sum / n as f64)
        } else {
            "lab: no GPU timestamps (adapter lacks TIMESTAMP_QUERY or too few frames)".to_owned()
        })
    }

    fn render(&mut self, f: &mut Frame<'_, Lab>) {
        let started = Instant::now();
        let rebuilt = self.res.as_ref().is_none_or(|r| r.size != f.size || r.format != f.format);
        if rebuilt {
            self.res = Some(Res::new(f.device, f.queue, f.size, f.format, f.ui));
        }
        self.view_pending = false;
        let _ = f.device.poll(wgpu::PollType::Poll);
        let res = self.res.as_mut().unwrap();
        let gpu_ms = res.timer.as_mut().and_then(|t| t.begin(f.encoder));
        let rt = &mut *f.rt;
        let scale = rt.env.scale;
        let now = rt.t;
        let t = now as f32;
        let (w, h) = f.size;
        let vp = Rect::new(0.0, 0.0, w as f32 / scale, h as f32 / scale);
        let stage_r = rt.presented("stage").map_or([vp.x, vp.y, vp.w, vp.h], |(r, _)| r);
        let in_stage = |p: (f32, f32)| p.0 >= stage_r[0] && p.0 <= stage_r[0] + stage_r[2] && p.1 >= stage_r[1] && p.1 <= stage_r[1] + stage_r[3];
        let pointer = rt.pointer().filter(|p| in_stage(*p));
        let canvases: Vec<(Scene, [f32; 2], f32)> = ALL
            .iter()
            .filter_map(|sc| rt.presented(&format!("canvas-{}", sc.id())).map(|(r, a)| (*sc, [r[0], r[1]], a)))
            .filter(|c| c.2 > 0.001)
            .collect();
        let s = &rt.state;
        let mut anim = false;

        // ---- lights and effect layers per visible canvas
        let mut lights: Vec<Light> = Vec::new();
        let mut settings: Option<(f32, Settings)> = None;
        let mut lit_alpha = 0.0f32;
        let mut pc = PrimCx::default();
        let mut lays: Vec<Lay> = Vec::new();
        for &(sc, origin, a) in &canvases {
            let cv = Cv {
                s,
                sc,
                t,
                now,
                scale,
                origin,
                a,
                pointer: pointer.map(|p| [p.0 - origin[0], p.1 - origin[1]]),
                reduced: s.reduced,
                viewport: vp,
            };
            if sc.lit() {
                lit_alpha = lit_alpha.max(a);
                let st = self.live.lights(&cv, &mut lights, &mut pc, &mut anim);
                if settings.as_ref().is_none_or(|(sa, _)| a > *sa) {
                    settings = Some((a, st));
                }
            } else {
                self.live.layers(&cv, &mut lays, &mut anim);
            }
        }
        for l in &lays {
            anim |= l.layer.is_animating();
        }

        // ---- the motion card: rest rect (where Rune drew it) and its pose along the track
        let card_drawn = rt.presented("card").map(|(r, _)| r);
        let mut card_blur: Option<(MotionBlurParams, MotionSamples, Rect)> = None;
        let mut card_off = [0.0f32; 2];
        if let (Some(r), Some(&(_, origin, _))) = (card_drawn, canvases.iter().find(|c| c.0 == Scene::Motion)) {
            let rest = Rect::new(r[0], r[1], r[2], r[3]);
            let dragging = matches!(s.grab, Some(Grab::Card(_)));
            let pose_at = |tt: f32| -> Pose {
                let c = if dragging {
                    let c = s.card.target();
                    let v = s.card_vt.velocity();
                    let dt = (tt as f64 - now) as f32;
                    Vec2 { x: c.x + v.x * dt, y: c.y + v.y * dt }
                } else {
                    s.card.sample(tt as f64)
                };
                Pose { x: origin[0] + c.x - rest.x, y: origin[1] + c.y - rest.y, ..Default::default() }
            };
            let p = MotionBlurParams {
                mode: if s.on(Scene::Motion, "directional") { BlurMode::Directional } else { BlurMode::Shutter },
                shutter_angle: if s.reduced || !rt.motion_blur_enabled() { 0.0 } else { s.k(Scene::Motion, "shutter_angle") },
                smooth_shutter: s.on(Scene::Motion, "smooth"),
                ..Default::default()
            };
            let samples = MotionSamples::from_track(&p, rest, t, scale, pose_at);
            let pm = pose_at(t);
            card_off = [pm.x * scale, pm.y * scale];
            anim |= dragging || !s.card.is_settled(now);
            if samples.is_moving() {
                card_blur = Some((p, samples, rest));
            }
        }
        if let Some((p, samples, rest)) = card_blur {
            let layer = Fx::motion_blur(p, samples).on(Shape::rounded(rest.x, rest.y, rest.w, rest.h, 14.0)).source(SRC_NODE, vp).at(t);
            lays.push(Lay { layer, scene: Scene::Motion, anchor: Anchor::Over(node::CARD) });
        }

        // ---- partition Rune's instances
        let inst = &rt.instances;
        let marks = &rt.marks;
        let mut main: Vec<Instance> = Vec::with_capacity(inst.len());
        let (mut base, mut text, mut nodel, mut mask) = (Vec::new(), Vec::new(), Vec::new(), Vec::new());
        let mut prims: Vec<Prim> = Vec::new();
        let mut first_lit: Option<usize> = None;
        let mut first: HashMap<(usize, u8), usize> = HashMap::new();
        let mut last: HashMap<(usize, u8), usize> = HashMap::new();
        let mut top: HashMap<usize, usize> = HashMap::new();
        let mut stage_fill = [0.0, 0.0, 0.0, 1.0];
        let needs_mask = lays.iter().any(|l| l.layer.source.is_some_and(|s| s.slot == SRC_MASK));
        // This demo owns a different effect graph: it splits glyphs from their backgrounds
        // and inserts effects between individual nodes. Preserve its existing flattened
        // opacity semantics until that graph can composite an entire scope after lighting.
        // Ordinary Rune compositors instead preserve scopes and call encode_layers.
        let mut opacity = 1.0;
        let mut opacity_stack = Vec::new();
        for (i, ins) in inst.iter().enumerate() {
            if ins.params[1] == KIND_LAYER {
                opacity_stack.push(opacity);
                opacity *= ins.fill[3];
                continue;
            }
            if ins.params[1] == KIND_LAYER_END {
                opacity = opacity_stack.pop().expect("balanced Rune layer scopes");
                continue;
            }
            let mut flattened = *ins;
            flattened.fill = flattened.fill.map(|v| v * opacity);
            flattened.border = flattened.border.map(|v| v * opacity);
            if flattened.params[1] == KIND_GRADIENT { flattened.uv = flattened.uv.map(|v| v * opacity); }
            let ins = &flattened;
            let m = marks.get(i).copied().unwrap_or(0);
            if m == stage::STAGE_MARK && ins.params[1] == KIND_RECT {
                stage_fill = ins.fill;
            }
            let sc = mark_scene(m);
            let nd = mark_node(m);
            if let Some(sc) = sc {
                first.entry((sc.idx(), nd)).or_insert(main.len());
            }
            match sc {
                Some(sc) if sc.lit() => {
                    first_lit.get_or_insert(main.len());
                    if is_glyph(ins) {
                        text.push(*ins);
                    } else {
                        base.push(*ins);
                        if ins.params[1] == KIND_RECT
                            && let Some(p) = prim(ins, mark_material(m), nd, &pc, scale)
                        {
                            prims.push(p);
                        }
                    }
                }
                Some(Scene::Motion) if nd == node::CARD => {
                    if card_blur.is_some() {
                        nodel.push(*ins);
                    } else {
                        let mut c = *ins;
                        c.rect[0] += card_off[0];
                        c.rect[1] += card_off[1];
                        main.push(c);
                    }
                }
                _ => main.push(*ins),
            }
            if needs_mask && matches!(nd, node::GLOW_TEXT | node::SKELETON | node::THINKING) && sc.is_some_and(|s| !s.lit()) {
                mask.push(*ins);
            }
            if let Some(sc) = sc {
                last.insert((sc.idx(), nd), main.len());
                top.insert(sc.idx(), main.len());
            }
        }
        let light_on = !base.is_empty() && settings.is_some();
        if light_on && lights.is_empty() {
            // GI off with no lights: an ambient-only pass keeps the canvas lit by its settings.
            lights.push(Light::point(0.0, 0.0, 1.0).intensity(0.0));
        }

        // ---- ops: where each effect layer goes in the main list
        enum Op {
            Ui,
            Blit,
            Fx(usize),
        }
        let mut ops: Vec<(usize, Op)> = Vec::new();
        if light_on && let Some(p) = first_lit {
            ops.push((p, Op::Blit));
        }
        let mut layers: Vec<FxLayer> = Vec::new();
        let mut needs_bd: Vec<bool> = Vec::new();
        for l in lays {
            let key = |n: u8| (l.scene.idx(), n);
            let pos = match l.anchor {
                Anchor::Under(n) => first.get(&key(n)).copied(),
                Anchor::Over(n) => last.get(&key(n)).copied(),
                Anchor::Top => top.get(&l.scene.idx()).copied(),
            };
            let Some(pos) = pos else { continue };
            needs_bd.push(l.layer.fx.needs_backdrop());
            ops.push((pos, Op::Fx(layers.len())));
            layers.push(l.layer.reduced(s.reduced));
        }
        ops.sort_by_key(|o| o.0);
        ops.push((main.len(), Op::Ui));

        // ---- upload and draw
        let clear = rt.clear_color();
        let ranges = f.ui.prepare_lists(f.device, f.queue, &mut rt.text.atlas, &[&main, &base, &text, &nodel, &mask], f.size);
        let sources = [&res.pyramid.view, &res.node.view];
        let stats = res.fx.prepare(
            f.device,
            f.queue,
            &FxFrame { viewport: [w, h], scale, layers: &layers, backdrop: Some(&res.backdrop.view), sources: &sources },
        );
        let enc = &mut *f.encoder;
        // GPU time spans the first pass the compositor records through the end of the resolve pass.
        let tq = res.timer.as_ref().and_then(Timer::query);
        let mut began = false;
        let mut stamp = |end: bool| {
            let first = !std::mem::replace(&mut began, true);
            tq.as_ref().filter(|_| first || end).map(|q| wgpu::RenderPassTimestampWrites {
                query_set: q,
                beginning_of_pass_write_index: first.then_some(0),
                end_of_pass_write_index: end.then_some(1),
            })
        };
        if needs_mask && !mask.is_empty() {
            {
                let mut p = pass_ts(enc, &res.pyramid.levels[0], Some([0.0; 4]), stamp(false));
                f.ui.draw_range(&mut p, HDR, ranges[4].clone());
            }
            res.fx.build_mips(f.device, enc, &res.pyramid);
        }
        if !nodel.is_empty() {
            let mut p = pass_ts(enc, &res.node.view, Some([0.0; 4]), stamp(false));
            f.ui.draw_range(&mut p, HDR, ranges[3].clone());
        }
        let mut lit_drawn = false;
        if light_on {
            let (_, mut st) = settings.unwrap();
            st.time = t;
            let a = stage_fill[3].max(1e-4);
            {
                let mut p = pass_ts(enc, &res.base.view, Some([stage_fill[0] / a, stage_fill[1] / a, stage_fill[2] / a, 1.0]), stamp(false));
                f.ui.draw_range(&mut p, HDR, ranges[1].clone());
            }
            {
                let mut p = pass(enc, &res.text.view, Some([0.0; 4]));
                f.ui.draw_range(&mut p, HDR, ranges[2].clone());
            }
            let light = res.light.get_or_insert_with(|| LightRenderer::new(f.device, f.queue, w, h, HDR));
            lit_drawn = light.render(
                f.device,
                f.queue,
                enc,
                &LightScene { prims: &prims, lights: &lights, settings: st },
                &res.base.view,
                &res.text.view,
                &res.lit.view,
            );
            f.queue.write_buffer(&res.blit_alpha, 0, bytemuck::cast_slice(&[lit_alpha, 0.0, 0.0, 0.0]));
        }
        let px = |v: f32| (v * scale).round().max(0.0) as u32;
        let (sx, sy) = (px(stage_r[0]).min(w), px(stage_r[1]).min(h));
        let (sw, sh) = (px(stage_r[0] + stage_r[2]).min(w) - sx, px(stage_r[1] + stage_r[3]).min(h) - sy);
        let mut cursor = 0usize;
        let mut rp = pass_ts(enc, &res.scene.view, Some(clear), stamp(false)).forget_lifetime();
        for (pos, op) in ops {
            if pos > cursor {
                rp.set_scissor_rect(0, 0, w, h);
                f.ui.draw_range(&mut rp, HDR, ranges[0].start + cursor as u32..ranges[0].start + pos as u32);
                cursor = pos;
            }
            match op {
                Op::Ui => {}
                Op::Blit if lit_drawn && sw > 0 && sh > 0 => {
                    rp.set_scissor_rect(sx, sy, sw, sh);
                    rp.set_pipeline(&res.blit.pipeline);
                    rp.set_bind_group(0, &res.blit_bg, &[]);
                    rp.draw(0..3, 0..1);
                }
                Op::Blit => {}
                Op::Fx(i) => {
                    if sw == 0 || sh == 0 {
                        continue;
                    }
                    if needs_bd[i] {
                        drop(rp);
                        let o = wgpu::Origin3d { x: sx, y: sy, z: 0 };
                        enc.copy_texture_to_texture(
                            wgpu::TexelCopyTextureInfo { texture: &res.scene.tex, mip_level: 0, origin: o, aspect: wgpu::TextureAspect::All },
                            wgpu::TexelCopyTextureInfo { texture: &res.backdrop.tex, mip_level: 0, origin: o, aspect: wgpu::TextureAspect::All },
                            wgpu::Extent3d { width: sw, height: sh, depth_or_array_layers: 1 },
                        );
                        rp = pass(enc, &res.scene.view, None).forget_lifetime();
                    }
                    rp.set_scissor_rect(sx, sy, sw, sh);
                    res.fx.draw_layer(&mut rp, i);
                }
            }
        }
        drop(rp);
        {
            let mut p = pass_ts(enc, f.target, Some([0.0, 0.0, 0.0, 1.0]), stamp(true));
            p.set_pipeline(&res.resolve.pipeline);
            p.set_bind_group(0, &res.resolve_bg, &[]);
            p.draw(0..3, 0..1);
        }
        if let Some(tm) = res.timer.as_mut() {
            tm.end();
        }
        self.animating = anim;

        // ---- readout: averaged over a quarter second of Rune's clock. It asks for a redraw only on frames that
        // were drawing anyway (animating, the display link flipping, the pointer moving), so a scene at rest never
        // draws frames for its own readout.
        let moved = std::mem::replace(&mut self.last_pointer, rt.pointer()) != self.last_pointer;
        // A frame that (re)built resources pays pipeline compiles; it would stick in an at-rest readout.
        if !rebuilt {
            self.cpu.0 += f.update_ms + started.elapsed().as_secs_f64() * 1000.0;
            self.cpu.1 += 1;
        }
        if let Some(g) = gpu_ms {
            self.gpu.0 += g as f64;
            self.gpu.1 += 1;
            self.total_gpu.0 += g as f64;
            self.total_gpu.1 += 1;
        }
        let due = self.last_push.is_none_or(|p| now - p >= 0.25);
        let live_flip = self.shown.is_some_and(|(_, _, l)| l != anim);
        if due && (anim || moved) || live_flip {
            let c = if self.cpu.1 > 0 { (self.cpu.0 / self.cpu.1 as f64) as f32 } else { rt.state.perf.cpu_ms };
            let g = (self.gpu.1 > 0).then(|| (self.gpu.0 / self.gpu.1 as f64) as f32).or(rt.state.perf.gpu_ms);
            let key = ((c * 100.0).round() as i32, g.map(|g| (g * 100.0).round() as i32), anim);
            if self.shown != Some(key) {
                rt.state.perf = crate::Perf { cpu_ms: c, gpu_ms: g, draws: stats.drawn, live: anim };
                rt.invalidate();
                self.view_pending = true;
                self.shown = Some(key);
            }
            self.cpu = (0.0, 0);
            self.gpu = (0.0, 0);
            self.last_push = Some(now);
        }
        rt.state.now = now;
    }
}

// ---------------------------------------------------------------- materials

fn prim(ins: &Instance, material: u8, nd: u8, pc: &PrimCx, s: f32) -> Option<Prim> {
    let a = ins.fill[3].max(1e-4);
    let alb = [ins.fill[0] / a, ins.fill[1] / a, ins.fill[2] / a];
    let p = Prim::from_rune(ins.rect, ins.radii).albedo(alb);
    let (bv, g) = (pc.bevel, pc.gloss);
    Some(match material {
        mat::CARD => p.raised(6.0 * s).bevel(6.0 * s, 2.0 * s).gloss(0.55, 0.42),
        mat::CHIP => p.on(6.0 * s).raised(2.5 * s).bevel(4.0 * s, 1.0 * s).gloss(0.4, 0.4),
        mat::BUTTON => p.on(10.0 * s).raised(4.0 * s).bevel(10.0 * s, 3.0 * s).gloss(0.8, 0.3),
        mat::COMPOSER => p.raised(10.0 * s).bevel(7.0 * s, 2.5 * s).gloss(0.6, 0.38),
        mat::ON_CARD => p.on(6.0 * s).gloss(0.4, 0.4),
        mat::SWITCH => p.on(6.0 * s).raised(5.0 * s).bevel(9.0 * s, 4.0 * s).gloss(0.7, 0.3),
        mat::PILL => p.on(6.0 * s).raised(3.0 * s).bevel(4.0 * s, 1.2 * s).gloss(0.5, 0.4),
        mat::BAR => p.on(6.0 * s).raised(2.0 * s),
        mat::DIAL => p.on(6.0 * s).bevel(16.0 * s, 4.0 * s).gloss(0.6, 0.35),
        mat::EMIT => {
            let k = match nd {
                node::REC => pc.rec,
                node::STATUS => pc.status,
                node::PROGRESS => pc.progress,
                _ => 0.0,
            };
            p.on(6.0 * s).emissive([alb[0] * k, alb[1] * k, alb[2] * k])
        }
        mat::FLAT => p,
        mat::KNOB => p.floating(10.0 * s, 22.0 * s).bevel(30.0 * bv * s, 10.0 * bv * s).gloss(0.9, 0.26),
        mat::KEY => {
            let d = nd.checked_sub(node::KEY0).and_then(|i| pc.keys.get(i as usize)).copied().unwrap_or(0.0).clamp(0.0, 1.2);
            p.floating((10.0 - 6.0 * d) * s, (18.0 - 11.0 * d) * s).bevel(12.0 * bv * s, 4.5 * bv * s).gloss(g, 0.28)
        }
        mat::BODY => p.raised(10.0 * s).bevel(22.0 * bv * s, 7.0 * bv * s).gloss(0.7, 0.32),
        mat::SCREEN => p.on(10.0 * s).gloss(1.0, 0.18),
        mat::TILE => p.raised(14.0 * s).bevel(8.0 * s, 2.5 * s).gloss(0.5, 0.4),
        _ => return None,
    })
}

// ---------------------------------------------------------------- scenes

impl Live {
    /// Lights for a lit canvas (appended, scaled by its presence) and its settings.
    fn lights(&mut self, cv: &Cv, out: &mut Vec<Light>, pc: &mut PrimCx, anim: &mut bool) -> Settings {
        let s = cv.scale;
        let (t, now) = (cv.t, cv.now);
        match cv.sc {
            Scene::Lamp => {
                let p = cv.px(cv.s.lamp);
                let col = lin(LAMP_COLORS[(cv.k("color") as usize).min(LAMP_COLORS.len() - 1)]);
                out.push(
                    Light::point(p[0], p[1], cv.k("height") * s)
                        .color(col)
                        .intensity(cv.k("intensity"))
                        .radius(cv.k("radius") * s)
                        .range(1100.0 * s)
                        .bulb(0.75)
                        .specular(1.2)
                        .scaled(cv.a),
                );
                if cv.on("fill") {
                    let q = cv.px([-300.0, -200.0]);
                    out.push(Light::point(q[0], q[1], 900.0 * s).color(srgbf(0.55, 0.65, 1.0)).intensity(0.35).radius(220.0 * s).range(6000.0 * s).specular(0.4).scaled(cv.a));
                }
                Settings {
                    ambient: [0.62, 0.63, 0.7],
                    albedo_floor: 0.07,
                    gi: cv.on("bounce").then_some(Gi { strength: 1.0, air: 0.0, bounce: 0.6, absorption: 0.24 }),
                    rim: 0.9,
                    contact_ao: 0.45,
                    ..Default::default()
                }
            }
            Scene::Flashlight => {
                let (p, k) = self.torch.update(cv.pointer, now);
                *anim |= self.torch.moving(now);
                let h = cv.k("height");
                let lp = [p[0] + 80.0, p[1] + 130.0];
                let (a, b) = (cv.px(p), cv.px(lp));
                let axis = [a[0] - b[0], a[1] - b[1], -h * s];
                out.push(
                    Light::point(b[0], b[1], h * s)
                        .color(srgbf(1.0, 0.95, 0.88))
                        .intensity(cv.k("intensity"))
                        .radius(cv.k("radius") * s)
                        .range(1800.0 * s)
                        .spot(axis, cv.k("beam"))
                        .specular(1.4)
                        .scaled(cv.a * k),
                );
                let am = cv.k("ambient");
                Settings {
                    ambient: [am, am * 1.07, am * 1.28],
                    albedo_floor: 0.04,
                    text: TextPolicy { floor: 0.32, ceiling: 1.3, lit: 1.0, specular: 0.1 },
                    rim: 1.0,
                    contact_ao: 0.3,
                    background_gloss: 0.5,
                    ..Default::default()
                }
            }
            Scene::Gi => {
                let b = if cv.reduced { 1.0 } else { 0.775 + 0.225 * (std::f32::consts::TAU * t / 2.2).sin() };
                *anim |= !cv.reduced;
                let e = cv.k("emission");
                let rec = self.rec.at(cv.s.recording, t);
                let up = self.upload.at(cv.s.uploading, t);
                *anim |= self.rec.moving(t) || self.upload.moving(t);
                pc.rec = 7.0 * b * rec * e;
                pc.status = 2.2 * (0.6 + 0.4 * b) * e;
                pc.progress = 1.9 * (0.85 + 0.15 * b) * up * e;
                let am = cv.k("ambient");
                Settings {
                    ambient: [am, am * 1.03, am * 1.17],
                    gi: cv.on("gi").then_some(Gi { strength: cv.k("strength"), air: cv.k("air"), bounce: cv.k("bounce"), absorption: 0.24 }),
                    contact_ao: 0.4,
                    rim: 0.6,
                    ..Default::default()
                }
            }
            Scene::Keys => {
                let (p, k) = self.keylight.update(cv.pointer, now);
                *anim |= self.keylight.moving(now);
                pc.keys = cv.s.keys.iter().map(|a| a.sample(now)).collect();
                *anim |= cv.s.keys.iter().any(|a| !a.is_settled(now));
                pc.bevel = cv.k("bevel");
                pc.gloss = cv.k("gloss");
                let q = cv.px(p);
                out.push(
                    Light::point(q[0], q[1], cv.k("height") * s)
                        .color(srgbf(1.0, 0.93, 0.85))
                        .intensity(3.4)
                        .radius(cv.k("radius") * s)
                        .range(2200.0 * s)
                        .specular(2.4)
                        .scaled(cv.a * k),
                );
                if cv.on("fill") {
                    let q = cv.px([1100.0, 760.0]);
                    out.push(Light::point(q[0], q[1], 700.0 * s).color(srgbf(0.5, 0.62, 1.0)).intensity(0.4).radius(200.0 * s).range(5000.0 * s).specular(0.6).scaled(cv.a));
                }
                Settings { ambient: [0.3, 0.31, 0.34], rim: 1.1, contact_ao: 0.5, ..Default::default() }
            }
            Scene::GodRays => {
                let (p, k) = self.sun.update(cv.pointer, now);
                *anim |= self.sun.moving(now);
                let q = cv.px(p);
                out.push(
                    Light::point(q[0], q[1], cv.k("height") * s)
                        .color(srgbf(1.0, 0.8, 0.55))
                        .intensity(cv.k("intensity"))
                        .radius(12.0 * s)
                        .range(3000.0 * s)
                        .shafts(cv.k("shafts"))
                        .softness(cv.k("softness"))
                        .scaled(cv.a * k),
                );
                Settings { ambient: [0.5, 0.5, 0.58], shafts_over: 0.12, rim: 0.8, contact_ao: 0.35, ..Default::default() }
            }
            _ => Settings::default(),
        }
    }

    /// Effect layers for an effects canvas, anchored to the nodes they decorate.
    fn layers(&mut self, cv: &Cv, out: &mut Vec<Lay>, anim: &mut bool) {
        let t = cv.t;
        let sc = cv.sc;
        let mut push = |layer: FxLayer, anchor: Anchor| out.push(Lay { layer: layer.at(t), scene: sc, anchor });
        match sc {
            Scene::Glow => {
                let (r, i, br, rim, hot) = (cv.k("radius") / 14.0, cv.k("intensity") * cv.a, cv.k("breathe"), cv.k("rim_width"), cv.k("hot"));
                let color = cv.swatch();
                push(
                    Fx::glow(GlowParams { radius: 16.0 * r, intensity: 0.85 * i, breathe: br.max(0.4), breathe_period: 2.6, hot: 0.4 * hot / 0.6, ..Default::default() })
                        .on(cv.capsule(stage::GLOW_BTN)),
                    Anchor::Under(node::GLOW_BUTTON),
                );
                let heat = self.neon.at(cv.s.neon_on, t);
                *anim |= self.neon.moving(t);
                push(
                    Fx::glow(GlowParams {
                        style: GlowStyle::Neon,
                        color: rgb(color),
                        core: rgb(0xeafcff),
                        radius: 10.0 * r,
                        intensity: i,
                        inner: 0.35,
                        heat,
                        hot: (0.9 * hot / 0.6).min(1.0),
                        rim_width: rim * 0.55 / 0.6,
                        breathe: br,
                        ..Default::default()
                    })
                    .on(cv.rounded(stage::NEON, 16.0))
                    .over(),
                    Anchor::Over(node::NEON),
                );
                push(
                    Fx::glow(GlowParams {
                        style: GlowStyle::Aura,
                        color: rgb(0xff5fa2),
                        core: rgb(0x8b5cf6),
                        third: rgb(0x38bdf8),
                        radius: 12.0 * r,
                        intensity: 0.9 * i,
                        inner: 0.55,
                        hot: 0.55,
                        aura_speed: 0.15,
                        rim_width: rim,
                        occlusion: 0.1,
                        ..Default::default()
                    })
                    .on(cv.capsule(stage::AURA))
                    .over(),
                    Anchor::Over(node::AURA),
                );
                push(
                    Fx::glow(GlowParams {
                        style: GlowStyle::Mask,
                        color: rgb(0xff8a2a),
                        core: rgb(0xfff1d6),
                        radius: 10.0 * r,
                        intensity: 1.1 * i,
                        hot: 0.0,
                        breathe: br.max(0.25),
                        breathe_period: 3.2,
                        ..Default::default()
                    })
                    .on(cv.rounded(stage::GLOW_TEXT, 0.0))
                    .source(SRC_MASK, cv.viewport),
                    Anchor::Under(node::GLOW_TEXT),
                );
                push(
                    Fx::glow(GlowParams { color: rgb(0xffa630), core: rgb(0xfff6e0), radius: 9.0 * r, intensity: 1.3 * i, hot: (hot / 0.6).min(1.0), breathe: br, ..Default::default() })
                        .on(cv.circle(stage::HOT_DOT)),
                    Anchor::Under(node::HOT_DOT),
                );
            }
            Scene::Pulses => {
                let (reach, width, period, count, i) = (cv.k("reach"), cv.k("width"), cv.k("period"), cv.k("count").round() as u32, cv.k("intensity") * cv.a);
                let color = rgb(cv.swatch());
                push(
                    Fx::pulse(PulseParams { color, core: rgb(0xffffff), reach: reach * 46.0 / 36.0, width: width * 1.3 / 1.6, period: period * 2.2 / 2.4, count, intensity: i, ..Default::default() })
                        .on(cv.circle(stage::RADAR)),
                    Anchor::Under(node::RADAR),
                );
                push(
                    Fx::pulse(PulseParams { style: PulseStyle::Breathe, color: rgb(0x34d399), core: rgb(0xd1fae5), reach: 12.0, period: 2.8, intensity: 0.8 * i, ..Default::default() })
                        .on(cv.circle(stage::AVATAR)),
                    Anchor::Under(node::AVATAR),
                );
                push(
                    Fx::pulse(PulseParams { style: PulseStyle::Heartbeat, color: rgb(0xff3b6b), core: rgb(0xffe4ec), reach: 26.0, width: 1.1, period: 1.1, intensity: i, ..Default::default() })
                        .on(cv.capsule(stage::LIVE)),
                    Anchor::Under(node::LIVE),
                );
                for sh in &cv.s.shots {
                    match sh.kind {
                        ShotKind::Ripple => push(
                            Fx::pulse(PulseParams {
                                style: PulseStyle::Ripple { at: cv.w(sh.at) },
                                color,
                                core: rgb(0xffffff),
                                reach: reach * 6.0,
                                width: 2.2 * width / 1.6,
                                t0: sh.t0,
                                duration: 0.9,
                                intensity: 0.7 * i,
                                ..Default::default()
                            })
                            .on(cv.everywhere())
                            .over(),
                            Anchor::Top,
                        ),
                        ShotKind::RippleCard => push(
                            Fx::pulse(PulseParams { style: PulseStyle::Ripple { at: cv.w(sh.at) }, color, core: rgb(0xffffff), reach: 0.0, width: 2.2, t0: sh.t0, duration: 0.9, intensity: 0.7 * i, ..Default::default() })
                                .on(cv.rounded(stage::INBOX, 16.0))
                                .over(),
                            Anchor::Over(node::INBOX),
                        ),
                        ShotKind::Ping => {
                            let from = [stage::SEND[0] + stage::SEND[2], stage::SEND[1]];
                            let to = [stage::INBOX.x + 6.0, stage::INBOX.y + stage::INBOX.h * 0.5];
                            let d = [to[0] - from[0], to[1] - from[1]];
                            let len = d[0].hypot(d[1]);
                            push(
                                Fx::pulse(PulseParams {
                                    style: PulseStyle::Ping { from: cv.w(from), dir: [d[0] / len, d[1] / len], travel: len },
                                    color,
                                    core: rgb(0xf0f7ff),
                                    t0: sh.t0,
                                    duration: stage::PING_DURATION,
                                    lag: 0.16,
                                    head_radius: 2.6,
                                    intensity: i,
                                    ..Default::default()
                                })
                                .on(cv.circle(stage::SEND)),
                                Anchor::Top,
                            );
                            push(
                                Fx::pulse(PulseParams {
                                    style: PulseStyle::Ripple { at: cv.w(to) },
                                    color,
                                    core: rgb(0xffffff),
                                    reach: 0.0,
                                    width: 2.2,
                                    t0: sh.t0 + stage::PING_DURATION,
                                    duration: 0.9,
                                    intensity: 0.7 * i,
                                    ..Default::default()
                                })
                                .on(cv.rounded(stage::INBOX, 16.0))
                                .over(),
                                Anchor::Over(node::INBOX),
                            );
                        }
                        ShotKind::Shockwave => {}
                    }
                }
            }
            Scene::Electric => {
                let (amp, fq, sp, gl, fk) = (cv.k("amplitude"), cv.k("frequency"), cv.k("speed"), cv.k("glow"), cv.k("forks"));
                let color = rgb(cv.swatch());
                let (a, b) = (cv.w([stage::ARC_A[0] + 6.0, stage::ARC_A[1]]), cv.w([stage::ARC_B[0] - 6.0, stage::ARC_B[1]]));
                push(
                    Fx::electric(ElectricParams {
                        style: ElectricStyle::Arc { a, b },
                        color,
                        amplitude: amp * 9.0 / 5.0,
                        frequency: fq,
                        speed: sp,
                        glow: gl * 6.0 / 5.0,
                        forks: (fk * 0.6 / 0.55).min(1.0),
                        strands: 3,
                        intensity: cv.a,
                        ..Default::default()
                    })
                    .on(Shape::rounded(a[0], a[1] - 1.0, b[0] - a[0], 2.0, 0.0)),
                    Anchor::Under(node::TERMINALS),
                );
                push(
                    Fx::electric(ElectricParams { color, amplitude: amp * 0.8, frequency: fq, speed: sp, glow: gl, forks: fk, gap: 3.0, fork_len: 22.0, intensity: cv.a, ..Default::default() })
                        .on(cv.rounded(stage::OVERLOAD, 18.0))
                        .seed(7),
                    Anchor::Over(node::OVERLOAD),
                );
                let hovered = cv.pointer.is_some_and(|p| {
                    let r = stage::CHARGED;
                    p[0] >= r.x && p[0] <= r.x + r.w && p[1] >= r.y && p[1] <= r.y + r.h
                });
                let heat = self.charged.at(hovered, t);
                *anim |= self.charged.moving(t);
                if heat > 0.002 {
                    let base = ElectricParams::charged();
                    push(
                        Fx::electric(ElectricParams { color, intensity: base.intensity * heat * cv.a, glow: base.glow * (0.6 + 0.4 * heat), ..base })
                            .on(cv.capsule(stage::CHARGED))
                            .seed(3),
                        Anchor::Over(node::CHARGED),
                    );
                }
            }
            Scene::Fire => {
                let i = cv.k("intensity") * cv.a;
                push(
                    Fx::fire(FireParams {
                        height: cv.k("height"),
                        scale: cv.k("scale"),
                        speed: cv.k("speed"),
                        embers: cv.k("embers").round() as u32,
                        intensity: i,
                        swirl: cv.k("swirl"),
                        ..Default::default()
                    })
                    .on(cv.rounded(stage::FLAMES, 18.0))
                    .seed(5),
                    Anchor::Under(node::FLAMES),
                );
                let pl = FireParams::plasma();
                push(Fx::fire(FireParams { intensity: pl.intensity * i, speed: pl.speed * cv.k("speed") / 1.5, ..pl }).on(cv.capsule(stage::PLASMA)).seed(11), Anchor::Under(node::PLASMA));
                let ht = FireParams::heat();
                push(Fx::fire(FireParams { intensity: ht.intensity * i, ..ht }).on(cv.capsule(stage::DEPLOY)).seed(2), Anchor::Under(node::DEPLOY));
                push(
                    Fx::distort(DistortParams::HeatHaze { height: 26.0, scale: 9.0, speed: 2.0, amount: 1.1 * cv.a, whole_shape: false }).on(cv.capsule(stage::DEPLOY)),
                    Anchor::Over(node::DEPLOY),
                );
            }
            Scene::Lens => {
                let now = cv.now;
                let target = Vec2 { x: cv.s.lens[0] + 128.0, y: cv.s.lens[1] + 8.0 };
                let d = self.droplet.get_or_insert_with(|| Animated::new(target));
                d.animate_to(target, Spring::new(0.5, 0.0), now);
                *anim |= !d.is_settled(now);
                let dp = d.sample(now);
                for sh in cv.s.shots.iter().filter(|sh| sh.kind == ShotKind::Shockwave) {
                    push(
                        Fx::distort(DistortParams::shockwave(cv.w(sh.at), sh.t0)).on(cv.everywhere()),
                        Anchor::Top,
                    );
                }
                if cv.on("crt") {
                    push(Fx::distort(DistortParams::crt()).on(cv.rounded(stage::CRT, 14.0)), Anchor::Over(node::CRT));
                }
                push(
                    Fx::distort(DistortParams::ChromaSplit { offset: [1.6, 0.0], glitch: 0.3, feather: 2.0 }).on(cv.rounded(stage::CHROMA, 6.0)),
                    Anchor::Over(node::CHROMA),
                );
                let q = cv.w([dp.x, dp.y]);
                push(
                    Fx::lens(LensParams {
                        depth: cv.k("depth"),
                        bevel: cv.k("bevel"),
                        dispersion: cv.k("dispersion"),
                        magnification: cv.k("magnification"),
                        frost: cv.k("frost"),
                        presence: cv.a,
                        second: Some(Shape::circle(q[0], q[1], 26.0)),
                        union_k: 26.0,
                        ..Default::default()
                    })
                    .on(cv.capsule(stage::lens_rect(cv.s.lens))),
                    Anchor::Top,
                );
            }
            Scene::Extras => {
                if cv.on("shimmer") {
                    push(
                        Fx::shimmer(ShimmerParams { intensity: 0.11 * cv.a, band: 42.0, use_mask: true, ..Default::default() })
                            .on(cv.rounded(stage::SKELETON, 16.0))
                            .source(SRC_MASK, cv.viewport),
                        Anchor::Over(node::SKELETON),
                    );
                    push(
                        Fx::shimmer(ShimmerParams { intensity: 0.85 * cv.a, band: 18.0, period: 1.6, angle: 0.25, use_mask: true, ..Default::default() })
                            .on(cv.rounded(stage::THINKING, 0.0))
                            .source(SRC_MASK, cv.viewport),
                        Anchor::Over(node::THINKING),
                    );
                }
                if cv.on("aurora") {
                    push(
                        Fx::aurora(AuroraParams { intensity: 0.75 * cv.a, scale: 110.0, feather: 18.0, speed: cv.k("speed"), ..Default::default() })
                            .on(cv.rounded(stage::AURORA, 18.0))
                            .over(),
                        Anchor::Over(node::AURORA),
                    );
                }
                if cv.on("grain") {
                    push(Fx::grain(GrainParams { amount: 0.016 * cv.a, ..Default::default() }).on(cv.rounded(stage::AURORA, 18.0)), Anchor::Over(node::AURORA));
                }
                if cv.on("sparkles") {
                    push(
                        Fx::sparkles(SparkleParams { margin: 20.0, size: 8.0, count: 9, intensity: cv.a, ..Default::default() }).on(cv.capsule(stage::PRO)).seed(4),
                        Anchor::Over(node::PRO),
                    );
                }
            }
            _ => {}
        }
    }
}
