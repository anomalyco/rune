//! The per-instance and per-frame GPU ABI shared by the uber shader, layer planner, CPU
//! boot painter and Windows damage planner.

use std::mem::{offset_of, size_of};

use bytemuck::{Pod, Zeroable};

use super::{Blend, LayerSpec, layer_blur};

/// Constants and ABI parameters parsed directly from `crates/rune/src/shaders/*.wgsl` by `build.rs`.
pub(crate) mod shader_consts {
    include!(concat!(env!("OUT_DIR"), "/shader_consts.rs"));
}

/// Primitive kind stored in [`Instance::params`]`[1]`.
///
/// The `KIND_*` float constants derive from these discriminants so every Rust consumer shares one
/// numeric table with `shader.wgsl`.
#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Kind {
    /// Rounded rect in `fill` with a `params.x`-wide `border` ring over it (`KIND_RECT`).
    Rect = shader_consts::KIND_RECT_U8,
    /// Analytic Gaussian rounded-rect shadow (`KIND_SHADOW`).
    Shadow = shader_consts::KIND_SHADOW_U8,
    /// Glyph-atlas or outline text (`KIND_GLYPH`).
    Glyph = shader_consts::KIND_GLYPH_U8,
    /// Inset shadow inside a rounded rect (`KIND_INSET`).
    Inset = shader_consts::KIND_INSET_U8,
    /// Rounded rect or border with a vertical gradient (`KIND_GRADIENT`).
    Gradient = shader_consts::KIND_GRADIENT_U8,
    /// Antialiased stroke segment (`KIND_SEGMENT`).
    Segment = shader_consts::KIND_SEGMENT_U8,
    /// Filled triangle (`KIND_TRIANGLE`).
    Triangle = shader_consts::KIND_TRIANGLE_U8,
    /// RGBA image from the image atlas (`KIND_IMAGE`).
    Image = shader_consts::KIND_IMAGE_U8,
    /// Isolated layer scope open / composite quad (`KIND_LAYER`).
    Layer = shader_consts::KIND_LAYER_U8,
    /// Isolated layer scope close (`KIND_LAYER_END`).
    LayerEnd = shader_consts::KIND_LAYER_END_U8,
    /// Dashed border ring (`KIND_DASHED`).
    Dashed = shader_consts::KIND_DASHED_U8,
    /// Radial or angled linear gradient stop segment (`KIND_RADIAL`).
    Radial = shader_consts::KIND_RADIAL_U8,
    /// Compositor effect slot (`KIND_EFFECT`).
    Effect = 12,
    /// In-window backdrop blur (`KIND_BACKDROP_BLUR`).
    BackdropBlur = 13,
}

/// Which fields of an [`Instance`] carry premultiplied colour or opacity and therefore scale
/// with enclosing layer opacity ([`Kind::opacity_fields`], [`Instance::scale_opacity`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct OpacityFields {
    /// `Instance::fill` holds premultiplied colour or opacity (`fill[3]`).
    pub fill: bool,
    /// `Instance::border` holds a premultiplied colour (never outline records, wrapped-image
    /// placement rects, layer pivots, or effect marks).
    pub border: bool,
    /// `Instance::uv` holds a premultiplied colour (`Kind::Gradient` bottom colour).
    pub uv: bool,
    /// `Instance::params[3]` holds the opacity the colours already carry (`border_over` on
    /// `Kind::Rect` and `Kind::Gradient`).
    pub params_w: bool,
}

const _: () = {
    assert!(size_of::<Option<Kind>>() == 1);
    assert!(Kind::Radial.as_f32() < shader_consts::UBER_MAX_KIND_THRESHOLD);
    assert!(Kind::Effect.as_f32() > shader_consts::UBER_MAX_KIND_THRESHOLD);
    assert!(Kind::BackdropBlur.as_f32() > shader_consts::UBER_MAX_KIND_THRESHOLD);
    assert!(shader_consts::OUTLINE_TEX_WIDTH == crate::text::outline::WIDTH);
    assert!(shader_consts::OUTLINE_HINT_REACH > 0.0);
};

impl Kind {
    /// Number of defined primitive kinds.
    pub const COUNT: usize = 14;

    #[inline]
    pub const fn as_u8(self) -> u8 {
        self as u8
    }

    #[inline]
    pub const fn as_u32(self) -> u32 {
        self as u32
    }

    #[inline]
    pub const fn as_f32(self) -> f32 {
        self as u8 as f32
    }

    /// Which fields of an [`Instance`] of this kind carry colour or opacity and scale when
    /// flattened into an enclosing layer's opacity or animated by GPU springs
    /// (`crates/rune-gpu/src/springs.wgsl`).
    ///
    /// Every variant is matched explicitly with no wildcard arm so adding a new `Kind` forces
    /// declaring which fields are colours before it can corrupt non-colour payloads.
    #[inline]
    pub(crate) const fn opacity_fields(self) -> OpacityFields {
        match self {
            Kind::Rect => OpacityFields { fill: true, border: true, uv: false, params_w: true },
            Kind::Gradient => OpacityFields { fill: true, border: true, uv: true, params_w: true },
            Kind::Dashed | Kind::Radial => OpacityFields { fill: true, border: true, uv: false, params_w: false },
            // Primitives whose only colour/alpha is `fill`: `Glyph` stores Windfoil outline
            // record metadata in `border`, `Image` stores its wrapped placement rect in `border`
            // and wrap mode in `params[3]`, `Layer` stores turned-layer rotation/pivot in
            // `border`, and `Shadow`/`Inset`/`Segment`/`Triangle`/`BackdropBlur` carry no
            // colour in `border`.
            Kind::Shadow
            | Kind::Glyph
            | Kind::Inset
            | Kind::Segment
            | Kind::Triangle
            | Kind::Image
            | Kind::Layer
            | Kind::BackdropBlur => OpacityFields { fill: true, border: false, uv: false, params_w: false },
            // Scope sentinel (`LayerEnd`) and compositor slot (`Effect`, whose `fill[3]` is
            // always 1, `border[0..2]` holds `EffectSite::mark`, `uv` holds `EffectSite::data`,
            // and faded slots stay inside isolated layers).
            Kind::LayerEnd | Kind::Effect => OpacityFields { fill: false, border: false, uv: false, params_w: false },
        }
    }

