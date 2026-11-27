use winit::dpi::PhysicalPosition;
use winit::event::MouseScrollDelta;
#[cfg(target_os = "macos")]
use winit::keyboard::ModifiersState;

use super::*;
use crate::{Cx, El, Env, GesturePhase, Runtime, WheelEvent, col, el};

const DT: f64 = 1.0 / 60.0;

fn scroll_offset<S: 'static>(rt: &Runtime<S>, key: &str) -> (f64, f64) {
    let id = rt
        .tree
        .nodes
        .iter()
        .find(|n| n.key.as_deref() == Some(key))
        .unwrap_or_else(|| panic!("missing scroll viewport {key}"))
        .id;
    let st = &rt.scroll.scrolls[&id];
    (st.offset, st.x.offset)
}

fn board_view(_: &(), _: &mut Cx<()>) -> El {
    col().size(200.0, 200.0).child(
        col()
            .key("board")
            .size(200.0, 200.0)
            .scroll_xy()
            .child(el().size(2000.0, 2000.0)),
    )
}

fn send_wheel_for_platform(
    rt: &mut RuntimeCore,
    delta: MouseScrollDelta,
    phase: GesturePhase,
    scale: f32,
    is_macos: bool,
    is_win_or_linux: bool,
) {
    let points = 40.0;
    let (dx, dy) = crate::runtime::wheel_delta(delta, scale, false, points);
    let precise = matches!(delta, MouseScrollDelta::PixelDelta(_));
    let ticks = match delta {
        MouseScrollDelta::LineDelta(_, y) => Some(y),
        MouseScrollDelta::PixelDelta(_) => None,
    };
    let notch = line_delta_is_notch(delta, is_macos, is_win_or_linux);
    let _ = wheel_core(
        rt,
        WheelInput {
            dx,
            dy,
            ticks,
            mods: crate::Mods::default(),
            phase,
            precise,
            notch,
            pinch: true,
        },
    );
}

#[test]
fn line_delta_is_notch_classifies_macos_windows_linux_and_other_platforms() {
    // macOS: AppKit sets hasPreciseScrollingDeltas == false (LineDelta) only for discrete
    // physical mouse wheels, whose OS-accelerated deltas are often fractional or multi-line.
    assert!(line_delta_is_notch(MouseScrollDelta::LineDelta(0.0, -0.3), true, false));
    assert!(line_delta_is_notch(MouseScrollDelta::LineDelta(0.0, -3.5), true, false));
    assert!(line_delta_is_notch(MouseScrollDelta::LineDelta(0.0, -1.0), true, false));
    assert!(line_delta_is_notch(MouseScrollDelta::LineDelta(-0.25, 0.0), true, false));
    // Trackpads, Magic Mouse, Mighty Mouse, and smooth-scroll utilities send PixelDelta.
    assert!(!line_delta_is_notch(
        MouseScrollDelta::PixelDelta(PhysicalPosition::new(0.0, -12.0)),
        true,
        false,
    ));
    assert!(!line_delta_is_notch(
        MouseScrollDelta::PixelDelta(PhysicalPosition::new(-8.0, 0.0)),
        true,
        false,
    ));

    // Windows and Linux: whole lines are notched detents; fractions are precision touchpads
    // or free-spinning wheels.
    assert!(line_delta_is_notch(MouseScrollDelta::LineDelta(0.0, -3.0), false, true));
    assert!(line_delta_is_notch(MouseScrollDelta::LineDelta(1.0, 0.0), false, true));
    assert!(!line_delta_is_notch(MouseScrollDelta::LineDelta(0.0, -0.5), false, true));
    assert!(!line_delta_is_notch(MouseScrollDelta::LineDelta(0.0, -3.5), false, true));
    assert!(!line_delta_is_notch(
        MouseScrollDelta::PixelDelta(PhysicalPosition::new(0.0, -40.0)),
        false,
        true,
    ));

    // Other platforms (e.g. web / iOS): never marked as desktop notched wheel here.
    assert!(!line_delta_is_notch(MouseScrollDelta::LineDelta(0.0, -1.0), false, false));
    assert!(!line_delta_is_notch(
        MouseScrollDelta::PixelDelta(PhysicalPosition::new(0.0, -40.0)),
        false,
        false,
    ));
}

