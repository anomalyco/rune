//! rune-light demos, bench and CPU reference check. Headless: renders offscreen, writes PNGs, frame strips and
//! MP4s (raw frames piped to ffmpeg) under `demo/light/`.
//!
//!     light all | chat | flashlight | emissive | raised | godrays | bench | verify  [--out DIR] [--frames N]

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

pub use rune::Spring;
use rune::color::{Color, oklch, oklcha, srgb_decode, srgb_encode};
use rune::render::{Gpu, Instance, KIND_GLYPH, KIND_RECT, NO_CLIP, Offscreen, Renderer, write_png};
use rune::text::TextSystem;
use rune::tok::TextStyle;
use rune_light::{Gi, Light, LightRenderer, Prim, Scene, Settings, TextPolicy, cpu};

thread_local! {
    static SCALE: std::cell::Cell<f32> = const { std::cell::Cell::new(2.0) };
}

/// Device px per point (2 for the headless renders; the live window sets its own).
pub fn sc() -> f32 {
    SCALE.with(|c| c.get())
}

pub fn set_scale(k: f32) {
    SCALE.with(|c| c.set(k));
}

const W: u32 = 2560;
const H: u32 = 1600;
const FPS: u32 = 60;

// ---------------------------------------------------------------- springs

pub struct Track {
    t0: f64,
    x0: f32,
    v0: f32,
    pub target: f32,
    spring: Spring,
}

impl Track {
    pub fn at(v: f32) -> Track {
        Track { t0: 0.0, x0: v, v0: 0.0, target: v, spring: Spring::new(0.3, 0.0) }
    }

    pub fn sample(&self, t: f64) -> (f32, f32) {
        self.spring.sample(self.x0, self.v0, self.target, (t - self.t0).max(0.0) as f32)
    }

    pub fn value(&self, t: f64) -> f32 {
        self.sample(t).0
    }

    pub fn retarget(&mut self, t: f64, target: f32, spring: Spring) {
        let (x, v) = self.sample(t);
        *self = Track { t0: t, x0: x, v0: v, target, spring };
    }
}

// ---------------------------------------------------------------- UI building

struct Ui<'a> {
    ts: &'a mut TextSystem,
    base: Vec<Instance>,
    text: Vec<Instance>,
    prims: Vec<Prim>,
    clear: Color,
}

fn inst(rect: [f32; 4], r: f32, fill: Color, border: Option<(Color, f32)>) -> Instance {
    let (bc, bw) = border.unwrap_or((Color::TRANSPARENT, 0.0));
    Instance {
        rect,
        radii: [r; 4],
        fill: fill.premul(1.0),
        border: if bw > 0.0 { bc.premul(1.0) } else { [0.0; 4] },
        params: [bw, KIND_RECT, 0.0, 1.0],
        uv: [0.0; 4],
        clip: NO_CLIP,
        fade: [0; 4],
    }
}

impl<'a> Ui<'a> {
    fn new(ts: &'a mut TextSystem, clear: Color) -> Self {
        Ui { ts, base: Vec::new(), text: Vec::new(), prims: Vec::new(), clear }
    }

    /// A rect in points. `light` configures its lighting primitive; return `None` to skip it (pure decoration).
    fn rect(&mut self, x: f32, y: f32, w: f32, h: f32, r: f32, fill: Color, border: Option<(Color, f32)>, light: impl FnOnce(Prim) -> Prim) {
        let rect = [x * sc(), y * sc(), w * sc(), h * sc()];
        self.base.push(inst(rect, r * sc(), fill, border.map(|(c, bw)| (c, bw * sc()))));
        self.prims.push(light(Prim::from_rune(rect, [r * sc(); 4]).albedo([fill.r, fill.g, fill.b])));
    }

    /// Paint only (no lighting primitive): hairlines, icons.
    fn paint(&mut self, x: f32, y: f32, w: f32, h: f32, r: f32, fill: Color) {
        self.base.push(inst([x * sc(), y * sc(), w * sc(), h * sc()], r * sc(), fill, None));
    }

