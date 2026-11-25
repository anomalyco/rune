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
