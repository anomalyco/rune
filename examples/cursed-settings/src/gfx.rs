//! The compositor: Rune builds the settings window as usual; this draws it into one HDR frame
//! with the gags' effect layers slotted under or over the elements they belong to (by mark),
//! lights the whole window with rune-light when a gag needs the lights, and resolves once with
//! dither. With nothing burning it draws the plain UI and asks for no frames.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};

use rune::compose::{Compositor, Frame, is_glyph};
use rune::render::{Instance, KIND_RECT};
use rune_fx::{Fx, FxFrame, FxLayer, FxOptions, FxRenderer, Rect, Shape};
use rune_light::{Light, LightRenderer, Prim, Scene as LightScene};

use crate::Cursed;
use crate::gags::{Anchor, FLUSH_SECONDS, Gags, In, SRC_NODE, Targets};
use crate::ui::{mark_material, mark_node, mat, node};

const HDR: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;

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

// The lit window (sRGB-encoded by rune-light's composite) over the linear scene.
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

/// Whole-frame GPU time from timestamps on the compositor's own passes, read back a couple of
/// frames later without stalling.
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
        let qs = device.create_query_set(&wgpu::QuerySetDescriptor { label: Some("cursed ts"), ty: wgpu::QueryType::Timestamp, count: 2 });
        let buf = |label, usage| device.create_buffer(&wgpu::BufferDescriptor { label: Some(label), size: 16, usage, mapped_at_creation: false });
        let resolve = buf("cursed ts resolve", wgpu::BufferUsages::QUERY_RESOLVE | wgpu::BufferUsages::COPY_SRC);
        let read = buf("cursed ts read", wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ);
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
        let scene = tex(device, "cursed scene", size, rt | U::COPY_SRC);
        let backdrop = tex(device, "cursed backdrop", size, U::TEXTURE_BINDING | U::COPY_DST);
        let node = tex(device, "cursed node layer", size, rt);
        let base = tex(device, "cursed base", size, rt);
        let text = tex(device, "cursed text", size, rt);
        let lit = tex(device, "cursed lit", size, rt);
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("cursed"), source: wgpu::ShaderSource::Wgsl(SHADERS.into()) });
        let resolve = fullscreen(device, &module, if format.is_srgb() { "fs_resolve_srgb" } else { "fs_resolve_unorm" }, format, None);
        let blit = fullscreen(device, &module, "fs_blit", HDR, Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING));
        let blit_alpha = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("cursed blit alpha"),
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