    fn text(&mut self, x: f32, y: f32, s: &str, size: f32, weight: f32, color: Color) -> f32 {
        let style = TextStyle::new(size, weight);
        let mut quads = Vec::new();
        self.ts.glyphs(s, style, sc(), [(x * sc()).round(), (y * sc()).round()], &mut quads);
        for q in quads {
            self.text.push(Instance {
                rect: q.rect,
                radii: [0.0; 4],
                fill: color.premul(1.0),
                border: [0.0; 4],
                params: [0.0, KIND_GLYPH, 0.0, 1.0],
                uv: [q.uv[0], q.uv[1], 0.0, 0.0],
                clip: NO_CLIP,
                fade: [0; 4],
            });
        }
        self.ts.measure(s, style, sc()).0
    }

    fn lines(&mut self, x: f32, y: f32, lh: f32, lines: &[&str], size: f32, weight: f32, color: Color) {
        for (i, l) in lines.iter().enumerate() {
            self.text(x, y + i as f32 * lh, l, size, weight, color);
        }
    }

    fn measure(&mut self, s: &str, size: f32, weight: f32) -> f32 {
        self.ts.measure(s, TextStyle::new(size, weight), sc()).0
    }
}

// ---------------------------------------------------------------- GPU context

struct Ctx {
    device: wgpu::Device,
    queue: wgpu::Queue,
    rune: Renderer,
    ts: TextSystem,
    light: LightRenderer,
    base: wgpu::TextureView,
    text: wgpu::TextureView,
    out: Offscreen,
    timestamps: bool,
}

impl Ctx {
    fn new() -> Ctx {
        let Gpu { adapter, device, queue, .. } = Gpu::headless();
        let timestamps = device.features().contains(wgpu::Features::TIMESTAMP_QUERY);
        eprintln!("adapter: {:?}, timestamps: {timestamps}", adapter.get_info().name);
        let mk = |label| {
            device
                .create_texture(&wgpu::TextureDescriptor {
                    label: Some(label),
                    size: wgpu::Extent3d { width: W, height: H, depth_or_array_layers: 1 },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format: wgpu::TextureFormat::Rgba16Float,
                    usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
                    view_formats: &[],
                })
                .create_view(&Default::default())
        };
        let base = mk("ui base");
        let text = mk("ui text");
        let out = Offscreen::with_format(&device, W, H, wgpu::TextureFormat::Rgba8Unorm);
        let rune = Renderer::new(&device, wgpu::TextureFormat::Rgba16Float);
        let light = LightRenderer::new(&device, &queue, W, H, wgpu::TextureFormat::Rgba8Unorm);
        Ctx { device, queue, rune, ts: TextSystem::default(), light, base, text, out, timestamps }
    }

    fn draw_ui(&mut self, base: &[Instance], text: &[Instance], clear: Color) {
        self.rune.prepare(&self.device, &self.queue, &mut self.ts.atlas, base, (W, H));
        let mut enc = self.device.create_command_encoder(&Default::default());
        self.rune.draw(&mut enc, &self.base, [clear.r, clear.g, clear.b, 1.0]);
        self.queue.submit([enc.finish()]);
        self.rune.prepare(&self.device, &self.queue, &mut self.ts.atlas, text, (W, H));
        let mut enc = self.device.create_command_encoder(&Default::default());
        self.rune.draw(&mut enc, &self.text, [0.0; 4]);
        self.queue.submit([enc.finish()]);
    }

    /// Lights the UI already drawn; returns sRGB RGBA8 pixels.
    fn light_frame(&mut self, prims: &[Prim], lights: &[Light], settings: Settings, read: bool) -> Option<Vec<u8>> {
        let mut enc = self.device.create_command_encoder(&Default::default());
        let scene = Scene { prims, lights, settings, emission: None };
        self.light.render(&self.device, &self.queue, &mut enc, &scene, &self.base, &self.text, &self.out.view);
        if read {
            self.out.copy(&mut enc);
        }
        self.queue.submit([enc.finish()]);
        read.then(|| self.out.read(&self.device))
    }
}

