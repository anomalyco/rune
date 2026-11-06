//! The compositor: every `El::effect` slot becomes one rune-fx layer. The slot's mark says which
//! effect, its `effect_data` carries the effect's few live numbers, and its presented rect and
//! radius are the shape. Glass, lenses and shockwaves read a backdrop copy taken at their slot.

use rune::compose::{Compositor, Frame};
use rune::render::{EffectPhase, EffectSite, KIND_EFFECT};
use rune_fx::*;

use crate::State;

pub const CHROME: u32 = 1;
pub const MESH: u32 = 2;
pub const FLUTED: u32 = 3;
pub const FIRE: u32 = 4;
pub const ELECTRIC: u32 = 5;
pub const ARC: u32 = 6;
pub const GLOW: u32 = 7;
pub const SPARKLES: u32 = 8;
pub const SHOCK: u32 = 9;
pub const LENS: u32 = 10;
pub const PLASTIC: u32 = 11;
pub const PULSE: u32 = 12;
pub const SHIMMER: u32 = 13;
pub const GRAIN: u32 = 14;
pub const AURORA: u32 = 15;
pub const HAZE: u32 = 16;
pub const GRAIN_GRADIENT: u32 = 17;

/// Effect colours, by index (effect data carries the index).
pub const COLORS: [u32; 14] = [
    0xffffff, 0x4f8dff, 0x22d3ee, 0x7c5cff, 0x34d399, 0xffb547, 0xff5ca8, 0xff4d4d, 0xff7a1a, 0xf1ece1, 0x2e3036, 0xe8552b,
    0x3d6bff, 0x54b3fd,
];

fn col(i: f32) -> Rgb {
    rgb(COLORS[(i.max(0.0) as usize).min(COLORS.len() - 1)])
}

fn layer(kind: u32, d: [f32; 4], shape: Shape, t: f32) -> Option<FxLayer> {
    let fx = match kind {
        CHROME => {
            let mut p = match d[0] as u32 {
                1 => LiquidMetalParams::gold(),
                2 => LiquidMetalParams::rose(),
                3 => LiquidMetalParams::gunmetal(),
                4 => LiquidMetalParams { light: rgb(0xd6e8ff), iridescence: 0.3, scale: 240.0, bevel: 3.5, dome: 0.2, contrast: 1.15, ..LiquidMetalParams::chrome() },
                5 => LiquidMetalParams { scale: 360.0, flow: 0.5, speed: 0.6, dome: 0.8, contrast: 0.85, bevel: 12.0, angle: 0.45, ..LiquidMetalParams::chrome() },
                _ => LiquidMetalParams::chrome(),
            };
            if d[2] > 0.0 {
                p.flow *= d[2];
            }
            if d[3] > 0.5 {
                p = p.anchor(FillAnchor::Element);
            }
            if d[1] > 0.0 { Fx::fill(p.border(d[1])) } else { Fx::fill(p) }
        }
        MESH => {
            let mut p = match d[0] as u32 {
                1 => MeshGradientParams::lagoon(),
                2 => MeshGradientParams::orchid(),
                3 => MeshGradientParams::graphite(),
                4 => MeshGradientParams { colors: Stops::hex(&[0x0d1b4c, 0x3d6bff, 0x9b5cff, 0x22d3ee, 0x101030]), ..MeshGradientParams::dusk() },
                5 => MeshGradientParams { colors: Stops::hex(&[0x2a0a12, 0xff4d4d, 0xffb547, 0x7a1640, 0xff7a1a]), ..MeshGradientParams::dusk() },
                6 => MeshGradientParams { colors: Stops::hex(&[0x062a24, 0x34d399, 0xd9f99d, 0x0f766e]), ..MeshGradientParams::dusk() },
                7 => MeshGradientParams { colors: Stops::hex(&[0x1a1033, 0xff5ca8, 0x7c5cff, 0xffd1e8, 0x2b1b5a]), ..MeshGradientParams::dusk() },
                _ => MeshGradientParams::dusk(),
            };
            if d[1] > 0.0 {
                p.speed = d[1];
            }
            if d[3] > 0.0 {
                p.drift = d[3];
            }
            return Some(Fx::fill(p).on(shape).at(t).seed(d[2] as u32 + 1));
        }
        FLUTED => Fx::fill(FlutedGlassParams { rib: d[0], refraction: d[1], frost: d[2], dispersion: d[3], highlight: 0.55, ..FlutedGlassParams::reeded() }),
        FIRE => {
            let style = match d[2] as u32 {
                1 => FireStyle::Border,
                2 => FireStyle::Heat,
                3 => FireStyle::Embers,
                _ => FireStyle::Top,
            };
            Fx::fire(FireParams { style, height: d[0], intensity: d[1], smoke: d[3], embers: 24, scale: 26.0, ..FireParams::default() })
        }
        ELECTRIC => {
            let base = if d[2] > 0.5 { ElectricParams::charged() } else { ElectricParams::default() };
            Fx::electric(ElectricParams { intensity: d[0], amplitude: if d[1] > 0.0 { d[1] } else { base.amplitude }, color: col(d[3]), ..base })
        }
        ARC => Fx::electric(ElectricParams { style: ElectricStyle::Arc { a: [d[0], d[1]], b: [d[2], d[3]] }, amplitude: 9.0, glow: 7.0, intensity: 1.2, ..Default::default() }),
        GLOW => {
            let style = match d[0] as u32 {
                1 => GlowStyle::Neon,
                2 => GlowStyle::Aura,
                _ => GlowStyle::Soft,
            };
            Fx::glow(GlowParams { style, color: col(d[1]), intensity: d[2], radius: if d[3] > 0.0 { d[3] } else { 14.0 }, aura_speed: 0.35, ..Default::default() })
        }
        SPARKLES => Fx::sparkles(SparkleParams { burst: (d[0] >= 0.0).then_some(d[0]), count: d[1] as u32, size: d[2], color: col(d[3]), margin: 26.0, ..Default::default() }),
        SHOCK => Fx::distort(DistortParams::Shockwave { at: [d[0], d[1]], reach: 1100.0, width: 90.0, amount: d[3], chroma: 0.6, t0: d[2], duration: 1.1 }),
        LENS => Fx::lens(LensParams {
            second: (d[2] > 0.0).then(|| Shape::circle(d[0], d[1], d[2])),
            frost: d[3],
            bevel: 16.0,
            depth: 20.0,
            dispersion: 0.18,
            magnification: 1.06,
            union_k: 26.0,
            ..Default::default()
        }),
        PLASTIC => Fx::fill(PlasticParams { color: col(d[0]), roughness: d[1], dome: d[2], bevel: if d[3] > 0.0 { d[3] } else { 6.0 }, angle: -0.6 }),
        PULSE => {
            let style = match d[0] as u32 {
                1 => PulseStyle::Breathe,
                2 => PulseStyle::Heartbeat,
                _ => PulseStyle::Radar,
            };
            Fx::pulse(PulseParams { style, color: col(d[1]), reach: d[3], period: if d[2] > 0.0 { d[2] } else { 2.4 }, ..Default::default() })
        }
        SHIMMER => Fx::shimmer(ShimmerParams { intensity: d[0], period: d[1], band: d[2], ..Default::default() }),
        GRAIN => Fx::grain(GrainParams { amount: d[0], fps: d[1], ..Default::default() }),
        AURORA => Fx::aurora(AuroraParams { intensity: d[0], speed: d[1], ..Default::default() }),
        HAZE => Fx::distort(DistortParams::HeatHaze { height: d[0], scale: 14.0, speed: 1.4, amount: d[1], whole_shape: d[2] > 0.5 }),
        GRAIN_GRADIENT => Fx::fill(GrainGradientParams { kind: GradientKind::Radial, colors: Stops::hex(&[0x1b1530, 0x0a0a0c]), amount: 0.12, ..GrainGradientParams::default() }),
        _ => return None,
    };
    Some(fx.on(shape).at(t))
}

