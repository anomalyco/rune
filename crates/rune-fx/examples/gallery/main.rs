//! Headless gallery for rune-fx: stills, frame strips, PNG sequences and GPU timings.
//!
//!   gallery                          all effects -> demo/fx/<effect>.png, -strip.png, -reduced.png, timings.md
//!   gallery --effect fire            one effect
//!   gallery --frames DIR --fps 60 --seconds 2 --effect fire   PNG sequence
//!   gallery --timings                GPU timings only

mod scenes;
mod ui;

use scenes::{fling, scene};

use rune::render::{Gpu, Offscreen, write_png};
use rune_fx::*;
use std::ops::Range;
use std::path::{Path, PathBuf};
use ui::{col, TextAtlas, UiInst, UiRenderer};

const W: f32 = 640.0;
const H: f32 = 360.0;
const S: f32 = 2.0;
const FMT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;
const EFFECTS: [&str; 15] = ["glow", "pulse", "electric", "fire", "motion", "distort", "extras", "liquid-metal", "plastic", "mesh-gradient", "dither", "grain-gradient", "fluted-glass", "fills", "fill-presets"];

// ------------------------------------------------------------------ scene builder

#[derive(Clone, Copy, PartialEq)]
enum Target {
    Main,
    Mask,
    Src,
}

enum Op {
    Ui(Range<usize>),
    Fx(usize),
    Backdrop,
}

struct B<'a> {
    atlas: &'a mut TextAtlas,
    ui: Vec<UiInst>,
    mask: Vec<UiInst>,
    src: Vec<UiInst>,
    src_rect: Option<Rect>,
    layers: Vec<FxLayer>,
    slots: Vec<FxSlot>,
    ops: Vec<Op>,
    target: Target,
    t: f32,
    reduced: bool,
}

const SRC_PAD: f32 = 4.0;

