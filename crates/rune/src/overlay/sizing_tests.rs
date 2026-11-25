use std::cell::Cell;
use std::rc::Rc;

use super::*;
use crate::color::Color;
use crate::el::{Align, col, el, text};
use crate::overlay::options::{ConfiguredOverlay, OverlayOptions};
use crate::overlay::{OverlaySpec, Placement, Presence, ScreenEdge};
use crate::runtime::{Cx, Env, Runtime};
use crate::{El, Pixels};

#[test]
fn content_sizing_defaults_to_fixed_and_fit_content_height_matches_fit_height() {
    let default_cfg: ConfiguredOverlay<()> = OverlaySpec::new("hud", (200.0, 80.0), |_: &(), _| el()).into();
    assert_eq!(default_cfg.sizing, ContentSizing::Fixed);
    assert!(default_cfg.sizing.is_fixed());
    assert!(!default_cfg.sizing.fits_any());
    assert_eq!(default_cfg.min_size, None);
    assert_eq!(default_cfg.max_size, None);

    let shorthand = OverlaySpec::new("h", (200.0, 0.0), |_: &(), _| el())
        .options(OverlayOptions::default())
        .fit_content_height();
    let explicit = OverlaySpec::new("h", (200.0, 0.0), |_: &(), _| el())
        .options(OverlayOptions::default())
        .sizing(ContentSizing::FitHeight);
    assert_eq!(shorthand.sizing, ContentSizing::FitHeight);
    assert_eq!(shorthand.sizing, explicit.sizing);
    assert_eq!(shorthand.min_size, explicit.min_size);
    assert_eq!(shorthand.max_size, explicit.max_size);

    let fit_cfg = OverlaySpec::new("candidates", (0.0, 0.0), |_: &(), _| el())
        .options(OverlayOptions::default())
        .sizing(ContentSizing::Fit)
        .min_size((120.0, 40.0))
        .max_size((420.0, 320.0));
    assert_eq!(fit_cfg.sizing, ContentSizing::Fit);
    assert!(fit_cfg.sizing.fits_width() && fit_cfg.sizing.fits_height());
    assert_eq!(fit_cfg.min_size, Some((120.0, 40.0)));
    assert_eq!(fit_cfg.max_size, Some((420.0, 320.0)));

    assert!(ContentSizing::FitHeight.fits_height() && !ContentSizing::FitHeight.fits_width());
    assert!(ContentSizing::FitWidth.fits_width() && !ContentSizing::FitWidth.fits_height());
}

#[test]
fn ceil_logical_to_pixels_rounds_up_to_whole_physical_pixels_across_scales() {
    for scale in [1.0f32, 1.25, 1.5, 2.0, 3.0] {
        let pts = ceil_logical_to_pixels(100.1, scale);
        let px = pts * scale;
        assert!((px - px.round()).abs() < 1e-4, "scale {scale}: {pts} pt -> {px} px not whole");
        assert!(pts >= 100.1 - 1e-5, "scale {scale}: must round up, got {pts}");
    }
    assert_eq!(ceil_logical_to_pixels(0.0, 2.0), 0.0);
    assert_eq!(ceil_logical_to_pixels(-10.0, 1.5), 0.0);
    assert_eq!(ceil_logical_to_pixels(f32::NAN, 1.5), 0.0);
}

