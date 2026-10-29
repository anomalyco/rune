//! Every built-in glyph rasterises like a reference SVG renderer (resvg) at the sizes
//! components draw them. Set `RUNE_GLYPH_PROOF=<dir>` to also write enlarged contact sheets:
//! Rune's mask, resvg's, and their difference, per glyph and size.
#![allow(clippy::disallowed_methods, clippy::expect_used, clippy::let_underscore_must_use, clippy::undocumented_unsafe_blocks, clippy::unwrap_used)]

use resvg::{tiny_skia, usvg};
use rune::Icon;
use rune::text::icon_coverage;
use rune_kit::glyphs;

const ALL: [&Icon; 26] = [
    &glyphs::CHECK,
    &glyphs::CHECK_SMALL,
    &glyphs::CHEVRON_DOWN,
    &glyphs::CHEVRON_UP,
    &glyphs::CARET_RIGHT,
    &glyphs::TRIANGLE_DOWN,
    &glyphs::TRIANGLE_RIGHT,
    &glyphs::CLOSE,
    &glyphs::COPY,
    &glyphs::SEARCH,
    &glyphs::CIRCLE_X,
    &glyphs::LOCK,
    &glyphs::EYE,
    &glyphs::EYE_OFF,
    &glyphs::DASH,
    &glyphs::FOLDER,
    &glyphs::FOLDER_OPEN,
    &glyphs::FILE,
    &glyphs::FILE_CODE,
    &glyphs::FILE_TEXT,
    &glyphs::FILE_IMAGE,
    &glyphs::MIC,
    &glyphs::KEYBOARD,
    &glyphs::CURSOR_TEXT,
    &glyphs::ARROW_RIGHT,
    &glyphs::SPARKLE,
];

/// Glyph sizes in points (a password manager's 16–20 pt plus a large one) at 1× and 2×.
const POINTS: [u32; 3] = [16, 20, 32];

fn reference(icon: &Icon, px: u32) -> Vec<u8> {
    // Rune strokes without filling when an element names only a stroke, like icon sets that put
    // `fill="none"` on the root `<svg>`.
    let v = icon.view;
    let svg = format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {v} {v}" fill="none" color="black">{}</svg>"#,
        icon.body
    );
    let tree = usvg::Tree::from_str(&svg, &usvg::Options::default()).expect("glyph parses as SVG");
    // tiny-skia quantises anti-aliased coverage to quarters vertically, so render 4× and
    // average each 4×4 block for exact area coverage.
    const SS: u32 = 4;
    let big = px * SS;
    let mut pixmap = tiny_skia::Pixmap::new(big, big).unwrap();
    let s = big as f32 / v;
    resvg::render(&tree, tiny_skia::Transform::from_scale(s, s), &mut pixmap.as_mut());
    let alpha = pixmap.pixels();
    (0..px * px)
        .map(|i| {
            let (x, y) = (i % px * SS, i / px * SS);
            let sum: u32 = (0..SS * SS).map(|j| alpha[((y + j / SS) * big + x + j % SS) as usize].alpha() as u32).sum();
            ((sum + SS * SS / 2) / (SS * SS)) as u8
        })
        .collect()
}

/// Coverage that differs, as a share of the coverage either mask has (0 = identical).
fn mismatch(a: &[u8], b: &[u8]) -> f32 {
    let diff: u32 = a.iter().zip(b).map(|(&a, &b)| a.abs_diff(b) as u32).sum();
    let ink: u32 = a.iter().zip(b).map(|(&a, &b)| a.max(b) as u32).sum();
    diff as f32 / ink.max(1) as f32
}

