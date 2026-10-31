//! The fill catalog: each fill style's id, display name, params (with ranges and units) and named
//! presets, plus generic get/set by param id and Rust emission. A tool can build its style list,
//! inspector and code output from this instead of keeping its own schema.
//!
//! Param ids are the Rust field names, so `FillParams::to_rust` can write struct literals.

use crate::*;

/// How a number reads in an inspector.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Unit {
    None,
    /// Points.
    Pt,
    /// Radians (show as degrees).
    Radians,
    /// 0..1 of something (show as a percentage).
    Fraction,
    /// Per second (speeds, rates).
    PerSecond,
}

/// One option of a choice param.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Choice {
    pub id: &'static str,
    pub name: &'static str,
    /// The Rust path of the variant, for emitted code.
    pub rust: &'static str,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ParamKind {
    /// A float; `step` is a sensible slider step.
    Number { min: f32, max: f32, step: f32, unit: Unit },
    /// A whole number.
    Count { min: u32, max: u32 },
    Toggle,
    Choice(&'static [Choice]),
    Color,
    /// An ordered colour list (`Stops`).
    Colors { min: usize, max: usize },
    /// A point in the shape's rect, 0..1 on each axis.
    Point,
    /// Up to `max` points in the shape's rect (one per colour of a `Colors` param).
    Points { max: usize },
}

/// A tunable param. Its default is whatever the style's `Default` (or a preset) holds: read it
/// with `FillParams::get`.
#[derive(Clone, Copy, Debug, PartialEq)]
#[non_exhaustive]
pub struct Param {
    /// The Rust field name.
    pub id: &'static str,
    pub name: &'static str,
    pub kind: ParamKind,
}

/// A param's value.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Value {
    Number(f32),
    Count(u32),
    Toggle(bool),
    /// Index into the param's `Choice` list.
    Choice(usize),
    Color(Rgb),
    Colors(Stops),
    Point([f32; 2]),
    Points([[f32; 2]; MAX_STOPS]),
}

/// A named starting point for a style.
#[derive(Clone, Copy)]
pub struct Preset {
    pub id: &'static str,
    pub name: &'static str,
    /// The Rust expression that builds it, for emitted code.
    pub rust: &'static str,
    pub(crate) make: fn() -> FillStyle,
}

impl Preset {
    pub fn style(&self) -> FillStyle {
        (self.make)()
    }
    pub fn fill(&self) -> FillParams {
        FillParams::new(self.style())
    }
}

impl std::fmt::Debug for Preset {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Preset").field("id", &self.id).finish()
    }
}

/// A fill style as a catalog entry.
#[derive(Clone, Copy, Debug)]
#[non_exhaustive]
pub struct StyleInfo {
    /// Stable id, snake_case (`"liquid_metal"`).
    pub id: &'static str,
    pub name: &'static str,
    pub summary: &'static str,
    /// The params type, for emitted code.
    pub rust: &'static str,
    /// The style's own params, in inspector order. `FillParams::COMMON` applies to every style.
    pub params: &'static [Param],
    /// Named presets; the first is the style's `Default`.
    pub presets: &'static [Preset],
    /// Whether the style may read the backdrop (always for glass; for some inputs otherwise).
    /// `Fx::needs_backdrop` gives the answer for a given set of params.
    pub reads_backdrop: bool,
}

impl StyleInfo {
    pub fn preset(&self, id: &str) -> Option<&'static Preset> {
        self.presets.iter().find(|p| p.id == id)
    }
    /// The style with its default params.
    pub fn default_fill(&self) -> FillParams {
        self.presets[0].fill()
    }
}

/// Every fill style, in catalog order.
pub fn fill_styles() -> &'static [StyleInfo] {
    FILL_STYLES
}

pub fn fill_style(id: &str) -> Option<&'static StyleInfo> {
    FILL_STYLES.iter().find(|s| s.id == id)
}

// ---------------------------------------------------------------- values

