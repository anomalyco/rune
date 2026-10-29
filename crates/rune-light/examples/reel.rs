//! The radiance-cascades reel: Rune's UI lit only by its own glowing elements, a breakdown that steps through
//! rune-light's real GI buffers, thick refracting glass, and an effect whose light enters the cascades. Headless.
//!
//!     reel render [--out demo/reel] [--from S] [--to S]   H.264 (8-bit) + HEVC (10-bit) MP4s, poster, contact sheet
//!     reel stills T1,T2,… [--out DIR]                     PNGs at those times, for review
//!     reel bench [--out demo/reel]                         GPU timestamps per pass → bench.json
//!
//! Frames render in Rgba16Float linear light and are converted on the GPU to BT.709 Y'CbCr 4:2:0 with blue-noise TPDF
//! dither at the exact 8- and 10-bit quantisation steps, then piped raw to ffmpeg (no swscale conversion).

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};

use rune::color::{Color, oklch, oklcha};
use rune::render::{Instance, KIND_GLYPH, KIND_RECT, KIND_SHADOW, NO_CLIP, Renderer};
use rune::text::TextSystem;
use rune::tok::TextStyle;
use rune_fx::{DistortParams, ElectricParams, ElectricStyle, Fx, FxFrame, FxLayer, FxOptions, FxRenderer, Shape, rgb};
use rune_light::{CASCADES, Gi, Glass, GlassLight, GlassRenderer, GlassSettings, LightRenderer, Prim, Scene, Settings, TextPolicy, Timings};

const S: f32 = 2.0;
const W: u32 = 2560;
const H: u32 = 1600;
const FPS: u32 = 60;
const DURATION: f32 = 31.0;
const F16: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;
const MIPS: u32 = 12;

// ---------------------------------------------------------------- springs (SwiftUI parameterisation: duration, bounce)

#[derive(Clone, Copy)]
struct Spring {
    duration: f32,
    bounce: f32,
}

const fn sp(duration: f32, bounce: f32) -> Spring {
    Spring { duration, bounce }
}

impl Spring {
    fn eval(&self, d0: f32, v0: f32, t: f32) -> (f32, f32) {
        let w = std::f32::consts::TAU / self.duration.max(1e-3);
        let z = (1.0 - self.bounce).clamp(0.05, 1.0);
        if z > 0.9999 {
            let c = v0 + w * d0;
            let e = (-w * t).exp();
            return ((d0 + c * t) * e, (c - w * (d0 + c * t)) * e);
        }
        let wd = w * (1.0 - z * z).sqrt();
        let a = d0;
        let b = (v0 + z * w * d0) / wd;
        let e = (-z * w * t).exp();
        let (s, c) = (wd * t).sin_cos();
        (e * (a * c + b * s), e * ((b * wd - z * w * a) * c - (a * wd + z * w * b) * s))
    }
}

/// A value driven by springs retargeted at key times; a pure function of absolute time (velocity carries over).
struct Keys {
    init: f32,
    keys: Vec<(f32, f32, Spring)>,
}

fn keys(init: f32, k: &[(f32, f32, Spring)]) -> Keys {
    Keys { init, keys: k.to_vec() }
}

impl Keys {
    fn at(&self, t: f32) -> f32 {
        let (mut x, mut v, mut t0, mut target, mut spring) = (self.init, 0.0, 0.0, self.init, sp(0.3, 0.0));
        for &(tk, tg, s) in &self.keys {
            if tk > t {
                break;
            }
            let (d, vv) = spring.eval(x - target, v, tk - t0);
            x = target + d;
            v = vv;
            t0 = tk;
            target = tg;
            spring = s;
        }
        target + spring.eval(x - target, v, t - t0).0
    }
}

/// 0 → 1 released at `t0` with a critically damped spring of `dur`.
fn rise(t: f32, t0: f32, dur: f32) -> f32 {
    if t <= t0 {
        return 0.0;
    }
    1.0 - sp(dur, 0.0).eval(1.0, 0.0, t - t0).0
}

fn window(t: f32, a: f32, b: f32, fade: f32) -> f32 {
    rise(t, a, fade) * (1.0 - rise(t, b, fade))
}

fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

// ---------------------------------------------------------------- timeline

const T_TILT: f32 = 7.0;
const T_INPUTS: f32 = 8.3;
const T_CASCADES: f32 = 10.0;
const T_MERGE: f32 = 12.9;
const T_FLUENCE: f32 = 15.2;
const T_UNTILT: f32 = 16.4;
const T_GLASS: f32 = 17.8;
const T_ARC: f32 = 25.2;
const T_END: f32 = 28.4;

// ---------------------------------------------------------------- UI building

struct Ui<'a> {
    ts: &'a mut TextSystem,
    base: Vec<Instance>,
    text: Vec<Instance>,
    prims: Vec<Prim>,
    /// pt → device px: px = (o + x·k)·S
    k: f32,
    o: [f32; 2],
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
    fn new(ts: &'a mut TextSystem, k: f32, o: [f32; 2]) -> Self {
        Ui { ts, base: Vec::new(), text: Vec::new(), prims: Vec::new(), k, o }
    }

    fn px(&self, x: f32, y: f32, w: f32, h: f32) -> [f32; 4] {
        [(self.o[0] + x * self.k) * S, (self.o[1] + y * self.k) * S, w * self.k * S, h * self.k * S]
    }

    fn d(&self, v: f32) -> f32 {
        v * self.k * S
    }

    #[allow(clippy::too_many_arguments)]
    fn rect(&mut self, x: f32, y: f32, w: f32, h: f32, r: f32, fill: Color, border: Option<(Color, f32)>, light: impl FnOnce(Prim, f32) -> Prim) {
        let rect = self.px(x, y, w, h);
        let rr = self.d(r);
        self.base.push(inst(rect, rr, fill, border.map(|(c, bw)| (c, bw * S))));
        let p = Prim::rect(rect[0], rect[1], rect[2], rect[3]).radius(rr).albedo([fill.r, fill.g, fill.b]);
        self.prims.push(light(p, self.k * S));
    }

    fn paint(&mut self, x: f32, y: f32, w: f32, h: f32, r: f32, fill: Color) {
        let rect = self.px(x, y, w, h);
        let rr = self.d(r);
        self.base.push(inst(rect, rr, fill, None));
    }

    fn shadow(&mut self, x: f32, y: f32, w: f32, h: f32, r: f32, sigma: f32, color: Color) {
        let mut i = inst(self.px(x, y, w, h), self.d(r), color, None);
        i.params = [0.0, KIND_SHADOW, self.d(sigma), 1.0];
        self.base.push(i);
    }

    fn text(&mut self, x: f32, y: f32, s: &str, size: f32, weight: f32, color: Color) -> f32 {
        let style = TextStyle::new(size, weight);
        let scale = S * self.k;
        let mut quads = Vec::new();
        self.ts.glyphs(s, style, scale, [((self.o[0] + x * self.k) * S).round(), ((self.o[1] + y * self.k) * S).round()], &mut quads);
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
        self.ts.measure(s, style, scale).0 / self.k
    }

    fn measure(&mut self, s: &str, size: f32, weight: f32) -> f32 {
        self.ts.measure(s, TextStyle::new(size, weight), S * self.k).0 / self.k
    }
}

fn scaled(c: Color, k: f32) -> [f32; 3] {
    [c.r * k, c.g * k, c.b * k]
}

/// Everything animated, at one instant.
struct St {
    t: f32,
    send: f32,
    rec: f32,
    prog: f32,
    prog_glow: f32,
    toast_x: f32,
    toast_glow: f32,
    status: f32,
    arc: f32,
    danger_glow: f32,
}

fn state(t: f32) -> St {
    // Emitters warm up one after another (lights cool, never cut).
    let warm = |t0: f32| rise(t, t0, 0.9);
    // Breathing: a spring retargeted every 1.1 s between two levels.
    let mut bk = Vec::new();
    let mut k = 0.25;
    let mut up = true;
    while k < DURATION + 2.0 {
        bk.push((k, if up { 1.0 } else { 0.5 }, sp(0.9, 0.0)));
        up = !up;
        k += 1.1;
    }
    let breath = keys(0.5, &bk).at(t);
    let click = 3.3;
    // The send button flares on the click and cools exponentially back to rest.
    let flare = if t > click { (-(t - click) / 0.35).exp() * rise(t, click, 0.08) } else { 0.0 };
    let send = warm(0.1) * (1.0 + 1.6 * flare);
    let prog = keys(0.08, &[(click + 0.15, 1.0, sp(2.8, 0.0))]).at(t).min(1.0);
    let done = 6.1;
    let pflash = if t > done { (-(t - done) / 0.5).exp() * rise(t, done, 0.1) } else { 0.0 };
    // Fills, floods on completion, then cools to a low resting glow.
    let prog_glow = warm(0.5) * (0.7 + 0.3 * rise(t, click, 0.6)) * (1.0 - 0.7 * rise(t, done + 0.4, 1.6)) + 1.4 * pflash;
    let toast_x = keys(420.0, &[(1.2, 0.0, sp(0.75, 0.12))]).at(t);
    let toast_glow = warm(1.0);
    let arc_in = rise(t, T_ARC, 0.35);
    let arc_out = rise(t, T_END - 0.4, 0.6);
    St {
        t,
        send,
        rec: warm(0.7) * breath,
        prog,
        prog_glow,
        toast_x,
        toast_glow,
        status: warm(0.3),
        arc: arc_in * (1.0 - arc_out),
        danger_glow: arc_in * (1.0 - arc_out),
    }
}

const BG: (f32, f32, f32) = (0.115, 0.012, 268.0);

