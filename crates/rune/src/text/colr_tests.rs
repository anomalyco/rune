use super::*;
use crate::text::{TextStyle, TextSystem};
use crate::tok::Family;

const TEST_GLYPHS: &[u8] = include_bytes!("../../tests/colr/test_glyphs-glyf_colr_1.ttf");
const NOTO_COLRV1: &[u8] = include_bytes!("../../tests/colr/Noto-COLRv1-grinning.ttf");
const NOTO_CBDT: &[u8] = include_bytes!("../../tests/colr/NotoColorEmoji-grinning.ttf");
const PPEM: f32 = 100.0;
const BLUE_INK: u32 = 0x2060C0FF;
const GREEN_INK: u32 = 0x30A050FF;

const GREEN: [u8; 3] = [0, 128, 0];
const WHITE: [u8; 3] = [255, 255, 255];
const RED: [u8; 3] = [255, 0, 0];
const BLUE: [u8; 3] = [0, 0, 255];
const ORANGE: [u8; 3] = [255, 165, 0];
const YELLOW: [u8; 3] = [255, 220, 1];
const SKY: [u8; 3] = [104, 199, 232];
const LINEN: [u8; 3] = [250, 240, 230];
const SLATE: [u8; 3] = [47, 79, 79];

fn glyph(font: &[u8], ch: u32) -> u32 {
    let id = swash::FontRef::from_index(font, 0)
        .unwrap()
        .charmap()
        .map(ch) as u32;
    assert_ne!(id, 0, "U+{ch:X}");
    id
}

fn render(ch: u32, ink: u32) -> Image {
    rasterize(TEST_GLYPHS, 0, glyph(TEST_GLYPHS, ch), PPEM, &[], ink).unwrap()
}

fn at(image: &Image, x: f32, y: f32) -> [u8; 4] {
    let s = PPEM / 1000.0;
    let p = image.placement;
    let col = (x * s - p.left as f32).floor() as u32;
    let row = (p.top as f32 - y * s).floor() as u32;
    assert!(col < p.width && row < p.height, "({x}, {y}) outside {p:?}");
    let i = ((row * p.width + col) * 4) as usize;
    image.rgba[i..i + 4].try_into().unwrap()
}

fn mix(a: [u8; 3], b: [u8; 3], f: f32) -> [u8; 3] {
    std::array::from_fn(|k| {
        let (a, b) = (
            srgb_decode(a[k] as f32 / 255.0),
            srgb_decode(b[k] as f32 / 255.0),
        );
        (srgb_encode(a + (b - a) * f) * 255.0).round() as u8
    })
}

#[track_caller]
fn assert_color(found: [u8; 4], rgb: [u8; 3], alpha: u8) {
    let close =
        rgb.iter().zip(&found).all(|(a, b)| a.abs_diff(*b) <= 3) && found[3].abs_diff(alpha) <= 1;
    assert!(close, "found {found:?}, expected {rgb:?} at alpha {alpha}");
}

fn ink_box(rgba: &[u8], p: swash::zeno::Placement) -> [i32; 4] {
    let mut b = [i32::MAX, i32::MAX, i32::MIN, i32::MIN];
    for (i, px) in rgba.as_chunks::<4>().0.iter().enumerate() {
        if px[3] > 127 {
            let (x, y) = (
                p.left + (i as u32 % p.width) as i32,
                (i as u32 / p.width) as i32 - p.top,
            );
            b = [b[0].min(x), b[1].min(y), b[2].max(x + 1), b[3].max(y + 1)];
        }
    }
    b
}