impl<'a> B<'a> {
    fn new(atlas: &'a mut TextAtlas, t: f32, reduced: bool) -> B<'a> {
        B { atlas, ui: vec![], mask: vec![], src: vec![], src_rect: None, layers: vec![], slots: vec![], ops: vec![], target: Target::Main, t, reduced }
    }
    fn push(&mut self, mut u: UiInst) {
        match self.target {
            Target::Main => {
                u.vp = [W * S, H * S, 0.0, 0.0];
                let i = self.ui.len();
                self.ui.push(u);
                if let Some(Op::Ui(r)) = self.ops.last_mut() {
                    if r.end == i {
                        r.end = i + 1;
                        return;
                    }
                }
                self.ops.push(Op::Ui(i..i + 1));
            }
            Target::Mask => {
                u.vp = [W * S, H * S, 0.0, 0.0];
                self.mask.push(u);
            }
            Target::Src => {
                let r = self.src_rect.unwrap();
                u.rect[0] -= (r.x - SRC_PAD) * S;
                u.rect[1] -= (r.y - SRC_PAD) * S;
                u.vp = [(r.w + 2.0 * SRC_PAD) * S, (r.h + 2.0 * SRC_PAD) * S, 0.0, 0.0];
                self.src.push(u);
            }
        }
    }
    fn rect(&mut self, r: Rect, radius: f32, fill: [f32; 4], fill2: [f32; 4], border: [f32; 4], bw: f32) {
        self.push(UiInst {
            rect: [r.x * S, r.y * S, r.w * S, r.h * S],
            p: [radius * S, bw * S, 0.0, 0.0],
            fill,
            fill2,
            border,
            uv: [0.0; 4],
            vp: [0.0; 4],
        });
    }
    fn shadow(&mut self, r: Rect, radius: f32, sigma: f32, c: [f32; 4]) {
        self.push(UiInst {
            rect: [r.x * S, r.y * S, r.w * S, r.h * S],
            p: [radius * S, 0.0, 2.0, sigma * S],
            fill: c,
            fill2: c,
            border: [0.0; 4],
            uv: [0.0; 4],
            vp: [0.0; 4],
        });
    }
    /// Text with its baseline at y. Returns width in pt.
    fn text(&mut self, x: f32, y: f32, s: &str, size: f32, weight: u8, c: [f32; 4]) -> f32 {
        let r = self.atlas.get(s, size * S, weight);
        self.push(UiInst {
            rect: [x * S - 2.0, y * S - r.ascent, r.w, r.h],
            p: [0.0, 0.0, 1.0, 0.0],
            fill: c,
            fill2: c,
            border: [0.0; 4],
            uv: r.uv,
            vp: [0.0; 4],
        });
        r.w / S
    }
    fn text_w(&mut self, s: &str, size: f32, weight: u8) -> f32 {
        self.atlas.get(s, size * S, weight).w / S - 4.0 / S
    }
    fn text_c(&mut self, cx: f32, cy: f32, s: &str, size: f32, weight: u8, c: [f32; 4]) {
        let w = self.text_w(s, size, weight);
        self.text(cx - w * 0.5, cy + size * 0.36, s, size, weight, c);
    }
    fn fx(&mut self, l: FxLayer) {
        self.fx_with(l, FxSlot::default());
    }
    fn fx_with(&mut self, l: FxLayer, slot: FxSlot) {
        let l = l.at(if l.time == 0.0 { self.t } else { l.time }).reduced(self.reduced);
        self.ops.push(Op::Fx(self.layers.len()));
        self.layers.push(l);
        self.slots.push(slot);
    }
    fn backdrop(&mut self) {
        self.ops.push(Op::Backdrop);
    }

    // --- design kit
    fn background(&mut self, title: &str, sub: &str) {
        self.rect(Rect::new(0.0, 0.0, W, H), 0.0, col(0x0c0d11, 1.0), col(0x0a0b0e, 1.0), [0.0; 4], 0.0);
        let w = self.text(24.0, 34.0, title, 15.0, 2, col(0xeceef2, 1.0));
        self.text(24.0 + w + 8.0, 34.0, sub, 12.0, 0, col(0x6b7280, 1.0));
    }
    fn card(&mut self, r: Rect, radius: f32) {
        self.shadow(Rect::new(r.x, r.y + 8.0, r.w, r.h), radius, 16.0, [0.0, 0.0, 0.0, 0.45]);
        self.rect(r, radius, col(0x17191f, 1.0), col(0x121419, 1.0), col(0xffffff, 0.075), 1.0 / S);
    }
    fn lines(&mut self, x: f32, y: f32, w: f32, n: usize) {
        for i in 0..n {
            let ww = w * [1.0, 0.82, 0.6, 0.9][i % 4];
            self.rect(Rect::new(x, y + i as f32 * 14.0, ww, 6.0), 3.0, col(0x262a33, 1.0), col(0x262a33, 1.0), [0.0; 4], 0.0);
        }
    }
    fn button(&mut self, r: Rect, label: &str, top: u32, bottom: u32, text: u32) {
        self.shadow(Rect::new(r.x, r.y + 4.0, r.w, r.h), r.h * 0.5, 8.0, [0.0, 0.0, 0.0, 0.4]);
        self.rect(r, r.h * 0.5, col(top, 1.0), col(bottom, 1.0), col(0xffffff, 0.16), 1.0 / S);
        let c = r.center();
        self.text_c(c.x, c.y, label, 14.0, 2, col(text, 1.0));
    }
    fn caption(&mut self, x: f32, y: f32, s: &str) {
        self.text(x, y, s, 11.0, 1, col(0x6b7280, 1.0));
    }
}


// ------------------------------------------------------------------ GPU

struct Ctx {
    device: wgpu::Device,
    queue: wgpu::Queue,
    ui: UiRenderer,
    fx: FxRenderer,
    atlas: TextAtlas,
    scene: wgpu::Texture,
    scene_view: wgpu::TextureView,
    backdrop: wgpu::Texture,
    backdrop_view: wgpu::TextureView,
    out: Offscreen,
    pyramid: MaskPyramid,
    src: Option<(wgpu::Texture, wgpu::TextureView, [u32; 2])>,
    resolve: wgpu::RenderPipeline,
    resolve_bg: wgpu::BindGroup,
    timestamps: bool,
    w: u32,
    h: u32,
}

fn tex(device: &wgpu::Device, w: u32, h: u32, format: wgpu::TextureFormat, usage: wgpu::TextureUsages) -> (wgpu::Texture, wgpu::TextureView) {
    let t = device.create_texture(&wgpu::TextureDescriptor {
        label: None,
        size: wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage,
        view_formats: &[],
    });
    let v = t.create_view(&Default::default());
    (t, v)
}

fn pass<'e>(enc: &'e mut wgpu::CommandEncoder, view: &'e wgpu::TextureView, clear: Option<wgpu::Color>, ts: Option<wgpu::RenderPassTimestampWrites<'e>>) -> wgpu::RenderPass<'e> {
    enc.begin_render_pass(&wgpu::RenderPassDescriptor {
        label: None,
        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
            view,
            depth_slice: None,
            resolve_target: None,
            ops: wgpu::Operations { load: clear.map_or(wgpu::LoadOp::Load, wgpu::LoadOp::Clear), store: wgpu::StoreOp::Store },
        })],
        depth_stencil_attachment: None,
        timestamp_writes: ts,
        occlusion_query_set: None,
        multiview_mask: None,
    })
}