#[derive(Default)]
pub struct Gfx {
    fx: Option<FxRenderer>,
}

impl Compositor<State> for Gfx {
    fn animating(&self) -> bool {
        true
    }

    fn render(&mut self, f: &mut Frame<'_, State>) {
        let (device, queue, size) = (f.device, f.queue, f.size);
        let t = f.rt.t as f32;
        let marks = &f.rt.marks;
        let mut at: Vec<u32> = Vec::new();
        let mut layers: Vec<FxLayer> = Vec::new();
        let mut placements: Vec<FxSlot> = Vec::new();
        let mut fire = false;
        for (n, i) in f.rt.instances.iter().enumerate() {
            if i.params[1] != KIND_EFFECT {
                continue;
            }
            let kind = marks.get(n).copied().unwrap_or(0) & 0xff;
            let k = i.params[3];
            let [x, y, w, h] = i.rect.map(|v| v / k);
            let r = i.radii.iter().fold(0.0f32, |a, v| a.max(v.abs())) / k;
            let Some(l) = layer(kind, i.uv, Shape::rounded(x, y, w, h, r), t) else { continue };
            fire |= kind == FIRE;
            at.push(n as u32);
            layers.push(l);
            placements.push(FxSlot::default().scale(k).clip(i.clip, i.fade));
        }
        let fx = self.fx.get_or_insert_with(|| {
            let mut fx = FxRenderer::new(device, f.format, FxOptions { dither: true });
            fx.set_fire_sim(device, true);
            fx
        });
        fx.set_srgb_blend(f.srgb_blend());
        fx.add_format(device, wgpu::TextureFormat::Rgba16Float);
        fx.prepare_with_slots(device, queue, &FxFrame { viewport: [size.0, size.1], scale: 1.0, layers: &layers, backdrop: None, sources: &[] }, &placements);
        if fire {
            fx.simulate(queue, f.encoder, None);
        }
        let clear = f.rt.clear_color();
        f.ui.prepare(device, queue, &mut f.rt.text.atlas, &f.rt.instances, size);
        f.ui.draw_with_backdrop(f.encoder, f.target, clear, &mut |phase, s: &EffectSite<'_>| {
            let Some(k) = at.iter().position(|n| *n == s.index) else { return };
            match phase {
                EffectPhase::Backdrop(enc) => {
                    if let Some(bd) = s.backdrop {
                        fx.blur_backdrop(device, queue, enc, bd, k);
                    }
                }
                EffectPhase::Draw(pass) => fx.draw_layer_in(pass, s.format, s.origin, k),
            }
        });
    }
}