#[test]
fn bounds_clamping_handles_zero_min_gt_max_work_area_and_placements() {
    let fit = SizingConfig { mode: ContentSizing::Fit, min_size: None, max_size: Some((420.0, 320.0)) };

    let zero = resolve_content_size(fit, None, (0.0, 0.0), (0.0, 0.0), Some((1920.0, 1080.0)), 1.0, 8192);
    assert_eq!(zero, (1.0, 1.0));

    let capped = resolve_content_size(fit, None, (0.0, 0.0), (900.0, 700.0), Some((1920.0, 1080.0)), 1.0, 8192);
    assert_eq!(capped, (420.0, 320.0));

    let wa = resolve_content_size(
        SizingConfig { mode: ContentSizing::Fit, min_size: None, max_size: Some((2000.0, 2000.0)) },
        None,
        (0.0, 0.0),
        (1500.0, 1200.0),
        Some((1280.0, 720.0)),
        1.0,
        8192,
    );
    assert_eq!(wa, (1280.0, 720.0));

    let inverted = resolve_content_size(
        SizingConfig { mode: ContentSizing::Fit, min_size: Some((500.0, 400.0)), max_size: Some((300.0, 200.0)) },
        None,
        (0.0, 0.0),
        (50.0, 50.0),
        Some((1920.0, 1080.0)),
        1.0,
        8192,
    );
    assert_eq!(inverted, (300.0, 200.0));

    let full = resolve_content_size(
        fit,
        Some(OverlayPlacement::FullScreen),
        (100.0, 50.0),
        (40.0, 20.0),
        Some((1440.0, 900.0)),
        2.0,
        8192,
    );
    assert_eq!(full, (1440.0, 900.0));

    let at = Placement { size: (180.0, 96.0), edge: ScreenEdge::Top, margin: 0.0, align: Align::Center, inset: 0.0 };
    let rect_place = Some(OverlayPlacement::Rect { x: 40.0, y: 60.0, width: 300.0, height: 200.0 });
    assert_eq!(local_rect_for(at, rect_place, (1920.0, 1080.0), ContentSizing::Fixed), [40.0, 60.0, 300.0, 200.0]);
    assert_eq!(local_rect_for(at, rect_place, (1920.0, 1080.0), ContentSizing::Fit), [40.0, 60.0, 180.0, 96.0]);
    assert_eq!(local_rect_for(at, rect_place, (1920.0, 1080.0), ContentSizing::FitHeight), [40.0, 60.0, 300.0, 96.0]);
    assert_eq!(local_rect_for(at, rect_place, (1920.0, 1080.0), ContentSizing::FitWidth), [40.0, 60.0, 180.0, 200.0]);
}

#[test]
fn fit_width_mode_fits_width_and_preserves_fixed_height() {
    fn bar(items: &usize, _: &mut Cx<usize>) -> El {
        crate::row().gap(4.0).padding(6.0).children((0..*items).map(|i| el().key(format!("c{i}")).size(50.0, 20.0)))
    }
    let spec = crate::tooling::AppSpec::new("host", (400.0, 300.0), || (), |_, _| el());
    let (mut overlay, _) = crate::overlay::app::Overlay::new(
        OverlaySpec::new("bar", (0.0, 44.0), bar)
            .options(OverlayOptions::default())
            .sizing(ContentSizing::FitWidth)
            .min_size((80.0, 44.0))
            .max_size((400.0, 44.0)),
        3usize,
        &spec,
    );
    overlay.rt.wall_start = None;
    overlay.fit_for_show();
    // 3 * 50 + 2 * 4 + 2 * 6 = 170 pt width, fixed 44 pt height.
    assert_eq!(overlay.canvas_size, (170.0, 44.0));
    // OverlayHandle::resize sets only the fixed height; width stays fitted.
    assert_eq!(overlay.requested_size(overlay.rt.core(), Some((999.0, 60.0)), None), Some((170.0, 60.0)));
}

#[test]
fn max_size_wraps_text_and_shadow_margin_is_included_in_measured_size() {
    #[allow(clippy::ptr_arg)]
    fn view(items: &Vec<String>, _: &mut Cx<Vec<String>>) -> El {
        col().padding(12.0).child(
            col()
                .padding(8.0)
                .gap(4.0)
                .fill(Color { r: 0.1, g: 0.12, b: 0.16, a: 0.95 })
                .children(items.iter().map(|s| text(s.clone()).wrap())),
        )
    }

    let cfg = SizingConfig { mode: ContentSizing::Fit, min_size: None, max_size: Some((180.0, 260.0)) };
    let mut rt = Runtime::new(vec!["Short".to_owned()], view, (0.0, 0.0), Env::new(true, 1.0));
    rt.core_mut().host.native_overlay = true;
    rt.core_mut().host.content_sizing = cfg.mode;
    rt.core_mut().host.content_limits = layout_limits(cfg, Some((1920.0, 1080.0)), 1.0, 8192);

    let short = rt.measure_content();
    let short_resolved = resolve_content_size(cfg, None, (0.0, 0.0), short, Some((1920.0, 1080.0)), 1.0, 8192);
    assert!(short_resolved.0 > 40.0 && short_resolved.0 < 180.0, "short text shrinks below max_w: {short_resolved:?}");
    assert!(short_resolved.1 >= 40.0, "12pt shadow margin + 8pt padding included: {short_resolved:?}");

    rt.state = vec!["This candidate entry is intentionally very long so that it must wrap across several lines inside max_size".to_owned()];
    let long = rt.measure_content();
    let long_resolved = resolve_content_size(cfg, None, (0.0, 0.0), long, Some((1920.0, 1080.0)), 1.0, 8192);
    assert!(long_resolved.0 <= 180.0, "must wrap within max_size.0: {long_resolved:?}");
    assert!(long_resolved.1 > short_resolved.1 + 16.0, "wrapped text grows vertically: short={short_resolved:?}, long={long_resolved:?}");
}