fn downscale(src: &[u8], w: u32, h: u32, k: u32) -> Vec<u8> {
    let (ow, oh) = (w / k, h / k);
    let mut out = vec![0u8; (ow * oh * 4) as usize];
    for y in 0..oh {
        for x in 0..ow {
            let mut acc = [0f32; 3];
            for dy in 0..k {
                for dx in 0..k {
                    let i = (((y * k + dy) * w + x * k + dx) * 4) as usize;
                    for c in 0..3 {
                        acc[c] += srgb_decode(src[i + c] as f32 / 255.0);
                    }
                }
            }
            let o = ((y * ow + x) * 4) as usize;
            for c in 0..3 {
                out[o + c] = (srgb_encode(acc[c] / (k * k) as f32) * 255.0).round() as u8;
            }
            out[o + 3] = 255;
        }
    }
    out
}


#[path = "light_demos/mod.rs"]
mod demos;
pub use demos::{DOT_HUES, DOT0, Frame, Hit, N_CTL, Panel, SLIDER, TOGGLE0, accent_for, chat_scene, flashlight_scene, lin};
use demos::Demo;

// ---------------------------------------------------------------- driver

fn render_demo(ctx: &mut Ctx, demo: &mut dyn Demo, out: &Path, frames: Option<u32>) {
    let name = demo.name();
    let still_frame = (demo.still() * FPS as f32).round() as u32;
    if frames == Some(0) {
        let t = still_frame as f32 / FPS as f32;
        let (prims, base, text, clear, lights, mut s) = demo.frame(&mut ctx.ts, t);
        s.seed = 0.0;
        ctx.draw_ui(&base, &text, clear);
        let px = ctx.light_frame(&prims, &lights, s, true).unwrap();
        write_png(&out.join(format!("{name}.png")), W, H, &px).unwrap();
        return;
    }
    let n = frames.unwrap_or((demo.seconds() * FPS as f32).round() as u32);
    let mp4 = out.join(format!("{name}.mp4"));
    let mut ff = Command::new("ffmpeg")
        .args(["-y", "-loglevel", "error", "-f", "rawvideo", "-pix_fmt", "rgba", "-s", &format!("{W}x{H}"), "-framerate", &FPS.to_string(), "-i", "-"])
        .args(["-vf", "scale=out_color_matrix=bt709:out_range=tv", "-c:v", "libx264", "-preset", "medium", "-crf", "18", "-pix_fmt", "yuv420p"])
        .args(["-colorspace", "bt709", "-color_primaries", "bt709", "-color_trc", "bt709", "-movflags", "+faststart"])
        .arg(&mp4)
        .stdin(Stdio::piped())
        .spawn()
        .expect("ffmpeg");
    let stdin = ff.stdin.as_mut().unwrap();
    let strip_at: Vec<u32> = (0..5).map(|i| (i * n) / 5 + n / 10).collect();
    let mut strip: Vec<Vec<u8>> = Vec::new();
    let t0 = std::time::Instant::now();
    for f in 0..n {
        let t = f as f32 / FPS as f32;
        let (prims, base, text, clear, lights, mut s) = demo.frame(&mut ctx.ts, t);
        s.seed = 0.0;
        ctx.draw_ui(&base, &text, clear);
        let px = ctx.light_frame(&prims, &lights, s, true).unwrap();
        if f == still_frame {
            write_png(&out.join(format!("{name}.png")), W, H, &px).unwrap();
        }
        if strip_at.contains(&f) {
            strip.push(downscale(&px, W, H, 4));
        }
        stdin.write_all(&px).expect("ffmpeg write");
    }
    drop(ff.stdin.take());
    ff.wait().expect("ffmpeg");
    let (sw, sh) = (W / 4, H / 4);
    let gap = 8;
    let tw = sw * strip.len() as u32 + gap * (strip.len() as u32 - 1);
    let mut img = vec![12u8; (tw * sh * 4) as usize];
    for (k, fr) in strip.iter().enumerate() {
        let ox = k as u32 * (sw + gap);
        for y in 0..sh {
            let src = &fr[(y * sw * 4) as usize..((y + 1) * sw * 4) as usize];
            let d = ((y * tw + ox) * 4) as usize;
            img[d..d + src.len()].copy_from_slice(src);
        }
    }
    for i in (3..img.len()).step_by(4) {
        img[i] = 255;
    }
    write_png(&out.join(format!("{name}-strip.png")), tw, sh, &img).unwrap();
    eprintln!("{name}: {n} frames in {:.1}s → {}", t0.elapsed().as_secs_f32(), mp4.display());
}