impl Ctx {
    fn new(w: u32, h: u32) -> Ctx {
        let Gpu { adapter, device, queue, .. } = Gpu::headless();
        let timestamps = device.features().contains(wgpu::Features::TIMESTAMP_QUERY);
        eprintln!("adapter: {:?} timestamps={timestamps}", adapter.get_info().name);
        let ui = UiRenderer::new(&device, FMT);
        let fx = FxRenderer::new(&device, FMT, FxOptions { dither: false });
        let u = wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_SRC;
        let (scene, scene_view) = tex(&device, w, h, FMT, u);
        let (backdrop, backdrop_view) = tex(&device, w, h, FMT, wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST);
        let out = Offscreen::with_format(&device, w, h, wgpu::TextureFormat::Rgba8Unorm);
        let pyramid = MaskPyramid::new(&device, w, h);
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("resolve"),
            source: wgpu::ShaderSource::Wgsl(include_str!("resolve.wgsl").into()),
        });
        let resolve = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("resolve"),
            layout: None,
            vertex: wgpu::VertexState { module: &module, entry_point: Some("vs_full"), compilation_options: Default::default(), buffers: &[] },
            primitive: Default::default(),
            depth_stencil: None,
            multisample: Default::default(),
            fragment: Some(wgpu::FragmentState {
                module: &module,
                entry_point: Some("fs_resolve"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState { format: wgpu::TextureFormat::Rgba8Unorm, blend: None, write_mask: wgpu::ColorWrites::ALL })],
            }),
            multiview_mask: None,
            cache: None,
        });
        let resolve_bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout: &resolve.get_bind_group_layout(0),
            entries: &[wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&scene_view) }],
        });
        Ctx {
            device,
            queue,
            ui,
            fx,
            atlas: TextAtlas::new(),
            scene,
            scene_view,
            backdrop,
            backdrop_view,
            out,
            pyramid,
            src: None,
            resolve,
            resolve_bg,
            timestamps,
            w,
            h,
        }
    }

    fn render(&mut self, name: &str, t: f32, reduced: bool) -> Vec<u8> {
        let mut b = B::new(&mut self.atlas, t, reduced);
        scene(name, &mut b);
        let B { ui, mask, src, src_rect, layers, slots, ops, .. } = b;
        let mut all = ui;
        let mask_r = all.len()..all.len() + mask.len();
        all.extend_from_slice(&mask);
        let src_r = all.len()..all.len() + src.len();
        all.extend_from_slice(&src);
        self.ui.prepare(&self.device, &self.queue, &mut self.atlas, &all);
        if let Some(r) = src_rect {
            let sz = [((r.w + 2.0 * SRC_PAD) * S) as u32, ((r.h + 2.0 * SRC_PAD) * S) as u32];
            if self.src.as_ref().is_none_or(|s| s.2 != sz) {
                let (t, v) = tex(&self.device, sz[0], sz[1], FMT, wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING);
                self.src = Some((t, v, sz));
            }
        }
        let src_view = self.src.as_ref().map(|s| &s.1);
        let mut sources: Vec<&wgpu::TextureView> = vec![&self.pyramid.view];
        if let Some(v) = src_view {
            sources.push(v);
        }
        let frame = FxFrame { viewport: [self.w, self.h], scale: S, layers: &layers, backdrop: Some(&self.backdrop_view), sources: &sources };
        self.fx.prepare_with_slots(&self.device, &self.queue, &frame, &slots);

        let mut enc = self.device.create_command_encoder(&Default::default());
        if !mask.is_empty() {
            {
                let mut p = pass(&mut enc, &self.pyramid.levels[0], Some(wgpu::Color::TRANSPARENT), None);
                self.ui.draw(&mut p, mask_r.clone());
            }
            self.fx.build_mips(&self.device, &mut enc, &self.pyramid);
        }
        if let Some(s) = &self.src {
            if !src.is_empty() {
                let mut p = pass(&mut enc, &s.1, Some(wgpu::Color::TRANSPARENT), None);
                self.ui.draw(&mut p, src_r.clone());
            }
        }
        {
            let _ = pass(&mut enc, &self.scene_view, Some(wgpu::Color::BLACK), None);
        }
        for op in &ops {
            match op {
                Op::Ui(r) => {
                    let mut p = pass(&mut enc, &self.scene_view, None, None);
                    self.ui.draw(&mut p, r.clone());
                }
                Op::Fx(i) => {
                    self.fx.blur_backdrop(&self.device, &self.queue, &mut enc, &self.backdrop_view, *i);
                    let mut p = pass(&mut enc, &self.scene_view, None, None);
                    self.fx.draw_layer(&mut p, *i);
                }
                Op::Backdrop => {
                    enc.copy_texture_to_texture(self.scene.as_image_copy(), self.backdrop.as_image_copy(), wgpu::Extent3d { width: self.w, height: self.h, depth_or_array_layers: 1 });
                }
            }
        }
        {
            let mut p = pass(&mut enc, &self.out.view, Some(wgpu::Color::BLACK), None);
            p.set_pipeline(&self.resolve);
            p.set_bind_group(0, &self.resolve_bg, &[]);
            p.draw(0..3, 0..1);
        }
        self.out.copy(&mut enc);
        self.queue.submit([enc.finish()]);
        self.out.read(&self.device)
    }
}

