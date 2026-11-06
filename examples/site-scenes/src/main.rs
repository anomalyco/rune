//! The material stills on Rune's home page (`www/`), rendered headless at 2x: liquid metal from
//! rune-fx, and glass and light from rune-light, over frames drawn by Rune's own renderer.
//!
//!     cargo run --release -p site-scenes -- [--out target/site-scenes] [--only NAME] [--t SECONDS]
//!
//! Each scene writes `<name>.png`, 1920 × 1080: one subject, centred and small, on the page's
//! black, in Rune blue and neutrals.

use std::path::PathBuf;

use rune::color::{Color, oklch, oklcha};
use rune::render::{Gpu, Instance, KIND_RADIAL, KIND_RECT, KIND_SHADOW, NO_CLIP, Offscreen, Renderer, write_png};
use rune::text::TextSystem;
use rune_fx::{FillAnchor, Fx, FxFrame, FxOptions, FxRenderer, LiquidMetalParams, Shape, rgb};
use rune_light::{Gi, Glass, GlassLight, GlassRenderer, GlassSettings, Light, LightRenderer, Prim, Scene, Settings};

/// The canvas in points, and device px per point.
const W: f32 = 960.0;
const H: f32 = 540.0;
const S: f32 = 2.0;
const PW: u32 = (W * S) as u32;
const PH: u32 = (H * S) as u32;
const F16: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;
/// Rune blue's OKLCH hue.
const HUE: f32 = 245.0;

const SCENES: [&str; 3] = ["metal", "glass", "light"];

// ------------------------------------------------------------------ painting

fn px(r: [f32; 4]) -> [f32; 4] {
    r.map(|v| v * S)
}

fn lin(c: Color) -> [f32; 3] {
    [c.r, c.g, c.b]
}

fn inst(rect: [f32; 4], radius: f32, fill: [f32; 4], border: [f32; 4], params: [f32; 4], uv: [f32; 4]) -> Instance {
    Instance { rect, radii: [radius; 4], fill, border, params, uv, clip: NO_CLIP, fade: [0; 4] }
}

/// Rune instances in points, and the lighting primitives that go with them.
#[derive(Default)]
struct Paint {
    out: Vec<Instance>,
    prims: Vec<Prim>,
}

impl Paint {
    fn rect(&mut self, r: [f32; 4], radius: f32, c: Color) {
        self.out.push(inst(px(r), radius * S, c.premul(1.0), [0.0; 4], [0.0, KIND_RECT, 0.0, 1.0], [0.0; 4]));
    }

    /// A rect that is also a lighting primitive, configured by `light`.
    fn solid(&mut self, r: [f32; 4], radius: f32, c: Color, light: impl FnOnce(Prim) -> Prim) {
        self.rect(r, radius, c);
        self.prims.push(light(Prim::from_rune(px(r), [radius * S; 4]).albedo(lin(c))));
    }

    /// A Gaussian shadow of a rounded rect, `sigma` in points.
    fn shadow(&mut self, r: [f32; 4], radius: f32, sigma: f32, c: Color) {
        self.out.push(inst(px(r), radius * S, c.premul(1.0), [0.0; 4], [0.0, KIND_SHADOW, sigma * S, 1.0], [0.0; 4]));
    }

    /// An elliptical pool of colour that falls smoothly to nothing at (rx, ry).
    fn pool(&mut self, cx: f32, cy: f32, rx: f32, ry: f32, c: Color) {
        const N: usize = 16;
        let f = |t: f32| (1.0 - t * t).max(0.0).powi(3);
        let rect = px([cx - rx, cy - ry, 2.0 * rx, 2.0 * ry]);
        for k in 0..N {
            let (t0, t1) = (k as f32 / N as f32, (k + 1) as f32 / N as f32);
            let open0 = if k > 0 { 2.0 } else { 0.0 };
            let open1 = if k + 1 < N { 2.0 } else { 0.0 };
            let uv = [rx * S, ry * S, rx * S, ry * S];
            self.out.push(inst(rect, 0.0, c.premul(f(t0)), c.premul(f(t1)), [t0 + open0, KIND_RADIAL, 0.0, t1 + open1], uv));
        }
    }