#[test]
fn fractional_macos_line_delta_eases_and_respects_reduced_motion_glide_toggle_and_trackpad() {
    // 1. Normal macOS fractional mouse-wheel deltas ease smoothly across frames under WheelCurve.
    let mut rt = Runtime::new((), board_view, (200.0, 200.0), Env::new(true, 1.0));
    rt.frame(0.0);
    rt.pointer_move(100.0, 100.0);

    // First fractional notch: -0.3 lines * 40 pt = -12 pt -> target y = 12.0.
    send_wheel_for_platform(rt.core_mut(), MouseScrollDelta::LineDelta(0.0, -0.3), GesturePhase::Moved, 1.0, true, false);
    assert!(rt.frame(DT), "first frame after macOS fractional notch must schedule easing");
    assert_eq!(scroll_offset(&rt, "board").0, 0.0, "ease starts from rest on its first frame");

    // Stack an accelerated multi-line notch mid-ease: -3.5 lines * 40 pt = -140 pt -> target y = 152.0.
    send_wheel_for_platform(rt.core_mut(), MouseScrollDelta::LineDelta(0.0, -3.5), GesturePhase::Moved, 1.0, true, false);
    assert!(rt.frame(DT), "second frame must still be easing");
    let mid = scroll_offset(&rt, "board").0;
    assert!(mid > 0.0 && mid < 152.0, "expected intermediate eased offset in (0, 152), got {mid}");

    let mut settled = false;
    for _ in 0..30 {
        if !rt.frame(DT) {
            settled = true;
            break;
        }
    }
    assert!(settled, "wheel ease must settle idle");
    assert!((scroll_offset(&rt, "board").0 - 152.0).abs() < 1e-3, "expected target 152.0, got {:?}", scroll_offset(&rt, "board"));

    // Horizontal fractional macOS line delta also eases smoothly and lands on target.
    send_wheel_for_platform(rt.core_mut(), MouseScrollDelta::LineDelta(-1.5, 0.0), GesturePhase::Moved, 1.0, true, false);
    assert!(rt.frame(DT), "horizontal macOS fractional notch must ease");
    assert!(rt.frame(DT), "horizontal ease must continue on next frame");
    let mid_x = scroll_offset(&rt, "board").1;
    assert!(mid_x > 0.0 && mid_x < 60.0, "expected intermediate horizontal offset in (0, 60), got {mid_x}");
    let mut x_settled = false;
    for _ in 0..30 {
        if !rt.frame(DT) {
            x_settled = true;
            break;
        }
    }
    assert!(x_settled, "horizontal wheel ease must settle idle");
    assert!((scroll_offset(&rt, "board").1 - 60.0).abs() < 1e-3, "expected horizontal target 60.0, got {:?}", scroll_offset(&rt, "board"));

    // 2. Trackpad / Magic Mouse PixelDelta with full Began -> Moved -> Ended phases stays 1:1.
    send_wheel_for_platform(rt.core_mut(), MouseScrollDelta::PixelDelta(PhysicalPosition::new(0.0, 0.0)), GesturePhase::Began, 2.0, true, false);
    send_wheel_for_platform(rt.core_mut(), MouseScrollDelta::PixelDelta(PhysicalPosition::new(0.0, -48.0)), GesturePhase::Moved, 2.0, true, false);
    assert_eq!(scroll_offset(&rt, "board").0, 176.0, "trackpad PixelDelta moves 1:1 immediately (-48 px / 2.0 = -24 pt)");
    send_wheel_for_platform(rt.core_mut(), MouseScrollDelta::PixelDelta(PhysicalPosition::new(0.0, 0.0)), GesturePhase::Ended, 2.0, true, false);
    assert!(!rt.frame(DT), "trackpad PixelDelta must not start WheelCurve animation frames");
    assert_eq!(scroll_offset(&rt, "board").0, 176.0);

    // 3. Reduced motion lands fractional macOS notches instantly with no ease frames.
    let mut rt_rm = Runtime::new((), board_view, (200.0, 200.0), Env::new(true, 1.0));
    rt_rm.set_system_reduced_motion(true);
    rt_rm.frame(0.0);
    rt_rm.pointer_move(100.0, 100.0);
    send_wheel_for_platform(rt_rm.core_mut(), MouseScrollDelta::LineDelta(0.0, -1.5), GesturePhase::Moved, 1.0, true, false);
    assert!(!rt_rm.frame(DT), "reduced_motion must land fractional notch without ease frames");
    assert_eq!(scroll_offset(&rt_rm, "board").0, 60.0);

    // 4. Runtime::set_wheel_glide(false) lands fractional macOS notches instantly too.
    let mut rt_off = Runtime::new((), board_view, (200.0, 200.0), Env::new(true, 1.0));
    rt_off.set_wheel_glide(false);
    rt_off.frame(0.0);
    rt_off.pointer_move(100.0, 100.0);
    send_wheel_for_platform(rt_off.core_mut(), MouseScrollDelta::LineDelta(0.0, -1.5), GesturePhase::Moved, 1.0, true, false);
    assert!(!rt_off.frame(DT), "set_wheel_glide(false) must land fractional notch without ease frames");
    assert_eq!(scroll_offset(&rt_off, "board").0, 60.0);

    // 5. Windows/Linux fractional LineDelta still moves 1:1 (unchanged), while whole-line eases.
    let mut rt_wl = Runtime::new((), board_view, (200.0, 200.0), Env::new(true, 1.0));
    rt_wl.frame(0.0);
    rt_wl.pointer_move(100.0, 100.0);
    send_wheel_for_platform(rt_wl.core_mut(), MouseScrollDelta::LineDelta(0.0, -0.5), GesturePhase::Moved, 1.0, false, true);
    assert!(!rt_wl.frame(DT), "Windows/Linux fractional LineDelta must move 1:1 without easing");
    assert_eq!(scroll_offset(&rt_wl, "board").0, 20.0);
    // After RUN_IDLE (> 0.5 s), a whole-line Windows/Linux delta starts a notched ease.
    rt_wl.frame(1.0);
    send_wheel_for_platform(rt_wl.core_mut(), MouseScrollDelta::LineDelta(0.0, -3.0), GesturePhase::Moved, 1.0, false, true);
    assert!(rt_wl.frame(DT), "Windows/Linux whole-line LineDelta must ease");
}