    #[inline]
    pub const fn from_u8(v: u8) -> Option<Kind> {
        match v {
            shader_consts::KIND_RECT_U8 => Some(Kind::Rect),
            shader_consts::KIND_SHADOW_U8 => Some(Kind::Shadow),
            shader_consts::KIND_GLYPH_U8 => Some(Kind::Glyph),
            shader_consts::KIND_INSET_U8 => Some(Kind::Inset),
            shader_consts::KIND_GRADIENT_U8 => Some(Kind::Gradient),
            shader_consts::KIND_SEGMENT_U8 => Some(Kind::Segment),
            shader_consts::KIND_TRIANGLE_U8 => Some(Kind::Triangle),
            shader_consts::KIND_IMAGE_U8 => Some(Kind::Image),
            shader_consts::KIND_LAYER_U8 => Some(Kind::Layer),
            shader_consts::KIND_LAYER_END_U8 => Some(Kind::LayerEnd),
            shader_consts::KIND_DASHED_U8 => Some(Kind::Dashed),
            shader_consts::KIND_RADIAL_U8 => Some(Kind::Radial),
            12 => Some(Kind::Effect),
            13 => Some(Kind::BackdropBlur),
            _ => None,
        }
    }

    /// Lossless conversion from the `f32` stored in `Instance::params[1]`.
    ///
    /// Matches IEEE-754 bit patterns directly so per-instance loops compile to integer ops with
    /// no float-to-int saturating cast, and non-integer or out-of-range values return `None`.
    /// All non-zero kinds (`1.0..=13.0`) have zero low 20 bits and span `0..=29` after
    /// `(bits - 1.0_f32.to_bits()).rotate_left(12)`, letting LLVM lower full matches to a dense
    /// lookup/jump table while still folding single-kind checks to one immediate compare.
    #[inline(always)]
    pub const fn from_f32(v: f32) -> Option<Kind> {
        const ONE: u32 = KIND_SHADOW.to_bits();
        const fn slot(k: Kind) -> u32 {
            k.as_f32().to_bits().wrapping_sub(ONE).rotate_left(12)
        }
        const SHADOW: u32 = slot(Kind::Shadow);
        const GLYPH: u32 = slot(Kind::Glyph);
        const INSET: u32 = slot(Kind::Inset);
        const GRADIENT: u32 = slot(Kind::Gradient);
        const SEGMENT: u32 = slot(Kind::Segment);
        const TRIANGLE: u32 = slot(Kind::Triangle);
        const IMAGE: u32 = slot(Kind::Image);
        const LAYER: u32 = slot(Kind::Layer);
        const LAYER_END: u32 = slot(Kind::LayerEnd);
        const DASHED: u32 = slot(Kind::Dashed);
        const RADIAL: u32 = slot(Kind::Radial);
        const EFFECT: u32 = slot(Kind::Effect);
        const BACKDROP_BLUR: u32 = slot(Kind::BackdropBlur);
        let bits = v.to_bits();
        if (bits << 1) == 0 {
            return Some(Kind::Rect);
        }
        match bits.wrapping_sub(ONE).rotate_left(12) {
            SHADOW => Some(Kind::Shadow),
            GLYPH => Some(Kind::Glyph),
            INSET => Some(Kind::Inset),
            GRADIENT => Some(Kind::Gradient),
            SEGMENT => Some(Kind::Segment),
            TRIANGLE => Some(Kind::Triangle),
            IMAGE => Some(Kind::Image),
            LAYER => Some(Kind::Layer),
            LAYER_END => Some(Kind::LayerEnd),
            DASHED => Some(Kind::Dashed),
            RADIAL => Some(Kind::Radial),
            EFFECT => Some(Kind::Effect),
            BACKDROP_BLUR => Some(Kind::BackdropBlur),
            _ => None,
        }
    }
}

/// Rounded rect in `fill` with a `params.x`-wide `border` ring drawn over it, as CSS paints a
/// border over its background. `params.w` is the opacity the colours already carry (1 when not
/// faded), so the ring hides the fill by `border.a / params.w`. Non-zero `uv` (with `uv.z >= 0`)
/// gives per-side widths `[top, right, bottom, left]`; `params.x` is then the widest.
pub const KIND_RECT: f32 = Kind::Rect.as_f32();
pub const KIND_SHADOW: f32 = Kind::Shadow.as_f32();
pub const KIND_GLYPH: f32 = Kind::Glyph.as_f32();
/// Inset shadow inside a rounded rect: `params.x` spread, `params.z` sigma, `uv.xy` offset.
pub const KIND_INSET: f32 = Kind::Inset.as_f32();
/// Rounded rect with a vertical gradient: `fill` at the top, `uv` (premultiplied) at the bottom.
/// With a border width in `params.x` it is a `KIND_RECT` whose ring runs from `border` to `uv`,
/// over the fill, with the same `params.w`.
pub const KIND_GRADIENT: f32 = Kind::Gradient.as_f32();
/// A stroke segment: `radii` = x0, y0, x1, y1; `params.x` half width, `params.z` 1 for round caps.
pub const KIND_SEGMENT: f32 = Kind::Segment.as_f32();
/// A filled triangle: `radii` = ax, ay, bx, by; `uv.xy` = cx, cy; `params.x` the antialiased-edge
/// mask (a→b, b→c, c→a as bits 0, 1, 2; other edges are hard so neighbours meet without seams).
pub const KIND_TRIANGLE: f32 = Kind::Triangle.as_f32();
/// An RGBA image (`El::pixels`): `uv.xy` its origin in the image atlas, `uv.zw` its size in
/// texels, drawn scaled into `rect` with the element's corner `radii`; `fill.a` is the opacity.
pub const KIND_IMAGE: f32 = Kind::Image.as_f32();
/// Balanced scope records in the painter stream. They occupy instance indices but are never
/// rasterized as ordinary quads. Keep scopes intact when partitioning compositor lists.
pub const KIND_LAYER: f32 = Kind::Layer.as_f32();
pub const KIND_LAYER_END: f32 = Kind::LayerEnd.as_f32();
/// A dashed border ring (`El::border_dash`), `params.x` wide inside the rounded rect, drawn
/// alone (the fill is its own instance, showing through the gaps). Its colour runs from `fill`
/// at the top to `border` at the bottom; `uv.xyz` = dash, gap, phase in device pixels.
pub const KIND_DASHED: f32 = Kind::Dashed.as_f32();
/// A radial (`uv.z > 0`: `uv` = `[cx, cy, rx, ry]` in rect-local device pixels) or angled linear
/// (`uv.z == 0`: `uv.xy` = gradient vector per device pixel) stop segment from `fill` at `t0`
/// (`params.x`) to `border` at `t1` (`params.w`), with `+ 2.0` marking an open interior stop edge.
pub const KIND_RADIAL: f32 = Kind::Radial.as_f32();
/// An effect slot (`El::effect`). It rasterizes no pixels but keeps its place in the painter
/// stream, so `Renderer::draw_range_with` or `Renderer::draw_with` can hand the compositor the pass
/// at exactly this point, inside the enclosing isolated layer if there is one. `rect` is the
/// element, `clip`/`fade` (and the rounded-clip radius in `fade[3]`) its clip, `params.x` `1.0`
/// when the slot requests a backdrop copy (`El::effect_backdrop(true)`), `params.z` the bleed past
/// `rect` in device pixels and `params.w` the slot's scale in device pixels per point (`Env::scale`
/// times any element or ancestor `scale` / `live_scale`). `fill[3]` is always 1: a faded slot is
/// drawn in an isolated layer that applies its opacity. `border[0]` and `border[1]` hold the low
/// and high 16 bits of its `El::effect` tag as exact floats (`EffectSite::mark`), and `uv` its
/// `El::effect_data` payload, bit for bit (`EffectSite::data`).
pub const KIND_EFFECT: f32 = Kind::Effect.as_f32();
/// A backdrop blur (`El::backdrop_blur`). Like an effect slot it rasterizes nothing itself in
/// the uber shader; the renderer stops there, copies and blurs what is below, and draws the blur
/// inside `rect` with signed corner `radii`, cut by `clip`/`fade`, at opacity `fill[3]`.
/// `params.x` is the blur's standard deviation in device pixels and `params.w` the scale in
/// device pixels per point.
pub const KIND_BACKDROP_BLUR: f32 = Kind::BackdropBlur.as_f32();

