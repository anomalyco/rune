use super::*;
use std::hash::{BuildHasher, Hash, RandomState};

fn hash(x: impl Hash, s: &RandomState) -> u64 {
    s.hash_one(x)
}

#[test]
fn equal_colours_hash_equal() {
    let s = RandomState::new();
    let a = oklcha(0.6, 0.1, 250.0, 0.5);
    assert_eq!(hash(a, &s), hash(oklcha(0.6, 0.1, 250.0, 0.5), &s));
    let zero = Color { r: 0.0, ..Color::BLACK };
    let negative = Color { r: -0.0, ..Color::BLACK };
    assert_eq!(zero, negative);
    assert_eq!(hash(zero, &s), hash(negative, &s));
    assert_ne!(hash(a, &s), hash(a.alpha(0.6), &s));
    assert_eq!(hash(Color { a: f32::NAN, ..a }, &s), hash(Color { a: -f32::NAN, ..a }, &s));
}

#[test]
fn display_p3_primaries_preserve_extended_linear_srgb_channels() {
    let p3_red = Color::p3(1.0, 0.0, 0.0);
    assert!((p3_red.r - 1.224_940_2).abs() < 1e-5, "p3 red r={}", p3_red.r);
    assert!((p3_red.g - -0.042_056_955).abs() < 1e-5, "p3 red g={}", p3_red.g);
    assert!((p3_red.b - -0.019_637_555).abs() < 1e-5, "p3 red b={}", p3_red.b);
    assert_eq!(p3_red.a, 1.0);

    let p3_green = Color::p3(0.0, 1.0, 0.0);
    assert!((p3_green.r - -0.224_940_18).abs() < 1e-5, "p3 green r={}", p3_green.r);
    assert!((p3_green.g - 1.042_057).abs() < 1e-5, "p3 green g={}", p3_green.g);
    assert!((p3_green.b - -0.078_636_05).abs() < 1e-5, "p3 green b={}", p3_green.b);

    let p3_blue = Color::p3(0.0, 0.0, 1.0);
    assert!(p3_blue.r.abs() < 1e-6);
    assert!(p3_blue.g.abs() < 1e-6);
    assert!((p3_blue.b - 1.098_273_6).abs() < 1e-5, "p3 blue b={}", p3_blue.b);
}

#[test]
fn display_p3_neutrals_and_round_trip_match_css_color_4() {
    for v in [0.0_f32, 0.25, 0.5, 0.75, 1.0] {
        let gray = Color::p3(v, v, v);
        let lin = srgb_decode(v);
        assert!((gray.r - lin).abs() < 1e-5, "v={v}: r={} lin={lin}", gray.r);
        assert!((gray.g - lin).abs() < 1e-5, "v={v}: g={} lin={lin}", gray.g);
        assert!((gray.b - lin).abs() < 1e-5, "v={v}: b={} lin={lin}", gray.b);
    }

    for [r, g, b, a] in [
        [1.0_f32, 0.0, 0.0, 1.0],
        [0.0, 1.0, 0.0, 0.5],
        [0.0, 0.0, 1.0, 0.25],
        [0.92, 0.21, 0.48, 0.8],
        [0.12, 0.84, 0.31, 1.0],
    ] {
        let c = Color::p3a(r, g, b, a);
        let back = c.to_p3();
        let want = [r, g, b, a];
        assert!((back[0] - r).abs() < 1e-4, "r mismatch: {back:?} vs {want:?}");
        assert!((back[1] - g).abs() < 1e-4, "g mismatch: {back:?} vs {want:?}");
        assert!((back[2] - b).abs() < 1e-4, "b mismatch: {back:?} vs {want:?}");
        assert!((back[3] - a).abs() < 1e-6);
    }

    assert_eq!(Color::p3a(1.0, 0.0, 0.0, 2.0).a, 1.0);
    assert_eq!(Color::p3a(1.0, 0.0, 0.0, -0.5).a, 0.0);
}

#[test]
fn oklch_wide_preserves_out_of_srgb_gamut_while_oklch_stays_clamped() {
    // High-chroma red/green inside Display P3 but outside sRGB.
    let clamped = oklch(0.65, 0.29, 29.0);
    let wide = oklch_wide(0.65, 0.29, 29.0);

    assert!(
        (0.0..=1.0).contains(&clamped.r)
            && (0.0..=1.0).contains(&clamped.g)
            && (0.0..=1.0).contains(&clamped.b),
        "default oklch must remain clamped to sRGB: {clamped:?}"
    );
    assert!(
        wide.r > 1.0 || wide.g < 0.0 || wide.b < 0.0,
        "oklch_wide must preserve out-of-sRGB-gamut linear channels: {wide:?}"
    );
    assert_ne!(clamped, wide);

    let wide_alpha = oklcha_wide(0.65, 0.29, 29.0, 0.4);
    assert_eq!(wide_alpha.r, wide.r);
    assert_eq!(wide_alpha.g, wide.g);
    assert_eq!(wide_alpha.b, wide.b);
    assert_eq!(wide_alpha.a, 0.4);
    assert_eq!(Color::from_oklab_unclamped([0.65, 0.25, 0.12, 1.5]).a, 1.0);
    assert_eq!(Color::from_oklab_unclamped([0.65, 0.25, 0.12, -0.5]).a, 0.0);

    // In-gamut colours match between clamped and unclamped constructors.
    let in_gamut_clamped = oklch(0.6, 0.08, 240.0);
    let in_gamut_wide = oklch_wide(0.6, 0.08, 240.0);
    assert_eq!(in_gamut_clamped, in_gamut_wide);
}