#[test]
fn macos_on_wheel_first_refusal_receives_notch_flag_and_blocks_scroll() {
    #[derive(Default)]
    struct S { events: Vec<WheelEvent> }
    fn view(_: &S, _: &mut Cx<S>) -> El {
        col().size(200.0, 200.0).child(
            col()
                .key("board")
                .size(200.0, 200.0)
                .scroll_xy()
                .on_wheel(|s: &mut S, ev, _| {
                    s.events.push(ev);
                    true
                })
                .child(el().size(2000.0, 2000.0)),
        )
    }
    let mut rt = Runtime::new(S::default(), view, (200.0, 200.0), Env::new(true, 1.0));
    rt.frame(0.0);
    rt.pointer_move(100.0, 100.0);

    send_wheel_for_platform(rt.core_mut(), MouseScrollDelta::LineDelta(0.0, -0.3), GesturePhase::Moved, 1.0, true, false);
    send_wheel_for_platform(rt.core_mut(), MouseScrollDelta::PixelDelta(PhysicalPosition::new(0.0, -20.0)), GesturePhase::Moved, 1.0, true, false);
    assert!(!rt.frame(DT), "claimed wheel events must not start viewport easing");
    assert_eq!(scroll_offset(&rt, "board"), (0.0, 0.0));
    assert_eq!(rt.state.events.len(), 2);
    assert!(rt.state.events[0].notch && !rt.state.events[0].precise);
    assert!(!rt.state.events[1].notch && rt.state.events[1].precise);
}

#[cfg(target_os = "macos")]
#[test]
fn macos_host_wheel_path_eases_fractional_lines_and_keeps_trackpad_pixel_deltas_unchanged() {
    let mut rt = Runtime::new((), board_view, (200.0, 200.0), Env::new(true, 2.0));
    rt.frame(0.0);
    rt.pointer_move(100.0, 100.0);

    // Real winit -> host_events::wheel path with fractional macOS LineDelta (-0.3 * 40 = -12 pt).
    wheel(rt.core_mut(), MouseScrollDelta::LineDelta(0.0, -0.3), GesturePhase::Moved, 2.0, ModifiersState::empty());
    assert!(rt.frame(DT), "real macOS wheel() must ease fractional LineDelta");
    assert_eq!(scroll_offset(&rt, "board").0, 0.0);

    // Second accelerated LineDelta (-3.5 * 40 = -140 pt -> total 152 pt).
    wheel(rt.core_mut(), MouseScrollDelta::LineDelta(0.0, -3.5), GesturePhase::Moved, 2.0, ModifiersState::empty());
    assert!(rt.frame(DT));
    let mid = scroll_offset(&rt, "board").0;
    assert!(mid > 0.0 && mid < 152.0, "expected intermediate eased offset in (0, 152), got {mid}");

    let mut settled = false;
    for _ in 0..30 {
        if !rt.frame(DT) {
            settled = true;
            break;
        }
    }
    assert!(settled);
    assert!((scroll_offset(&rt, "board").0 - 152.0).abs() < 1e-3);

    // Real winit -> host_events::wheel path with phased trackpad PixelDelta (-48 px / 2.0 = -24 pt).
    wheel(rt.core_mut(), MouseScrollDelta::PixelDelta(PhysicalPosition::new(0.0, 0.0)), GesturePhase::Began, 2.0, ModifiersState::empty());
    wheel(rt.core_mut(), MouseScrollDelta::PixelDelta(PhysicalPosition::new(0.0, -48.0)), GesturePhase::Moved, 2.0, ModifiersState::empty());
    assert_eq!(scroll_offset(&rt, "board").0, 176.0, "trackpad PixelDelta must move 1:1 immediately");
    wheel(rt.core_mut(), MouseScrollDelta::PixelDelta(PhysicalPosition::new(0.0, 0.0)), GesturePhase::Ended, 2.0, ModifiersState::empty());
    assert!(!rt.frame(DT), "trackpad PixelDelta must not animate on WheelCurve");
}

