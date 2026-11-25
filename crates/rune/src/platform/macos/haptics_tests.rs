use std::cell::RefCell;

use objc2_app_kit::{NSHapticFeedbackPattern, NSHapticFeedbackPerformanceTime};

use super::*;
use crate::{Env, Runtime, el};

#[derive(Default)]
struct FakePerformer {
    calls: RefCell<Vec<(NSHapticFeedbackPattern, NSHapticFeedbackPerformanceTime)>>,
}

impl HapticPerformer for FakePerformer {
    fn perform_pattern(
        &self,
        pattern: NSHapticFeedbackPattern,
        time: NSHapticFeedbackPerformanceTime,
    ) {
        self.calls.borrow_mut().push((pattern, time));
    }
}

#[test]
fn macos_pattern_mapping_uses_performance_time_now_and_silences_notifications() {
    let fake = FakePerformer::default();

    for haptic in [
        Haptic::Selection,
        Haptic::Impact(ImpactStrength::Light),
        Haptic::Impact(ImpactStrength::Soft),
        Haptic::Impact(ImpactStrength::Medium),
        Haptic::Impact(ImpactStrength::Heavy),
        Haptic::Impact(ImpactStrength::Rigid),
        Haptic::Notify(HapticOutcome::Success),
        Haptic::Notify(HapticOutcome::Warning),
        Haptic::Notify(HapticOutcome::Error),
    ] {
        perform_with(&fake, haptic);
    }

    let now = NSHapticFeedbackPerformanceTime::Now;
    assert_eq!(
        *fake.calls.borrow(),
        vec![
            (NSHapticFeedbackPattern::LevelChange, now),
            (NSHapticFeedbackPattern::Alignment, now),
            (NSHapticFeedbackPattern::Alignment, now),
            (NSHapticFeedbackPattern::Generic, now),
            (NSHapticFeedbackPattern::Generic, now),
            (NSHapticFeedbackPattern::Generic, now),
        ],
        "Selection -> LevelChange, Light/Soft -> Alignment, Medium/Heavy/Rigid -> Generic, Notify -> silent no-op",
    );
}

#[test]
fn macos_visible_settle_dispatches_to_performer_and_covered_frame_discards() {
    let fake = FakePerformer::default();
    let mut rt = Runtime::new((), |_, _| el().size(20.0, 20.0), (20.0, 20.0), Env::default());
    rt.host.open_urls_natively = true;
    rt.frame(0.0);

    // Visible live settle / on_unchanged dispatches to the performer seam.
    rt.update(|_, tx| {
        tx.haptic(Haptic::Selection);
        tx.haptic(Haptic::Impact(ImpactStrength::Light));
        tx.haptic(Haptic::Notify(HapticOutcome::Success));
    });
    crate::haptics::flush_with(rt.core_mut(), true, |h| perform_with(&fake, h));
    assert_eq!(
        *fake.calls.borrow(),
        vec![
            (NSHapticFeedbackPattern::LevelChange, NSHapticFeedbackPerformanceTime::Now),
            (NSHapticFeedbackPattern::Alignment, NSHapticFeedbackPerformanceTime::Now),
        ],
    );
    assert!(rt.take_haptics().is_empty(), "visible flush drains queue");

    // Covered / hidden frame discards pending haptics without calling the performer.
    fake.calls.borrow_mut().clear();
    rt.update(|_, tx| tx.haptic(Haptic::Impact(ImpactStrength::Heavy)));
    crate::haptics::flush_with(rt.core_mut(), false, |h| perform_with(&fake, h));
    assert!(fake.calls.borrow().is_empty(), "covered/hidden frame must not trigger trackpad feedback");
    assert!(rt.take_haptics().is_empty(), "covered/hidden frame discards queued haptics");
}
