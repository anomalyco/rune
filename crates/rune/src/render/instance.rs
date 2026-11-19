//! The per-instance and per-frame GPU ABI shared by the uber shader, layer planner, CPU
//! boot painter and Windows damage planner.

use std::mem::{offset_of, size_of};

use bytemuck::{Pod, Zeroable};

use super::{Blend, LayerSpec, layer_blur};

/// Primitive kind stored in [`Instance::params`]`[1]`.
///
/// The `KIND_*` float constants derive from these discriminants so every Rust consumer shares one
/// numeric table with `shader.wgsl`.
#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Kind {
    /// Rounded rect in `fill` with a `params.x`-wide `border` ring over it (`KIND_RECT`).
    Rect = 0,
    /// Analytic Gaussian rounded-rect shadow (`KIND_SHADOW`).
    Shadow = 1,
    /// Glyph-atlas or outline text (`KIND_GLYPH`).
    Glyph = 2,
    /// Inset shadow inside a rounded rect (`KIND_INSET`).
    Inset = 3,
    /// Rounded rect or border with a vertical gradient (`KIND_GRADIENT`).
    Gradient = 4,
    /// Antialiased stroke segment (`KIND_SEGMENT`).
    Segment = 5,
    /// Filled triangle (`KIND_TRIANGLE`).
    Triangle = 6,
    /// RGBA image from the image atlas (`KIND_IMAGE`).
    Image = 7,
    /// Isolated layer scope open / composite quad (`KIND_LAYER`).
    Layer = 8,
    /// Isolated layer scope close (`KIND_LAYER_END`).
    LayerEnd = 9,
    /// Dashed border ring (`KIND_DASHED`).
    Dashed = 10,
    /// Radial or angled linear gradient stop segment (`KIND_RADIAL`).
    Radial = 11,
    /// Compositor effect slot (`KIND_EFFECT`).
    Effect = 12,
    /// In-window backdrop blur (`KIND_BACKDROP_BLUR`).
    BackdropBlur = 13,
}