fn median(v: &mut [f64]) -> f64 {
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    v[v.len() / 2]
}

fn bench(ctx: &mut Ctx, out: &Path) {
    if !ctx.timestamps {
        eprintln!("no TIMESTAMP_QUERY on this adapter");
        return;
    }
    // 1280×800 pt at 2×: ~100 raised occluders (a dense chat + sidebar), 4 lights.
    let mut ts = TextSystem::default();
    let mut ui = Ui::new(&mut ts, oklch(0.17, 0.006, 265.0));
    let card = oklch(0.235, 0.007, 265.0);
    for i in 0..10 {
        for j in 0..10 {
            let x = 30.0 + j as f32 * 124.0;
            let y = 20.0 + i as f32 * 78.0;
            ui.rect(x, y, 104.0 + (i * 7 % 13) as f32, 56.0, 14.0, card, None, |p| p.raised(7.0 * sc()).bevel(6.0 * sc(), 2.0 * sc()).gloss(0.5, 0.4));
            ui.text(x + 12.0, y + 18.0, "Message", 13.0, 400.0, oklch(0.9, 0.0, 0.0));
        }
    }
    let (prims, base, text, clear) = (ui.prims, ui.base, ui.text, ui.clear);
    ctx.draw_ui(&base, &text, clear);
    let cfgs: Vec<(&str, usize, bool, bool)> = vec![
        ("1 light", 1, false, false),
        ("4 lights", 4, false, false),
        ("8 lights", 8, false, false),
        ("4 lights + shafts", 4, true, false),
        ("GI only (emitters)", 0, false, true),
        ("4 lights + GI bounce", 4, false, true),
    ];
    let mut report = String::from("{\n");
    for (ci, (label, nl, shafts, gi)) in cfgs.iter().enumerate() {
        let mut runs: Vec<[f64; 6]> = Vec::new();
        for f in 0..90 {
            let t = f as f32 / 60.0;
            let lights: Vec<Light> = (0..*nl)
                .map(|k| {
                    let a = t * 0.8 + k as f32 * 1.7;
                    let l = Light::point((640.0 + 420.0 * a.cos()) * sc(), (400.0 + 260.0 * a.sin()) * sc(), 60.0 * sc())
                        .intensity(1.5)
                        .radius(10.0 * sc())
                        .range(1200.0 * sc());
                    if *shafts { l.shafts(0.4) } else { l }
                })
                .collect();
            let mut p = prims.clone();
            if *gi {
                for (k, pr) in p.iter_mut().enumerate() {
                    if k % 9 == 0 {
                        *pr = pr.emissive([2.0, 0.6, 0.3]);
                    }
                }
            }
            let s = Settings {
                gi: gi.then(|| Gi { bounce: if *nl > 0 { 0.25 } else { 0.0 }, ..Default::default() }),
                ..Default::default()
            };
            ctx.light_frame(&p, &lights, s, false);
            if let Some(tm) = ctx.light.read_timings(&ctx.device) {
                if f >= 30 {
                    runs.push([tm.field, tm.shadows, tm.gi, tm.raster, tm.total, 0.0]);
                }
            }
        }
        // Wall clock of 120 back-to-back frames with no readback: GPU-bound, an upper bound on the whole lighting pass
        // (surface + field + shadows + GI + composite), since render-pass timestamps aren't ordered on tile GPUs.
        let wall = {
            let lights: Vec<Light> = (0..*nl)
                .map(|k| {
                    let l = Light::point((300.0 + k as f32 * 90.0) * sc(), 400.0 * sc(), 60.0 * sc()).intensity(1.5).radius(10.0 * sc()).range(1200.0 * sc());
                    if *shafts { l.shafts(0.4) } else { l }
                })
                .collect();
            let mut p = prims.clone();
            if *gi {
                for (k, pr) in p.iter_mut().enumerate() {
                    if k % 9 == 0 {
                        *pr = pr.emissive([2.0, 0.6, 0.3]);
                    }
                }
            }
            let s = Settings { gi: gi.then(|| Gi { bounce: if *nl > 0 { 0.25 } else { 0.0 }, ..Default::default() }), ..Default::default() };
            ctx.light_frame(&p, &lights, s, false);
            ctx.device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
            let t0 = std::time::Instant::now();
            for _ in 0..120 {
                ctx.light_frame(&p, &lights, s, false);
            }
            ctx.device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
            t0.elapsed().as_secs_f64() * 1000.0 / 120.0
        };
        let col = |i: usize| median(&mut runs.iter().map(|r| r[i]).collect::<Vec<_>>());
        let (fi, sh, gi_t) = (col(0), col(1), col(2));
        println!("{label:>24}: field {fi:.3}  shadows {sh:.3}  gi {gi_t:.3} ms (timestamps)   whole lighting frame {wall:.3} ms (wall, GPU-bound)  ({} prims)", prims.len());
        report += &format!(
            "  \"{label}\": {{\"field\": {fi:.4}, \"shadows\": {sh:.4}, \"gi\": {gi_t:.4}, \"frame_wall\": {wall:.4}}}{}\n",
            if ci + 1 < cfgs.len() { "," } else { "" }
        );
    }
    report += "}\n";
    std::fs::write(out.join("bench.json"), report).unwrap();
}

