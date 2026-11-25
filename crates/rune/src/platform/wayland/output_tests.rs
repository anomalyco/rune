use super::{OutputTransform, RawOutput, ResolvedOutput, resolve};
use crate::NativeDisplayId;
use crate::window::spec::native_bounds_for;

fn bounds(resolved: ResolvedOutput) -> [f64; 4] {
    native_bounds_for(
        Some(NativeDisplayId::Wayland(1)),
        resolved.position,
        resolved.size,
        resolved.scale,
    )
}

#[test]
fn issue_1700_fractional_125_and_rotated_outputs() {
    // 1836x996 at 1.25x: compositor advertises wl_output.scale = 2 and logical_size = 1469x797.
    // winit reports untransformed mode 1836x996, integer scale 2, and logical_position * 2.
    let winit_fallback = ResolvedOutput {
        position: (2938, 0),
        size: (1836, 996),
        scale: 2.0,
    };

    let normal = resolve(
        Some(&RawOutput {
            mode: Some((1836, 996)),
            transform: OutputTransform::Normal,
            geometry_position: Some((1469, 0)),
            integer_scale: Some(2),
            logical_position: Some((1469, 0)),
            logical_size: Some((1469, 797)),
        }),
        winit_fallback,
    );
    assert_eq!(normal.position, (1469, 0));
    assert_eq!(normal.size, (1836, 996));
    assert!((normal.scale - 1.25).abs() < 0.001, "scale={}", normal.scale);
    let b = bounds(normal);
    assert_eq!((b[0], b[1]), (1469.0, 0.0));
    assert_eq!(b[2].round() as i32, 1469);
    assert_eq!(b[3].round() as i32, 797);

    // Same panel rotated 90 and 270 degrees: logical_size becomes 797x1469, mode stays 1836x996.
    for transform in [
        OutputTransform::Rotate90,
        OutputTransform::Rotate270,
        OutputTransform::Flipped90,
        OutputTransform::Flipped270,
    ] {
        let rotated = resolve(
            Some(&RawOutput {
                mode: Some((1836, 996)),
                transform,
                geometry_position: Some((0, 0)),
                integer_scale: Some(2),
                logical_position: Some((-797, 120)),
                logical_size: Some((797, 1469)),
            }),
            winit_fallback,
        );
        assert_eq!(rotated.position, (-797, 120), "transform={transform:?}");
        assert_eq!(rotated.size, (996, 1836), "transform={transform:?}");
        assert_eq!(rotated.scale, normal.scale, "scale must be rotation-invariant");
        let rb = bounds(rotated);
        assert_eq!((rb[0], rb[1]), (-797.0, 120.0));
        assert_eq!(rb[2].round() as i32, 797);
        assert_eq!(rb[3].round() as i32, 1469);
    }
}

#[test]
fn table_scales_and_transforms_match_compositor_logical_bounds() {
    let all_transforms = [
        (OutputTransform::Normal, false),
        (OutputTransform::Rotate90, true),
        (OutputTransform::Rotate180, false),
        (OutputTransform::Rotate270, true),
        (OutputTransform::Flipped, false),
        (OutputTransform::Flipped90, true),
        (OutputTransform::Flipped180, false),
        (OutputTransform::Flipped270, true),
    ];

    // 3840x2160 divides cleanly by 1.0, 1.25, 1.5, and 2.0.
    let scales = [
        (1.0_f64, 1_i32, (3840_i32, 2160_i32)),
        (1.25_f64, 2_i32, (3072_i32, 1728_i32)),
        (1.5_f64, 2_i32, (2560_i32, 1440_i32)),
        (2.0_f64, 2_i32, (1920_i32, 1080_i32)),
    ];

    let logical_pos = (-1280, 360);

    for (expected_scale, integer_scale, (base_lw, base_lh)) in scales {
        for (transform, swapped) in all_transforms {
            let (expected_pw, expected_ph) = if swapped { (2160_u32, 3840_u32) } else { (3840_u32, 2160_u32) };
            let (lw, lh) = if swapped { (base_lh, base_lw) } else { (base_lw, base_lh) };

            let winit_fallback = ResolvedOutput {
                position: (logical_pos.0 * integer_scale, logical_pos.1 * integer_scale),
                size: (3840, 2160),
                scale: f64::from(integer_scale),
            };

            let resolved = resolve(
                Some(&RawOutput {
                    mode: Some((3840, 2160)),
                    transform,
                    geometry_position: Some((0, 0)),
                    integer_scale: Some(integer_scale),
                    logical_position: Some(logical_pos),
                    logical_size: Some((lw, lh)),
                }),
                winit_fallback,
            );

            assert_eq!(resolved.position, logical_pos, "scale={expected_scale} transform={transform:?}");
            assert_eq!(resolved.size, (expected_pw, expected_ph), "scale={expected_scale} transform={transform:?}");
            assert!((resolved.scale - expected_scale).abs() < 1e-12, "scale={expected_scale} transform={transform:?} got {}", resolved.scale);

            let b = bounds(resolved);
            assert_eq!(b, [f64::from(logical_pos.0), f64::from(logical_pos.1), f64::from(lw), f64::from(lh)]);
        }
    }
}