pub(crate) trait ParamValue: Sized {
    fn to_value(&self) -> Value;
    fn from_value(v: Value) -> Option<Self>;
    fn rust(&self) -> String;
}

fn float(v: f32) -> String {
    let v = if v.is_finite() { v } else { 0.0 };
    format!("{v:?}")
}

impl ParamValue for f32 {
    fn to_value(&self) -> Value {
        Value::Number(*self)
    }
    fn from_value(v: Value) -> Option<Self> {
        match v {
            Value::Number(x) if x.is_finite() => Some(x),
            Value::Count(n) => Some(n as f32),
            _ => None,
        }
    }
    fn rust(&self) -> String {
        float(*self)
    }
}

impl ParamValue for u32 {
    fn to_value(&self) -> Value {
        Value::Count(*self)
    }
    fn from_value(v: Value) -> Option<Self> {
        match v {
            Value::Count(n) => Some(n),
            Value::Number(x) if x.is_finite() => Some(x.round().max(0.0) as u32),
            _ => None,
        }
    }
    fn rust(&self) -> String {
        self.to_string()
    }
}

impl ParamValue for bool {
    fn to_value(&self) -> Value {
        Value::Toggle(*self)
    }
    fn from_value(v: Value) -> Option<Self> {
        match v {
            Value::Toggle(b) => Some(b),
            _ => None,
        }
    }
    fn rust(&self) -> String {
        self.to_string()
    }
}

impl Rgb {
    /// To sRGB hex (0xRRGGBB), rounding.
    pub fn hex(self) -> u32 {
        let e = |c: f32| (rune_motion::srgb_encode(c.clamp(0.0, 1.0)) * 255.0).round() as u32;
        (e(self.0[0]) << 16) | (e(self.0[1]) << 8) | e(self.0[2])
    }
}

impl ParamValue for Rgb {
    fn to_value(&self) -> Value {
        Value::Color(*self)
    }
    fn from_value(v: Value) -> Option<Self> {
        match v {
            Value::Color(c) => Some(c),
            _ => None,
        }
    }
    fn rust(&self) -> String {
        format!("rgb(0x{:06x})", self.hex())
    }
}

impl ParamValue for Stops {
    fn to_value(&self) -> Value {
        Value::Colors(*self)
    }
    fn from_value(v: Value) -> Option<Self> {
        match v {
            Value::Colors(s) => Some(s),
            _ => None,
        }
    }
    fn rust(&self) -> String {
        let hex: Vec<String> = self.as_slice().iter().map(|c| format!("0x{:06x}", c.hex())).collect();
        format!("Stops::hex(&[{}])", hex.join(", "))
    }
}

impl ParamValue for [f32; 2] {
    fn to_value(&self) -> Value {
        Value::Point(*self)
    }
    fn from_value(v: Value) -> Option<Self> {
        match v {
            Value::Point(p) if p[0].is_finite() && p[1].is_finite() => Some(p),
            _ => None,
        }
    }
    fn rust(&self) -> String {
        format!("[{}, {}]", float(self[0]), float(self[1]))
    }
}

impl ParamValue for [[f32; 2]; MAX_STOPS] {
    fn to_value(&self) -> Value {
        Value::Points(*self)
    }
    fn from_value(v: Value) -> Option<Self> {
        match v {
            Value::Points(p) if p.iter().flatten().all(|x| x.is_finite()) => Some(p),
            _ => None,
        }
    }
    fn rust(&self) -> String {
        let v: Vec<String> = self.iter().map(|p| p.rust()).collect();
        format!("[{}]", v.join(", "))
    }
}

