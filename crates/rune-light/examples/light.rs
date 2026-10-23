//! rune-light demos, bench and CPU reference check. Headless: renders offscreen, writes PNGs, frame strips and
//! MP4s (raw frames piped to ffmpeg) under `demo/light/`.
//!
//!     light all | chat | flashlight | emissive | raised | godrays | bench | verify  [--out DIR] [--frames N]

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use rune::Spring;
use rune::color::{Color, oklch, oklcha, srgb_decode, srgb_encode};
use rune::render::{Gpu, Instance, KIND_GLYPH, KIND_RECT, NO_CLIP, Offscreen, Renderer, write_png};
use rune::text::TextSystem;
use rune::tok::TextStyle;
use rune_light::{Gi, Light, LightRenderer, Prim, Scene, Settings, TextPolicy, cpu};

const S: f32 = 2.0;
const W: u32 = 2560;
const H: u32 = 1600;
const FPS: u32 = 60;

// ---------------------------------------------------------------- springs

struct Track {
    t0: f64,
    x0: f32,
    v0: f32,
    target: f32,
    spring: Spring,
}

impl Track {
    fn at(v: f32) -> Track {
        Track { t0: 0.0, x0: v, v0: 0.0, target: v, spring: Spring::new(0.3, 0.0) }
    }

    fn sample(&self, t: f64) -> (f32, f32) {
        self.spring.sample(self.x0, self.v0, self.target, (t - self.t0).max(0.0) as f32)
    }

    fn value(&self, t: f64) -> f32 {
        self.sample(t).0
    }

    fn retarget(&mut self, t: f64, target: f32, spring: Spring) {
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
        let rect = [x * S, y * S, w * S, h * S];
        self.base.push(inst(rect, r * S, fill, border.map(|(c, bw)| (c, bw * S))));
        self.prims.push(light(Prim::from_rune(rect, [r * S; 4]).albedo([fill.r, fill.g, fill.b])));
    }

    /// Paint only (no lighting primitive): hairlines, icons.
    fn paint(&mut self, x: f32, y: f32, w: f32, h: f32, r: f32, fill: Color) {
        self.base.push(inst([x * S, y * S, w * S, h * S], r * S, fill, None));
    }

    fn text(&mut self, x: f32, y: f32, s: &str, size: f32, weight: f32, color: Color) -> f32 {
        let style = TextStyle::new(size, weight);
        let mut quads = Vec::new();
        self.ts.glyphs(s, style, S, [(x * S).round(), (y * S).round()], &mut quads);
        let gamma = rune::text::glyph_coverage_exponent(color, S);
        for q in quads {
            self.text.push(Instance {
                rect: q.rect,
                radii: [0.0; 4],
                fill: color.premul(1.0),
                border: [0.0; 4],
                params: [0.0, KIND_GLYPH, 0.0, gamma],
                uv: [q.uv[0], q.uv[1], 0.0, 0.0],
                clip: NO_CLIP,
                fade: [0; 4],
            });
        }
        self.ts.measure(s, style, S).0
    }

    fn lines(&mut self, x: f32, y: f32, lh: f32, lines: &[&str], size: f32, weight: f32, color: Color) {
        for (i, l) in lines.iter().enumerate() {
            self.text(x, y + i as f32 * lh, l, size, weight, color);
        }
    }