fn half(px: &[u8], w: u32, h: u32) -> Vec<u8> {
    let (hw, hh) = (w / 2, h / 2);
    let mut o = vec![0u8; (hw * hh * 4) as usize];
    for y in 0..hh {
        for x in 0..hw {
            for c in 0..4 {
                let s = |dx: u32, dy: u32| px[(((y * 2 + dy) * w + x * 2 + dx) * 4 + c) as usize] as u32;
                o[((y * hw + x) * 4 + c) as usize] = ((s(0, 0) + s(1, 0) + s(0, 1) + s(1, 1) + 2) / 4) as u8;
            }
        }
    }
    o
}

fn strip_times(name: &str) -> (f32, f32) {
    // (start, span) of the 8-frame strip.
    match name {
        "motion" => (0.0, 0.6),
        "glow" => (0.0, 1.4),
        "pulse" => (0.0, 1.4),
        "distort" => (0.0, 0.9),
        // Fills move slowly: eight frames over four seconds.
        "liquid-metal" | "plastic" | "mesh-gradient" | "dither" | "grain-gradient" | "fluted-glass" | "fills" | "fill-presets" => (0.0, 4.0),
        _ => (1.0, 1.0),
    }
}

fn still_time(name: &str) -> f32 {
    match name {
        "glow" => 0.3,
        "pulse" => 0.8,
        "electric" => 1.37,
        "fire" => 2.0,
        "motion" => 0.1,
        "distort" => 0.3,
        "liquid-metal" | "plastic" | "mesh-gradient" | "dither" | "grain-gradient" | "fluted-glass" | "fills" | "fill-presets" => 1.2,
        _ => 0.55,
    }
}

fn render_effect(ctx: &mut Ctx, dir: &Path, name: &str) {
    let (w, h) = (ctx.w, ctx.h);
    let px = ctx.render(name, still_time(name), false);
    write_png(&dir.join(format!("{name}.png")), w, h, &px).unwrap();
    let px = ctx.render(name, still_time(name), true);
    write_png(&dir.join(format!("{name}-reduced.png")), w / 2, h / 2, &half(&px, w, h)).unwrap();
    let (t0, span) = strip_times(name);
    let (fw, fh) = (w / 2, h / 2);
    let mut strip = vec![0u8; (fw * 4 * fh * 2 * 4) as usize];
    for i in 0..8u32 {
        let t = t0 + span * i as f32 / 8.0;
        let f = half(&ctx.render(name, t, false), w, h);
        let (ox, oy) = ((i % 4) * fw, (i / 4) * fh);
        for y in 0..fh {
            let d = (((oy + y) * fw * 4 + ox) * 4) as usize;
            let s = (y * fw * 4) as usize;
            strip[d..d + (fw * 4) as usize].copy_from_slice(&f[s..s + (fw * 4) as usize]);
        }
    }
    write_png(&dir.join(format!("{name}-strip.png")), fw * 4, fh * 2, &strip).unwrap();
    eprintln!("wrote {name}");
}

