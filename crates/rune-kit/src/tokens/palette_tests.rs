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

/// A dark palette: near-black, off-white and a red accent.
fn red_accent(dark: bool) -> (PaletteSeed, Kit) {
    let (bg, fg) = if dark { ("#0a0a0a", "#ededed") } else { ("#ffffff", "#171717") };
    let seed = PaletteSeed { bg: hex(bg).unwrap(), fg: hex(fg).unwrap(), accent: hex("#e5484d").unwrap() };
    (seed, Kit::from_palette(seed))
}

#[test]
fn near_black_layers_step_gently_in_oklab_lightness() {
    let (_, k) = red_accent(true);
    let layers = [k.c.layer_01, k.c.layer_02, k.c.layer_03, k.c.layer_04].map(srgb8);
    for (i, rgb) in layers.iter().enumerate() {
        assert!(rgb.iter().all(|&v| (0x12..=0x2a).contains(&v)), "layer_0{} is {rgb:02x?}", i + 1);
    }
    assert!(layers.windows(2).all(|w| w[0][0] < w[1][0]), "layers must rise: {layers:02x?}");
    // Light is the mirror image: a little darker per step, never grey.
    let (_, k) = red_accent(false);
    let layers = [k.c.layer_01, k.c.layer_02, k.c.layer_03, k.c.layer_04].map(srgb8);
    assert!(layers.windows(2).all(|w| w[0][0] > w[1][0]), "layers must fall: {layers:02x?}");
    assert!(layers.iter().all(|rgb| rgb[0] >= 0xd8 && rgb[0] < 0xff), "{layers:02x?}");
}

#[test]
fn borders_are_low_alpha_foreground_and_focus_is_not_the_accent() {
    for dark in [true, false] {
        let (seed, k) = red_accent(dark);
        for border in [k.c.border_muted, k.c.border_base, k.c.border_strong] {
            // The authored alpha (8–20%): built outside a linear-light window.
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
    let (seed, k) = red_accent(true);
    let (muted, faint) = (srgb8(k.c.text_muted), srgb8(k.c.text_faint));
    assert!(muted[0] >= 0x98 && muted[0] < 0xd8, "muted {muted:02x?}");
    assert!(faint[0] < muted[0] && faint[0] >= 0x68, "faint {faint:02x?}");
    assert!(contrast(seed.bg, k.c.text_muted) >= 4.5 && contrast(seed.bg, k.c.text_faint) >= 3.0);
}

#[test]
fn elevation_rings_follow_border_overrides() {
    let (_, mut k) = red_accent(true);
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

/// `c` painted over `bg` as Rune blends (linear light), in sRGB bytes.
fn over(c: Color, bg: Color) -> [u8; 3] {
    let m = |x: f32, y: f32| x * c.a + y * (1.0 - c.a);
    srgb8(Color { r: m(c.r, bg.r), g: m(c.g, bg.g), b: m(c.b, bg.b), a: 1.0 })
}

fn oklab_distance(a: Color, b: Color) -> f32 {
    let (a, b) = (a.to_oklab(), b.to_oklab());
    (0..3).map(|i| (a[i] - b[i]).powi(2)).sum::<f32>().sqrt()
}

#[test]
fn pure_black_still_gets_visible_layers_borders_and_status_tints() {
    for bg in ["#000000", "#0a0a0a"] {
        let bg = hex(bg).unwrap();
        let k = Kit::from_palette(PaletteSeed { bg, fg: Color::WHITE, accent: hex("#ff2a2a").unwrap() });
        let layers = [k.c.layer_01, k.c.layer_02, k.c.layer_03, k.c.layer_04].map(srgb8);
        assert!(layers[0].iter().all(|&v| v >= 0x12), "layer_01 {:02x?} on {}", layers[0], bg.hex());
        assert!(layers.windows(2).all(|w| w[1][0] >= w[0][0] + 4), "{layers:02x?}");
        assert!(layers[3][0] <= 0x2c, "{layers:02x?}");
        for opaque in [k.c.border_weak, k.c.std_border, k.c.border_disabled] {
            assert!(srgb8(opaque)[0] >= 0x1c, "opaque border {} on {}", opaque.hex(), bg.hex());
        }
        for border in [k.c.border_muted, k.c.border_base, k.c.border_strong] {
            assert!(over(border, bg)[0] >= 0x14, "border {:02x?} on {}", over(border, bg), bg.hex());
        }
        for state in [k.c.danger, k.c.warning, k.c.success, k.c.info] {
            let tint = srgb8(state.bg);
            assert!(tint.iter().max().unwrap() >= &0x1c, "status bg {tint:02x?} on {}", bg.hex());
        }
        let [r, g, _] = srgb8(k.c.danger.bg);
        assert!(r > g, "danger bg should read red");
    }
    let k = Kit::from_palette(PaletteSeed { bg: Color::WHITE, fg: Color::BLACK, accent: hex("#d81414").unwrap() });
    let layers = [k.c.layer_01, k.c.layer_02, k.c.layer_03, k.c.layer_04].map(srgb8);
    assert!(layers.windows(2).all(|w| w[0][0] > w[1][0]) && layers[0][0] < 0xff, "{layers:02x?}");
}

#[test]
fn warm_palettes_keep_their_hue_in_derived_neutrals() {
    // A warm paper palette: the site's own inks are #4a463f and #857f74.
    let bg = hex("#e8e4dc").unwrap();
    let k = Kit::from_palette(PaletteSeed { bg, fg: Color::BLACK, accent: hex("#d81414").unwrap() });
    let lch = |c: Color| {
        let o = c.to_oklab();
        (o[1].hypot(o[2]), o[2].atan2(o[1]).to_degrees())
    };
    for (token, site) in [(k.c.text_muted, "#4a463f"), (k.c.text_faint, "#857f74"), (k.c.layer_03, "#dcd7cc")] {
        let (c, h) = lch(token);
        assert!(c > 0.009 && (70.0..100.0).contains(&h), "{} is C {c:.4} h {h:.0}", token.hex());
        assert!(oklab_distance(token, hex(site).unwrap()) < 0.03, "{} vs {site}", token.hex());
    }
    assert!(contrast(bg, k.c.text_muted) >= 4.5 && contrast(bg, k.c.text_faint) >= 3.0);
    // A warm foreground on black tints the dark neutrals the same way.
    let k = Kit::from_palette(PaletteSeed { bg: Color::BLACK, fg: hex("#f3e7cf").unwrap(), accent: Color::WHITE });
    let (c, h) = lch(k.c.text_muted);
    assert!(c > 0.02 && (60.0..100.0).contains(&h), "C {c:.4} h {h:.0}");
    assert!(contrast(Color::BLACK, k.c.text_muted) >= 4.5);
}

#[test]
fn focus_can_follow_accent_ink_or_a_custom_colour() {
    let seed = PaletteSeed { bg: Color::BLACK, fg: Color::WHITE, accent: hex("#ff2a2a").unwrap() };
    let blue = hex("#3b82f6").unwrap();
    assert_eq!(Kit::from_palette_focus(seed, PaletteFocus::Ink).c.border_focus, Color::WHITE);
    assert_eq!(Kit::from_palette_focus(seed, PaletteFocus::Accent).c.border_focus, seed.accent);
    assert_eq!(Kit::from_palette_focus(seed, PaletteFocus::Custom(blue)).c.border_focus, blue);
    let default = Kit::from_palette_focus(seed, PaletteFocus::default());
    assert_eq!(Kit::from_palette(seed).c.border_focus, default.c.border_focus);
}
