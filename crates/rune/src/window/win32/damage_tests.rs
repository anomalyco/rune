use bytemuck::Zeroable;

use super::*;
use crate::render::{KIND_BACKDROP_BLUR, KIND_DASHED, KIND_EFFECT, KIND_GLYPH, KIND_GRADIENT, KIND_IMAGE, KIND_INSET, KIND_RADIAL, KIND_RECT, KIND_SEGMENT, KIND_SHADOW, KIND_TRIANGLE, Kind, LayerSpec, NO_CLIP, cpu_boot};

fn old_bounds(i: &Instance, size: (u32, u32)) -> Option<[u32; 4]> {
    let kind = i.params[1];
    if (8.5..9.5).contains(&kind) { return None; }
    let grow = match kind {
        k if (0.5..1.5).contains(&k) => i.params[2] * 3.0 + 1.0,
        k if (1.5..2.5).contains(&k) => 0.0,
        k if (2.5..3.5).contains(&k) || (4.5..9.5).contains(&k) => 1.0,
        _ => 1.0 + i.params[2] * 2.0,
    };
    let [x, y, w, h] = i.rect;
    let (mut x0, mut y0, mut x1, mut y1) = (x - grow, y - grow, x + w + grow, y + h + grow);
    if (1.5..2.5).contains(&kind) && i.params[0] != 0.0 {
        let (cx, cy, r) = (x + w * 0.5, y + h * 0.5, (w * w + h * h).sqrt() * 0.5);
        (x0, y0, x1, y1) = (cx - r, cy - r, cx + r, cy + r);
    }
    let c = i.clip;
    let (x0, y0) = ((x0.max(c[0]) - 1.0).floor().max(0.0), (y0.max(c[1]) - 1.0).floor().max(0.0));
    let (x1, y1) = ((x1.min(c[2]) + 1.0).ceil().min(size.0 as f32), (y1.min(c[3]) + 1.0).ceil().min(size.1 as f32));
    (x1 > x0 && y1 > y0).then(|| [x0 as u32, y0 as u32, x1 as u32, y1 as u32])
}

fn contains(outer: [i32; 4], inner: [i32; 4]) -> bool {
    if inner[2] <= inner[0] || inner[3] <= inner[1] { return true; }
    outer[2] > outer[0] && outer[3] > outer[1]
        && outer[0] <= inner[0] && outer[1] <= inner[1]
        && outer[2] >= inner[2] && outer[3] >= inner[3]
}