fn build(ui: &mut Ui, st: &St) {
    let bg = oklch(0.155, 0.01, 268.0);
    let side = oklch(0.175, 0.01, 268.0);
    let card = oklch(0.215, 0.011, 268.0);
    let card2 = oklch(0.245, 0.012, 268.0);
    let user = oklch(0.27, 0.04, 268.0);
    let txt = oklch(0.94, 0.005, 268.0);
    let txt2 = oklch(0.70, 0.012, 268.0);
    let txt3 = oklch(0.54, 0.012, 268.0);
    let hair = oklcha(1.0, 0.0, 0.0, 0.07);
    let raise = |p: Prim, k: f32| p.raised(6.0 * k).bevel(6.0 * k, 2.0 * k).gloss(0.5, 0.42);
    let chip = |p: Prim, k: f32| p.on(6.0 * k).raised(2.5 * k).bevel(4.0 * k, 1.0 * k).gloss(0.45, 0.4);

    // Window: soft drop shadow on the desktop, then the window itself (flat: it receives light).
    ui.shadow(0.0, 18.0, 1280.0, 800.0, 18.0, 40.0, oklcha(0.0, 0.0, 0.0, 0.6));
    ui.rect(0.0, 0.0, 1280.0, 800.0, 18.0, bg, Some((oklcha(1.0, 0.0, 0.0, 0.08), 1.0)), |p, _| p);

    // Sidebar
    ui.rect(0.0, 0.0, 244.0, 800.0, 18.0, side, None, |p, _| p);
    let r = ui.d(18.0);
    ui.base.last_mut().unwrap().radii = [r, 0.0, 0.0, r];
    *ui.prims.last_mut().unwrap() = ui.prims.last().unwrap().radii([r, 0.0, 0.0, r]);
    ui.paint(244.0, 0.0, 1.0, 800.0, 0.0, hair);
    for (i, c) in [oklch(0.62, 0.17, 25.0), oklch(0.78, 0.15, 85.0), oklch(0.72, 0.17, 145.0)].iter().enumerate() {
        ui.paint(20.0 + i as f32 * 18.0, 20.0, 11.0, 11.0, 5.5, oklcha(c.to_oklab()[0], 0.02, 268.0, 0.9));
    }
    ui.text(22.0, 52.0, "OpenCode", 16.0, 600.0, txt);
    ui.rect(16.0, 86.0, 212.0, 36.0, 10.0, card, Some((hair, 1.0)), |p, k| p.raised(3.0 * k).bevel(3.0 * k, 1.2 * k).gloss(0.5, 0.45));
    ui.text(32.0, 95.0, "+  New session", 13.0, 500.0, txt);
    ui.text(22.0, 146.0, "TODAY", 11.0, 600.0, txt3);
    let sessions = ["Radiance cascades for UI", "Thick glass optics", "Electric arcs into GI", "Theme tokens codegen", "Scroll momentum", "Slug text spike"];
    for (i, s) in sessions.iter().enumerate() {
        let y = 170.0 + i as f32 * 38.0;
        if i == 0 {
            ui.rect(10.0, y - 8.0, 224.0, 34.0, 9.0, oklcha(1.0, 0.0, 0.0, 0.055), None, |p, _| p);
        }
        ui.text(24.0, y, s, 13.0, if i == 0 { 500.0 } else { 400.0 }, if i == 0 { txt } else { txt2 });
    }
    // Status pill with a green light.
    ui.rect(16.0, 740.0, 212.0, 40.0, 20.0, card, Some((hair, 1.0)), |p, k| p.raised(3.0 * k).bevel(5.0 * k, 1.5 * k).gloss(0.5, 0.4));
    let green = oklch(0.80, 0.19, 150.0);
    let g = 3.2 * st.status;
    ui.rect(34.0, 755.0, 10.0, 10.0, 5.0, green, None, |p, k| p.on(3.0 * k).emissive(scaled(green, g)));
    ui.text(54.0, 751.0, "Connected · studio", 12.0, 500.0, txt2);

    // Header
    ui.text(272.0, 22.0, "Radiance cascades for UI", 15.0, 600.0, txt);
    let model = "claude-opus-5.5";
    let mw = ui.measure(model, 12.0, 500.0);
    ui.rect(900.0 - mw - 26.0, 16.0, mw + 26.0, 28.0, 14.0, card, Some((hair, 1.0)), |p, k| p.raised(2.0 * k).bevel(4.0 * k, 1.0 * k).gloss(0.5, 0.4));
    ui.text(900.0 - mw - 13.0, 22.0, model, 12.0, 500.0, txt2);
    ui.paint(245.0, 60.0, 1035.0, 1.0, 0.0, hair);

    // Chat column
    let cx = 272.0;
    let cw = 628.0;
    let ub = "come on, do radiance cascades UI";
    let uw = ui.measure(ub, 14.0, 400.0) + 36.0;
    ui.rect(cx + cw - uw, 82.0, uw, 46.0, 16.0, user, Some((oklcha(1.0, 0.0, 0.0, 0.08), 1.0)), raise);
    ui.text(cx + cw - uw + 18.0, 95.0, ub, 14.0, 400.0, txt);

    ui.rect(cx, 150.0, cw, 140.0, 16.0, card, Some((hair, 1.0)), raise);
    let lines = [
        "Every Rune element is an analytic rounded rect, so the frame is already",
        "an exact distance field. Glowing elements are the only lights here: five",
        "cascades of probes trace that field, merge top down, and light the cards",
        "around them with soft coloured bounce and shadow.",
    ];
    for (i, l) in lines.iter().enumerate() {
        ui.text(cx + 22.0, 168.0 + i as f32 * 22.0, l, 14.0, 400.0, txt);
    }
    for (i, c) in ["Read gi.wgsl", "Edit cascades", "Run bench"].iter().enumerate() {
        let x = cx + 22.0 + [0.0, 116.0, 236.0][i];
        let w = ui.measure(c, 12.0, 500.0) + 22.0;
        ui.rect(x, 256.0 - 4.0 + 8.0, w, 24.0, 12.0, card2, Some((hair, 1.0)), chip);
        ui.text(x + 11.0, 264.0, c, 12.0, 500.0, txt2);
    }

    // Tool card with the progress bar.
    ui.rect(cx, 310.0, cw, 102.0, 16.0, card, Some((hair, 1.0)), raise);
    ui.text(cx + 22.0, 326.0, "Rendering reel", 14.0, 600.0, txt);
    let frame = (st.prog * 1860.0).round() as u32;
    let sub = format!("frame {frame} / 1860 · 2560×1600 · 60 fps");
    ui.text(cx + 22.0, 348.0, &sub, 12.0, 400.0, txt2);
    let pct = format!("{}%", (st.prog * 100.0).round() as u32);
    let pw = ui.measure(&pct, 12.0, 600.0);
    ui.text(cx + cw - 22.0 - pw, 326.0, &pct, 12.0, 600.0, txt2);
    ui.rect(cx + 22.0, 380.0, cw - 44.0, 10.0, 5.0, oklch(0.19, 0.01, 268.0), None, |p, k| p.on(6.0 * k));
    let cyan = oklch(0.82, 0.14, 205.0);
    let pg = 1.7 * st.prog_glow;
    ui.rect(cx + 22.0, 380.0, (cw - 44.0) * st.prog, 10.0, 5.0, cyan, None, |p, k| p.on(6.0 * k).emissive(scaled(cyan, pg)));

    // Code card
    ui.rect(cx, 432.0, cw, 156.0, 16.0, card, Some((hair, 1.0)), raise);
    ui.text(cx + 22.0, 448.0, "Glowing elements are lights:", 14.0, 400.0, txt);
    ui.rect(cx + 18.0, 478.0, cw - 36.0, 92.0, 10.0, oklch(0.18, 0.01, 268.0), Some((hair, 1.0)), |p, k| p.on(6.0 * k).gloss(0.2, 0.5));
    let code = [
        ("let send = Prim::from_rune(rect, radii)", oklch(0.84, 0.07, 230.0)),
        ("    .emissive(ACCENT * glow);", oklch(0.80, 0.09, 300.0)),
        ("lights.render(&Scene { prims, gi: Some(Gi::default()), .. });", oklch(0.84, 0.07, 160.0)),
    ];
    for (i, (l, c)) in code.iter().enumerate() {
        ui.text(cx + 36.0, 492.0 + i as f32 * 22.0, l, 13.0, 400.0, *c);
    }

    // Composer with the recording dot and the send button.
    ui.rect(cx, 652.0, cw, 112.0, 20.0, card, Some((oklcha(1.0, 0.0, 0.0, 0.1), 1.0)), |p, k| p.raised(9.0 * k).bevel(7.0 * k, 2.5 * k).gloss(0.6, 0.38));
    ui.text(cx + 22.0, 672.0, "Ask anything…", 14.0, 400.0, txt3);
    let red = oklch(0.66, 0.22, 25.0);
    let rg = 6.0 * st.rec;
    ui.rect(cx + 22.0, 724.0, 14.0, 14.0, 7.0, red, None, |p, k| p.on(9.0 * k).emissive(scaled(red, rg)));
    let secs = 7 + st.t as u32;
    ui.text(cx + 44.0, 722.0, &format!("Recording  0:{secs:02}"), 12.0, 500.0, oklch(0.78, 0.09, 25.0));
    let accent = oklch(0.70, 0.19, 275.0);
    let sg = 3.4 * st.send;
    ui.rect(cx + cw - 58.0, 712.0, 38.0, 38.0, 19.0, accent, None, |p, k| p.on(9.0 * k).raised(3.0 * k).emissive(scaled(accent, sg)));
    let aw = ui.measure("↑", 17.0, 700.0);
    ui.text(cx + cw - 39.0 - aw * 0.5, 719.0, "↑", 17.0, 700.0, oklch(0.99, 0.01, 275.0));

    // Right panel
    let rx = 924.0;
    let rw = 332.0;
    ui.rect(rx, 150.0, rw, 170.0, 16.0, card, Some((hair, 1.0)), raise);
    ui.text(rx + 20.0, 166.0, "Agents", 14.0, 600.0, txt);
    let agents = [("cascades", "tracing · 5 levels"), ("glass", "refraction pass"), ("review", "Argon · no blockers")];
    for (i, (a, b)) in agents.iter().enumerate() {
        let y = 198.0 + i as f32 * 38.0;
        ui.rect(rx + 20.0, y + 4.0, 8.0, 8.0, 4.0, oklch(0.5, 0.02, 268.0), None, |p, k| p.on(6.0 * k));
        ui.text(rx + 38.0, y, a, 13.0, 500.0, txt);
        let bw = ui.measure(b, 12.0, 400.0);
        ui.text(rx + rw - 20.0 - bw, y + 1.0, b, 12.0, 400.0, txt2);
    }
    ui.rect(rx, 340.0, rw, 140.0, 16.0, card, Some((hair, 1.0)), raise);
    ui.text(rx + 20.0, 356.0, "Branch rc-reel", 14.0, 600.0, txt);
    ui.text(rx + 20.0, 380.0, "12 commits ahead · 3 files changed", 12.0, 400.0, txt2);
    let danger = oklch(0.42, 0.13, 25.0);
    let dg = 1.4 * st.danger_glow;
    ui.rect(rx + 20.0, 424.0, 150.0, 36.0, 10.0, danger, Some((oklcha(0.75, 0.14, 25.0, 0.5), 1.0)), |p, k| {
        let p = p.on(6.0 * k).raised(3.0 * k).bevel(5.0 * k, 1.5 * k).gloss(0.6, 0.35);
        if dg > 0.0 { p.emissive(scaled(oklch(0.66, 0.2, 25.0), dg)) } else { p }
    });
    ui.text(rx + 43.0, 433.0, "Delete branch", 13.0, 600.0, oklch(0.95, 0.03, 25.0));
    ui.rect(rx + 182.0, 424.0, 130.0, 36.0, 10.0, card2, Some((hair, 1.0)), chip);
    ui.text(rx + 225.0, 433.0, "Cancel", 13.0, 600.0, txt);
    ui.rect(rx, 500.0, rw, 264.0, 16.0, card, Some((hair, 1.0)), raise);
    ui.text(rx + 20.0, 516.0, "GPU per frame", 14.0, 600.0, txt);
    for i in 0..10 {
        let h = 30.0 + 90.0 * (0.5 + 0.5 * ((i as f32 * 0.9).sin() * 0.7 + (i as f32 * 0.37).cos() * 0.3));
        ui.rect(rx + 24.0 + i as f32 * 29.0, 720.0 - h, 16.0, h, 4.0, oklch(0.34, 0.03, 268.0), None, |p, k| p.on(6.0 * k).raised(2.0 * k));
    }
    ui.text(rx + 20.0, 734.0, "GI · composite · glass", 11.0, 500.0, txt3);

    // Toast: slides in from the right edge; its icon glows amber.
    let tx = rx + st.toast_x;
    ui.rect(tx, 76.0, rw, 56.0, 14.0, card2, Some((oklcha(1.0, 0.0, 0.0, 0.1), 1.0)), |p, k| p.floating(10.0 * k, 16.0 * k).bevel(6.0 * k, 2.0 * k).gloss(0.6, 0.38));
    let amber = oklch(0.80, 0.16, 70.0);
    let ag = 2.8 * st.toast_glow;
    ui.rect(tx + 16.0, 92.0, 24.0, 24.0, 12.0, amber, None, |p, k| p.on(16.0 * k).emissive(scaled(amber, ag)));
    let cw2 = ui.measure("✓", 14.0, 700.0);
    if cw2 > 0.0 {
        ui.text(tx + 28.0 - cw2 * 0.5, 94.0, "✓", 14.0, 700.0, oklch(0.25, 0.05, 70.0));
    }
    ui.text(tx + 54.0, 88.0, "Build passed", 13.0, 600.0, txt);
    ui.text(tx + 54.0, 107.0, "ci · 1m 12s", 12.0, 400.0, txt2);
}

