//! `ConfiguredOverlay::fit_content_height`: the overlay takes its content's height before it
//! shows, and asks for a new one when a shown overlay's content changes.
use std::time::Duration;

use super::super::{Overlay, OverlaySpec, Presence};
use crate::{AppSpec, Cx, El, col, el};

fn view(rows: &usize, _: &mut Cx<usize>) -> El {
    col().w_full().children((0..*rows).map(|i| el().key(format!("row{i}")).size(100.0, 20.0)))
}

fn overlay(rows: usize) -> Overlay<usize> {
    let app = AppSpec::new("fit", (200.0, 300.0), || 0, view);
    let spec = OverlaySpec::new("fit", app.size, view).options(Default::default()).fit_content_height();
    let (mut overlay, _) = Overlay::new(spec, rows, &app);
    overlay.rt.wall_start = None;
    overlay
}

/// An overlay measured and drawn once, as its first show leaves it.
fn shown_once(rows: usize) -> Overlay<usize> {
    let mut overlay = overlay(rows);
    overlay.fit_for_show();
    overlay.rt.set_viewport(overlay.canvas_size, 1.0);
    overlay.rt.frame(0.0);
    overlay.take_window_requests();
    overlay
}

fn set_rows(overlay: &mut Overlay<usize>, rows: usize) {
    overlay.rt.update(move |state, _| *state = rows);
}

type QueuedSizes = (Option<(f32, f32)>, Option<(f32, f32)>);

fn queued(overlay: &Overlay<usize>) -> QueuedSizes {
    let requests = overlay.shared.core.requests.lock().unwrap();
    (requests.size, requests.fit_size)
}

#[test]
fn an_overlay_that_fits_its_content_takes_its_height_before_its_first_frame() {
    let mut overlay = overlay(2);
    overlay.fit_for_show();
    assert_eq!(overlay.canvas_size, (200.0, 40.0));
    assert_eq!(overlay.rt.stats.commits, 0, "measured, not committed");
}

#[test]
fn a_change_made_while_hidden_is_measured_before_the_next_show() {
    let mut overlay = shown_once(2);
    assert_eq!(queued(&overlay), (None, None), "the first frame already fits");
    set_rows(&mut overlay, 5);
    overlay.fit_for_show();
    assert_eq!(overlay.canvas_size, (200.0, 100.0));
    assert!(overlay.rt.stale(), "measured: the first visible frame builds it");
}

#[test]
fn a_change_committed_while_hidden_is_kept_for_the_next_show() {
    let mut overlay = shown_once(2);
    set_rows(&mut overlay, 4);
    // A hidden frame commits it (pending input, a drag's end), as `hidden_wake` does.
    overlay.rt.frame(0.0);
    overlay.take_window_requests();
    assert_eq!(queued(&overlay), (None, None), "hidden, nothing to resize yet");
    assert_eq!(overlay.canvas_size, (200.0, 80.0));
    overlay.fit_for_show();
    assert_eq!(overlay.canvas_size, (200.0, 80.0), "already built: the show keeps it");
}

#[test]
fn a_shown_overlay_asks_for_its_contents_new_height() {
    let mut overlay = shown_once(2);
    overlay.presence = Presence::Shown { occluded: false };
    set_rows(&mut overlay, 3);
    overlay.rt.frame(0.0);
    overlay.take_window_requests();
    assert_eq!(queued(&overlay), (None, Some((200.0, 60.0))));
    assert_eq!(overlay.requested_size(overlay.rt.core(), None, Some((200.0, 60.0))), Some((200.0, 60.0)));
}

/// An explicit resize gives a fitted overlay its width; its height stays the content's, even
/// when the content's height doesn't change and no commit asks for it again.
#[test]
fn a_resize_sets_only_the_width_of_an_overlay_that_fits_its_content() {
    let mut overlay = shown_once(2);
    let size = overlay.requested_size(overlay.rt.core(), Some((320.0, 0.0)), None);
    assert_eq!(size, Some((320.0, 40.0)));
    overlay.canvas_size = size.unwrap();
    overlay.fit_for_show();
    assert_eq!(overlay.canvas_size, (320.0, 40.0));
}

/// `Tx::resize_window` asks a window; an overlay, fitted or not, ignores it.
#[test]
fn a_fitted_overlay_ignores_resize_window() {
    let mut overlay = shown_once(2);
    overlay.presence = Presence::Shown { occluded: false };
    overlay.rt.update(|_, tx| tx.resize_window((500.0, 500.0)));
    overlay.rt.frame(0.0);
    overlay.take_window_requests();
    assert_eq!(queued(&overlay), (None, None));
    assert_eq!(overlay.canvas_size, (200.0, 40.0));
}