/// Uniform block bound at `@group(0) @binding(0)` by `shader.wgsl` and `backdrop_composite.wgsl`.
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub(crate) struct Globals {
    pub viewport: [f32; 2],
    /// Split presentation timestamp `[t_hi, t_lo]` for GPU springs (`springs.wgsl`).
    pub pad: [f32; 2],
    /// `[srgb_blend, byte_source, 0.0, 0.0]`.
    pub flags: [f32; 4],
}

const _: () = {
    assert!(size_of::<Globals>() == 32);
    assert!(offset_of!(Globals, viewport) == 0);
    assert!(offset_of!(Globals, pad) == 8);
    assert!(offset_of!(Globals, flags) == 16);
};

/// One instance, in device pixels.
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct Instance {
    pub rect: [f32; 4],
    pub radii: [f32; 4],
    pub fill: [f32; 4],
    pub border: [f32; 4],
    /// border width, kind, shadow sigma, glyph coverage exponent
    pub params: [f32; 4],
    pub uv: [f32; 4],
    /// x0, y0, x1, y1
    pub clip: [f32; 4],
    /// Attribute 7 is Uint32x4; the instance ABI stays 128 bytes (8 × vec4).
    /// `[0]` = pack2x16float(left, top), `[1]` = pack2x16float(right, bottom) widths; a negative
    /// width ramps linearly, a positive one on smoothstep.
    /// `[2]` = pack4x8unorm(clamp(inward origin→clip distance / width, 0, 1)), LTRB.
    /// `[3]` = signed uniform f32 clip-radius bits or tagged mixed radii (`pack_clip_radii`).
    /// The rounded rectangle is `clip`; fade widths never occupy this word.
    /// Layer records may leave all four words zero. See `pack_fade`.
    pub fade: [u32; 4],
}

const _: () = assert!(size_of::<Instance>() == 128);

/// Word count of [`Instance`] when viewed as `[u32; INSTANCE_WORDS]`.
pub(crate) const INSTANCE_WORDS: usize = size_of::<Instance>() / 4;

const fn word_of(byte_offset: usize) -> usize {
    assert!(byte_offset.is_multiple_of(4), "Instance field offset must be a multiple of 4 bytes");
    byte_offset / 4
}

/// 32-bit word indices into [`Instance`] (viewed as `[u32; INSTANCE_WORDS]`), derived from
/// `offset_of!` so word-level comparisons cannot drift from the struct layout.
pub(crate) mod word {
    use std::mem::{offset_of, size_of};
    use std::ops::Range;

    use super::{Instance, shader_consts, word_of};

    const _: () = {
        assert!(size_of::<Instance>() == shader_consts::UBER_ATTR_COUNT * 16);
        assert!(word_of(offset_of!(Instance, rect)) == shader_consts::ATTR_RECT_LOC as usize * 4);
        assert!(word_of(offset_of!(Instance, radii)) == shader_consts::ATTR_RADII_LOC as usize * 4);
        assert!(word_of(offset_of!(Instance, fill)) == shader_consts::ATTR_FILL_LOC as usize * 4);
        assert!(word_of(offset_of!(Instance, border)) == shader_consts::ATTR_BORDER_LOC as usize * 4);
        assert!(word_of(offset_of!(Instance, params)) == shader_consts::ATTR_PARAMS_LOC as usize * 4);
        assert!(word_of(offset_of!(Instance, uv)) == shader_consts::ATTR_UV_LOC as usize * 4);
        assert!(word_of(offset_of!(Instance, clip)) == shader_consts::ATTR_CLIP_LOC as usize * 4);
        assert!(word_of(offset_of!(Instance, fade)) == shader_consts::ATTR_FADE_LOC as usize * 4);
    };

