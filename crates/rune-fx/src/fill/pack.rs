//! GPU instance and per-frame table packing for fill styles.

use super::*;

/// Linear sRGB to OKLab.
pub(super) fn oklab(c: Rgb) -> [f32; 3] {
    let [r, g, b] = c.0;
    let l = 0.412_221_46 * r + 0.536_332_55 * g + 0.051_445_995 * b;
    let m = 0.211_903_5 * r + 0.680_699_5 * g + 0.107_396_96 * b;
    let s = 0.088_302_46 * r + 0.281_718_85 * g + 0.629_978_7 * b;
    let (l, m, s) = (l.max(0.0).cbrt(), m.max(0.0).cbrt(), s.max(0.0).cbrt());
    [
        0.210_454_26 * l + 0.793_617_8 * m - 0.004_072_047 * s,
        1.977_998_5 * l - 2.428_592_2 * m + 0.450_593_7 * s,
        0.025_904_037 * l + 0.782_771_77 * m - 0.808_675_77 * s,
    ]
}

impl FillParams {
    /// Pack into `i` (kind is `KIND_FILL`); `None` when the fill draws nothing.
    pub(crate) fn pack(&self, i: &mut InstRaw, rm: bool, time: f32) -> Option<Rect> {
        let r = rect_of(i);
        if self.draws_nothing(r) {
            return None;
        }
        let fin = |x: f32, d: f32| if x.is_finite() { x } else { d };
        // Looping styles settle on one pose under reduced motion, and fold large times to keep f32 precision.
        i.t[0] = if rm { CALM_T } else { cpu::fold_t(time) };
        i.t[2] = self.style.index() as f32;
        i.aux = [self.border, self.intensity.min(1.0), 0.0, 0.0];
        match self.style {
            FillStyle::LiquidMetal(p) => {
                i.c0 = p.tint.v4();
                i.c1 = p.light.v4();
                i.p0 = [fin(p.flow, 0.0).max(0.0), fin(p.scale, 100.0).max(1.0), fin(p.speed, 0.0), fin(p.bevel, 0.0).max(0.0)];
                i.p1 = [fin(p.dome, 0.0), fin(p.angle, 0.0), fin(p.iridescence, 0.0).max(0.0), fin(p.contrast, 1.0).max(0.1)];
                i.p2 = [(p.anchor == FillAnchor::Element) as u32 as f32, 0.0, 0.0, 0.0];
            }
            FillStyle::MeshGradient(p) => {
                i.p0 = [p.drift, p.speed, p.sharpness.clamp(0.1, 8.0), p.warp];
            }
            FillStyle::Dither(p) => {
                let pat = match p.pattern {
                    DitherPattern::Bayer2 => 1.0,
                    DitherPattern::Bayer4 => 2.0,
                    DitherPattern::Bayer8 => 3.0,
                    DitherPattern::BlueNoise => 4.0,
                    DitherPattern::White => 5.0,
                };
                i.p0 = [pat, p.input.code(), p.pixel.max(0.25), p.angle];
                i.p1 = [p.scale.max(1.0), p.speed, p.bias, p.contrast];
            }
            FillStyle::GrainGradient(p) => {
                let kind = match p.kind {
                    GradientKind::Linear => 0.0,
                    GradientKind::Radial => 1.0,
                    GradientKind::Conic => 2.0,
                };
                i.p0 = [kind, p.angle, p.center[0], p.center[1]];
                i.p1 = [p.amount, p.size.max(0.25), p.mono as u32 as f32, if rm { 0.0 } else { p.fps }];
            }
            FillStyle::FlutedGlass(p) => {
                i.c0 = p.tint.v4();
                i.p0 = [p.rib.max(1.0), p.angle, p.refraction, p.frost];
                i.p1 = [p.dispersion, p.highlight, p.tint_amount, 0.0];
            }
            FillStyle::Metaballs(p) => {
                i.c0 = p.base.v4();
                i.p0 = [p.count.clamp(1, 8) as f32, p.size, p.speed, p.gloss];
            }
            FillStyle::Swirl(p) => {
                i.p0 = [p.scale.max(1.0), p.speed, p.twist, p.warp];
            }
            FillStyle::Voronoi(p) => {
                i.c0 = p.edge_color.v4();
                i.p0 = [p.scale.max(2.0), p.speed, p.edge, p.depth];
            }
            FillStyle::Waves(p) => {
                i.p0 = [p.amplitude, p.frequency, p.speed, p.angle];
                i.p1 = [p.softness, 0.0, 0.0, 0.0];
            }
            FillStyle::Halftone(p) => {
                i.c0 = p.ink.v4();
                i.c1 = p.base.v4();
                i.p0 = [0.0, p.input.code(), p.cell.max(1.0), p.ramp_angle];
                i.p1 = [p.scale.max(1.0), p.speed, p.bias, p.contrast];
                i.p2 = [p.angle, 0.0, 0.0, 0.0];
            }
            FillStyle::Plastic(p) => {
                i.c0 = p.color.v4();
                i.p0 = [fin(p.roughness, 0.15).clamp(0.0, 1.0), fin(p.dome, 0.0), fin(p.bevel, 0.0).max(0.0), fin(p.angle, 0.0)];
            }
        }
        Some(r.inflate(1.0))
    }

    /// Per-frame tables: colour stops (linear, then OKLab; mesh points ride in `w`), metaball
    /// paths and the blue-noise tile. `xf` = (stops offset, stop count, extra offset, extra count).
    pub(crate) fn tables(&self, i: &mut InstRaw, rect: Rect, t: f32, seed: u32, data: &mut Vec<[f32; 4]>) {
        let stops = |data: &mut Vec<[f32; 4]>, s: &Stops, pts: Option<&[[f32; 2]]>| -> [f32; 2] {
            let first = data.len() as f32;
            for (k, c) in s.as_slice().iter().enumerate() {
                let p = pts.map_or([0.0, 0.0], |v| v[k]);
                let l = oklab(*c);
                data.push([c.0[0], c.0[1], c.0[2], p[0]]);
                data.push([l[0], l[1], l[2], p[1]]);
            }
            [first, s.len() as f32]
        };
        let [f, n] = match &self.style {
            FillStyle::MeshGradient(p) => {
                let pts = cpu::mesh_points(p, t, seed);
                stops(data, &p.colors, Some(&pts))
            }
            FillStyle::Dither(p) => stops(data, &p.colors, None),
            FillStyle::GrainGradient(p) => stops(data, &p.colors, None),
            FillStyle::Metaballs(p) => stops(data, &p.colors, None),
            FillStyle::Swirl(p) => stops(data, &p.colors, None),
            FillStyle::Voronoi(p) => stops(data, &p.colors, None),
            FillStyle::Waves(p) => stops(data, &p.colors, None),
            _ => {
                // Glass leaves `xf` to `blur_backdrop`, which marks the frost pyramid there.
                return;
            }
        };
        i.xf = [f, n, 0.0, 0.0];
        match &self.style {
            FillStyle::Dither(p) if p.pattern == DitherPattern::BlueNoise => {
                i.xf[2] = data.len() as f32;
                i.xf[3] = cpu::BLUE_N as f32;
                data.extend_from_slice(cpu::blue_noise());
            }
            FillStyle::Metaballs(p) => {
                i.xf[2] = data.len() as f32;
                i.xf[3] = p.count.clamp(1, 8) as f32;
                cpu::metaballs(p, rect, t, seed, data);
            }
            _ => {}
        }
    }
}

fn rect_of(i: &InstRaw) -> Rect {
    Rect::new(i.rect[0], i.rect[1], i.rect[2], i.rect[3])
}