    fn measure(&mut self, s: &str, size: f32, weight: f32) -> f32 {
        self.ts.measure(s, TextStyle::new(size, weight), S).0
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
        let scene = Scene { prims, lights, settings };
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

// ---------------------------------------------------------------- demos

type Frame = (Vec<Prim>, Vec<Instance>, Vec<Instance>, Color, Vec<Light>, Settings);

trait Demo {
    fn name(&self) -> &'static str;
    fn seconds(&self) -> f32 {
        4.0
    }
    fn still(&self) -> f32;
    fn frame(&mut self, ts: &mut TextSystem, t: f32) -> Frame;
}

fn finish(ui: Ui<'_>, lights: Vec<Light>, s: Settings) -> Frame {
    (ui.prims, ui.base, ui.text, ui.clear, lights, s)
}

/// Linear colour from a colour temperature-ish triplet in sRGB.
fn lin(r: f32, g: f32, b: f32) -> [f32; 3] {
    [srgb_decode(r), srgb_decode(g), srgb_decode(b)]
}

/// A smooth closed path through `pts` (Catmull–Rom), u in [0, 1).
fn loop_path(pts: &[[f32; 2]], u: f32) -> [f32; 2] {
    let n = pts.len();
    let x = u.rem_euclid(1.0) * n as f32;
    let i = x.floor() as usize;
    let f = x - i as f32;
    let p = |k: isize| pts[((i as isize + k).rem_euclid(n as isize)) as usize];
    let (p0, p1, p2, p3) = (p(-1), p(0), p(1), p(2));
    let cr = |a: f32, b: f32, c: f32, d: f32| {
        0.5 * ((2.0 * b) + (-a + c) * f + (2.0 * a - 5.0 * b + 4.0 * c - d) * f * f + (-a + 3.0 * b - 3.0 * c + d) * f * f * f)
    };
    [cr(p0[0], p1[0], p2[0], p3[0]), cr(p0[1], p1[1], p2[1], p3[1])]
}

/// A dragged light: waypoints the "hand" jumps between, followed by springs (Kit's palette "calm" .4/0 → softer .7/0
/// for a heavy lamp), sampled at absolute time so frames are deterministic.
struct Drag {
    x: Track,
    y: Track,
    stops: Vec<(f32, [f32; 2])>,
    next: usize,
    spring: Spring,
}

impl Drag {
    fn new(stops: Vec<(f32, [f32; 2])>, spring: Spring) -> Drag {
        let p0 = stops[0].1;
        Drag { x: Track::at(p0[0]), y: Track::at(p0[1]), stops, next: 1, spring }
    }

    fn at(&mut self, t: f32) -> [f32; 2] {
        while self.next < self.stops.len() && self.stops[self.next].0 <= t {
            let (ts, p) = self.stops[self.next];
            self.x.retarget(ts as f64, p[0], self.spring);
            self.y.retarget(ts as f64, p[1], self.spring);
            self.next += 1;
        }
        [self.x.value(t as f64), self.y.value(t as f64)]
    }
}

// 1. Chat timeline lit by a lamp ------------------------------------------------

struct Chat {
    drag: Drag,
}

impl Chat {
    fn new() -> Chat {
        let stops = vec![
            (0.0, [1085.0, 250.0]),
            (0.2, [1060.0, 268.0]),
            (0.9, [928.0, 382.0]),
            (1.6, [1100.0, 530.0]),
            (2.35, [905.0, 657.0]),
            (3.0, [560.0, 658.0]),
            (3.55, [1085.0, 250.0]),
        ];
        Chat { drag: Drag::new(stops, Spring::new(0.85, 0.0)) }
    }
}

fn bubble_text(ui: &mut Ui, x: f32, y: f32, lines: &[&str], color: Color) {
    ui.lines(x, y, 21.0, lines, 14.0, 400.0, color);
}

impl Demo for Chat {
    fn name(&self) -> &'static str {
        "chat"
    }
    fn still(&self) -> f32 {
        1.6
    }
    fn frame(&mut self, ts: &mut TextSystem, t: f32) -> Frame {
        let bg = oklch(0.17, 0.006, 265.0);
        let mut ui = Ui::new(ts, bg);
        let side = oklch(0.195, 0.006, 265.0);
        let txt = oklch(0.93, 0.004, 265.0);
        let txt2 = oklch(0.66, 0.01, 265.0);
        let txt3 = oklch(0.52, 0.01, 265.0);
        let hair = oklcha(1.0, 0.0, 0.0, 0.06);
        let card = oklch(0.235, 0.007, 265.0);
        let user = oklch(0.3, 0.045, 258.0);
        let code = oklch(0.195, 0.008, 265.0);
        let accent = oklch(0.72, 0.15, 255.0);

        // Sidebar: flat, it receives light but doesn't stand proud.
        ui.rect(0.0, 0.0, 268.0, 800.0, 0.0, side, None, |p| p);
        ui.paint(268.0, 0.0, 1.0, 800.0, 0.0, hair);
        ui.text(24.0, 22.0, "OpenCode", 15.0, 600.0, txt);
        ui.rect(20.0, 58.0, 228.0, 34.0, 9.0, card, Some((hair, 1.0)), |p| p.raised(3.0 * S).bevel(3.0 * S, 1.2 * S).gloss(0.5, 0.45));
        ui.text(36.0, 66.0, "+  New session", 13.0, 500.0, txt);
        ui.text(24.0, 116.0, "TODAY", 11.0, 600.0, txt3);
        let sessions = [
            "rune-light: lighting for UIs",
            "Scroll momentum on iOS",
            "Theme tokens codegen",
            "Hex settings screen",
            "Slug text spike",
            "Radiance cascades notes",
            "Motion blur from tracks",
        ];
        for (i, s) in sessions.iter().enumerate() {
            let y = 138.0 + i as f32 * 36.0;
            if i == 0 {
                ui.rect(14.0, y - 7.0, 240.0, 32.0, 8.0, oklcha(1.0, 0.0, 0.0, 0.06), None, |p| p);
            }
            ui.text(26.0, y, s, 13.0, if i == 0 { 500.0 } else { 400.0 }, if i == 0 { txt } else { txt2 });
        }

        // Header
        ui.text(300.0, 22.0, "rune-light: lighting for UIs", 14.0, 600.0, txt);
        let chip = "claude-opus-5.5";
        let cw = ui.measure(chip, 12.0, 500.0);
        ui.rect(1240.0 - cw - 24.0, 16.0, cw + 24.0, 28.0, 14.0, card, Some((hair, 1.0)), |p| p.raised(2.0 * S).bevel(4.0 * S, 1.0 * S).gloss(0.5, 0.4));
        ui.text(1240.0 - cw - 12.0, 22.0, chip, 12.0, 500.0, txt2);
        ui.paint(269.0, 60.0, 1011.0, 1.0, 0.0, hair);

        let raise = |p: Prim| p.raised(7.0 * S).bevel(6.0 * S, 2.0 * S).gloss(0.55, 0.42);
        // User bubble
        let l1 = ["what if lighting effects. like you could put a light and", "it would raytrace around the ui elements…"];
        ui.rect(800.0, 88.0, 440.0, 68.0, 16.0, user, Some((oklcha(1.0, 0.0, 0.0, 0.08), 1.0)), raise);
        bubble_text(&mut ui, 818.0, 101.0, &l1, txt);

        // Assistant
        ui.rect(300.0, 182.0, 600.0, 152.0, 16.0, card, Some((hair, 1.0)), raise);
        bubble_text(
            &mut ui,
            322.0,
            198.0,
            &[
                "Every Rune element is an analytic rounded rect, so the frame is",
                "already an exact distance field. A light can sphere-trace soft",
                "shadows straight through it: contact-hard near each bubble,",
                "wide and soft where the light is far.",
            ],
            txt,
        );
        for (i, chipt) in ["Read shader.wgsl", "Edit rune-light/src/lib.rs", "Run bench"].iter().enumerate() {
            let x = 322.0 + [0.0, 140.0, 330.0][i];
            let w = ui.measure(chipt, 12.0, 500.0) + 22.0;
            ui.rect(x, 292.0, w, 26.0, 13.0, oklch(0.27, 0.008, 265.0), Some((hair, 1.0)), |p| p.on(7.0 * S).raised(2.5 * S).bevel(4.0 * S, 1.0 * S).gloss(0.4, 0.4));
            ui.text(x + 11.0, 297.0, chipt, 12.0, 500.0, txt2);
        }

        // User
        ui.rect(960.0, 360.0, 280.0, 44.0, 16.0, user, Some((oklcha(1.0, 0.0, 0.0, 0.08), 1.0)), raise);
        bubble_text(&mut ui, 980.0, 372.0, &["MAKE THIS SOOO GOOOD"], txt);

        // Assistant with code
        ui.rect(300.0, 430.0, 640.0, 196.0, 16.0, card, Some((hair, 1.0)), raise);
        bubble_text(&mut ui, 322.0, 446.0, &["Here's the API: lights are springs like everything else."], txt);
        ui.rect(318.0, 480.0, 604.0, 126.0, 10.0, code, Some((hair, 1.0)), |p| p.on(7.0 * S).gloss(0.2, 0.5));
        let mono = oklch(0.8, 0.06, 200.0);
        ui.lines(
            336.0,
            494.0,
            20.0,
            &[
                "let lamp = Light::point(x, y, 60.0)",
                "    .color(WARM).intensity(1.6).radius(10.0);",
                "",
                "lights.render(&scene, &[lamp], target);",
                "",
            ],
            13.0,
            400.0,
            mono,
        );

        // Composer
        ui.rect(300.0, 690.0, 940.0, 78.0, 18.0, card, Some((oklcha(1.0, 0.0, 0.0, 0.09), 1.0)), |p| {
            p.raised(10.0 * S).bevel(7.0 * S, 2.5 * S).gloss(0.6, 0.38)
        });
        ui.text(322.0, 708.0, "Ask anything…", 14.0, 400.0, txt3);
        ui.rect(1192.0, 724.0, 32.0, 32.0, 16.0, accent, None, |p| p.on(10.0 * S).raised(4.0 * S).bevel(10.0 * S, 3.0 * S).gloss(0.8, 0.3));
        ui.paint(1203.0, 739.0, 10.0, 2.0, 1.0, oklch(0.2, 0.02, 255.0));

        let p = self.drag.at(t);
        let lamp = Light::point(p[0] * S, p[1] * S, 40.0 * S)
            .color(lin(1.0, 0.78, 0.55))
            .intensity(2.6)
            .radius(11.0 * S)
            .range(1100.0 * S)
            .bulb(0.75)
            .specular(1.2);
        let fill = Light::point(-300.0 * S, -200.0 * S, 900.0 * S)
            .color(lin(0.55, 0.65, 1.0))
            .intensity(0.35)
            .radius(220.0 * S)
            .range(6000.0 * S)
            .specular(0.4);
        let s = Settings {
            ambient: [0.62, 0.63, 0.7],
            albedo_floor: 0.07,
            gi: None,
            rim: 0.9,
            contact_ao: 0.45,
            time: t,
            ..Default::default()
        };
        finish(ui, vec![lamp, fill], s)
    }
}

// 2. Cursor flashlight ------------------------------------------------------------

struct Flashlight;

fn settings_panel(ui: &mut Ui, card: Color, txt: Color, txt2: Color, hair: Color, accent: Color) {
    let raise_card = |p: Prim| p.raised(4.0 * S).bevel(5.0 * S, 1.5 * S).gloss(0.45, 0.45);
    let top = 4.0 * S;
    ui.text(160.0, 70.0, "Settings", 24.0, 600.0, txt);
    ui.text(160.0, 106.0, "Appearance, sound and shortcuts", 13.0, 400.0, txt2);
    let rows: [(&str, &str, u8); 9] = [
        ("Appearance", "Dark", 2),
        ("Accent colour", "", 3),
        ("Reduce motion", "", 1),
        ("Sound effects", "", 0),
        ("Notification volume", "", 4),
        ("Push to talk", "⌥ Space", 2),
        ("Auto-update", "", 1),
        ("Send crash reports", "", 0),
        ("Launch at login", "", 1),
    ];
    let mut y = 150.0;
    for (gi, group) in [&rows[0..3], &rows[3..6], &rows[6..9]].iter().enumerate() {
        let x = 160.0 + gi as f32 * 330.0;
        let h = group.len() as f32 * 56.0;
        ui.rect(x, y, 310.0, h, 14.0, card, Some((hair, 1.0)), raise_card);
        for (ri, (label, value, kind)) in group.iter().enumerate() {
            let ry = y + ri as f32 * 56.0;
            if ri > 0 {
                ui.paint(x + 16.0, ry, 278.0, 1.0, 0.0, hair);
            }
            ui.text(x + 18.0, ry + 19.0, label, 13.0, 500.0, txt);
            let rx = x + 310.0 - 18.0;
            match kind {
                0 | 1 => {
                    let on = *kind == 1;
                    let track = if on { accent } else { oklch(0.33, 0.006, 265.0) };
                    ui.rect(rx - 42.0, ry + 16.0, 42.0, 24.0, 12.0, track, None, |p| p.on(top).gloss(0.4, 0.4));
                    let kx = if on { rx - 22.0 } else { rx - 40.0 };
                    ui.rect(kx, ry + 18.0, 20.0, 20.0, 10.0, oklch(0.96, 0.0, 0.0), None, |p| {
                        p.on(top).raised(5.0 * S).bevel(10.0 * S, 4.0 * S).gloss(0.7, 0.3)
                    });
                }
                2 => {
                    let w = ui.measure(value, 12.0, 500.0) + 20.0;
                    ui.rect(rx - w, ry + 14.0, w, 28.0, 8.0, oklch(0.3, 0.006, 265.0), Some((hair, 1.0)), |p| {
                        p.on(top).raised(3.0 * S).bevel(4.0 * S, 1.2 * S).gloss(0.5, 0.4)
                    });
                    ui.text(rx - w + 10.0, ry + 20.0, value, 12.0, 500.0, txt);
                }
                3 => {
                    for (k, h) in [255.0, 300.0, 350.0, 40.0, 150.0].iter().enumerate() {
                        let c = oklch(0.68, 0.15, *h);
                        let cx = rx - 18.0 - k as f32 * 26.0;
                        ui.rect(cx, ry + 19.0, 18.0, 18.0, 9.0, c, None, |p| p.on(top).raised(3.5 * S).bevel(9.0 * S, 3.0 * S).gloss(0.7, 0.3));
                    }
                }
                _ => {
                    ui.rect(rx - 130.0, ry + 26.0, 130.0, 4.0, 2.0, oklch(0.33, 0.006, 265.0), None, |p| p.on(top));
                    ui.rect(rx - 130.0, ry + 26.0, 82.0, 4.0, 2.0, accent, None, |p| p.on(top));
                    ui.rect(rx - 58.0, ry + 18.0, 20.0, 20.0, 10.0, oklch(0.96, 0.0, 0.0), None, |p| {
                        p.on(top).raised(6.0 * S).bevel(10.0 * S, 4.0 * S).gloss(0.7, 0.3)
                    });
                }
            }
        }
        let _ = &mut y;
    }
    // A wide card below with buttons.
    let by = 340.0;
    ui.rect(160.0, by, 970.0, 150.0, 14.0, card, Some((hair, 1.0)), raise_card);
    ui.text(180.0, by + 20.0, "Keyboard shortcuts", 15.0, 600.0, txt);
    ui.text(180.0, by + 46.0, "Press a key combination to record a new shortcut.", 13.0, 400.0, txt2);
    for (i, k) in ["⌘", "⇧", "K"].iter().enumerate() {
        ui.rect(180.0 + i as f32 * 52.0, by + 82.0, 44.0, 44.0, 10.0, oklch(0.3, 0.006, 265.0), Some((hair, 1.0)), |p| {
            p.on(top).raised(6.0 * S).bevel(7.0 * S, 2.5 * S).gloss(0.6, 0.35)
        });
        ui.text(195.0 + i as f32 * 52.0, by + 94.0, k, 15.0, 600.0, txt);
    }
    ui.rect(940.0, by + 90.0, 170.0, 36.0, 10.0, accent, None, |p| p.on(top).raised(5.0 * S).bevel(6.0 * S, 2.0 * S).gloss(0.7, 0.3));
    ui.text(975.0, by + 99.0, "Record shortcut", 13.0, 600.0, oklch(0.18, 0.03, 255.0));
    ui.rect(780.0, by + 90.0, 140.0, 36.0, 10.0, oklch(0.3, 0.006, 265.0), Some((hair, 1.0)), |p| {
        p.on(top).raised(5.0 * S).bevel(6.0 * S, 2.0 * S).gloss(0.6, 0.35)
    });
    ui.text(828.0, by + 99.0, "Reset", 13.0, 600.0, txt);
    // Lower row of tiles.
    for i in 0..5 {
        let x = 160.0 + i as f32 * 198.0;
        ui.rect(x, 520.0, 180.0, 120.0, 14.0, card, Some((hair, 1.0)), raise_card);
        let names = ["Microphone", "Speakers", "Camera", "Display", "Battery"];
        ui.text(x + 18.0, 538.0, names[i], 13.0, 600.0, txt);
        ui.text(x + 18.0, 560.0, ["MacBook Pro", "Studio Display", "FaceTime HD", "Built-in", "92 %"][i], 12.0, 400.0, txt2);
        ui.rect(x + 18.0, 600.0, 144.0, 18.0, 9.0, oklch(0.27, 0.006, 265.0), None, |p| p.on(top));
        let fill = [0.7, 0.45, 0.0, 0.85, 0.92][i];
        if fill > 0.0 {
            ui.rect(x + 18.0, 600.0, 144.0 * fill, 18.0, 9.0, accent, None, |p| p.on(top).raised(2.0 * S).bevel(9.0 * S, 2.0 * S).gloss(0.6, 0.3));
        }
    }
}

impl Demo for Flashlight {
    fn name(&self) -> &'static str {
        "flashlight"
    }
    fn still(&self) -> f32 {
        1.15
    }
    fn frame(&mut self, ts: &mut TextSystem, t: f32) -> Frame {
        let bg = oklch(0.16, 0.005, 265.0);
        let mut ui = Ui::new(ts, bg);
        settings_panel(
            &mut ui,
            oklch(0.225, 0.006, 265.0),
            oklch(0.94, 0.003, 265.0),
            oklch(0.66, 0.01, 265.0),
            oklcha(1.0, 0.0, 0.0, 0.06),
            oklch(0.7, 0.15, 255.0),
        );
        // The pointer wanders like a hand: one smooth closed path at an even pace.
        let path = [[300.0, 230.0], [700.0, 180.0], [1020.0, 300.0], [860.0, 470.0], [560.0, 600.0], [260.0, 520.0]];
        let p = loop_path(&path, t / 4.0 + 0.02);
        // The torch is held low and to the right of the pointer, aimed at it: the pool stretches and every shadow
        // swings as it moves.
        let lp = [p[0] + 80.0, p[1] + 130.0, 260.0];
        let axis = [p[0] - lp[0], p[1] - lp[1], -lp[2]];
        let torch = Light::point(lp[0] * S, lp[1] * S, lp[2] * S)
            .color(lin(1.0, 0.95, 0.88))
            .intensity(7.0)
            .radius(9.0 * S)
            .range(1800.0 * S)
            .spot(axis, 0.58)
            .specular(1.4);
        let s = Settings {
            ambient: [0.07, 0.075, 0.09],
            albedo_floor: 0.04,
            text: TextPolicy { floor: 0.32, ceiling: 1.3, lit: 1.0, specular: 0.1 },
            rim: 1.0,
            contact_ao: 0.3,
            background_gloss: 0.5,
            time: t,
            ..Default::default()
        };
        finish(ui, vec![torch], s)
    }
}

