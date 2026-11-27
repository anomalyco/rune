//! Windows 11 Acrylic Luminosity + Color recipe and WinRT/Direct2D effect graph.
//!
//! Implements Microsoft's documented Windows 11 Acrylic recipe (`AcrylicBrush.cpp` and
//! `AcrylicBrush_themeresources.xaml`) on top of DWM's already-pre-blurred `HostBackdropBrush`
//! with zero Win2D and zero new crates, using documented `IGraphicsEffect` +
//! `IGraphicsEffectD2D1Interop` (`windows.graphics.effects.interop.h`, Microsoft Learn) and
//! `CLSID_D2D1Flood` / `CLSID_D2D1Blend` / `CLSID_D2D1ColorMatrix` (`d2d1effects.h`).

use crate::system_material::MAX_MATERIAL_SATURATION;
use crate::{Color, MaterialParams, SystemMaterial};

/// D2D1 `CLSID_D2D1Blend` mode value for `D2D1_BLEND_MODE_COLOR` (`d2d1effects.h`).
///
/// In WinUI's `AcrylicBrush.cpp` D2D input-order mapping (`source[0]` = background backdrop,
/// `source[1]` = foreground flood), `D2D1_BLEND_MODE_COLOR` (22) applies the Luminosity blend
/// step (taking luminosity from `source[1]` and hue/saturation from `source[0]`).
pub(crate) const D2D1_BLEND_MODE_COLOR_VALUE: u32 = 22;

/// D2D1 `CLSID_D2D1Blend` mode value for `D2D1_BLEND_MODE_LUMINOSITY` (`d2d1effects.h`).
///
/// In WinUI's `AcrylicBrush.cpp` D2D input-order mapping (`source[0]` = previous backdrop result,
/// `source[1]` = foreground tint flood), `D2D1_BLEND_MODE_LUMINOSITY` (23) applies the Color/Tint
/// blend step (taking hue/saturation from `source[1]` and luminosity from `source[0]`).
pub(crate) const D2D1_BLEND_MODE_LUMINOSITY_VALUE: u32 = 23;

/// Rec. 709 luma weights matching Direct2D's saturation effect and CSS `filter: saturate()`.
pub(crate) const LUMA_R: f32 = 0.2126;
pub(crate) const LUMA_G: f32 = 0.7152;
pub(crate) const LUMA_B: f32 = 0.0722;

/// WCAG >=4.5:1 flat-tint ARGB fallback (`#BF202022` dark: 7.1:1 vs `#FFFFFF`, 6.2:1 vs Rune dark
/// text; `#B8FCFCFC` light: 10.3:1 vs `#000000`, 8.4:1 vs Rune light text), used for
/// `RUNE_WIN_MATERIAL_TINT=flat` and whenever `CreateEffectFactory` fails at runtime.
pub(crate) const FLAT_TINT_DARK: [u8; 4] = [0xBF, 0x20, 0x20, 0x22];
pub(crate) const FLAT_TINT_LIGHT: [u8; 4] = [0xB8, 0xFC, 0xFC, 0xFC];

/// Returns the WCAG >=4.5:1 ARGB flat-tint color for `dark`.
pub(crate) const fn flat_tint_color(dark: bool) -> [u8; 4] {
    if dark { FLAT_TINT_DARK } else { FLAT_TINT_LIGHT }
}

/// Quantized, hashable cache key for one compiled Acrylic `CompositionEffectBrush`.
///
/// Stores opacities in thousandths (`0..=1000`, exact for all 6 WinUI presets) and saturation
/// in hundredths (`0..=400`, where `100` is unchanged `1.0`), avoiding float hashing hazards
/// and deduplicating visually indistinguishable slider values.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) struct AcrylicBrushKey {
    /// Straight sRGB tint `[r, g, b]` (`0..=255`).
    pub tint: [u8; 3],
    /// Color/Tint blend pass opacity in thousandths (`0..=1000`).
    pub tint_milli: u16,
    /// Luminosity blend pass opacity in thousandths (`0..=1000`).
    pub luminosity_milli: u16,
    /// Backdrop saturation multiplier in hundredths (`0..=400`; `100` = `1.0`).
    pub saturation_centi: u16,
}

