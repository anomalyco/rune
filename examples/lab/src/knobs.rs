//! Scenes and their knobs. Effect knobs take their range and default from the effect's typed
//! params (`GlowParams::KNOBS`, …); lighting knobs are declared here.

use std::sync::OnceLock;

use rune_fx::{ElectricParams, FireParams, GlowParams, Knob, MotionBlurParams, PulseParams};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Scene {
    Lamp,
    Flashlight,
    Gi,
    Keys,
    GodRays,
    Glow,
    Pulses,
    Electric,
    Fire,
    Motion,
    Lens,
    Extras,
}

pub const ALL: [Scene; 12] = [
    Scene::Lamp,
    Scene::Flashlight,
    Scene::Gi,
    Scene::Keys,
    Scene::GodRays,
    Scene::Glow,
    Scene::Pulses,
    Scene::Electric,
    Scene::Fire,
    Scene::Motion,
    Scene::Lens,
    Scene::Extras,
];

impl Scene {
    pub const fn idx(self) -> usize {
        self as usize
    }

    /// Lit by rune-light (the others composite rune-fx layers).
    pub fn lit(self) -> bool {
        self.idx() < 5
    }

    fn meta(self) -> (&'static str, &'static str, &'static str) {
        match self {
            Scene::Lamp => ("lamp", "Lamp", "Drag the lamp. Every bubble casts a soft shadow from the analytic field."),
            Scene::Flashlight => ("flashlight", "Flashlight", "Move the pointer. The torch is held low and to the right, so shadows swing."),
            Scene::Gi => ("gi", "Emissive light", "Glowing elements light their neighbours. Click the record button or the progress bar."),
            Scene::Keys => ("keys", "Raised keys", "Move the pointer to sweep the light. Press a key."),
            Scene::GodRays => ("godrays", "God rays", "Move the pointer. The light sits behind the grid and shines through the gaps."),
            Scene::Glow => ("glow", "Glow", "Click the neon card to switch it off: lights cool, never cut."),
            Scene::Pulses => ("pulses", "Pulses", "Click anywhere for a ripple. Press send to ping the inbox."),
            Scene::Electric => ("electric", "Electricity", "Hover the charged button."),
            Scene::Fire => ("fire", "Fire", "Flames, a plasma border and heat haze over a button."),
            Scene::Motion => ("motion", "Motion blur", "Drag the card and fling it. It glides on a spring with shutter blur."),
            Scene::Lens => ("lens", "Liquid glass", "Drag the glass over the page. Click anywhere for a shockwave."),
            Scene::Extras => ("extras", "Extras", "Shimmer, sparkles, aurora and grain."),
        }
    }

    pub fn id(self) -> &'static str { self.meta().0 }
    pub fn title(self) -> &'static str { self.meta().1 }
    pub fn hint(self) -> &'static str { self.meta().2 }
}

#[derive(Clone, Copy, Debug)]
pub enum Kind {
    /// Decimal places shown.
    Slider(u8),
    Toggle,
    /// sRGB hex colours; the value is the index.
    Swatch(&'static [u32]),
}

#[derive(Clone, Copy, Debug)]
pub struct KnobSpec {
    pub id: &'static str,
    pub label: &'static str,
    pub min: f32,
    pub max: f32,
    pub default: f32,
    pub kind: Kind,
}

pub const FX_COLORS: &[u32] = &[0x22d3ee, 0x8b5cf6, 0xff5fa2, 0xff8a2a, 0x34d399, 0x60a5fa];
pub const LAMP_COLORS: &[u32] = &[0xffc78c, 0xff9a4d, 0xfff3e6, 0xb3ccff, 0xff8fb0, 0x9dffd0];

fn sl(id: &'static str, label: &'static str, min: f32, max: f32, default: f32, dec: u8) -> KnobSpec {
    KnobSpec { id, label, min, max, default, kind: Kind::Slider(dec) }
}

fn tg(id: &'static str, label: &'static str, on: bool) -> KnobSpec {
    KnobSpec { id, label, min: 0.0, max: 1.0, default: if on { 1.0 } else { 0.0 }, kind: Kind::Toggle }
}

fn sw(id: &'static str, label: &'static str, palette: &'static [u32]) -> KnobSpec {
    KnobSpec { id, label, min: 0.0, max: (palette.len() - 1) as f32, default: 0.0, kind: Kind::Swatch(palette) }
}

/// A slider over an effect's own knob: its range and default come from the typed params.
fn fx(knobs: &'static [Knob], name: &'static str, label: &'static str, dec: u8) -> KnobSpec {
    let k = knobs.iter().find(|k| k.name == name).unwrap_or_else(|| panic!("no knob {name}"));
    KnobSpec { id: k.name, label, min: k.min, max: k.max, default: k.default, kind: Kind::Slider(dec) }
}

pub fn specs(scene: Scene) -> &'static [KnobSpec] {
    static ALL_SPECS: OnceLock<Vec<Vec<KnobSpec>>> = OnceLock::new();
    &ALL_SPECS.get_or_init(|| ALL.iter().map(|s| build(*s)).collect())[scene.idx()]
}