#[test]
fn damage_bounds_contain_exact_drawn_pixels_and_tighten_overestimates() {
    let size = (256u32, 256u32);
    let (w, h) = (size.0 as i32, size.1 as i32);
    let mut atlas = crate::text::Atlas::new();
    atlas.data.resize((crate::text::ATLAS_SIZE * crate::text::ATLAS_SIZE) as usize, 0);
    for y in 0..16usize {
        for x in 0..16usize {
            atlas.data[y * crate::text::ATLAS_SIZE as usize + x] = 255;
        }
    }
    let mut images = crate::pixels::ImageAtlas::default();
    let px = crate::Pixels::new();
    px.set(16, 16, vec![255u8; 16 * 16 * 4]);
    let placed = images.place(&px, [16.0, 16.0]).expect("place test image");
    let base = |kind: Kind, rect: [f32; 4]| Instance {
        rect,
        fill: [1.0, 0.4, 0.2, 1.0],
        border: [0.2, 0.8, 1.0, 1.0],
        params: [0.0, kind.as_f32(), 0.0, 1.0],
        clip: NO_CLIP,
        ..Instance::zeroed()
    };
    let cases: Vec<(&str, Instance, bool)> = vec![
        ("rect_plain", base(Kind::Rect, [30.3, 40.7, 50.0, 30.0]), true),
        ("rect_border_soft", Instance { params: [3.0, KIND_RECT, 2.0, 1.0], radii: [8.0; 4], ..base(Kind::Rect, [40.0, 40.0, 60.0, 40.0]) }, true),
        ("rect_clipped", Instance { clip: [45.2, 48.6, 82.4, 70.1], ..base(Kind::Rect, [30.0, 40.0, 80.0, 50.0]) }, true),
        ("rect_banded", Instance { params: [2.0, KIND_RECT, 0.0, 1.0], uv: [-1.0, -1.0, -101.0, 6.0], ..base(Kind::Rect, [50.0, 60.0, 100.0, 80.0]) }, true),
        ("shadow_plain", Instance { params: [0.0, KIND_SHADOW, 6.0, 1.0], radii: [8.0; 4], ..base(Kind::Shadow, [60.0, 60.0, 80.0, 50.0]) }, true),
        ("shadow_banded", Instance { params: [0.0, KIND_SHADOW, 6.0, 1.0], uv: [90.0, -19.0, -119.0, 99.0], ..base(Kind::Shadow, [40.0, 50.0, 100.0, 80.0]) }, true),
        ("glyph_upright", base(Kind::Glyph, [70.0, 70.0, 16.0, 16.0]), true),
        ("glyph_rot_15deg", Instance { params: [0.26, KIND_GLYPH, 0.0, 1.0], uv: [0.0, 0.0, 16.0, 16.0], ..base(Kind::Glyph, [80.0, 80.0, 40.0, 12.0]) }, true),
        ("glyph_rot_45deg", Instance { params: [std::f32::consts::FRAC_PI_4, KIND_GLYPH, 0.0, 1.0], uv: [0.0, 0.0, 16.0, 16.0], ..base(Kind::Glyph, [80.0, 80.0, 40.0, 12.0]) }, true),
        ("glyph_rot_90deg", Instance { params: [std::f32::consts::FRAC_PI_2, KIND_GLYPH, 0.0, 1.0], uv: [0.0, 0.0, 16.0, 16.0], ..base(Kind::Glyph, [80.0, 80.0, 40.0, 12.0]) }, true),
        ("inset", Instance { params: [2.0, KIND_INSET, 4.0, 1.0], uv: [2.0, 2.0, 0.0, 0.0], ..base(Kind::Inset, [50.0, 50.0, 60.0, 40.0]) }, true),
        ("gradient", Instance { uv: [0.1, 0.2, 0.9, 1.0], params: [0.0, KIND_GRADIENT, 1.5, 1.0], ..base(Kind::Gradient, [30.0, 30.0, 50.0, 50.0]) }, true),
        ("segment", Instance { radii: [40.0, 40.0, 110.0, 85.0], params: [3.0, KIND_SEGMENT, 1.0, 1.0], ..base(Kind::Segment, [36.0, 36.0, 78.0, 53.0]) }, true),
        ("triangle", Instance { radii: [50.0, 40.0, 120.0, 60.0], uv: [70.0, 110.0, 0.0, 0.0], params: [7.0, KIND_TRIANGLE, 0.0, 1.0], ..base(Kind::Triangle, [50.0, 40.0, 70.0, 70.0]) }, true),
        ("image", Instance { params: [placed.page as f32, KIND_IMAGE, 0.0, 1.0], uv: [placed.rect[0] as f32, placed.rect[1] as f32, placed.rect[2] as f32, placed.rect[3] as f32], ..base(Kind::Image, [80.0, 80.0, 24.0, 24.0]) }, true),
        ("layer_plain", Instance::begin_layer([40.0, 50.0, 80.0, 60.0], NO_CLIP, LayerSpec::default()), false),
        ("layer_turned", Instance::begin_layer([60.0, 60.0, 80.0, 40.0], NO_CLIP, LayerSpec::default()).turned(0.6, [100.0, 80.0]), false),
        ("layer_end", Instance::end_layer(), false),
        ("dashed", Instance { params: [3.0, KIND_DASHED, 0.5, 1.0], uv: [6.0, 4.0, 0.0, 0.0], ..base(Kind::Dashed, [40.0, 40.0, 80.0, 60.0]) }, true),
        ("radial", Instance { params: [0.0, KIND_RADIAL, 1.0, 1.0], uv: [40.0, 30.0, 40.0, 30.0], ..base(Kind::Radial, [40.0, 40.0, 80.0, 60.0]) }, true),
        ("effect_bleed16", Instance { params: [0.0, KIND_EFFECT, 16.0, 1.0], ..base(Kind::Effect, [60.0, 60.0, 40.0, 30.0]) }, false),
        ("backdrop_blur", Instance { params: [12.0, KIND_BACKDROP_BLUR, 0.0, 1.0], ..base(Kind::BackdropBlur, [50.0, 50.0, 70.0, 50.0]) }, false),
    ];
    let mut tighter = Vec::new();
    for (name, inst, paintable) in &cases {
        let new_r = bounds(inst, size).map_or([0; 4], |r| r.map(|v| v as i32));
        let old_r = old_bounds(inst, size).map_or([0; 4], |r| r.map(|v| v as i32));
        let pb = inst.pixel_bounds(w, h);
        assert!(contains(new_r, pb), "{name}: damage {new_r:?} missed pixel_bounds {pb:?}");
        if *paintable {
            let mut buf = vec![0u8; (w * h * 4) as usize];
            let scene = cpu_boot::Scene::new(std::slice::from_ref(inst), &atlas, &images, size, [0.0; 4]);
            cpu_boot::paint(&scene, &mut cpu_boot::Target::new(&mut buf, size.0, size.1), 1).expect("paint");
            let (mut x0, mut y0, mut x1, mut y1) = (w, h, 0, 0);
            for y in 0..h {
                for x in 0..w {
                    let at = ((y * w + x) * 4) as usize;
                    if buf[at..at + 4] != [0, 0, 0, 0] {
                        x0 = x0.min(x); y0 = y0.min(y); x1 = x1.max(x + 1); y1 = y1.max(y + 1);
                    }
                }
            }
            let painted = if x1 > x0 && y1 > y0 { [x0, y0, x1, y1] } else { [0; 4] };
            assert!(contains(new_r, painted), "{name}: damage {new_r:?} missed CPU-painted {painted:?}");
        }
        if matches!(inst.kind(), Some(Kind::Effect | Kind::BackdropBlur)) {
            let lb = inst.layer_bounds();
            let lb_px = [lb[0].max(0.0).floor() as i32, lb[1].max(0.0).floor() as i32, lb[2].min(w as f32).ceil() as i32, lb[3].min(h as f32).ceil() as i32];
            assert!(contains(new_r, lb_px), "{name}: damage {new_r:?} missed layer_bounds {lb_px:?}");
        }
        let new_a = i64::from((new_r[2] - new_r[0]).max(0)) * i64::from((new_r[3] - new_r[1]).max(0));
        let old_a = i64::from((old_r[2] - old_r[0]).max(0)) * i64::from((old_r[3] - old_r[1]).max(0));
        if new_a < old_a {
            assert!(contains(old_r, new_r), "{name}: tighter damage must be subset of old");
            tighter.push(*name);
        }
    }
    assert!(tighter.contains(&"rect_banded"), "tighter: {tighter:?}");
    assert!(tighter.contains(&"shadow_banded"), "tighter: {tighter:?}");
    assert!(tighter.contains(&"glyph_rot_15deg"), "tighter: {tighter:?}");
    assert!(tighter.contains(&"effect_bleed16"), "tighter: {tighter:?}");
}