    pub(crate) const RECT: usize = word_of(offset_of!(Instance, rect));
    pub(crate) const RECT_Y: usize = RECT + 1;
    pub(crate) const FILL: Range<usize> = word_of(offset_of!(Instance, fill))..word_of(offset_of!(Instance, fill)) + 4;
    pub(crate) const BORDER: Range<usize> = word_of(offset_of!(Instance, border))..word_of(offset_of!(Instance, border)) + 4;
    pub(crate) const PARAMS: usize = word_of(offset_of!(Instance, params));
    pub(crate) const PARAMS_W: usize = PARAMS + 3;
    pub(crate) const CLIP: usize = word_of(offset_of!(Instance, clip));
    pub(crate) const CLIP_Y0: usize = CLIP + 1;
    pub(crate) const CLIP_Y1: usize = CLIP + 3;
}

/// Bias added to a fade width in `Inh::fade` / `pack_fade` to tag an edge as using the frame's
/// piecewise-linear explicit stops (`ScrollFade::stops`) rather than smoothstep or pure linear.
/// Strictly above `f16::MAX` (`65504.0`) so any normal smoothstep width passes through untouched.
pub(crate) const STOP_FADE_BIAS: f32 = 131_072.0;

pub fn pack_fade(width: [f32; 4], origin: [f32; 4], clip: [f32; 4]) -> [u32; 4] {
    // A negative width is a linear ramp (`El::clip_fade_linear`); `width > STOP_FADE_BIAS` tags
    // an edge using explicit piecewise-linear stops (`ScrollFade::stops`, bit 7 of its offset byte).
    let mut halves = [0u32; 4];
    let mut offsets = 0u32;
    for edge in 0..4 {
        let is_stops = width[edge] > STOP_FADE_BIAS;
        let raw_w = if is_stops { width[edge] - STOP_FADE_BIAS } else { width[edge] };
        halves[edge] = half::f16::from_f32(raw_w.clamp(-65504.0, 65504.0)).to_bits() as u32;
        let distance = if edge < 2 { clip[edge] - origin[edge] } else { origin[edge] - clip[edge] };
        let w = raw_w.abs();
        let offset = if w > 0.0 { (distance / w).clamp(0.0, 1.0) } else { 0.0 };
        let byte = ((offset * 127.0 + 0.5) as u32).min(127) | if is_stops { 0x80 } else { 0 };
        offsets |= byte << (edge * 8);
    }
    [halves[0] | (halves[1] << 16), halves[2] | (halves[3] << 16), offsets, 0]
}

/// Decode widths and normalized origin offsets for tools inspecting the instance stream. A
/// negative width is a linear ramp.
pub fn unpack_fade(fade: [u32; 4]) -> ([f32; 4], [f32; 4]) {
    (std::array::from_fn(|e| half::f16::from_bits((fade[e / 2] >> ((e % 2) * 16)) as u16).to_f32()),
     std::array::from_fn(|e| ((fade[2] >> (e * 8)) & 0x7f) as f32 / 127.0))
}

pub const NO_CLIP: [f32; 4] = [-1e6, -1e6, 1e6, 1e6];

/// Pack per-corner signed clip radii `[tl, tr, br, bl]` (negative = continuous corners)
/// into `Instance::fade[3]`, clamping each corner to half the shorter `clip` side.
/// Uniform corners stay an exact IEEE-754 `f32`; mixed corners use the `0x6000_0000`
/// high-exponent tag (`|r| >= 2^65` in IEEE-754) with 4 × 7-bit magnitudes.
/// Mixed radii round to 0.5 px through 63.5 px, otherwise 2 px, saturating at 254 px.
/// All nonzero radii must have the same sign (one curve style per clip). Uniform radii
/// saturate just below the tag's exponent range, far beyond device-pixel coordinates.
pub fn pack_clip_radii(clip: [f32; 4], radii: [f32; 4]) -> u32 {
    const UNIFORM_MAX: f32 = f32::from_bits(shader_consts::CLIP_RADII_MIXED_TAG - 1);
    const LANE_MAX: f32 = shader_consts::CLIP_RADII_LANE_MASK as f32;
    const FINE_MAX: f32 = LANE_MAX * shader_consts::CLIP_RADII_FINE_STEP;
    const INV_COARSE: f32 = 1.0 / shader_consts::CLIP_RADII_COARSE_STEP;
    const INV_FINE: f32 = 1.0 / shader_consts::CLIP_RADII_FINE_STEP;
    let half = ((clip[2] - clip[0]).min(clip[3] - clip[1]) * 0.5).max(0.0);
    let clamped = radii.map(|radius| {
        let r = radius.abs().min(half).min(UNIFORM_MAX);
        if radius < 0.0 && r > 0.0 { -r } else { r }
    });
    if clamped[0] == clamped[1] && clamped[1] == clamped[2] && clamped[2] == clamped[3] {
        return if clamped[0] == 0.0 { 0 } else { clamped[0].to_bits() };
    }
    let continuous = clamped.iter().any(|&r| r < 0.0);
    let max_r = clamped.iter().fold(0.0f32, |m, r| m.max(r.abs()));
    let coarse = max_r > FINE_MAX;
    let inv_step = if coarse { INV_COARSE } else { INV_FINE };
    let mut bits = shader_consts::CLIP_RADII_MIXED_TAG
        | if continuous { shader_consts::CLIP_RADII_CONTINUOUS_BIT } else { 0 }
        | if coarse { shader_consts::CLIP_RADII_COARSE_BIT } else { 0 };
    for (idx, r) in clamped.into_iter().enumerate() {
        let q = (r.abs() * inv_step).round().clamp(0.0, LANE_MAX) as u32;
        bits |= q << (idx as u32 * shader_consts::CLIP_RADII_LANE_BITS);
    }
    bits
}

/// Decode `Instance::fade[3]` into four signed corner radii `[tl, tr, br, bl]`.
pub fn unpack_clip_radii(w: u32) -> [f32; 4] {
    if w == 0 {
        return [0.0; 4];
    }
    if (w & shader_consts::CLIP_RADII_MIXED_TAG) != shader_consts::CLIP_RADII_MIXED_TAG {
        return [f32::from_bits(w); 4];
    }
    let step = if (w & shader_consts::CLIP_RADII_COARSE_BIT) != 0 {
        shader_consts::CLIP_RADII_COARSE_STEP
    } else {
        shader_consts::CLIP_RADII_FINE_STEP
    };
    let sign = if (w & shader_consts::CLIP_RADII_CONTINUOUS_BIT) != 0 { -1.0 } else { 1.0 };
    std::array::from_fn(|idx| {
        let q = (w >> (idx as u32 * shader_consts::CLIP_RADII_LANE_BITS)) & shader_consts::CLIP_RADII_LANE_MASK;
        if q == 0 { 0.0 } else { q as f32 * step * sign }
    })
}