#[test]
fn linear_gradients_interpolate_in_linear_light_with_each_extend_mode() {
    let t = |x: f32| x / 307.0;
    let pad = render(0xF0500, BLUE_INK);
    assert_color(at(&pad, 75.0, 505.0), mix(GREEN, WHITE, t(75.0) / 0.5), 255);
    assert_color(
        at(&pad, 235.0, 505.0),
        mix(WHITE, RED, (t(235.0) - 0.5) / 0.5),
        255,
    );
    assert_color(at(&pad, 505.0, 505.0), RED, 255);
    assert_color(at(&pad, 905.0, 105.0), RED, 255);
    let repeat = render(0xF0501, BLUE_INK);
    assert_color(
        at(&repeat, 75.0, 505.0),
        at(&pad, 75.0, 505.0)[..3].try_into().unwrap(),
        255,
    );
    assert_color(
        at(&repeat, 505.0, 505.0),
        mix(WHITE, RED, (t(505.0) - 1.5) / 0.5),
        255,
    );
    let reflect = render(0xF0502, BLUE_INK);
    assert_color(
        at(&reflect, 505.0, 505.0),
        mix(GREEN, WHITE, (2.0 - t(505.0)) / 0.5),
        255,
    );
}

#[test]
fn radial_gradients_follow_the_circle_and_extend() {
    let t = |x: f32, y: f32| (x - 166.0).hypot(y - 768.0) / 256.0;
    let pad = render(0xF0503, BLUE_INK);
    assert_color(
        at(&pad, 225.0, 765.0),
        mix(GREEN, WHITE, t(225.0, 765.0) / 0.5),
        255,
    );
    assert_color(
        at(&pad, 345.0, 765.0),
        mix(WHITE, RED, (t(345.0, 765.0) - 0.5) / 0.5),
        255,
    );
    assert_color(at(&pad, 905.0, 105.0), RED, 255);
    let repeat = render(0xF0504, BLUE_INK);
    let far = t(465.0, 765.0);
    assert!((1.0..1.5).contains(&far));
    assert_color(
        at(&repeat, 465.0, 765.0),
        mix(GREEN, WHITE, (far - 1.0) / 0.5),
        255,
    );
    let reflect = render(0xF0505, BLUE_INK);
    assert_color(
        at(&reflect, 465.0, 765.0),
        mix(WHITE, RED, (2.0 - far - 0.5) / 0.5),
        255,
    );
}

#[test]
fn sweep_gradients_run_counter_clockwise_on_the_design_grid() {
    let sweep = render(0xF0202, BLUE_INK);
    assert_color(at(&sweep, 695.0, 635.0), LINEN, 255);
    let offset = |o: f32| (o * 16384.0).round() / 16384.0;
    let f = (0.5 - offset(0.4167)) / (offset(0.5833) - offset(0.4167));
    assert_color(at(&sweep, 645.0, 745.0), mix(BLUE, RED, f), 255);
    assert_color(at(&sweep, 535.0, 795.0), SLATE, 255);
    assert_color(at(&sweep, 695.0, 565.0), SLATE, 255);
}

#[test]
fn glyph_and_box_clips_bound_the_paint() {
    let sweep = render(0xF0202, BLUE_INK);
    assert_eq!(
        at(&sweep, 205.0, 305.0)[3],
        0,
        "outside the circle glyph clip"
    );
    assert_eq!(at(&sweep, 505.0, 605.0)[3], 255);
    let p = render(0xF0C04, BLUE_INK).placement;
    assert_eq!((p.left, p.top, p.width, p.height), (25, 75, 50, 50));
    let p = render(0xF0C00, BLUE_INK).placement;
    assert_eq!((p.left, p.top, p.width, p.height), (0, 100, 50, 50));
}

#[test]
fn composite_modes_and_transforms_combine_layers() {
    let dest_in = render(0xF0A06, BLUE_INK);
    assert_color(at(&dest_in, 505.0, 555.0), YELLOW, 255);
    assert_eq!(at(&dest_in, 255.0, 755.0)[3], 0);
    assert_eq!(at(&dest_in, 755.0, 255.0)[3], 0);
    let multiply = render(0xF0A17, BLUE_INK);
    assert_color(
        at(&multiply, 505.0, 555.0),
        std::array::from_fn(|k| ((YELLOW[k] as u32 * SKY[k] as u32 + 127) / 255) as u8),
        255,
    );
    assert_color(at(&multiply, 255.0, 755.0), YELLOW, 255);
    assert_color(at(&multiply, 755.0, 255.0), SKY, 255);
    let translate = render(0xF0905, BLUE_INK);
    assert_color(at(&translate, 705.0, 705.0), ORANGE, 179);
    assert_color(at(&translate, 505.0, 305.0), BLUE, 128);
    assert_color(at(&translate, 505.0, 705.0), [105, 68, 150], 217);
}