#[test]
fn changed_order_check_handles_inserts_deletes_duplicates_and_reorders() {
    let r = |x: f32| Instance {
        rect: [x, 10.0, 20.0, 20.0],
        fill: [1.0; 4],
        params: [0.0, KIND_RECT, 0.0, 1.0],
        clip: NO_CLIP,
        ..Instance::zeroed()
    };
    let mut s = Scratch::default();
    let size = (256, 256);

    // Partial in-place update keeps order.
    let old = [r(0.0), r(30.0), r(60.0), r(90.0)];
    let mut new = old;
    new[2] = r(65.0);
    assert!(matches!(changed(&old, &new, size, &mut s), Area::Rects(ref rs) if !rs.is_empty()));

    // Insert and delete with duplicates preserve order of remaining items.
    let old_dup = [r(0.0), r(30.0), r(30.0), r(60.0), r(90.0)];
    let new_dup = [r(0.0), r(15.0), r(30.0), r(30.0), r(90.0)];
    assert!(matches!(changed(&old_dup, &new_dup, size, &mut s), Area::Rects(ref rs) if !rs.is_empty()));

    // Swapping two unchanged instances triggers full redraw (`Area::All`).
    let swapped = [r(0.0), r(60.0), r(30.0), r(90.0)];
    assert_eq!(changed(&old, &swapped, size, &mut s), Area::All);
}