/// Resolved brush configuration for one pooled Windows `SpriteVisual`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum BrushChoice {
    /// Cached Windows 11 Acrylic `CompositionEffectBrush` for the resolved recipe key.
    Acrylic { key: AcrylicBrushKey },
    /// `HostBackdropBrush` plus WCAG >=4.5:1 flat-tint `CompositionColorBrush` child visual.
    FlatTint { argb: [u8; 4] },
    /// Raw `HostBackdropBrush` with no tint child visual (`RUNE_WIN_MATERIAL_TINT=raw`).
    RawHostBackdrop,
}

/// Pure brush-selection rule: selects Acrylic when `TintMode::Acrylic` and effect compilation is
/// available, degrades gracefully to WCAG >=4.5:1 flat tint on effect failure (never raw
/// unreadable blur or `State::Failed`), and honors explicit `Flat` / `Raw` diagnostic modes.
pub(crate) fn resolve_brush_choice(
    mode: super::TintMode,
    material: SystemMaterial,
    params: MaterialParams,
    dark: bool,
    acrylic_available: bool,
) -> BrushChoice {
    match mode {
        super::TintMode::Acrylic if acrylic_available => {
            let key = AcrylicRecipe::resolve(material, dark, params).cache_key();
            BrushChoice::Acrylic { key }
        }
        super::TintMode::Acrylic | super::TintMode::Flat => {
            BrushChoice::FlatTint { argb: flat_tint_color(dark) }
        }
        super::TintMode::Raw => BrushChoice::RawHostBackdrop,
    }
}

/// Builds the 5x4 row-major `D2D_MATRIX_5X4_F` for Direct2D `CLSID_D2D1ColorMatrix`
/// implementing colour saturation `s` (`0.0` = monochrome, `1.0` = identity, `> 1.0` = boost).
///
/// Uses `CLSID_D2D1ColorMatrix` rather than `CLSID_D2D1Saturation` because Direct2D's
/// `D2D1_SATURATION_PROP_SATURATION` is restricted to `0.0..=1.0`, whereas over-saturation
/// (`> 1.0`, e.g. Apple-style vibrant glass) requires a full colour matrix.
///
/// Direct2D multiplies row vectors `[R, G, B, A] * M_4x4 + M_row5`: `out = luma * (1 - s) + in * s`,
/// preserving alpha and leaving the translation row zeroed.
pub(crate) fn saturation_color_matrix(saturation: f32) -> [f32; 20] {
    let s = if saturation.is_finite() {
        saturation.clamp(0.0, MAX_MATERIAL_SATURATION)
    } else {
        1.0
    };
    let inv = 1.0 - s;
    let r = LUMA_R * inv;
    let g = LUMA_G * inv;
    let b = LUMA_B * inv;
    [
        r + s, r,     r,     0.0,
        g,     g + s, g,     0.0,
        b,     b,     b + s, 0.0,
        0.0,   0.0,   0.0,   1.0,
        0.0,   0.0,   0.0,   0.0,
    ]
}

/// Converts a linear Rune [`Color`] to straight sRGB `[r, g, b]` bytes (`0..=255`).
fn linear_color_to_srgb_rgb(c: Color) -> [u8; 3] {
    let enc = |v: f32| {
        let cl = if v.is_finite() { v.clamp(0.0, 1.0) } else { 0.0 };
        (rune_core::color::srgb_encode(cl) * 255.0).round() as u8
    };
    [enc(c.r), enc(c.g), enc(c.b)]
}

/// Windows 11 Acrylic Luminosity + Color (+ optional Saturation) parameters.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct AcrylicRecipe {
    /// Straight sRGB tint colour `[r, g, b]` (`0..=255`).
    pub tint: [u8; 3],
    /// Opacity of the Color/Tint blend pass (`0.0..=1.0`).
    pub tint_opacity: f32,
    /// Opacity of the Luminosity blend pass (`0.0..=1.0`).
    pub luminosity_opacity: f32,
    /// Backdrop colour saturation multiplier (`0.0..=4.0`; `1.0` = unchanged, omits ColorMatrix).
    pub saturation: f32,
}