// 3. Emissive UI lights its neighbours ----------------------------------------------

struct Emissive {
    breath: Track,
    flip: f32,
    up: bool,
}

impl Demo for Emissive {
    fn name(&self) -> &'static str {
        "emissive"
    }
    fn still(&self) -> f32 {
        2.4
    }
    fn frame(&mut self, ts: &mut TextSystem, t: f32) -> Frame {
        // Breathing: a spring retargets between two levels every 1.1 s ("slow" .85/0), so the glow eases in and
        // out with no phase jump; lights cool, never cut.
        while t >= self.flip {
            self.up = !self.up;
            self.breath.retarget(self.flip as f64, if self.up { 1.0 } else { 0.55 }, Spring::new(0.85, 0.0));
            self.flip += 1.1;
        }
        let b = self.breath.value(t as f64);
        let bg = oklch(0.15, 0.006, 265.0);
        let mut ui = Ui::new(ts, bg);
        let card = oklch(0.23, 0.006, 265.0);
        let txt = oklch(0.94, 0.003, 265.0);
        let txt2 = oklch(0.64, 0.01, 265.0);
        let hair = oklcha(1.0, 0.0, 0.0, 0.06);
        let raise = |p: Prim| p.raised(6.0 * S).bevel(6.0 * S, 2.0 * S).gloss(0.5, 0.42);

        ui.text(140.0, 64.0, "Voice memo", 24.0, 600.0, txt);
        ui.text(140.0, 100.0, "Recording · Studio Display microphone", 13.0, 400.0, txt2);

        // Left column of cards around the recorder
        for (i, (title, sub)) in [("Transcript", "Live, 2 speakers"), ("Markers", "3 added"), ("Noise", "Voice isolation on")].iter().enumerate() {
            let y = 150.0 + i as f32 * 128.0;
            ui.rect(140.0, y, 300.0, 108.0, 16.0, card, Some((hair, 1.0)), raise);
            ui.text(162.0, y + 20.0, title, 14.0, 600.0, txt);
            ui.text(162.0, y + 44.0, sub, 13.0, 400.0, txt2);
        }
        // The recorder card in the middle
        ui.rect(470.0, 150.0, 400.0, 364.0, 20.0, card, Some((hair, 1.0)), raise);
        ui.text(494.0, 172.0, "00:42.18", 34.0, 600.0, txt);
        ui.text(494.0, 220.0, "Recording", 13.0, 500.0, oklch(0.75, 0.12, 25.0));
        // Waveform bars (flat, receive light)
        for k in 0..40 {
            let h = 8.0 + 46.0 * (0.5 + 0.5 * ((k as f32 * 0.7).sin() * (k as f32 * 0.23 + 1.0).cos())).abs();
            ui.rect(494.0 + k as f32 * 8.8, 290.0 - h * 0.5, 4.6, h, 2.3, oklch(0.5, 0.01, 265.0), None, |p| p.on(12.0).raised(2.0 * S));
        }
        // Glowing record button with a spring breath
        let red = oklch(0.66, 0.2, 25.0);
        let red_lin = [red.r, red.g, red.b];
        let glow = 7.0 * b;
        ui.rect(620.0, 370.0, 100.0, 100.0, 50.0, oklch(0.27, 0.008, 265.0), Some((hair, 1.0)), |p| {
            p.on(12.0).bevel(16.0 * S, 4.0 * S).gloss(0.6, 0.35)
        });
        ui.rect(640.0, 390.0, 60.0, 60.0, 30.0, red, None, |p| p.on(12.0).emissive([red_lin[0] * glow, red_lin[1] * glow, red_lin[2] * glow]));

        // Progress bar under it, emitting cyan, growing
        let prog = 0.25 + 0.5 * (t / 4.0).min(1.0);
        let cyan = oklch(0.78, 0.13, 210.0);
        ui.rect(140.0, 560.0, 730.0, 10.0, 5.0, oklch(0.24, 0.006, 265.0), None, |p| p);
        let g2 = 1.9 * (0.85 + 0.15 * b);
        ui.rect(140.0, 560.0, 730.0 * prog, 10.0, 5.0, cyan, None, |p| p.emissive([cyan.r * g2, cyan.g * g2, cyan.b * g2]));
        ui.text(140.0, 584.0, "Uploading to OpenCode · 3 of 5 files", 12.0, 500.0, txt2);

        // Right column: notifications with a small green status dot (emissive)
        let green = oklch(0.78, 0.17, 150.0);
        for (i, (title, sub)) in [("Agent finished", "rune-light demos rendered"), ("Build passed", "hivemind · 1m 12s"), ("Review", "Argon: no blockers")].iter().enumerate() {
            let y = 150.0 + i as f32 * 128.0;
            ui.rect(900.0, y, 300.0, 108.0, 16.0, card, Some((hair, 1.0)), raise);
            ui.text(940.0, y + 20.0, title, 14.0, 600.0, txt);
            ui.text(940.0, y + 44.0, sub, 13.0, 400.0, txt2);
            let gd = if i == 0 { 2.2 * (0.6 + 0.4 * b) } else { 0.0 };
            let c = if i == 0 { green } else { oklch(0.45, 0.01, 265.0) };
            ui.rect(918.0, y + 24.0, 10.0, 10.0, 5.0, c, None, |p| p.on(12.0).emissive([c.r * gd, c.g * gd, c.b * gd]));
        }
        // Bottom dock of chips
        for i in 0..6 {
            let x = 140.0 + i as f32 * 178.0;
            ui.rect(x, 640.0, 160.0, 90.0, 16.0, card, Some((hair, 1.0)), raise);
            ui.text(x + 18.0, 660.0, ["Pause", "Marker", "Trim", "Share", "Export", "Delete"][i], 13.0, 600.0, txt);
        }

        let s = Settings {
            ambient: [0.3, 0.31, 0.35],
            gi: Some(Gi { strength: 1.0, air: 0.09, bounce: 0.0, absorption: 0.24 }),
            contact_ao: 0.4,
            rim: 0.6,
            time: t,
            ..Default::default()
        };
        finish(ui, vec![], s)
    }
}