    /// The page's black, with the faintest lift behind the subject.
    fn backdrop(&mut self) {
        self.pool(W * 0.5, H * 0.5, W * 0.62, H * 0.9, oklcha(0.2, 0.012, HUE, 0.5));
    }
}

// ------------------------------------------------------------------ GPU

fn target(device: &wgpu::Device, w: u32, h: u32, usage: wgpu::TextureUsages) -> wgpu::TextureView {
    device
        .create_texture(&wgpu::TextureDescriptor {
            label: None,
            size: wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: F16,
            usage,
            view_formats: &[],
        })
        .create_view(&Default::default())
}

struct Ctx {
    device: wgpu::Device,
    queue: wgpu::Queue,
    rune: Renderer,
    ts: TextSystem,
    /// The frame Rune draws (linear); effects draw onto it, glass and light read it.
    base: wgpu::TextureView,
    /// Glyphs only, for rune-light. These scenes have none.
    text: wgpu::TextureView,
    /// Glass output.
    glassed: wgpu::TextureView,
    black: wgpu::TextureView,
    out: Offscreen,
    resolve: wgpu::RenderPipeline,
}

impl Ctx {
    fn new() -> Ctx {
        let Gpu { adapter, device, queue, .. } = Gpu::headless();
        eprintln!("adapter: {}", adapter.get_info().name);
        let rt = wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING;
        let base = target(&device, PW, PH, rt);
        let text = target(&device, PW, PH, rt);
        let glassed = target(&device, PW, PH, rt);
        let black = target(&device, 1, 1, wgpu::TextureUsages::TEXTURE_BINDING);
        let out = Offscreen::with_format(&device, PW, PH, wgpu::TextureFormat::Rgba8Unorm);
        let rune = Renderer::new(&device, F16);
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("resolve"),
            source: wgpu::ShaderSource::Wgsl(include_str!("resolve.wgsl").into()),
        });
        let resolve = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("resolve"),
            layout: None,
            vertex: wgpu::VertexState { module: &module, entry_point: Some("vs"), compilation_options: Default::default(), buffers: &[] },
            primitive: Default::default(),
            depth_stencil: None,
            multisample: Default::default(),
            fragment: Some(wgpu::FragmentState {
                module: &module,
                entry_point: Some("fs"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState { format: wgpu::TextureFormat::Rgba8Unorm, blend: None, write_mask: wgpu::ColorWrites::ALL })],
            }),
            multiview_mask: None,
            cache: None,
        });
        Ctx { device, queue, rune, ts: TextSystem::default(), base, text, glassed, black, out, resolve }
    }

    /// Draws `p` into `base` over `clear`, and clears the text layer.
    fn draw(&mut self, p: &Paint, clear: Color) {
        self.rune.prepare(&self.device, &self.queue, &mut self.ts.atlas, &p.out, (PW, PH));
        let mut enc = self.device.create_command_encoder(&Default::default());
        self.rune.draw(&mut enc, &self.base, [clear.r, clear.g, clear.b, 1.0]);
        self.queue.submit([enc.finish()]);
        self.rune.prepare(&self.device, &self.queue, &mut self.ts.atlas, &[], (PW, PH));
        let mut enc = self.device.create_command_encoder(&Default::default());
        self.rune.draw(&mut enc, &self.text, [0.0; 4]);
        self.queue.submit([enc.finish()]);
    }

    fn pass<'e>(enc: &'e mut wgpu::CommandEncoder, view: &'e wgpu::TextureView, load: wgpu::LoadOp<wgpu::Color>) -> wgpu::RenderPass<'e> {
        enc.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: None,
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations { load, store: wgpu::StoreOp::Store },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        })
    }

    /// Encodes a linear frame to sRGB and reads it back.
    fn resolve(&self, src: &wgpu::TextureView) -> Vec<u8> {
        let bg = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout: &self.resolve.get_bind_group_layout(0),
            entries: &[wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(src) }],
        });
        let mut enc = self.device.create_command_encoder(&Default::default());
        {
            let mut p = Self::pass(&mut enc, &self.out.view, wgpu::LoadOp::Clear(wgpu::Color::BLACK));
            p.set_pipeline(&self.resolve);
            p.set_bind_group(0, &bg, &[]);
            p.draw(0..3, 0..1);
        }
        self.out.copy(&mut enc);
        self.queue.submit([enc.finish()]);
        self.out.read(&self.device)
    }
}