impl AcrylicRecipe {
    /// Returns Microsoft's published Windows 11 Acrylic recipe for `(material, dark)`:
    /// - `SystemMaterial::Popover` (and `_`): `AcrylicBackgroundFillColorDefaultBrush`
    ///   (dark `#2C2C2C` tint `0.15` / lum `0.96`; light `#FCFCFC` tint `0.00` / lum `0.85`).
    /// - `SystemMaterial::Sidebar`: `AcrylicBackgroundFillColorBaseBrush`
    ///   (dark `#202020` tint `0.50` / lum `0.96`; light `#F3F3F3` tint `0.00` / lum `0.90`).
    /// - `SystemMaterial::Glass`: clearer chromatic Acrylic preserving full backdrop hue while
    ///   exceeding WCAG AAA 7:1 over pure white and pure black
    ///   (dark `#2C2C2C` tint `0.00` / lum `0.85`; light `#FCFCFC` tint `0.00` / lum `0.78`).
    pub(crate) const fn for_material(material: SystemMaterial, dark: bool) -> Self {
        match (material, dark) {
            (SystemMaterial::Sidebar, true) => Self {
                tint: [0x20, 0x20, 0x20],
                tint_opacity: 0.50,
                luminosity_opacity: 0.96,
                saturation: 1.0,
            },
            (SystemMaterial::Sidebar, false) => Self {
                tint: [0xF3, 0xF3, 0xF3],
                tint_opacity: 0.00,
                luminosity_opacity: 0.90,
                saturation: 1.0,
            },
            (SystemMaterial::Glass, true) => Self {
                tint: [0x2C, 0x2C, 0x2C],
                tint_opacity: 0.00,
                luminosity_opacity: 0.85,
                saturation: 1.0,
            },
            (SystemMaterial::Glass, false) => Self {
                tint: [0xFC, 0xFC, 0xFC],
                tint_opacity: 0.00,
                luminosity_opacity: 0.78,
                saturation: 1.0,
            },
            // SystemMaterial::Popover and any future non_exhaustive variant.
            (_, true) => Self {
                tint: [0x2C, 0x2C, 0x2C],
                tint_opacity: 0.15,
                luminosity_opacity: 0.96,
                saturation: 1.0,
            },
            (_, false) => Self {
                tint: [0xFC, 0xFC, 0xFC],
                tint_opacity: 0.00,
                luminosity_opacity: 0.85,
                saturation: 1.0,
            },
        }
    }

    /// Resolves `(material, dark)` with any per-element [`MaterialParams`] overrides applied.
    pub(crate) fn resolve(material: SystemMaterial, dark: bool, params: MaterialParams) -> Self {
        let mut r = Self::for_material(material, dark);
        if let Some(c) = params.tint {
            r.tint = linear_color_to_srgb_rgb(c);
        }
        if let Some(t) = params.tint_opacity {
            r.tint_opacity = t.clamp(0.0, 1.0);
        }
        if let Some(l) = params.luminosity_opacity {
            r.luminosity_opacity = l.clamp(0.0, 1.0);
        }
        if let Some(s) = params.saturation {
            r.saturation = s.clamp(0.0, MAX_MATERIAL_SATURATION);
        }
        r
    }

    /// Quantizes this recipe into its hashable [`AcrylicBrushKey`].
    pub(crate) fn cache_key(self) -> AcrylicBrushKey {
        let q_milli = |v: f32| (v.clamp(0.0, 1.0) * 1000.0).round() as u16;
        let q_centi = |v: f32| (v.clamp(0.0, MAX_MATERIAL_SATURATION) * 100.0).round() as u16;
        AcrylicBrushKey {
            tint: self.tint,
            tint_milli: q_milli(self.tint_opacity),
            luminosity_milli: q_milli(self.luminosity_opacity),
            saturation_centi: q_centi(self.saturation),
        }
    }

    /// Reconstructs the quantized [`AcrylicRecipe`] from an [`AcrylicBrushKey`].
    #[cfg_attr(not(any(test, all(target_os = "windows", feature = "windows-system-material"))), allow(dead_code))]
    pub(crate) fn from_key(key: AcrylicBrushKey) -> Self {
        Self {
            tint: key.tint,
            tint_opacity: f32::from(key.tint_milli.min(1000)) / 1000.0,
            luminosity_opacity: f32::from(key.luminosity_milli.min(1000)) / 1000.0,
            saturation: f32::from(key.saturation_centi.min(400)) / 100.0,
        }
    }