// 4. Raised material -------------------------------------------------------------------

struct Raised;

impl Demo for Raised {
    fn name(&self) -> &'static str {
        "raised"
    }
    fn still(&self) -> f32 {
        1.4
    }
    fn frame(&mut self, ts: &mut TextSystem, t: f32) -> Frame {
        let bg = oklch(0.2, 0.004, 265.0);
        let mut ui = Ui::new(ts, bg);
        let txt = oklch(0.92, 0.003, 265.0);
        let txt2 = oklch(0.62, 0.008, 265.0);
        let body = oklch(0.255, 0.004, 265.0);
        let key = oklch(0.29, 0.004, 265.0);
        // Device body
        ui.rect(120.0, 90.0, 1040.0, 620.0, 36.0, body, Some((oklcha(1.0, 0.0, 0.0, 0.05), 1.0)), |p| {
            p.raised(10.0 * S).bevel(22.0 * S, 7.0 * S).gloss(0.7, 0.32)
        });
        ui.text(170.0, 130.0, "OP-1 Field · Control surface", 15.0, 600.0, txt);
        ui.text(170.0, 156.0, "Bevels from the SDF catch the light", 12.0, 400.0, txt2);
        // Four encoder knobs
        let hues = [255.0, 150.0, 60.0, 25.0];
        for i in 0..4 {
            let cx = 200.0 + i as f32 * 140.0;
            ui.rect(cx - 48.0, 210.0, 96.0, 96.0, 48.0, key, None, |p| p.floating(10.0 * S, 22.0 * S).bevel(30.0 * S, 10.0 * S).gloss(0.9, 0.26));
            let c = oklch(0.7, 0.14, hues[i]);
            ui.rect(cx - 5.0, 222.0, 10.0, 10.0, 5.0, c, None, |p| p);
        }
        // Screen (flat, recessed look by being darker; glossy)
        ui.rect(780.0, 200.0, 330.0, 200.0, 18.0, oklch(0.14, 0.01, 265.0), None, |p| p.gloss(1.0, 0.18));
        ui.text(806.0, 226.0, "128 BPM", 28.0, 600.0, oklch(0.85, 0.12, 150.0));
        ui.text(806.0, 270.0, "Pattern A · bar 12", 13.0, 400.0, oklch(0.6, 0.06, 150.0));
        // Keys: two rows
        for r in 0..2 {
            for i in 0..12 {
                let x = 170.0 + i as f32 * 78.0;
                let y = 450.0 + r as f32 * 110.0;
                let accent = (r == 0 && (i == 2 || i == 7)) || (r == 1 && i == 10);
                let c = if accent { oklch(0.62, 0.17, 35.0) } else { key };
                ui.rect(x, y, 68.0, 92.0, 14.0, c, None, |p| p.floating(10.0 * S, 18.0 * S).bevel(12.0 * S, 4.5 * S).gloss(0.85, 0.28));
                ui.text(x + 12.0, y + 12.0, ["C", "D", "E", "F", "G", "A", "B", "C", "D", "E", "F", "G"][i], 12.0, 600.0, if accent { oklch(0.2, 0.03, 35.0) } else { txt2 });
            }
        }
        // The light sweeps across on a slow arc (one smooth path, no reversal mid-clip).
        let u = (t / self.seconds()).clamp(0.0, 1.0);
        let e = u * u * (3.0 - 2.0 * u);
        let x = 40.0 + e * 1200.0;
        let y = 560.0 - 300.0 * (1.0 - (2.0 * e - 1.0).powi(2));
        let key_l = Light::point(x * S, y * S, 150.0 * S).color(lin(1.0, 0.93, 0.85)).intensity(3.4).radius(26.0 * S).range(2200.0 * S).specular(2.4);
        let fill = Light::point(1400.0 * S, 900.0 * S, 700.0 * S)
            .color(lin(0.5, 0.62, 1.0))
            .intensity(0.4)
            .radius(200.0 * S)
            .range(5000.0 * S)
            .specular(0.6);
        let s = Settings { ambient: [0.3, 0.31, 0.34], rim: 1.1, contact_ao: 0.5, time: t, ..Default::default() };
        finish(ui, vec![key_l, fill], s)
    }
}

