//! The look both builds share: colours as OKLCH, the grid's measurements, and the springs, as
//! perceptual duration and bounce (SwiftUI's `Spring(duration:bounce:)`).

/// An OKLCH colour with alpha.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Lch {
    pub l: f32,
    pub c: f32,
    pub h: f32,
    pub a: f32,
}

pub const fn lch(l: f32, c: f32, h: f32) -> Lch {
    Lch { l, c, h, a: 1.0 }
}

impl Lch {
    pub const fn alpha(self, a: f32) -> Lch {
        Lch { a, ..self }
    }

    /// OKLab (L, a, b): the space both builds interpolate colour in.
    pub fn lab(self) -> [f32; 3] {
        let h = self.h.to_radians();
        [self.l, self.c * h.cos(), self.c * h.sin()]
    }

    /// Gamma-encoded sRGB, clamped to the gamut, with alpha.
    pub fn srgb(self) -> [f32; 4] {
        let [r, g, b] = lab_to_linear(self.lab());
        [encode(r), encode(g), encode(b), self.a]
    }
}

pub fn lab_to_linear([l, a, b]: [f32; 3]) -> [f32; 3] {
    let l_ = l + 0.396_337_78 * a + 0.215_803_76 * b;
    let m_ = l - 0.105_561_346 * a - 0.063_854_17 * b;
    let s_ = l - 0.089_484_18 * a - 1.291_485_5 * b;
    let (l3, m3, s3) = (l_ * l_ * l_, m_ * m_ * m_, s_ * s_ * s_);
    [
        4.076_741_7 * l3 - 3.307_711_6 * m3 + 0.230_969_94 * s3,
        -1.268_438 * l3 + 2.609_757_4 * m3 - 0.341_319_38 * s3,
        -0.004_196_086_3 * l3 - 0.703_418_6 * m3 + 1.707_614_7 * s3,
    ]
}

/// Linear to gamma-encoded sRGB, clamped to 0..1.
pub fn encode(x: f32) -> f32 {
    let x = x.clamp(0.0, 1.0);
    if x <= 0.003_130_8 { 12.92 * x } else { 1.055 * x.powf(1.0 / 2.4) - 0.055 }
}

pub mod color {
    use super::{Lch, lch};

    pub const ACCENT_HUE: f32 = 245.0;
    pub const BG: Lch = lch(0.155, 0.012, 260.0);
    pub const TILE: Lch = lch(0.205, 0.014, 258.0);
    pub const TILE_FOCUS: Lch = lch(0.235, 0.022, 252.0);
    pub const EDGE: Lch = lch(1.0, 0.0, 0.0).alpha(0.07);
    pub const WELL: Lch = lch(0.255, 0.014, 258.0);
    pub const WELL_HI: Lch = lch(0.31, 0.016, 258.0);
    pub const TEXT: Lch = lch(0.96, 0.005, 258.0);
    pub const TEXT_2: Lch = lch(0.72, 0.012, 258.0);
    pub const TEXT_3: Lch = lch(0.54, 0.014, 258.0);
    pub const ACCENT: Lch = lch(0.66, 0.17, ACCENT_HUE);
    pub const ACCENT_DIM: Lch = lch(0.66, 0.17, ACCENT_HUE).alpha(0.18);
    pub const ON_ACCENT: Lch = lch(0.99, 0.0, 0.0);
    pub const GOOD: Lch = lch(0.74, 0.15, 155.0);
    pub const WARM: Lch = lch(0.78, 0.14, 70.0);

    /// Each scene's mood: its swatch, and the glow it casts on the header.
    pub const SCENE: [Lch; 4] = [
        lch(0.66, 0.17, ACCENT_HUE),
        lch(0.74, 0.13, 175.0),
        lch(0.78, 0.14, 70.0),
        lch(0.6, 0.17, 295.0),
    ];
}

/// The window and the grid, in points.
pub mod grid {
    pub const WIDTH: f32 = 1440.0;
    pub const HEIGHT: f32 = 900.0;
    pub const PAD: f32 = 28.0;
    pub const GAP: f32 = 14.0;
    pub const HEADER: f32 = 64.0;
    pub const RADIUS: f32 = 18.0;
    pub const TILE_PAD: f32 = 18.0;
    /// Row heights below the header; the dock row takes what's left.
    pub const ROW1: f32 = 236.0;
    pub const ROW2: f32 = 262.0;
    /// The focused dock tile's share of the row against 1 for each other tile.
    pub const DOCK_FOCUS: f32 = 2.4;
}

/// A spring as perceptual duration (seconds) and bounce, mass 1.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Spring {
    pub duration: f32,
    pub bounce: f32,
}

impl Spring {
    pub const fn new(duration: f32, bounce: f32) -> Spring {
        Spring { duration, bounce }
    }

    /// ω = 2π / duration.
    pub fn omega(self) -> f32 {
        std::f32::consts::TAU / self.duration
    }

    /// ζ = 1 − bounce (≥ 0), 1 / (1 + bounce) below.
    pub fn zeta(self) -> f32 {
        if self.bounce >= 0.0 { 1.0 - self.bounce } else { 1.0 / (1.0 + self.bounce) }
    }

    /// k = ω².
    pub fn stiffness(self) -> f32 {
        self.omega() * self.omega()
    }

    /// c = 2ζω.
    pub fn damping(self) -> f32 {
        2.0 * self.zeta() * self.omega()
    }
}

/// The springs each kind of motion uses.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Springs {
    /// Tiles reflowing, cards and feed rows moving.
    pub layout: Spring,
    /// Controls: switches, segments, chips filling.
    pub snap: Spring,
    /// Shape and colour changes.
    pub morph: Spring,
    /// Things arriving with a little life.
    pub pop: Spring,
    /// The XY pad's puck and the mixer's faders.
    pub travel: Spring,
}

impl Springs {
    /// `default` is the shipped tuning. `bouncy` and `stiff` exist for A/B runs.
    pub fn variant(name: &str) -> Springs {
        let base = Springs {
            layout: Spring::new(0.4, 0.0),
            snap: Spring::new(0.25, 0.0),
            morph: Spring::new(0.3, 0.16),
            pop: Spring::new(0.25, 0.15),
            travel: Spring::new(0.35, 0.12),
        };
        match name {
            "bouncy" => Springs {
                layout: Spring::new(0.5, 0.28),
                snap: Spring::new(0.3, 0.22),
                morph: Spring::new(0.45, 0.35),
                pop: Spring::new(0.35, 0.35),
                travel: Spring::new(0.5, 0.35),
            },
            "stiff" => Springs {
                layout: Spring::new(0.22, 0.0),
                snap: Spring::new(0.15, 0.0),
                morph: Spring::new(0.18, 0.0),
                pop: Spring::new(0.15, 0.0),
                travel: Spring::new(0.2, 0.0),
            },
            _ => base,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accent_is_rune_blue() {
        let [r, g, b, _] = color::ACCENT.srgb();
        assert!(b > r && b > g && r < 0.4, "{r} {g} {b}");
    }

    #[test]
    fn spring_constants_match_swiftui() {
        let s = Spring::new(0.5, 0.0);
        assert!((s.stiffness() - (4.0 * std::f32::consts::PI).powi(2)).abs() < 1e-3);
        assert!((s.damping() - 2.0 * 4.0 * std::f32::consts::PI).abs() < 1e-3);
    }
}
