use super::*;

const TASK_UNSPECIFIED: i32 = 0;
const TASK_BACKGROUND_APPLICATION: i32 = 2;

/// Only an app Launch Services started and the system hasn't placed asks for the foreground
/// role: a child process (no role), an app already placed, and a launch that stays in the
/// background keep theirs.
#[test]
fn only_an_unplaced_app_that_comes_to_the_front_raises_its_role() {
    assert!(raises(Some(TASK_DEFAULT_APPLICATION), true));
    assert!(!raises(Some(TASK_DEFAULT_APPLICATION), false));
    assert!(!raises(Some(TASK_UNSPECIFIED), true));
    assert!(!raises(Some(TASK_BACKGROUND_APPLICATION), true));
    assert!(!raises(Some(TASK_FOREGROUND_APPLICATION), true));
    assert!(!raises(None, true));
}

/// The raised role goes back only when the app isn't the active one after its launch and the
/// role is still the one Rune set: an active app keeps it, and a role the system has set since
/// (the app left the front) is the system's.
#[test]
fn the_role_goes_back_when_the_app_is_not_active_after_the_launch() {
    assert_eq!(gives_back(TASK_DEFAULT_APPLICATION, false, Some(TASK_FOREGROUND_APPLICATION)), Some(TASK_DEFAULT_APPLICATION));
    assert_eq!(gives_back(TASK_DEFAULT_APPLICATION, true, Some(TASK_FOREGROUND_APPLICATION)), None);
    assert_eq!(gives_back(TASK_DEFAULT_APPLICATION, false, Some(TASK_BACKGROUND_APPLICATION)), None);
    assert_eq!(gives_back(TASK_DEFAULT_APPLICATION, false, None), None);
}

/// macOS answers a task about its own role and lets it move between the roles of an app (the
/// public `task_policy_get` / `task_policy_set` with `TASK_CATEGORY_POLICY`): default, the
/// foreground, and default again, which is the way `begin` and `settle` go. It refuses "no
/// role" (a test process's own), so the role is never raised from anything but default.
#[test]
fn a_task_reads_and_sets_its_own_role() {
    assert!(role().is_some());
    assert!(set_role(TASK_DEFAULT_APPLICATION));
    assert_eq!(role(), Some(TASK_DEFAULT_APPLICATION));
    assert!(set_role(TASK_FOREGROUND_APPLICATION));
    assert_eq!(role(), Some(TASK_FOREGROUND_APPLICATION));
    assert!(set_role(TASK_DEFAULT_APPLICATION));
    assert_eq!(role(), Some(TASK_DEFAULT_APPLICATION));
    assert!(!set_role(TASK_UNSPECIFIED));
}

/// A test process never took the role, so a loop turn has nothing to give back.
#[test]
fn settle_does_nothing_when_no_role_was_raised() {
    settle();
    assert_eq!(RAISED_FROM.load(Ordering::Relaxed), UNRAISED);
}

/// AppKit reads the default before it looks for saved window state: once the launch has
/// registered it, the app's defaults answer yes, and a window server session isn't needed to ask.
#[test]
fn the_launch_ignores_appkits_saved_windows() {
    use objc2_foundation::{NSString, NSUserDefaults};
    ignore_saved_windows();
    assert!(NSUserDefaults::standardUserDefaults().boolForKey(&NSString::from_str(IGNORE_SAVED_WINDOWS)));
}
