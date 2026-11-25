//! `Hsv`: a colour as hue, saturation and value in sRGB, with exact `#rrggbb` round-trips.

use rune::Color;
use rune::color::{srgb_decode, srgb_encode};

/// A colour as hue (degrees, `0.0..=360.0`), saturation and value (`0.0..=1.0`) of its
/// sRGB-encoded channels, as CSS and most colour pickers define HSV (HSB).
///
/// A gray has no hue of its own and black no saturation, so a picker that keeps an `Hsv`
/// rather than a [`Color`] keeps the hue and saturation its square shows while the user drags
/// through them ([`Self::keep_hue`]).
///
/// Hex codes round-trip exactly: `Hsv::from_hex(code).unwrap().hex()` gives `code` back in
/// its `#rrggbb` form.
///
/// ```
/// use rune_kit::Hsv;
///
/// let accent = Hsv::from_hex("#3366ff").unwrap();
/// assert_eq!(accent.hex(), "#3366ff");
/// assert_eq!(Hsv::from_color(accent.to_color()).hex(), "#3366ff");
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct Hsv {
    /// Hue in degrees: `0.0` red, `120.0` green, `240.0` blue, `360.0` red again.
    pub h: f32,
    /// Saturation, `0.0` (gray) to `1.0` (the pure hue).
    pub s: f32,
    /// Value (brightness), `0.0` (black) to `1.0`.
    pub v: f32,
}

fn unit(x: f32) -> f32 {
    if x.is_nan() { 0.0 } else { x.clamp(0.0, 1.0) }
}

impl Hsv {
    /// `h` wraps into `0.0..360.0`; `s` and `v` clamp to `0.0..=1.0`. NaN becomes `0.0`.
    pub fn new(h: f32, s: f32, v: f32) -> Hsv {
        let h = if h.is_finite() { h.rem_euclid(360.0) } else { 0.0 };
        Hsv { h, s: unit(s), v: unit(v) }
    }

    /// From sRGB-encoded channels in `0.0..=1.0`. A gray gets hue `0.0`, black saturation `0.0`.
    pub fn from_srgb(rgb: [f32; 3]) -> Hsv {
        let [r, g, b] = rgb.map(|c| f64::from(unit(c)));
        let max = r.max(g).max(b);
        let d = max - r.min(g).min(b);
        let s = if max > 0.0 { d / max } else { 0.0 };
        let h = if d <= 0.0 {
            0.0
        } else if max == r {
            60.0 * ((g - b) / d).rem_euclid(6.0)
        } else if max == g {
            60.0 * ((b - r) / d + 2.0)
        } else {
            60.0 * ((r - g) / d + 4.0)
        };
        Hsv { h: h as f32, s: s as f32, v: max as f32 }
    }

    /// The sRGB-encoded channels in `0.0..=1.0`.
    pub fn to_srgb(self) -> [f32; 3] {
        let h = if self.h.is_finite() { f64::from(self.h).rem_euclid(360.0) / 60.0 } else { 0.0 };
        let (s, v) = (f64::from(unit(self.s)), f64::from(unit(self.v)));
        let c = v * s;
        let x = c * (1.0 - (h.rem_euclid(2.0) - 1.0).abs());
        let m = v - c;
        let (r, g, b) = match h as u32 {
            0 => (c, x, 0.0),
            1 => (x, c, 0.0),
            2 => (0.0, c, x),
            3 => (0.0, x, c),
            4 => (x, 0.0, c),
            _ => (c, 0.0, x),
        };
        [r + m, g + m, b + m].map(|c| c.clamp(0.0, 1.0) as f32)
    }

    /// From 8-bit sRGB channels.
    pub fn from_srgb8(rgb: [u8; 3]) -> Hsv {
        Hsv::from_srgb(rgb.map(|c| f32::from(c) / 255.0))
    }

    /// The nearest 8-bit sRGB channels. Exact for an `Hsv` made by [`Self::from_srgb8`].
    pub fn to_srgb8(self) -> [u8; 3] {
        self.to_srgb().map(|c| (c * 255.0).round() as u8)
    }

    /// `#rgb` or `#rrggbb`, the `#` optional, case-insensitive, surrounding spaces ignored.
    /// Anything else (including an alpha digit pair) is `None`.
    pub fn from_hex(code: &str) -> Option<Hsv> {
        let t = code.trim();
        let h = t.strip_prefix('#').unwrap_or(t);
        if !h.bytes().all(|b| b.is_ascii_hexdigit()) {
            return None;
        }
        let digit = |i: usize| u8::from_str_radix(&h[i..i + 1], 16).ok();
        let pair = |i: usize| u8::from_str_radix(&h[i..i + 2], 16).ok();
        let rgb = match h.len() {
            3 => [digit(0)? * 17, digit(1)? * 17, digit(2)? * 17],
            6 => [pair(0)?, pair(2)?, pair(4)?],
            _ => return None,
        };
        Some(Hsv::from_srgb8(rgb))
    }

    /// The colour's `#rrggbb` code, in lowercase.
    pub fn hex(self) -> String {
        let [r, g, b] = self.to_srgb8();
        format!("#{r:02x}{g:02x}{b:02x}")
    }

    /// From a linear [`Color`]; its alpha is dropped.
    pub fn from_color(c: Color) -> Hsv {
        Hsv::from_srgb([c.r, c.g, c.b].map(|x| srgb_encode(unit(x))))
    }

    /// The opaque linear [`Color`].
    pub fn to_color(self) -> Color {
        let [r, g, b] = self.to_srgb().map(srgb_decode);
        Color { r, g, b, a: 1.0 }
    }

    /// `self` with `from`'s hue when `self` is a gray or black (it has no hue of its own), and
    /// `from`'s saturation too when `self` is black. The colour itself doesn't change.
    pub fn keep_hue(self, from: Hsv) -> Hsv {
        let mut out = self;
        if self.s <= 0.0 || self.v <= 0.0 {
            out.h = from.h;
        }
        if self.v <= 0.0 {
            out.s = from.s;
        }
        out
    }
}

impl From<Hsv> for Color {
    fn from(c: Hsv) -> Color {
        c.to_color()
    }
}

impl From<Color> for Hsv {
    fn from(c: Color) -> Hsv {
        Hsv::from_color(c)
    }
}
