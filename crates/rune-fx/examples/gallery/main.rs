//! Headless gallery for rune-fx: stills, frame strips, PNG sequences and GPU timings.
//!
//!   gallery                          all effects -> demo/fx/<effect>.png, -strip.png, -reduced.png, timings.md
//!   gallery --effect fire            one effect
//!   gallery --frames DIR --fps 60 --seconds 2 --effect fire   PNG sequence
//!   gallery --timings                GPU timings only
#![allow(clippy::disallowed_methods, clippy::expect_used, clippy::let_underscore_must_use, clippy::undocumented_unsafe_blocks, clippy::unwrap_used)]

mod ui;

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

// ------------------------------------------------------------------ scenes

fn fling(t: f32) -> Pose {
    // Thrown in from off-frame left: critically damped spring (duration 0.6 s, bounce 0), released at t = 0.
    let (x0, target, v0, d, ts) = (-150.0f32, 410.0f32, 3000.0f32, 0.6f32, 0.0f32);
    let w = std::f32::consts::TAU / d;
    let tau = (t - ts).max(0.0);
    let a = x0 - target;
    let b = v0 + w * a;
    let e = (-w * tau).exp();
    let x = target + (a + b * tau) * e;
    let v = if t < ts { 0.0 } else { (b - w * (a + b * tau)) * e };
    Pose { x, y: 0.0, rotation: v * 0.00005, scale: 1.0, opacity: 1.0 }
}