/// Sentinel index for `None` (invalid `params[1]` with `<= 11.5` or NaN) in the 15-entry `Kind`
/// tables. Every `KIND_NONE` entry inside `KIND_SLOT_LUT[0..30]` sits at `slot <= 23` (`<= 7.5`),
/// so any `None` with `params[1] > 11.5` always falls outside `0..30`.
const KIND_NONE: usize = Kind::COUNT;

/// Maps `(bits - 1.0_f32.to_bits()).rotate_left(12)` in `0..30` to `Kind as u8` (`0..14`) or
/// `KIND_NONE` (`14`). Derived solely from [`Kind::from_u8`] and [`Kind::as_f32`].
const KIND_SLOT_LUT: [u8; 30] = {
    const ONE: u32 = KIND_SHADOW.to_bits();
    let mut table = [KIND_NONE as u8; 30];
    let mut k = 1u8;
    while (k as usize) < Kind::COUNT {
        let Some(kind) = Kind::from_u8(k) else { panic!("dense Kind discriminants") };
        let slot = kind.as_f32().to_bits().wrapping_sub(ONE).rotate_left(12) as usize;
        assert!(slot < 30);
        table[slot] = k;
        k += 1;
    }
    table
};

/// Decodes non-zero `Instance::params[1]` bits to a dense index `0..=Kind::COUNT`
/// (`Kind::COUNT` = `None`). Caller has already handled `±0.0` (`Kind::Rect`).
#[inline(always)]
const fn kind_index_nonzero(bits: u32) -> usize {
    const ONE: u32 = KIND_SHADOW.to_bits();
    let slot = bits.wrapping_sub(ONE).rotate_left(12);
    if slot < 30 {
        KIND_SLOT_LUT[slot as usize] as usize
    } else {
        KIND_NONE
    }
}

/// Bit-exact integer predicate for `f32::from_bits(bits) > UBER_MAX_KIND_THRESHOLD` (positive
/// finite floats above the threshold and `+inf`; false for negatives, `<=` threshold, and `NaN`).
/// Keeps `bounds_core` purely in integer registers with no XMM-to-GPR transfer.
#[inline(always)]
const fn bits_gt_uber_max(bits: u32) -> bool {
    const LIMIT: u32 = shader_consts::UBER_MAX_KIND_THRESHOLD.to_bits();
    const INF: u32 = f32::INFINITY.to_bits();
    bits > LIMIT && bits <= INF
}

/// Bitmask of `Kind` indices (`0..=Kind::COUNT`) that draw no uber-shader quad (`quad_bounds` is
/// `None`, except `KIND_NONE` which only skips when `params[1] > UBER_MAX_KIND_THRESHOLD`).
const NO_UBER_QUAD_MASK: u32 =
    (1 << Kind::LayerEnd.as_u8()) | (1 << Kind::Effect.as_u8()) | (1 << Kind::BackdropBlur.as_u8());

#[derive(Clone, Copy)]
enum GrowMode {
    Quad,
    Layer,
    Damage,
}

const fn grow_spec(kind: Option<Kind>, mode: GrowMode) -> (f32, f32) {
    match (mode, kind) {
        (GrowMode::Layer | GrowMode::Damage, Some(Kind::Effect)) => (shader_consts::GROW_FLAT, 1.0),
        (GrowMode::Layer, Some(Kind::BackdropBlur) | None) => (shader_consts::BACKDROP_COMPOSITE_GROW, 0.0),
        (GrowMode::Damage, Some(Kind::BackdropBlur)) => (shader_consts::BACKDROP_COMPOSITE_GROW, 0.0),
        (_, Some(Kind::Shadow)) => (shader_consts::GROW_SHADOW_PAD, shader_consts::GROW_SHADOW_SIGMA),
        (_, Some(Kind::Glyph)) => (shader_consts::GROW_GLYPH, 0.0),
        (_, Some(Kind::Inset | Kind::Segment | Kind::Triangle | Kind::Image | Kind::Layer)) => (shader_consts::GROW_FLAT, 0.0),
        (_, Some(Kind::Rect | Kind::Gradient | Kind::Dashed | Kind::Radial) | None) => {
            (shader_consts::GROW_RECT_PAD, shader_consts::GROW_RECT_SOFT)
        }
        (_, Some(Kind::LayerEnd | Kind::Effect | Kind::BackdropBlur)) => (0.0, 0.0),
    }
}

struct GrowTable {
    base: [f32; Kind::COUNT + 1],
    scale: [f32; Kind::COUNT + 1],
    /// Bit `k` is set when `scale[k] != 0.0`, so `params[2]` is masked to `0.0` for other kinds
    /// (preserving exact IEEE-754 results even if `params[2]` were NaN or infinite).
    uses_p2: u32,
}

const fn build_grow_table(mode: GrowMode) -> GrowTable {
    let mut base = [0.0f32; Kind::COUNT + 1];
    let mut scale = [0.0f32; Kind::COUNT + 1];
    let mut uses_p2 = 0u32;
    let mut k = 0usize;
    while k <= Kind::COUNT {
        let (b, s) = grow_spec(Kind::from_u8(k as u8), mode);
        base[k] = b;
        scale[k] = s;
        if s != 0.0 {
            uses_p2 |= 1u32 << k;
        }
        k += 1;
    }
    GrowTable { base, scale, uses_p2 }
}

const QUAD_GROW: GrowTable = build_grow_table(GrowMode::Quad);
const LAYER_GROW: GrowTable = build_grow_table(GrowMode::Layer);
const DAMAGE_GROW: GrowTable = build_grow_table(GrowMode::Damage);

