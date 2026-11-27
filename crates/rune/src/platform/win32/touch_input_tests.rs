use std::cell::Cell;
use std::rc::Rc;

use windows_sys::Win32::UI::WindowsAndMessaging::{
    NID_EXTERNAL_PEN, NID_EXTERNAL_TOUCH, NID_INTEGRATED_PEN, NID_INTEGRATED_TOUCH, NID_MULTI_INPUT,
    NID_READY, PT_MOUSE, PT_PEN, PT_TOUCH, PT_TOUCHPAD, WM_MOUSEMOVE, WM_POINTERDOWN, WM_POINTERUP,
    WM_POINTERUPDATE, WM_TOUCH,
};
use winit::dpi::PhysicalPosition;
use winit::event::{DeviceId, Touch, TouchPhase as WinitTouchPhase};

use super::{
    MessagePointerKind, PromotedSource, SystemTouchMetrics, classify_extra_info,
    classify_win32_message, has_touch_digitizer, is_tablet_posture, is_tablet_setting_change_area,
    with_current_pointer_for_test,
};
use crate::runtime::PrimaryPointer;
use crate::{Cx, El, Env, Runtime, col, el, text};

#[test]
fn system_touch_metrics_classify_digitizer_and_tablet_posture() {
    let desktop_no_touch = SystemTouchMetrics {
        max_touches: 0,
        digitizer: 0,
        slate_mode: 1,
        mouse_present: 1,
    };
    assert!(!has_touch_digitizer(desktop_no_touch));
    assert!(!is_tablet_posture(desktop_no_touch));

    // Slate mode = 0 without a touch digitizer is never tablet posture.
    let no_touch_slate_zero = SystemTouchMetrics { slate_mode: 0, mouse_present: 0, ..desktop_no_touch };
    assert!(!has_touch_digitizer(no_touch_slate_zero));
    assert!(!is_tablet_posture(no_touch_slate_zero));

    // Pen-only digitizer (no touch flags) does not enable touch controls.
    let pen_only = SystemTouchMetrics {
        max_touches: 1,
        digitizer: NID_READY | NID_INTEGRATED_PEN | NID_EXTERNAL_PEN,
        slate_mode: 0,
        mouse_present: 0,
    };
    assert!(!has_touch_digitizer(pen_only));
    assert!(!is_tablet_posture(pen_only));

    // Touch digitizer without NID_READY is not ready.
    let unready_touch = SystemTouchMetrics {
        max_touches: 10,
        digitizer: NID_INTEGRATED_TOUCH | NID_MULTI_INPUT,
        slate_mode: 0,
        mouse_present: 0,
    };
    assert!(!has_touch_digitizer(unready_touch));
    assert!(!is_tablet_posture(unready_touch));

    // Hybrid touchscreen laptop in clamshell mode with mouse/trackpad: has touch, starts Hover.
    let clamshell_touch = SystemTouchMetrics {
        max_touches: 10,
        digitizer: NID_READY | NID_INTEGRATED_TOUCH | NID_MULTI_INPUT,
        slate_mode: 1,
        mouse_present: 1,
    };
    assert!(has_touch_digitizer(clamshell_touch));
    assert!(!is_tablet_posture(clamshell_touch));

    // External USB touchscreen paired with a desktop mouse: has touch, starts Hover.
    let external_touch_desktop = SystemTouchMetrics {
        max_touches: 10,
        digitizer: NID_READY | NID_EXTERNAL_TOUCH,
        slate_mode: 1,
        mouse_present: 1,
    };
    assert!(has_touch_digitizer(external_touch_desktop));
    assert!(!is_tablet_posture(external_touch_desktop));

    // 2-in-1 folded into slate/tablet mode: starts Touch even if a HID mouse driver reports present.
    let slate_tablet = SystemTouchMetrics {
        max_touches: 10,
        digitizer: NID_READY | NID_INTEGRATED_TOUCH | NID_MULTI_INPUT,
        slate_mode: 0,
        mouse_present: 1,
    };
    assert!(has_touch_digitizer(slate_tablet));
    assert!(is_tablet_posture(slate_tablet));

    // Touch-only Windows device with no mouse installed: starts Touch.
    let touch_only_kiosk = SystemTouchMetrics {
        max_touches: 5,
        digitizer: NID_READY | NID_EXTERNAL_TOUCH,
        slate_mode: 1,
        mouse_present: 0,
    };
    assert!(has_touch_digitizer(touch_only_kiosk));
    assert!(is_tablet_posture(touch_only_kiosk));
}

