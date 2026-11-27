use crate::render::cpu_boot::{CpuPainter, NeedsGpu, Scene, Target};
use crate::{Color, Env, Runtime, el};
use super::*;

#[test]
fn dirty_rect_row_slice_clamps_bounds_and_computes_tight_row_range() {
    assert_eq!(
        dirty_rect_row_slice(2560, 1600, [0, 0, 2560, 1600]),
        Some(DirtyRectUpload {
            x0: 0,
            y0: 0,
            rw: 2560,
            rh: 1600,
            byte_start: 0,
            byte_end: 2560 * 1600 * 4,
        }),
    );
    assert_eq!(
        dirty_rect_row_slice(100, 80, [16, 10, 48, 30]),
        Some(DirtyRectUpload {
            x0: 16,
            y0: 10,
            rw: 32,
            rh: 20,
            byte_start: 10 * 100 * 4,
            byte_end: 30 * 100 * 4,
        }),
    );
    // Out-of-bounds end clamps to canvas extents.
    assert_eq!(
        dirty_rect_row_slice(64, 32, [32, 16, 128, 64]),
        Some(DirtyRectUpload {
            x0: 32,
            y0: 16,
            rw: 32,
            rh: 16,
            byte_start: 16 * 64 * 4,
            byte_end: 32 * 64 * 4,
        }),
    );
    // Empty or inverted rects return None.
    assert_eq!(dirty_rect_row_slice(64, 32, [10, 10, 10, 20]), None);
    assert_eq!(dirty_rect_row_slice(64, 32, [20, 10, 10, 20]), None);
    assert_eq!(dirty_rect_row_slice(64, 32, [64, 0, 80, 32]), None);
}

#[test]
fn classify_cpu_needs_gpu_degrades_all_degradable_and_holds_non_degradable() {
    for reason in [
        NeedsGpu::LinearBlend,
        NeedsGpu::NativeSurface,
        NeedsGpu::LayerBlur,
        NeedsGpu::TurnedLayer,
        NeedsGpu::BackdropBlur,
        NeedsGpu::Effect,
    ] {
        assert_eq!(
            classify_cpu_needs_gpu(reason),
            WebCpuNeedsGpuAction::PaintDegraded(reason),
        );
    }
    for reason in [
        NeedsGpu::SlugText,
        NeedsGpu::OutlineGlyph,
        NeedsGpu::UnknownKind,
        NeedsGpu::Target,
    ] {
        assert_eq!(
            classify_cpu_needs_gpu(reason),
            WebCpuNeedsGpuAction::HoldPreviousFrame(reason),
        );
    }
}

#[test]
fn needs_gpu_log_mask_reports_each_reason_once() {
    let mut mask = NeedsGpuLogMask::default();
    assert!(mask.note(NeedsGpu::LayerBlur));
    assert!(!mask.note(NeedsGpu::LayerBlur));
    assert!(mask.note(NeedsGpu::TurnedLayer));
    assert!(!mask.note(NeedsGpu::TurnedLayer));
    assert!(mask.note(NeedsGpu::SlugText));
    assert!(!mask.note(NeedsGpu::SlugText));
}

#[test]
fn handover_drift_and_pending_gpu_gating_respect_degraded_and_blocked_frames() {
    assert!(should_count_handover_drift(true, false));
    assert!(!should_count_handover_drift(true, true));
    assert!(!should_count_handover_drift(false, false));
    assert!(!should_count_handover_drift(false, true));

    // Normal CPU-first waits for frame 1 to present before taking GPU.
    assert!(!can_take_pending_gpu(false, false, true, true, false, true));
    assert!(can_take_pending_gpu(true, false, true, true, false, true));
    // Non-degradable NeedsGpu on frame 1 unblocks GPU takeover so the page does not deadlock blank.
    assert!(can_take_pending_gpu(false, true, true, true, false, true));
    // Failed CPU-only pages never take GPU unless WebGL2 is restoring.
    assert!(!can_take_pending_gpu(true, true, true, false, false, true));
    assert!(can_take_pending_gpu(true, false, true, false, true, true));
}

#[test]
fn degraded_fallback_paints_live_blurred_scene_without_freezing() {
    let mut rt = Runtime::new(
        0u8,
        |step, _| {
            let color = if *step == 0 {
                Color { r: 0.9, g: 0.2, b: 0.2, a: 1.0 }
            } else {
                Color { r: 0.2, g: 0.2, b: 0.9, a: 1.0 }
            };
            el().size(32.0, 32.0).fill(color).blur(6.0)
        },
        (32.0, 32.0),
        Env::new(true, 1.0),
    );
    let mut painter = CpuPainter::new(1);
    let mut buf = vec![0u8; 32 * 32 * 4];

    // Frame 1 (red blurred box): exact returns NeedsGpu::LayerBlur, degraded paints red live.
    rt.frame(0.0);
    let scene = Scene::new(&rt.instances, &rt.text.atlas, &rt.images, (32, 32), rt.clear_color());
    let err = painter.paint(&scene, &mut Target::new(&mut buf, 32, 32)).unwrap_err();
    assert_eq!(classify_cpu_needs_gpu(err), WebCpuNeedsGpuAction::PaintDegraded(NeedsGpu::LayerBlur));
    let painted1 = painter.paint(&scene.degraded(true), &mut Target::new(&mut buf, 32, 32)).expect("degraded frame 1");
    assert!(!painted1.rects.is_empty());
    let first_px = [buf[0], buf[1], buf[2], buf[3]];
    assert!(first_px[2] > first_px[0], "expected red-dominant BGRA pixel, got {first_px:?}");

    // Frame 2 (blue blurred box): must update pixels instead of freezing on frame 1.
    rt.state = 1;
    rt.invalidate();
    rt.settle_all();
    rt.frame(0.016);
    let scene2 = Scene::new(&rt.instances, &rt.text.atlas, &rt.images, (32, 32), rt.clear_color());
    let err2 = painter.paint(&scene2, &mut Target::new(&mut buf, 32, 32)).unwrap_err();
    assert_eq!(classify_cpu_needs_gpu(err2), WebCpuNeedsGpuAction::PaintDegraded(NeedsGpu::LayerBlur));
    let painted2 = painter.paint(&scene2.degraded(true), &mut Target::new(&mut buf, 32, 32)).expect("degraded frame 2");
    assert!(!painted2.rects.is_empty());
    let second_px = [buf[0], buf[1], buf[2], buf[3]];
    assert!(second_px[0] > second_px[2], "expected blue-dominant BGRA pixel, got {second_px:?}");
}