// Window placement on the 1280×800 pt canvas: a caption band sits below it.
const UI_SCALE: f32 = 0.9;
const UI_O: [f32; 2] = [64.0, 18.0];

struct Built {
    base: Vec<Instance>,
    text: Vec<Instance>,
    prims: Vec<Prim>,
}

fn build_ui(ts: &mut TextSystem, st: &St) -> Built {
    let mut ui = Ui::new(ts, UI_SCALE, UI_O);
    build(&mut ui, st);
    // The electric border around the danger button may glow anywhere within this rect: tell the GI tracer.
    if st.arc > 0.0 {
        let r = ui.px(924.0 + 20.0 - 24.0, 424.0 - 24.0, 150.0 + 48.0, 36.0 + 48.0);
        ui.prims.push(Prim::rect(r[0], r[1], r[2], r[3]).radius(ui.d(30.0)).gi_hint());
    }
    Built { base: ui.base, text: ui.text, prims: ui.prims }
}

/// Device-px rect of a design-space rect.
fn dpx(x: f32, y: f32, w: f32, h: f32) -> [f32; 4] {
    [(UI_O[0] + x * UI_SCALE) * S, (UI_O[1] + y * UI_SCALE) * S, w * UI_SCALE * S, h * UI_SCALE * S]
}

/// The same rect in points on the canvas (rune-fx shapes are in points).
fn dpt(x: f32, y: f32, w: f32, h: f32) -> [f32; 4] {
    [UI_O[0] + x * UI_SCALE, UI_O[1] + y * UI_SCALE, w * UI_SCALE, h * UI_SCALE]
}

// ---------------------------------------------------------------- GPU context

struct Tex {
    tex: wgpu::Texture,
    view: wgpu::TextureView,
}

fn tex(device: &wgpu::Device, label: &str, w: u32, h: u32, mips: u32, format: wgpu::TextureFormat, usage: wgpu::TextureUsages) -> Tex {
    let tex = device.create_texture(&wgpu::TextureDescriptor {
        label: Some(label),
        size: wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
        mip_level_count: mips,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage,
        view_formats: &[],
    });
    let view = tex.create_view(&Default::default());
    Tex { tex, view }
}

struct Layer {
    t: Tex,
    mips: Vec<wgpu::TextureView>,
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct GU {
    size: [f32; 4],
    gi: [f32; 4],
    hp: [[f32; 4]; 2],
}

#[repr(C)]
#[derive(Clone, Copy, Default, bytemuck::Pod, bytemuck::Zeroable)]
struct VP {
    a: [f32; 4],
    b: [f32; 4],
    c: [f32; 4],
    q: [[f32; 4]; 4],
}

const MAX_VP: usize = 64;

struct Viz {
    g: wgpu::Buffer,
    vp: wgpu::Buffer,
    bgl: wgpu::BindGroupLayout,
    field: wgpu::RenderPipeline,
    scene: wgpu::RenderPipeline,
    cascade: wgpu::RenderPipeline,
    fluence: wgpu::RenderPipeline,
    mip: wgpu::RenderPipeline,
    layer: wgpu::RenderPipeline,
    accum: wgpu::RenderPipeline,
    mean_bgl: wgpu::BindGroupLayout,
    mean: wgpu::ComputePipeline,
    mean_u: Vec<wgpu::Buffer>,
    means_own: Vec<Tex>,
    means_mrg: Vec<Tex>,
    lin: wgpu::Sampler,
    aniso: wgpu::Sampler,
    dummy: Tex,
}

struct Enc {
    u: wgpu::Buffer,
    bgl: wgpu::BindGroupLayout,
    yuv: wgpu::ComputePipeline,
    rgba: wgpu::ComputePipeline,
    noise: Tex,
    y8: wgpu::Buffer,
    y10: wgpu::Buffer,
    rgba_buf: wgpu::Buffer,
    read8: wgpu::Buffer,
    read10: wgpu::Buffer,
    read_rgba: wgpu::Buffer,
}

struct Ctx {
    device: wgpu::Device,
    queue: wgpu::Queue,
    rune: Renderer,
    ts: TextSystem,
    light: LightRenderer,
    glass: GlassRenderer,
    fx: FxRenderer,
    viz: Viz,
    enc: Enc,
    base: Tex,
    text: Tex,
    emission: Tex,
    lit: Tex,
    backdrop: Tex,
    glassed: Tex,
    stack: Tex,
    overlay: Tex,
    accum: Tex,
    bloom: Bloom,
    layers: Vec<Layer>,
    timestamps: bool,
    query: Option<(wgpu::QuerySet, wgpu::Buffer, wgpu::Buffer)>,
    period: f32,
    noop: wgpu::ComputePipeline,
    gpu: String,
}

fn bgl_entry(binding: u32, vis: wgpu::ShaderStages, ty: wgpu::BindingType) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry { binding, visibility: vis, ty, count: None }
}

fn ftex() -> wgpu::BindingType {
    wgpu::BindingType::Texture { sample_type: wgpu::TextureSampleType::Float { filterable: true }, view_dimension: wgpu::TextureViewDimension::D2, multisampled: false }
}

fn ubuf() -> wgpu::BindingType {
    wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None }
}

fn sbuf(read_only: bool) -> wgpu::BindingType {
    wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Storage { read_only }, has_dynamic_offset: false, min_binding_size: None }
}

fn bind(device: &wgpu::Device, layout: &wgpu::BindGroupLayout, entries: &[wgpu::BindingResource<'_>]) -> wgpu::BindGroup {
    let e: Vec<wgpu::BindGroupEntry> = entries.iter().enumerate().map(|(i, r)| wgpu::BindGroupEntry { binding: i as u32, resource: r.clone() }).collect();
    device.create_bind_group(&wgpu::BindGroupDescriptor { label: None, layout, entries: &e })
}

fn tv(v: &wgpu::TextureView) -> wgpu::BindingResource<'_> {
    wgpu::BindingResource::TextureView(v)
}

fn wgsl_part(name: &str) -> String {
    let src = include_str!("reel.wgsl");
    let (viz, rest) = src.split_once("// ---- bloom ----").unwrap();
    let (bloom, rest) = rest.split_once("// ---- per-probe mean of a cascade level ----").unwrap();
    let (mean, enc) = rest.split_once("// ---- final encode ----").unwrap();
    match name {
        "viz" => viz.to_string(),
        "bloom" => bloom.to_string(),
        "mean" => mean.to_string(),
        _ => enc.to_string(),
    }
}

/// 64×64 blue noise, three independent channels (void and cluster, Ulichney 1993).
fn blue_noise() -> Vec<u8> {
    const N: usize = 64;
    let sigma = 1.9f32;
    let mut kernel = vec![0f32; N * N];
    for y in 0..N {
        for x in 0..N {
            let dx = (x.min(N - x)) as f32;
            let dy = (y.min(N - y)) as f32;
            kernel[y * N + x] = (-(dx * dx + dy * dy) / (2.0 * sigma * sigma)).exp();
        }
    }
    let mut out = vec![255u8; N * N * 4];
    let mut seed = 0x9e3779b9u32;
    let mut rnd = || {
        seed ^= seed << 13;
        seed ^= seed >> 17;
        seed ^= seed << 5;
        seed
    };
    for ch in 0..3 {
        let mut on = vec![false; N * N];
        let mut energy = vec![0f32; N * N];
        let toggle = |on: &mut Vec<bool>, energy: &mut Vec<f32>, i: usize, add: bool| {
            on[i] = add;
            let (ix, iy) = (i % N, i / N);
            let s = if add { 1.0 } else { -1.0 };
            for y in 0..N {
                for x in 0..N {
                    let kx = (x + N - ix) % N;
                    let ky = (y + N - iy) % N;
                    energy[y * N + x] += s * kernel[ky * N + kx];
                }
            }
        };
        let ones = N * N / 10;
        let mut placed = 0;
        while placed < ones {
            let i = (rnd() as usize) % (N * N);
            if !on[i] {
                toggle(&mut on, &mut energy, i, true);
                placed += 1;
            }
        }
        let tightest = |on: &Vec<bool>, energy: &Vec<f32>| (0..N * N).filter(|&i| on[i]).max_by(|&a, &b| energy[a].total_cmp(&energy[b])).unwrap();
        let voidest = |on: &Vec<bool>, energy: &Vec<f32>| (0..N * N).filter(|&i| !on[i]).min_by(|&a, &b| energy[a].total_cmp(&energy[b])).unwrap();
        loop {
            let c = tightest(&on, &energy);
            toggle(&mut on, &mut energy, c, false);
            let v = voidest(&on, &energy);
            toggle(&mut on, &mut energy, v, true);
            if v == c {
                break;
            }
        }
        let mut rank = vec![0usize; N * N];
        let (proto_on, proto_e) = (on.clone(), energy.clone());
        for r in (0..ones).rev() {
            let c = tightest(&on, &energy);
            toggle(&mut on, &mut energy, c, false);
            rank[c] = r;
        }
        on = proto_on;
        energy = proto_e;
        for r in ones..N * N {
            let v = voidest(&on, &energy);
            toggle(&mut on, &mut energy, v, true);
            rank[v] = r;
        }
        for i in 0..N * N {
            out[i * 4 + ch] = ((rank[i] as f32 + 0.5) / (N * N) as f32 * 255.0).round() as u8;
        }
    }
    out
}