fn scene(name: &str, b: &mut B) {
    let t = b.t;
    match name {
        "glow" => {
            b.background("Glow", "soft · neon that cools · aura · text bloom · hot core");
            // A: breathing soft glow under a button.
            let btn = Rect::new(56.0, 92.0, 150.0, 44.0);
            b.fx(Fx::glow(GlowParams { radius: 16.0, intensity: 0.85, breathe: 0.4, breathe_period: 2.6, hot: 0.4, ..Default::default() })
                .on(Shape::capsule(btn.x, btn.y, btn.w, btn.h)));
            b.button(btn, "Generate", 0x8f72ff, 0x6646ee, 0xffffff);
            b.caption(56.0, 166.0, "Soft · breathing");
            // B: neon card; switched off at 0.35 s in the strip so it visibly cools.
            let card = Rect::new(262.0, 70.0, 156.0, 96.0);
            b.card(card, 14.0);
            b.text(278.0, 98.0, "Neon", 14.0, 2, col(0xe6f9ff, 1.0));
            b.lines(278.0, 116.0, 100.0, 3);
            let mut heat = Heat::new(0.12, 0.32);
            heat.set(-1.0, true);
            heat.set(0.35, false);
            let h = if t < 0.35 { 1.0 } else { heat.at(t) };
            b.fx(Fx::glow(GlowParams {
                style: GlowStyle::Neon,
                color: rgb(0x22d3ee),
                core: rgb(0xeafcff),
                radius: 10.0,
                intensity: 1.0,
                inner: 0.35,
                heat: h,
                hot: 0.9,
                rim_width: 0.55,
                ..Default::default()
            })
            .on(Shape::rounded(card.x, card.y, card.w, card.h, 14.0))
            .over());
            b.caption(262.0, 192.0, "Neon · lights cool, never cut");
            // C: aura search field.
            let cap = Rect::new(56.0, 236.0, 300.0, 48.0);
            b.shadow(Rect::new(cap.x, cap.y + 6.0, cap.w, cap.h), 24.0, 12.0, [0.0, 0.0, 0.0, 0.5]);
            b.rect(cap, 24.0, col(0x15171d, 1.0), col(0x111318, 1.0), col(0xffffff, 0.08), 0.5);
            b.text(80.0, 265.0, "Ask anything…", 14.0, 0, col(0x7d8390, 1.0));
            b.fx(Fx::glow(GlowParams {
                style: GlowStyle::Aura,
                color: rgb(0xff5fa2),
                core: rgb(0x8b5cf6),
                third: rgb(0x38bdf8),
                radius: 12.0,
                intensity: 0.9,
                inner: 0.55,
                hot: 0.55,
                aura_speed: 0.15,
                rim_width: 0.6,
                occlusion: 0.1,
                ..Default::default()
            })
            .on(Shape::capsule(cap.x, cap.y, cap.w, cap.h))
            .over());
            b.caption(56.0, 308.0, "Aura · flowing spectrum");
            // D: text bloom from a mask pyramid.
            let (tx, ty) = (432.0, 292.0);
            b.target = Target::Mask;
            b.text(tx, ty, "Rune", 64.0, 2, col(0xffffff, 1.0));
            b.target = Target::Main;
            b.fx(Fx::glow(GlowParams {
                style: GlowStyle::Mask,
                color: rgb(0xff8a2a),
                core: rgb(0xfff1d6),
                radius: 10.0,
                intensity: 1.1,
                hot: 0.0,
                breathe: 0.25,
                breathe_period: 3.2,
                ..Default::default()
            })
            .on(Shape::rounded(tx, ty - 56.0, 160.0, 72.0, 0.0))
            .source(0, Rect::new(0.0, 0.0, W, H)));
            b.text(tx, ty, "Rune", 64.0, 2, col(0xfff4e6, 1.0));
            b.caption(432.0, 320.0, "Mask · text bloom");
            // E: hot core status dot.
            let dot = Shape::circle(470.0, 112.0, 7.0);
            b.fx(Fx::glow(GlowParams { color: rgb(0xffa630), core: rgb(0xfff6e0), radius: 9.0, intensity: 1.3, hot: 1.0, ..Default::default() }).on(dot));
            b.rect(dot.rect, 7.0, col(0xfff3dc, 1.0), col(0xffd9a0, 1.0), [0.0; 4], 0.0);
            b.text(490.0, 117.0, "Recording", 13.0, 1, col(0xd6d9df, 1.0));
            b.caption(462.0, 166.0, "Hot core");
        }
        "pulse" => {
            b.background("Pulses", "radar · breathing halo · heartbeat · ping that lands and ripples");
            let rec = Shape::circle(96.0, 112.0, 18.0);
            b.fx(Fx::pulse(PulseParams { color: rgb(0xff4d5e), core: rgb(0xffe4e6), reach: 46.0, width: 1.3, period: 2.2, ..Default::default() }).on(rec));
            b.rect(rec.rect, 18.0, col(0xff5a68, 1.0), col(0xe5243b, 1.0), col(0xffffff, 0.2), 0.5);
            b.rect(Rect::new(90.0, 106.0, 12.0, 12.0), 3.0, col(0xffffff, 0.95), col(0xffffff, 0.95), [0.0; 4], 0.0);
            b.caption(76.0, 184.0, "Radar");
            let av = Shape::circle(236.0, 112.0, 22.0);
            b.fx(Fx::pulse(PulseParams { style: PulseStyle::Breathe, color: rgb(0x34d399), core: rgb(0xd1fae5), reach: 12.0, period: 2.8, intensity: 0.8, ..Default::default() }).on(av));
            b.rect(av.rect, 22.0, col(0x3a4150, 1.0), col(0x262b35, 1.0), col(0x34d399, 0.9), 1.0);
            b.text_c(236.0, 112.0, "KL", 14.0, 2, col(0xeef0f3, 1.0));
            b.caption(196.0, 184.0, "Breathing halo");
            let live = Rect::new(354.0, 98.0, 74.0, 28.0);
            b.fx(Fx::pulse(PulseParams { style: PulseStyle::Heartbeat, color: rgb(0xff3b6b), core: rgb(0xffe4ec), reach: 26.0, width: 1.1, period: 1.1, ..Default::default() })
                .on(Shape::capsule(live.x, live.y, live.w, live.h)));
            b.rect(live, 14.0, col(0xf43f5e, 1.0), col(0xdb1f48, 1.0), col(0xffffff, 0.2), 0.5);
            b.text_c(391.0, 112.0, "LIVE", 12.0, 2, col(0xffffff, 1.0));
            b.caption(360.0, 184.0, "Heartbeat");
            // Ping from the send button to the inbox card, which ripples on arrival.
            let send = Shape::circle(84.0, 268.0, 16.0);
            let inbox = Rect::new(446.0, 222.0, 150.0, 92.0);
            b.card(inbox, 14.0);
            b.text(462.0, 250.0, "Inbox", 14.0, 2, col(0xeef0f3, 1.0));
            b.lines(462.0, 266.0, 96.0, 3);
            b.fx(Fx::pulse(PulseParams {
                style: PulseStyle::Ping { from: [104.0, 268.0], dir: [1.0, 0.0], travel: 332.0 },
                color: rgb(0x60a5fa),
                core: rgb(0xf0f7ff),
                t0: 0.05,
                duration: 0.62,
                lag: 0.16,
                head_radius: 2.6,
                ..Default::default()
            })
            .on(send));
            b.button(send.rect, "", 0x4b8dff, 0x2f6fe8, 0xffffff);
            b.text_c(84.0, 267.0, "→", 15.0, 2, col(0xffffff, 1.0));
            b.fx(Fx::pulse(PulseParams { style: PulseStyle::Ripple { at: [452.0, 268.0] }, color: rgb(0x93c5fd), core: rgb(0xffffff), reach: 0.0, width: 2.2, t0: 0.62, duration: 0.9, intensity: 0.7, ..Default::default() })
                .on(Shape::rounded(inbox.x, inbox.y, inbox.w, inbox.h, 14.0))
                .over());
            b.caption(64.0, 330.0, "Ping → ripple (anticipation, decelerating landing, cools)");
        }
        "electric" => {
            b.background("Electricity", "arc with forks · border current · calm charged border");
            let (a, c) = ([64.0f32, 96.0f32], [576.0f32, 96.0f32]);
            b.fx(Fx::electric(ElectricParams { style: ElectricStyle::Arc { a: [a[0] + 6.0, a[1]], b: [c[0] - 6.0, c[1]] }, amplitude: 9.0, glow: 6.0, forks: 0.6, strands: 3, ..Default::default() })
                .on(Shape::rounded(a[0], a[1] - 1.0, c[0] - a[0], 2.0, 0.0)));
            for p in [a, c] {
                b.rect(Rect::new(p[0] - 8.0, p[1] - 8.0, 16.0, 16.0), 8.0, col(0xb8c0cc, 1.0), col(0x4b5563, 1.0), col(0xffffff, 0.3), 0.5);
            }
            b.caption(64.0, 140.0, "Arc · forks shoot out and cool");
            let card = Rect::new(64.0, 178.0, 236.0, 132.0);
            b.card(card, 16.0);
            b.text(84.0, 210.0, "Overload", 15.0, 2, col(0xeef0f3, 1.0));
            b.lines(84.0, 228.0, 170.0, 4);
            b.fx(Fx::electric(ElectricParams { amplitude: 4.0, gap: 3.0, fork_len: 22.0, glow: 5.0, ..Default::default() })
                .on(Shape::rounded(card.x, card.y, card.w, card.h, 16.0))
                .seed(7));
            b.caption(64.0, 340.0, "Border current");
            let btn = Rect::new(380.0, 222.0, 196.0, 50.0);
            b.button(btn, "Charged", 0x1e2a44, 0x162036, 0xdbe8ff);
            b.fx(Fx::electric(ElectricParams { color: rgb(0x60a5fa), ..ElectricParams::charged() })
                .on(Shape::capsule(btn.x, btn.y, btn.w, btn.h))
                .seed(3));
            b.caption(380.0, 300.0, "Charged border (calm, for UI)");
        }
        "fire" => {
            b.background("Fire & plasma", "flames + embers · plasma border · heat for a button");
            let card = Rect::new(72.0, 204.0, 224.0, 124.0);
            b.fx(Fx::fire(FireParams::default()).on(Shape::rounded(card.x, card.y, card.w, card.h, 16.0)).seed(5));
            b.card(card, 16.0);
            b.text(92.0, 236.0, "Hot take", 15.0, 2, col(0xfff1e6, 1.0));
            b.lines(92.0, 254.0, 160.0, 4);
            let cap = Rect::new(356.0, 118.0, 224.0, 56.0);
            b.fx(Fx::fire(FireParams::plasma()).on(Shape::capsule(cap.x, cap.y, cap.w, cap.h)).seed(11));
            b.rect(cap, 28.0, col(0x17142a, 1.0), col(0x100e1f, 1.0), col(0xb9a8ff, 0.25), 0.5);
            b.text_c(cap.x + cap.w * 0.5, cap.y + cap.h * 0.5, "Plasma", 15.0, 2, col(0xe9e4ff, 1.0));
            b.caption(356.0, 214.0, "Plasma border");
            let btn = Rect::new(392.0, 266.0, 152.0, 44.0);
            b.fx(Fx::fire(FireParams::heat()).on(Shape::capsule(btn.x, btn.y, btn.w, btn.h)).seed(2));
            b.button(btn, "Deploy", 0xff8a3d, 0xe8590c, 0xffffff);
            b.backdrop();
            b.fx(Fx::distort(DistortParams::HeatHaze { height: 26.0, scale: 9.0, speed: 2.0, amount: 1.1, whole_shape: false })
                .on(Shape::capsule(btn.x, btn.y, btn.w, btn.h)));
            b.caption(392.0, 336.0, "Heat (UI) + haze");
            b.caption(72.0, 350.0, "Flames + embers");
        }
        "motion" => {
            b.background("Motion blur", "a card flung across the frame · none / directional / shutter 180°");
            let rest = Rect::new(60.0, 0.0, 124.0, 74.0);
            let lanes = [("No blur", 62.0f32, 0.0f32, BlurMode::Shutter), ("Directional (cheap fallback)", 162.0, 180.0, BlurMode::Directional), ("Shutter 180° (exact sub-frame poses)", 262.0, 180.0, BlurMode::Shutter)];
            for (i, (label, y, angle, mode)) in lanes.iter().enumerate() {
                let r = Rect::new(rest.x, *y, rest.w, rest.h);
                b.caption(24.0, y + rest.h + 18.0, label);
                if i == 0 {
                    // Card content, drawn once into the node layer at lane 0's rest rect.
                    b.src_rect = Some(r);
                    b.target = Target::Src;
                    b.shadow(Rect::new(r.x, r.y + 4.0, r.w, r.h), 14.0, 4.0, [0.0, 0.0, 0.0, 0.35]);
                    b.rect(r, 14.0, col(0x23262e, 1.0), col(0x1a1c22, 1.0), col(0xffffff, 0.1), 0.5);
                    b.rect(Rect::new(r.x + 12.0, r.y + 12.0, 26.0, 26.0), 13.0, col(0x9b7bff, 1.0), col(0x6d4cf5, 1.0), [0.0; 4], 0.0);
                    b.text(r.x + 46.0, r.y + 23.0, "Flight 204", 12.5, 2, col(0xf3f4f6, 1.0));
                    b.text(r.x + 46.0, r.y + 37.0, "Departing 9:41", 10.5, 0, col(0x9aa1ad, 1.0));
                    b.rect(Rect::new(r.x + 12.0, r.y + 56.0, r.w - 24.0, 5.0), 2.5, col(0x30343d, 1.0), col(0x30343d, 1.0), [0.0; 4], 0.0);
                    b.rect(Rect::new(r.x + 12.0, r.y + 56.0, (r.w - 24.0) * 0.62, 5.0), 2.5, col(0x34d399, 1.0), col(0x10b981, 1.0), [0.0; 4], 0.0);
                    b.target = Target::Main;
                }
                let src_rect = b.src_rect.unwrap().inflate(SRC_PAD);
                let dy = y - 62.0;
                let p = MotionBlurParams { mode: *mode, shutter_angle: *angle, ..Default::default() };
                let sp = if b.reduced { MotionBlurParams { shutter_angle: 0.0, ..p } } else { p };
                let samples = MotionSamples::from_track(&sp, b.src_rect.unwrap(), t, S, |tt| {
                    let mut q = fling(tt);
                    q.y += dy;
                    q
                });
                b.fx(Fx::motion_blur(p, samples).on(Shape::rounded(r.x, r.y, r.w, r.h, 14.0)).source(1, src_rect));
            }
        }
        "distort" => {
            b.background("Distortion", "liquid-glass lens (gooey union) · shockwave · CRT · chromatic split");
            // Page content beneath.
            let tiles = [(0x8b5cf6, 0x5b21b6), (0xfb923c, 0xea580c), (0x2dd4bf, 0x0f766e), (0xf472b6, 0xbe185d)];
            for (i, (a, c)) in tiles.iter().enumerate() {
                let x = 40.0 + i as f32 * 143.0;
                b.rect(Rect::new(x, 60.0, 131.0, 64.0), 12.0, col(*a, 1.0), col(*c, 1.0), col(0xffffff, 0.15), 0.5);
                b.text(x + 12.0, 112.0, ["Violet", "Ember", "Lagoon", "Orchid"][i], 13.0, 2, col(0xffffff, 0.95));
            }
            let para = ["Glass bends what is behind it: the rim squeezes the page,", "the middle stays sharp, and colour splits only at the edge.", "Rune samples the backdrop once per lens and lights the rim."];
            for (i, l) in para.iter().enumerate() {
                b.text(40.0, 152.0 + i as f32 * 20.0, l, 13.0, 0, col(0xb4b9c4, 1.0));
            }
            // CRT content.
            let crt = Rect::new(40.0, 222.0, 250.0, 118.0);
            b.rect(crt, 12.0, col(0x0d1a12, 1.0), col(0x07110b, 1.0), [0.0; 4], 0.0);
            b.text(56.0, 250.0, "> rune build --release", 13.0, 1, col(0x4ade80, 1.0));
            b.text(56.0, 270.0, "  compiling rune-fx v0.0.1", 13.0, 0, col(0x86efac, 0.8));
            b.text(56.0, 290.0, "  finished in 1.84 s", 13.0, 0, col(0x86efac, 0.8));
            b.rect(Rect::new(56.0, 304.0, 120.0, 14.0), 3.0, col(0x22c55e, 1.0), col(0x16a34a, 1.0), [0.0; 4], 0.0);
            b.text(330.0, 300.0, "Chromatic", 40.0, 2, col(0xf3f4f6, 1.0));
            // Lens: capsule + a droplet pulling away.
            let lx = if b.reduced { 380.0 } else { 352.0 + 70.0 * (0.5 - 0.5 * (t * 2.2).cos()) };
            b.backdrop();
            b.fx(Fx::lens(LensParams { second: Some(Shape::circle(lx, 116.0, 30.0)), union_k: 26.0, ..Default::default() })
                .on(Shape::capsule(140.0, 84.0, 200.0, 64.0)));
            b.backdrop();
            b.fx(Fx::lens(LensParams { frost: 1.0, ..Default::default() }).on(Shape::capsule(388.0, 92.0, 220.0, 66.0)));
            b.backdrop();
            b.fx(Fx::distort(DistortParams::shockwave([470.0, 150.0], 0.0)).on(Shape::rounded(0.0, 0.0, W, H, 0.0)));
            b.backdrop();
            b.fx(Fx::distort(DistortParams::crt()).on(Shape::rounded(crt.x, crt.y, crt.w, crt.h, 12.0)));
            b.backdrop();
            b.fx(Fx::distort(DistortParams::ChromaSplit { offset: [1.6, 0.0], glitch: 0.3, feather: 2.0 }).on(Shape::rounded(322.0, 256.0, 260.0, 60.0, 6.0)));
        }
        "extras" => {
            b.background("Extras", "skeleton shimmer · text sheen · sparkles · aurora + grain");
            let list = Rect::new(40.0, 62.0, 280.0, 186.0);
            b.card(list, 16.0);
            for i in 0..3 {
                let y = list.y + 22.0 + i as f32 * 54.0;
                for tgt in [Target::Main, Target::Mask] {
                    b.target = tgt;
                    let c = if tgt == Target::Main { col(0x22262e, 1.0) } else { col(0xffffff, 1.0) };
                    b.rect(Rect::new(list.x + 20.0, y, 32.0, 32.0), 16.0, c, c, [0.0; 4], 0.0);
                    b.rect(Rect::new(list.x + 64.0, y + 4.0, 160.0 - i as f32 * 24.0, 10.0), 5.0, c, c, [0.0; 4], 0.0);
                    b.rect(Rect::new(list.x + 64.0, y + 20.0, 104.0, 8.0), 4.0, c, c, [0.0; 4], 0.0);
                }
                b.target = Target::Main;
            }
            b.fx(Fx::shimmer(ShimmerParams { intensity: 0.11, band: 42.0, use_mask: true, ..Default::default() })
                .on(Shape::rounded(list.x, list.y, list.w, list.h, 16.0))
                .source(0, Rect::new(0.0, 0.0, W, H)));
            b.caption(40.0, 272.0, "Skeleton shimmer");
            b.text(40.0, 316.0, "Thinking…", 20.0, 1, col(0x8b919c, 1.0));
            b.target = Target::Mask;
            b.text(40.0, 316.0, "Thinking…", 20.0, 1, col(0xffffff, 1.0));
            b.target = Target::Main;
            b.fx(Fx::shimmer(ShimmerParams { intensity: 0.85, band: 18.0, period: 1.6, angle: 0.25, use_mask: true, ..Default::default() })
                .on(Shape::rounded(36.0, 292.0, 110.0, 32.0, 0.0))
                .source(0, Rect::new(0.0, 0.0, W, H)));
            b.caption(40.0, 344.0, "Text sheen");
            let au = Rect::new(352.0, 62.0, 248.0, 152.0);
            b.card(au, 18.0);
            b.fx(Fx::aurora(AuroraParams { intensity: 0.75, scale: 110.0, feather: 18.0, ..Default::default() }).on(Shape::rounded(au.x, au.y, au.w, au.h, 18.0)).over());
            b.fx(Fx::grain(GrainParams { amount: 0.016, ..Default::default() }).on(Shape::rounded(au.x, au.y, au.w, au.h, 18.0)));
            b.text(372.0, 196.0, "Aurora", 18.0, 2, col(0xf5f7fa, 1.0));
            b.caption(352.0, 238.0, "Aurora + static grain");
            let pro = Rect::new(432.0, 274.0, 88.0, 34.0);
            b.rect(pro, 17.0, col(0xf7d98b, 1.0), col(0xcf9f38, 1.0), col(0xffffff, 0.4), 0.5);
            b.text_c(476.0, 291.0, "PRO", 13.0, 2, col(0x3b2a06, 1.0));
            b.fx(Fx::sparkles(SparkleParams { margin: 20.0, size: 8.0, count: 6, ..Default::default() }).on(Shape::capsule(pro.x, pro.y, pro.w, pro.h)).seed(4));
            b.caption(432.0, 344.0, "Sparkles");
        }
        "liquid-metal" => {
            b.background("Liquid metal", "chrome · gold · rose gold · gunmetal · as a border");
            let shadow = |b: &mut B, r: Rect, radius: f32| b.shadow(Rect::new(r.x, r.y + 10.0, r.w, r.h), radius, 16.0, [0.0, 0.0, 0.0, 0.55]);
            let slab = Rect::new(32.0, 58.0, 260.0, 160.0);
            shadow(b, slab, 40.0);
            b.fx(Fx::fill(LiquidMetalParams::chrome()).on(Shape::rounded(slab.x, slab.y, slab.w, slab.h, 40.0)));
            b.caption(32.0, 238.0, "Chrome");
            let btn = Rect::new(318.0, 62.0, 180.0, 52.0);
            shadow(b, btn, 26.0);
            b.fx(Fx::fill(LiquidMetalParams::gold()).on(Shape::capsule(btn.x, btn.y, btn.w, btn.h)));
            b.caption(318.0, 134.0, "Gold");
            shadow(b, Rect::new(534.0, 54.0, 68.0, 68.0), 34.0);
            b.fx(Fx::fill(LiquidMetalParams::rose()).on(Shape::circle(568.0, 88.0, 34.0)));
            b.caption(534.0, 134.0, "Rose gold");
            let gm = Rect::new(318.0, 152.0, 290.0, 66.0);
            shadow(b, gm, 18.0);
            b.fx(Fx::fill(LiquidMetalParams::gunmetal()).on(Shape::rounded(gm.x, gm.y, gm.w, gm.h, 18.0)));
            b.caption(318.0, 238.0, "Gunmetal");
            // Borders: a chrome tube round a card, a gold ring round an avatar, a rose-gold capsule.
            let card = Rect::new(32.0, 262.0, 260.0, 74.0);
            b.card(card, 24.0);
            b.fx(Fx::fill(LiquidMetalParams::chrome().border(7.0)).on(Shape::rounded(card.x, card.y, card.w, card.h, 24.0)));
            b.text_c(card.center().x, card.center().y, "Chrome border", 15.0, 2, col(0xe8eaee, 1.0));
            b.rect(Rect::new(330.0, 263.0, 72.0, 72.0), 36.0, col(0x7c5cff, 1.0), col(0x2b1b5a, 1.0), [0.0; 4], 0.0);
            b.text_c(366.0, 299.0, "KL", 22.0, 2, col(0xffffff, 0.95));
            b.fx(Fx::fill(LiquidMetalParams::gold().border(4.0)).on(Shape::circle(366.0, 299.0, 36.0)));
            let cap = Rect::new(428.0, 272.0, 180.0, 54.0);
            b.rect(cap, 27.0, col(0x1a1c22, 1.0), col(0x121419, 1.0), [0.0; 4], 0.0);
            b.fx(Fx::fill(LiquidMetalParams::rose().border(5.0)).on(Shape::capsule(cap.x, cap.y, cap.w, cap.h)));
            b.text_c(cap.center().x, cap.center().y, "Rose border", 14.0, 2, col(0xf2d4c8, 1.0));
        }
        "plastic" => {
            b.background("Plastic", "glossy · matte and dished · liquid metal keys pinned to the window or to each key");
            let colors = [0xece8de, 0x2e3036, 0xd7263d, 0xf46036, 0xffd60a, 0x2a9d8f, 0x3d6fd6, 0x7b5cd6];
            let labels = ["Esc", "Q", "W", "E", "R", "T", "Y", "U"];
            let rows = [(52.0, PlasticParams { roughness: 0.12, ..PlasticParams::glossy() }, "Glossy: roughness .12, dome .35"), (138.0, PlasticParams { roughness: 0.8, dome: -0.4, ..PlasticParams::glossy() }, "Matte, dished: roughness .8, dome −.4")];
            for (y, p, caption) in rows {
                for (k, (c, label)) in colors.into_iter().zip(labels).enumerate() {
                    let r = Rect::new(54.0 + k as f32 * 68.0, y, 56.0, 56.0);
                    b.shadow(Rect::new(r.x, r.y + 5.0, r.w, r.h), 10.0, 6.0, [0.0, 0.0, 0.0, 0.55]);
                    b.fx(Fx::fill(PlasticParams { color: rgb(c), ..p }).on(Shape::rounded(r.x, r.y, r.w, r.h, 10.0)));
                    let ink = if rgb(c).0.iter().sum::<f32>() > 1.2 { col(0x26272b, 0.85) } else { col(0xf4f4f6, 0.9) };
                    b.text_c(r.center().x, r.center().y, label, 13.0, 2, ink);
                }
                b.caption(54.0, y + 74.0, caption);
            }
            for (y, anchor, caption) in [(232.0, FillAnchor::World, "Liquid metal on 24 pt keys, anchor: world (default)"), (290.0, FillAnchor::Element, "Liquid metal on 24 pt keys, anchor: element")] {
                for k in 0..18 {
                    let r = Rect::new(53.0 + k as f32 * 30.0, y, 24.0, 24.0);
                    b.shadow(Rect::new(r.x, r.y + 3.0, r.w, r.h), 5.0, 4.0, [0.0, 0.0, 0.0, 0.55]);
                    b.fx(Fx::fill(LiquidMetalParams::chrome().anchor(anchor)).on(Shape::rounded(r.x, r.y, r.w, r.h, 5.0)));
                }
                b.caption(54.0, y + 42.0, caption);
            }
        }
        "mesh-gradient" => {
            b.background("Mesh gradient", "dusk · lagoon · orchid · graphite: drifting points, blended in OKLab");
            b.fx(Fx::fill(MeshGradientParams::dusk()).on(Shape::rounded(32.0, 58.0, 300.0, 272.0, 28.0)));
            b.text(56.0, 306.0, "Dusk", 28.0, 2, col(0xffffff, 0.95));
            for (k, (p, name)) in [(MeshGradientParams::lagoon(), "Lagoon"), (MeshGradientParams::orchid(), "Orchid"), (MeshGradientParams::graphite(), "Graphite")].into_iter().enumerate() {
                let y = 58.0 + k as f32 * 94.0;
                b.fx(Fx::fill(p).on(Shape::rounded(352.0, y, 256.0, 84.0, 18.0)).seed(k as u32 + 2));
                b.text(372.0, y + 68.0, name, 15.0, 2, col(0xffffff, 0.92));
            }
        }
        "dither" => {
            b.background("Dither", "Bayer 2×2, 4×4, 8×8 · blue noise · of a ramp, noise or the backdrop");
            let xs = [32.0, 182.0, 332.0, 482.0];
            let tiles = [(DitherParams::bayer(), "Bayer 4×4, ramp"), (DitherParams::handheld(), "Palette, noise"), (DitherParams::grain(), "Blue noise, radial"), (DitherParams::backdrop(), "Backdrop")];
            for (x, (p, name)) in xs.into_iter().zip(tiles) {
                let r = Rect::new(x, 58.0, 140.0, 128.0);
                if p.input == FillInput::Backdrop {
                    b.rect(r, 16.0, col(0xff7aa2, 1.0), col(0x3b2a8f, 1.0), [0.0; 4], 0.0);
                    b.rect(Rect::new(x + 62.0, 74.0, 64.0, 64.0), 32.0, col(0xfff1c2, 1.0), col(0xffb44d, 1.0), [0.0; 4], 0.0);
                    b.text(x + 14.0, 172.0, "Aa", 40.0, 2, col(0xffffff, 1.0));
                    b.backdrop();
                }
                b.fx(Fx::fill(p).on(Shape::rounded(r.x, r.y, r.w, r.h, 16.0)));
                b.caption(x, 204.0, name);
            }
            let pats = [(DitherPattern::Bayer2, "Bayer 2×2"), (DitherPattern::Bayer4, "Bayer 4×4"), (DitherPattern::Bayer8, "Bayer 8×8"), (DitherPattern::White, "White noise")];
            for (x, (pattern, name)) in xs.into_iter().zip(pats) {
                let p = DitherParams { pattern, pixel: 3.0, angle: 0.0, colors: Stops::hex(&[0x1b1530, 0x9be7ff]), ..DitherParams::bayer() };
                b.fx(Fx::fill(p).on(Shape::rounded(x, 222.0, 140.0, 96.0, 14.0)));
                b.caption(x, 336.0, name);
            }
        }
        "grain-gradient" => {
            b.background("Grain gradient", "linear · radial · conic, with film grain (mono or colour)");
            for (x, (p, name)) in [32.0, 229.0, 426.0].into_iter().zip([(GrainGradientParams::ember(), "Ember (linear, mono)"), (GrainGradientParams::halo(), "Halo (radial)"), (GrainGradientParams::sweep(), "Sweep (conic, colour grain)")]) {
                b.fx(Fx::fill(p).on(Shape::rounded(x, 58.0, 182.0, 250.0, 22.0)));
                b.caption(x, 328.0, name);
            }
        }
        "fluted-glass" => {
            b.background("Fluted glass", "reeded · fine · frosted · wide ribs, over moving content");
            b.rect(Rect::new(24.0, 48.0, 592.0, 266.0), 24.0, col(0x1e2a5a, 1.0), col(0x0d1226, 1.0), [0.0; 4], 0.0);
            let blobs = [(0xff8a3d, 0xe0451f, 140.0, 118.0, 54.0), (0x8b5cf6, 0x4c1d95, 255.0, 236.0, 62.0), (0x2dd4bf, 0x0f766e, 405.0, 112.0, 50.0), (0xf472b6, 0xbe185d, 548.0, 226.0, 58.0)];
            for (k, (a, c, x, y, r)) in blobs.into_iter().enumerate() {
                let dx = if b.reduced { 0.0 } else { 26.0 * (t * 0.9 + k as f32 * 1.7).sin() };
                b.rect(Rect::new(x - r + dx, y - r, 2.0 * r, 2.0 * r), r, col(a, 1.0), col(c, 1.0), [0.0; 4], 0.0);
            }
            b.text(46.0, 214.0, "Fluted", 96.0, 2, col(0xffffff, 0.96));
            for (x, (p, name)) in [40.0, 190.0, 340.0, 490.0].into_iter().zip([(FlutedGlassParams::reeded(), "Reeded"), (FlutedGlassParams::fine(), "Fine"), (FlutedGlassParams::frosted(), "Frosted"), (FlutedGlassParams::wide(), "Wide")]) {
                b.backdrop();
                b.fx(Fx::fill(p).on(Shape::rounded(x, 60.0, 120.0, 240.0, 16.0)));
                b.caption(x, 324.0, name);
            }
        }
        "fills" => {
            b.background("More fills", "metaballs · swirl · voronoi · waves · halftone");
            let xs = [32.0, 229.0, 426.0];
            let row1: [(FillParams, &str); 3] = [(MetaballParams::lava().into(), "Metaballs"), (SwirlParams::ink().into(), "Swirl"), (VoronoiParams::stained().into(), "Voronoi")];
            for (x, (p, name)) in xs.into_iter().zip(row1) {
                b.fx(Fx::fill(p).on(Shape::rounded(x, 56.0, 182.0, 118.0, 18.0)));
                b.caption(x, 192.0, name);
            }
            b.fx(Fx::fill(WaveParams::sea()).on(Shape::rounded(32.0, 208.0, 182.0, 118.0, 18.0)));
            b.caption(32.0, 344.0, "Waves");
            b.fx(Fx::fill(HalftoneParams::print()).on(Shape::rounded(229.0, 208.0, 182.0, 118.0, 18.0)));
            b.caption(229.0, 344.0, "Halftone");
            let r = Rect::new(426.0, 208.0, 182.0, 118.0);
            b.rect(r, 18.0, col(0xf6d365, 1.0), col(0xfda085, 1.0), [0.0; 4], 0.0);
            b.rect(Rect::new(470.0, 222.0, 90.0, 90.0), 45.0, col(0x5b8def, 1.0), col(0x1d2b64, 1.0), [0.0; 4], 0.0);
            b.backdrop();
            b.fx(Fx::fill(HalftoneParams::backdrop()).on(Shape::rounded(r.x, r.y, r.w, r.h, 18.0)));
            b.caption(426.0, 344.0, "Halftone of the backdrop");
        }
        "fill-presets" => {
            b.background("Fill presets", "every style and preset in the catalog");
            let all: Vec<(&StyleInfo, &Preset)> = fill_styles().iter().flat_map(|s| s.presets.iter().map(move |p| (s, p))).collect();
            for (k, (s, p)) in all.into_iter().enumerate() {
                let (x, y) = (28.0 + (k % 8) as f32 * 74.0, 50.0 + (k / 8) as f32 * 78.0);
                let r = Rect::new(x, y, 66.0, 44.0);
                let f = p.fill();
                if f.needs_backdrop() {
                    b.rect(r, 10.0, col(0x3b82f6, 1.0), col(0x1e1b4b, 1.0), [0.0; 4], 0.0);
                    b.rect(Rect::new(x + 8.0, y + 6.0, 30.0, 30.0), 15.0, col(0xffd166, 1.0), col(0xef476f, 1.0), [0.0; 4], 0.0);
                    b.text(x + 34.0, y + 36.0, "Aa", 22.0, 2, col(0xffffff, 1.0));
                    b.backdrop();
                }
                b.fx(Fx::fill(f).on(Shape::rounded(r.x, r.y, r.w, r.h, 10.0)).seed(k as u32));
                b.text(x, y + 56.0, s.name, 9.0, 1, col(0x6b7280, 1.0));
                b.text(x, y + 67.0, p.name, 9.0, 2, col(0xb4b9c4, 1.0));
            }
        }
        "blend" => {
            // Cream paper (#FAF7F2) backdrop for an onboarding hero and shader fills.
            b.rect(Rect::new(0.0, 0.0, W, H), 0.0, col(0xfaf7f2, 1.0), col(0xf3ede3, 1.0), [0.0; 4], 0.0);
            let tw = b.text(24.0, 34.0, "Blend", 15.0, 2, col(0x1f1c18, 1.0));
            b.text(24.0 + tw + 8.0, 34.0, "warm glow & shader fill over #FAF7F2 cream paper", 12.0, 0, col(0x6e6559, 1.0));

            let modes = [
                ("Normal", FxBlend::Normal),
                ("Multiply", FxBlend::Multiply),
                ("Screen", FxBlend::Screen),
                ("Overlay", FxBlend::Overlay),
                ("SoftLight", FxBlend::SoftLight),
            ];
            for (idx, (label, _) ) in modes.iter().enumerate() {
                let x = 24.0 + idx as f32 * 120.0;
                // Subtle warm card rules on the cream paper so blend interaction with midtones is visible.
                let well = Rect::new(x, 54.0, 112.0, 132.0);
                b.rect(well, 12.0, col(0xfaf7f2, 1.0), col(0xf4efe6, 1.0), col(0x2b241c, 0.10), 1.0 / S);
                for r in 0..3 {
                    let ry = 140.0 + r as f32 * 12.0;
                    b.rect(Rect::new(x + 16.0, ry, 80.0 - (r as f32 * 14.0), 4.0), 2.0, col(0xd8cfc2, 1.0), col(0xd8cfc2, 1.0), [0.0; 4], 0.0);
                }

                // Lower row: warm ruled paper card beneath a shader fill.
                let card = Rect::new(x, 212.0, 112.0, 108.0);
                b.rect(card, 12.0, col(0xf6f0e6, 1.0), col(0xebe2d3, 1.0), col(0x2b241c, 0.12), 1.0 / S);
                b.rect(Rect::new(x + 12.0, 228.0, 56.0, 22.0), 6.0, col(0xd48b45, 0.35), col(0xb85d22, 0.35), [0.0; 4], 0.0);
                b.text(x + 16.0, 274.0, "Paper", 13.0, 2, col(0x3a3128, 0.85));
                for r in 0..2 {
                    let ry = 286.0 + r as f32 * 11.0;
                    b.rect(Rect::new(x + 16.0, ry, 76.0 - (r as f32 * 18.0), 4.0), 2.0, col(0x9c8e7b, 0.55), col(0x9c8e7b, 0.55), [0.0; 4], 0.0);
                }
                b.text_c(x + 56.0, 336.0, label, 11.0, 2, col(0x3f372e, 1.0));
            }
            b.backdrop();
            for (idx, (_, mode)) in modes.iter().enumerate() {
                let x = 24.0 + idx as f32 * 120.0;
                let pill = Rect::new(x + 22.0, 92.0, 68.0, 32.0);
                let glow = Fx::glow(GlowParams {
                    color: rgb(0xc85a14),
                    core: rgb(0xe88c28),
                    radius: 22.0,
                    intensity: 1.15,
                    inner: 0.65,
                    hot: 0.45,
                    ..Default::default()
                })
                .on(Shape::capsule(pill.x, pill.y, pill.w, pill.h));
                b.fx_with(glow, FxSlot::default().blend(*mode));
                b.button(pill, "Hero", 0x2c2520, 0x1e1915, 0xfaf7f2);

                let fill_shape = Shape::rounded(x + 8.0, 220.0, 96.0, 92.0, 10.0);
                let fill = Fx::fill(GrainGradientParams::ember()).on(fill_shape).opacity(0.82);
                b.fx_with(fill, FxSlot::default().blend(*mode));
            }
        }
        _ => panic!("unknown effect {name}"),
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