// ------------------------------------------------------------------ timings

fn timing_layers() -> Vec<(&'static str, FxLayer)> {
    let sh = Shape::rounded(60.0, 60.0, 480.0, 280.0, 24.0);
    let t = 1.3;
    let mut v: Vec<(&str, FxLayer)> = vec![
        ("glow soft", Fx::glow(GlowParams { radius: 24.0, ..Default::default() }).on(sh)),
        ("glow neon", Fx::glow(GlowParams { style: GlowStyle::Neon, radius: 24.0, inner: 0.4, ..Default::default() }).on(sh)),
        ("glow aura", Fx::glow(GlowParams { style: GlowStyle::Aura, radius: 24.0, inner: 0.5, ..Default::default() }).on(sh)),
        ("glow mask (text)", Fx::glow(GlowParams { style: GlowStyle::Mask, radius: 24.0, ..Default::default() }).on(sh).source(0, Rect::new(0.0, 0.0, 600.0, 400.0))),
        ("pulse radar", Fx::pulse(PulseParams { reach: 80.0, ..Default::default() }).on(sh)),
        ("pulse breathe", Fx::pulse(PulseParams { style: PulseStyle::Breathe, reach: 30.0, ..Default::default() }).on(sh)),
        ("pulse heartbeat", Fx::pulse(PulseParams { style: PulseStyle::Heartbeat, reach: 60.0, ..Default::default() }).on(sh)),
        ("pulse ripple", Fx::pulse(PulseParams { style: PulseStyle::Ripple { at: [200.0, 200.0] }, reach: 0.0, t0: 1.0, duration: 1.0, ..Default::default() }).on(sh)),
        ("electric arc (3 strands)", Fx::electric(ElectricParams { style: ElectricStyle::Arc { a: [20.0, 200.0], b: [580.0, 200.0] }, strands: 3, glow: 40.0, ..Default::default() }).on(sh)),
        ("electric border", Fx::electric(ElectricParams { glow: 30.0, ..Default::default() }).on(sh)),
        ("electric charged", Fx::electric(ElectricParams { glow: 30.0, ..ElectricParams::charged() }).on(sh)),
        ("fire top + 16 embers", Fx::fire(FireParams { height: 200.0, ..Default::default() }).on(Shape::rounded(60.0, 300.0, 480.0, 90.0, 16.0))),
        ("fire plasma border", Fx::fire(FireParams { height: 200.0, ..FireParams::plasma() }).on(sh)),
        ("fire heat (UI)", Fx::fire(FireParams { height: 200.0, ..FireParams::heat() }).on(Shape::rounded(60.0, 300.0, 480.0, 90.0, 16.0))),
        ("distort heat haze", Fx::distort(DistortParams::HeatHaze { height: 40.0, scale: 14.0, speed: 2.0, amount: 2.0, whole_shape: true }).on(Shape::rounded(0.0, 0.0, 600.0, 400.0, 0.0))),
        ("distort shockwave", Fx::distort(DistortParams::Shockwave { at: [300.0, 200.0], reach: 400.0, width: 14.0, amount: 9.0, chroma: 0.3, t0: 0.9, duration: 1.0 }).on(sh)),
        ("distort lens", Fx::lens(LensParams::default()).on(Shape::rounded(20.0, 20.0, 560.0, 360.0, 60.0))),
        ("distort lens + frost", Fx::lens(LensParams { frost: 0.6, ..Default::default() }).on(Shape::rounded(20.0, 20.0, 560.0, 360.0, 60.0))),
        ("distort CRT", Fx::distort(DistortParams::crt()).on(Shape::rounded(0.0, 0.0, 600.0, 400.0, 12.0))),
        ("distort chroma split", Fx::distort(DistortParams::chroma_split([2.0, 0.0])).on(Shape::rounded(0.0, 0.0, 600.0, 400.0, 12.0))),
        ("shimmer (shape)", Fx::shimmer(ShimmerParams::default()).on(Shape::rounded(0.0, 0.0, 600.0, 400.0, 12.0))),
        ("sparkles x9", Fx::sparkles(SparkleParams::default()).on(Shape::rounded(0.0, 0.0, 600.0, 400.0, 12.0))),
        ("aurora", Fx::aurora(AuroraParams::default()).on(Shape::rounded(0.0, 0.0, 600.0, 400.0, 12.0))),
        ("grain", Fx::grain(GrainParams::default()).on(Shape::rounded(0.0, 0.0, 600.0, 400.0, 12.0))),
        ("fill liquid metal", Fx::fill(LiquidMetalParams::chrome()).on(sh)),
        ("fill liquid metal, still", Fx::fill(LiquidMetalParams { flow: 0.0, ..LiquidMetalParams::chrome() }).on(sh)),
        ("fill liquid metal border 8", Fx::fill(LiquidMetalParams::gold().border(8.0)).on(sh)),
        ("fill liquid metal, element anchor", Fx::fill(LiquidMetalParams::chrome().anchor(FillAnchor::Element)).on(sh)),
        ("fill plastic glossy", Fx::fill(PlasticParams::glossy()).on(sh)),
        ("fill plastic matte", Fx::fill(PlasticParams::matte()).on(sh)),
        ("fill plastic border 8", Fx::fill(PlasticParams::glossy().border(8.0)).on(sh)),
        ("fill mesh gradient (5 colours)", Fx::fill(MeshGradientParams::dusk()).on(sh)),
        ("fill dither bayer (ramp)", Fx::fill(DitherParams::bayer()).on(sh)),
        ("fill dither blue noise (radial)", Fx::fill(DitherParams::grain()).on(sh)),
        ("fill dither palette (noise)", Fx::fill(DitherParams::handheld()).on(sh)),
        ("fill dither backdrop", Fx::fill(DitherParams::backdrop()).on(sh)),
        ("fill grain gradient", Fx::fill(GrainGradientParams::ember()).on(sh)),
        ("fill grain gradient conic, colour", Fx::fill(GrainGradientParams::sweep()).on(sh)),
        ("fill fluted glass", Fx::fill(FlutedGlassParams::reeded()).on(sh)),
        ("fill fluted glass + frost", Fx::fill(FlutedGlassParams::frosted()).on(sh)),
        ("fill metaballs x6", Fx::fill(MetaballParams::lava()).on(sh)),
        ("fill swirl", Fx::fill(SwirlParams::ink()).on(sh)),
        ("fill voronoi", Fx::fill(VoronoiParams::cells()).on(sh)),
        ("fill waves (5 bands)", Fx::fill(WaveParams::sea()).on(sh)),
        ("fill halftone (radial)", Fx::fill(HalftoneParams::print()).on(sh)),
        ("fill halftone backdrop", Fx::fill(HalftoneParams::backdrop()).on(sh)),
    ];
    for (n, mode, angle) in [("motion shutter (fling peak)", BlurMode::Shutter, 360.0f32), ("motion directional", BlurMode::Directional, 360.0)] {
        let rest = Rect::new(100.0, 100.0, 400.0, 200.0);
        let p = MotionBlurParams { mode, shutter_angle: angle, ..Default::default() };
        let s = MotionSamples::from_track(&p, rest, 0.02, S, fling);
        v.push((n, Fx::motion_blur(p, s).on(Shape::rounded(rest.x, rest.y, rest.w, rest.h, 14.0)).source(1, rest.inflate(SRC_PAD))));
    }
    for (_, l) in v.iter_mut() {
        l.time = t;
    }
    v
}