#[test]
fn pre_show_measure_runs_zero_side_effects_and_never_doubles_callbacks() {
    #[derive(Default)]
    struct Counts {
        layouts: Rc<Cell<u32>>,
        polls: Rc<Cell<u32>>,
        materials: Rc<Cell<u32>>,
    }

    let counts = Counts::default();
    let layout_hits = counts.layouts.clone();
    let poll_hits = counts.polls.clone();
    let mat_hits = counts.materials.clone();

    let spec = crate::tooling::AppSpec::new("host", (400.0, 300.0), || (), |_, _| el());
    let (mut overlay, _handle) = crate::overlay::app::Overlay::new(
        OverlaySpec::new("candidates", (0.0, 0.0), |s: &Counts, _| {
            let hits = s.layouts.clone();
            col()
                .key("card")
                .padding(8.0)
                .system_material(crate::SystemMaterial::Popover)
                .on_layout(move |_: &mut Counts, _| hits.set(hits.get() + 1))
                .child(text("Candidate one"))
        })
        .options(OverlayOptions::default())
        .sizing(ContentSizing::Fit)
        .poll(|s: &mut Counts, _, tx| {
            s.polls.set(s.polls.get() + 1);
            tx.wake_after(std::time::Duration::from_millis(50));
            false
        })
        .on_materials(move |_| mat_hits.set(mat_hits.get() + 1)),
        Counts { layouts: layout_hits.clone(), polls: poll_hits.clone(), materials: counts.materials.clone() },
        &spec,
    );

    overlay.fit_for_show();
    assert!(overlay.canvas_size.0 > 40.0 && overlay.canvas_size.1 > 16.0);
    assert_eq!(layout_hits.get(), 0, "pre-show fit must not fire on_layout");
    assert_eq!(poll_hits.get(), 0, "pre-show fit must not run poll");
    assert_eq!(counts.materials.get(), 0, "pre-show fit must not fire on_materials");
    assert_eq!(overlay.rt.core().stats.commits, 0, "pre-show fit must not commit");
    assert!(overlay.rt.core().wake_deadline().is_none(), "pre-show fit must not arm timers");

    overlay.rt.frame(0.0);
    assert_eq!(overlay.rt.core().stats.commits, 1);
    assert_eq!(poll_hits.get(), 1, "poll runs once on real frame");
    assert!(overlay.rt.core().wake_deadline().is_some(), "timer arms on real frame");
    overlay.rt.frame(0.0);
    assert_eq!(layout_hits.get(), 1, "on_layout fires once for the real commit");
}