/// Choice params: an enum with a fixed list of options.
macro_rules! choices {
    ($ty:ident { $($v:ident = $id:literal $name:literal),* $(,)? }) => {
        impl $ty {
            pub const CHOICES: &'static [Choice] = &[$(Choice { id: $id, name: $name, rust: concat!(stringify!($ty), "::", stringify!($v)) }),*];
            const ALL: &'static [$ty] = &[$($ty::$v),*];
        }
        impl ParamValue for $ty {
            fn to_value(&self) -> Value {
                Value::Choice($ty::ALL.iter().position(|x| x == self).unwrap_or(0))
            }
            fn from_value(v: Value) -> Option<Self> {
                match v {
                    Value::Choice(i) => $ty::ALL.get(i).copied(),
                    _ => None,
                }
            }
            fn rust(&self) -> String {
                $ty::CHOICES[$ty::ALL.iter().position(|x| x == self).unwrap_or(0)].rust.to_string()
            }
        }
    };
}
pub(crate) use choices;

/// A params struct's table: `PARAMS` plus get/set/emit by field name.
macro_rules! params {
    ($ty:ident { $($field:ident : $name:literal => $kind:expr),* $(,)? }) => {
        impl $ty {
            pub const PARAMS: &'static [Param] = &[$(Param { id: stringify!($field), name: $name, kind: $kind }),*];
            pub(crate) fn get_param(&self, id: &str) -> Option<Value> {
                match id {
                    $(stringify!($field) => Some(ParamValue::to_value(&self.$field)),)*
                    _ => None,
                }
            }
            pub(crate) fn set_param(&mut self, id: &str, v: Value) -> bool {
                match id {
                    $(stringify!($field) => match ParamValue::from_value(v) {
                        Some(x) => { self.$field = x; true }
                        None => false,
                    },)*
                    _ => false,
                }
            }
            pub(crate) fn rust_field(&self, id: &str) -> Option<String> {
                match id {
                    $(stringify!($field) => Some(ParamValue::rust(&self.$field)),)*
                    _ => None,
                }
            }
        }
    };
}
pub(crate) use params;

pub(crate) const fn num(min: f32, max: f32, step: f32, unit: Unit) -> ParamKind {
    ParamKind::Number { min, max, step, unit }
}

// ---------------------------------------------------------------- FillParams by id

impl FillParams {
    /// Params every fill style has.
    pub const COMMON: &'static [Param] = &[
        Param { id: "border", name: "Border", kind: num(0.0, 32.0, 0.5, Unit::Pt) },
        Param { id: "intensity", name: "Intensity", kind: num(0.0, 1.0, 0.01, Unit::Fraction) },
    ];

    /// This style's catalog entry.
    pub fn info(&self) -> &'static StyleInfo {
        &FILL_STYLES[self.style.index()]
    }

    /// A param's value, common or the style's own, by id.
    pub fn get(&self, id: &str) -> Option<Value> {
        match id {
            "border" => Some(Value::Number(self.border)),
            "intensity" => Some(Value::Number(self.intensity)),
            _ => self.style.get_param(id),
        }
    }

    /// Set a param by id. False when the id is unknown or the value has the wrong kind.
    pub fn set(&mut self, id: &str, v: Value) -> bool {
        match (id, f32::from_value(v)) {
            ("border", Some(x)) => self.border = x,
            ("intensity", Some(x)) => self.intensity = x,
            ("border" | "intensity", None) => return false,
            _ => return self.style.set_param(id, v),
        }
        true
    }

    /// Rust that rebuilds these params: `Fx::fill(...)`, written against the closest preset.
    pub fn to_rust(&self) -> String {
        let info = self.info();
        let mut best: Option<(usize, &Preset, Vec<String>)> = None;
        for p in info.presets {
            let base = p.style();
            let diff: Vec<String> = info
                .params
                .iter()
                .filter(|q| base.get_param(q.id) != self.style.get_param(q.id))
                .filter_map(|q| self.style.rust_field(q.id).map(|v| format!("{}: {v}", q.id)))
                .collect();
            if best.as_ref().is_none_or(|b| diff.len() < b.0) {
                best = Some((diff.len(), p, diff));
            }
        }
        let (_, preset, diff) = best.expect("every style has a preset");
        let mut s = if diff.is_empty() { preset.rust.to_string() } else { format!("{} {{ {}, ..{} }}", info.rust, diff.join(", "), preset.rust) };
        if self.border != 0.0 {
            s += &format!(".border({})", float(self.border));
        }
        if self.intensity != 1.0 {
            s += &format!(".intensity({})", float(self.intensity));
        }
        format!("Fx::fill({s})")
    }
}