#[test]
fn extra_info_distinguishes_mouse_pen_and_touch() {
    assert_eq!(classify_extra_info(0), PromotedSource::Mouse);
    assert_eq!(classify_extra_info(0x1234_5678), PromotedSource::Mouse);
    assert_eq!(classify_extra_info(0xFF51_5680), PromotedSource::Mouse);

    // MI_WP_SIGNATURE with bit 7 clear -> pen-promoted.
    assert_eq!(classify_extra_info(0xFF51_5700), PromotedSource::Pen);
    assert_eq!(classify_extra_info(0xFF51_5701), PromotedSource::Pen);
    assert_eq!(classify_extra_info(0xFF51_577F), PromotedSource::Pen);

    // MI_WP_SIGNATURE with bit 7 set -> touch-promoted.
    assert_eq!(classify_extra_info(0xFF51_5780), PromotedSource::Touch);
    assert_eq!(classify_extra_info(0xFF51_5783), PromotedSource::Touch);
    assert_eq!(classify_extra_info(0xFF51_57FF), PromotedSource::Touch);
}

#[test]
fn win32_message_classifies_wm_touch_and_wm_pointer_sources() {
    // WM_TOUCH is unconditionally finger touch without calling GetPointerType.
    let mut queried = false;
    let touch_kind = classify_win32_message(WM_TOUCH, 0, |_| {
        queried = true;
        None
    });
    assert_eq!(touch_kind, Some(MessagePointerKind::Touch));
    assert!(!queried);

    // WM_POINTER* extracts LOWORD(wparam) as pointer_id and maps PT_TOUCH / PT_PEN / PT_MOUSE / PT_TOUCHPAD.
    for msg in [WM_POINTERDOWN, WM_POINTERUPDATE, WM_POINTERUP] {
        let wparam = 0x1234_0042usize;
        let mut seen_id = 0u32;
        assert_eq!(
            classify_win32_message(msg, wparam, |id| { seen_id = id; Some(PT_TOUCH) }),
            Some(MessagePointerKind::Touch),
        );
        assert_eq!(seen_id, 0x0042);
        assert_eq!(
            classify_win32_message(msg, wparam, |_| Some(PT_PEN)),
            Some(MessagePointerKind::Pen),
        );
        assert_eq!(
            classify_win32_message(msg, wparam, |_| Some(PT_MOUSE)),
            Some(MessagePointerKind::Hover),
        );
        assert_eq!(
            classify_win32_message(msg, wparam, |_| Some(PT_TOUCHPAD)),
            Some(MessagePointerKind::Hover),
        );
        assert_eq!(classify_win32_message(msg, wparam, |_| None), None);
    }

    assert_eq!(MessagePointerKind::Touch.primary_pointer(), PrimaryPointer::Touch);
    assert_eq!(MessagePointerKind::Pen.primary_pointer(), PrimaryPointer::Hover);
    assert_eq!(MessagePointerKind::Hover.primary_pointer(), PrimaryPointer::Hover);
    assert_eq!(classify_win32_message(WM_MOUSEMOVE, 0, |_| Some(PT_TOUCH)), None);
}

#[test]
fn tablet_setting_change_area_matches_slate_and_interaction_mode() {
    assert!(is_tablet_setting_change_area(Some("ConvertibleSlateMode")));
    assert!(is_tablet_setting_change_area(Some("convertibleslatemode")));
    assert!(is_tablet_setting_change_area(Some("UserInteractionMode")));
    assert!(!is_tablet_setting_change_area(None));
    assert!(!is_tablet_setting_change_area(Some("")));
    assert!(!is_tablet_setting_change_area(Some("ImmersiveColorSet")));
    assert!(!is_tablet_setting_change_area(Some("WindowsThemeElement")));
}