#[test]
fn web_firefox_line_delta_notches_ease_on_wheel_curve_and_pixel_deltas_stay_direct() {
    let send_web = |rt: &mut RuntimeCore, delta: MouseScrollDelta, is_mac: bool| {
        let (dx, dy) = crate::runtime::wheel_delta(delta, 1.0, false, 40.0);
        let precise = matches!(delta, MouseScrollDelta::PixelDelta(_));
        let notch = line_delta_is_notch(delta, is_mac, !is_mac);
        let input = WheelInput {
            dx, dy, ticks: None, mods: crate::Mods::default(),
            phase: GesturePhase::Moved, precise, notch, pinch: false,
        };
        web_wheel(rt, input, false, false)
    };

    // 1. Windows/Linux Firefox: whole 3-line detent (-120 pt) eases on WheelCurve instead of jumping in one frame.
    let mut rt = Runtime::new((), board_view, (200.0, 200.0), Env::new(true, 1.0));
    rt.frame(0.0);
    rt.pointer_move(100.0, 100.0);
    let (redraw, used) = send_web(rt.core_mut(), MouseScrollDelta::LineDelta(0.0, -3.0), false);
    assert!(redraw && used.is_none());
    assert!(rt.frame(DT), "Firefox 3-line detent must schedule WheelCurve easing");
    assert_eq!(scroll_offset(&rt, "board").0, 0.0, "ease begins from rest on its first frame");
    assert!(rt.frame(DT));
    let mid = scroll_offset(&rt, "board").0;
    assert!(mid > 0.0 && mid < 120.0, "expected intermediate eased offset in (0, 120), got {mid}");

    // Stacking a second 3-line detent mid-ease smoothly retargets to 240 pt.
    let (redraw, _) = send_web(rt.core_mut(), MouseScrollDelta::LineDelta(0.0, -3.0), false);
    assert!(redraw);
    let mut settled = false;
    for _ in 0..40 {
        if !rt.frame(DT) { settled = true; break; }
    }
    assert!(settled, "stacked Firefox wheel ease must settle idle");
    assert!((scroll_offset(&rt, "board").0 - 240.0).abs() < 1e-3);

    // 2. Chrome/Safari/trackpad PixelDelta moves 1:1 immediately with no ease frames.
    rt.frame(1.0); // past WheelLock RUN_IDLE
    let (redraw, _) = send_web(rt.core_mut(), MouseScrollDelta::PixelDelta(PhysicalPosition::new(0.0, -40.0)), false);
    assert!(redraw);
    assert_eq!(scroll_offset(&rt, "board").0, 280.0, "PixelDelta must land 1:1 immediately");
    assert!(!rt.frame(DT), "PixelDelta must not start WheelCurve easing");

    // 3. Windows/Linux fractional LineDelta stays 1:1; macOS Firefox fractional accelerated LineDelta eases.
    rt.frame(1.0);
    let (redraw, _) = send_web(rt.core_mut(), MouseScrollDelta::LineDelta(0.0, -0.5), false);
    assert!(redraw);
    assert_eq!(scroll_offset(&rt, "board").0, 300.0, "non-mac fractional LineDelta moves 1:1");
    assert!(!rt.frame(DT));

    rt.frame(1.0);
    let (redraw, _) = send_web(rt.core_mut(), MouseScrollDelta::LineDelta(0.0, -1.5), true);
    assert!(redraw);
    assert!(rt.frame(DT), "macOS Firefox fractional LineDelta must ease on WheelCurve");
    assert!(rt.frame(DT));
    let mac_mid = scroll_offset(&rt, "board").0;
    assert!(mac_mid > 300.0 && mac_mid < 360.0, "expected macOS ease in (300, 360), got {mac_mid}");
    for _ in 0..40 { if !rt.frame(DT) { break; } }
    assert!((scroll_offset(&rt, "board").0 - 360.0).abs() < 1e-3);
}