const DRAWS: u32 = 10;

fn timings() -> String {
    let (w, h) = ((600.0 * S) as u32, (400.0 * S) as u32);
    let mut ctx = Ctx::new(w, h);
    if !ctx.timestamps {
        return "timestamp queries unavailable on this adapter\n".into();
    }
    let (_st, src_view) = tex(&ctx.device, 808, 408, FMT, wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING);
    let qs = ctx.device.create_query_set(&wgpu::QuerySetDescriptor { label: None, ty: wgpu::QueryType::Timestamp, count: 64 });
    let qbuf = ctx.device.create_buffer(&wgpu::BufferDescriptor { label: None, size: 512, usage: wgpu::BufferUsages::QUERY_RESOLVE | wgpu::BufferUsages::COPY_SRC, mapped_at_creation: false });
    let rbuf = ctx.device.create_buffer(&wgpu::BufferDescriptor { label: None, size: 512, usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ, mapped_at_creation: false });
    let period = ctx.queue.get_timestamp_period() as f64;
    let mut out = String::from("| effect | covered px (2x) | reads/px | noise/px | backdrop | GPU ms per draw (median; 10 draws/pass) |\n|---|---:|---:|---:|:-:|---:|\n");
    for (name, layer) in timing_layers() {
        let layers = [layer];
        let sources: Vec<&wgpu::TextureView> = vec![&ctx.pyramid.view, &src_view];
        let frame = FxFrame { viewport: [w, h], scale: S, layers: &layers, backdrop: Some(&ctx.backdrop_view), sources: &sources };
        let stats = ctx.fx.prepare(&ctx.device, &ctx.queue, &frame);
        let mut samples = Vec::new();
        for round in 0..4 {
            let mut enc = ctx.device.create_command_encoder(&Default::default());
            for k in 0..8u32 {
                let tw = wgpu::RenderPassTimestampWrites { query_set: &qs, beginning_of_pass_write_index: Some(k * 2), end_of_pass_write_index: Some(k * 2 + 1) };
                let mut p = pass(&mut enc, &ctx.scene_view, Some(wgpu::Color::BLACK), Some(tw));
                for _ in 0..DRAWS {
                    ctx.fx.draw_layer(&mut p, 0);
                }
            }
            enc.resolve_query_set(&qs, 0..16, &qbuf, 0);
            enc.copy_buffer_to_buffer(&qbuf, 0, &rbuf, 0, 128);
            ctx.queue.submit([enc.finish()]);
            let sl = rbuf.slice(..128);
            sl.map_async(wgpu::MapMode::Read, |r| r.unwrap());
            ctx.device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
            {
                let d = sl.get_mapped_range().unwrap();
                let v: &[u64] = bytemuck::cast_slice(&d);
                if round > 0 {
                    for k in 0..8 {
                        let d = v[k * 2 + 1].saturating_sub(v[k * 2]);
                        if d > 0 {
                            samples.push(d as f64 * period / 1e6 / DRAWS as f64);
                        }
                    }
                }
            }
            rbuf.unmap();
        }
        samples.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let med = if samples.is_empty() { f64::NAN } else { samples[samples.len() / 2] };
        let c = layers[0].fx.cost();
        out += &format!("| {name} | {} | {} | {} | {} | {:.3} |\n", stats.pixels, c.samples, c.alu, if c.backdrop { "yes" } else { "" }, med);
    }
    // Empty pass baseline.
    out
}