/// The upright bounds of edges `r` (`[x0, y0, x1, y1]`) turned `angle` radians clockwise about `p`.
#[cold]
#[inline(never)]
pub(crate) fn turned_bounds(r: [f32; 4], angle: f32, p: [f32; 2]) -> [f32; 4] {
    let (sn, c) = angle.sin_cos();
    let mut out = [f32::INFINITY, f32::INFINITY, f32::NEG_INFINITY, f32::NEG_INFINITY];
    for (x, y) in [(r[0], r[1]), (r[2], r[1]), (r[0], r[3]), (r[2], r[3])] {
        let (dx, dy) = (x - p[0], y - p[1]);
        let (qx, qy) = (p[0] + c * dx - sn * dy, p[1] + sn * dx + c * dy);
        out = [out[0].min(qx), out[1].min(qy), out[2].max(qx), out[3].max(qy)];
    }
    out
}

/// The upright bounds of a glyph rect `[x, y, w, h]` turned `angle` radians about its centre.
#[cold]
#[inline(never)]
fn turned_glyph_bounds(rect: [f32; 4], angle: f32) -> [f32; 4] {
    let (sin, cos) = angle.sin_cos();
    let (w, h) = (cos.abs() * rect[2] + sin.abs() * rect[3], sin.abs() * rect[2] + cos.abs() * rect[3]);
    let lo = [rect[0] + (rect[2] - w) * 0.5, rect[1] + (rect[3] - h) * 0.5];
    [lo[0], lo[1], lo[0] + w, lo[1] + h]
}

/// Band window clamp for a big shadow or border ring (`runtime/bands.rs`, `uv[2] < 0`).
/// Kept inline: it makes no calls (no extra XMM spills) and fires on banded rects/shadows.
#[inline(always)]
fn clamp_band(rect: [f32; 4], uv: [f32; 4], lo0: f32, lo1: f32, hi0: f32, hi1: f32) -> (f32, f32, f32, f32) {
    let lo0 = lo0.max(rect[0] + uv[0]);
    let lo1 = lo1.max(rect[1] + uv[1]);
    let hi0 = hi0.min(rect[0] - uv[2]).max(lo0);
    let hi1 = hi1.min(rect[1] + uv[3]).max(lo1);
    (lo0, lo1, hi0, hi1)
}

impl Instance {
    /// Decodes the primitive kind stored in `params[1]`.
    #[inline(always)]
    pub const fn kind(&self) -> Option<Kind> {
        Kind::from_f32(self.params[1])
    }

    #[inline(always)]
    fn grow_from(table: &GrowTable, k: usize, p2: f32) -> f32 {
        let p2 = if ((table.uses_p2 >> k) & 1) != 0 { p2 } else { 0.0 };
        table.base[k] + p2 * table.scale[k]
    }

    #[inline(always)]
    fn bounds_core<const DAMAGE: bool>(&self, glyph_rot_pad: f32) -> Option<(f32, f32, f32, f32)> {
        let bits = self.params[1].to_bits();
        // Fast path for the two dominant kinds (`Rect` and `Glyph`): pure integer tests, no LUT
        // load, no `uv` load for glyphs, and zero extra kind checks.
        if (bits & 0x7FFF_FFFF) == 0 {
            let grow = shader_consts::GROW_RECT_PAD + self.params[2] * shader_consts::GROW_RECT_SOFT;
            let lo0 = self.rect[0] - grow;
            let lo1 = self.rect[1] - grow;
            let (hi0, hi1) = if DAMAGE {
                (self.rect[0] + self.rect[2] + grow, self.rect[1] + self.rect[3] + grow)
            } else {
                (lo0 + self.rect[2] + 2.0 * grow, lo1 + self.rect[3] + 2.0 * grow)
            };
            return Some(if self.uv[2] < 0.0 {
                clamp_band(self.rect, self.uv, lo0, lo1, hi0, hi1)
            } else {
                (lo0, lo1, hi0, hi1)
            });
        }
        if bits == KIND_GLYPH.to_bits() {
            if self.params[0] != 0.0 {
                let [x0, y0, x1, y1] = turned_glyph_bounds(self.rect, self.params[0]);
                return Some((x0 - glyph_rot_pad, y0 - glyph_rot_pad, x1 + glyph_rot_pad, y1 + glyph_rot_pad));
            }
            return Some((self.rect[0], self.rect[1], self.rect[0] + self.rect[2], self.rect[1] + self.rect[3]));
        }

        // Remaining kinds (`1.0..=13.0` and invalid): branchless 30-entry LUT + affine grow table.
        const ONE: u32 = KIND_SHADOW.to_bits();
        let slot = bits.wrapping_sub(ONE).rotate_left(12);
        let k = if slot < 30 {
            KIND_SLOT_LUT[slot as usize] as usize
        } else if bits_gt_uber_max(bits) {
            return None;
        } else {
            KIND_NONE
        };

        if !DAMAGE && ((NO_UBER_QUAD_MASK >> k) & 1) != 0 {
            return None;
        }
        if DAMAGE && k == Kind::LayerEnd.as_u8() as usize {
            return None;
        }

        let p2 = if DAMAGE && k == Kind::Effect.as_u8() as usize {
            self.params[2].max(0.0)
        } else {
            self.params[2]
        };
        let table = if DAMAGE { &DAMAGE_GROW } else { &QUAD_GROW };
        let grow = Self::grow_from(table, k, p2);
        let lo0 = self.rect[0] - grow;
        let lo1 = self.rect[1] - grow;
        let (hi0, hi1) = if DAMAGE {
            (self.rect[0] + self.rect[2] + grow, self.rect[1] + self.rect[3] + grow)
        } else {
            (lo0 + self.rect[2] + 2.0 * grow, lo1 + self.rect[3] + 2.0 * grow)
        };
        // A band of a big shadow draws only its window (`runtime/bands.rs`; Rect handled above).
        if k == Kind::Shadow.as_u8() as usize && self.uv[2] < 0.0 {
            return Some(clamp_band(self.rect, self.uv, lo0, lo1, hi0, hi1));
        }
        // A turned layer composite (`Instance::turned`) turns about `border[2..4]`.
        if k == Kind::Layer.as_u8() as usize && self.border[1] != 0.0 {
            let [x0, y0, x1, y1] = turned_bounds([lo0, lo1, hi0, hi1], self.border[1], [self.border[2], self.border[3]]);
            return Some((x0, y0, x1, y1));
        }
        Some((lo0, lo1, hi0, hi1))
    }