// 5. God rays --------------------------------------------------------------------------

struct GodRays;

impl Demo for GodRays {
    fn name(&self) -> &'static str {
        "godrays"
    }
    fn still(&self) -> f32 {
        1.0
    }
    fn frame(&mut self, ts: &mut TextSystem, t: f32) -> Frame {
        let bg = oklch(0.11, 0.014, 265.0);
        let mut ui = Ui::new(ts, bg);
        let card = oklch(0.21, 0.008, 265.0);
        let txt = oklch(0.93, 0.003, 265.0);
        let txt2 = oklch(0.6, 0.01, 265.0);
        let hair = oklcha(1.0, 0.0, 0.0, 0.06);
        let names = [
            "Inbox", "Calendar", "Notes", "Music", "Photos", "Maps", "Files", "Terminal", "Mail", "Weather", "Clock", "Books", "Stocks", "Podcasts",
            "Home", "Wallet", "Health", "Camera", "Code", "Fleet", "Scrawl", "Hex", "Flit", "Rune",
        ];
        let (cols, rows) = (8usize, 3usize);
        let (cw, ch, gx, gy) = (124.0, 124.0, 28.0, 28.0);
        let x0 = (1280.0 - (cols as f32 * cw + (cols - 1) as f32 * gx)) * 0.5;
        let y0 = 290.0;
        for r in 0..rows {
            for c in 0..cols {
                let x = x0 + c as f32 * (cw + gx);
                let y = y0 + r as f32 * (ch + gy);
                let i = r * cols + c;
                ui.rect(x, y, cw, ch, 24.0, card, Some((hair, 1.0)), |p| p.raised(14.0 * S).bevel(8.0 * S, 2.5 * S).gloss(0.5, 0.4));
                let hue = (i as f32 * 47.0) % 360.0;
                ui.rect(x + 20.0, y + 20.0, 40.0, 40.0, 12.0, oklch(0.62, 0.12, hue), None, |p| p);
                ui.text(x + 20.0, y + 84.0, names[i], 13.0, 600.0, txt);
            }
        }
        ui.text(x0, 120.0, "Good evening", 30.0, 600.0, txt);
        ui.text(x0, 164.0, "24 apps · light from behind the grid", 14.0, 400.0, txt2);
        // The source sits behind the grid's top edge, lower than the cards, and drifts once across: every gap
        // throws a shaft and the fan swings with it.
        let u = (t / self.seconds()).clamp(0.0, 1.0);
        let e = u * u * (3.0 - 2.0 * u);
        let lx = 488.0 + e * 304.0;
        let warm = lin(1.0, 0.8, 0.55);
        let sun = Light::point(lx * S, 428.0 * S, 4.0 * S)
            .color(warm)
            .intensity(1.6)
            .radius(12.0 * S)
            .range(3000.0 * S)
            .shafts(1.5)
            .softness(2.2);
        let s = Settings { ambient: [0.5, 0.5, 0.58], shafts_over: 0.12, rim: 0.8, contact_ao: 0.35, time: t, ..Default::default() };
        finish(ui, vec![sun], s)
    }
}

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
            ui.rect(x, y, 104.0 + (i * 7 % 13) as f32, 56.0, 14.0, card, None, |p| p.raised(7.0 * S).bevel(6.0 * S, 2.0 * S).gloss(0.5, 0.4));
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
                    let l = Light::point((640.0 + 420.0 * a.cos()) * S, (400.0 + 260.0 * a.sin()) * S, 60.0 * S)
                        .intensity(1.5)
                        .radius(10.0 * S)
                        .range(1200.0 * S);
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
                    let l = Light::point((300.0 + k as f32 * 90.0) * S, 400.0 * S, 60.0 * S).intensity(1.5).radius(10.0 * S).range(1200.0 * S);
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
    let mut demos: Vec<Box<dyn Demo>> = vec![
        Box::new(Chat::new()),
        Box::new(Flashlight),
        Box::new(Emissive { breath: Track::at(0.8), flip: 0.0, up: false }),
        Box::new(Raised),
        Box::new(GodRays),
    ];
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