#[test]
fn foreground_palette_entry_paints_with_the_text_ink() {
    let solid = render(0xF0B06, BLUE_INK);
    assert!(solid.foreground);
    assert_color(at(&solid, 505.0, 505.0), [0x20, 0x60, 0xC0], 255);
    assert_color(
        at(&render(0xF0B06, GREEN_INK), 505.0, 505.0),
        [0x30, 0xA0, 0x50],
        255,
    );
    assert_color(
        at(&render(0xF0B07, GREEN_INK), 505.0, 505.0),
        [0x30, 0xA0, 0x50],
        77,
    );
    let t = (505.0 - 100.0) / 800.0;
    for (ink, rgb) in [
        (BLUE_INK, [0x20, 0x60, 0xC0]),
        (GREEN_INK, [0x30, 0xA0, 0x50]),
    ] {
        let linear = render(0xF0B00, ink);
        assert_color(
            at(&linear, 505.0, 505.0),
            mix(rgb, ORANGE, (t - 0.5) / 0.5),
            255,
        );
        assert_color(
            at(&linear, 105.0, 505.0),
            mix(ORANGE, rgb, (5.0 / 800.0) / 0.5),
            255,
        );
    }
    assert!(!render(0xF0500, BLUE_INK).foreground);
}

#[test]
fn colrv1_emoji_matches_the_bitmap_emoji_size_and_placement() {
    let colr = glyph(NOTO_COLRV1, 0x1F600);
    let cbdt = swash::FontRef::from_index(NOTO_CBDT, 0).unwrap();
    let id = cbdt.charmap().map(0x1F600_u32);
    let mut cx = swash::scale::ScaleContext::new();
    for ppem in [16.0, 20.0, 32.0, 64.0, 128.0] {
        let vector = rasterize(NOTO_COLRV1, 0, colr, ppem, &[], BLUE_INK).unwrap();
        let mut scaler = cx.builder(cbdt).size(ppem).hint(false).build();
        let (pixels, at) = crate::text::rasterize_color(&mut scaler, id).unwrap();
        let bitmap = ink_box(&pixels.rgba(), at);
        let drawn = ink_box(&vector.rgba, vector.placement);
        assert!(
            drawn.iter().zip(&bitmap).all(|(a, b)| a.abs_diff(*b) <= 1),
            "{ppem}: {drawn:?} vs {bitmap:?}"
        );
        let wide = ((bitmap[2] - bitmap[0]) as f32 - ppem * 1.0).abs();
        assert!(wide <= ppem * 0.2, "{ppem}: {bitmap:?}");
    }
}