impl Ctx {
    fn new() -> Ctx {
        let rune::render::Gpu { adapter, device, queue, .. } = rune::render::Gpu::headless();
        let ts_feat = device.features() & wgpu::Features::TIMESTAMP_QUERY;
        eprintln!("adapter: {:?}, timestamps: {}", adapter.get_info().name, !ts_feat.is_empty());
        use wgpu::TextureUsages as U;
        let rt = U::RENDER_ATTACHMENT | U::TEXTURE_BINDING;
        let full = |label: &str| tex(&device, label, W, H, 1, F16, rt | U::COPY_SRC | U::COPY_DST);
        let base = full("ui base");
        let text = full("ui text");
        let emission = full("emission");
        let lit = full("lit");
        let backdrop = full("backdrop");
        let glassed = full("glassed");
        let stack = full("stack");
        let overlay = full("overlay");
        let accum = full("accum");
        let bloom = Bloom::new(&device);
        let layers = (0..10)
            .map(|i| {
                let t = tex(&device, &format!("layer {i}"), W, H, MIPS, F16, rt | U::COPY_DST);
                let mips = (0..MIPS)
                    .map(|m| t.tex.create_view(&wgpu::TextureViewDescriptor { base_mip_level: m, mip_level_count: Some(1), ..Default::default() }))
                    .collect();
                Layer { t, mips }
            })
            .collect();
        let rune = Renderer::new(&device, F16);
        let light = LightRenderer::new(&device, &queue, W, H, F16);
        let glass = GlassRenderer::new(&device, W, H, F16);
        let fx = FxRenderer::new(&device, F16, FxOptions { dither: false });
        let viz = Self::viz(&device, light.inspect().gi_size);
        let enc = Self::enc(&device, &queue);
        let query = (!ts_feat.is_empty()).then(|| {
            let qs = device.create_query_set(&wgpu::QuerySetDescriptor { label: Some("reel ts"), ty: wgpu::QueryType::Timestamp, count: 4 });
            let resolve = device.create_buffer(&wgpu::BufferDescriptor { label: None, size: 32, usage: wgpu::BufferUsages::QUERY_RESOLVE | wgpu::BufferUsages::COPY_SRC, mapped_at_creation: false });
            let read = device.create_buffer(&wgpu::BufferDescriptor { label: None, size: 32, usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ, mapped_at_creation: false });
            (qs, resolve, read)
        });
        let period = queue.get_timestamp_period();
        let nm = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: None, source: wgpu::ShaderSource::Wgsl("@compute @workgroup_size(1) fn noop() {}".into()) });
        let noop = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("noop"),
            layout: None,
            module: &nm,
            entry_point: Some("noop"),
            compilation_options: Default::default(),
            cache: None,
        });
        Ctx {
            device,
            queue,
            rune,
            ts: TextSystem::default(),
            light,
            glass,
            fx,
            viz,
            enc,
            base,
            text,
            emission,
            lit,
            backdrop,
            glassed,
            stack,
            overlay,
            accum,
            bloom,
            layers,
            timestamps: !ts_feat.is_empty(),
            query,
            period,
            noop,
            gpu: adapter.get_info().name.trim_start_matches("Apple ").to_string(),
        }
    }

    /// An empty compute pass that writes a timestamp at its start or end (render-pass timestamps aren't ordered on
    /// tile GPUs, so render work is timed between two of these).
    fn marker(&self, enc: &mut wgpu::CommandEncoder, begin: Option<u32>, end: Option<u32>) {
        if let Some((qs, _, _)) = &self.query {
            let mut p = enc.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("ts marker"),
                timestamp_writes: Some(wgpu::ComputePassTimestampWrites { query_set: qs, beginning_of_pass_write_index: begin, end_of_pass_write_index: end }),
            });
            p.set_pipeline(&self.noop);
            p.dispatch_workgroups(1, 1, 1);
        }
    }

    fn viz(device: &wgpu::Device, gs: (u32, u32)) -> Viz {
        use wgpu::ShaderStages as Ss;
        let vf = Ss::VERTEX_FRAGMENT;
        let f = Ss::FRAGMENT;
        let bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("viz"),
            entries: &[
                bgl_entry(0, vf, ubuf()),
                bgl_entry(1, vf, sbuf(true)),
                bgl_entry(2, f, ftex()),
                bgl_entry(3, f, ftex()),
                bgl_entry(4, f, ftex()),
                bgl_entry(5, f, ftex()),
                bgl_entry(6, f, ftex()),
                bgl_entry(7, f, ftex()),
                bgl_entry(8, f, ftex()),
                bgl_entry(9, f, ftex()),
                bgl_entry(10, f, wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering)),
                bgl_entry(11, f, ftex()),
                bgl_entry(12, f, wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering)),
            ],
        });
        let m = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("reel viz"), source: wgpu::ShaderSource::Wgsl(wgsl_part("viz").into()) });
        let pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor { label: None, bind_group_layouts: &[Some(&bgl)], immediate_size: 0 });
        let pipe = |vs: &str, fs: &str, blend: Option<wgpu::BlendState>, strip: bool| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(fs),
                layout: Some(&pl),
                vertex: wgpu::VertexState { module: &m, entry_point: Some(vs), compilation_options: Default::default(), buffers: &[] },
                primitive: wgpu::PrimitiveState {
                    topology: if strip { wgpu::PrimitiveTopology::TriangleStrip } else { wgpu::PrimitiveTopology::TriangleList },
                    ..Default::default()
                },
                depth_stencil: None,
                multisample: Default::default(),
                fragment: Some(wgpu::FragmentState {
                    module: &m,
                    entry_point: Some(fs),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState { format: F16, blend, write_mask: wgpu::ColorWrites::ALL })],
                }),
                multiview_mask: None,
                cache: None,
            })
        };
        let field = pipe("vs_full", "fs_field", None, false);
        let scene = pipe("vs_full", "fs_scene", None, false);
        let cascade = pipe("vs_full", "fs_cascade", None, false);
        let fluence = pipe("vs_full", "fs_fluence", None, false);
        let mip = pipe("vs_full", "fs_mip", None, false);
        let layer = pipe("vs_layer", "fs_layer", Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING), true);
        let add = wgpu::BlendComponent { src_factor: wgpu::BlendFactor::One, dst_factor: wgpu::BlendFactor::One, operation: wgpu::BlendOperation::Add };
        let accum = pipe("vs_full", "fs_accum", Some(wgpu::BlendState { color: add, alpha: add }), false);
        let mean_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("mean"),
            entries: &[
                bgl_entry(0, Ss::COMPUTE, ubuf()),
                bgl_entry(1, Ss::COMPUTE, ftex()),
                bgl_entry(
                    2,
                    Ss::COMPUTE,
                    wgpu::BindingType::StorageTexture { access: wgpu::StorageTextureAccess::WriteOnly, format: F16, view_dimension: wgpu::TextureViewDimension::D2 },
                ),
            ],
        });
        let mm = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("reel mean"), source: wgpu::ShaderSource::Wgsl(wgsl_part("mean").into()) });
        let mpl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor { label: None, bind_group_layouts: &[Some(&mean_bgl)], immediate_size: 0 });
        let mean = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("mean"),
            layout: Some(&mpl),
            module: &mm,
            entry_point: Some("cs_mean"),
            compilation_options: Default::default(),
            cache: None,
        });
        let mean_u = (0..CASCADES)
            .map(|n| {
                use wgpu::util::DeviceExt;
                device.create_buffer_init(&wgpu::util::BufferInitDescriptor { label: None, contents: bytemuck::cast_slice(&[n, 0, 0, 0]), usage: wgpu::BufferUsages::UNIFORM })
            })
            .collect();
        use wgpu::TextureUsages as U;
        let mk = |n: u32| tex(device, "mean", gs.0 >> n, gs.1 >> n, 1, F16, U::STORAGE_BINDING | U::TEXTURE_BINDING);
        let means_own = (0..CASCADES).map(mk).collect();
        let means_mrg = (0..CASCADES).map(mk).collect();
        let lin = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("viz lin"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let aniso = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("viz aniso"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Linear,
            anisotropy_clamp: 16,
            ..Default::default()
        });
        let g = device.create_buffer(&wgpu::BufferDescriptor { label: Some("viz g"), size: std::mem::size_of::<GU>() as u64, usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false });
        let vp = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("viz vp"),
            size: (MAX_VP * std::mem::size_of::<VP>()) as u64,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let dummy = tex(device, "dummy", 1, 1, 1, F16, U::TEXTURE_BINDING);
        Viz { g, vp, bgl, field, scene, cascade, fluence, mip, layer, accum, mean_bgl, mean, mean_u, means_own, means_mrg, lin, aniso, dummy }
    }

    fn enc(device: &wgpu::Device, queue: &wgpu::Queue) -> Enc {
        use wgpu::ShaderStages as Ss;
        let c = Ss::COMPUTE;
        let bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("enc"),
            entries: &[
                bgl_entry(0, c, ubuf()),
                bgl_entry(1, c, ftex()),
                bgl_entry(2, c, ftex()),
                bgl_entry(3, c, ftex()),
                bgl_entry(4, c, sbuf(false)),
                bgl_entry(5, c, sbuf(false)),
                bgl_entry(6, c, sbuf(false)),
                bgl_entry(7, c, ftex()),
                bgl_entry(8, c, wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering)),
            ],
        });
        let m = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("reel enc"), source: wgpu::ShaderSource::Wgsl(wgsl_part("enc").into()) });
        let pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor { label: None, bind_group_layouts: &[Some(&bgl)], immediate_size: 0 });
        let cp = |e: &str| {
            device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor { label: Some(e), layout: Some(&pl), module: &m, entry_point: Some(e), compilation_options: Default::default(), cache: None })
        };
        let yuv = cp("cs_yuv");
        let rgba = cp("cs_rgba");
        let noise = tex(device, "blue noise", 64, 64, 1, wgpu::TextureFormat::Rgba8Unorm, wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST);
        queue.write_texture(
            wgpu::TexelCopyTextureInfo { texture: &noise.tex, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
            &blue_noise(),
            wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(256), rows_per_image: Some(64) },
            wgpu::Extent3d { width: 64, height: 64, depth_or_array_layers: 1 },
        );
        let n = (W * H) as u64;
        let buf = |label: &str, size: u64, usage: wgpu::BufferUsages| device.create_buffer(&wgpu::BufferDescriptor { label: Some(label), size, usage, mapped_at_creation: false });
        use wgpu::BufferUsages as B;
        let y8 = buf("y8", n * 3 / 2, B::STORAGE | B::COPY_SRC);
        let y10 = buf("y10", n * 3, B::STORAGE | B::COPY_SRC);
        let rgba_buf = buf("rgba", n * 4, B::STORAGE | B::COPY_SRC);
        let read8 = buf("read8", n * 3 / 2, B::COPY_DST | B::MAP_READ);
        let read10 = buf("read10", n * 3, B::COPY_DST | B::MAP_READ);
        let read_rgba = buf("read rgba", n * 4, B::COPY_DST | B::MAP_READ);
        let u = buf("enc u", 32, B::UNIFORM | B::COPY_DST);
        Enc { u, bgl, yuv, rgba, noise, y8, y10, rgba_buf, read8, read10, read_rgba }
    }

    fn draw_rune(&mut self, insts: &[Instance], target: &wgpu::TextureView, clear: [f32; 4]) {
        self.rune.prepare(&self.device, &self.queue, &mut self.ts.atlas, insts, (W, H));
        let mut enc = self.device.create_command_encoder(&Default::default());
        self.rune.draw(&mut enc, target, clear);
        self.queue.submit([enc.finish()]);
    }

    fn pass<'a>(enc: &'a mut wgpu::CommandEncoder, view: &'a wgpu::TextureView, load: Option<[f32; 4]>) -> wgpu::RenderPass<'a> {
        let ops = match load {
            Some(c) => wgpu::LoadOp::Clear(wgpu::Color { r: c[0] as f64, g: c[1] as f64, b: c[2] as f64, a: c[3] as f64 }),
            None => wgpu::LoadOp::Load,
        };
        enc.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: None,
            color_attachments: &[Some(wgpu::RenderPassColorAttachment { view, depth_slice: None, resolve_target: None, ops: wgpu::Operations { load: ops, store: wgpu::StoreOp::Store } })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        })
    }

    fn copy(&self, enc: &mut wgpu::CommandEncoder, src: &wgpu::Texture, dst: &wgpu::Texture) {
        enc.copy_texture_to_texture(
            wgpu::TexelCopyTextureInfo { texture: src, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
            wgpu::TexelCopyTextureInfo { texture: dst, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
            wgpu::Extent3d { width: W, height: H, depth_or_array_layers: 1 },
        );
    }

    /// Viz bind group with the GI buffers of `level` (or level 0) and `layer` as the sampled layer texture.
    fn viz_bg(&self, level: usize, layer: Option<&wgpu::TextureView>) -> wgpu::BindGroup {
        let ins = self.light.inspect();
        let v = &self.viz;
        bind(
            &self.device,
            &v.bgl,
            &[
                v.g.as_entire_binding(),
                v.vp.as_entire_binding(),
                tv(ins.field),
                tv(ins.emission),
                tv(ins.occluders),
                tv(&ins.own[level]),
                tv(&ins.merged[level]),
                tv(&v.means_own[level].view),
                tv(&v.means_mrg[level].view),
                tv(ins.fluence),
                wgpu::BindingResource::Sampler(&v.lin),
                tv(layer.unwrap_or(&v.dummy.view)),
                wgpu::BindingResource::Sampler(&v.aniso),
            ],
        )
    }

    fn means(&self, enc: &mut wgpu::CommandEncoder) {
        let ins = self.light.inspect();
        let mut pass = enc.begin_compute_pass(&wgpu::ComputePassDescriptor { label: Some("means"), timestamp_writes: None });
        pass.set_pipeline(&self.viz.mean);
        for n in 0..CASCADES as usize {
            for (src, dst) in [(&ins.own[n], &self.viz.means_own[n]), (&ins.merged[n], &self.viz.means_mrg[n])] {
                let bg = bind(&self.device, &self.viz.mean_bgl, &[self.viz.mean_u[n].as_entire_binding(), tv(src), tv(&dst.view)]);
                pass.set_bind_group(0, &bg, &[]);
                pass.dispatch_workgroups((ins.gi_size.0 >> n).div_ceil(8), (ins.gi_size.1 >> n).div_ceil(8), 1);
            }
        }
    }

    fn build_mips(&self, enc: &mut wgpu::CommandEncoder, layer: usize) {
        for m in 1..MIPS as usize {
            let bg = self.viz_bg(0, Some(&self.layers[layer].mips[m - 1]));
            let mut pass = Self::pass(enc, &self.layers[layer].mips[m], Some([0.0; 4]));
            pass.set_pipeline(&self.viz.mip);
            pass.set_bind_group(0, &bg, &[]);
            pass.draw(0..3, 0..1);
        }
    }

    /// Encode the final image (+ overlay) to yuv420p (8-bit) and yuv420p10le, and optionally RGBA8.
    fn encode(&mut self, frame: &wgpu::TextureView, rgba: bool, yuv: bool) -> (Option<Vec<u8>>, Option<Vec<u8>>, Option<Vec<u8>>) {
        let e = &self.enc;
        let bloom_mix = 0.045;
        self.queue.write_buffer(&e.u, 0, bytemuck::cast_slice(&[W as f32, H as f32, 1.0, 1.0, bloom_mix, 1.0 / BLOOM_LEVELS as f32, 0.0, 0.0]));
        let bg = bind(
            &self.device,
            &e.bgl,
            &[
                e.u.as_entire_binding(),
                tv(frame),
                tv(&self.overlay.view),
                tv(&e.noise.view),
                e.y8.as_entire_binding(),
                e.y10.as_entire_binding(),
                e.rgba_buf.as_entire_binding(),
                tv(&self.bloom.levels[0]),
                wgpu::BindingResource::Sampler(&self.bloom.sampler),
            ],
        );
        let mut enc = self.device.create_command_encoder(&Default::default());
        {
            let mut pass = enc.begin_compute_pass(&wgpu::ComputePassDescriptor { label: Some("encode"), timestamp_writes: None });
            pass.set_bind_group(0, &bg, &[]);
            if yuv {
                pass.set_pipeline(&e.yuv);
                pass.dispatch_workgroups((W / 8).div_ceil(8), (H / 2).div_ceil(8), 1);
            }
            if rgba {
                pass.set_pipeline(&e.rgba);
                pass.dispatch_workgroups(W.div_ceil(8), H.div_ceil(8), 1);
            }
        }
        let n = (W * H) as u64;
        if yuv {
            enc.copy_buffer_to_buffer(&e.y8, 0, &e.read8, 0, n * 3 / 2);
            enc.copy_buffer_to_buffer(&e.y10, 0, &e.read10, 0, n * 3);
        }
        if rgba {
            enc.copy_buffer_to_buffer(&e.rgba_buf, 0, &e.read_rgba, 0, n * 4);
        }
        self.queue.submit([enc.finish()]);
        let read = |b: &wgpu::Buffer| {
            let s = b.slice(..);
            s.map_async(wgpu::MapMode::Read, |r| r.expect("map"));
            self.device.poll(wgpu::PollType::wait_indefinitely()).expect("poll");
            let v = s.get_mapped_range().expect("range").to_vec();
            b.unmap();
            v
        };
        let (a, b) = if yuv { (Some(read(&e.read8)), Some(read(&e.read10))) } else { (None, None) };
        let c = if rgba { Some(read(&e.read_rgba)) } else { None };
        (a, b, c)
    }
}

const BLOOM_LEVELS: u32 = 7;

struct Bloom {
    levels: Vec<wgpu::TextureView>,
    bgl: wgpu::BindGroupLayout,
    down: wgpu::RenderPipeline,
    up: wgpu::RenderPipeline,
    karis: wgpu::Buffer,
    plain: wgpu::Buffer,
    sampler: wgpu::Sampler,
}

impl Bloom {
    fn new(device: &wgpu::Device) -> Bloom {
        use wgpu::util::DeviceExt;
        let f = wgpu::ShaderStages::FRAGMENT;
        let bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("bloom"),
            entries: &[bgl_entry(0, f, ubuf()), bgl_entry(1, f, ftex()), bgl_entry(2, f, wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering))],
        });
        let m = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("bloom"), source: wgpu::ShaderSource::Wgsl(wgsl_part("bloom").into()) });
        let pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor { label: None, bind_group_layouts: &[Some(&bgl)], immediate_size: 0 });
        let pipe = |fs: &str, blend: Option<wgpu::BlendState>| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(fs),
                layout: Some(&pl),
                vertex: wgpu::VertexState { module: &m, entry_point: Some("vs_b"), compilation_options: Default::default(), buffers: &[] },
                primitive: wgpu::PrimitiveState::default(),
                depth_stencil: None,
                multisample: Default::default(),
                fragment: Some(wgpu::FragmentState {
                    module: &m,
                    entry_point: Some(fs),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState { format: F16, blend, write_mask: wgpu::ColorWrites::ALL })],
                }),
                multiview_mask: None,
                cache: None,
            })
        };
        let add = wgpu::BlendComponent { src_factor: wgpu::BlendFactor::One, dst_factor: wgpu::BlendFactor::One, operation: wgpu::BlendOperation::Add };
        let down = pipe("fs_bdown", None);
        let up = pipe("fs_bup", Some(wgpu::BlendState { color: add, alpha: add }));
        let t = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("bloom"),
            size: wgpu::Extent3d { width: W / 2, height: H / 2, depth_or_array_layers: 1 },
            mip_level_count: BLOOM_LEVELS,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: F16,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let levels = (0..BLOOM_LEVELS)
            .map(|k| t.create_view(&wgpu::TextureViewDescriptor { base_mip_level: k, mip_level_count: Some(1), ..Default::default() }))
            .collect();
        let karis = device.create_buffer_init(&wgpu::util::BufferInitDescriptor { label: None, contents: bytemuck::cast_slice(&[1.0f32, 1.0, 0.0, 0.0]), usage: wgpu::BufferUsages::UNIFORM });
        let plain = device.create_buffer_init(&wgpu::util::BufferInitDescriptor { label: None, contents: bytemuck::cast_slice(&[0.0f32, 1.0, 0.0, 0.0]), usage: wgpu::BufferUsages::UNIFORM });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("bloom"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            ..Default::default()
        });
        Bloom { levels, bgl, down, up, karis, plain, sampler }
    }
}