fn build(scene: Scene) -> Vec<KnobSpec> {
    let g = GlowParams::KNOBS;
    let p = PulseParams::KNOBS;
    let e = ElectricParams::KNOBS;
    let f = FireParams::KNOBS;
    let m = MotionBlurParams::KNOBS;
    match scene {
        Scene::Lamp => vec![
            sl("height", "Lamp height", 10.0, 240.0, 40.0, 0),
            sl("radius", "Source size", 2.0, 40.0, 11.0, 0),
            sl("intensity", "Intensity", 0.0, 6.0, 2.6, 1),
            sw("color", "Colour", LAMP_COLORS),
            tg("fill", "Fill light", true),
            tg("bounce", "Bounce light (GI)", false),
        ],
        Scene::Flashlight => vec![
            sl("beam", "Beam angle", 0.2, 1.2, 0.58, 2),
            sl("height", "Torch height", 60.0, 420.0, 260.0, 0),
            sl("radius", "Source size", 2.0, 30.0, 9.0, 0),
            sl("intensity", "Intensity", 0.0, 14.0, 7.0, 1),
            sl("ambient", "Ambient", 0.0, 0.4, 0.07, 2),
        ],
        Scene::Gi => vec![
            tg("gi", "Global illumination", true),
            sl("strength", "GI strength", 0.0, 3.0, 1.0, 2),
            sl("air", "Air scatter", 0.0, 0.3, 0.09, 2),
            sl("bounce", "Bounce", 0.0, 1.0, 0.0, 2),
            sl("emission", "Emission", 0.0, 3.0, 1.0, 2),
            sl("ambient", "Ambient", 0.0, 1.0, 0.3, 2),
        ],
        Scene::Keys => vec![
            sl("height", "Light height", 30.0, 400.0, 150.0, 0),
            sl("radius", "Source size", 4.0, 60.0, 26.0, 0),
            sl("bevel", "Bevel", 0.3, 2.0, 1.0, 2),
            sl("gloss", "Gloss", 0.0, 1.0, 0.85, 2),
            tg("fill", "Fill light", true),
        ],
        Scene::GodRays => vec![
            sl("shafts", "Shafts", 0.0, 3.0, 1.5, 2),
            sl("intensity", "Intensity", 0.0, 4.0, 1.6, 2),
            sl("softness", "Softness", 0.5, 4.0, 2.2, 2),
            sl("height", "Light height", 1.0, 40.0, 4.0, 0),
        ],
        Scene::Glow => vec![
            fx(g, "radius", "Radius", 0),
            fx(g, "intensity", "Intensity", 2),
            fx(g, "breathe", "Breathe", 2),
            fx(g, "rim_width", "Rim width", 2),
            fx(g, "hot", "Hot core", 2),
            sw("color", "Neon colour", FX_COLORS),
        ],
        Scene::Pulses => vec![
            fx(p, "reach", "Reach", 0),
            fx(p, "width", "Ring width", 1),
            fx(p, "period", "Period", 1),
            fx(p, "count", "Rings", 0),
            fx(p, "intensity", "Intensity", 2),
            sw("color", "Colour", FX_COLORS),
        ],
        Scene::Electric => vec![
            fx(e, "amplitude", "Amplitude", 1),
            fx(e, "frequency", "Frequency", 1),
            fx(e, "speed", "Speed", 1),
            fx(e, "glow", "Glow", 1),
            fx(e, "forks", "Forks", 2),
            sw("color", "Colour", FX_COLORS),
        ],
        Scene::Fire => vec![
            fx(f, "height", "Flame height", 0),
            fx(f, "scale", "Scale", 0),
            fx(f, "speed", "Speed", 2),
            fx(f, "embers", "Embers", 0),
            fx(f, "intensity", "Intensity", 2),
            fx(f, "swirl", "Swirl", 2),
        ],
        Scene::Motion => vec![
            fx(m, "shutter_angle", "Shutter angle", 0),
            sl("glide", "Glide duration", 0.3, 1.4, 0.6, 2),
            tg("smooth", "Soft shutter ends", true),
            tg("directional", "Directional fallback", false),
        ],
        Scene::Lens => vec![
            sl("depth", "Depth", 0.0, 40.0, 16.0, 0),
            sl("bevel", "Bevel", 2.0, 40.0, 14.0, 0),
            sl("dispersion", "Dispersion", 0.0, 0.5, 0.14, 2),
            sl("magnification", "Magnification", 0.9, 1.3, 1.04, 2),
            sl("frost", "Frost", 0.0, 1.0, 0.0, 2),
            tg("crt", "CRT terminal", true),
        ],
        Scene::Extras => vec![
            tg("shimmer", "Shimmer", true),
            tg("aurora", "Aurora", true),
            tg("grain", "Grain", true),
            tg("sparkles", "Sparkles", true),
            sl("speed", "Aurora speed", 0.0, 1.0, 0.25, 2),
        ],
    }
}

pub fn defaults() -> Vec<Vec<f32>> {
    ALL.iter().map(|s| specs(*s).iter().map(|k| k.default).collect()).collect()
}

/// Linear RGB of an sRGB hex colour.
pub fn lin(hex: u32) -> [f32; 3] {
    let f = |c: u32| rune::color::srgb_decode(c as f32 / 255.0);
    [f((hex >> 16) & 255), f((hex >> 8) & 255), f(hex & 255)]
}