#[test]
fn bitmap_and_colrv0_glyphs_stay_with_swash() {
    let cbdt = swash::FontRef::from_index(NOTO_CBDT, 0)
        .unwrap()
        .charmap()
        .map(0x1F600_u32) as u32;
    assert!(rasterize(NOTO_CBDT, 0, cbdt, 32.0, &[], BLUE_INK).is_none());
    assert!(
        rasterize(
            TEST_GLYPHS,
            0,
            glyph(TEST_GLYPHS, 0xF0E00),
            32.0,
            &[],
            BLUE_INK
        )
        .is_none()
    );
    assert!(
        rasterize(
            TEST_GLYPHS,
            0,
            glyph(TEST_GLYPHS, 0xF0E01),
            32.0,
            &[],
            BLUE_INK
        )
        .is_some()
    );
    let mut ts = TextSystem::new(&[TEST_GLYPHS], ("Inter", "Inter"));
    let style = TextStyle::new(32.0, 400.0).family(Family::Named("COLRv1 Static Test Glyphs"));
    let layout = ts.layout_rich("\u{F0E00}\u{F0E01}", &[], style, 1.0, None);
    let mut quads = Vec::new();
    ts.glyphs_for_layout(&layout, 1.0, [0.0, 0.0], false, &mut quads);
    assert_eq!(quads.len(), 2, "{quads:?}");
    assert!(quads.iter().all(|q| q.color != 0), "{quads:?}");
    let font = swash::FontRef::from_index(TEST_GLYPHS, 0).unwrap();
    let mut scaler = ts.scale_cx.builder(font).size(32.0).hint(false).build();
    let (v0, _) =
        crate::text::rasterize_color(&mut scaler, glyph(TEST_GLYPHS, 0xF0E00) as u16).unwrap();
    assert_eq!(ts.color_glyph(quads[0].color).rgba(), v0.rgba());
    let v1 = rasterize(
        TEST_GLYPHS,
        0,
        glyph(TEST_GLYPHS, 0xF0E01),
        32.0,
        &[],
        BLUE_INK,
    )
    .unwrap();
    assert_eq!(ts.color_glyph(quads[1].color).rgba(), v1.rgba);
}

#[test]
fn foreground_glyphs_cache_per_ink() {
    let mut ts = TextSystem::new(&[TEST_GLYPHS], ("Inter", "Inter"));
    let style = TextStyle::new(32.0, 400.0).family(Family::Named("COLRv1 Static Test Glyphs"));
    let layout = ts.layout_rich("\u{F0B06}\u{F0500}", &[], style, 1.0, None);
    let draw = |ts: &mut TextSystem, ink: &str| {
        ts.set_glyph_inks(crate::color::hex(ink).unwrap(), &[]);
        let mut quads = Vec::new();
        ts.glyphs_for_layout(&layout, 1.0, [0.0, 0.0], false, &mut quads);
        assert_eq!(quads.len(), 2, "{quads:?}");
        let pixels = ts.color_glyph(quads[0].color).rgba();
        let (w, h) = ts.color_glyph(quads[0].color).size();
        let i = ((h / 2 * w + w / 2) * 4) as usize;
        (
            quads[0].color,
            quads[1].color,
            [pixels[i], pixels[i + 1], pixels[i + 2]],
        )
    };
    let blue = draw(&mut ts, "#2060c0");
    let green = draw(&mut ts, "#30a050");
    assert_eq!(blue.2, [0x20, 0x60, 0xC0]);
    assert_eq!(green.2, [0x30, 0xA0, 0x50]);
    assert_ne!(blue.0, green.0);
    assert_eq!(
        blue.1, green.1,
        "a glyph without the foreground entry is shared across inks"
    );
    assert_eq!(draw(&mut ts, "#2060c0"), blue);
    let pixels = ts.color_pixels.len();
    draw(&mut ts, "#30a050");
    assert_eq!(ts.color_pixels.len(), pixels);
}

#[test]
fn malformed_or_empty_requests_draw_nothing() {
    assert!(rasterize(&[], 0, 1, 32.0, &[], BLUE_INK).is_none());
    assert!(
        rasterize(
            TEST_GLYPHS,
            0,
            glyph(TEST_GLYPHS, 0xF0500),
            0.0,
            &[],
            BLUE_INK
        )
        .is_none()
    );
    assert!(
        rasterize(
            TEST_GLYPHS,
            0,
            glyph(TEST_GLYPHS, 0xF0500),
            f32::NAN,
            &[],
            BLUE_INK
        )
        .is_none()
    );
    assert!(
        rasterize(
            TEST_GLYPHS,
            0,
            glyph(TEST_GLYPHS, 0xF1100),
            32.0,
            &[],
            BLUE_INK
        )
        .is_none(),
        "paint cycle"
    );
    assert!(
        rasterize(
            TEST_GLYPHS,
            0,
            glyph(TEST_GLYPHS, 0xF0500),
            100_000.0,
            &[],
            BLUE_INK
        )
        .is_none()
    );
}