    /// Builds the pure Direct2D effect graph description for this recipe (4 nodes when
    /// `saturation == 1.0`, or 5 nodes with a `CLSID_D2D1ColorMatrix` saturation stage before
    /// `LuminosityBlend` when `saturation != 1.0`).
    pub(crate) fn graph_desc(self) -> AcrylicGraphDesc {
        let r = f32::from(self.tint[0]) / 255.0;
        let g = f32::from(self.tint[1]) / 255.0;
        let b = f32::from(self.tint[2]) / 255.0;
        let sat = if self.saturation.is_finite() {
            self.saturation.clamp(0.0, MAX_MATERIAL_SATURATION)
        } else {
            1.0
        };
        let saturation_matrix = ((sat - 1.0).abs() > 1e-4).then(|| saturation_color_matrix(sat));
        AcrylicGraphDesc {
            saturation_matrix,
            luminosity_color: [r, g, b, self.luminosity_opacity.clamp(0.0, 1.0)],
            luminosity_blend_mode: D2D1_BLEND_MODE_COLOR_VALUE,
            tint_color: [r, g, b, self.tint_opacity.clamp(0.0, 1.0)],
            tint_blend_mode: D2D1_BLEND_MODE_LUMINOSITY_VALUE,
        }
    }

    /// Straight ARGB bytes `[a, r, g, b]` for `LuminosityColor.Color` (`InsertColor`).
    #[cfg(test)]
    pub(crate) fn luminosity_argb(self) -> [u8; 4] {
        let a = (self.luminosity_opacity.clamp(0.0, 1.0) * 255.0).round() as u8;
        [a, self.tint[0], self.tint[1], self.tint[2]]
    }

    /// Straight ARGB bytes `[a, r, g, b]` for `TintColor.Color` (`InsertColor`).
    #[cfg(test)]
    pub(crate) fn tint_argb(self) -> [u8; 4] {
        let a = (self.tint_opacity.clamp(0.0, 1.0) * 255.0).round() as u8;
        [a, self.tint[0], self.tint[1], self.tint[2]]
    }
}

/// Pure description of the Direct2D Acrylic effect graph (optional `Saturation` ColorMatrix,
/// `LuminosityColor`, `LuminosityBlend`, `TintColor`, `ColorBlend`) over
/// `CompositionEffectSourceParameter("Backdrop")`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct AcrylicGraphDesc {
    pub saturation_matrix: Option<[f32; 20]>,
    pub luminosity_color: [f32; 4],
    pub luminosity_blend_mode: u32,
    pub tint_color: [f32; 4],
    pub tint_blend_mode: u32,
}

/// Applies a row-major Direct2D 5x4 color matrix to an opaque sRGB `[r, g, b]` colour with
/// `ClampOutput = TRUE` (`0.0..=1.0`).
#[cfg(test)]
pub(crate) fn apply_color_matrix_srgb(m: &[f32; 20], [r, g, b]: [f32; 3]) -> [f32; 3] {
    let out_r = r * m[0] + g * m[4] + b * m[8] + m[12] + m[16];
    let out_g = r * m[1] + g * m[5] + b * m[9] + m[13] + m[17];
    let out_b = r * m[2] + g * m[6] + b * m[10] + m[14] + m[18];
    [out_r.clamp(0.0, 1.0), out_g.clamp(0.0, 1.0), out_b.clamp(0.0, 1.0)]
}