impl Ctx {
    fn bloom(&self, src: &wgpu::TextureView) {
        let b = &self.bloom;
        let mut enc = self.device.create_command_encoder(&Default::default());
        let bg = |u: &wgpu::Buffer, v: &wgpu::TextureView| bind(&self.device, &b.bgl, &[u.as_entire_binding(), tv(v), wgpu::BindingResource::Sampler(&b.sampler)]);
        for k in 0..BLOOM_LEVELS as usize {
            let g = if k == 0 { bg(&b.karis, src) } else { bg(&b.plain, &b.levels[k - 1]) };
            let mut pass = Self::pass(&mut enc, &b.levels[k], Some([0.0; 4]));
            pass.set_pipeline(&b.down);
            pass.set_bind_group(0, &g, &[]);
            pass.draw(0..3, 0..1);
        }
        for k in (1..BLOOM_LEVELS as usize).rev() {
            let g = bg(&b.plain, &b.levels[k]);
            let mut pass = Self::pass(&mut enc, &b.levels[k - 1], None);
            pass.set_pipeline(&b.up);
            pass.set_bind_group(0, &g, &[]);
            pass.draw(0..3, 0..1);
        }
        self.queue.submit([enc.finish()]);
    }
}

// ---------------------------------------------------------------- the camera for the 2.5D stack

#[derive(Clone, Copy)]
struct Cam {
    pitch: f32,
    yaw: f32,
    scale: f32,
    ty: f32,
    tx: f32,
}

const FOV_D: f32 = 3200.0; // camera distance in px: pixel-exact at rest

fn project(c: &Cam, x: f32, y: f32, z: f32) -> [f32; 4] {
    // Layer plane centred on the frame, z up toward the viewer; yaw spins it in its plane, pitch tips its top away.
    let (x, y) = ((x - W as f32 * 0.5) * c.scale, (y - H as f32 * 0.5) * c.scale);
    let z = z * c.scale;
    let (sy, cy) = c.yaw.sin_cos();
    let (x, y) = (x * cy - y * sy, x * sy + y * cy);
    let (sp_, cp) = c.pitch.sin_cos();
    let cz = -z;
    let y2 = y * cp + cz * sp_;
    let z2 = -y * sp_ + cz * cp;
    let x2 = x + c.tx;
    let y2 = y2 + c.ty;
    let w = z2 + FOV_D;
    [x2 * FOV_D / (W as f32 * 0.5), -y2 * FOV_D / (H as f32 * 0.5), 0.5 * w, w]
}

fn to_screen(c: [f32; 4]) -> [f32; 2] {
    [(c[0] / c[3] + 1.0) * 0.5 * W as f32, (1.0 - c[1] / c[3]) * 0.5 * H as f32]
}

// ---------------------------------------------------------------- frame

/// Captions and labels drawn in screen space over everything.
struct Overlay {
    items: Vec<(f32, f32, String, f32, f32, Color)>,
    rects: Vec<Instance>,
}

impl Overlay {
    fn text(&mut self, x: f32, y: f32, s: &str, size: f32, weight: f32, c: Color) {
        self.items.push((x, y, s.to_string(), size, weight, c));
    }
}

#[derive(Default, Clone, Copy)]
struct Bench {
    timings: Option<Timings>,
    glass_ms: f64,
}

struct BenchNumbers {
    gpu: String,
    gi: f64,
    field: f64,
    scene: f64,
    cascades: [f64; 5],
    fluence: f64,
    glass: f64,
    frame: f64,
}

fn load_bench(out: &Path) -> BenchNumbers {
    let mut b = BenchNumbers { gpu: "M5 Max".into(), gi: 2.3, field: 0.07, scene: 0.05, cascades: [0.4; 5], fluence: 0.01, glass: 0.5, frame: 4.0 };
    if let Ok(s) = std::fs::read_to_string(out.join("bench.json")) {
        let num = |k: &str| -> Option<f64> {
            let i = s.find(&format!("\"{k}\""))?;
            let rest = &s[i + k.len() + 3..];
            let rest = rest.trim_start_matches([':', ' ']);
            let end = rest.find([',', '}', ']', '\n']).unwrap_or(rest.len());
            rest[..end].trim().parse().ok()
        };
        b.gi = num("gi").unwrap_or(b.gi);
        b.field = num("field").unwrap_or(b.field);
        b.scene = num("gi_scene").unwrap_or(b.scene);
        b.fluence = num("fluence").unwrap_or(b.fluence);
        b.glass = num("glass").unwrap_or(b.glass);
        b.frame = num("frame").unwrap_or(b.frame);
        for n in 0..5 {
            b.cascades[n] = num(&format!("c{n}")).unwrap_or(b.cascades[n]);
        }
        if let Some(i) = s.find("\"gpu\": \"") {
            let rest = &s[i + 8..];
            if let Some(e) = rest.find('"') {
                b.gpu = rest[..e].to_string();
            }
        }
    }
    b
}

struct Reel {
    bench: BenchNumbers,
    hp: [[f32; 2]; 2],
}

