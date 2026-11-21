//! A compact starting palette for apps that don't carry a full semantic token set.

use rune::{Color, oklch};

use crate::{Kit, Palette, StateColors};

/// Three opaque theme colours from which [`Kit::from_palette`] derives its semantic tokens.
/// Typography, radii, motion and individual colours remain editable on the resulting kit.
#[derive(Clone, Copy, Debug)]
pub struct PaletteSeed {
    pub bg: Color,
    pub fg: Color,
    pub accent: Color,
}

fn contrast(a: Color, b: Color) -> f32 {
    let (a, b) = (a.luminance(), b.luminance());
    (a.max(b) + 0.05) / (a.min(b) + 0.05)
}

/// Where [`Kit::from_palette_focus`] takes the focus-ring colour from.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum PaletteFocus {
    /// A neutral blue that reads as focus on any palette.
    #[default]
    Blue,
    /// The seed's accent.
    Accent,
    /// The seed's foreground (text) colour.
    Ink,
    /// This colour. `Custom(cx.focus_color())` follows the user's system accent, as AppKit's
    /// rings do.
    Custom(Color),
}

/// Perceptual dimming along OKLab lightness at the palette's tint (`[a, b]`), bounded by a
/// contrast floor when the supplied foreground supports it.
fn secondary(bg: Color, fg: Color, tint: [f32; 2], amount: f32, minimum: f32) -> Color {
    let (lb, lf) = (bg.to_oklab()[0], fg.to_oklab()[0]);
    let mix = |t: f32| Color::from_oklab([lb + (lf - lb) * t, tint[0], tint[1], 1.0]);
    let candidate = mix(amount);
    if contrast(bg, candidate) >= minimum {
        return candidate;
    }
    if contrast(bg, fg) < minimum {
        return fg;
    }
    let (mut low, mut high) = (amount, 1.0);
    for _ in 0..24 {
        let mid = (low + high) * 0.5;
        if contrast(bg, mix(mid)) >= minimum {
            high = mid;
        } else {
            low = mid;
        }
    }
    mix(high)
}

/// `t` of the way from `a` to `b` in OKLab.
fn mix_oklab(a: Color, b: Color, t: f32) -> Color {
    let (a, b) = (a.to_oklab(), b.to_oklab());
    Color::from_oklab(std::array::from_fn(|i| a[i] + (b[i] - a[i]) * t))
}

/// `t` of the way from `base` toward `hue` in OKLab, at `base`'s alpha: a quiet wash that keeps
/// `base`'s lightness close in both themes.
pub(crate) fn tint(base: Color, hue: Color, t: f32) -> Color {
    let [a, b] = [base.to_oklab(), hue.to_oklab()];
    Color::from_oklab([0, 1, 2, 3].map(|i| if i == 3 { a[3] } else { a[i] + (b[i] - a[i]) * t }))
}

/// OKLab lightness steps away from `bg` (lighter on dark themes, darker on light ones) per
/// elevation: `layer_01`–`layer_04`.
const DARK_LAYERS: [f32; 4] = [0.04, 0.07, 0.10, 0.13];
const LIGHT_LAYERS: [f32; 4] = [0.02, 0.045, 0.065, 0.10];
/// Dark palettes derive surfaces from at least this OKLab lightness (about #0a0a0a): lightness
/// steps from pure black stay invisible (+0.04 L from #000 is #010101).
const DARK_FLOOR: f32 = 0.145;

impl Kit {
    /// Derives surfaces, text, borders, states and elevation from three theme colours.
    /// Appearance is dark when `fg` is lighter than `bg`.
    ///
    /// - Layers step OKLab lightness away from `bg` (+0.04, then +0.03 per elevation when
    ///   dark; smaller steps the other way when light). Dark surfaces step from at least
    ///   #0a0a0a's lightness, so pure black still gets visible layers and status tints.
    /// - Derived neutrals (layers, opaque borders, secondary text) keep the hue and chroma of
    ///   the more colourful of `bg` and `fg`, so warm palettes stay warm.
    /// - `border_muted`/`border_base`/`border_strong`, hover and press are `fg` at low alpha,
    ///   as a browser blends it: authored for sRGB blending, and solved over `bg`
    ///   (`blend_over`) when the kit is built in a window that blends in linear light.
    ///   Opaque `std_border`/`border_*` tokens are lightness steps.
    /// - Secondary text mixes towards `fg` in OKLab, with contrast floors of 4.5:1 (muted) and
    ///   3:1 (faint) when `fg` supports them.
    /// - Focus rings are a neutral blue, not `accent`; [`Kit::from_palette_focus`] picks
    ///   accent, ink or a custom colour instead.
    /// - Elevation rings are `c.border_base`, and re-resolve when painted (`Kit::ring`), so
    ///   overriding it restyles them.
    ///
    /// ```
    /// use rune::{Color, color::hex};
    /// use rune_kit::{Kit, PaletteSeed};
    /// let kit = Kit::from_palette(PaletteSeed {
    ///     bg: Color::BLACK,
    ///     fg: Color::WHITE,
    ///     accent: hex("#e53935").unwrap(),
    /// });
    /// ```
    pub fn from_palette(seed: PaletteSeed) -> Kit {
        Kit::from_palette_focus(seed, PaletteFocus::Blue)
    }