/// Evaluates Direct2D's optional `CLSID_D2D1ColorMatrix` saturation stage plus `CLSID_D2D1Blend`
/// Luminosity + Color two-pass Acrylic formula on an opaque sRGB backdrop `[r, g, b]` in
/// `0.0..=1.0`, returning the resulting sRGB `[r, g, b]`.
///
/// Uses Microsoft Learn's documented D2D1 Blend HSL luma weights (`kR = 0.30`, `kG = 0.59`,
/// `kB = 0.11`) and gamut-clamping `SetLum` (`clip_color`) formula.
#[cfg(test)]
pub(crate) fn simulate_acrylic_srgb(recipe: AcrylicRecipe, backdrop: [f32; 3]) -> [f32; 3] {
    let g = recipe.graph_desc();
    let backdrop = match g.saturation_matrix {
        Some(m) => apply_color_matrix_srgb(&m, backdrop),
        None => backdrop,
    };
    let lum_fg = [g.luminosity_color[0], g.luminosity_color[1], g.luminosity_color[2]];
    let lum_a = g.luminosity_color[3];
    // Pass 1 (Luminosity): take luminosity of lum_fg, hue/saturation of backdrop.
    let lum_blended = set_lum(backdrop, d2d_lum(lum_fg));
    let after_lum = [
        (lum_blended[0] * lum_a + backdrop[0] * (1.0 - lum_a)).clamp(0.0, 1.0),
        (lum_blended[1] * lum_a + backdrop[1] * (1.0 - lum_a)).clamp(0.0, 1.0),
        (lum_blended[2] * lum_a + backdrop[2] * (1.0 - lum_a)).clamp(0.0, 1.0),
    ];
    // Pass 2 (Color/Tint): take hue/saturation of tint_fg, luminosity of after_lum.
    let tint_fg = [g.tint_color[0], g.tint_color[1], g.tint_color[2]];
    let tint_a = g.tint_color[3];
    let color_blended = set_lum(tint_fg, d2d_lum(after_lum));
    [
        (color_blended[0] * tint_a + after_lum[0] * (1.0 - tint_a)).clamp(0.0, 1.0),
        (color_blended[1] * tint_a + after_lum[1] * (1.0 - tint_a)).clamp(0.0, 1.0),
        (color_blended[2] * tint_a + after_lum[2] * (1.0 - tint_a)).clamp(0.0, 1.0),
    ]
}

/// Evaluates straight SourceOver compositing of an ARGB flat tint over an opaque sRGB backdrop.
#[cfg(test)]
pub(crate) fn simulate_flat_tint_srgb(argb: [u8; 4], backdrop: [f32; 3]) -> [f32; 3] {
    let a = f32::from(argb[0]) / 255.0;
    let fg = [f32::from(argb[1]) / 255.0, f32::from(argb[2]) / 255.0, f32::from(argb[3]) / 255.0];
    [
        (fg[0] * a + backdrop[0] * (1.0 - a)).clamp(0.0, 1.0),
        (fg[1] * a + backdrop[1] * (1.0 - a)).clamp(0.0, 1.0),
        (fg[2] * a + backdrop[2] * (1.0 - a)).clamp(0.0, 1.0),
    ]
}

/// WCAG 2.1 contrast ratio between two sRGB colours `[r, g, b]` in `0.0..=1.0`.
#[cfg(test)]
pub(crate) fn wcag_contrast(a: [f32; 3], b: [f32; 3]) -> f32 {
    let la = wcag_luminance(a);
    let lb = wcag_luminance(b);
    let (hi, lo) = if la >= lb { (la, lb) } else { (lb, la) };
    (hi + 0.05) / (lo + 0.05)
}

#[cfg(test)]
fn wcag_luminance([r, g, b]: [f32; 3]) -> f32 {
    let lin = |c: f32| if c <= 0.03928 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) };
    0.2126 * lin(r) + 0.7152 * lin(g) + 0.0722 * lin(b)
}

#[cfg(test)]
fn d2d_lum([r, g, b]: [f32; 3]) -> f32 {
    0.30 * r + 0.59 * g + 0.11 * b
}

#[cfg(test)]
fn set_lum(c: [f32; 3], target_l: f32) -> [f32; 3] {
    let d = target_l - d2d_lum(c);
    let mut out = [c[0] + d, c[1] + d, c[2] + d];
    let l = d2d_lum(out);
    let n = out[0].min(out[1]).min(out[2]);
    let x = out[0].max(out[1]).max(out[2]);
    if n < 0.0 && (l - n) > 1e-6 {
        for ch in &mut out {
            *ch = l + ((*ch - l) * l) / (l - n);
        }
    }
    if x > 1.0 && (x - l) > 1e-6 {
        for ch in &mut out {
            *ch = l + ((*ch - l) * (1.0 - l)) / (x - l);
        }
    }
    [out[0].clamp(0.0, 1.0), out[1].clamp(0.0, 1.0), out[2].clamp(0.0, 1.0)]
}

#[cfg(all(target_os = "windows", feature = "windows-system-material"))]
#[allow(unsafe_code)]
pub(crate) mod winrt_graph {
    use std::cell::RefCell;

