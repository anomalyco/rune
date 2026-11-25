//! macOS Force Touch trackpad haptic feedback via `NSHapticFeedbackManager`.

use objc2_app_kit::{
    NSHapticFeedbackManager, NSHapticFeedbackPattern, NSHapticFeedbackPerformanceTime,
    NSHapticFeedbackPerformer,
};

use crate::{Haptic, HapticOutcome, ImpactStrength};

#[cfg(test)]
#[path = "haptics_tests.rs"]
mod tests;

/// Small seam over `NSHapticFeedbackPerformer` so unit tests verify pattern mapping,
/// `PerformanceTime::Now`, silent notification no-ops and visible-vs-covered dispatch without
/// opening a window or touching trackpad hardware.
pub(super) trait HapticPerformer {
    fn perform_pattern(
        &self,
        pattern: NSHapticFeedbackPattern,
        time: NSHapticFeedbackPerformanceTime,
    );
}

struct DefaultPerformer;

impl HapticPerformer for DefaultPerformer {
    fn perform_pattern(
        &self,
        pattern: NSHapticFeedbackPattern,
        time: NSHapticFeedbackPerformanceTime,
    ) {
        NSHapticFeedbackManager::defaultPerformer()
            .performFeedbackPattern_performanceTime(pattern, time);
    }
}

/// Maps Rune's semantic [`Haptic`] to AppKit's `NSHapticFeedbackPattern`, or `None` for
/// notifications (AppKit has no notification patterns, matching SwiftUI on macOS).
pub(super) fn pattern_for(haptic: Haptic) -> Option<NSHapticFeedbackPattern> {
    match haptic {
        Haptic::Selection => Some(NSHapticFeedbackPattern::LevelChange),
        Haptic::Impact(ImpactStrength::Light | ImpactStrength::Soft) => {
            Some(NSHapticFeedbackPattern::Alignment)
        }
        Haptic::Impact(
            ImpactStrength::Medium | ImpactStrength::Heavy | ImpactStrength::Rigid,
        ) => Some(NSHapticFeedbackPattern::Generic),
        Haptic::Notify(
            HapticOutcome::Success | HapticOutcome::Warning | HapticOutcome::Error,
        ) => None,
    }
}

pub(super) fn perform_with(performer: &impl HapticPerformer, haptic: Haptic) {
    if let Some(pattern) = pattern_for(haptic) {
        performer.perform_pattern(pattern, NSHapticFeedbackPerformanceTime::Now);
    }
}

/// Plays `haptic` immediately on macOS's default `NSHapticFeedbackPerformer` (Force Touch
/// trackpad when active; silent no-op otherwise).
pub(crate) fn perform(haptic: Haptic) {
    perform_with(&DefaultPerformer, haptic);
}