#[test]
fn web_gesture_start_only_claims_magnify_or_rotate_outside_document_and_fit_height() {
    #[derive(Default)]
    struct S { modal: bool }
    fn view(s: &S, cx: &mut Cx<S>) -> El {
        cx.zoom(1.5);
        let mut root = col().size(400.0, 400.0)
            .child(el().key("plain").size(100.0, 50.0).hit())
            .child(col().key("scroller").size(100.0, 50.0).scroll_xy().child(el().size(100.0, 200.0)))
            .child(el().key("zoomable").size(100.0, 50.0).on_magnify(|_: &mut S, _, _| true))
            .child(el().key("any_zoom").size(100.0, 50.0).on_magnify_any(|_, _| true))
            .child(el().key("rotatable").size(100.0, 50.0).on_rotate(|_: &mut S, _, _| true))
            .child(el().key("inert_zoom").size(100.0, 50.0).inert().on_magnify(|_: &mut S, _, _| true));
        if s.modal {
            root = root.child(el().key("modal").size(400.0, 400.0).modal(false, |_: &mut S| {}).hit());
        }
        root
    }
    let mut rt = Runtime::new(S::default(), view, (600.0, 600.0), Env::new(true, 1.0));
    rt.frame(0.0);

    let at = |rt: &Runtime<S>, key: &str| {
        let (vx, vy) = rt.find(key).unwrap();
        // Convert back to CSS window points (zoom 1.5) and through window_point to test zoom scaling.
        window_point(rt.core(), vx * 1.5, vy * 1.5)
    };

    // Plain elements and plain scroll viewports leave Safari pinch-zoom untouched.
    assert!(!web_gesture_start(rt.core(), Some(at(&rt, "plain")), false, false));
    assert!(!web_gesture_start(rt.core(), Some(at(&rt, "scroller")), false, false));
    assert!(!web_gesture_start(rt.core(), Some(at(&rt, "inert_zoom")), false, false));

    // on_magnify, on_magnify_any, and on_rotate claim Safari pinch/rotation gestures.
    let zoom_pt = at(&rt, "zoomable");
    assert!(web_gesture_start(rt.core(), Some(zoom_pt), false, false));
    assert!(web_gesture_start(rt.core(), Some(at(&rt, "any_zoom")), false, false));
    assert!(web_gesture_start(rt.core(), Some(at(&rt, "rotatable")), false, false));

    // Document mode and fit_height canvases never prevent browser pinch-zoom.
    assert!(!web_gesture_start(rt.core(), Some(zoom_pt), true, false));
    assert!(!web_gesture_start(rt.core(), Some(zoom_pt), false, true));

    // Missing event coordinates fall back to rt.pointer().
    assert!(!web_gesture_start(rt.core(), None, false, false), "no pointer -> unclaimed");
    rt.pointer_move(zoom_pt.0, zoom_pt.1);
    assert!(web_gesture_start(rt.core(), None, false, false), "pointer over on_magnify -> claimed");
    let plain_pt = at(&rt, "plain");
    rt.pointer_move(plain_pt.0, plain_pt.1);
    assert!(!web_gesture_start(rt.core(), None, false, false), "pointer over plain -> unclaimed");

    // An open modal blocks underlying on_magnify elements.
    rt.update(|s, _| s.modal = true);
    rt.frame(0.0);
    assert!(!web_gesture_start(rt.core(), Some(zoom_pt), false, false), "modal blocks underlying on_magnify");
}