#[test]
fn headless_overlay_host_seam_proves_zero_flash_first_show_live_resize_prewarm_and_springs() {
    #[derive(Debug, PartialEq)]
    enum HostOp {
        CreateSurface((f32, f32)),
        ResizeAndPlace((f32, f32), (f32, f32)),
        Present((f32, f32)),
    }

    struct State {
        rows: usize,
        label: String,
        animated_pad: f32,
    }

    fn view(s: &State, _: &mut Cx<State>) -> El {
        col()
            .key("root")
            .padding(8.0)
            .gap(4.0)
            .children((0..s.rows).map(|i| {
                el().key(format!("row-{i}"))
                    .h(24.0)
                    .padding(s.animated_pad)
                    .motion(crate::tok::motion::DEFAULT)
                    .child(text(format!("{} #{i}", s.label)))
            }))
    }

    let spec = crate::tooling::AppSpec::new("host", (400.0, 300.0), || (), |_, _| el());
    let (mut overlay, handle) = crate::overlay::app::Overlay::new(
        OverlaySpec {
            edge: ScreenEdge::Bottom,
            align: Align::End,
            margin: 16.0,
            inset: 16.0,
            ..OverlaySpec::new("candidates", (0.0, 0.0), view)
        }
        .options(OverlayOptions { prewarm: true, ..Default::default() })
        .sizing(ContentSizing::Fit)
        .max_size((420.0, 320.0)),
        State { rows: 2, label: "Item".into(), animated_pad: 0.0 },
        &spec,
    );
    overlay.rt.wall_start = None;
    let work_area = (1920.0f32, 1080.0f32);
    let scale = 1.5f32;
    overlay.core.last_work_area = Some(work_area);
    overlay.rt.set_viewport((0.0, 0.0), scale);
    let mut ops: Vec<HostOp> = Vec::new();

    let anchor_origin = |at: Placement| -> (f32, f32) {
        let r = local_rect_for(at, None, work_area, ContentSizing::Fit);
        (r[0], r[1])
    };

    // 1. Prewarm measures initial 2-row content BEFORE creating the surface (never (0, 0)).
    overlay.fit_for_show();
    let prewarm_size = overlay.core.at.size;
    ops.push(HostOp::CreateSurface(prewarm_size));
    assert!(prewarm_size.0 > 40.0 && prewarm_size.1 >= 60.0, "prewarm surface sized to content: {prewarm_size:?}");

    // 2. Hidden update adds 3 more rows before first show; show re-measures and resizes BEFORE first present.
    handle.update(|s, _| {
        s.rows = 5;
        s.label = "Longer candidate entry".into();
    });
    let updates = std::mem::take(&mut *overlay.shared.updates.lock().unwrap());
    overlay.apply_wake(false, updates);
    overlay.fit_for_show();
    let show_size = overlay.core.at.size;
    assert!(show_size.0 > prewarm_size.0 && show_size.1 > prewarm_size.1);
    ops.push(HostOp::ResizeAndPlace(anchor_origin(overlay.core.at), show_size));
    overlay.rt.settle_all();
    overlay.rt.frame(0.0);
    overlay.presence = Presence::Shown { occluded: false };
    assert!(overlay.core.take_live_fit_resize(overlay.rt.core_mut()).is_none(), "first show already sized");
    ops.push(HostOp::Present(overlay.core.at.size));

    let presents: Vec<_> = ops.iter().filter(|op| matches!(op, HostOp::Present(_))).collect();
    assert_eq!(presents, vec![&HostOp::Present(show_size)]);

    // 3. Live shrink while shown (5 rows -> 1 row) resizes + re-places strictly BEFORE present in same frame.
    ops.clear();
    overlay.rt.update(|s, _| {
        s.rows = 1;
        s.label = "Hi".into();
    });
    let _ = overlay.rt.frame(1.0 / 60.0);
    let shrunk = overlay.core.take_live_fit_resize(overlay.rt.core_mut()).expect("shrink triggers same-frame resize");
    assert!(shrunk.0 < show_size.0 && shrunk.1 < show_size.1);
    overlay.core.at.size = shrunk;
    overlay.rt.set_viewport(shrunk, scale);
    assert!(!overlay.rt.core().stale(), "fitted viewport update must not trigger relayout loop");
    let new_origin = anchor_origin(overlay.core.at);
    assert!((new_origin.0 + shrunk.0 - (work_area.0 - 16.0)).abs() < 1e-3);
    assert!((new_origin.1 + shrunk.1 - (work_area.1 - 16.0)).abs() < 1e-3);
    ops.push(HostOp::ResizeAndPlace(new_origin, shrunk));
    ops.push(HostOp::Present(shrunk));
    assert_eq!(ops, vec![HostOp::ResizeAndPlace(new_origin, shrunk), HostOp::Present(shrunk)]);

    // 4. Child spring animation: triggers 1 resize on commit to target root.local.(w, h), and 0 resizes during 20 spring frames.
    overlay.rt.update(|s, _| s.animated_pad = 10.0);
    let mut animating = overlay.rt.frame(1.0 / 60.0);
    let target = overlay.core.take_live_fit_resize(overlay.rt.core_mut()).expect("commit resizes once to spring target");
    overlay.core.at.size = target;
    overlay.rt.set_viewport(target, scale);
    let mut resizes_during_spring = 0usize;
    for _ in 0..20 {
        if !animating { break; }
        animating = overlay.rt.frame(1.0 / 60.0);
        if overlay.core.take_live_fit_resize(overlay.rt.core_mut()).is_some() {
            resizes_during_spring += 1;
        }
    }
    assert_eq!(resizes_during_spring, 0, "spring animation frames must trigger zero surface reconfigures");
}