fn glow_settings(t: f32) -> Settings {
    Settings {
        ambient: [0.2, 0.205, 0.235],
        exposure: 1.0,
        gi: Some(Gi { strength: 1.15, air: 0.10, bounce: 0.0, absorption: 0.22, emission: 0.9 }),
        text: TextPolicy { floor: 0.84, ceiling: 1.3, lit: 1.0, specular: 0.1 },
        albedo_floor: 0.035,
        contact_ao: 0.35,
        rim: 0.5,
        background_gloss: 0.35,
        time: t,
        ..Default::default()
    }
}

/// Glass shape and look at time t (None when off screen).
fn glass_at(t: f32) -> Option<Glass> {
    if !(T_GLASS - 0.1..T_ARC + 0.4).contains(&t) {
        return None;
    }
    let x = keys(-520.0, &[(T_GLASS, 300.0, sp(1.3, 0.08)), (T_GLASS + 3.4, 330.0, sp(1.0, 0.0)), (T_ARC - 1.3, 1500.0, sp(1.4, 0.0))]).at(t);
    let y = keys(300.0, &[(T_GLASS + 3.4, 360.0, sp(1.2, 0.0))]).at(t);
    let presence = rise(t, T_GLASS, 0.9);
    let (w, h, r) = (380.0, 240.0, 56.0);
    let a = dpx(x, y, w, h);
    // A drop of glass buds off the card toward the send button and is pulled back.
    let bud = window(t, T_GLASS + 1.7, T_GLASS + 3.9, 0.9);
    let bx = x + w - 40.0 + bud * 210.0;
    let by = y + h - 50.0 + bud * 110.0;
    let br = 46.0 + 14.0 * bud;
    let b = dpx(bx, by, br * 2.0, br * 2.0);
    let frost = 0.85 * window(t, T_GLASS + 4.3, T_GLASS + 6.1, 0.7);
    let k = UI_SCALE * S;
    Some(
        Glass::rounded(a[0], a[1], a[2], a[3], r * k)
            .thickness(34.0 * k)
            .bevel(24.0 * k)
            .gap(12.0 * k)
            .dome(10.0 * k)
            .ior(1.5)
            .dispersion(0.045)
            .frost(frost)
            .presence(presence)
            .rim_glow(0.45)
            .union(b, br * k, 60.0 * k, 0.0),
    )
}

/// Sub-frames for the shutter: enough that the fastest-moving element steps at most ~2 px between them.
fn subframes(t: f32) -> u32 {
    let h = 1.0 / 480.0;
    let toast = |t: f32| state(t).toast_x * UI_SCALE * S;
    let gx = |t: f32| glass_at(t).map_or(0.0, |g| g.a[0] + g.a[1] + g.b[0] + g.b[1]);
    let v = ((toast(t + h) - toast(t - h)).abs()).max((gx(t + h) - gx(t - h)).abs()) / (2.0 * h);
    let travel = v * 0.5 / FPS as f32;
    ((travel / 2.0).ceil() as u32).clamp(1, 24)
}

impl Reel {
    /// One frame: the lit image (shutter-integrated while things move fast), the breakdown, bloom and captions.
    fn frame(&self, ctx: &mut Ctx, t: f32, bench: bool) -> (wgpu::TextureView, Bench) {
        let n = if bench { 1 } else { subframes(t) };
        let (mut frame_tex, mut frame_view, b) = if n <= 1 {
            self.image(ctx, t, bench)
        } else {
            // 180° shutter: n sub-frames across [t − 1/240, t + 1/240], weighted by a raised cosine so trails have soft
            // ends (as in rune-fx's motion blur).
            let shutter = 0.5 / FPS as f32;
            let ws: Vec<f32> = (0..n).map(|k| 0.5 - 0.5 * (std::f32::consts::TAU * (k as f32 + 0.5) / n as f32).cos()).collect();
            let wsum: f32 = ws.iter().sum();
            let mut last = Bench::default();
            for k in 0..n {
                let tk = t + shutter * ((k as f32 + 0.5) / n as f32 - 0.5);
                let (_, view, b) = self.image(ctx, tk, false);
                last = b;
                let mut vps = [VP::default(); 1];
                vps[0].a = [ws[k as usize] / wsum, 0.0, 0.0, 0.0];
                ctx.queue.write_buffer(&ctx.viz.vp, (60 * std::mem::size_of::<VP>()) as u64, bytemuck::cast_slice(&vps));
                let bg = ctx.viz_bg(0, Some(&view));
                let mut enc = ctx.device.create_command_encoder(&Default::default());
                {
                    let mut pass = Ctx::pass(&mut enc, &ctx.accum.view, if k == 0 { Some([0.0; 4]) } else { None });
                    pass.set_pipeline(&ctx.viz.accum);
                    pass.set_bind_group(0, &bg, &[]);
                    pass.draw(0..3, 60..61);
                }
                ctx.queue.submit([enc.finish()]);
            }
            (ctx.accum.tex.clone(), ctx.accum.view.clone(), last)
        };

        // Breakdown: the 2.5D stack of real buffers.
        let mut ov = Overlay { items: Vec::new(), rects: Vec::new() };
        if (T_TILT..T_UNTILT + 1.4).contains(&t) {
            frame_view = self.breakdown(ctx, t, &frame_tex, &mut ov);
            frame_tex = ctx.stack.tex.clone();
        }
        let _ = frame_tex;
        ctx.bloom(&frame_view);
        self.captions(t, &mut ov);
        // Overlay text.
        let mut insts = ov.rects.clone();
        for (x, y, s, size, weight, c) in &ov.items {
            let style = TextStyle::new(*size, *weight);
            let mut quads = Vec::new();
            ctx.ts.glyphs(s, style, S, [(x * S).round(), (y * S).round()], &mut quads);
            for q in quads {
                insts.push(Instance {
                    rect: q.rect,
                    radii: [0.0; 4],
                    fill: c.premul(1.0),
                    border: [0.0; 4],
                    params: [0.0, KIND_GLYPH, 0.0, 1.0],
                    uv: [q.uv[0], q.uv[1], 0.0, 0.0],
                    clip: NO_CLIP,
                    fade: [0; 4],
                });
            }
        }
        ctx.draw_rune(&insts, &ctx.overlay.view.clone(), [0.0; 4]);
        (frame_view, b)
    }

    /// The lit UI with effects and glass at exactly time t.
    #[allow(clippy::too_many_lines)]
    fn image(&self, ctx: &mut Ctx, t: f32, bench: bool) -> (wgpu::Texture, wgpu::TextureView, Bench) {
        let st = state(t);
        let built = build_ui(&mut ctx.ts, &st);
        let bgc = oklch(BG.0, BG.1, BG.2);
        ctx.draw_rune(&built.base, &ctx.base.view.clone(), [bgc.r, bgc.g, bgc.b, 1.0]);
        ctx.draw_rune(&built.text, &ctx.text.view.clone(), [0.0; 4]);

        // Effects: the electric border on the danger button. Its light is rendered once into the emission texture
        // (which the GI reads) and again over the lit frame.
        let mut fx_layers: Vec<FxLayer> = Vec::new();
        if st.arc > 0.0 {
            let b = dpt(924.0 + 20.0, 424.0, 150.0, 36.0);
            let e = ElectricParams {
                style: ElectricStyle::Border,
                color: rgb(0xff5a4a),
                core: rgb(0xfff1e8),
                cool: rgb(0x9b4dff),
                amplitude: 3.2,
                frequency: 2.6,
                speed: 3.0,
                glow: 5.5,
                forks: 0.5,
                flicker: 0.35,
                intensity: 1.15 * st.arc,
                gap: 3.0,
                ..ElectricParams::default()
            };
            fx_layers.push(Fx::electric(e).on(Shape::rounded(b[0], b[1], b[2], b[3], 10.0 * UI_SCALE)).seed(7).at(t).over());
        }
        let click = 3.3;
        let shock = (click..click + 1.2).contains(&t);
        let mut ts_glass = 0.0;
        let mut enc = ctx.device.create_command_encoder(&Default::default());
        if !fx_layers.is_empty() {
            ctx.fx.prepare(&ctx.device, &ctx.queue, &FxFrame { viewport: [W, H], scale: S, layers: &fx_layers, backdrop: None, sources: &[] });
            let mut pass = Ctx::pass(&mut enc, &ctx.emission.view, Some([0.0; 4]));
            ctx.fx.draw_all(&mut pass, &fx_layers, None);
        }
        {
            let scene = Scene {
                prims: &built.prims,
                lights: &[],
                settings: glow_settings(t),
                emission: (!fx_layers.is_empty()).then_some(&ctx.emission.view),
            };
            ctx.light.inspect = (T_TILT - 0.1..T_UNTILT + 1.5).contains(&t) || bench;
            ctx.light.render(&ctx.device, &ctx.queue, &mut enc, &scene, &ctx.base.view, &ctx.text.view, &ctx.lit.view);
        }
        ctx.queue.submit([enc.finish()]);
        let timings = if bench { ctx.light.read_timings(&ctx.device) } else { None };
        let mut enc = ctx.device.create_command_encoder(&Default::default());
        if !fx_layers.is_empty() {
            let mut pass = Ctx::pass(&mut enc, &ctx.lit.view, None);
            ctx.fx.draw_all(&mut pass, &fx_layers, None);
        }
        if shock {
            // The click: a shockwave ripple out of the send button.
            let sb = dpt(272.0 + 628.0 - 58.0, 712.0, 38.0, 38.0);
            let at = [sb[0] + sb[2] * 0.5, sb[1] + sb[3] * 0.5];
            let mut d = DistortParams::shockwave(at, click);
            if let DistortParams::Shockwave { reach, width, amount, chroma, duration, .. } = &mut d {
                *reach = 260.0;
                *width = 16.0;
                *amount = 9.0;
                *chroma = 0.35;
                *duration = 1.1;
            }
            let layers = [Fx::distort(d).on(Shape::rounded(at[0] - 280.0, at[1] - 280.0, 560.0, 560.0, 0.0)).at(t)];
            ctx.copy(&mut enc, &ctx.lit.tex, &ctx.backdrop.tex);
            ctx.queue.submit([enc.finish()]);
            enc = ctx.device.create_command_encoder(&Default::default());
            ctx.fx.prepare(&ctx.device, &ctx.queue, &FxFrame { viewport: [W, H], scale: S, layers: &layers, backdrop: Some(&ctx.backdrop.view), sources: &[] });
            let mut pass = Ctx::pass(&mut enc, &ctx.lit.view, None);
            ctx.fx.draw_all(&mut pass, &layers, None);
        }
        let mut frame_view = ctx.lit.view.clone();
        let mut frame_tex = ctx.lit.tex.clone();
        if let Some(g) = glass_at(t).or(if bench { glass_at(T_GLASS + 1.0) } else { None }) {
            let lights: Vec<GlassLight> = built.prims.iter().filter_map(|p| GlassLight::from_emitter(p, 18.0 * S, 0.9)).collect();
            ctx.marker(&mut enc, None, Some(0));
            let settings = GlassSettings::default();
            ctx.glass.render(&ctx.device, &ctx.queue, &mut enc, &[g], &lights, settings, &ctx.lit.view, ctx.light.inspect().fluence, &ctx.glassed.view);
            ctx.marker(&mut enc, Some(1), None);
            if let Some((qs, resolve, read)) = &ctx.query {
                enc.resolve_query_set(qs, 0..2, resolve, 0);
                enc.copy_buffer_to_buffer(resolve, 0, read, 0, 16);
            }
            frame_view = ctx.glassed.view.clone();
            frame_tex = ctx.glassed.tex.clone();
            ts_glass = 1.0;
        }
        ctx.queue.submit([enc.finish()]);
        let mut glass_ms = 0.0;
        if bench && ts_glass > 0.0 {
            if let Some((_, _, read)) = &ctx.query {
                let s = read.slice(..);
                s.map_async(wgpu::MapMode::Read, |r| r.unwrap());
                ctx.device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
                let ticks: Vec<u64> = bytemuck::cast_slice(&s.get_mapped_range().unwrap()).to_vec();
                read.unmap();
                glass_ms = ticks[1].saturating_sub(ticks[0]) as f64 * ctx.period as f64 / 1e6;
            }
        }

        (frame_tex, frame_view, Bench { timings, glass_ms })
    }