// ------------------------------------------------------------------ main

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let get = |k: &str| args.iter().position(|a| a == k).and_then(|i| args.get(i + 1)).cloned();
    let effect = get("--effect").or_else(|| get("--still"));
    let still_only = args.iter().any(|a| a == "--still");
    let dir = get("--out").map_or_else(|| PathBuf::from("demo/fx"), PathBuf::from);
    if args.iter().any(|a| a == "--timings") {
        let t = timings();
        println!("{t}");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("timings.md"), t).unwrap();
        return;
    }
    let mut ctx = Ctx::new((W * S) as u32, (H * S) as u32);
    if let Some(dir) = get("--frames") {
        let fps: f32 = get("--fps").map_or(60.0, |s| s.parse().unwrap());
        let secs: f32 = get("--seconds").map_or(2.0, |s| s.parse().unwrap());
        let start: f32 = get("--start").map_or(0.0, |s| s.parse().unwrap());
        let name = effect.expect("--effect");
        let reduced = args.iter().any(|a| a == "--reduced");
        let n = (fps * secs).round() as u32;
        for i in 0..n {
            let px = ctx.render(&name, start + i as f32 / fps, reduced);
            write_png(&Path::new(&dir).join(format!("{name}_{i:04}.png")), ctx.w, ctx.h, &px).unwrap();
        }
        eprintln!("wrote {n} frames to {dir}");
        return;
    }
    if still_only {
        let name = effect.as_deref().unwrap_or("blend");
        std::fs::create_dir_all(&dir).unwrap();
        let px = ctx.render(name, still_time(name), false);
        write_png(&dir.join(format!("{name}.png")), ctx.w, ctx.h, &px).unwrap();
        eprintln!("wrote {name}.png");
        return;
    }
    let list: Vec<&str> = match &effect {
        Some(e) => vec![e.as_str()],
        None => EFFECTS.to_vec(),
    };
    for e in list {
        render_effect(&mut ctx, &dir, e);
    }
    if effect.is_none() && !args.iter().any(|a| a == "--no-timings") {
        let t = timings();
        println!("{t}");
        std::fs::write(dir.join("timings.md"), t).unwrap();
    }
}