    /// [`Kit::from_palette`] with the focus ring from `focus`.
    ///
    /// ```
    /// use rune::Color;
    /// use rune_kit::{Kit, PaletteFocus, PaletteSeed};
    /// let seed = PaletteSeed { bg: Color::BLACK, fg: Color::WHITE, accent: Color::WHITE };
    /// let kit = Kit::from_palette_focus(seed, PaletteFocus::Ink);
    /// assert_eq!(kit.c.border_focus, Color::WHITE);
    /// ```
    pub fn from_palette_focus(seed: PaletteSeed, focus: PaletteFocus) -> Kit {
        let PaletteSeed { bg, fg, accent } = seed;
        let dark = fg.luminance() > bg.luminance();
        let mut kit = Kit::neutral(dark);
        let (lab_bg, lab_fg) = (bg.to_oklab(), fg.to_oklab());
        let chroma = |o: [f32; 4]| o[1].hypot(o[2]);
        let tinted = if chroma(lab_bg) >= chroma(lab_fg) { lab_bg } else { lab_fg };
        let tint = [tinted[1], tinted[2]];
        let tone = |l: f32| Color::from_oklab([l.clamp(0.0, 1.0), tint[0], tint[1], 1.0]);
        let base = if dark { lab_bg[0].max(DARK_FLOOR) } else { lab_bg[0] };
        let toward_fg = if dark { 1.0 } else { -1.0 };
        let lift = |dl: f32| tone(base + toward_fg * dl);
        let [l1, l2, l3, l4] = if dark { DARK_LAYERS } else { LIGHT_LAYERS }.map(lift);
        let surface = lift(0.0);
        let ink = |a: f32| rune::theme::for_blend(fg.alpha(a), bg);
        let muted = secondary(bg, fg, tint, if dark { 0.72 } else { 0.57 }, 4.5);
        let faint = secondary(bg, fg, tint, if dark { 0.54 } else { 0.36 }, 3.0);
        let border = ink(if dark { 0.11 } else { 0.10 });
        let solid_border = lift(if dark { 0.20 } else { 0.18 });
        let hover = ink(if dark { 0.06 } else { 0.05 });
        let pressed = ink(if dark { 0.10 } else { 0.08 });
        let focus = match focus {
            PaletteFocus::Blue => oklch(if dark { 0.70 } else { 0.58 }, 0.15, 264.0),
            PaletteFocus::Accent => accent,
            PaletteFocus::Ink => fg,
            PaletteFocus::Custom(c) => c,
        };
        let state = |role: StateColors| StateColors {
            bg: mix_oklab(surface, role.fg, 0.12),
            border: mix_oklab(surface, role.fg, 0.35),
            ..role
        };
        kit.blend_over = Some(bg);
        kit.c = Palette {
            page: bg,
            bg_base: bg,
            bg_deep: bg,
            layer_01: l1,
            layer_02: l2,
            layer_03: l3,
            layer_04: l4,
            bg_inverse: fg,
            bg_contrast: fg,
            bg_button_neutral: l1,
            bg_icon_button_contrast: fg,
            bg_accent: accent,
            submit_highlight: Color::WHITE.alpha(0.20),
            submit_hover: bg.alpha(0.12),
            submit_fg: bg,
            submit_icon: bg,
            text_base: fg,
            text_muted: muted,
            text_faint: faint,
            text_inverse: bg,
            text_contrast: bg,
            text_accent: accent,
            icon_base: fg,
            icon_muted: muted,
            icon_faint: faint,
            icon_inverse: bg,
            icon_contrast: bg,
            icon_accent: accent,
            border_muted: ink(0.08),
            border_base: border,
            border_strong: ink(0.20),
            border_focus: focus,
            hover,
            pressed,
            contrast_hover: bg.alpha(0.10),
            contrast_pressed: bg.alpha(0.20),
            scrim: Color::BLACK.alpha(0.30),
            depth_top: Color::WHITE,
            depth_bottom: Color::WHITE.alpha(0.0),
            white: Color::WHITE,
            black: Color::BLACK,
            knob: fg,
            knob_on: fg,
            success: state(kit.c.success),
            warning: state(kit.c.warning),
            danger: state(kit.c.danger),
            info: state(kit.c.info),
            surface_base: l2,
            surface_weak: l1,
            surface_hover: hover,
            surface_raised: l1,
            surface_raised_base: l1,
            surface_raised_hover: l3,
            surface_float: fg,
            surface_disabled: l2,
            border_weak: lift(0.12),
            border_hover: lift(0.26),
            border_active: lift(0.28),
            border_disabled: lift(0.10),
            text_strong: fg,
            text_weak: muted,
            text_invert_strong: bg,
            icon_weaker: faint,
            std_border: solid_border,
            std_icon: muted,
            std_icon_invert: bg,
            std_icon_strong: fg,
            std_text: fg,
        };
        // Keep neutral elevation geometry with black shadows. Rings carry the derived border for
        // callers reading `elevation` directly and follow `c.border_base` when painted.
        kit.ring = Some(|c: &Palette| c.border_base);
        let e = &mut kit.elevation;
        for shadows in [
            &mut e.raised,
            &mut e.floating,
            &mut e.overlay,
            &mut e.button_neutral,
            &mut e.button_contrast,
            &mut e.elements,
            &mut e.switch_off,
            &mut e.switch_on,
            &mut e.md,
            &mut e.knob,
        ] {
            for shadow in shadows.iter_mut() {
                shadow.color =
                    if shadow.blur == 0.0 { border } else { Color::BLACK.alpha(if dark { 0.30 } else { 0.12 }) };
            }
        }
        kit
    }
}

#[cfg(test)]
#[path = "palette_tests.rs"]
mod tests;