    fn captions(&self, t: f32, ov: &mut Overlay) {
        let b = &self.bench;
        let caps: Vec<(f32, f32, String, String)> = vec![
            (1.0, T_TILT - 0.3, "Lit only by its own glowing elements".into(), "2D radiance cascades · every emitter lights its neighbours, with soft coloured shadow".into()),
            (T_TILT + 0.2, T_INPUTS - 0.1, "How it's lit".into(), "rune-light's real GI buffers, frame by frame".into()),
            (
                T_INPUTS,
                T_CASCADES - 0.1,
                "① The scene as a distance field".into(),
                format!("Rune draws analytic rounded rects, so the SDF is exact (no jump flood) · ½ res · {:.2} ms", b.field),
            ),
            (
                T_CASCADES,
                T_MERGE - 0.1,
                "② Five cascades of probes".into(),
                "each level: probes 2× sparser, rays 4× longer, 4× the directions · same memory per level".into(),
            ),
            (
                T_MERGE,
                T_FLUENCE - 0.1,
                "③ Merge, top cascade down".into(),
                "each ray continues into the four nearest upper probes (Sannikov's bilinear fix: no leaks between probes)".into(),
            ),
            (T_FLUENCE, T_UNTILT - 0.1, "④ Fluence".into(), "cascade 0's directions averaged: the light arriving at every point".into()),
            (
                T_UNTILT,
                T_GLASS - 0.2,
                "⑤ Back onto the UI".into(),
                format!("whole GI pass {:.1} ms on {} at 2560×1600 · cost fixed by the grid, not the number of lights", b.gi, b.gpu),
            ),
            (
                T_GLASS + 0.3,
                T_ARC - 0.4,
                "Thick glass, refracted per pixel".into(),
                format!("Snell n = 1.50 · exact Fresnel · 8-wavelength dispersion · total internal reflection at the bevel · frost · {:.2} ms", b.glass),
            ),
            (T_ARC + 0.2, T_END - 0.3, "Effects are light too".into(), "the electric arc is emission: it enters the cascades and lights the cards around it".into()),
        ];
        for (a, z, title, sub) in caps {
            let k = window(t, a, z, 0.35);
            if k <= 0.01 {
                continue;
            }
            let y = 754.0 + (1.0 - k) * 6.0;
            let x = UI_O[0] + 4.0;
            ov.text(x, y, &title, 15.0, 600.0, oklcha(0.95, 0.005, 268.0, k));
            ov.text(x, y + 22.0, &sub, 12.5, 400.0, oklcha(0.68, 0.012, 268.0, k));
        }
        let k = rise(t, T_END + 0.3, 0.8);
        if k > 0.01 {
            let x = 640.0 - 150.0;
            ov.text(x, 760.0 + (1.0 - k) * 6.0, "Rune  ·  radiance cascades  ·  120 Hz", 16.0, 600.0, oklcha(0.95, 0.005, 268.0, k));
        }
    }

    /// The breakdown: renders each real buffer into a layer texture and composes the 2.5D stack.
    fn breakdown(&self, ctx: &mut Ctx, t: f32, lit: &wgpu::Texture, ov: &mut Overlay) -> wgpu::TextureView {
        let ins = ctx.light.inspect();
        let gs = ins.gi_size;
        let texel = ins.texel;
        let tilt = rise(t, T_TILT, 1.3) * (1.0 - rise(t, T_UNTILT, 1.2));
        let cam = Cam { pitch: 0.84 * tilt, yaw: -0.28 * tilt, scale: lerp(1.0, 0.6, tilt), ty: -20.0 * tilt, tx: -190.0 * tilt };
        // Per layer: (texture slot, z, alpha, kind)
        let inputs = window(t, T_INPUTS - 0.2, T_CASCADES + 0.3, 0.6);
        let ui_lift = rise(t, T_INPUTS - 0.3, 0.9) * (1.0 - rise(t, T_FLUENCE + 0.6, 1.0));
        let ui_alpha = 1.0 - 0.9 * rise(t, T_INPUTS - 0.3, 0.7) + 0.9 * rise(t, T_UNTILT - 0.2, 0.8);
        let merge_k = |n: usize| {
            // C4 merges first; each level collapses into the one below and switches to its merged radiance.
            let tn = T_MERGE + 0.45 * (CASCADES as usize - 1 - n) as f32;
            rise(t, tn, 0.5)
        };
        let gu = GU {
            size: [W as f32, H as f32, gs.0 as f32, gs.1 as f32],
            gi: [texel, t, 2.0, 0.0],
            hp: [[self.hp[0][0], self.hp[0][1], self.hp[1][0], self.hp[1][1]], [0.0; 4]],
        };
        ctx.queue.write_buffer(&ctx.viz.g, 0, bytemuck::bytes_of(&gu));
        let mut vps = vec![VP::default(); MAX_VP];
        let mut enc = ctx.device.create_command_encoder(&Default::default());
        ctx.means(&mut enc);
        // Layer content passes: slot 0 field, 1 scene, 2..7 cascades 0..4, 7 fluence, 8 lit UI.
        let expo = [0.9, 1.6, 2.6, 4.0, 6.0];
        let mut content: Vec<(usize, &wgpu::RenderPipeline, usize)> = vec![(0, &ctx.viz.field, 0), (1, &ctx.viz.scene, 0)];
        for n in 0..CASCADES as usize {
            let own_to_merged = if n == CASCADES as usize - 1 { 1.0 } else { merge_k(n + 1) };
            let grow = rise(t, T_CASCADES + 0.25 + 0.5 * n as f32, 0.8);
            vps[2 + n] = VP {
                a: [n as f32, own_to_merged, expo[n] * lerp(1.0, 0.6, own_to_merged), 0.0],
                b: [1.0, 1.0, 1.0, 1.0],
                c: [grow, 1.0, 0.0, 3.2],
                q: [[0.0; 4]; 4],
            };
            content.push((2 + n, &ctx.viz.cascade, n));
        }
        vps[1].a = [0.0, 0.0, 0.9, 0.0];
        vps[7].a = [0.0, 0.0, 1.5, 0.0];
        content.push((7, &ctx.viz.fluence, 0));
        drop(ins);
        for (slot, pipe, level) in &content {
            let bg = ctx.viz_bg(*level, None);
            let mut pass = Ctx::pass(&mut enc, &ctx.layers[*slot].mips[0], Some([0.0, 0.0, 0.0, 1.0]));
            pass.set_pipeline(pipe);
            pass.set_bind_group(0, &bg, &[]);
            pass.draw(0..3, *slot as u32..*slot as u32 + 1);
        }
        ctx.copy(&mut enc, lit, &ctx.layers[8].t.tex);
        ctx.queue.submit([enc.finish()]);
        let mut enc = ctx.device.create_command_encoder(&Default::default());
        for slot in 0..9 {
            ctx.build_mips(&mut enc, slot);
        }

        // Stack layout: (slot, z, alpha, label)
        let mut stack: Vec<(usize, f32, f32, String)> = Vec::new();
        let sep = 210.0;
        // Inputs: field and media sit below the UI while it lifts away.
        stack.push((0, -sep * 1.0 * inputs - 80.0 * (1.0 - inputs), inputs, "Distance field · ½ res, exact".into()));
        stack.push((1, 0.0, inputs, format!("Emitters + cards · {}×{} texels · sub-texel coverage", gs.0, gs.1)));
        // Cascades rise in from below, C0 lowest; then collapse top down.
        for n in 0..CASCADES as usize {
            let a_in = rise(t, T_CASCADES + 0.5 * n as f32, 0.7);
            let a_out = rise(t, T_FLUENCE + 0.2, 0.6);
            let base_z = (n as f32 - 2.0) * sep * 0.95;
            // Collapse: once merged into level n−1, sink onto it.
            let collapsed = if n == 0 { 0.0 } else { merge_k(n) };
            let below = (n as f32 - 3.0) * sep * 0.95;
            let z = lerp(base_z - 300.0 * (1.0 - a_in), below.min(base_z), collapsed);
            // Focus: once the next level arrives, this one recedes until the merge brings it back.
            let next_in = if n + 1 < CASCADES as usize { rise(t, T_CASCADES + 0.5 * (n + 1) as f32, 0.6) } else { 0.0 };
            let refocus = rise(t, T_MERGE - 0.2, 0.6);
            let focus = 1.0 - 0.6 * next_in * (1.0 - refocus);
            let alpha = a_in * (1.0 - a_out) * focus * (1.0 - 0.85 * if n > 0 { merge_k(n) } else { 0.0 });
            let p = (gs.0 >> n, gs.1 >> n);
            let t0 = (4f32.powi(n as i32) - 1.0) / 3.0 * texel;
            let t1 = (4f32.powi(n as i32 + 1) - 1.0) / 3.0 * texel;
            let ms = self.bench.cascades[n];
            stack.push((
                2 + n,
                z,
                alpha,
                format!("C{n} · {}×{} probes · {} rays · {:.0}–{:.0} px · {:.2} ms", p.0, p.1, 4 << (2 * n), t0, t1, ms),
            ));
        }
        let fl = window(t, T_FLUENCE, T_UNTILT + 0.5, 0.6);
        stack.push((7, -sep * 1.9 + sep * 1.9 * rise(t, T_FLUENCE, 0.8), fl, format!("Fluence · {:.2} ms", self.bench.fluence)));
        stack.push((8, sep * 3.2 * ui_lift, ui_alpha.clamp(0.0, 1.0), String::new()));

        // Back to front: sort by projected depth of the layer's centre.
        let mut order: Vec<usize> = (0..stack.len()).filter(|&i| stack[i].2 > 0.003).collect();
        order.sort_by(|&a, &b| {
            let za = project(&cam, W as f32 * 0.5, H as f32 * 0.5, stack[a].1)[3];
            let zb = project(&cam, W as f32 * 0.5, H as f32 * 0.5, stack[b].1)[3];
            zb.total_cmp(&za)
        });
        let first_vp = 16;
        for (k, &i) in order.iter().enumerate() {
            let (_, z, alpha, _) = &stack[i];
            let q = [project(&cam, 0.0, 0.0, *z), project(&cam, W as f32, 0.0, *z), project(&cam, 0.0, H as f32, *z), project(&cam, W as f32, H as f32, *z)];
            vps[first_vp + k] = VP { a: [0.0; 4], b: [0.0; 4], c: [0.0, *alpha, 0.55 * tilt, 0.0], q };
        }
        ctx.queue.write_buffer(&ctx.viz.vp, 0, bytemuck::cast_slice(&vps));
        let bgc = oklch(BG.0, BG.1, BG.2);
        {
            let bgs: Vec<wgpu::BindGroup> = order.iter().map(|&i| ctx.viz_bg(0, Some(&ctx.layers[stack[i].0].t.view))).collect();
            let mut pass = Ctx::pass(&mut enc, &ctx.stack.view, Some([bgc.r, bgc.g, bgc.b, 1.0]));
            pass.set_pipeline(&ctx.viz.layer);
            for (k, bg) in bgs.iter().enumerate() {
                pass.set_bind_group(0, bg, &[]);
                pass.draw(0..4, (first_vp + k) as u32..(first_vp + k) as u32 + 1);
            }
        }
        ctx.queue.submit([enc.finish()]);
        // Labels beside each layer's right edge, pushed apart so they never overlap.
        let mut labels: Vec<(f32, f32, f32, &str)> = Vec::new();
        for &i in &order {
            let (_, z, alpha, label) = &stack[i];
            if label.is_empty() || *alpha < 0.05 || tilt < 0.3 {
                continue;
            }
            let p = to_screen(project(&cam, W as f32, H as f32 * 0.5, *z));
            let a = alpha.min(1.0) * ((tilt - 0.3) / 0.4).clamp(0.0, 1.0);
            labels.push((p[0] / S + 44.0, p[1] / S - 8.0, a, label.as_str()));
        }
        labels.sort_by(|a, b| a.1.total_cmp(&b.1));
        for k in 1..labels.len() {
            let min = labels[k - 1].1 + 22.0;
            if labels[k].1 < min {
                labels[k].1 = min;
            }
        }
        for (x, y, a, l) in labels {
            ov.text(x, y, l, 12.0, 500.0, oklcha(0.9, 0.01, 268.0, a));
        }
        ctx.stack.view.clone()
    }
}