#[test]
fn windows_touch_core_distinguishes_finger_touch_from_pen_without_breaking_clicks() {
    struct State {
        clicks: usize,
        seen_touch: Rc<Cell<bool>>,
        seen_hover_none: Rc<Cell<bool>>,
    }
    fn view(s: &State, cx: &mut Cx<State>) -> El {
        let touch = cx.touch_controls();
        let hover_none = cx.hover_none();
        s.seen_touch.set(touch);
        s.seen_hover_none.set(hover_none);
        col().size(200.0, 100.0).child(
            el().key("btn")
                .size(120.0, 60.0)
                .on_press(|s: &mut State| s.clicks += 1)
                .child(text(if touch { "Touch targets" } else { "Mouse targets" })),
        )
    }

    let seen_touch = Rc::new(Cell::new(false));
    let seen_hover_none = Rc::new(Cell::new(false));
    let mut rt = Runtime::new(
        State { clicks: 0, seen_touch: seen_touch.clone(), seen_hover_none: seen_hover_none.clone() },
        view,
        (200.0, 100.0),
        Env::new(true, 1.0),
    );
    rt.frame(0.0);
    assert!(!seen_touch.get());
    assert!(!seen_hover_none.get());

    let mk = |id: u64, phase: WinitTouchPhase| Touch {
        device_id: DeviceId::dummy(),
        phase,
        location: PhysicalPosition::new(40.0, 25.0),
        force: None,
        id,
    };

    // Finger touch (WM_TOUCH / WM_POINTER* PT_TOUCH) activates touch_controls and clicks normally.
    with_current_pointer_for_test(Some(MessagePointerKind::Touch), || {
        let (core, mut app) = rt.split_mut();
        crate::window::touch_core_at(core, &mut app, &mk(1, WinitTouchPhase::Started), 0.0);
        crate::window::touch_core_at(core, &mut app, &mk(1, WinitTouchPhase::Ended), 0.02);
    });
    rt.frame(0.0);
    assert!(seen_touch.get(), "finger touch enables Cx::touch_controls");
    assert!(seen_hover_none.get(), "finger touch enables Cx::hover_none");
    assert_eq!(rt.state.clicks, 1, "finger tap still clicks");

    // Windows pen (WM_POINTER* PT_PEN) returns Cx::touch_controls to false while still clicking.
    with_current_pointer_for_test(Some(MessagePointerKind::Pen), || {
        let (core, mut app) = rt.split_mut();
        crate::window::touch_core_at(core, &mut app, &mk(2, WinitTouchPhase::Started), 0.1);
        crate::window::touch_core_at(core, &mut app, &mk(2, WinitTouchPhase::Ended), 0.12);
    });
    rt.frame(0.0);
    assert!(!seen_touch.get(), "pen input leaves Cx::touch_controls false");
    assert!(!seen_hover_none.get(), "pen input leaves Cx::hover_none false");
    assert_eq!(rt.state.clicks, 2, "pen tap still clicks normally");

    // Synthetic touch outside a Win32 message defaults to finger touch.
    with_current_pointer_for_test(None, || {
        let (core, mut app) = rt.split_mut();
        crate::window::touch_core_at(core, &mut app, &mk(3, WinitTouchPhase::Started), 0.2);
        crate::window::touch_core_at(core, &mut app, &mk(3, WinitTouchPhase::Ended), 0.22);
    });
    rt.frame(0.0);
    assert!(seen_touch.get());
    assert_eq!(rt.state.clicks, 3);
}

#[test]
fn tablet_posture_sync_updates_on_transitions_without_clobbering_live_touch_on_refocus() {
    let seen = Rc::new(Cell::new(false));
    let seen_v = seen.clone();
    let mut rt = Runtime::new(
        seen_v,
        |s: &Rc<Cell<bool>>, cx: &mut Cx<Rc<Cell<bool>>>| {
            s.set(cx.touch_controls());
            el().size(100.0, 100.0)
        },
        (100.0, 100.0),
        Env::new(true, 1.0),
    );
    // Hybrid touchscreen laptop starts with tablet_mode = false.
    rt.core_mut().sync_system_tablet_mode(false);
    rt.frame(0.0);
    assert!(!seen.get());

    // User touches the screen -> switches live to touch controls.
    rt.touch(1, crate::TouchPhase::Started, 50.0, 50.0, 0.0);
    rt.touch(1, crate::TouchPhase::Ended, 50.0, 50.0, 0.02);
    rt.frame(0.0);
    assert!(seen.get());

    // Window regains focus with tablet_mode still false: live touch mode stays intact.
    assert!(!rt.core_mut().sync_system_tablet_mode(false));
    rt.frame(0.0);
    assert!(seen.get(), "unchanged tablet_mode on focus must not reset live touch mode");

    // Device enters tablet mode (false -> true), then exits tablet mode (true -> false): switches back to Hover.
    let _ = rt.core_mut().sync_system_tablet_mode(true);
    rt.frame(0.0);
    assert!(seen.get());
    assert!(rt.core_mut().sync_system_tablet_mode(false), "leaving tablet mode dirties view");
    rt.frame(0.0);
    assert!(!seen.get(), "leaving tablet mode switches back to fine controls");
}
