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

/// Perceptual dimming, bounded by a contrast floor when the supplied foreground supports it.
fn secondary(bg: Color, fg: Color, amount: f32, minimum: f32) -> Color {
    let a = bg.to_oklab();
    let b = fg.to_oklab();
    let mix = |t: f32| Color::from_oklab(std::array::from_fn(|i| a[i] + (b[i] - a[i]) * t));
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

/// OKLab lightness steps away from `bg` (lighter on dark themes, darker on light ones) per
/// elevation: `layer_01`–`layer_04`.
const DARK_LAYERS: [f32; 4] = [0.04, 0.07, 0.10, 0.13];
const LIGHT_LAYERS: [f32; 4] = [0.02, 0.045, 0.065, 0.10];

impl Kit {
    /// Derives surfaces, text, borders, states and elevation from three theme colours.
    /// Appearance is dark when `fg` is lighter than `bg`.
    ///
    /// - Layers step OKLab lightness away from `bg` (+0.04, then +0.03 per elevation when
    ///   dark; smaller steps the other way when light), keeping `bg`'s hue.
    /// - `border_muted`/`border_base`/`border_strong`, hover and press are `fg` at low alpha,
    ///   solved so Rune's linear-light blending over `bg` matches sRGB blending
    ///   (`blend_over` is `bg`). Opaque `std_border`/`border_*` tokens are lightness steps.
    /// - Secondary text mixes towards `fg` in OKLab, with contrast floors of 4.5:1 (muted) and
    ///   3:1 (faint) when `fg` supports them.
    /// - Focus rings are a neutral blue, not `accent`; set `c.border_focus` to change it.
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
        let PaletteSeed { bg, fg, accent } = seed;
        let dark = fg.luminance() > bg.luminance();
        let mut kit = Kit::neutral(dark);
        let base = bg.to_oklab();
        let toward_fg = if dark { 1.0 } else { -1.0 };
        let lift = |dl: f32| Color::from_oklab([(base[0] + toward_fg * dl).clamp(0.0, 1.0), base[1], base[2], 1.0]);
        let [l1, l2, l3, l4] = if dark { DARK_LAYERS } else { LIGHT_LAYERS }.map(lift);
        let ink = |a: f32| rune::theme::srgb_equivalent(fg.alpha(a), bg);
        let muted = secondary(bg, fg, if dark { 0.72 } else { 0.60 }, 4.5);
        let faint = secondary(bg, fg, if dark { 0.54 } else { 0.42 }, 3.0);
        let border = ink(if dark { 0.11 } else { 0.10 });
        let solid_border = lift(if dark { 0.20 } else { 0.18 });
        let hover = ink(if dark { 0.06 } else { 0.05 });
        let pressed = ink(if dark { 0.10 } else { 0.08 });
        let focus = oklch(if dark { 0.70 } else { 0.58 }, 0.15, 264.0);
        let state = |role: StateColors| StateColors {
            bg: mix_oklab(bg, role.fg, 0.12),
            border: mix_oklab(bg, role.fg, 0.35),
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
mod tests {
    use super::*;
    use rune::color::hex;

    #[test]
    fn secondary_text_is_readable_on_light_dark_and_middle_backgrounds() {
        for (bg, fg) in [("#ffffff", "#000000"), ("#101010", "#ffffff"), ("#767676", "#000000"), ("#767676", "#ffffff")]
        {
            let (bg, fg) = (hex(bg).unwrap(), hex(fg).unwrap());
            let kit = Kit::from_palette(PaletteSeed { bg, fg, accent: fg });
            assert!(contrast(bg, kit.c.text_muted) >= 4.5, "muted on {}", bg.hex());
            assert!(contrast(bg, kit.c.text_faint) >= 3.0, "faint on {}", bg.hex());
            assert!(contrast(bg, kit.c.text_faint) <= contrast(bg, kit.c.text_muted));
            if kit.dark && bg.luminance() < 0.02 {
                assert!(kit.c.text_muted.luminance() < fg.luminance() * 0.5, "dark muted must look dimmer");
            }
        }
    }

    fn srgb8(c: Color) -> [u8; 3] {
        let h = c.hex();
        std::array::from_fn(|i| u8::from_str_radix(&h[1 + 2 * i..3 + 2 * i], 16).unwrap())
    }

    /// Deadbolt's palette: near-black, off-white and a red accent.
    fn deadbolt(dark: bool) -> (PaletteSeed, Kit) {
        let (bg, fg) = if dark { ("#0a0a0a", "#ededed") } else { ("#ffffff", "#171717") };
        let seed = PaletteSeed { bg: hex(bg).unwrap(), fg: hex(fg).unwrap(), accent: hex("#e5484d").unwrap() };
        (seed, Kit::from_palette(seed))
    }

    #[test]
    fn near_black_layers_step_gently_in_oklab_lightness() {
        let (_, k) = deadbolt(true);
        let layers = [k.c.layer_01, k.c.layer_02, k.c.layer_03, k.c.layer_04].map(srgb8);
        for (i, rgb) in layers.iter().enumerate() {
            assert!(rgb.iter().all(|&v| (0x12..=0x2a).contains(&v)), "layer_0{} is {rgb:02x?}", i + 1);
        }
        assert!(layers.windows(2).all(|w| w[0][0] < w[1][0]), "layers must rise: {layers:02x?}");
        // Light is the mirror image: a little darker per step, never grey.
        let (_, k) = deadbolt(false);
        let layers = [k.c.layer_01, k.c.layer_02, k.c.layer_03, k.c.layer_04].map(srgb8);
        assert!(layers.windows(2).all(|w| w[0][0] > w[1][0]), "layers must fall: {layers:02x?}");
        assert!(layers.iter().all(|rgb| rgb[0] >= 0xd8 && rgb[0] < 0xff), "{layers:02x?}");
    }

    #[test]
    fn borders_are_low_alpha_foreground_and_focus_is_not_the_accent() {
        for dark in [true, false] {
            let (seed, k) = deadbolt(dark);
            for border in [k.c.border_muted, k.c.border_base, k.c.border_strong] {
                // Alpha as solved for linear-light blending (sRGB 8–20%).
                assert!(border.a > 0.0 && border.a < 0.4, "border alpha {}", border.a);
                assert_eq!(srgb8(border.alpha(1.0)), srgb8(seed.fg));
            }
            assert!(k.c.border_muted.a < k.c.border_base.a && k.c.border_base.a < k.c.border_strong.a);
            let [r, _, b] = srgb8(k.c.border_focus);
            assert!(b > r, "focus should read blue, not {}", k.c.border_focus.hex());
            assert_ne!(srgb8(k.c.border_active), srgb8(seed.accent));
            assert_eq!(k.blend_over.map(srgb8), Some(srgb8(seed.bg)));
        }
    }

    #[test]
    fn dark_secondary_text_is_dimmed_foreground_not_mid_grey() {
        let (seed, k) = deadbolt(true);
        let (muted, faint) = (srgb8(k.c.text_muted), srgb8(k.c.text_faint));
        assert!(muted[0] >= 0x98 && muted[0] < 0xd8, "muted {muted:02x?}");
        assert!(faint[0] < muted[0] && faint[0] >= 0x68, "faint {faint:02x?}");
        assert!(contrast(seed.bg, k.c.text_muted) >= 4.5 && contrast(seed.bg, k.c.text_faint) >= 3.0);
    }

    #[test]
    fn elevation_rings_follow_border_overrides() {
        let (_, mut k) = deadbolt(true);
        let ring = |k: &Kit| k.shadow_layers(&k.elevation.floating).find(|s| s.blur == 0.0).unwrap().color;
        assert_eq!(srgb8(ring(&k)), srgb8(k.c.border_base));
        // Read directly, rings already carry the derived border (not the neutral kit's).
        let stored = k.elevation.floating.iter().find(|s| s.blur == 0.0).unwrap().color;
        assert_eq!(stored.hex(), k.c.border_base.hex());
        k.c.border_base = hex("#336699").unwrap();
        assert_eq!(ring(&k).hex(), "#336699ff");
        let drops: Vec<_> = k.shadow_layers(&k.elevation.floating).filter(|s| s.blur > 0.0).collect();
        assert!(!drops.is_empty() && drops.iter().all(|s| srgb8(s.color) == [0, 0, 0]));
    }
}