// ---------------------------------------------------------------- output

fn write_png(path: &Path, w: u32, h: u32, rgba: &[u8]) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    rune::render::write_png(path, w, h, rgba).unwrap();
}

fn downscale(src: &[u8], w: u32, h: u32, k: u32) -> Vec<u8> {
    let (ow, oh) = (w / k, h / k);
    let mut out = vec![0u8; (ow * oh * 4) as usize];
    let lin = |v: u8| {
        let c = v as f32 / 255.0;
        if c <= 0.04045 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) }
    };
    for y in 0..oh {
        for x in 0..ow {
            let mut acc = [0f32; 3];
            for dy in 0..k {
                for dx in 0..k {
                    let i = (((y * k + dy) * w + x * k + dx) * 4) as usize;
                    for c in 0..3 {
                        acc[c] += lin(src[i + c]);
                    }
                }
            }
            let o = ((y * ow + x) * 4) as usize;
            for c in 0..3 {
                out[o + c] = (rune::color::srgb_encode(acc[c] / (k * k) as f32) * 255.0).round() as u8;
            }
            out[o + 3] = 255;
        }
    }
    out
}

fn ffmpeg(args: &[&str], out: &Path) -> Child {
    Command::new("ffmpeg")
        .args(["-y", "-loglevel", "error", "-f", "rawvideo", "-s", &format!("{W}x{H}"), "-framerate", &FPS.to_string()])
        .args(args)
        .arg(out)
        .stdin(Stdio::piped())
        .spawn()
        .expect("ffmpeg")
}

fn render(reel: &Reel, ctx: &mut Ctx, out: &Path, from: f32, to: f32) {
    let tag = ["-colorspace", "bt709", "-color_primaries", "bt709", "-color_trc", "bt709", "-color_range", "tv", "-movflags", "+faststart"];
    let mut h264_args = vec!["-pix_fmt", "yuv420p", "-i", "-", "-c:v", "libx264", "-preset", "slow", "-crf", "12", "-tune", "film", "-pix_fmt", "yuv420p"];
    h264_args.extend(tag);
    let mut hevc_args = vec!["-pix_fmt", "yuv420p10le", "-i", "-", "-c:v", "libx265", "-preset", "medium", "-crf", "10", "-pix_fmt", "yuv420p10le", "-tag:v", "hvc1", "-x265-params", "log-level=error:aq-mode=3"];
    hevc_args.extend(tag);
    let mut h264 = ffmpeg(&h264_args, &out.join("rc-reel.mp4"));
    let mut hevc = ffmpeg(&hevc_args, &out.join("rc-reel-master-10bit.mp4"));
    let n0 = (from * FPS as f32).round() as u32;
    let n1 = (to * FPS as f32).round() as u32;
    let poster_t = T_END + 1.8;
    let sheet_t = [T_TILT + 2.2, T_INPUTS + 1.0, T_CASCADES + 2.2, T_MERGE + 1.4, T_FLUENCE + 0.7, T_UNTILT + 0.9];
    let mut sheet: Vec<Vec<u8>> = Vec::new();
    let t0 = std::time::Instant::now();
    for f in n0..n1 {
        let t = f as f32 / FPS as f32;
        let (view, _) = reel.frame(ctx, t, false);
        let want_png = (t - poster_t).abs() < 0.5 / FPS as f32 || sheet_t.iter().any(|s| (t - s).abs() < 0.5 / FPS as f32);
        let (y8, y10, rgba) = ctx.encode(&view, want_png, true);
        h264.stdin.as_mut().unwrap().write_all(&y8.unwrap()).expect("h264 write");
        hevc.stdin.as_mut().unwrap().write_all(&y10.unwrap()).expect("hevc write");
        if let Some(px) = rgba {
            if (t - poster_t).abs() < 0.5 / FPS as f32 {
                write_png(&out.join("poster.png"), W, H, &px);
            } else {
                sheet.push(downscale(&px, W, H, 4));
            }
        }
        if f % 60 == 0 {
            let line = format!("frame {f}/{n1} ({:.1}s)\n", t0.elapsed().as_secs_f32());
            let _ = std::fs::OpenOptions::new().create(true).append(true).open(out.join("progress.log")).and_then(|mut fh| fh.write_all(line.as_bytes()));
        }
    }
    drop(h264.stdin.take());
    drop(hevc.stdin.take());
    h264.wait().expect("h264");
    hevc.wait().expect("hevc");
    if !sheet.is_empty() {
        let (sw, sh) = (W / 4, H / 4);
        let cols = 3u32;
        let rows = (sheet.len() as u32).div_ceil(cols);
        let gap = 8;
        let (tw, th) = (sw * cols + gap * (cols - 1), sh * rows + gap * (rows - 1));
        let mut img = vec![10u8; (tw * th * 4) as usize];
        for (k, fr) in sheet.iter().enumerate() {
            let (cx, cy) = (k as u32 % cols, k as u32 / cols);
            for y in 0..sh {
                let src = &fr[(y * sw * 4) as usize..((y + 1) * sw * 4) as usize];
                let d = (((cy * (sh + gap) + y) * tw + cx * (sw + gap)) * 4) as usize;
                img[d..d + src.len()].copy_from_slice(src);
            }
        }
        for i in (3..img.len()).step_by(4) {
            img[i] = 255;
        }
        write_png(&out.join("breakdown-sheet.png"), tw, th, &img);
    }
    eprintln!("rendered {} frames in {:.1}s", n1 - n0, t0.elapsed().as_secs_f32());
}

fn median(v: &mut [f64]) -> f64 {
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    v[v.len() / 2]
}

fn bench(reel: &Reel, ctx: &mut Ctx, out: &Path, gpu: &str) {
    if !ctx.timestamps {
        eprintln!("no TIMESTAMP_QUERY on this adapter");
        return;
    }
    let mut runs: Vec<(Timings, f64)> = Vec::new();
    for f in 0..150 {
        // The hero at its fullest: every emitter on, progress flooding.
        let t = 5.0 + (f % 60) as f32 / 60.0;
        let (_, b) = reel.frame(ctx, t, true);
        if f >= 30 {
            if let Some(tm) = b.timings {
                runs.push((tm, b.glass_ms));
            }
        }
    }
    // Whole frame, wall clock of back-to-back light + glass passes (GPU-bound), no readback.
    let wall = {
        let st = state(5.5);
        let built = build_ui(&mut ctx.ts, &st);
        let g = glass_at(T_GLASS + 1.5).unwrap();
        let lights: Vec<GlassLight> = built.prims.iter().filter_map(|p| GlassLight::from_emitter(p, 18.0 * S, 0.9)).collect();
        ctx.light.inspect = false;
        let one = |ctx: &mut Ctx| {
            let mut enc = ctx.device.create_command_encoder(&Default::default());
            let scene = Scene { prims: &built.prims, lights: &[], settings: glow_settings(5.5), emission: None };
            ctx.light.render(&ctx.device, &ctx.queue, &mut enc, &scene, &ctx.base.view, &ctx.text.view, &ctx.lit.view);
            ctx.glass.render(&ctx.device, &ctx.queue, &mut enc, &[g], &lights, GlassSettings::default(), &ctx.lit.view, ctx.light.inspect().fluence, &ctx.glassed.view);
            ctx.queue.submit([enc.finish()]);
        };
        one(ctx);
        ctx.device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
        let t0 = std::time::Instant::now();
        for _ in 0..120 {
            one(ctx);
        }
        ctx.device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
        t0.elapsed().as_secs_f64() * 1000.0 / 120.0
    };
    let col = |f: &dyn Fn(&(Timings, f64)) -> f64| median(&mut runs.iter().map(f).collect::<Vec<_>>());
    let field = col(&|r| r.0.field);
    let gi = col(&|r| r.0.gi);
    let scene = col(&|r| r.0.gi_scene);
    let fluence = col(&|r| r.0.fluence);
    let glass = col(&|r| r.1);
    let cs: Vec<f64> = (0..CASCADES as usize).map(|n| col(&|r| r.0.cascades[n])).collect();
    let mut s = format!("{{\n  \"gpu\": \"{gpu}\",\n  \"resolution\": \"{W}x{H}\",\n  \"field\": {field:.4},\n  \"gi\": {gi:.4},\n  \"gi_scene\": {scene:.4},\n");
    for (n, c) in cs.iter().enumerate() {
        s += &format!("  \"c{n}\": {c:.4},\n");
    }
    s += &format!("  \"fluence\": {fluence:.4},\n  \"glass\": {glass:.4},\n  \"frame\": {wall:.4}\n}}\n");
    print!("{s}");
    std::fs::write(out.join("bench.json"), s).unwrap();
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let cmd = args.get(1).map(String::as_str).unwrap_or("render");
    let mut out = PathBuf::from("demo/reel");
    let (mut from, mut to) = (0.0, DURATION);
    let mut times: Vec<f32> = Vec::new();
    let mut i = 2;
    while i < args.len() {
        match args[i].as_str() {
            "--out" => {
                out = PathBuf::from(&args[i + 1]);
                i += 1;
            }
            "--from" => {
                from = args[i + 1].parse().unwrap();
                i += 1;
            }
            "--to" => {
                to = args[i + 1].parse().unwrap();
                i += 1;
            }
            s if cmd == "stills" && times.is_empty() => times = s.split(',').filter_map(|x| x.parse().ok()).collect(),
            _ => {}
        }
        i += 1;
    }
    std::fs::create_dir_all(&out).unwrap();
    let mut ctx = Ctx::new();
    let bench_out = PathBuf::from(std::env::var("REEL_BENCH").unwrap_or_else(|_| "demo/reel".into()));
    let reel = Reel {
        bench: load_bench(&bench_out),
        // Highlighted probes: in the gap under the progress bar, and between the chat and the right panel.
        hp: [
            [dpx(620.0, 421.0, 0.0, 0.0)[0], dpx(620.0, 421.0, 0.0, 0.0)[1]],
            [dpx(912.0, 300.0, 0.0, 0.0)[0], dpx(912.0, 300.0, 0.0, 0.0)[1]],
        ],
    };
    match cmd {
        "bench" => {
            let gpu = std::env::var("REEL_GPU").unwrap_or_else(|_| ctx.gpu.clone());
            bench(&reel, &mut ctx, &out, &gpu);
        }
        "stills" => {
            for t in times {
                let (view, _) = reel.frame(&mut ctx, t, false);
                let (_, _, rgba) = ctx.encode(&view, true, false);
                write_png(&out.join(format!("still-{t:05.2}.png")), W, H, &rgba.unwrap());
                eprintln!("still {t}");
            }
        }
        _ => render(&reel, &mut ctx, &out, from, to),
    }
}