#[test]
fn late_image_decode_and_dpi_scale_change_resize_fitted_overlay_cleanly() {
    let sprite = Pixels::new();
    fn view(p: &Pixels, _: &mut Cx<Pixels>) -> El {
        el().padding(8.0).child(el().w(120.0).pixels(p))
    }
    let cfg = SizingConfig { mode: ContentSizing::Fit, min_size: None, max_size: Some((400.0, 300.0)) };
    let mut rt = Runtime::new(sprite.clone(), view, (0.0, 0.0), Env::new(true, 1.0));
    rt.core_mut().host.native_overlay = true;
    rt.core_mut().host.content_sizing = cfg.mode;
    rt.core_mut().host.content_limits = layout_limits(cfg, Some((1920.0, 1080.0)), 1.0, 8192);
    rt.frame(0.0);
    let before = resolve_content_size(cfg, None, (0.0, 0.0), rt.core().committed_content_size().unwrap(), Some((1920.0, 1080.0)), 1.0, 8192);

    sprite.set(120, 60, vec![255u8; 120 * 60 * 4]);
    rt.invalidate();
    rt.frame(0.0);
    let after = resolve_content_size(cfg, None, (0.0, 0.0), rt.core().committed_content_size().unwrap(), Some((1920.0, 1080.0)), 1.0, 8192);
    assert!(after.1 > before.1 + 40.0, "late image decode must grow fitted overlay: before={before:?}, after={after:?}");

    for scale in [1.25f32, 1.5, 2.0, 3.0] {
        rt.core_mut().host.content_limits = layout_limits(cfg, Some((1920.0, 1080.0)), scale, 8192);
        rt.set_viewport(after, scale);
        rt.frame(0.0);
        let sized = resolve_content_size(cfg, None, (0.0, 0.0), rt.core().committed_content_size().unwrap(), Some((1920.0, 1080.0)), scale, 8192);
        let px_w = sized.0 * scale;
        let px_h = sized.1 * scale;
        assert!((px_w - px_w.round()).abs() < 1e-3 && (px_h - px_h.round()).abs() < 1e-3);
    }
}

