use super::*;
use crate::{Env, Runtime, el};

const ALL_HAPTICS: [Haptic; 9] = [
    Haptic::Selection,
    Haptic::Impact(ImpactStrength::Light),
    Haptic::Impact(ImpactStrength::Medium),
    Haptic::Impact(ImpactStrength::Heavy),
    Haptic::Impact(ImpactStrength::Soft),
    Haptic::Impact(ImpactStrength::Rigid),
    Haptic::Notify(HapticOutcome::Success),
    Haptic::Notify(HapticOutcome::Warning),
    Haptic::Notify(HapticOutcome::Error),
];

#[test]
fn android_haptic_constant_numeric_values_match_sdk() {
    assert_eq!(LONG_PRESS, 0);
    assert_eq!(VIRTUAL_KEY, 1);
    assert_eq!(KEYBOARD_TAP, 3);
    assert_eq!(CLOCK_TICK, 4);
    assert_eq!(CONTEXT_CLICK, 6);
    assert_eq!(CONFIRM, 16);
    assert_eq!(REJECT, 17);
    assert_eq!(SEGMENT_TICK, 26);
    assert_eq!(SEGMENT_FREQUENT_TICK, 27);
}

#[test]
fn android_haptic_mapping_respects_api_level_tiers() {
    let codes_at = |api: i32| ALL_HAPTICS.map(|h| feedback_constant(h, api));

    // Pre-API 23 fallback tier (e.g., API 21/22 or unknown 0).
    for api in [0, 21, 22] {
        assert_eq!(
            codes_at(api),
            [
                CLOCK_TICK,
                VIRTUAL_KEY,
                KEYBOARD_TAP,
                LONG_PRESS,
                CLOCK_TICK,
                KEYBOARD_TAP,
                VIRTUAL_KEY,
                KEYBOARD_TAP,
                LONG_PRESS,
            ],
            "API {api} must use pre-23 fallbacks",
        );
    }

    // API 23..=29: CONTEXT_CLICK available; CONFIRM/REJECT and SEGMENT_* fall back.
    for api in [23, 26, 29] {
        assert_eq!(
            codes_at(api),
            [
                CLOCK_TICK,
                VIRTUAL_KEY,
                KEYBOARD_TAP,
                CONTEXT_CLICK,
                CLOCK_TICK,
                CONTEXT_CLICK,
                VIRTUAL_KEY,
                KEYBOARD_TAP,
                LONG_PRESS,
            ],
            "API {api} must use CONTEXT_CLICK and pre-30/34 fallbacks",
        );
    }

    // API 30..=33: CONFIRM/REJECT available; SEGMENT_* still fall back to CLOCK_TICK.
    for api in [30, 33] {
        assert_eq!(
            codes_at(api),
            [
                CLOCK_TICK,
                VIRTUAL_KEY,
                KEYBOARD_TAP,
                CONTEXT_CLICK,
                CLOCK_TICK,
                CONTEXT_CLICK,
                CONFIRM,
                KEYBOARD_TAP,
                REJECT,
            ],
            "API {api} must use CONFIRM/REJECT and CLOCK_TICK fallback for segments",
        );
    }

    // API 34+: full modern constants including SEGMENT_TICK and SEGMENT_FREQUENT_TICK.
    for api in [34, 35, 36] {
        assert_eq!(
            codes_at(api),
            [
                SEGMENT_TICK,
                VIRTUAL_KEY,
                KEYBOARD_TAP,
                CONTEXT_CLICK,
                SEGMENT_FREQUENT_TICK,
                CONTEXT_CLICK,
                CONFIRM,
                KEYBOARD_TAP,
                REJECT,
            ],
            "API {api} must use full API 34+ HapticFeedbackConstants",
        );
    }
}

#[test]
fn android_haptic_flush_delivers_each_request_once_in_fifo_order() {
    let mut rt = Runtime::new(
        (),
        |_, _| {
            el().key("submit")
                .size(40.0, 40.0)
                .on_press_tx(|_: &mut (), tx| {
                    tx.haptic(Haptic::Notify(HapticOutcome::Success));
                    tx.haptic(Haptic::Selection);
                    tx.haptic(Haptic::Selection); // coalesced within 30 ms cooldown
                })
        },
        (40.0, 40.0),
        Env::default(),
    );
    rt.frame(0.0);

    rt.pointer_down(10.0, 10.0);
    rt.pointer_up(10.0, 10.0);
    rt.frame(1.0 / 60.0);

    let mut played = Vec::new();
    flush_with(rt.take_haptics(), 35, true, |code| played.push(code));
    assert_eq!(
        played,
        vec![CONFIRM, SEGMENT_TICK],
        "drawn frame must forward queued haptics in FIFO order with coalescing preserved",
    );

    // Next frame with no new requests must not replay anything.
    rt.frame(2.0 / 60.0);
    played.clear();
    flush_with(rt.take_haptics(), 35, true, |code| played.push(code));
    assert!(played.is_empty(), "a haptic request must reach the view once");
}

#[test]
fn android_haptic_flush_discards_when_frame_not_drawn() {
    let mut rt = Runtime::new((), |_, _| el().size(20.0, 20.0), (20.0, 20.0), Env::default());
    rt.frame(0.0);

    rt.update(|_, tx| {
        tx.haptic(Haptic::Impact(ImpactStrength::Heavy));
        tx.haptic(Haptic::Notify(HapticOutcome::Error));
    });

    let mut played = Vec::new();
    flush_with(rt.take_haptics(), 35, false, |code| played.push(code));
    assert!(played.is_empty(), "undrawn/missed frame must not invoke performHapticFeedback");
    assert!(
        rt.take_haptics().is_empty(),
        "undrawn frame drain must discard pending haptics so later frames do not replay them",
    );
}