fn pass<'e>(
    enc: &'e mut wgpu::CommandEncoder,
    view: &'e wgpu::TextureView,
    clear: Option<[f32; 4]>,
    timestamp_writes: Option<wgpu::RenderPassTimestampWrites<'e>>,
) -> wgpu::RenderPass<'e> {
    let load = clear.map_or(wgpu::LoadOp::Load, |c| wgpu::LoadOp::Clear(wgpu::Color { r: c[0] as f64, g: c[1] as f64, b: c[2] as f64, a: c[3] as f64 }));
    enc.begin_render_pass(&wgpu::RenderPassDescriptor {
        label: Some("cursed"),
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

// ---------------------------------------------------------------- the compositor

#[derive(Default)]
pub struct Gfx {
    res: Option<Res>,
    gags: Gags,
    animating: bool,
    view_pending: bool,
    total_gpu: (f64, u32),
    frames: u32,
    lit_frames: u32,
    max_layers: usize,
}

/// How a marked rect stands on the UI plane (device px).
fn prim(ins: &Instance, material: u8, s: f32) -> Option<Prim> {
    if ins.fill[3] < 0.02 {
        return None;
    }
    let a = ins.fill[3].max(1e-4);
    let p = Prim::from_rune(ins.rect, ins.radii).albedo([ins.fill[0] / a, ins.fill[1] / a, ins.fill[2] / a]);
    Some(match material {
        mat::CARD => p.raised(5.0 * s).bevel(5.0 * s, 1.5 * s).gloss(0.45, 0.45),
        mat::CONTROL => p.on(5.0 * s).raised(2.5 * s).bevel(4.0 * s, 1.2 * s).gloss(0.6, 0.35),
        mat::SEL => p.raised(1.5 * s).bevel(4.0 * s, 0.8 * s).gloss(0.5, 0.4),
        _ => return None,
    })
}

impl Compositor<Cursed> for Gfx {
    fn features(&self) -> wgpu::Features {
        wgpu::Features::TIMESTAMP_QUERY
    }

    fn animating(&self) -> bool {
        self.animating || self.view_pending
    }

    fn summary(&mut self, _device: &wgpu::Device) -> Option<String> {
        let (sum, n) = self.total_gpu;
        let gpu = if n > 0 { format!("gpu frame mean {:.3} ms over {n} timed frames", sum / n as f64) } else { "no GPU timestamps".to_owned() };
        Some(format!("cursed-settings: {gpu}; {} frames, {} lit, at most {} effect layers", self.frames, self.lit_frames, self.max_layers))
    }

    fn render(&mut self, f: &mut Frame<'_, Cursed>) {
        if self.res.as_ref().is_none_or(|r| r.size != f.size || r.format != f.format) {
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
        let pointer = rt.pointer().map(|p| [p.0, p.1]);

        // ---- state the compositor owns: the clock, flush timing, the demo pointer
        let mut dirty = false;
        {
            let st = &mut rt.state;
            st.now = now;
            if st.flush_requested {
                st.flush_requested = false;
                st.flush_t0 = Some(t);
                dirty = true;
            }
            if st.flush_t0.is_some_and(|t0| t > t0 + FLUSH_SECONDS + 0.4) {
                st.flush_t0 = None;
                dirty = true;
            }
            if st.show_cursor && st.cursor != pointer {
                st.cursor = pointer;
                dirty = true;
            }
        }
        if dirty {
            rt.invalidate();
            self.view_pending = true;
        }

        // ---- the gags
        let rect = |k: &str| rt.presented(k).filter(|(r, _)| r[2] > 0.0 && r[3] > 0.0).map(|(r, a)| (Rect::new(r[0], r[1], r[2], r[3]), a));
        let targets = Targets {
            fast: rect("fast-switch"),
            search: rect("search"),
            firewall: rect("firewall-switch"),
            allowed: rect("allowed-card"),
            ip: rect("ip-field"),
            fence: rect("fence-switch"),
            perimeter: rect("perimeter-card"),
            reload: rect("reload-btn"),
            cache: rect("cache-card"),
            sel: rect("nav-sel"),
        };
        let dark_theme = crate::ui::dark_theme(&rt.state, rt.env.system_dark);
        let tint = rune_theme_opencode::kit(dark_theme).c.layer_03;
        let out = self.gags.frame(&In {
            s: &rt.state,
            t,
            now,
            scale,
            vp,
            pointer,
            dark_theme,
            glass_tint: [tint.r, tint.g, tint.b],
            targets,
        });
        let mut anim = out.anim;
        let lit_on = out.lit > 0.002;
        let flushing = out.flush.is_some();

        // ---- partition Rune's instances
        let inst = &rt.instances;
        let marks = &rt.marks;
        let mut main: Vec<Instance> = Vec::with_capacity(inst.len());
        let (mut base, mut text, mut nodel, mut top) = (Vec::new(), Vec::new(), Vec::new(), Vec::new());
        let mut prims: Vec<Prim> = Vec::new();
        let mut first: HashMap<u16, usize> = HashMap::new();
        let mut last: HashMap<u16, usize> = HashMap::new();
        for (i, ins) in inst.iter().enumerate() {
            let m = marks.get(i).copied().unwrap_or(0);
            let nd = mark_node(m);
            if nd == node::CURSOR {
                top.push(*ins);
                continue;
            }
            first.entry(nd).or_insert(main.len());
            if flushing && nd == node::CACHE {
                nodel.push(*ins);
            } else {
                main.push(*ins);
                if lit_on {
                    if is_glyph(ins) {
                        text.push(*ins);
                    } else {
                        base.push(*ins);
                        if ins.params[1] == KIND_RECT
                            && let Some(p) = prim(ins, mark_material(m), scale)
                        {
                            prims.push(p);
                        }
                    }
                }
            }
            last.insert(nd, main.len());
        }

        // ---- where each effect layer goes in the main list
        enum Op {
            Blit,
            Fx(usize),
        }
        let end = main.len();
        let mut ops: Vec<(usize, Op)> = Vec::new();
        if lit_on {
            ops.push((end, Op::Blit));
        }
        let mut lays = out.lays;
        if let Some((p, samples, rest)) = out.flush {
            let layer = Fx::motion_blur(p, samples).on(Shape::rounded(rest.x, rest.y, rest.w, rest.h, 8.0)).source(SRC_NODE, vp).at(t);
            lays.push(crate::gags::Lay { layer, anchor: Anchor::Over(node::CACHE) });
        }
        let mut layers: Vec<FxLayer> = Vec::new();
        let mut needs_bd: Vec<bool> = Vec::new();
        for l in lays {
            anim |= l.layer.is_animating();
            let pos = match l.anchor {
                Anchor::Under(n) => first.get(&n).copied(),
                Anchor::Over(n) => last.get(&n).copied(),
                Anchor::Top => Some(end),
            };
            let Some(pos) = pos else { continue };
            // Lit, the window is one image: effects go on top of it.
            let pos = if lit_on { end } else { pos };
            needs_bd.push(l.layer.fx.needs_backdrop());
            ops.push((pos, Op::Fx(layers.len())));
            layers.push(l.layer);
        }
        ops.sort_by_key(|o| o.0);
        self.max_layers = self.max_layers.max(layers.len());

        // ---- upload and draw
        let clear = rt.clear_color();
        let ranges = f.ui.prepare_lists(f.device, f.queue, &mut rt.text.atlas, &[&main, &base, &text, &nodel, &top], f.size);
        let sources = [&res.node.view];
        res.fx.prepare(f.device, f.queue, &FxFrame { viewport: [w, h], scale, layers: &layers, backdrop: Some(&res.backdrop.view), sources: &sources });
        let enc = &mut *f.encoder;
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
        if !nodel.is_empty() {
            let mut p = pass(enc, &res.node.view, Some([0.0; 4]), stamp(false));
            f.ui.draw_range(&mut p, HDR, ranges[3].clone());
        }
        let mut lit_drawn = false;
        if lit_on {
            let a = clear[3].max(1e-4);
            {
                let mut p = pass(enc, &res.base.view, Some([clear[0] / a, clear[1] / a, clear[2] / a, 1.0]), stamp(false));
                f.ui.draw_range(&mut p, HDR, ranges[1].clone());
            }
            {
                let mut p = pass(enc, &res.text.view, Some([0.0; 4]), None);
                f.ui.draw_range(&mut p, HDR, ranges[2].clone());
            }
            let mut lights: Vec<Light> = out.lights;
            if lights.is_empty() {
                // Ambient only (the brightness slider): one dark light keeps the pass running.
                lights.push(Light::point(0.0, 0.0, 1.0).intensity(0.0));
            }
            let mut st = out.settings;
            st.time = t;
            let light = res.light.get_or_insert_with(|| LightRenderer::new(f.device, f.queue, w, h, HDR));
            lit_drawn = light.render(f.device, f.queue, enc, &LightScene { prims: &prims, lights: &lights, settings: st, emission: None }, &res.base.view, &res.text.view, &res.lit.view);
            f.queue.write_buffer(&res.blit_alpha, 0, bytemuck::cast_slice(&[out.lit, 0.0, 0.0, 0.0]));
            self.lit_frames += 1;
        }
        let mut cursor = 0usize;
        let mut rp = pass(enc, &res.scene.view, Some(clear), stamp(false)).forget_lifetime();
        let main_range = ranges[0].clone();
        for (pos, op) in ops {
            if pos > cursor {
                f.ui.draw_range(&mut rp, HDR, main_range.start + cursor as u32..main_range.start + pos as u32);
                cursor = pos;
            }
            match op {
                Op::Blit => {
                    if lit_drawn {
                        rp.set_pipeline(&res.blit.pipeline);
                        rp.set_bind_group(0, &res.blit_bg, &[]);
                        rp.draw(0..3, 0..1);
                    }
                }
                Op::Fx(i) => {
                    if needs_bd[i] {
                        drop(rp);
                        enc.copy_texture_to_texture(
                            wgpu::TexelCopyTextureInfo { texture: &res.scene.tex, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
                            wgpu::TexelCopyTextureInfo { texture: &res.backdrop.tex, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
                            wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
                        );
                        rp = pass(enc, &res.scene.view, None, None).forget_lifetime();
                    }
                    res.fx.draw_layer(&mut rp, i);
                }
            }
        }
        if cursor < end {
            f.ui.draw_range(&mut rp, HDR, main_range.start + cursor as u32..main_range.end);
        }
        f.ui.draw_range(&mut rp, HDR, ranges[4].clone());
        drop(rp);
        {
            let mut p = pass(enc, f.target, Some([0.0, 0.0, 0.0, 1.0]), stamp(true));
            p.set_pipeline(&res.resolve.pipeline);
            p.set_bind_group(0, &res.resolve_bg, &[]);
            p.draw(0..3, 0..1);
        }
        if let Some(tm) = res.timer.as_mut() {
            tm.end();
        }
        if let Some(g) = gpu_ms {
            self.total_gpu.0 += g as f64;
            self.total_gpu.1 += 1;
        }
        self.frames += 1;
        self.animating = anim;
    }
}