// ------------------------------------------------------------------ scenes

/// Liquid metal, nearly still, lit by a cool studio.
fn metal(ctx: &mut Ctx, t: f32, shape: Shape, r: [f32; 4], radius: f32) -> Vec<u8> {
    let mut p = Paint::default();
    p.backdrop();
    // A contact shadow under the object, then a soft blue spill on the floor.
    p.pool(r[0] + r[2] * 0.5, r[1] + r[3] + 6.0, r[2] * 0.62, 26.0, oklcha(0.42, 0.09, HUE, 0.22));
    p.shadow([r[0] + 8.0, r[1] + 18.0, r[2] - 16.0, r[3]], radius, 22.0, oklcha(0.0, 0.0, 0.0, 0.85));
    ctx.draw(&p, oklch(0.0, 0.0, 0.0));
    let params = LiquidMetalParams {
        tint: rgb(0xe4e9f0),
        light: rgb(0xd8e6ff),
        flow: 0.12,
        scale: 180.0,
        speed: 0.2,
        bevel: 18.0,
        dome: 0.85,
        angle: -0.5,
        iridescence: 0.0,
        contrast: 1.2,
        ..LiquidMetalParams::chrome()
    }
    .anchor(FillAnchor::Element);
    let layers = [Fx::fill(params).on(shape).at(t)];
    let mut fx = FxRenderer::new(&ctx.device, F16, FxOptions { dither: false });
    fx.prepare(&ctx.device, &ctx.queue, &FxFrame { viewport: [PW, PH], scale: S, layers: &layers, backdrop: None, sources: &[] });
    let mut enc = ctx.device.create_command_encoder(&Default::default());
    {
        let mut pass = Ctx::pass(&mut enc, &ctx.base, wgpu::LoadOp::Load);
        fx.draw_layer(&mut pass, 0);
    }
    ctx.queue.submit([enc.finish()]);
    ctx.resolve(&ctx.base)
}

/// A frosted pane of thick glass over crisp blue discs: sharp beside the pane, soft through it.
fn glass(ctx: &mut Ctx, t: f32, pane: [f32; 4], radius: f32, frost: f32, orbs: &[(f32, f32, f32, Color)]) -> Vec<u8> {
    let mut p = Paint::default();
    p.backdrop();
    for &(x, y, r, c) in orbs {
        // Each disc drifts a little on its own slow circle.
        let (x, y) = (x + (t * 0.35 + x).sin() * 6.0, y + (t * 0.3 + y).cos() * 4.0);
        p.pool(x, y, r * 2.4, r * 2.4, Color { a: 0.3, ..c });
        p.rect([x - r, y - r, 2.0 * r, 2.0 * r], r, c);
    }
    ctx.draw(&p, oklch(0.0, 0.0, 0.0));
    let k = S;
    let slab = Glass::rounded(pane[0] * k, pane[1] * k, pane[2] * k, pane[3] * k, radius * k)
        .thickness(40.0 * k)
        .bevel(34.0 * k)
        .dome(6.0 * k)
        .gap(16.0 * k)
        .ior(1.5)
        .dispersion(0.025)
        .frost(frost)
        .tint([0.0005, 0.00035, 0.0002]);
    let lamp = GlassLight { pos: [(pane[0] + pane[2] * 0.3) * k, (pane[1] - 220.0) * k, 420.0 * k, 260.0 * k], color: [0.9, 1.0, 1.15, 0.0] };
    let mut gl = GlassRenderer::new(&ctx.device, PW, PH, F16);
    let settings = GlassSettings { env: 0.45, shadow: 0.6, shadow_offset: [0.0, 24.0 * k], shadow_blur: 36.0 * k, caustic: 0.5 };
    let mut enc = ctx.device.create_command_encoder(&Default::default());
    gl.render(&ctx.device, &ctx.queue, &mut enc, &[slab], &[lamp], settings, &ctx.base, &ctx.black, &ctx.glassed);
    ctx.queue.submit([enc.finish()]);
    ctx.resolve(&ctx.glassed)
}