    use windows::Foundation::{IPropertyValue, PropertyValue};
    use windows::Graphics::Effects::{
        IGraphicsEffect, IGraphicsEffect_Impl, IGraphicsEffectSource, IGraphicsEffectSource_Impl,
    };
    use windows::UI::Composition::{
        CompositionBackdropBrush, CompositionEffectBrush, CompositionEffectFactoryLoadStatus,
        CompositionEffectSourceParameter, Compositor,
    };
    use windows::Win32::Foundation::E_INVALIDARG;
    use windows::Win32::Graphics::Direct2D::Common::{
        D2D1_BLEND_MODE_COLOR, D2D1_BLEND_MODE_LUMINOSITY,
        D2D1_COLORMATRIX_ALPHA_MODE_PREMULTIPLIED,
    };
    use windows::Win32::Graphics::Direct2D::{
        CLSID_D2D1Blend, CLSID_D2D1ColorMatrix, CLSID_D2D1Flood, D2D1_BLEND_PROP_MODE,
        D2D1_COLORMATRIX_PROP_ALPHA_MODE, D2D1_COLORMATRIX_PROP_CLAMP_OUTPUT,
        D2D1_COLORMATRIX_PROP_COLOR_MATRIX, D2D1_FLOOD_PROP_COLOR,
    };
    use windows::Win32::System::WinRT::Graphics::Direct2D::{
        GRAPHICS_EFFECT_PROPERTY_MAPPING,
        GRAPHICS_EFFECT_PROPERTY_MAPPING_COLORMATRIX_ALPHA_MODE,
        GRAPHICS_EFFECT_PROPERTY_MAPPING_COLOR_TO_VECTOR4,
        GRAPHICS_EFFECT_PROPERTY_MAPPING_DIRECT, IGraphicsEffectD2D1Interop,
        IGraphicsEffectD2D1Interop_Impl,
    };
    use windows::core::{Error, GUID, HSTRING, Interface, PCWSTR, Result as WinResult, implement};

    use super::AcrylicRecipe;

    const BACKDROP_PARAM: &str = "Backdrop";
    const LUMINOSITY_NODE: &str = "LuminosityColor";
    const TINT_NODE: &str = "TintColor";

    struct EffectProp {
        name: &'static str,
        d2d_index: u32,
        mapping: GRAPHICS_EFFECT_PROPERTY_MAPPING,
        value: IPropertyValue,
    }

    /// Minimal COM implementation of documented `IGraphicsEffect`, `IGraphicsEffectSource`, and
    /// `IGraphicsEffectD2D1Interop` (`windows.graphics.effects.interop.h`) for describing D2D1
    /// effect graphs directly to `Windows.UI.Composition::Compositor::CreateEffectFactory` with
    /// zero Win2D dependency.
    #[implement(IGraphicsEffect, IGraphicsEffectSource, IGraphicsEffectD2D1Interop)]
    struct D2dEffect {
        name: RefCell<HSTRING>,
        effect_id: GUID,
        properties: Vec<EffectProp>,
        sources: Vec<IGraphicsEffectSource>,
    }

    impl D2dEffect {
        fn into_effect(self) -> IGraphicsEffect {
            self.into()
        }

        fn into_source(self) -> IGraphicsEffectSource {
            self.into()
        }
    }

    impl IGraphicsEffectSource_Impl for D2dEffect_Impl {}

    impl IGraphicsEffect_Impl for D2dEffect_Impl {
        fn Name(&self) -> WinResult<HSTRING> {
            Ok(self.name.borrow().clone())
        }

        fn SetName(&self, name: &HSTRING) -> WinResult<()> {
            *self.name.borrow_mut() = name.clone();
            Ok(())
        }
    }

    impl IGraphicsEffectD2D1Interop_Impl for D2dEffect_Impl {
        fn GetEffectId(&self) -> WinResult<GUID> {
            Ok(self.effect_id)
        }

        fn GetNamedPropertyMapping(
            &self,
            name: &PCWSTR,
            index: *mut u32,
            mapping: *mut GRAPHICS_EFFECT_PROPERTY_MAPPING,
        ) -> WinResult<()> {
            if name.is_null() || index.is_null() || mapping.is_null() {
                return Err(Error::from(E_INVALIDARG));
            }
            // SAFETY: `name` is a non-null NUL-terminated UTF-16 string supplied by DWM/WinRT.
            let key = unsafe { name.to_string() }.map_err(|_| Error::from(E_INVALIDARG))?;
            for prop in &self.properties {
                if prop.name == key {
                    // SAFETY: `index` and `mapping` are non-null out pointers from COM caller.
                    unsafe {
                        *index = prop.d2d_index;
                        *mapping = prop.mapping;
                    }
                    return Ok(());
                }
            }
            Err(Error::from(E_INVALIDARG))
        }