    /// Axis-aligned device-pixel bounds `[x0, y0, x1, y1]` of the quad the uber vertex shader
    /// (`shader.wgsl` `base_vs`) emits for this instance, before clipping.
    ///
    /// Returns `None` for records that rasterize no uber-shader quad (`Kind::LayerEnd`,
    /// `Kind::Effect`, `Kind::BackdropBlur`, and out-of-range values with `params[1] > 11.5`).
    #[inline]
    pub fn quad_bounds(&self) -> Option<[f32; 4]> {
        let (x0, y0, x1, y1) = self.bounds_core::<false>(0.0)?;
        Some([x0, y0, x1, y1])
    }

    /// Pre-clip bounds for retained damage planning (`window/surface/damage.rs`): `quad_bounds`
    /// plus `Kind::Effect` (`params[2].max(0) + 1`) and `Kind::BackdropBlur` (`1`).
    #[inline(always)]
    pub(crate) fn damage_quad_bounds(&self) -> Option<(f32, f32, f32, f32)> {
        self.bounds_core::<true>(0.0)
    }

    /// [`Self::layer_bounds`] when the caller has already ruled out `Kind::Layer` and
    /// `Kind::LayerEnd` (`render/layers.rs`, `render/cpu_boot/bin.rs`). Fast-paths the two
    /// dominant kinds (`Glyph` and `Rect`) without LUT loads or `uv` reads on glyphs.
    #[inline(always)]
    pub(crate) fn layer_bounds_non_scope(&self) -> (f32, f32, f32, f32) {
        let bits = self.params[1].to_bits();
        if bits == KIND_GLYPH.to_bits() {
            let (x0, y0, x1, y1) = if self.params[0] != 0.0 {
                let [a, b, c, d] = turned_glyph_bounds(self.rect, self.params[0]);
                (a, b, c, d)
            } else {
                (self.rect[0], self.rect[1], self.rect[0] + self.rect[2], self.rect[1] + self.rect[3])
            };
            return (x0.max(self.clip[0]), y0.max(self.clip[1]), x1.min(self.clip[2]), y1.min(self.clip[3]));
        }
        if (bits & 0x7FFF_FFFF) == 0 {
            let grow = shader_consts::GROW_RECT_PAD + self.params[2] * shader_consts::GROW_RECT_SOFT;
            let lo0 = self.rect[0] - grow;
            let lo1 = self.rect[1] - grow;
            let hi0 = self.rect[0] + self.rect[2] + grow;
            let hi1 = self.rect[1] + self.rect[3] + grow;
            let (x0, y0, x1, y1) = if self.uv[2] < 0.0 {
                clamp_band(self.rect, self.uv, lo0, lo1, hi0, hi1)
            } else {
                (lo0, lo1, hi0, hi1)
            };
            return (x0.max(self.clip[0]), y0.max(self.clip[1]), x1.min(self.clip[2]), y1.min(self.clip[3]));
        }
        let k = kind_index_nonzero(bits);
        let grow = Self::grow_from(&LAYER_GROW, k, self.params[2]);
        let lo0 = self.rect[0] - grow;
        let lo1 = self.rect[1] - grow;
        let hi0 = self.rect[0] + self.rect[2] + grow;
        let hi1 = self.rect[1] + self.rect[3] + grow;
        let (x0, y0, x1, y1) = if k == Kind::Shadow.as_u8() as usize && self.uv[2] < 0.0 {
            clamp_band(self.rect, self.uv, lo0, lo1, hi0, hi1)
        } else {
            (lo0, lo1, hi0, hi1)
        };
        (x0.max(self.clip[0]), y0.max(self.clip[1]), x1.min(self.clip[2]), y1.min(self.clip[3]))
    }

    /// Device-pixel bounds `[x0, y0, x1, y1]` this instance contributes to an enclosing isolated
    /// layer (`render/layers.rs`), intersected with `self.clip`. Includes non-uber records painted
    /// in the layer pass (`Kind::Effect` with bleed `params[2] + 1`, `Kind::BackdropBlur` with `1`).
    #[inline]
    pub fn layer_bounds(&self) -> [f32; 4] {
        if matches!(self.kind(), Some(Kind::Layer | Kind::LayerEnd)) {
            return [0.0; 4];
        }
        let (x0, y0, x1, y1) = self.layer_bounds_non_scope();
        [x0, y0, x1, y1]
    }

    /// The pixels `[x0, y0, x1, y1)` whose centres lie in the quad `shader.wgsl` (`base_vs`) draws
    /// for this instance, within the reach of its clip and a `w × h` target. Empty (`[0; 4]`) for
    /// records that rasterize nothing in the uber shader.
    pub(crate) fn pixel_bounds(&self, w: i32, h: i32) -> [i32; 4] {
        // A rotated glyph gets a 1 px margin for trig rounding between CPU and GPU.
        let Some((x0, y0, x1, y1)) = self.bounds_core::<false>(1.0) else { return [0; 4] };
        // Centres in [lo, hi): the rasterizer's top-left rule.
        // `ceil(a).min(ceil(b)) == ceil(a.min(b))` for all floats (including NaN), and when
        // `x0 >= c[0]` or `c[0] <= 0.5` the clip floor cannot exceed `ceil(x0 - 0.5).max(0.0)`.
        let c = self.clip;
        let x0_ceil = (x0 - 0.5).ceil();
        let y0_ceil = (y0 - 0.5).ceil();
        let px0 = if x0 >= c[0] || c[0] <= 0.5 { x0_ceil } else { x0_ceil.max((c[0] - 0.5).floor()) }.max(0.0);
        let py0 = if y0 >= c[1] || c[1] <= 0.5 { y0_ceil } else { y0_ceil.max((c[1] - 0.5).floor()) }.max(0.0);
        let px1 = (x1 - 0.5).min(c[2] + 0.5).ceil().min(w as f32);
        let py1 = (y1 - 0.5).min(c[3] + 0.5).ceil().min(h as f32);
        if !(px1 > px0 && py1 > py0) {
            return [0; 4];
        }
        [px0 as i32, py0 as i32, px1 as i32, py1 as i32]
    }

    /// Rounded clipping shares the packed fade ABI across primitives and layer composites.
    /// A negative radius keeps its sign: continuous corners (`corner_extent`).
    pub(crate) fn round_clip(&mut self, radius: f32) {
        self.round_clip_corners([radius; 4]);
    }