// ---------------------------------------------------------------- the table

macro_rules! presets {
    ($style:ident($ty:ident) { $($id:literal $name:literal => $f:ident),* $(,)? }) => {
        &[$(Preset { id: $id, name: $name, rust: concat!(stringify!($ty), "::", stringify!($f), "()"), make: || FillStyle::$style($ty::$f()) }),*]
    };
}

static FILL_STYLES: &[StyleInfo] = &[
    StyleInfo {
        id: "liquid_metal",
        name: "Liquid metal",
        summary: "Polished metal reflecting a studio, flowing like liquid. Works as a fill or a tube-like border.",
        rust: "LiquidMetalParams",
        params: LiquidMetalParams::PARAMS,
        presets: presets!(LiquidMetal(LiquidMetalParams) { "chrome" "Chrome" => chrome, "gold" "Gold" => gold, "rose" "Rose gold" => rose, "gunmetal" "Gunmetal" => gunmetal }),
        reads_backdrop: false,
    },
    StyleInfo {
        id: "mesh_gradient",
        name: "Mesh gradient",
        summary: "Up to six colours pinned to slowly drifting points, blended in OKLab.",
        rust: "MeshGradientParams",
        params: MeshGradientParams::PARAMS,
        presets: presets!(MeshGradient(MeshGradientParams) { "dusk" "Dusk" => dusk, "lagoon" "Lagoon" => lagoon, "orchid" "Orchid" => orchid, "graphite" "Graphite" => graphite }),
        reads_backdrop: false,
    },
    StyleInfo {
        id: "dither",
        name: "Dither",
        summary: "Bayer or blue-noise dithering of a ramp, noise, the backdrop or an image into a palette.",
        rust: "DitherParams",
        params: DitherParams::PARAMS,
        presets: presets!(Dither(DitherParams) { "bayer" "Bayer" => bayer, "handheld" "Handheld" => handheld, "grain" "Blue noise" => grain, "backdrop" "Backdrop" => backdrop }),
        reads_backdrop: true,
    },
    StyleInfo {
        id: "grain_gradient",
        name: "Grain gradient",
        summary: "A linear, radial or conic gradient scattered into film grain.",
        rust: "GrainGradientParams",
        params: GrainGradientParams::PARAMS,
        presets: presets!(GrainGradient(GrainGradientParams) { "ember" "Ember" => ember, "halo" "Halo" => halo, "sweep" "Sweep" => sweep }),
        reads_backdrop: false,
    },
    StyleInfo {
        id: "fluted_glass",
        name: "Fluted glass",
        summary: "Reeded glass over the backdrop: each rib shows a squeezed, flipped strip of what is behind.",
        rust: "FlutedGlassParams",
        params: FlutedGlassParams::PARAMS,
        presets: presets!(FlutedGlass(FlutedGlassParams) { "reeded" "Reeded" => reeded, "fine" "Fine" => fine, "frosted" "Frosted" => frosted, "wide" "Wide" => wide }),
        reads_backdrop: true,
    },
    StyleInfo {
        id: "metaballs",
        name: "Metaballs",
        summary: "Glossy blobs that drift and merge.",
        rust: "MetaballParams",
        params: MetaballParams::PARAMS,
        presets: presets!(Metaballs(MetaballParams) { "lava" "Lava" => lava, "mint" "Mint" => mint }),
        reads_backdrop: false,
    },
    StyleInfo {
        id: "swirl",
        name: "Swirl",
        summary: "Colours stirred into a slow swirl.",
        rust: "SwirlParams",
        params: SwirlParams::PARAMS,
        presets: presets!(Swirl(SwirlParams) { "ink" "Ink" => ink, "marble" "Marble" => marble }),
        reads_backdrop: false,
    },
    StyleInfo {
        id: "voronoi",
        name: "Voronoi",
        summary: "Cells around drifting seed points, with seams.",
        rust: "VoronoiParams",
        params: VoronoiParams::PARAMS,
        presets: presets!(Voronoi(VoronoiParams) { "cells" "Cells" => cells, "stained" "Stained glass" => stained }),
        reads_backdrop: false,
    },
    StyleInfo {
        id: "waves",
        name: "Waves",
        summary: "Layered bands of colour with rolling edges.",
        rust: "WaveParams",
        params: WaveParams::PARAMS,
        presets: presets!(Waves(WaveParams) { "sea" "Sea" => sea, "dunes" "Dunes" => dunes }),
        reads_backdrop: false,
    },
    StyleInfo {
        id: "halftone",
        name: "Halftone",
        summary: "Print dots on a rotated screen, sized by a ramp, noise, the backdrop or an image.",
        rust: "HalftoneParams",
        params: HalftoneParams::PARAMS,
        presets: presets!(Halftone(HalftoneParams) { "print" "Print" => print, "backdrop" "Backdrop" => backdrop }),
        reads_backdrop: true,
    },
    StyleInfo {
        id: "plastic",
        name: "Plastic",
        summary: "Moulded plastic under a soft light: glossy to matte, domed or dished, with a rolled rim.",
        rust: "PlasticParams",
        params: PlasticParams::PARAMS,
        presets: presets!(Plastic(PlasticParams) { "glossy" "Glossy" => glossy, "matte" "Matte" => matte, "dished" "Dished" => dished }),
        reads_backdrop: false,
    },
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn styles_are_in_variant_order_with_unique_ids() {
        for (k, s) in fill_styles().iter().enumerate() {
            assert_eq!(s.default_fill().style.index(), k, "{}", s.id);
            assert_eq!(s.default_fill().info().id, s.id);
            assert_eq!(fill_style(s.id).map(|x| x.id), Some(s.id));
            assert!(!s.presets.is_empty() && !s.params.is_empty());
            let mut ids: Vec<_> = s.params.iter().map(|p| p.id).chain(FillParams::COMMON.iter().map(|p| p.id)).collect();
            ids.sort();
            ids.dedup();
            assert_eq!(ids.len(), s.params.len() + FillParams::COMMON.len(), "{}: unique param ids", s.id);
            let mut ps: Vec<_> = s.presets.iter().map(|p| p.id).collect();
            ps.sort();
            ps.dedup();
            assert_eq!(ps.len(), s.presets.len());
        }
        let mut ids: Vec<_> = fill_styles().iter().map(|s| s.id).collect();
        ids.dedup();
        assert_eq!(ids.len(), fill_styles().len());
        assert!(fill_style("nope").is_none());
    }

    #[test]
    fn first_preset_is_the_default() {
        let d = [
            FillStyle::from(LiquidMetalParams::default()),
            MeshGradientParams::default().into(),
            DitherParams::default().into(),
            GrainGradientParams::default().into(),
            FlutedGlassParams::default().into(),
            MetaballParams::default().into(),
            SwirlParams::default().into(),
            VoronoiParams::default().into(),
            WaveParams::default().into(),
            HalftoneParams::default().into(),
            PlasticParams::default().into(),
        ];
        assert_eq!(d.len(), fill_styles().len());
        for (s, d) in fill_styles().iter().zip(d) {
            assert_eq!(s.presets[0].style(), d, "{}", s.id);
        }
    }

    #[test]
    fn every_param_gets_sets_and_sits_in_range() {
        for s in fill_styles() {
            for pr in s.presets {
                let f = pr.fill();
                for p in s.params.iter().chain(FillParams::COMMON) {
                    let v = f.get(p.id).unwrap_or_else(|| panic!("{}.{} reads", s.id, p.id));
                    match (p.kind, v) {
                        (ParamKind::Number { min, max, .. }, Value::Number(x)) => assert!((min..=max).contains(&x), "{}/{}.{} = {x} outside {min}..{max}", s.id, pr.id, p.id),
                        (ParamKind::Count { min, max }, Value::Count(n)) => assert!((min..=max).contains(&n), "{}.{}", s.id, p.id),
                        (ParamKind::Choice(c), Value::Choice(k)) => assert!(k < c.len()),
                        (ParamKind::Colors { min, max }, Value::Colors(c)) => assert!((min..=max).contains(&c.len()), "{}/{}.{}", s.id, pr.id, p.id),
                        (ParamKind::Toggle, Value::Toggle(_)) | (ParamKind::Color, Value::Color(_)) | (ParamKind::Point, Value::Point(_)) | (ParamKind::Points { .. }, Value::Points(_)) => {}
                        (k, v) => panic!("{}.{}: {k:?} holds {v:?}", s.id, p.id),
                    }
                    let mut g = f;
                    assert!(g.set(p.id, v), "{}.{} writes", s.id, p.id);
                    assert_eq!(g, f);
                    assert!(!g.set(p.id, Value::Point([9.0, 9.0])) || matches!(p.kind, ParamKind::Point), "{}.{} rejects the wrong kind", s.id, p.id);
                    assert!(!g.set(p.id, Value::Number(f32::NAN)), "{}.{} rejects NaN", s.id, p.id);
                    assert!(!g.set(p.id, Value::Number(f32::INFINITY)), "{}.{} rejects inf", s.id, p.id);
                    // Round-trip a distinct value for every param kind.
                    let alt = match (p.kind, v) {
                        (ParamKind::Number { min, max, .. }, Value::Number(x)) => Value::Number(if (x - max).abs() > 1e-4 { max } else { min }),
                        (ParamKind::Count { min, max }, Value::Count(n)) => Value::Count(if n != max { max } else { min }),
                        (ParamKind::Choice(c), Value::Choice(k)) => Value::Choice((k + 1) % c.len()),
                        (ParamKind::Toggle, Value::Toggle(b)) => Value::Toggle(!b),
                        (ParamKind::Color, _) => Value::Color(rgb(0x123456)),
                        (ParamKind::Colors { max, .. }, _) => Value::Colors(Stops::hex(&[0x112233, 0x445566, 0x778899, 0xaabbcc, 0xddeeff, 0x102030][..max])),
                        (ParamKind::Point, _) => Value::Point([0.25, 0.75]),
                        (ParamKind::Points { .. }, _) => Value::Points([[0.1, 0.2]; MAX_STOPS]),
                        _ => unreachable!(),
                    };
                    assert_ne!(alt, v, "{}.{} has a distinct test value", s.id, p.id);
                    assert!(g.set(p.id, alt), "{}.{} writes distinct value", s.id, p.id);
                    assert_eq!(g.get(p.id), Some(alt), "{}.{} round-trips distinct value", s.id, p.id);
                    assert_ne!(g, f, "{}.{} mutates FillParams", s.id, p.id);
                }
                assert!(f.get("nope").is_none());
            }
        }
    }

    #[test]
    fn changing_a_param_changes_the_fill() {
        let mut f = Fx::fill(LiquidMetalParams::gold());
        let Fx::Fill(p) = &mut f else { unreachable!() };
        assert!(p.set("flow", Value::Number(0.9)));
        assert!(p.set("tint", Value::Color(rgb(0x808080))));
        assert!(p.set("border", Value::Number(3.0)));
        let FillStyle::LiquidMetal(m) = p.style else { unreachable!() };
        assert_eq!(m.flow, 0.9);
        assert_eq!(p.border, 3.0);
        assert!(!p.set("pattern", Value::Choice(0)), "not a liquid metal param");
        let mut d = FillParams::from(DitherParams::bayer());
        assert!(d.set("pattern", Value::Choice(3)));
        let FillStyle::Dither(dp) = d.style else { unreachable!() };
        assert_eq!(dp.pattern, DitherPattern::BlueNoise);
    }

    /// Parse and evaluate the Rust subset emitted by `FillParams::to_rust()`.
    fn eval_emitted_rust(code: &str) -> FillParams {
        let mut s = code.strip_prefix("Fx::fill(").and_then(|s| s.strip_suffix(')')).expect(code);
        let mut intensity = 1.0f32;
        if let Some((base, arg)) = s.rsplit_once(".intensity(") {
            intensity = arg.strip_suffix(')').expect(code).parse().expect(code);
            s = base;
        }
        let mut border = 0.0f32;
        if let Some((base, arg)) = s.rsplit_once(".border(") {
            border = arg.strip_suffix(')').expect(code).parse().expect(code);
            s = base;
        }
        let mut fill = if let Some((ty, body)) = s.split_once(" { ") {
            let body = body.strip_suffix(" }").expect(code);
            let (fields_str, preset_rust) = body.rsplit_once(", ..").expect(code);
            let info = fill_styles().iter().find(|i| i.rust == ty.trim()).expect(ty);
            let preset = info.presets.iter().find(|p| p.rust == preset_rust.trim()).expect(preset_rust);
            let mut f = preset.fill();
            // Split top-level `field: expr` pairs outside `[...]` and `(...)`.
            let mut depth = 0i32;
            let mut start = 0usize;
            for (idx, ch) in fields_str.char_indices() {
                match ch {
                    '[' | '(' => depth += 1,
                    ']' | ')' => depth -= 1,
                    ',' if depth == 0 => {
                        apply_field(&mut f, info, &fields_str[start..idx]);
                        start = idx + 1;
                    }
                    _ => {}
                }
            }
            apply_field(&mut f, info, &fields_str[start..]);
            f
        } else {
            fill_styles().iter().flat_map(|i| i.presets).find(|p| p.rust == s.trim()).expect(s).fill()
        };
        fill.border = border;
        fill.intensity = intensity;
        fill
    }

    fn parse_hex(s: &str) -> u32 {
        let hex = s.trim().strip_prefix("0x").expect(s);
        assert_eq!(hex.len(), 6, "6-digit hex literal: {s}");
        u32::from_str_radix(hex, 16).expect(s)
    }

    fn parse_pt(s: &str) -> [f32; 2] {
        let inner = s.trim().strip_prefix('[').and_then(|s| s.strip_suffix(']')).expect(s);
        let (x, y) = inner.split_once(',').expect(s);
        [x.trim().parse().expect(s), y.trim().parse().expect(s)]
    }

    fn apply_field(f: &mut FillParams, info: &StyleInfo, entry: &str) {
        let (id, val) = entry.trim().split_once(": ").expect(entry);
        let param = info.params.iter().find(|p| p.id == id).expect(id);
        let v = match param.kind {
            ParamKind::Number { .. } => {
                assert!(val.contains('.') || val.contains('e'), "float literal has decimal or exp: {val}");
                Value::Number(val.parse().expect(val))
            }
            ParamKind::Count { .. } => Value::Count(val.parse().expect(val)),
            ParamKind::Toggle => Value::Toggle(val.parse().expect(val)),
            ParamKind::Choice(choices) => {
                let idx = choices.iter().position(|c| c.rust == val).expect(val);
                Value::Choice(idx)
            }
            ParamKind::Color => {
                let hex = val.strip_prefix("rgb(").and_then(|s| s.strip_suffix(')')).expect(val);
                Value::Color(rgb(parse_hex(hex)))
            }
            ParamKind::Colors { .. } => {
                let list = val.strip_prefix("Stops::hex(&[").and_then(|s| s.strip_suffix("])")).expect(val);
                let hexes: Vec<u32> = list.split(',').map(parse_hex).collect();
                Value::Colors(Stops::hex(&hexes))
            }
            ParamKind::Point => Value::Point(parse_pt(val)),
            ParamKind::Points { .. } => {
                let inner = val.strip_prefix('[').and_then(|s| s.strip_suffix(']')).expect(val);
                let mut pts = [[0.0; 2]; MAX_STOPS];
                let mut k = 0;
                let mut start = 0;
                let mut depth = 0;
                for (i, ch) in inner.char_indices() {
                    match ch {
                        '[' => depth += 1,
                        ']' => depth -= 1,
                        ',' if depth == 0 => {
                            pts[k] = parse_pt(&inner[start..i]);
                            k += 1;
                            start = i + 1;
                        }
                        _ => {}
                    }
                }
                pts[k] = parse_pt(&inner[start..]);
                assert_eq!(k + 1, MAX_STOPS);
                Value::Points(pts)
            }
        };
        assert!(f.set(id, v), "{}.{id}", info.id);
    }

    #[test]
    fn rust_is_written_against_the_closest_preset_and_parses_back() {
        assert_eq!(FillParams::from(LiquidMetalParams::gold()).to_rust(), "Fx::fill(LiquidMetalParams::gold())");
        let f = FillParams::from(LiquidMetalParams { flow: 0.8, ..LiquidMetalParams::rose() }).border(4.0);
        assert_eq!(f.to_rust(), "Fx::fill(LiquidMetalParams { flow: 0.8, ..LiquidMetalParams::rose() }.border(4.0))");
        let d = FillParams::from(DitherParams { pattern: DitherPattern::Bayer8, colors: Stops::hex(&[0x000000, 0xffffff]), ..DitherParams::bayer() });
        assert_eq!(d.to_rust(), "Fx::fill(DitherParams { pattern: DitherPattern::Bayer8, colors: Stops::hex(&[0x000000, 0xffffff]), ..DitherParams::bayer() })");
        for s in fill_styles() {
            for p in s.presets {
                let base = p.fill();
                assert_eq!(base.to_rust(), format!("Fx::fill({})", p.rust));
                assert_eq!(eval_emitted_rust(&base.to_rust()), base);
                let mut all_changed = base.border(3.5).intensity(0.75);
                for param in s.params {
                    let alt = match param.kind {
                        ParamKind::Number { min, max, .. } => Value::Number((min + max) * 0.5 + 0.125),
                        ParamKind::Count { max, .. } => Value::Count(max),
                        ParamKind::Toggle => Value::Toggle(!matches!(base.get(param.id), Some(Value::Toggle(true)))),
                        ParamKind::Choice(c) => Value::Choice(c.len() - 1),
                        ParamKind::Color => Value::Color(rgb(0xabcdef)),
                        ParamKind::Colors { min, .. } => Value::Colors(Stops::hex(&[0x102030, 0x405060, 0x708090, 0xa0b0c0, 0xd0e0f0, 0x112233][..min])),
                        ParamKind::Point => Value::Point([0.3, 0.7]),
                        ParamKind::Points { .. } => Value::Points([[0.25, 0.75]; MAX_STOPS]),
                    };
                    let mut one = base;
                    assert!(one.set(param.id, alt));
                    assert_eq!(eval_emitted_rust(&one.to_rust()), one, "{}/{}.{}", s.id, p.id, param.id);
                    assert!(all_changed.set(param.id, alt));
                }
                assert_eq!(eval_emitted_rust(&all_changed.to_rust()), all_changed, "{}/{} all fields", s.id, p.id);
            }
        }
    }

    #[test]
    fn colours_round_trip_through_hex() {
        for h in [0x000000, 0xffffff, 0xf3c56b, 0x0b3d5c, 0x808080] {
            assert_eq!(rgb(h).hex(), h);
        }
    }
}
