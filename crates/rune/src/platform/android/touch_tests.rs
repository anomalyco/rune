//! Host-side unit tests for Android `MotionEvent` action mapping, coordinate/time conversion,
//! and end-to-end touch tap/scroll/fling/long-press/cancel behavior on a Rune `Runtime`.

use super::touch::{
    ACTION_CANCEL, ACTION_DOWN, ACTION_HOVER_ENTER, ACTION_HOVER_EXIT, ACTION_HOVER_MOVE,
    ACTION_MOVE, ACTION_OUTSIDE, ACTION_POINTER_DOWN, ACTION_POINTER_UP, ACTION_SCROLL, ACTION_UP,
    FLAG_CANCELED, action_to_phase, event_time_ms_to_secs, long_press_ms_to_secs,
    pointer_id_to_u64, px_to_dp_point,
};
use crate::{Cx, El, Env, Runtime, ScrollSpec, TouchPhase, col, el};

#[test]
fn action_to_phase_maps_every_android_touch_action_and_flag_canceled() {
    assert_eq!(action_to_phase(ACTION_DOWN, 0), Some(TouchPhase::Started));
    assert_eq!(action_to_phase(ACTION_POINTER_DOWN, 0), Some(TouchPhase::Started));
    // Unmasked action with pointer index 2 encoded in bits 8..15 still maps cleanly.
    assert_eq!(action_to_phase(ACTION_POINTER_DOWN | (2 << 8), 0), Some(TouchPhase::Started));

    assert_eq!(action_to_phase(ACTION_MOVE, 0), Some(TouchPhase::Moved));

    assert_eq!(action_to_phase(ACTION_UP, 0), Some(TouchPhase::Ended));
    assert_eq!(action_to_phase(ACTION_POINTER_UP, 0), Some(TouchPhase::Ended));
    assert_eq!(action_to_phase(ACTION_POINTER_UP | (1 << 8), 0), Some(TouchPhase::Ended));

    // API 33+ FLAG_CANCELED turns UP / POINTER_UP into Cancelled (never a tap).
    assert_eq!(action_to_phase(ACTION_UP, FLAG_CANCELED), Some(TouchPhase::Cancelled));
    assert_eq!(action_to_phase(ACTION_POINTER_UP | (1 << 8), FLAG_CANCELED), Some(TouchPhase::Cancelled));
    assert_eq!(action_to_phase(ACTION_CANCEL, 0), Some(TouchPhase::Cancelled));

    // Non-touch actions are ignored.
    for ignored in [
        ACTION_OUTSIDE,
        ACTION_HOVER_MOVE,
        ACTION_SCROLL,
        ACTION_HOVER_ENTER,
        ACTION_HOVER_EXIT,
        99,
        -1,
    ] {
        assert_eq!(action_to_phase(ignored, 0), None, "action {ignored} must not map to TouchPhase");
    }
}

#[test]
fn coordinate_timestamp_pointer_id_and_long_press_conversions() {
    assert_eq!(px_to_dp_point(300.0, 600.0, 3.0), Some((100.0, 200.0)));
    assert_eq!(px_to_dp_point(50.0, 80.0, 0.0), Some((50.0, 80.0)));
    assert_eq!(px_to_dp_point(50.0, 80.0, f32::NAN), Some((50.0, 80.0)));
    assert_eq!(px_to_dp_point(f32::NAN, 80.0, 2.0), None);
    assert_eq!(px_to_dp_point(50.0, f32::INFINITY, 2.0), None);

    assert_eq!(pointer_id_to_u64(0), 0);
    assert_eq!(pointer_id_to_u64(7), 7);
    assert_ne!(pointer_id_to_u64(-1), pointer_id_to_u64(0));

    assert!((event_time_ms_to_secs(12_345) - 12.345).abs() < 1e-9);

    assert_eq!(long_press_ms_to_secs(400), Some(0.4));
    assert_eq!(long_press_ms_to_secs(1000), Some(1.0));
    assert_eq!(long_press_ms_to_secs(1500), Some(1.5));
    assert_eq!(long_press_ms_to_secs(0), None);
    assert_eq!(long_press_ms_to_secs(-400), None);
}

