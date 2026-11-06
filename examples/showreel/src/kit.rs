//! Shared look and motion helpers for the scenes.

use rune::color::Color;
use rune::{El, TextStyle, el, text};
use rune_motion::Spring;

pub const W: f32 = 960.0;
pub const H: f32 = 540.0;

/// An sRGB hex colour.
pub fn c(hex: u32) -> Color {
    rune::color::hex(&format!("#{hex:06x}")).unwrap()
}

pub fn ca(hex: u32, a: f32) -> Color {
    c(hex).alpha(a)
}

pub const BG: u32 = 0x0b0b0d;
pub const PANEL: u32 = 0x141417;
pub const RAISED: u32 = 0x1b1b1f;
pub const TEXT: u32 = 0xededef;
pub const MUTED: u32 = 0x9a9aa3;
pub const FAINT: u32 = 0x5f5f68;

pub fn hair() -> Color {
    ca(0xffffff, 0.08)
}

pub fn t(s: impl Into<String>, size: f32, weight: f32, color: u32) -> El {
    text(s).font(TextStyle::new(size, weight)).color(c(color))
}

pub fn ta(s: impl Into<String>, size: f32, weight: f32, color: u32, alpha: f32) -> El {
    text(s).font(TextStyle::new(size, weight)).color(ca(color, alpha.clamp(0.0, 1.0)))
}

pub fn mono_a(s: impl Into<String>, size: f32, color: u32, alpha: f32) -> El {
    text(s).font(TextStyle::new(size, 500.0).mono()).color(ca(color, alpha.clamp(0.0, 1.0)))
}

pub fn mono(s: impl Into<String>, size: f32, color: u32) -> El {
    text(s).font(TextStyle::new(size, 500.0).mono()).color(c(color))
}

/// An effect slot: `kind` is a `gfx` constant, `data` its numbers.
pub fn fx(kind: u32, data: [f32; 4]) -> El {
    rune::effect(kind).effect_data(data)
}

/// 0 → 1 between `t0` and `t0 + dur`, smoothstep.
pub fn ease(t: f64, t0: f64, dur: f64) -> f32 {
    let x = ((t - t0) / dur).clamp(0.0, 1.0) as f32;
    x * x * (3.0 - 2.0 * x)
}

/// A value that starts at `from` and springs to each key's target at its time, carrying velocity
/// across retargets (as an interrupted Rune spring does).
pub fn track(t: f64, from: f32, keys: &[(f64, f32, Spring)]) -> f32 {
    let (mut x, mut v, mut target, mut t0, mut spring) = (from, 0.0f32, from, 0.0f64, Spring::new(0.3, 0.0));
    for &(tk, to, sp) in keys {
        if t < tk {
            break;
        }
        let (nx, nv) = spring.sample(x, v, target, (tk - t0) as f32);
        x = nx;
        v = nv;
        target = to;
        t0 = tk;
        spring = sp;
    }
    spring.sample(x, v, target, (t - t0) as f32).0
}

pub const fn sp(duration: f32, bounce: f32) -> Spring {
    Spring::new(duration, bounce)
}

/// An iPad-style round pointer at (x, y), centred, squeezed while pressed.
pub fn pointer(x: f32, y: f32, press: f32) -> El {
    let r = 9.0 * (1.0 - 0.22 * press);
    el().key("pointer")
        .absolute(x - r, y - r)
        .size(2.0 * r, 2.0 * r)
        .radius(r)
        .fill(ca(0xf4f4f6, 0.92))
        .border(1.0, ca(0x000000, 0.35))
        .shadow(2.0, 8.0, ca(0x000000, 0.45))
}

/// A pointer that follows `path` (time, x, y) on springs and presses at each time in `taps`.
pub fn pointer_at(t: f64, start: [f32; 2], path: &[(f64, f32, f32)], taps: &[f64]) -> El {
    let s = sp(0.55, 0.0);
    let kx: Vec<(f64, f32, Spring)> = path.iter().map(|&(tk, x, _)| (tk, x, s)).collect();
    let ky: Vec<(f64, f32, Spring)> = path.iter().map(|&(tk, _, y)| (tk, y, s)).collect();
    let x = track(t, start[0], &kx);
    let y = track(t, start[1], &ky);
    let k: Vec<(f64, f32, Spring)> = taps.iter().flat_map(|&tp| [(tp, 1.0, sp(0.12, 0.0)), (tp + 0.14, 0.0, sp(0.3, 0.4))]).collect();
    pointer(x, y, track(t, 0.0, &k))
}

/// Press depth (0 → 1 → 0) for a tap at `tp`: down fast, back on a bouncy spring.
pub fn pressed(t: f64, tp: f64) -> f32 {
    track(t, 0.0, &[(tp, 1.0, sp(0.12, 0.0)), (tp + 0.14, 0.0, sp(0.35, 0.45))])
}

pub fn avatar(letter: &str, color: u32, size: f32) -> El {
    el().size(size, size)
        .radius(size / 2.0)
        .fill(c(color))
        .center()
        .child(t(letter, size * 0.42, 600.0, 0x0b0b0d))
}

pub fn dot(color: u32, size: f32) -> El {
    el().size(size, size).radius(size / 2.0).fill(c(color))
}
