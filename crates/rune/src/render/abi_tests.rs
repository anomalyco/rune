use std::mem::{offset_of, size_of};

use bytemuck::Zeroable;

use super::*;

#[test]
fn kind_enum_round_trips_and_rejects_invalid_floats() {
    assert_eq!(size_of::<Option<Kind>>(), 1);
    let all = [
        (Kind::Rect, KIND_RECT, 0u8),
        (Kind::Shadow, KIND_SHADOW, 1),
        (Kind::Glyph, KIND_GLYPH, 2),
        (Kind::Inset, KIND_INSET, 3),
        (Kind::Gradient, KIND_GRADIENT, 4),
        (Kind::Segment, KIND_SEGMENT, 5),
        (Kind::Triangle, KIND_TRIANGLE, 6),
        (Kind::Image, KIND_IMAGE, 7),
        (Kind::Layer, KIND_LAYER, 8),
        (Kind::LayerEnd, KIND_LAYER_END, 9),
        (Kind::Dashed, KIND_DASHED, 10),
        (Kind::Radial, KIND_RADIAL, 11),
        (Kind::Effect, KIND_EFFECT, 12),
        (Kind::BackdropBlur, KIND_BACKDROP_BLUR, 13),
    ];
    assert_eq!(all.len(), Kind::COUNT);
    for (kind, constant, num) in all {
        assert_eq!(kind.as_u8(), num);
        assert_eq!(kind.as_u32(), u32::from(num));
        assert_eq!(kind.as_f32(), constant);
        assert_eq!(constant, f32::from(num));
        assert_eq!(Kind::from_u8(num), Some(kind));
        assert_eq!(Kind::from_f32(constant), Some(kind));
        let mut inst = Instance::zeroed();
        inst.params[1] = constant;
        assert_eq!(inst.kind(), Some(kind));
    }
    assert_eq!(Kind::from_f32(-0.0), Some(Kind::Rect));
    assert_eq!(Kind::from_u8(14), None);
    for bad in [-1.0, -0.5, 0.5, 1.5, 8.5, 9.5, 11.5, 13.5, 14.0, 255.0, f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
        assert_eq!(Kind::from_f32(bad), None, "expected None for {bad:?}");
    }
}