#[test]
fn android_touch_pipeline_drives_tap_batched_fling_and_400ms_long_press() {
    #[derive(Default)]
    struct S {
        taps: usize,
        holds: usize,
        touch_controls: bool,
        hover_none: bool,
    }
    fn view(s: &S, cx: &mut Cx<S>) -> El {
        let _ = (s.touch_controls, s.hover_none);
        let tc = cx.touch_controls();
        let hn = cx.hover_none();
        col().size(360.0, 640.0)
            .child(
                el().key("btn")
                    .size(360.0, 60.0)
                    .on_press(move |s: &mut S| {
                        s.taps += 1;
                        s.touch_controls = tc;
                        s.hover_none = hn;
                    })
                    .on_secondary_press(|s: &mut S, _, _| s.holds += 1),
            )
            .child(
                col().key("list")
                    .w_full()
                    .h(400.0)
                    .scroll(ScrollSpec { stick_bottom: false })
                    .children((0..40).map(|i| el().key(format!("r{i}")).w_full().h(48.0))),
            )
    }

    let scale = 3.0f32;
    let mut rt = Runtime::new(S::default(), view, (360.0, 640.0), Env::new(true, scale));
    rt.frame.set_test_clock(crate::ManualClock::new(60.0));
    rt.set_touch_controls(true);
    rt.set_touch_long_press(long_press_ms_to_secs(400).unwrap());
    rt.frame(0.0);

    let feed = |rt: &mut Runtime<S>, action: i32, flags: i32, id: i32, x_px: f32, y_px: f32, t_ms: i64| {
        if let (Some(phase), Some((x, y))) = (action_to_phase(action, flags), px_to_dp_point(x_px, y_px, scale)) {
            rt.touch(pointer_id_to_u64(id), phase, x, y, event_time_ms_to_secs(t_ms));
        }
    };

    // 1. Tap on button reports Cx::touch_controls() == true and Cx::hover_none() == true.
    feed(&mut rt, ACTION_DOWN, 0, 0, 180.0 * scale, 30.0 * scale, 1_000);
    feed(&mut rt, ACTION_UP, 0, 0, 180.0 * scale, 30.0 * scale, 1_040);
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.state.taps, 1);
    assert!(rt.state.touch_controls && rt.state.hover_none);

    // 2. FLAG_CANCELED on ACTION_UP cancels without firing a tap.
    feed(&mut rt, ACTION_DOWN, 0, 0, 180.0 * scale, 30.0 * scale, 1_100);
    feed(&mut rt, ACTION_UP, FLAG_CANCELED, 0, 180.0 * scale, 30.0 * scale, 1_130);
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.state.taps, 1, "FLAG_CANCELED must not commit a tap");

    // 3. Long-press arms and fires at Android's 400 ms delay.
    let t0 = rt.core().clock;
    feed(&mut rt, ACTION_DOWN, 0, 0, 180.0 * scale, 30.0 * scale, 1_200);
    assert!((rt.wake_deadline().unwrap() - (t0 + 0.4)).abs() < 1e-9);
    rt.frame.test_clock_mut().unwrap().seek(t0 + 0.41);
    rt.frame(0.41);
    assert_eq!(rt.state.holds, 1, "long-press fires at 400 ms");
    feed(&mut rt, ACTION_UP, 0, 0, 180.0 * scale, 30.0 * scale, 1_650);
    rt.frame(0.0);
    assert_eq!(rt.state.taps, 1, "lifting after long-press must not also tap");

    // 4. Batched ACTION_MOVE historical samples scroll under the finger and fling after lift.
    let list_id = rt.tree.nodes.iter().find(|n| n.key.as_deref() == Some("list")).unwrap().id;
    feed(&mut rt, ACTION_DOWN, 0, 0, 180.0 * scale, 300.0 * scale, 2_000);
    for i in 1..=5 {
        let y = (300.0 - 24.0 * i as f32) * scale;
        feed(&mut rt, ACTION_MOVE, 0, 0, 180.0 * scale, y, 2_000 + i * 8);
    }
    feed(&mut rt, ACTION_UP, 0, 0, 180.0 * scale, 180.0 * scale, 2_044);
    rt.frame(1.0 / 60.0);
    let lifted = rt.scroll.scrolls[&list_id].offset;
    assert!(lifted >= 100.0, "list followed finger during batched move: {lifted}");
    for _ in 0..20 {
        rt.frame(1.0 / 60.0);
    }
    assert!(
        rt.scroll.scrolls[&list_id].offset > lifted + 40.0,
        "list coasted on momentum after lift: {} vs {lifted}",
        rt.scroll.scrolls[&list_id].offset
    );
}