#[test]
fn app_wheel_line_and_glide_apply_to_notches_while_touchpad_and_pinch_keep_defaults() {
    #[derive(Default)]
    struct S { magnified: Vec<f32> }
    fn view(_: &S, _: &mut Cx<S>) -> El {
        col().size(200.0, 200.0).child(
            col()
                .key("board")
                .size(200.0, 200.0)
                .scroll_xy()
                .on_magnify(|s: &mut S, ev, _| {
                    s.magnified.push(ev.delta);
                    true
                })
                .child(el().size(2000.0, 2000.0)),
        )
    }
    let spec = crate::AppSpec::new("wheel-cfg", (200.0, 200.0), S::default, view);
    let mut rt = crate::App::new(spec)
        .wheel_line(60.0)
        .wheel_glide(false)
        .headless_runtime((200.0, 200.0), Env::new(true, 1.0));
    rt.frame(0.0);
    rt.pointer_move(100.0, 100.0);

    // 1. One vertical wheel notch scrolls 60 pt immediately with no glide frames.
    wheel_from_delta(
        rt.core_mut(),
        MouseScrollDelta::LineDelta(0.0, -1.0),
        GesturePhase::Moved,
        2.0,
        crate::Mods::default(),
    );
    assert!(!rt.frame(DT), "wheel_glide(false) must land notch without ease frames");
    assert_eq!(scroll_offset(&rt, "board"), (60.0, 0.0));

    // 2. Half-detent high-res wheel scrolls 30 pt immediately.
    wheel_from_delta(
        rt.core_mut(),
        MouseScrollDelta::LineDelta(0.0, -0.5),
        GesturePhase::Moved,
        2.0,
        crate::Mods::default(),
    );
    assert!(!rt.frame(DT));
    assert_eq!(scroll_offset(&rt, "board"), (90.0, 0.0));

    // 3. Horizontal line delta scrolls 60 pt immediately.
    wheel_from_delta(
        rt.core_mut(),
        MouseScrollDelta::LineDelta(-1.0, 0.0),
        GesturePhase::Moved,
        2.0,
        crate::Mods::default(),
    );
    assert!(!rt.frame(DT));
    assert_eq!(scroll_offset(&rt, "board"), (90.0, 60.0));

    // 4. Touchpad PixelDelta keeps its exact point delta (-48 px / 2.0 = -24 pt -> +24 pt).
    wheel_from_delta(
        rt.core_mut(),
        MouseScrollDelta::PixelDelta(PhysicalPosition::new(0.0, 0.0)),
        GesturePhase::Began,
        2.0,
        crate::Mods::default(),
    );
    wheel_from_delta(
        rt.core_mut(),
        MouseScrollDelta::PixelDelta(PhysicalPosition::new(0.0, -48.0)),
        GesturePhase::Moved,
        2.0,
        crate::Mods::default(),
    );
    wheel_from_delta(
        rt.core_mut(),
        MouseScrollDelta::PixelDelta(PhysicalPosition::new(0.0, 0.0)),
        GesturePhase::Ended,
        2.0,
        crate::Mods::default(),
    );
    assert!(!rt.frame(DT));
    assert_eq!(scroll_offset(&rt, "board"), (114.0, 60.0));

    // 5. Ctrl + mouse wheel pinch still zooms by 1.2x (+0.2) per notch regardless of wheel_line.
    let ctrl = crate::Mods { ctrl: true, ..crate::Mods::default() };
    wheel_from_delta(
        rt.core_mut(),
        MouseScrollDelta::LineDelta(0.0, 1.0),
        GesturePhase::Moved,
        2.0,
        ctrl,
    );
    rt.frame(0.0);
    assert_eq!(rt.state.magnified.len(), 1);
    assert!((rt.state.magnified[0] - 0.2).abs() < 1e-4, "expected +0.2 pinch delta, got {:?}", rt.state.magnified);

    // 6. With default wheel_glide(true) + wheel_line(60.0), a whole notch eases and lands at 60 pt.
    let mut rt_glide = crate::App::new(crate::AppSpec::new("wheel-glide", (200.0, 200.0), || (), board_view))
        .wheel_line(60.0)
        .headless_runtime((200.0, 200.0), Env::new(true, 1.0));
    rt_glide.frame(0.0);
    rt_glide.pointer_move(100.0, 100.0);
    wheel_from_delta(
        rt_glide.core_mut(),
        MouseScrollDelta::LineDelta(0.0, -1.0),
        GesturePhase::Moved,
        1.0,
        crate::Mods::default(),
    );
    assert!(rt_glide.frame(DT), "default wheel_glide(true) must ease whole notch");
    let mut settled = false;
    for _ in 0..30 {
        if !rt_glide.frame(DT) {
            settled = true;
            break;
        }
    }
    assert!(settled, "wheel ease must settle");
    assert!((scroll_offset(&rt_glide, "board").0 - 60.0).abs() < 1e-3);
}