#[test]
fn cpu_layered_engine_atomically_resizes_and_shrinks_fitted_overlay_dib() {
    #[cfg(target_os = "windows")]
    use crate::platform::win32::layered::{DibHeaderSpec, LayeredSeam, LayeredUpdate, ULW_ALPHA, bounded_layered_size};
    #[cfg(all(test, not(target_os = "windows")))]
    use crate::platform::win32_layered::{DibHeaderSpec, LayeredSeam, LayeredUpdate, ULW_ALPHA, bounded_layered_size};

    #[derive(Default)]
    struct Seam { created: Vec<DibHeaderSpec>, updates: Vec<LayeredUpdate> }
    struct Surf { w: u32, h: u32, px: Vec<u8> }
    impl LayeredSeam for Seam {
        type Surface = Surf;
        fn create_surface(&mut self, header: DibHeaderSpec) -> Result<Self::Surface, String> {
            self.created.push(header);
            let (w, h) = header.dimensions();
            Ok(Surf { w, h, px: vec![0u8; w as usize * h as usize * 4] })
        }
        fn surface_pixels<'a>(&self, s: &'a mut Self::Surface) -> (&'a mut [u8], usize) { let st = s.w as usize * 4; (&mut s.px, st) }
        fn surface_size(&self, s: &Self::Surface) -> (u32, u32) { (s.w, s.h) }
        fn update_layered(&mut self, _: isize, _: &Self::Surface, u: LayeredUpdate) -> Result<(), String> { self.updates.push(u); Ok(()) }
    }

    fn view(big: &bool, _: &mut Cx<bool>) -> El {
        let (w, h) = if *big { (800.0, 600.0) } else { (120.0, 40.0) };
        el().size(w, h).fill(Color { r: 0.15, g: 0.2, b: 0.3, a: 0.9 })
    }
    let cfg = SizingConfig { mode: ContentSizing::Fit, min_size: None, max_size: Some((1000.0, 800.0)) };
    let mut rt = Runtime::new(true, view, (0.0, 0.0), Env::new(true, 1.0));
    rt.core_mut().transparent = true;
    rt.core_mut().host.native_overlay = true;
    rt.core_mut().host.content_sizing = cfg.mode;
    rt.core_mut().host.content_limits = layout_limits(cfg, Some((1920.0, 1080.0)), 1.0, 8192);
    let mut engine = super::super::cpu::CpuPaintEngine::new(Seam::default(), 1);

    let m1 = rt.measure_content();
    let s1 = resolve_content_size(cfg, None, (0.0, 0.0), m1, Some((1920.0, 1080.0)), 1.0, 8192);
    assert_eq!(s1, (800.0, 600.0));
    rt.set_viewport(s1, 1.0);
    rt.frame(0.0);
    let px1 = bounded_layered_size(s1, 1.0);
    engine.paint_and_present("cand", 1, (100, 100), px1, 255, rt.core_mut());
    assert_eq!(engine.presenter().seam().created.len(), 1);
    assert_eq!(engine.presenter().seam().updates[0].size, (800, 600));
    assert_eq!(engine.presenter().seam().updates[0].flags, ULW_ALPHA);
    assert_eq!(engine.presenter().seam().updates[0].dirty, None);

    rt.state = false;
    rt.invalidate();
    rt.frame(0.0);
    let s2 = resolve_content_size(cfg, None, (0.0, 0.0), rt.core().committed_content_size().unwrap(), Some((1920.0, 1080.0)), 1.0, 8192);
    assert_eq!(s2, (120.0, 40.0));
    let px2 = bounded_layered_size(s2, 1.0);
    engine.paint_and_present("cand", 1, (780, 660), px2, 255, rt.core_mut());
    assert_eq!(engine.presenter().seam().created.len(), 2, "4x+ shrink must reallocate smaller DIB");
    let u2 = engine.presenter().seam().updates[1];
    assert_eq!(u2.dst, (780, 660));
    assert_eq!(u2.size, (120, 40));
    assert_eq!(u2.dirty, None);
}

#[test]
fn pre_show_measure_sets_planned_renderer_kind_and_fixed_width_change_remeasures_wrapped_height() {
    fn branching_view(text_str: &String, cx: &mut Cx<String>) -> El {
        let extra = if cx.renderer_kind().is_cpu() { 30.0 } else { 0.0 };
        col().padding(8.0).child(text(text_str.clone()).wrap()).child(el().size(10.0, 20.0 + extra))
    }
    let spec = crate::tooling::AppSpec::new("host", (400.0, 300.0), || (), |_, _| el());
    let (mut overlay, _) = crate::overlay::app::Overlay::new(
        OverlaySpec::new("wrap", (320.0, 0.0), branching_view)
            .options(OverlayOptions::default())
            .renderer(crate::RendererKind::Cpu)
            .fit_content_height(),
        "A long wrapping candidate line that breaks into more lines when narrowed from 320pt down to 120pt".to_owned(),
        &spec,
    );
    overlay.rt.wall_start = None;
    overlay.fit_for_show();
    let wide = overlay.canvas_size;
    #[cfg(target_os = "windows")]
    assert_eq!(overlay.rt.core().renderer_kind(), crate::RendererKind::Cpu);
    assert_eq!(wide.0, 320.0);

    // Narrowing fixed width via requested_size + fit_for_show re-measures taller wrapped height in one step.
    let narrowed = overlay.requested_size(overlay.rt.core(), Some((120.0, 0.0)), None).unwrap();
    overlay.canvas_size = narrowed;
    overlay.fit_for_show();
    assert_eq!(overlay.canvas_size.0, 120.0);
    assert!(overlay.canvas_size.1 > wide.1 + 10.0, "narrowing fixed width re-measures wrapped height: wide={wide:?} narrow={:?}", overlay.canvas_size);
}