        fn GetPropertyCount(&self) -> WinResult<u32> {
            Ok(self.properties.len() as u32)
        }

        fn GetProperty(&self, index: u32) -> WinResult<IPropertyValue> {
            self
                .properties
                .iter()
                .find(|p| p.d2d_index == index)
                .map(|p| p.value.clone())
                .ok_or_else(|| Error::from(E_INVALIDARG))
        }

        fn GetSource(&self, index: u32) -> WinResult<IGraphicsEffectSource> {
            self
                .sources
                .get(index as usize)
                .cloned()
                .ok_or_else(|| Error::from(E_INVALIDARG))
        }

        fn GetSourceCount(&self) -> WinResult<u32> {
            Ok(self.sources.len() as u32)
        }
    }

    fn prop_f32_array(values: &[f32]) -> Result<IPropertyValue, String> {
        PropertyValue::CreateSingleArray(values)
            .map_err(|e| format!("PropertyValue::CreateSingleArray: {e}"))?
            .cast()
            .map_err(|e| format!("cast IPropertyValue(SingleArray): {e}"))
    }

    fn prop_u32(v: u32) -> Result<IPropertyValue, String> {
        PropertyValue::CreateUInt32(v)
            .map_err(|e| format!("PropertyValue::CreateUInt32: {e}"))?
            .cast()
            .map_err(|e| format!("cast IPropertyValue(UInt32): {e}"))
    }

    fn prop_bool(v: bool) -> Result<IPropertyValue, String> {
        PropertyValue::CreateBoolean(v)
            .map_err(|e| format!("PropertyValue::CreateBoolean: {e}"))?
            .cast()
            .map_err(|e| format!("cast IPropertyValue(Boolean): {e}"))
    }

