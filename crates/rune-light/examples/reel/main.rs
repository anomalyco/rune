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

mod frame;
mod gpu;
mod output;
mod ui;

use frame::{Reel, glass_at, glow_settings, load_bench};
use gpu::{Ctx, GU, MAX_VP, VP};
use output::{bench, render, write_png};
use ui::{BG, UI_O, UI_SCALE, build_ui, dpt, dpx, state};

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