#[test]
fn instance_word_indices_match_field_offsets() {
    assert_eq!(INSTANCE_WORDS, 32);
    assert_eq!(word::RECT * 4, offset_of!(Instance, rect));
    assert_eq!(word::RECT_Y, 1);
    assert_eq!(word::FILL, 8..12);
    assert_eq!(word::FILL.start * 4, offset_of!(Instance, fill));
    assert_eq!(word::BORDER, 12..16);
    assert_eq!(word::BORDER.start * 4, offset_of!(Instance, border));
    assert_eq!(word::PARAMS * 4, offset_of!(Instance, params));
    assert_eq!(word::PARAMS_W, 19);
    assert_eq!(word::CLIP * 4, offset_of!(Instance, clip));
    assert_eq!(word::CLIP_Y0, 25);
    assert_eq!(word::CLIP_Y1, 27);
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
fn wgsl_uniform_and_storage_layouts_match_rust() {
    use crate::gpu::springs;
    use wgpu::naga::{Module, TypeInner, front::wgsl, proc::Layouter, valid::{Capabilities, ValidationFlags, Validator}};

    let parse = |label: &str, source: &str| -> (Module, Layouter) {
        let module = wgsl::parse_str(source).unwrap_or_else(|e| panic!("{label}: {}", e.emit_to_string(source)));
        Validator::new(ValidationFlags::all(), Capabilities::default())
            .validate(&module)
            .unwrap_or_else(|e| panic!("{label}: {e}"));
        let mut layouter = Layouter::default();
        layouter.update(module.to_ctx()).unwrap_or_else(|e| panic!("{label}: {e}"));
        (module, layouter)
    };

    let check_struct = |label: &str, module: &Module, layouter: &Layouter, name: &str, size: usize, expected: &[(&str, usize)]| {
        let (handle, ty) = module
            .types
            .iter()
            .find(|(_, t)| t.name.as_deref() == Some(name))
            .unwrap_or_else(|| panic!("{label}: WGSL struct {name} not found"));
        assert_eq!(layouter[handle].size as usize, size, "{label} {name} size");
        let TypeInner::Struct { members, span } = &ty.inner else { panic!("{label}: {name} is not a struct") };
        assert_eq!(*span as usize, size, "{label} {name} span");
        assert_eq!(members.len(), expected.len(), "{label} {name} member count");
        for (member, &(field, offset)) in members.iter().zip(expected) {
            assert_eq!(member.name.as_deref(), Some(field), "{label} {name}.{field} name");
            assert_eq!(member.offset as usize, offset, "{label} {name}.{field} offset");
        }
    };

    let globals_fields: &[(&str, usize)] = &[
        ("viewport", offset_of!(Globals, viewport)),
        ("pad", offset_of!(Globals, pad)),
        ("flags", offset_of!(Globals, flags)),
    ];
    let level_fields: &[(&str, usize)] = &[
        ("w", offset_of!(backdrop_blur::Level, w)),
        ("region", offset_of!(backdrop_blur::Level, region)),
        ("map", offset_of!(backdrop_blur::Level, map)),
        ("place", offset_of!(backdrop_blur::Level, place)),
    ];
    let layer_blur_fields: &[(&str, usize)] = &[
        ("config", offset_of!(layer_blur::Uniforms, meta)),
        ("mapping", offset_of!(layer_blur::Uniforms, mapping)),
        ("taps", offset_of!(layer_blur::Uniforms, taps)),
    ];

    let (classic, classic_layout) = parse("uber_wgsl::classic", uber_wgsl::classic());
    check_struct("CLASSIC", &classic, &classic_layout, "Globals", size_of::<Globals>(), globals_fields);

    // Also confirm Instance's vertex attributes agree with `struct In` locations and `offset_of!`.
    let expected_attrs = [
        (0u32, offset_of!(Instance, rect) as u64, wgpu::VertexFormat::Float32x4),
        (1, offset_of!(Instance, radii) as u64, wgpu::VertexFormat::Float32x4),
        (2, offset_of!(Instance, fill) as u64, wgpu::VertexFormat::Float32x4),
        (3, offset_of!(Instance, border) as u64, wgpu::VertexFormat::Float32x4),
        (4, offset_of!(Instance, params) as u64, wgpu::VertexFormat::Float32x4),
        (5, offset_of!(Instance, uv) as u64, wgpu::VertexFormat::Float32x4),
        (6, offset_of!(Instance, clip) as u64, wgpu::VertexFormat::Float32x4),
        (7, offset_of!(Instance, fade) as u64, wgpu::VertexFormat::Uint32x4),
    ];
    let vlayout = metallib::uber_vertex_layout();
    assert_eq!(vlayout.array_stride, size_of::<Instance>() as u64);
    assert_eq!(vlayout.attributes.len(), expected_attrs.len());
    for (attr, &(loc, off, fmt)) in vlayout.attributes.iter().zip(&expected_attrs) {
        assert_eq!((attr.shader_location, attr.offset, attr.format), (loc, off, fmt));
    }

    let (windfoil, windfoil_layout) = parse("uber_wgsl::windfoil", uber_wgsl::windfoil());
    check_struct("WINDFOIL", &windfoil, &windfoil_layout, "Globals", size_of::<Globals>(), globals_fields);

    let (bd_blur, bd_blur_layout) = parse("backdrop_blur.wgsl", backdrop_blur::BLUR_WGSL);
    check_struct("backdrop_blur.wgsl", &bd_blur, &bd_blur_layout, "Level", size_of::<backdrop_blur::Level>(), level_fields);

    let composite_src = backdrop_blur::composite_wgsl();
    let (bd_comp, bd_comp_layout) = parse("backdrop_composite.wgsl", &composite_src);
    check_struct("backdrop_composite.wgsl", &bd_comp, &bd_comp_layout, "Globals", size_of::<Globals>(), globals_fields);
    check_struct("backdrop_composite.wgsl", &bd_comp, &bd_comp_layout, "Level", size_of::<backdrop_blur::Level>(), level_fields);

    let (lb, lb_layout) = parse("layer_blur.wgsl", layer_blur::WGSL);
    check_struct("layer_blur.wgsl", &lb, &lb_layout, "Uniforms", size_of::<layer_blur::Uniforms>(), layer_blur_fields);

    let springs_src = format!("{}\n{}", uber_wgsl::classic(), springs::SHADER);
    let (sp, sp_layout) = parse("CLASSIC+springs.wgsl", &springs_src);
    check_struct("CLASSIC+springs.wgsl", &sp, &sp_layout, "Globals", size_of::<Globals>(), globals_fields);
    check_struct(
        "CLASSIC+springs.wgsl",
        &sp,
        &sp_layout,
        "SpringNode",
        size_of::<springs::Node>(),
        &[
            ("x", offset_of!(springs::Node, x)),
            ("y", offset_of!(springs::Node, y)),
            ("opacity", offset_of!(springs::Node, opacity)),
        ],
    );
}

/// Exact `shader.wgsl` `base_vs` quad corners -> pixel centres `[x0, y0, x1, y1)` inside clip & target.
fn shader_exact_pixels(i: &Instance, w: i32, h: i32) -> [i32; 4] {
    let kind = i.params[1];
    if kind > 11.5 || kind == KIND_LAYER_END {
        return [0; 4];
    }
    let mut grow = 1.0 + i.params[2] * 2.0;
    if kind > 0.5 && kind < 1.5 {
        grow = i.params[2] * 3.0 + 1.0;
    } else if kind > 1.5 && kind < 2.5 {
        grow = 0.0;
    } else if (kind > 2.5 && kind < 3.5) || (kind > 4.5 && kind < 9.5) {
        grow = 1.0;
    }
    let mut lo = [i.rect[0] - grow, i.rect[1] - grow];
    let mut hi = [lo[0] + i.rect[2] + 2.0 * grow, lo[1] + i.rect[3] + 2.0 * grow];
    if kind < 1.5 && i.uv[2] < 0.0 {
        lo = [lo[0].max(i.rect[0] + i.uv[0]), lo[1].max(i.rect[1] + i.uv[1])];
        hi = [hi[0].min(i.rect[0] - i.uv[2]).max(lo[0]), hi[1].min(i.rect[1] + i.uv[3]).max(lo[1])];
    }
    let mut min_x = f32::INFINITY;
    let mut min_y = f32::INFINITY;
    let mut max_x = f32::NEG_INFINITY;
    let mut max_y = f32::NEG_INFINITY;
    for vi in 0..4u32 {
        let p = [if vi & 1 != 0 { hi[0] } else { lo[0] }, if vi >> 1 != 0 { hi[1] } else { lo[1] }];
        let mut screen = p;
        if kind > 1.5 && kind < 2.5 && i.params[0] != 0.0 {
            let center = [i.rect[0] + i.rect[2] * 0.5, i.rect[1] + i.rect[3] * 0.5];
            let q = [p[0] - center[0], p[1] - center[1]];
            let (s, c) = i.params[0].sin_cos();
            screen = [center[0] + c * q[0] - s * q[1], center[1] + s * q[0] + c * q[1]];
        }
        if kind > 7.5 && kind < 8.5 && i.border[1] != 0.0 {
            let q = [p[0] - i.border[2], p[1] - i.border[3]];
            let (s, c) = i.border[1].sin_cos();
            screen = [i.border[2] + c * q[0] - s * q[1], i.border[3] + s * q[0] + c * q[1]];
        }
        min_x = min_x.min(screen[0]);
        min_y = min_y.min(screen[1]);
        max_x = max_x.max(screen[0]);
        max_y = max_y.max(screen[1]);
    }
    let first = |v: f32| (v - 0.5).ceil();
    let c = i.clip;
    // Exact `clip_alpha > 0` reach for hard clip edges (`ramp(d, 0) = clamp(d + 0.5, 0, 1)`):
    // pixel `x` has centre `x + 0.5`, so `(x + 0.5 - c0) + 0.5 > 0` <=> `x >= c0.floor()`,
    // and `(c2 - (x + 0.5)) + 0.5 > 0` <=> `x < c2.ceil()`.
    let x0 = first(min_x).max(c[0].floor()).max(0.0);
    let y0 = first(min_y).max(c[1].floor()).max(0.0);
    let x1 = first(max_x).min(c[2].ceil()).min(w as f32);
    let y1 = first(max_y).min(c[3].ceil()).min(h as f32);
    if !(x1 > x0 && y1 > y0) {
        return [0; 4];
    }
    [x0 as i32, y0 as i32, x1 as i32, y1 as i32]
}

fn contains(outer: [i32; 4], inner: [i32; 4]) -> bool {
    if inner[2] <= inner[0] || inner[3] <= inner[1] {
        return true;
    }
    outer[2] > outer[0]
        && outer[3] > outer[1]
        && outer[0] <= inner[0]
        && outer[1] <= inner[1]
        && outer[2] >= inner[2]
        && outer[3] >= inner[3]
}

fn area_i32(r: [i32; 4]) -> i64 {
    i64::from((r[2] - r[0]).max(0)) * i64::from((r[3] - r[1]).max(0))
}

fn old_layer_bounds(i: &Instance) -> [f32; 4] {
    let mut rect = i.rect;
    if i.params[1] == KIND_GLYPH && i.params[0] != 0.0 {
        let (sin, cos) = i.params[0].sin_cos();
        let width = cos.abs() * rect[2] + sin.abs() * rect[3];
        let height = sin.abs() * rect[2] + cos.abs() * rect[3];
        rect = [rect[0] + (rect[2] - width) * 0.5, rect[1] + (rect[3] - height) * 0.5, width, height];
    }
    let grow = match i.params[1] {
        KIND_GLYPH => 0.0,
        KIND_SHADOW => i.params[2] * 3.0 + 1.0,
        KIND_EFFECT => i.params[2] + 1.0,
        KIND_RECT | KIND_GRADIENT | KIND_DASHED | KIND_RADIAL => 1.0 + i.params[2] * 2.0,
        _ => 1.0,
    };
    [
        (rect[0] - grow).max(i.clip[0]),
        (rect[1] - grow).max(i.clip[1]),
        (rect[0] + rect[2] + grow).min(i.clip[2]),
        (rect[1] + rect[3] + grow).min(i.clip[3]),
    ]
}

#[test]
fn instance_bounds_contain_exact_drawn_pixels_across_all_kinds() {
    let (w, h) = (256i32, 256i32);
    let mut atlas = crate::text::Atlas::new();
    atlas.data.resize((crate::text::ATLAS_SIZE * crate::text::ATLAS_SIZE) as usize, 0);
    // Put a solid 16×16 glyph mask at (0, 0) on page 0 so CPU rasterization paints every glyph pixel.
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

    let mut cases: Vec<(&str, Instance, bool)> = vec![
        // (name, instance, cpu_paintable)
        ("rect_plain", base(Kind::Rect, [30.3, 40.7, 50.0, 30.0]), true),
        ("rect_border_soft", Instance { params: [3.0, KIND_RECT, 2.0, 1.0], radii: [8.0; 4], ..base(Kind::Rect, [40.0, 40.0, 60.0, 40.0]) }, true),
        ("rect_clipped", Instance { clip: [45.2, 48.6, 82.4, 70.1], ..base(Kind::Rect, [30.0, 40.0, 80.0, 50.0]) }, true),
        // Banded border ring (top band via uv.z < 0).
        ("rect_banded", Instance { params: [2.0, KIND_RECT, 0.0, 1.0], uv: [-1.0, -1.0, -101.0, 6.0], ..base(Kind::Rect, [50.0, 60.0, 100.0, 80.0]) }, true),
        ("shadow_plain", Instance { params: [0.0, KIND_SHADOW, 6.0, 1.0], radii: [8.0; 4], ..base(Kind::Shadow, [60.0, 60.0, 80.0, 50.0]) }, true),
        ("shadow_hole", Instance { params: [0.0, KIND_SHADOW, 5.0, 1.0], uv: [10.0, 10.0, 40.0, 20.0], ..base(Kind::Shadow, [60.0, 60.0, 80.0, 50.0]) }, true),
        // Banded shadow (right strip via uv.z < 0).
        ("shadow_banded", Instance { params: [0.0, KIND_SHADOW, 6.0, 1.0], uv: [90.0, -19.0, -119.0, 99.0], ..base(Kind::Shadow, [40.0, 50.0, 100.0, 80.0]) }, true),
        ("glyph_upright", Instance { uv: [0.0, 0.0, 0.0, 0.0], ..base(Kind::Glyph, [70.0, 70.0, 16.0, 16.0]) }, true),
        ("glyph_scaled", Instance { uv: [0.0, 0.0, 16.0, 16.0], ..base(Kind::Glyph, [70.2, 70.8, 32.0, 12.0]) }, true),
        ("inset_sharp", Instance { params: [3.0, KIND_INSET, 0.0, 1.0], ..base(Kind::Inset, [50.0, 50.0, 60.0, 40.0]) }, true),
        ("inset_blur", Instance { params: [2.0, KIND_INSET, 4.0, 1.0], uv: [2.0, 2.0, 0.0, 0.0], ..base(Kind::Inset, [50.0, 50.0, 60.0, 40.0]) }, true),
        ("gradient_fill", Instance { uv: [0.1, 0.2, 0.9, 1.0], params: [0.0, KIND_GRADIENT, 1.5, 1.0], ..base(Kind::Gradient, [30.0, 30.0, 50.0, 50.0]) }, true),
        ("segment_round", Instance { radii: [40.0, 40.0, 110.0, 85.0], params: [3.0, KIND_SEGMENT, 1.0, 1.0], ..base(Kind::Segment, [36.0, 36.0, 78.0, 53.0]) }, true),
        ("triangle", Instance { radii: [50.0, 40.0, 120.0, 60.0], uv: [70.0, 110.0, 0.0, 0.0], params: [7.0, KIND_TRIANGLE, 0.0, 1.0], ..base(Kind::Triangle, [50.0, 40.0, 70.0, 70.0]) }, true),
        ("image", Instance { params: [placed.page as f32, KIND_IMAGE, 0.0, 1.0], uv: [placed.rect[0] as f32, placed.rect[1] as f32, placed.rect[2] as f32, placed.rect[3] as f32], ..base(Kind::Image, [80.0, 80.0, 24.0, 24.0]) }, true),
        ("dashed", Instance { params: [3.0, KIND_DASHED, 0.5, 1.0], uv: [6.0, 4.0, 0.0, 0.0], ..base(Kind::Dashed, [40.0, 40.0, 80.0, 60.0]) }, true),
        ("radial", Instance { params: [0.0, KIND_RADIAL, 1.0, 1.0], uv: [40.0, 30.0, 40.0, 30.0], ..base(Kind::Radial, [40.0, 40.0, 80.0, 60.0]) }, true),
        ("linear_angle", Instance { params: [0.0, KIND_RADIAL, 1.0, 1.0], uv: [0.01, 0.005, 0.0, 0.0], ..base(Kind::Radial, [40.0, 40.0, 80.0, 60.0]) }, true),
        ("layer_plain", Instance::begin_layer([40.0, 50.0, 80.0, 60.0], NO_CLIP, LayerSpec::default()), false),
        ("layer_turned", Instance::begin_layer([60.0, 60.0, 80.0, 40.0], NO_CLIP, LayerSpec::default()).turned(0.6, [100.0, 80.0]), false),
        ("layer_end", Instance::end_layer(), false),
        ("effect_bleed0", Instance { params: [0.0, KIND_EFFECT, 0.0, 1.0], ..base(Kind::Effect, [60.0, 60.0, 40.0, 30.0]) }, false),
        ("effect_bleed16", Instance { params: [0.0, KIND_EFFECT, 16.0, 1.0], ..base(Kind::Effect, [60.0, 60.0, 40.0, 30.0]) }, false),
        ("backdrop_blur", Instance { params: [12.0, KIND_BACKDROP_BLUR, 0.0, 1.0], ..base(Kind::BackdropBlur, [50.0, 50.0, 70.0, 50.0]) }, false),
    ];

    // Spread of rotated glyphs across angles and aspect ratios.
    for &(label, rect, angle) in &[
        ("glyph_rot_small", [80.0, 80.0, 40.0, 12.0], 0.05f32),
        ("glyph_rot_15deg", [80.0, 80.0, 40.0, 12.0], 0.26),
        ("glyph_rot_45deg", [80.0, 80.0, 40.0, 12.0], std::f32::consts::FRAC_PI_4),
        ("glyph_rot_90deg", [80.0, 80.0, 40.0, 12.0], std::f32::consts::FRAC_PI_2),
        ("glyph_rot_tall", [80.0, 80.0, 12.0, 40.0], 1.1),
        ("glyph_rot_clipped", [80.0, 80.0, 40.0, 12.0], 0.4),
    ] {
        let mut inst = Instance { params: [angle, KIND_GLYPH, 0.0, 1.0], uv: [0.0, 0.0, 16.0, 16.0], ..base(Kind::Glyph, rect) };
        if label == "glyph_rot_clipped" {
            inst.clip = [85.0, 75.0, 115.0, 98.0];
        }
        cases.push((label, inst, true));
    }

    let mut tighter_layer_sites = Vec::new();

    for (name, inst, paintable) in &cases {
        let exact_vs = shader_exact_pixels(inst, w, h);
        let pb = inst.pixel_bounds(w, h);
        assert!(contains(pb, exact_vs), "{name}: pixel_bounds {pb:?} missed base_vs exact {exact_vs:?}");
        assert_eq!(cpu_boot::bin::pixel_bounds(inst, w, h), pb, "{name}: bin::pixel_bounds delegates to Instance::pixel_bounds");

        // Ground truth from actual CPU rasterized pixels.
        let mut painted_box = [0i32; 4];
        if *paintable {
            let mut buf = vec![0u8; (w * h * 4) as usize];
            let scene = cpu_boot::Scene::new(std::slice::from_ref(inst), &atlas, &images, (w as u32, h as u32), [0.0; 4]);
            cpu_boot::paint(&scene, &mut cpu_boot::Target::new(&mut buf, w as u32, h as u32), 1)
                .unwrap_or_else(|e| panic!("{name}: cpu_boot::paint failed: {e:?}"));
            let (mut x0, mut y0, mut x1, mut y1) = (w, h, 0, 0);
            for y in 0..h {
                for x in 0..w {
                    let at = ((y * w + x) * 4) as usize;
                    if buf[at..at + 4] != [0, 0, 0, 0] {
                        x0 = x0.min(x);
                        y0 = y0.min(y);
                        x1 = x1.max(x + 1);
                        y1 = y1.max(y + 1);
                    }
                }
            }
            if x1 > x0 && y1 > y0 {
                painted_box = [x0, y0, x1, y1];
            }
            assert!(contains(pb, painted_box), "{name}: pixel_bounds {pb:?} missed CPU-painted pixels {painted_box:?}");
        }

        // Layer bounds (non-scope records only, as `prepare_layers` calls it).
        if !matches!(inst.kind(), Some(Kind::Layer | Kind::LayerEnd)) {
            let lb = inst.layer_bounds();
            let lb_px = [
                lb[0].max(0.0).floor() as i32,
                lb[1].max(0.0).floor() as i32,
                lb[2].min(w as f32).ceil() as i32,
                lb[3].min(h as f32).ceil() as i32,
            ];
            let target_exact = match inst.kind() {
                Some(Kind::Effect) => {
                    let g = inst.params[2] + 1.0;
                    [(inst.rect[0] - g).max(inst.clip[0]).max(0.0).floor() as i32,
                     (inst.rect[1] - g).max(inst.clip[1]).max(0.0).floor() as i32,
                     (inst.rect[0] + inst.rect[2] + g).min(inst.clip[2]).min(w as f32).ceil() as i32,
                     (inst.rect[1] + inst.rect[3] + g).min(inst.clip[3]).min(h as f32).ceil() as i32]
                }
                Some(Kind::BackdropBlur) => [
                    (inst.rect[0] - 1.0).max(inst.clip[0]).max(0.0).floor() as i32,
                    (inst.rect[1] - 1.0).max(inst.clip[1]).max(0.0).floor() as i32,
                    (inst.rect[0] + inst.rect[2] + 1.0).min(inst.clip[2]).min(w as f32).ceil() as i32,
                    (inst.rect[1] + inst.rect[3] + 1.0).min(inst.clip[3]).min(h as f32).ceil() as i32,
                ],
                _ => exact_vs,
            };
            assert!(contains(lb_px, target_exact), "{name}: layer_bounds {lb_px:?} missed exact {target_exact:?}");
            assert!(contains(lb_px, painted_box), "{name}: layer_bounds {lb_px:?} missed CPU-painted {painted_box:?}");

            // CPU boot's layer planner (`cpu_boot::bin::plan_layers`) must agree with
            // `Instance::layer_bounds` and contain every exact and painted pixel.
            let seed = [lb[0], lb[1], 0.0, 0.0];
            let stream = [
                Instance::begin_layer(seed, NO_CLIP, LayerSpec { opacity: 0.5, ..Default::default() }),
                *inst,
                Instance::end_layer(),
            ];
            let (plans, _) = cpu_boot::bin::plan_layers(&stream, (w as u32, h as u32));
            assert_eq!(plans.len(), 1, "{name}: one planned layer");
            assert_eq!(plans[0].bounds, lb_px, "{name}: cpu_boot plan_layers bounds match Instance::layer_bounds");
            assert!(contains(plans[0].bounds, target_exact), "{name}: cpu_boot plan_layers {:?} missed exact {target_exact:?}", plans[0].bounds);
            assert!(contains(plans[0].bounds, painted_box), "{name}: cpu_boot plan_layers {:?} missed CPU-painted {painted_box:?}", plans[0].bounds);

            let old_lb = old_layer_bounds(inst);
            let old_px = [
                old_lb[0].max(0.0).floor() as i32,
                old_lb[1].max(0.0).floor() as i32,
                old_lb[2].min(w as f32).ceil() as i32,
                old_lb[3].min(h as f32).ceil() as i32,
            ];
            if area_i32(lb_px) < area_i32(old_px) {
                assert!(contains(old_px, lb_px), "{name}: tighter layer_bounds must be subset of old");
                tighter_layer_sites.push(*name);
            }
        }
    }

    assert!(tighter_layer_sites.contains(&"rect_banded"), "expected tighter layer bounds on banded rect: {tighter_layer_sites:?}");
    assert!(tighter_layer_sites.contains(&"shadow_banded"), "expected tighter layer bounds on banded shadow: {tighter_layer_sites:?}");

    // A clipped-away child inside an isolated layer must not pull `plan_layers` bounds outward.
    let clipped_away = Instance { clip: [200.0, 200.0, 220.0, 220.0], ..base(Kind::Rect, [10.0, 10.0, 20.0, 20.0]) };
    let stream = [
        Instance::begin_layer([100.0, 100.0, 20.0, 20.0], NO_CLIP, LayerSpec { opacity: 0.5, ..Default::default() }),
        clipped_away,
        Instance::end_layer(),
    ];
    let (plans, _) = cpu_boot::bin::plan_layers(&stream, (w as u32, h as u32));
    assert_eq!(plans[0].bounds, [100, 100, 120, 120], "clipped-away child must not bloat layer bounds");
}

#[test]
fn from_f32_and_bounds_handle_edge_floats_and_nans() {
    let reference_from_f32 = |v: f32| -> Option<Kind> {
        if v.is_nan() || v.fract() != 0.0 || !(0.0..=13.0).contains(&v) {
            return None;
        }
        Kind::from_u8(v as u8)
    };

    // Every 1/8 step in [-2, 18], plus bit perturbations of every valid kind, subnormals, inf, NaN.
    let mut probe = vec![
        -0.0f32,
        0.0,
        f32::MIN_POSITIVE,
        f32::from_bits(1),
        f32::from_bits(0x8000_0001),
        11.5f32,
        f32::from_bits(11.5f32.to_bits() + 1),
        f32::from_bits(11.5f32.to_bits() - 1),
        f32::INFINITY,
        f32::NEG_INFINITY,
        f32::NAN,
        f32::from_bits(0x7fc0_0001),
        f32::from_bits(0xffc0_0001),
        f32::from_bits(0x7f80_0001),
        f32::MAX,
        f32::MIN,
    ];
    for step in -16..=144 {
        probe.push(step as f32 * 0.125);
    }
    for k in 0..Kind::COUNT {
        let bits = Kind::from_u8(k as u8).unwrap().as_f32().to_bits();
        probe.push(f32::from_bits(bits));
        probe.push(f32::from_bits(bits.wrapping_add(1)));
        probe.push(f32::from_bits(bits.wrapping_sub(1)));
        probe.push(f32::from_bits(bits | 0x8000_0000));
    }

    for v in probe {
        assert_eq!(Kind::from_f32(v), reference_from_f32(v), "from_f32({v:?}, bits={:#010x})", v.to_bits());
        let inst = Instance {
            rect: [20.0, 30.0, 40.0, 25.0],
            params: [0.0, v, 2.0, 1.0],
            clip: NO_CLIP,
            ..Instance::zeroed()
        };
        let qb = inst.quad_bounds();
        let expected_none = matches!(Kind::from_f32(v), Some(Kind::LayerEnd | Kind::Effect | Kind::BackdropBlur))
            || (Kind::from_f32(v).is_none() && v > 11.5);
        assert_eq!(qb.is_none(), expected_none, "quad_bounds none mismatch for {v:?}");
        if Kind::from_f32(v).is_none() {
            let lb = inst.layer_bounds();
            assert_eq!(lb, [19.0, 29.0, 61.0, 56.0], "layer_bounds fallback for {v:?}");
        }
    }

    // Kinds whose growth does not depend on `params[2]` ignore non-finite `params[2]`.
    for kind in [Kind::Glyph, Kind::Inset, Kind::Segment, Kind::Triangle, Kind::Image, Kind::Layer] {
        let inst = Instance {
            rect: [10.0, 12.0, 20.0, 15.0],
            params: [0.0, kind.as_f32(), f32::NAN, 1.0],
            clip: NO_CLIP,
            ..Instance::zeroed()
        };
        let b = inst.quad_bounds().expect("valid quad");
        assert!(b.iter().all(|v| v.is_finite()), "{kind:?} quad_bounds leaked NaN p2: {b:?}");
    }

    // Verify `pixel_bounds` matches the un-simplified 8-round reference across clip edge cases.
    let ref_pixel_bounds = |i: &Instance, w: i32, h: i32| -> [i32; 4] {
        let Some([mut x0, mut y0, mut x1, mut y1]) = i.quad_bounds() else { return [0; 4] };
        if i.kind() == Some(Kind::Glyph) && i.params[0] != 0.0 {
            x0 -= 1.0; y0 -= 1.0; x1 += 1.0; y1 += 1.0;
        }
        let first = |v: f32| (v - 0.5).ceil();
        let c = i.clip;
        let px0 = first(x0).max((c[0] - 0.5).floor()).max(0.0);
        let py0 = first(y0).max((c[1] - 0.5).floor()).max(0.0);
        let px1 = first(x1).min((c[2] + 0.5).ceil()).min(w as f32);
        let py1 = first(y1).min((c[3] + 0.5).ceil()).min(h as f32);
        if !(px1 > px0 && py1 > py0) { return [0; 4]; }
        [px0 as i32, py0 as i32, px1 as i32, py1 as i32]
    };

    let clips = [
        NO_CLIP,
        [0.0, 0.0, 256.0, 256.0],
        [0.4, 0.5, 100.2, 80.7],
        [15.2, 18.8, 45.6, 55.1],
        [40.0, 40.0, 40.0, 40.0],
        [-5.0, 0.51, 200.5, 120.49],
        [f32::NAN, 10.0, 100.0, f32::NAN],
    ];
    for clip in clips {
        for rect in [[10.3, 12.7, 30.0, 20.0], [0.2, 0.3, 10.0, 10.0], [35.0, 35.0, 50.0, 40.0], [f32::NAN, 10.0, 20.0, 20.0]] {
            for kind in [KIND_RECT, KIND_SHADOW, KIND_GLYPH, KIND_LAYER, KIND_LAYER_END, KIND_EFFECT, 14.0, -1.0] {
                let inst = Instance { rect, clip, params: [0.0, kind, 2.0, 1.0], ..Instance::zeroed() };
                assert_eq!(inst.pixel_bounds(256, 256), ref_pixel_bounds(&inst, 256, 256), "clip={clip:?} rect={rect:?} kind={kind}");
            }
        }
    }
}

#[test]
fn kind_runs_matches_reference_across_valid_none_and_out_of_bounds_spans() {
    let reference = |kinds: &[Option<Kind>], span: std::ops::Range<u32>| -> Vec<std::ops::Range<u32>> {
        let mut at = span.start;
        let mut out = Vec::new();
        while at < span.end {
            let kind = kinds.get(at as usize).copied();
            let start = at;
            while at < span.end && kinds.get(at as usize).copied() == kind { at += 1; }
            out.push(start..at);
        }
        out
    };
    let kinds = [
        Some(Kind::Rect), Some(Kind::Rect), Some(Kind::Glyph), None, None,
        Some(Kind::Shadow), Some(Kind::Rect), None,
    ];
    for span in [0..0, 0..1, 0..8, 2..6, 3..5, 6..12, 8..12, 10..10] {
        let got: Vec<_> = kind_runs(&kinds, span.clone()).collect();
        assert_eq!(got, reference(&kinds, span.clone()), "span {span:?}");
    }
}

#[test]
fn gpu_spring_opacity_folding_matches_cpu_across_all_kinds() {
    use crate::gpu::springs::{Node, Track};

    let (w, h) = (192u32, 128u32);
    let mut atlas = crate::text::Atlas::new();
    atlas.data.resize((crate::text::ATLAS_SIZE * crate::text::ATLAS_SIZE) as usize, 0);
    for y in 0..16usize {
        for x in 0..16usize {
            atlas.data[y * crate::text::ATLAS_SIZE as usize + x] = ((x * 15 + y * 9) & 0xff) as u8;
        }
    }
    atlas.dirty = Some((0, 16));

    let mut images = crate::pixels::ImageAtlas::default();
    let px = crate::Pixels::new();
    px.set(
        4,
        4,
        (0..16u8).flat_map(|i| [220, i.wrapping_mul(15), 80, 255]).collect(),
    );
    let placed = images.place(&px, [24.0, 24.0]).expect("place test image");

    // Non-overlapping grid covering every primitive kind a GPU spring can drive.
    let base_instances = vec![
        // (8..44, 8..40): KIND_RECT, opaque fill + opaque border, rounded (checks border_over hide + shrink)
        Instance {
            rect: [8.0, 8.0, 36.0, 32.0],
            radii: [8.0; 4],
            fill: [0.15, 0.35, 0.85, 1.0],
            border: [0.9, 0.12, 0.1, 1.0],
            params: [4.0, KIND_RECT, 0.0, 1.0],
            uv: [0.0; 4],
            clip: NO_CLIP,
            fade: [0; 4],
        },
        // (52..88, 8..40): KIND_RECT, translucent fill + translucent border (border.a = 0.6)
        Instance {
            rect: [52.0, 8.0, 36.0, 32.0],
            radii: [6.0; 4],
            fill: [0.12, 0.56, 0.24, 0.8],
            border: [0.54, 0.06, 0.48, 0.6],
            params: [4.0, KIND_RECT, 0.0, 1.0],
            uv: [0.0; 4],
            clip: NO_CLIP,
            fade: [0; 4],
        },
        // (96..132, 8..40): KIND_RECT, pre-dimmed (o = 0.6) with per-side border widths
        Instance {
            rect: [96.0, 8.0, 36.0, 32.0],
            radii: [4.0; 4],
            fill: [0.12, 0.24, 0.48, 0.6],
            border: [0.54, 0.42, 0.06, 0.6],
            params: [5.0, KIND_RECT, 0.0, 0.6],
            uv: [3.0, 5.0, 4.0, 2.0],
            clip: NO_CLIP,
            fade: [0; 4],
        },
        // (140..176, 8..40): KIND_GRADIENT with gradient border (bw = 4.0, border top -> uv bottom)
        Instance {
            rect: [140.0, 8.0, 36.0, 32.0],
            radii: [6.0; 4],
            fill: [0.1, 0.2, 0.7, 1.0],
            border: [0.95, 0.2, 0.1, 1.0],
            params: [4.0, KIND_GRADIENT, 0.0, 1.0],
            uv: [0.1, 0.85, 0.3, 1.0],
            clip: NO_CLIP,
            fade: [0; 4],
        },
        // (8..44, 48..80): KIND_GRADIENT fill-only (bw = 0.0, fill top -> uv bottom)
        Instance {
            rect: [8.0, 48.0, 36.0, 32.0],
            radii: [6.0; 4],
            fill: [0.8, 0.2, 0.1, 1.0],
            border: [0.0; 4],
            params: [0.0, KIND_GRADIENT, 0.0, 1.0],
            uv: [0.1, 0.3, 0.9, 1.0],
            clip: NO_CLIP,
            fade: [0; 4],
        },
        // (52..88, 48..80): KIND_SHADOW
        Instance {
            rect: [56.0, 52.0, 28.0, 24.0],
            radii: [6.0; 4],
            fill: [0.0, 0.0, 0.0, 0.6],
            border: [0.0; 4],
            params: [0.0, KIND_SHADOW, 4.0, 1.0],
            uv: [0.0; 4],
            clip: NO_CLIP,
            fade: [0; 4],
        },
        // (96..132, 48..80): KIND_INSET
        Instance {
            rect: [96.0, 48.0, 36.0, 32.0],
            radii: [6.0; 4],
            fill: [0.05, 0.05, 0.15, 0.7],
            border: [0.0; 4],
            params: [2.0, KIND_INSET, 3.0, 1.0],
            uv: [1.0, 2.0, 0.0, 0.0],
            clip: NO_CLIP,
            fade: [0; 4],
        },
        // (140..176, 48..80): KIND_DASHED
        Instance {
            rect: [140.0, 48.0, 36.0, 32.0],
            radii: [6.0; 4],
            fill: [0.9, 0.7, 0.1, 1.0],
            border: [0.2, 0.8, 0.9, 1.0],
            params: [3.0, KIND_DASHED, 0.0, 1.0],
            uv: [6.0, 4.0, 0.0, 0.0],
            clip: NO_CLIP,
            fade: [0; 4],
        },
        // (8..44, 88..120): KIND_RADIAL
        Instance {
            rect: [8.0, 88.0, 36.0, 32.0],
            radii: [6.0; 4],
            fill: [0.9, 0.3, 0.2, 1.0],
            border: [0.1, 0.4, 0.9, 0.8],
            params: [0.0, KIND_RADIAL, 0.0, 1.0],
            uv: [18.0, 16.0, 18.0, 16.0],
            clip: NO_CLIP,
            fade: [0; 4],
        },
        // (52..88, 88..120): KIND_SEGMENT + KIND_TRIANGLE
        Instance {
            rect: [52.0, 88.0, 36.0, 32.0],
            radii: [56.0, 92.0, 84.0, 114.0],
            fill: [0.95, 0.55, 0.15, 1.0],
            border: [0.0; 4],
            params: [2.5, KIND_SEGMENT, 1.0, 1.0],
            uv: [0.0; 4],
            clip: NO_CLIP,
            fade: [0; 4],
        },
        Instance {
            rect: [96.0, 88.0, 36.0, 32.0],
            radii: [100.0, 116.0, 114.0, 92.0],
            fill: [0.55, 0.3, 0.9, 0.9],
            border: [0.0; 4],
            params: [7.0, KIND_TRIANGLE, 0.0, 1.0],
            uv: [128.0, 116.0, 0.0, 0.0],
            clip: NO_CLIP,
            fade: [0; 4],
        },
        // (140..156, 92..108): KIND_GLYPH + (160..184, 92..116): KIND_IMAGE
        Instance {
            rect: [140.0, 92.0, 16.0, 16.0],
            radii: [0.0; 4],
            fill: [0.95, 0.95, 0.85, 1.0],
            border: [0.0; 4],
            params: [0.0, KIND_GLYPH, 0.0, 1.45],
            uv: [0.0, 0.0, 16.0, 16.0],
            clip: NO_CLIP,
            fade: [0; 4],
        },
        Instance {
            rect: [160.0, 92.0, 24.0, 24.0],
            radii: [4.0; 4],
            fill: [0.0, 0.0, 0.0, 0.9],
            border: [0.0; 4],
            params: [placed.page as f32, KIND_IMAGE, 0.0, 1.0],
            uv: placed.rect.map(|v| v as f32),
            clip: NO_CLIP,
            fade: [0; 4],
        },
    ];

    let Some(gpu) = Gpu::test_headless() else { return };
    let clear = [0.06, 0.08, 0.12, 1.0];

    for alpha in [0.5f32, 0.25] {
        // Canonical CPU opacity fold via `instance::end_layer`, applied to each primitive.
        let cpu_folded: Vec<Instance> = base_instances
            .iter()
            .map(|&inst| {
                let mut list = vec![
                    Instance::begin_layer(inst.rect, NO_CLIP, LayerSpec { opacity: alpha, ..Default::default() }),
                    inst,
                ];
                let mut marks = Vec::new();
                end_layer(&mut list, &mut [], &mut marks, 0, 0);
                assert_eq!(list.len(), 1, "single primitive must fold opacity directly");
                list[0]
            })
            .collect();

        for (format, srgb_blend) in [(OFFSCREEN_FORMAT_LINEAR, false), (wgpu::TextureFormat::Bgra8Unorm, true)] {
            atlas.dirty = Some((0, 16));
            let mut r_cpu = Renderer::new(&gpu.device, format);
            r_cpu.set_srgb_blend(srgb_blend);
            r_cpu.upload_images(&gpu.device, &gpu.queue, &mut images);
            r_cpu.prepare(&gpu.device, &gpu.queue, &mut atlas, &cpu_folded, (w, h));
            let t_cpu = Offscreen::with_format(&gpu.device, w, h, format);
            let mut enc = gpu.device.create_command_encoder(&Default::default());
            r_cpu.draw(&mut enc, &t_cpu.view, clear);
            t_cpu.copy(&mut enc);
            r_cpu.submit(&gpu.queue, enc);
            let px_cpu = t_cpu.read(&gpu.device);

            atlas.dirty = Some((0, 16));
            let zero = Track::constant(0.0);
            let node = Node { x: zero, y: zero, opacity: Track::constant(alpha) };
            let ids = vec![0u32; base_instances.len()];
            let mut r_spring = Renderer::with_springs(&gpu.device, format, &[node], &ids);
            r_spring.set_srgb_blend(srgb_blend);
            r_spring.upload_images(&gpu.device, &gpu.queue, &mut images);
            r_spring.prepare(&gpu.device, &gpu.queue, &mut atlas, &base_instances, (w, h));
            let t_spring = Offscreen::with_format(&gpu.device, w, h, format);
            let mut enc = gpu.device.create_command_encoder(&Default::default());
            r_spring.draw(&mut enc, &t_spring.view, clear);
            t_spring.copy(&mut enc);
            r_spring.submit(&gpu.queue, enc);
            let px_spring = t_spring.read(&gpu.device);

            let mut max_diff = 0u8;
            let mut worst = None;
            for y in 0..h {
                for x in 0..w {
                    let i = ((y * w + x) * 4) as usize;
                    let a = [px_cpu[i], px_cpu[i + 1], px_cpu[i + 2], px_cpu[i + 3]];
                    let b = [px_spring[i], px_spring[i + 1], px_spring[i + 2], px_spring[i + 3]];
                    let d = (0..4).map(|k| a[k].abs_diff(b[k])).max().unwrap();
                    if d > max_diff {
                        max_diff = d;
                        worst = Some((x, y, a, b));
                    }
                }
            }
            assert!(
                max_diff <= 1,
                "GPU spring opacity={alpha} format={format:?} drifted from CPU fold: max_diff={max_diff} worst={worst:?}"
            );

            if srgb_blend {
                let half = cpu_boot::HalfRounding::of_vendor(gpu.adapter.get_info().vendor);
                let scene = cpu_boot::Scene::new(&cpu_folded, &atlas, &images, (w, h), clear)
                    .srgb_blend(true)
                    .half_rounding(half);
                let mut boot = vec![0u8; (w * h * 4) as usize];
                cpu_boot::paint(&scene, &mut cpu_boot::Target::new(&mut boot, w, h), 1).expect("cpu_boot paints");
                let d = cpu_boot::drift(
                    cpu_boot::Pixels::bgra(&boot, w, h, w as usize * 4),
                    cpu_boot::Pixels::bgra(&px_spring, w, h, w as usize * 4),
                );
                assert!(d.max <= 1, "GPU spring opacity={alpha} drifted from cpu_boot: {d:?}");
            }
        }
    }
}

#[test]
fn shader_consts_and_clip_radii_invariants() {
    use super::instance::shader_consts as sc;

    // Generated Kind discriminants and uber threshold match Rust's Kind enum.
    assert_eq!(Kind::Rect.as_u8(), sc::KIND_RECT_U8);
    assert_eq!(Kind::Shadow.as_u8(), sc::KIND_SHADOW_U8);
    assert_eq!(Kind::Glyph.as_u8(), sc::KIND_GLYPH_U8);
    assert_eq!(Kind::Inset.as_u8(), sc::KIND_INSET_U8);
    assert_eq!(Kind::Gradient.as_u8(), sc::KIND_GRADIENT_U8);
    assert_eq!(Kind::Segment.as_u8(), sc::KIND_SEGMENT_U8);
    assert_eq!(Kind::Triangle.as_u8(), sc::KIND_TRIANGLE_U8);
    assert_eq!(Kind::Image.as_u8(), sc::KIND_IMAGE_U8);
    assert_eq!(Kind::Layer.as_u8(), sc::KIND_LAYER_U8);
    assert_eq!(Kind::LayerEnd.as_u8(), sc::KIND_LAYER_END_U8);
    assert_eq!(Kind::Dashed.as_u8(), sc::KIND_DASHED_U8);
    assert_eq!(Kind::Radial.as_u8(), sc::KIND_RADIAL_U8);
    assert!(Kind::Radial.as_f32() < sc::UBER_MAX_KIND_THRESHOLD);
    assert!(Kind::Effect.as_f32() > sc::UBER_MAX_KIND_THRESHOLD);
    assert!(Kind::BackdropBlur.as_f32() > sc::UBER_MAX_KIND_THRESHOLD);

    // Analytic checks on WGSL math constants mirrored into Rust.
    assert_eq!(CONTINUOUS_K.to_bits(), sc::CONTINUOUS_K.to_bits());
    assert_eq!(sc::CONTINUOUS_PI.to_bits(), std::f32::consts::PI.to_bits());
    let unit_g = |x: f32| (-0.5 * x * x).exp() / (2.0 * std::f32::consts::PI).sqrt();
    let expected_shadow_inside = 3.0 * (unit_g(0.75) + unit_g(2.25));
    assert!((sc::SHADOW_INSIDE - expected_shadow_inside).abs() < 1e-6);
    assert_eq!(sc::OUTLINE_TEX_WIDTH, crate::text::outline::WIDTH);
    assert_eq!(sc::IMAGE_FLAG_PREMULTIPLIED, 1);
    assert_eq!(sc::IMAGE_FLAG_TINT, 2);
    assert_eq!(sc::IMAGE_FLAG_NEAREST, 4);
    assert_eq!(sc::IMAGE_WRAP_EXTEND, 2);
    assert_eq!(sc::IMAGE_WRAP_REPEAT, 3);
    assert_eq!(sc::IMAGE_WRAP_MIRROR, 4);

    // `variant::LCD_MEAN_EPSILON` matches the threshold literal in `variant::DUAL_ENTRY`.
    let eps_start = variant::DUAL_ENTRY.find("mean < ").expect("DUAL_ENTRY mean threshold") + "mean < ".len();
    let eps_end = eps_start + variant::DUAL_ENTRY[eps_start..].find(')').expect("DUAL_ENTRY threshold paren");
    let dual_eps: f32 = variant::DUAL_ENTRY[eps_start..eps_end].trim().parse().expect("parse DUAL_ENTRY epsilon");
    assert_eq!(dual_eps.to_bits(), variant::LCD_MEAN_EPSILON.to_bits());

    // `pack_clip_radii` / `unpack_clip_radii` agree on fine, coarse, uniform and continuous corners.
    let big = [0.0, 0.0, 1000.0, 1000.0];
    let fine_max = sc::CLIP_RADII_LANE_MASK as f32 * sc::CLIP_RADII_FINE_STEP;
    let coarse_max = sc::CLIP_RADII_LANE_MASK as f32 * sc::CLIP_RADII_COARSE_STEP;
    for radii in [
        [0.0; 4],
        [12.0; 4],
        [-18.5; 4],
        [4.0, 8.5, 0.0, fine_max],
        [-4.0, -8.5, 0.0, -fine_max],
        [0.0, 64.0, 128.0, coarse_max],
        [-2.0, -66.0, 0.0, -coarse_max],
    ] {
        let packed = pack_clip_radii(big, radii);
        assert_eq!(unpack_clip_radii(packed), radii, "clip radii round-trip for {radii:?}");
    }

    // `crate::color::srgb_encode` agrees with `shader.wgsl`'s IEC 61966-2-1 transfer function.
    for byte in 0..=255u8 {
        let v = (byte as f32 / 255.0).clamp(0.0, 1.0);
        let wgsl = if v <= 0.0031308 { v * 12.92 } else { 1.055 * v.powf(1.0 / 2.4) - 0.055 };
        assert!((crate::color::srgb_encode(v) - wgsl).abs() < 1e-6, "srgb_encode mismatch at {byte}");
    }
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
fn wgsl_generated_constants_match_naga_ir() {
    use super::instance::shader_consts as sc;
    use wgpu::naga::{ArraySize, Binding, Expression, Handle, Literal, Module, TypeInner, UnaryOperator, front::wgsl};

    let eval_f32 = |m: &Module, mut h: Handle<Expression>| -> f32 {
        let mut neg = false;
        loop {
            match &m.global_expressions[h] {
                Expression::Unary { op: UnaryOperator::Negate, expr } => { neg = !neg; h = *expr; }
                Expression::Literal(Literal::F32(v)) => return if neg { -*v } else { *v },
                Expression::Literal(Literal::AbstractFloat(v)) => return if neg { -(*v as f32) } else { *v as f32 },
                other => panic!("unexpected f32 const expr: {other:?}"),
            }
        }
    };
    let eval_u32 = |m: &Module, h: Handle<Expression>| -> u32 {
        match &m.global_expressions[h] {
            Expression::Literal(Literal::U32(v)) => *v,
            Expression::Literal(Literal::AbstractInt(v)) => u32::try_from(*v).unwrap(),
            other => panic!("unexpected u32 const expr: {other:?}"),
        }
    };
    let eval_bool = |m: &Module, h: Handle<Expression>| -> bool {
        match &m.global_expressions[h] {
            Expression::Literal(Literal::Bool(v)) => *v,
            other => panic!("unexpected bool const expr: {other:?}"),
        }
    };
    let const_expr = |m: &Module, name: &str| {
        m.constants.iter().find(|(_, c)| c.name.as_deref() == Some(name))
            .unwrap_or_else(|| panic!("WGSL const `{name}` not found")).1.init
    };
    let override_expr = |m: &Module, name: &str| {
        m.overrides.iter().find(|(_, o)| o.name.as_deref() == Some(name))
            .unwrap_or_else(|| panic!("WGSL override `{name}` not found")).1.init
            .unwrap_or_else(|| panic!("WGSL override `{name}` has no default"))
    };

    let classic = wgsl::parse_str(uber_wgsl::classic()).expect("parse classic");
    assert_eq!(eval_f32(&classic, const_expr(&classic, "CONTINUOUS_K")).to_bits(), sc::CONTINUOUS_K.to_bits());
    assert_eq!(eval_f32(&classic, const_expr(&classic, "SHADOW_INSIDE")).to_bits(), sc::SHADOW_INSIDE.to_bits());
    assert_eq!(eval_f32(&classic, const_expr(&classic, "PINNED_KIND")).to_bits(), sc::PINNED_KIND.to_bits());

    let windfoil = wgsl::parse_str(uber_wgsl::windfoil()).expect("parse windfoil");
    assert_eq!(eval_f32(&windfoil, const_expr(&windfoil, "OUTLINE_HINT_REACH")).to_bits(), sc::OUTLINE_HINT_REACH.to_bits());
    assert_eq!(eval_bool(&windfoil, const_expr(&windfoil, "OUTLINE_SUBPIXEL")), sc::OUTLINE_SUBPIXEL);

    // Every pinned variant and the dual-source variant resolve their substituted constants in Naga.
    for &kind in &variant::PINNED {
        let src = variant::pinned_wgsl(kind).expect("pinned_wgsl");
        let m = wgsl::parse_str(&src).expect("parse pinned WGSL");
        assert_eq!(eval_f32(&m, const_expr(&m, "PINNED_KIND")), kind as f32);
    }
    let dual = wgsl::parse_str(&variant::dual_wgsl().expect("dual_wgsl")).expect("parse dual WGSL");
    assert!(eval_bool(&dual, const_expr(&dual, "OUTLINE_SUBPIXEL")));

    // `layer_blur.wgsl` overrides and `Uniforms.taps` array length.
    let lb = wgsl::parse_str(layer_blur::WGSL).expect("parse layer_blur.wgsl");
    assert_eq!(eval_u32(&lb, override_expr(&lb, sc::LAYER_BLUR_OVERRIDE_MODE)), sc::LAYER_BLUR_MODE_BLUR);
    assert!(!eval_bool(&lb, override_expr(&lb, sc::LAYER_BLUR_OVERRIDE_ADDITIVE)));
    let (_, uniforms) = lb.types.iter().find(|(_, t)| t.name.as_deref() == Some("Uniforms")).expect("Uniforms");
    let TypeInner::Struct { members, .. } = &uniforms.inner else { panic!("Uniforms is not a struct") };
    let taps = members.iter().find(|m| m.name.as_deref() == Some("taps")).expect("Uniforms.taps");
    let TypeInner::Array { size: ArraySize::Constant(n), .. } = lb.types[taps.ty].inner else { panic!("taps not fixed array") };
    assert_eq!(n.get() as usize, sc::LAYER_BLUR_TAPS);

    // Vertex input `@location(N)` bindings in Naga agree with `shader_consts` for uber and backdrop_composite.
    let check_locs = |label: &str, m: &Module, expected: &[(&str, u32)]| {
        let (_, ty) = m.types.iter().find(|(_, t)| t.name.as_deref() == Some("In")).unwrap_or_else(|| panic!("{label}: no In"));
        let TypeInner::Struct { members, .. } = &ty.inner else { panic!("{label}: In not struct") };
        for &(field, loc) in expected {
            let member = members.iter().find(|m| m.name.as_deref() == Some(field)).unwrap_or_else(|| panic!("{label}: In.{field}"));
            assert!(matches!(member.binding, Some(Binding::Location { location, .. }) if location == loc),
                "{label}: In.{field} expected @location({loc}), got {:?}", member.binding);
        }
    };
    check_locs("CLASSIC", &classic, &[
        ("rect", sc::ATTR_RECT_LOC), ("radii", sc::ATTR_RADII_LOC), ("fill", sc::ATTR_FILL_LOC), ("border", sc::ATTR_BORDER_LOC),
        ("params", sc::ATTR_PARAMS_LOC), ("uv", sc::ATTR_UV_LOC), ("clip", sc::ATTR_CLIP_LOC), ("fade", sc::ATTR_FADE_LOC),
    ]);
    let bc = wgsl::parse_str(&backdrop_blur::composite_wgsl()).expect("parse backdrop_composite.wgsl");
    check_locs("backdrop_composite", &bc, &[
        ("rect", sc::ATTR_RECT_LOC), ("radii", sc::ATTR_RADII_LOC), ("fill", sc::ATTR_FILL_LOC),
        ("clip", sc::ATTR_CLIP_LOC), ("fade", sc::ATTR_FADE_LOC),
    ]);
}

/// Bit-exact Rust mirror of `unpack_f16` / `unpack_f16x2` in `clip.wgsl` and `video.wgsl`, checked against
/// `half::f16::to_f32` across all 65,536 binary16 bit patterns (zeros, subnormals, normals,
/// infinities, and quiet/signaling NaNs).
#[test]
fn unpack_f16x2_matches_half_for_all_65536_inputs() {
    fn unpack_f16x2(packed: u32) -> [f32; 2] {
        [packed & 0xFFFF, packed >> 16].map(|h| {
            let sign = (h & 0x8000) << 16;
            let exp = (h >> 10) & 0x1F;
            let mant = h & 0x03FF;
            let sub = sign | ((mant as f32) * (1.0 / 16777216.0)).to_bits();
            let norm = sign | ((exp + 112) << 23) | (mant << 13);
            let inf_nan = sign | 0x7F80_0000 | (mant << 13) | if mant != 0 { 0x0040_0000 } else { 0 };
            let bits = if exp == 0 { sub } else if exp == 31 { inf_nan } else { norm };
            f32::from_bits(bits)
        })
    }
    for lo in 0..=u16::MAX {
        let hi = lo.wrapping_mul(257).wrapping_add(0x5A3Cu16);
        let [got_lo, got_hi] = unpack_f16x2(u32::from(lo) | (u32::from(hi) << 16));
        for (bits, got) in [(lo, got_lo), (hi, got_hi)] {
            let want = half::f16::from_bits(bits).to_f32();
            assert_eq!(got.to_bits(), want.to_bits(), "mismatch for f16 0x{bits:04x}");
        }
    }
}