fn f16(h: u16) -> f32 {
    let s = if h & 0x8000 != 0 { -1.0 } else { 1.0 };
    let e = ((h >> 10) & 0x1f) as i32;
    let m = (h & 0x3ff) as f32;
    if e == 0 {
        s * m * 2f32.powi(-24)
    } else if e == 31 {
        s * f32::INFINITY
    } else {
        s * (1.0 + m / 1024.0) * 2f32.powi(e - 15)
    }
}

fn verify(ctx: &mut Ctx) {
    // Two raised cards and a floating popover; one light low and to the side, so the floor has long penumbrae.
    let prims = vec![
        Prim::rect(600.0, 500.0, 400.0, 200.0).radius(28.0).raised(14.0),
        Prim::rect(1200.0, 650.0, 300.0, 300.0).radius(60.0).raised(24.0),
        Prim::rect(1500.0, 300.0, 360.0, 160.0).radius(20.0).floating(40.0, 48.0),
    ];
    let light = Light::point(400.0, 420.0, 120.0).intensity(1.5).radius(24.0).range(6000.0);
    let mut ts = TextSystem::default();
    let ui = Ui::new(&mut ts, oklch(0.2, 0.0, 0.0));
    let (base, text, clear) = (ui.base, ui.text, ui.clear);
    ctx.draw_ui(&base, &text, clear);
    ctx.light_frame(&prims, &[light], Settings::default(), false);
    // Read back layer 0 of the visibility texture.
    let (fw, fh) = ctx.light.field_size();
    let row = (fw * 8).div_ceil(256) * 256;
    let buf = ctx.device.create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size: (row * fh) as u64,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut enc = ctx.device.create_command_encoder(&Default::default());
    enc.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo { texture: ctx.light.visibility_texture(), mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
        wgpu::TexelCopyBufferInfo { buffer: &buf, layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(row), rows_per_image: Some(fh) } },
        wgpu::Extent3d { width: fw, height: fh, depth_or_array_layers: 1 },
    );
    ctx.queue.submit([enc.finish()]);
    let slice = buf.slice(..);
    slice.map_async(wgpu::MapMode::Read, |r| r.unwrap());
    ctx.device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
    let data = slice.get_mapped_range().unwrap().to_vec();
    let gpu = |x: u32, y: u32| {
        let i = (y * row + x * 8) as usize;
        f16(u16::from_le_bytes([data[i], data[i + 1]]))
    };
    // A grid of field texels over the whole frame: the estimator must match the GPU everywhere; in penumbrae
    // (ground truth strictly between 0 and 1) we report how far the fast estimate is from brute force.
    let top_z = |p: [f32; 2]| {
        prims.iter().rev().find(|pr| cpu::field(&[(**pr).raised(1.0).floating(pr.z[0], pr.z[1])], p)[0] < 0.0).map_or(0.0, |pr| pr.z[1])
    };
    let (mut max_est, mut sum_est, mut n_all) = (0.0f32, 0.0f32, 0u32);
    let (mut sum_pen, mut n_pen, mut max_pen) = (0.0f32, 0u32, 0.0f32);
    let mut rows = Vec::new();
    let mut worst = String::new();
    for gy in 0..40u32 {
        for gx in 0..64u32 {
            let (x, y) = (gx * fw / 64 + 3, gy * fh / 40 + 5);
            let pxy = [(x as f32 + 0.5) * 2.0, (y as f32 + 0.5) * 2.0];
            let z = top_z(pxy);
            // Skip receivers within 3 px of an edge (the receiver height is ambiguous there by construction).
            if prims.iter().any(|pr| (cpu::field(std::slice::from_ref(pr), pxy)[0]).abs() < 3.0) {
                continue;
            }
            let p = [pxy[0], pxy[1], z + 0.35];
            let g = gpu(x, y);
            let e = cpu::estimate(&prims, &light, p);
            if (g - e).abs() > max_est {
                worst = format!("worst texel ({x},{y}) z {z}: gpu {g:.3} vs cpu {e:.3}");
            }
            max_est = max_est.max((g - e).abs());
            sum_est += (g - e).abs();
            n_all += 1;
            let gt = cpu::ground_truth(&prims, &light, p, 20);
            if gt > 0.02 && gt < 0.98 {
                sum_pen += (g - gt).abs();
                max_pen = max_pen.max((g - gt).abs());
                n_pen += 1;
                if rows.len() < 12 {
                    rows.push(format!("  ({x:4},{y:4}) z {z:4.1}  gpu {g:.3}  cpu-est {e:.3}  ground-truth {gt:.3}"));
                }
            }
        }
    }
    for r in &rows {
        println!("{r}");
    }
    println!("{worst}");
    println!(
        "{n_all} texels: max |gpu − cpu estimator| = {max_est:.4}, mean {:.5}; {n_pen} penumbra texels: mean |gpu − ground truth| = {:.3}, max {max_pen:.3}",
        sum_est / n_all.max(1) as f32,
        sum_pen / n_pen.max(1) as f32
    );
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let cmd = args.get(1).map(String::as_str).unwrap_or("all");
    let mut out = PathBuf::from("demo/light");
    let mut frames = None;
    let mut i = 2;
    while i < args.len() {
        match args[i].as_str() {
            "--out" => {
                out = PathBuf::from(&args[i + 1]);
                i += 1;
            }
            "--frames" => {
                frames = args[i + 1].parse().ok();
                i += 1;
            }
            _ => {}
        }
        i += 1;
    }
    std::fs::create_dir_all(&out).unwrap();
    let mut ctx = Ctx::new();
    let mut demos = demos::all_demos();
    match cmd {
        "bench" => bench(&mut ctx, &out),
        "verify" => verify(&mut ctx),
        "all" => {
            for d in demos.iter_mut() {
                render_demo(&mut ctx, d.as_mut(), &out, frames);
            }
        }
        name => {
            let d = demos.iter_mut().find(|d| d.name() == name).unwrap_or_else(|| panic!("unknown demo {name}"));
            render_demo(&mut ctx, d.as_mut(), &out, frames);
        }
    }
}

/// Appends a line of text (points) to a built frame's text layer.
pub fn caption(ts: &mut TextSystem, f: &mut Frame, x: f32, y: f32, s: &str, color: Color) {
    let mut ui = Ui::new(ts, f.3);
    ui.text(x, y, s, 12.0, 500.0, color);
    f.2.extend(ui.text);
}