#[test]
fn built_in_glyphs_match_a_reference_rasteriser() {
    let proof = std::env::var_os("RUNE_GLYPH_PROOF").map(std::path::PathBuf::from);
    let mut failures = Vec::new();
    let mut sheets = [Vec::new(), Vec::new()];
    for icon in ALL {
        let mut rows = [Vec::new(), Vec::new()];
        for pt in POINTS {
            for scale in [1, 2] {
                let px = pt * scale;
                let (ours, theirs) = (icon_coverage(icon, px), reference(icon, px));
                let m = mismatch(&ours, &theirs);
                println!("{:18} {pt:2}pt @{scale}x  mismatch {m:.3}", icon.name);
                // Anti-aliasing differs between rasterisers by a few percent at these sizes; a
                // wrong control point or a stray fill differs by far more.
                if m > 0.12 {
                    failures.push(format!("{} at {pt}pt @{scale}x: {m:.3}", icon.name));
                }
                rows[scale as usize - 1].push((px, ours, theirs));
            }
        }
        for (sheet, row) in sheets.iter_mut().zip(rows) {
            sheet.push((icon.name, row));
        }
    }
    if let Some(dir) = proof {
        std::fs::create_dir_all(&dir).unwrap();
        for (scale, sheet) in sheets.iter().enumerate() {
            let scale = scale + 1;
            write_sheet(&dir.join(format!("glyphs-{scale}x.png")), sheet);
            let rows: Vec<_> =
                sheet.iter().filter(|(n, _)| ["kit-lock", "kit-eye", "kit-eye-off"].contains(n)).cloned().collect();
            write_sheet(&dir.join(format!("lock-eye-{scale}x.png")), &rows);
        }
    }
    assert!(failures.is_empty(), "glyphs differ from resvg:\n{}", failures.join("\n"));
}

type Row = (&'static str, Vec<(u32, Vec<u8>, Vec<u8>)>);

/// One row per glyph; per size: Rune, resvg, then red where Rune has extra ink and blue where
/// it lacks ink. Cells are enlarged 4× (nearest) on a white page.
fn write_sheet(path: &std::path::Path, rows: &[Row]) {
    const ZOOM: u32 = 4;
    const GAP: u32 = 8;
    let cell = rows[0].1.iter().map(|(px, ..)| px).max().unwrap() * ZOOM;
    let cols = POINTS.len() as u32 * 3;
    let (w, h) = (cols * (cell + GAP) + GAP, rows.len() as u32 * (cell + GAP) + GAP);
    let mut pm = tiny_skia::Pixmap::new(w, h).unwrap();
    pm.fill(tiny_skia::Color::WHITE);
    let stride = w as usize;
    let data = pm.pixels_mut();
    for (r, (_, sizes)) in rows.iter().enumerate() {
        for (s, (px, ours, theirs)) in sizes.iter().enumerate() {
            for k in 0..3 {
                let (x0, y0) = (GAP + (s as u32 * 3 + k) * (cell + GAP), GAP + r as u32 * (cell + GAP));
                for y in 0..px * ZOOM {
                    for x in 0..px * ZOOM {
                        let i = ((y / ZOOM) * px + x / ZOOM) as usize;
                        let (a, b) = (ours[i] as i32, theirs[i] as i32);
                        let rgb = match k {
                            0 => [255 - a; 3],
                            1 => [255 - b; 3],
                            _ => {
                                let d = (a - b).clamp(-255, 255);
                                if d > 0 { [255, 255 - d, 255 - d] } else { [255 + d, 255 + d, 255] }
                            }
                        };
                        let rgb = rgb.map(|c| c.clamp(0, 255) as u8);
                        data[(y0 + y) as usize * stride + (x0 + x) as usize] =
                            tiny_skia::ColorU8::from_rgba(rgb[0], rgb[1], rgb[2], 255).premultiply();
                    }
                }
            }
        }
    }
    pm.save_png(path).unwrap();
}

#[test]
fn extra_glyphs_rasterize_with_ink() {
    for icon in [&glyphs::MIC, &glyphs::KEYBOARD, &glyphs::CURSOR_TEXT, &glyphs::ARROW_RIGHT, &glyphs::SPARKLE] {
        let ink: u32 = icon_coverage(icon, 32).iter().map(|&a| a as u32).sum();
        assert!(ink > 2000, "{} has no ink", icon.name);
    }
}