#[test]
fn an_overlay_of_fixed_size_drops_what_its_frames_ask_of_a_window() {
    let app = AppSpec::new("fixed", (200.0, 300.0), || 2, view);
    let (mut overlay, _) = Overlay::new(OverlaySpec::new("fixed", app.size, view).options(Default::default()), 2, &app);
    overlay.fit_for_show();
    assert_eq!(overlay.canvas_size, (200.0, 300.0));
    overlay.rt.frame(0.0);
    set_rows(&mut overlay, 3);
    overlay.rt.frame(0.0);
    overlay.take_window_requests();
    assert_eq!(queued(&overlay), (None, None));
    assert_eq!(overlay.requested_size(overlay.rt.core(), Some((320.0, 10.0)), None), Some((320.0, 10.0)));
}

/// Measuring before the show commits and settles nothing: with `animate_reveal` a card added while hidden
/// enters on the first visible frame; without it the show lands it.
#[test]
fn fitting_before_an_animated_reveal_lets_the_new_card_enter() {
    fn cards(card: &bool, _: &mut Cx<bool>) -> El {
        let card = card.then(|| el().key("card").size(100.0, 40.0).transition(crate::Transition::offset(0.0, -24.0)));
        col().w_full().child(el().size(100.0, 20.0)).children(card)
    }
    let app = AppSpec::new("stack", (200.0, 300.0), || false, cards);
    for animate in [true, false] {
        let mut spec = OverlaySpec::new("stack", app.size, cards).options(Default::default()).fit_content_height();
        if animate { spec = spec.animate_reveal(); }
        let (mut overlay, _) = Overlay::new(spec, false, &app);
        overlay.rt.wall_start = None;
        overlay.rt.set_system_reduced_motion(false);
        overlay.fit_for_show();
        overlay.rt.set_viewport(overlay.canvas_size, 1.0);
        let start = crate::time::now();
        overlay.step_frame(start, false);
        overlay.take_window_requests();
        overlay.rt.update(|card, _| *card = true);
        overlay.fit_for_show();
        assert_eq!(overlay.canvas_size, (200.0, 60.0), "animate {animate}");
        // As `show_fading` goes on: the surface takes the height, the live values resume, and
        // the show settles or not.
        overlay.rt.set_viewport(overlay.canvas_size, 1.0);
        overlay.presence = Presence::Shown { occluded: false };
        overlay.resume_live();
        overlay.settle_before_reveal();
        overlay.step_frame(start + Duration::from_millis(16), false);
        let y = overlay.rt.presented("card").unwrap().0[1];
        if animate {
            assert!(y < 10.0 && overlay.rt.animating, "the first frame shows the card entering: y={y}");
        } else {
            assert_eq!(y, 20.0, "an ordinary show lands at rest");
        }
    }
}

#[test]
fn same_frame_live_resize_presents_zero_frames_at_old_height_on_gpu_and_cpu_layered() {
    use super::super::sizing::{SizingConfig, local_rect_for, resolve_content_size};
    use crate::{ContentSizing, OverlayPlacement, ScreenEdge};

    // 1. Live height change while shown is consumed immediately by `take_live_fit_resize`
    //    (used in `OverlayFrame::post_step_check` and `draw_cpu` before paint/present),
    //    with zero frames presented at the old height and nothing deferred to the next turn.
    let mut overlay = shown_once(2);
    overlay.core.at.size = overlay.canvas_size;
    overlay.core.last_work_area = Some((1920.0, 1080.0));
    overlay.presence = Presence::Shown { occluded: false };
    set_rows(&mut overlay, 5);
    overlay.rt.frame(0.0);
    let next = overlay
        .core
        .take_live_fit_resize(overlay.rt.core_mut())
        .expect("live height change resizes before present in the same frame");
    assert_eq!(next, (200.0, 100.0));
    // Nothing left for `take_window_requests` to defer to the next turn.
    overlay.take_window_requests();
    assert_eq!(queued(&overlay), (None, None), "must not defer resize to next event-loop turn");

    // 2. Clamp across fractional monitor scales (1.0, 1.25, 1.5, 2.0, 3.0), work area, and 0 height.
    let cfg = SizingConfig { mode: ContentSizing::FitHeight, min_size: None, max_size: None };
    for scale in [1.0f32, 1.25, 1.5, 2.0, 3.0] {
        let h = resolve_content_size(cfg, None, (200.0, 0.0), (200.0, 41.1), Some((1920.0, 1080.0)), scale, 8192).1;
        let px = h * scale;
        assert!((px - px.round()).abs() < 1e-4, "scale {scale}: {h} pt -> {px} px must be whole");
        assert!(h >= 41.1 - 1e-5);
    }
    assert_eq!(resolve_content_size(cfg, None, (200.0, 0.0), (200.0, 0.0), Some((1920.0, 1080.0)), 1.5, 8192).1, 1.0, "zero height clamps to 1 pt floor");
    assert_eq!(resolve_content_size(cfg, None, (200.0, 0.0), (200.0, 2000.0), Some((1920.0, 600.0)), 1.0, 8192).1, 600.0, "work area clamps height");

    // 3. OverlayPlacement::Rect preserves (x, y) and fixed width while taking fitted height.
    let at = super::super::Placement { size: (200.0, 88.0), edge: ScreenEdge::Top, margin: 0.0, align: crate::Align::Center, inset: 0.0 };
    let rect_p = Some(OverlayPlacement::Rect { x: 30.0, y: 50.0, width: 240.0, height: 10.0 });
    assert_eq!(local_rect_for(at, rect_p, (1920.0, 1080.0), ContentSizing::FitHeight), [30.0, 50.0, 240.0, 88.0]);
    assert_eq!(local_rect_for(at, rect_p, (1920.0, 1080.0), ContentSizing::Fixed), [30.0, 50.0, 240.0, 10.0]);
}