const _: () = assert!(size_of::<Option<Kind>>() == 1);

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

    #[inline]
    pub const fn from_u8(v: u8) -> Option<Kind> {
        match v {
            0 => Some(Kind::Rect),
            1 => Some(Kind::Shadow),
            2 => Some(Kind::Glyph),
            3 => Some(Kind::Inset),
            4 => Some(Kind::Gradient),
            5 => Some(Kind::Segment),
            6 => Some(Kind::Triangle),
            7 => Some(Kind::Image),
            8 => Some(Kind::Layer),
            9 => Some(Kind::LayerEnd),
            10 => Some(Kind::Dashed),
            11 => Some(Kind::Radial),
            12 => Some(Kind::Effect),
            13 => Some(Kind::BackdropBlur),
            _ => None,
        }
    }

    /// Lossless conversion from the `f32` stored in `Instance::params[1]`.
    ///
    /// Matches IEEE-754 bit patterns directly so per-instance loops compile to integer compares
    /// with no float-to-int saturating cast, and non-integer or out-of-range values return `None`.
    #[inline]
    pub const fn from_f32(v: f32) -> Option<Kind> {
        const RECT: u32 = KIND_RECT.to_bits();
        const NEG_ZERO: u32 = (-0.0f32).to_bits();
        const SHADOW: u32 = KIND_SHADOW.to_bits();
        const GLYPH: u32 = KIND_GLYPH.to_bits();
        const INSET: u32 = KIND_INSET.to_bits();
        const GRADIENT: u32 = KIND_GRADIENT.to_bits();
        const SEGMENT: u32 = KIND_SEGMENT.to_bits();
        const TRIANGLE: u32 = KIND_TRIANGLE.to_bits();
        const IMAGE: u32 = KIND_IMAGE.to_bits();
        const LAYER: u32 = KIND_LAYER.to_bits();
        const LAYER_END: u32 = KIND_LAYER_END.to_bits();
        const DASHED: u32 = KIND_DASHED.to_bits();
        const RADIAL: u32 = KIND_RADIAL.to_bits();
        const EFFECT: u32 = KIND_EFFECT.to_bits();
        const BACKDROP_BLUR: u32 = KIND_BACKDROP_BLUR.to_bits();
        match v.to_bits() {
            RECT | NEG_ZERO => Some(Kind::Rect),
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
    /// [0] = pack2x16float(left, top), [1] = pack2x16float(right, bottom) widths; a negative
    /// width ramps linearly, a positive one on smoothstep.
    /// [2] = pack4x8unorm(clamp(inward origin→clip distance / width, 0, 1)), LTRB.
    /// [3] = signed uniform f32 clip-radius bits or tagged mixed radii (`pack_clip_radii`).
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

    use super::{Instance, word_of};

    const _: () = {
        assert!(size_of::<Instance>() == 128);
        assert!(offset_of!(Instance, rect).is_multiple_of(4));
        assert!(offset_of!(Instance, radii).is_multiple_of(4));
        assert!(offset_of!(Instance, fill).is_multiple_of(4));
        assert!(offset_of!(Instance, border).is_multiple_of(4));
        assert!(offset_of!(Instance, params).is_multiple_of(4));
        assert!(offset_of!(Instance, uv).is_multiple_of(4));
        assert!(offset_of!(Instance, clip).is_multiple_of(4));
        assert!(offset_of!(Instance, fade).is_multiple_of(4));
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

pub fn pack_fade(width: [f32; 4], origin: [f32; 4], clip: [f32; 4]) -> [u32; 4] {
    // A negative width is a linear ramp of that width (`El::clip_fade_linear`).
    let halves = width.map(|w| half::f16::from_f32(w.clamp(-65504.0, 65504.0)).to_bits() as u32);
    let mut offsets = 0;
    for edge in 0..4 {
        let distance = if edge < 2 { clip[edge] - origin[edge] } else { origin[edge] - clip[edge] };
        let w = width[edge].abs();
        let offset = if w > 0.0 { (distance / w).clamp(0.0, 1.0) } else { 0.0 };
        offsets |= ((offset * 255.0 + 0.5) as u32) << (edge * 8);
    }
    [halves[0] | (halves[1] << 16), halves[2] | (halves[3] << 16), offsets, 0]
}

/// Decode widths and normalized origin offsets for tools inspecting the instance stream. A
/// negative width is a linear ramp.
pub fn unpack_fade(fade: [u32; 4]) -> ([f32; 4], [f32; 4]) {
    (std::array::from_fn(|e| half::f16::from_bits((fade[e / 2] >> ((e % 2) * 16)) as u16).to_f32()),
     std::array::from_fn(|e| ((fade[2] >> (e * 8)) & 255) as f32 / 255.0))
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
    let half = ((clip[2] - clip[0]).min(clip[3] - clip[1]) * 0.5).max(0.0);
    let clamped = radii.map(|radius| {
        let r = radius.abs().min(half).min(f32::from_bits(0x5fff_ffff));
        if radius < 0.0 && r > 0.0 { -r } else { r }
    });
    if clamped[0] == clamped[1] && clamped[1] == clamped[2] && clamped[2] == clamped[3] {
        return if clamped[0] == 0.0 { 0 } else { clamped[0].to_bits() };
    }
    let continuous = clamped.iter().any(|&r| r < 0.0);
    let max_r = clamped.iter().fold(0.0f32, |m, r| m.max(r.abs()));
    let coarse = max_r > 63.5;
    let inv_step = if coarse { 0.5 } else { 2.0 };
    let mut bits = 0x6000_0000u32
        | if continuous { 0x8000_0000 } else { 0 }
        | if coarse { 0x1000_0000 } else { 0 };
    for (idx, r) in clamped.into_iter().enumerate() {
        let q = (r.abs() * inv_step).round().clamp(0.0, 127.0) as u32;
        bits |= q << (idx * 7);
    }
    bits
}

/// Decode `Instance::fade[3]` into four signed corner radii `[tl, tr, br, bl]`.
pub fn unpack_clip_radii(w: u32) -> [f32; 4] {
    if w == 0 {
        return [0.0; 4];
    }
    if (w & 0x6000_0000) != 0x6000_0000 {
        return [f32::from_bits(w); 4];
    }
    let step = if (w & 0x1000_0000) != 0 { 2.0 } else { 0.5 };
    let sign = if (w & 0x8000_0000) != 0 { -1.0 } else { 1.0 };
    std::array::from_fn(|idx| {
        let q = (w >> (idx * 7)) & 0x7f;
        if q == 0 { 0.0 } else { q as f32 * step * sign }
    })
}

/// The upright bounds of edges `r` (`[x0, y0, x1, y1]`) turned `angle` radians clockwise about `p`.
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

impl Instance {
    /// Decodes the primitive kind stored in `params[1]`.
    #[inline]
    pub const fn kind(&self) -> Option<Kind> {
        Kind::from_f32(self.params[1])
    }

    /// Axis-aligned device-pixel bounds `[x0, y0, x1, y1]` of the quad the uber vertex shader
    /// (`shader.wgsl` `base_vs`) emits for this instance, before clipping.
    ///
    /// Returns `None` for records that rasterize no uber-shader quad (`Kind::LayerEnd`,
    /// `Kind::Effect`, `Kind::BackdropBlur`, and out-of-range values with `params[1] > 11.5`).
    pub fn quad_bounds(&self) -> Option<[f32; 4]> {
        let kind = self.kind();
        let grow = match kind {
            Some(Kind::LayerEnd | Kind::Effect | Kind::BackdropBlur) => return None,
            None if self.params[1] > 11.5 => return None,
            Some(Kind::Shadow) => self.params[2] * 3.0 + 1.0,
            Some(Kind::Glyph) => 0.0,
            Some(Kind::Inset | Kind::Segment | Kind::Triangle | Kind::Image | Kind::Layer) => 1.0,
            Some(Kind::Rect | Kind::Gradient | Kind::Dashed | Kind::Radial) | None => 1.0 + self.params[2] * 2.0,
        };
        let mut lo = [self.rect[0] - grow, self.rect[1] - grow];
        let mut hi = [lo[0] + self.rect[2] + 2.0 * grow, lo[1] + self.rect[3] + 2.0 * grow];
        // A band of a big shadow or border ring draws only its window (`runtime/bands.rs`).
        if matches!(kind, Some(Kind::Rect | Kind::Shadow)) && self.uv[2] < 0.0 {
            lo = [lo[0].max(self.rect[0] + self.uv[0]), lo[1].max(self.rect[1] + self.uv[1])];
            hi = [hi[0].min(self.rect[0] - self.uv[2]).max(lo[0]), hi[1].min(self.rect[1] + self.uv[3]).max(lo[1])];
        }
        // A turned icon's quad turns about its centre: its bounds are the turned quad's.
        if kind == Some(Kind::Glyph) && self.params[0] != 0.0 {
            let (sin, cos) = self.params[0].sin_cos();
            let (w, h) = (cos.abs() * self.rect[2] + sin.abs() * self.rect[3], sin.abs() * self.rect[2] + cos.abs() * self.rect[3]);
            lo = [self.rect[0] + (self.rect[2] - w) * 0.5, self.rect[1] + (self.rect[3] - h) * 0.5];
            hi = [lo[0] + w, lo[1] + h];
        }
        // A turned layer composite (`Instance::turned`) turns about `border[2..4]`.
        if kind == Some(Kind::Layer) && self.border[1] != 0.0 {
            return Some(turned_bounds([lo[0], lo[1], hi[0], hi[1]], self.border[1], [self.border[2], self.border[3]]));
        }
        Some([lo[0], lo[1], hi[0], hi[1]])
    }

    /// Device-pixel bounds `[x0, y0, x1, y1]` this instance contributes to an enclosing isolated
    /// layer (`render/layers.rs`), intersected with `self.clip`. Includes non-uber records painted
    /// in the layer pass (`Kind::Effect` with bleed `params[2] + 1`, `Kind::BackdropBlur` with `1`).
    pub fn layer_bounds(&self) -> [f32; 4] {
        let [x0, y0, x1, y1] = match self.kind() {
            Some(Kind::Effect) => {
                let grow = self.params[2] + 1.0;
                [self.rect[0] - grow, self.rect[1] - grow, self.rect[0] + self.rect[2] + grow, self.rect[1] + self.rect[3] + grow]
            }
            Some(Kind::BackdropBlur) => [self.rect[0] - 1.0, self.rect[1] - 1.0, self.rect[0] + self.rect[2] + 1.0, self.rect[1] + self.rect[3] + 1.0],
            Some(Kind::Layer | Kind::LayerEnd) => return [0.0; 4],
            Some(_) => match self.quad_bounds() {
                Some(b) => b,
                None => return [0.0; 4],
            },
            None => [self.rect[0] - 1.0, self.rect[1] - 1.0, self.rect[0] + self.rect[2] + 1.0, self.rect[1] + self.rect[3] + 1.0],
        };
        [x0.max(self.clip[0]), y0.max(self.clip[1]), x1.min(self.clip[2]), y1.min(self.clip[3])]
    }

    /// The pixels `[x0, y0, x1, y1)` whose centres lie in the quad `shader.wgsl` (`base_vs`) draws
    /// for this instance, within the reach of its clip and a `w × h` target. Empty (`[0; 4]`) for
    /// records that rasterize nothing in the uber shader.
    pub(crate) fn pixel_bounds(&self, w: i32, h: i32) -> [i32; 4] {
        let Some([mut x0, mut y0, mut x1, mut y1]) = self.quad_bounds() else { return [0; 4] };
        // A rotated glyph gets a 1 px margin for trig rounding between CPU and GPU.
        if self.kind() == Some(Kind::Glyph) && self.params[0] != 0.0 {
            x0 -= 1.0;
            y0 -= 1.0;
            x1 += 1.0;
            y1 += 1.0;
        }
        // Centres in [lo, hi): the rasterizer's top-left rule.
        let first = |v: f32| (v - 0.5).ceil();
        let c = self.clip;
        let px0 = first(x0).max((c[0] - 0.5).floor()).max(0.0);
        let py0 = first(y0).max((c[1] - 0.5).floor()).max(0.0);
        let px1 = first(x1).min((c[2] + 0.5).ceil()).min(w as f32);
        let py1 = first(y1).min((c[3] + 0.5).ceil()).min(h as f32);
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

    /// Begin an isolated scope. `bounds` is a device-pixel [x,y,w,h] content bound, including
    /// native surfaces. Primitive bounds (including shadow support) expand it during prepare.
    pub fn begin_layer(bounds: [f32; 4], clip: [f32; 4], spec: LayerSpec) -> Self {
        let mut instance = Self { rect: bounds, clip, fill: [0.0, 0.0, 0.0, spec.opacity.clamp(0.0, 1.0)],
            params: [if spec.blend == Blend::PlusLighter { 1.0 } else { 0.0 }, KIND_LAYER,
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
}

/// An effect slot's `El::effect` tag, from the two exact 16-bit floats in `border` (`KIND_EFFECT`).
pub(crate) fn effect_mark(i: &Instance) -> u32 {
    (i.border[0] as u32 & 0xFFFF) | (i.border[1] as u32 & 0xFFFF) << 16
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
    // A lone effect slot keeps a faded scope: the compositor's paint can't take the opacity.
    let faded_effect = primitives == 1 && instances[start + 1].kind() == Some(Kind::Effect) && scope.fill[3] != 1.0;
    // A layer's `radii` hold its hole (`El::cutout_circle`), which only its composite can cut.
    if !faded_effect && primitives + videos <= 1 && (primitives + videos == 0 || (scope.border[1] == 0.0 && scope.radii == [0.0; 4])) && scope.params[0] == 0.0 && scope.params[2] == 0.0 && scope.params[3] == 0.0
        && (transfer_clip || (scope.clip == NO_CLIP && scope.fade == [0; 4])) {
        let opacity = scope.fill[3];
        if primitives == 1 {
            let i = &mut instances[start + 1];
            if transfer_clip { i.clip = scope.clip; i.fade = scope.fade; }
            i.fill = i.fill.map(|v| v * opacity);
            // A glyph's `border` holds no colour (an outline glyph's names its record).
            // A box's `params.w` is the opacity its colours carry (shader.wgsl `border_over`).
            match i.kind() {
                Some(Kind::Glyph) => {}
                Some(Kind::Rect) => {
                    i.border = i.border.map(|v| v * opacity);
                    i.params[3] *= opacity;
                }
                Some(Kind::Gradient) => {
                    i.border = i.border.map(|v| v * opacity);
                    i.uv = i.uv.map(|v| v * opacity);
                    i.params[3] *= opacity;
                }
                Some(
                    Kind::Shadow | Kind::Inset | Kind::Segment | Kind::Triangle | Kind::Image
                    | Kind::Layer | Kind::LayerEnd | Kind::Dashed | Kind::Radial
                    | Kind::Effect | Kind::BackdropBlur,
                ) | None => {
                    i.border = i.border.map(|v| v * opacity);
                }
            }
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