/// Raised controls on a dark floor, under a single cool light.
fn light(ctx: &mut Ctx, t: f32, build: impl FnOnce(&mut Paint, f32, f32)) -> Vec<u8> {
    let mut p = Paint::default();
    let (cx, cy) = (W * 0.5, H * 0.5);
    // A dark floor that fades to the page's black before the frame's edge.
    p.pool(cx, cy, W * 0.46, H * 0.8, oklch(0.15, 0.004, HUE));
    build(&mut p, cx, cy);
    ctx.draw(&p, oklch(0.0, 0.0, 0.0));
    let k = S;
    // The key light sways slowly above and to the left; a faint blue fill from behind.
    let a = -2.25 + 0.12 * (t * 0.5).sin();
    let key = Light::point((cx + 150.0 * a.cos()) * k, (cy + 130.0 * a.sin()) * k, 200.0 * k)
        .color(lin(oklch(0.97, 0.012, HUE)))
        .intensity(2.6)
        .radius(40.0 * k)
        .range(1000.0 * k)
        .specular(2.6);
    let fill = Light::point((cx + 420.0) * k, (cy - 300.0) * k, 420.0 * k)
        .color(lin(oklch(0.72, 0.12, HUE)))
        .intensity(0.35)
        .radius(160.0 * k)
        .range(2400.0 * k)
        .specular(0.8);
    let settings = Settings {
        ambient: [0.06, 0.06, 0.07],
        rim: 1.1,
        contact_ao: 0.55,
        background_gloss: 0.35,
        background_roughness: 0.6,
        gi: Some(Gi::default()),
        time: t,
        ..Default::default()
    };
    let mut lights = LightRenderer::new(&ctx.device, &ctx.queue, PW, PH, wgpu::TextureFormat::Rgba8Unorm);
    let mut enc = ctx.device.create_command_encoder(&Default::default());
    let scene = Scene { prims: &p.prims, lights: &[key, fill], settings, emission: None };
    lights.render(&ctx.device, &ctx.queue, &mut enc, &scene, &ctx.base, &ctx.text, &ctx.out.view);
    ctx.out.copy(&mut enc);
    ctx.queue.submit([enc.finish()]);
    ctx.out.read(&ctx.device)
}

fn render(ctx: &mut Ctx, name: &str, t: f32) -> Vec<u8> {
    let (cx, cy) = (W * 0.5, H * 0.5);
    match name {
        "metal" => {
            let r = 92.0;
            metal(ctx, t, Shape::circle(cx, cy, r), [cx - r, cy - r, 2.0 * r, 2.0 * r], r)
        }
        "glass" => glass(ctx, t, [cx - 70.0, cy - 110.0, 220.0, 220.0], 50.0, 0.6, &[(cx - 48.0, cy - 50.0, 100.0, oklch(0.62, 0.16, HUE))]),
        "light" => light(ctx, t, |p, cx, cy| {
            // A keypad: eight dark keys and one lit in blue.
            let (s, gap) = (60.0, 16.0);
            for i in 0..3 {
                for j in 0..3 {
                    let (x, y) = (cx - 1.5 * s - gap + j as f32 * (s + gap), cy - 1.5 * s - gap + i as f32 * (s + gap));
                    let lit = i == 1 && j == 1;
                    let c = if lit { oklch(0.55, 0.15, HUE) } else { oklch(0.4, 0.005, HUE) };
                    p.solid([x, y, s, s], 16.0, c, |q| {
                        let q = q.raised(12.0 * S).bevel(12.0 * S, 4.0 * S).gloss(0.8, 0.3);
                        if lit { q.emissive([0.006, 0.024, 0.08]) } else { q }
                    });
                }
            }
        }),
        _ => panic!("unknown scene {name}; scenes: {}", SCENES.join(", ")),
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let get = |k: &str| args.iter().position(|a| a == k).and_then(|i| args.get(i + 1)).cloned();
    let out = get("--out").map_or_else(|| PathBuf::from("target/site-scenes"), PathBuf::from);
    let t: f32 = get("--t").map_or(1.2, |s| s.parse().expect("--t SECONDS"));
    let only = get("--only");
    std::fs::create_dir_all(&out).unwrap();
    let mut ctx = Ctx::new();
    for name in SCENES.iter().copied().filter(|n| only.as_deref().is_none_or(|o| o == *n)) {
        let px = render(&mut ctx, name, t);
        write_png(&out.join(format!("{name}.png")), PW, PH, &px).unwrap();
        eprintln!("wrote {name}.png");
    }
}