#[test]
fn fit_window_converts_cx_zoom_to_window_points_for_overlays() {
    fn zoomed_view(rows: &usize, cx: &mut Cx<usize>) -> El {
        cx.zoom(1.5);
        col().w_full().children((0..*rows).map(|i| el().key(format!("r{i}")).size(100.0, 20.0)))
    }
    let app = AppSpec::new("zoom", (300.0, 400.0), || 2usize, zoomed_view);
    let spec = OverlaySpec::new("zoom", app.size, zoomed_view).options(Default::default()).fit_content_height();
    let (mut overlay, _) = Overlay::new(spec, 2, &app);
    overlay.rt.wall_start = None;
    overlay.fit_for_show();
    // First commit latches zoom=1.5; 2 rows * 20 view pt * 1.5 zoom = 60 window pt.
    overlay.rt.set_viewport(overlay.canvas_size, 1.0);
    overlay.rt.frame(0.0);
    let _ = overlay.core.take_live_fit_resize(overlay.rt.core_mut());
    overlay.rt.set_viewport((300.0, 60.0), 1.0);
    overlay.core.at.size = (300.0, 60.0);
    overlay.presence = Presence::Shown { occluded: false };

    // Grow to 4 rows: 4 * 20 * 1.5 = 120 window pt (not 80 unzoomed view pt).
    overlay.rt.update(|n, _| *n = 4);
    overlay.rt.frame(0.0);
    let next = overlay.core.take_live_fit_resize(overlay.rt.core_mut()).expect("zoomed overlay resizes in window points");
    assert_eq!(next, (300.0, 120.0));
}

#[test]
fn wayland_pending_configure_machine_holds_previous_frame_until_configured() {
    use super::super::sizing::{WaylandResize, WaylandResizeEffect, WaylandResizeEvent};

    let mut m = WaylandResize::default();
    assert!(!m.is_pending());
    let eff: Vec<_> = m.step(WaylandResizeEvent::RequestResize((200, 100))).into_iter().collect();
    assert_eq!(eff, vec![WaylandResizeEffect::CommitAndHold((200, 100))]);
    assert!(m.is_pending());
    // Duplicate request is ignored while pending.
    assert!(m.step(WaylandResizeEvent::RequestResize((200, 100))).is_empty());
    // Compositor clamps to 96: leaves pending and applies confirmed height.
    let eff: Vec<_> = m.step(WaylandResizeEvent::Configured((200, 96))).into_iter().collect();
    assert_eq!(eff, vec![WaylandResizeEffect::ApplyConfigured((200, 96))]);
    assert!(!m.is_pending());
}

#[test]
fn cpu_layered_fitted_overlay_resizes_dib_atomically_before_present() {
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

    let mut overlay = overlay(2);
    overlay.core.requested_renderer = crate::RendererKind::Cpu;
    overlay.rt.core_mut().transparent = true;
    overlay.fit_for_show();
    assert_eq!(overlay.canvas_size, (200.0, 40.0));
    overlay.core.at.size = overlay.canvas_size;
    overlay.rt.set_viewport(overlay.canvas_size, 1.5);
    overlay.rt.frame(0.0);
    let mut engine = super::super::cpu::CpuPaintEngine::new(Seam::default(), 1);
    let px1 = bounded_layered_size(overlay.core.at.size, 1.5);
    engine.paint_and_present("fit", 1, (10, 20), px1, 255, overlay.rt.core_mut());
    assert_eq!(engine.presenter().seam().updates[0].size, (300, 60));
    assert_eq!(engine.presenter().seam().updates[0].flags, ULW_ALPHA);

    // Grow while shown: take_live_fit_resize runs before paint_and_present -> 0 frames at old size.
    overlay.presence = Presence::Shown { occluded: false };
    set_rows(&mut overlay, 5);
    overlay.rt.frame(0.0);
    let next = overlay.core.take_live_fit_resize(overlay.rt.core_mut()).expect("same-frame grow");
    assert_eq!(next, (200.0, 100.0));
    overlay.core.at.size = next;
    let px2 = bounded_layered_size(next, 1.5);
    engine.paint_and_present("fit", 1, (10, 20), px2, 255, overlay.rt.core_mut());
    assert_eq!(engine.presenter().seam().updates[1].size, (300, 150));
    assert_eq!(engine.presenter().seam().updates[1].dirty, None);
}