    pub(crate) fn round_clip_corners(&mut self, radii: [f32; 4]) {
        self.fade[3] = pack_clip_radii(self.clip, radii);
    }

    /// Begin an isolated scope. `bounds` is a device-pixel `[x, y, w, h]` content bound, including
    /// native surfaces. Primitive bounds (including shadow support) expand it during prepare.
    pub fn begin_layer(bounds: [f32; 4], clip: [f32; 4], spec: LayerSpec) -> Self {
        let mut instance = Self { rect: bounds, clip, fill: [0.0, 0.0, 0.0, spec.opacity.clamp(0.0, 1.0)],
            params: [if spec.blend == Blend::PlusLighter { shader_consts::LAYER_ADDITIVE_PARAM } else { 0.0 }, KIND_LAYER,
                layer_blur::sigma(spec.blur), 0.0], ..Self::zeroed() };
        instance.round_clip(spec.clip_radius);
        instance
    }

    /// Turn this scope's composite `angle` radians clockwise about `pivot` (device pixels).
    /// The contents draw upright; the layer's clip applies after the turn. Layer records keep
    /// it in `border[1..4]` as `[angle, pivot.x, pivot.y]`.
    pub fn turned(mut self, angle: f32, pivot: [f32; 2]) -> Self {
        self.border = [0.0, angle, pivot[0], pivot[1]];
        self
    }

    pub fn end_layer() -> Self {
        Self { params: [0.0, KIND_LAYER_END, 0.0, 0.0], ..Self::zeroed() }
    }

    /// The `El::effect` tag carried by a `Kind::Effect` record (`EffectSite::mark`), or `None`
    /// for any other primitive kind.
    #[inline]
    pub const fn effect_mark(&self) -> Option<u32> {
        if !matches!(self.kind(), Some(Kind::Effect)) {
            return None;
        }
        Some(effect_mark(self))
    }

    /// Scales only the colour and opacity fields of this instance by `opacity`, leaving
    /// non-colour payloads (outline glyph records, wrapped-image placement rects, layer pivots,
    /// effect marks, wrap modes, etc.) untouched. Mirrors `crates/rune-gpu/src/springs.wgsl`.
    #[doc(hidden)]
    pub fn scale_opacity(&mut self, opacity: f32) {
        if opacity == 1.0 {
            return;
        }
        let Some(kind) = self.kind() else {
            return;
        };
        let f = kind.opacity_fields();
        if f.fill { self.fill = self.fill.map(|v| v * opacity); }
        if f.border { self.border = self.border.map(|v| v * opacity); }
        if f.uv { self.uv = self.uv.map(|v| v * opacity); }
        if f.params_w { self.params[3] *= opacity; }
    }
}

/// An effect slot's `El::effect` tag, from the two exact 16-bit floats in `border` (`KIND_EFFECT`).
#[inline]
pub(crate) const fn effect_mark(i: &Instance) -> u32 {
    (i.border[0] as u32 & 0xFFFF) | ((i.border[1] as u32 & 0xFFFF) << 16)
}

/// Pack a 64-bit integer into four exact non-negative 16-bit integer floats (`0.0..=65535.0`).
pub(crate) fn pack_u64_f32(v: u64) -> [f32; 4] {
    [
        (v & 0xFFFF) as f32,
        ((v >> 16) & 0xFFFF) as f32,
        ((v >> 32) & 0xFFFF) as f32,
        ((v >> 48) & 0xFFFF) as f32,
    ]
}

/// Unpack a 64-bit integer from [`pack_u64_f32`].
pub(crate) fn unpack_u64_f32(a: [f32; 4]) -> u64 {
    (a[0] as u64 & 0xFFFF)
        | ((a[1] as u64 & 0xFFFF) << 16)
        | ((a[2] as u64 & 0xFFFF) << 32)
        | ((a[3] as u64 & 0xFFFF) << 48)
}

/// Close a runtime scope, folding empty/single-draw groups back into the direct path. Only the
/// last one or two records move, so video insertions before this scope keep their indices.
pub(crate) fn end_layer(instances: &mut Vec<Instance>, surfaces: &mut [crate::native_surface::SurfaceDraw], marks: &mut Vec<u32>, start: usize, surface_start: usize) {
    let primitives = instances.len() - start - 1;
    let videos = surfaces.len() - surface_start;
    let scope = instances[start];
    let unmasked = if primitives == 1 {
        instances[start + 1].clip == NO_CLIP && instances[start + 1].fade == [0; 4]
    } else if videos == 1 {
        surfaces[surface_start].clip == NO_CLIP && surfaces[surface_start].fade == [0; 4]
    } else { true };
    let transfer_clip = scope.fade[3] == 0 && unmasked;
    // A lone effect slot or backdrop blur keeps a faded scope: an effect compositor can't take
    // the opacity, and an ancestor with opacity < 1 is a backdrop root whose blur sees only that
    // layer, as in CSS Filter Effects 2.
    let faded_effect = primitives == 1
        && matches!(instances[start + 1].kind(), Some(Kind::Effect | Kind::BackdropBlur))
        && scope.fill[3] != 1.0;
    // A layer's `radii` hold its hole (`El::cutout_circle`), which only its composite can cut.
    if !faded_effect && primitives + videos <= 1 && (primitives + videos == 0 || (scope.border[1] == 0.0 && scope.radii == [0.0; 4])) && scope.params[0] == 0.0 && scope.params[2] == 0.0 && scope.params[3] == 0.0
        && (transfer_clip || (scope.clip == NO_CLIP && scope.fade == [0; 4])) {
        let opacity = scope.fill[3];
        if primitives == 1 {
            let i = &mut instances[start + 1];
            if transfer_clip { i.clip = scope.clip; i.fade = scope.fade; }
            i.scale_opacity(opacity);
        }
        if videos == 1 {
            if transfer_clip { surfaces[surface_start].clip = scope.clip; surfaces[surface_start].fade = scope.fade; }
            surfaces[surface_start].opacity *= opacity;
            surfaces[surface_start].before_instance -= 1;
        }
        instances.remove(start);
        if marks.len() > start { marks.remove(start); }
    } else {
        instances.push(Instance::end_layer());
        if let Some(&mark) = marks.get(start) { marks.push(mark); }
    }
}