#[test]
fn physical_layout_mode_keeps_wl_output_integer_scale() {
    // Mutter without `scale-monitor-framebuffer` reports logical_size == physical_size alongside
    // wl_output.scale = 2. Keep scale = 2.0 instead of collapsing to 1.0.
    let fallback = ResolvedOutput { position: (0, 0), size: (3840, 2160), scale: 2.0 };
    let normal = resolve(
        Some(&RawOutput {
            mode: Some((3840, 2160)),
            transform: OutputTransform::Normal,
            geometry_position: Some((0, 0)),
            integer_scale: Some(2),
            logical_position: Some((0, 0)),
            logical_size: Some((3840, 2160)),
        }),
        fallback,
    );
    assert_eq!(normal.size, (3840, 2160));
    assert_eq!(normal.scale, 2.0);
    assert_eq!(bounds(normal), [0.0, 0.0, 1920.0, 1080.0]);

    let rotated = resolve(
        Some(&RawOutput {
            mode: Some((3840, 2160)),
            transform: OutputTransform::Rotate90,
            geometry_position: Some((0, 0)),
            integer_scale: Some(2),
            logical_position: Some((0, 0)),
            logical_size: Some((2160, 3840)),
        }),
        fallback,
    );
    assert_eq!(rotated.size, (2160, 3840));
    assert_eq!(rotated.scale, 2.0);
    assert_eq!(bounds(rotated), [0.0, 0.0, 1080.0, 1920.0]);
}

#[test]
fn fallbacks_without_xdg_output_or_partial_events() {
    let winit_fallback = ResolvedOutput { position: (3840, 0), size: (1920, 1080), scale: 2.0 };

    // Without zxdg_output_manager_v1, keep winit's values unchanged.
    assert_eq!(resolve(None, winit_fallback), winit_fallback);

    // Without logical_position, fall back to wl_output.geometry position.
    let geom_only = resolve(
        Some(&RawOutput {
            mode: Some((1920, 1080)),
            transform: OutputTransform::Rotate270,
            geometry_position: Some((1920, 0)),
            integer_scale: Some(2),
            logical_position: None,
            logical_size: None,
        }),
        winit_fallback,
    );
    assert_eq!(geom_only, ResolvedOutput { position: (1920, 0), size: (1080, 1920), scale: 2.0 });

    // Non-positive mode or logical_size falls back safely without NaN or zero scale.
    let invalid = resolve(
        Some(&RawOutput {
            mode: Some((0, -1)),
            transform: OutputTransform::Normal,
            geometry_position: None,
            integer_scale: Some(0),
            logical_position: Some((100, 200)),
            logical_size: Some((0, 0)),
        }),
        winit_fallback,
    );
    assert_eq!(invalid, ResolvedOutput { position: (100, 200), size: (1920, 1080), scale: 2.0 });

    // Compositor zoom-out (scale < 1, e.g. 0.75 on wlroots/Sway) preserves fractional scale.
    let zoomed_out = resolve(
        Some(&RawOutput {
            mode: Some((1920, 1080)),
            transform: OutputTransform::Normal,
            geometry_position: Some((0, 0)),
            integer_scale: Some(1),
            logical_position: Some((0, 0)),
            logical_size: Some((2560, 1440)),
        }),
        ResolvedOutput { position: (0, 0), size: (1920, 1080), scale: 1.0 },
    );
    assert_eq!(zoomed_out.scale, 0.75);
    assert_eq!(bounds(zoomed_out), [0.0, 0.0, 2560.0, 1440.0]);
}

#[test]
fn multi_monitor_native_bounds_are_contiguous() {
    // Left: 2560x1440 @ 1.5x (logical 1707x960 at x = -1707)
    // Center: 3840x2160 @ 2.0x (logical 1920x1080 at x = 0)
    // Right: 1836x996 rotated 90 deg @ 1.25x (logical 797x1469 at x = 1920)
    let left = resolve(
        Some(&RawOutput {
            mode: Some((2560, 1440)),
            transform: OutputTransform::Normal,
            geometry_position: None,
            integer_scale: Some(2),
            logical_position: Some((-1707, 0)),
            logical_size: Some((1707, 960)),
        }),
        ResolvedOutput { position: (-3414, 0), size: (2560, 1440), scale: 2.0 },
    );
    let center = resolve(
        Some(&RawOutput {
            mode: Some((3840, 2160)),
            transform: OutputTransform::Normal,
            geometry_position: None,
            integer_scale: Some(2),
            logical_position: Some((0, 0)),
            logical_size: Some((1920, 1080)),
        }),
        ResolvedOutput { position: (0, 0), size: (3840, 2160), scale: 2.0 },
    );
    let right = resolve(
        Some(&RawOutput {
            mode: Some((1836, 996)),
            transform: OutputTransform::Rotate90,
            geometry_position: None,
            integer_scale: Some(2),
            logical_position: Some((1920, 0)),
            logical_size: Some((797, 1469)),
        }),
        ResolvedOutput { position: (3840, 0), size: (1836, 996), scale: 2.0 },
    );

    let [lx, _, lw, _] = bounds(left);
    let [cx, _, cw, _] = bounds(center);
    let [rx, _, rw, rh] = bounds(right);
    assert_eq!((lx + lw).round(), cx);
    assert_eq!((cx + cw).round(), rx);
    assert_eq!((rw.round(), rh.round()), (797.0, 1469.0));
}