    /// Compiles the Windows 11 Acrylic Luminosity + Color (+ optional `CLSID_D2D1ColorMatrix`
    /// saturation) `CompositionEffectBrush` for `recipe` bound to `backdrop`, checking
    /// `ExtendedError()` and `LoadStatus()`.
    pub(crate) fn create_acrylic_brush(
        compositor: &Compositor,
        backdrop: &CompositionBackdropBrush,
        recipe: AcrylicRecipe,
    ) -> Result<CompositionEffectBrush, String> {
        let graph = recipe.graph_desc();
        let backdrop_source: IGraphicsEffectSource =
            CompositionEffectSourceParameter::Create(&HSTRING::from(BACKDROP_PARAM))
                .map_err(|e| format!("CompositionEffectSourceParameter::Create: {e}"))?
                .cast()
                .map_err(|e| format!("cast Backdrop IGraphicsEffectSource: {e}"))?;

        let base_source = if let Some(matrix) = graph.saturation_matrix {
            D2dEffect {
                name: RefCell::new(HSTRING::from("Saturation")),
                effect_id: CLSID_D2D1ColorMatrix,
                properties: vec![
                    EffectProp {
                        name: "ColorMatrix",
                        d2d_index: D2D1_COLORMATRIX_PROP_COLOR_MATRIX.0 as u32,
                        mapping: GRAPHICS_EFFECT_PROPERTY_MAPPING_DIRECT,
                        value: prop_f32_array(&matrix)?,
                    },
                    EffectProp {
                        name: "AlphaMode",
                        d2d_index: D2D1_COLORMATRIX_PROP_ALPHA_MODE.0 as u32,
                        mapping: GRAPHICS_EFFECT_PROPERTY_MAPPING_COLORMATRIX_ALPHA_MODE,
                        value: prop_u32(D2D1_COLORMATRIX_ALPHA_MODE_PREMULTIPLIED.0 as u32)?,
                    },
                    EffectProp {
                        name: "ClampOutput",
                        d2d_index: D2D1_COLORMATRIX_PROP_CLAMP_OUTPUT.0 as u32,
                        mapping: GRAPHICS_EFFECT_PROPERTY_MAPPING_DIRECT,
                        value: prop_bool(true)?,
                    },
                ],
                sources: vec![backdrop_source],
            }
            .into_source()
        } else {
            backdrop_source
        };

        let lum_flood = D2dEffect {
            name: RefCell::new(HSTRING::from(LUMINOSITY_NODE)),
            effect_id: CLSID_D2D1Flood,
            properties: vec![EffectProp {
                name: "Color",
                d2d_index: D2D1_FLOOD_PROP_COLOR.0 as u32,
                mapping: GRAPHICS_EFFECT_PROPERTY_MAPPING_COLOR_TO_VECTOR4,
                value: prop_f32_array(&graph.luminosity_color)?,
            }],
            sources: Vec::new(),
        }
        .into_source();

        // WinUI `AcrylicBrush.cpp`: with source[0] = backdrop and source[1] = flood,
        // D2D1_BLEND_MODE_COLOR (22) performs the Luminosity blend step.
        let lum_blend = D2dEffect {
            name: RefCell::new(HSTRING::from("LuminosityBlend")),
            effect_id: CLSID_D2D1Blend,
            properties: vec![EffectProp {
                name: "Mode",
                d2d_index: D2D1_BLEND_PROP_MODE.0 as u32,
                mapping: GRAPHICS_EFFECT_PROPERTY_MAPPING_DIRECT,
                value: prop_u32(D2D1_BLEND_MODE_COLOR.0 as u32)?,
            }],
            sources: vec![base_source, lum_flood],
        }
        .into_source();

        let tint_flood = D2dEffect {
            name: RefCell::new(HSTRING::from(TINT_NODE)),
            effect_id: CLSID_D2D1Flood,
            properties: vec![EffectProp {
                name: "Color",
                d2d_index: D2D1_FLOOD_PROP_COLOR.0 as u32,
                mapping: GRAPHICS_EFFECT_PROPERTY_MAPPING_COLOR_TO_VECTOR4,
                value: prop_f32_array(&graph.tint_color)?,
            }],
            sources: Vec::new(),
        }
        .into_source();

        // WinUI `AcrylicBrush.cpp`: with source[0] = lum_blend and source[1] = tint_flood,
        // D2D1_BLEND_MODE_LUMINOSITY (23) performs the Color/Tint blend step.
        let color_blend = D2dEffect {
            name: RefCell::new(HSTRING::from("ColorBlend")),
            effect_id: CLSID_D2D1Blend,
            properties: vec![EffectProp {
                name: "Mode",
                d2d_index: D2D1_BLEND_PROP_MODE.0 as u32,
                mapping: GRAPHICS_EFFECT_PROPERTY_MAPPING_DIRECT,
                value: prop_u32(D2D1_BLEND_MODE_LUMINOSITY.0 as u32)?,
            }],
            sources: vec![lum_blend, tint_flood],
        }
        .into_effect();

        let factory = compositor
            .CreateEffectFactory(&color_blend)
            .map_err(|e| format!("Compositor::CreateEffectFactory: {e}"))?;
        let brush_res = (|| {
            let ext = factory
                .ExtendedError()
                .map_err(|e| format!("CompositionEffectFactory::ExtendedError: {e}"))?;
            if ext.is_err() {
                return Err(format!(
                    "CompositionEffectFactory ExtendedError: 0x{:08x}",
                    ext.0 as u32
                ));
            }
            let status = factory
                .LoadStatus()
                .map_err(|e| format!("CompositionEffectFactory::LoadStatus: {e}"))?;
            // DWM may compile the effect asynchronously (`Pending` with `ExtendedError == S_OK`);
            // `CreateBrush()` is valid for both `Success` and `Pending`, whereas `EffectTooComplex`
            // or `Other` (or a failing `ExtendedError`) signals real compilation failure.
            if !matches!(
                status,
                CompositionEffectFactoryLoadStatus::Success
                    | CompositionEffectFactoryLoadStatus::Pending
            ) {
                return Err(format!("CompositionEffectFactory LoadStatus: {}", status.0));
            }
            factory
                .CreateBrush()
                .map_err(|e| format!("CompositionEffectFactory::CreateBrush: {e}"))
        })();
        // Always close the factory handle on both success and error paths.
        factory.Close().ok();
        let brush = brush_res?;
        if let Err(e) = brush.SetSourceParameter(&HSTRING::from(BACKDROP_PARAM), backdrop) {
            brush.Close().ok();
            return Err(format!("SetSourceParameter(Backdrop): {e}"));
        }
        Ok(brush)
    }
}
