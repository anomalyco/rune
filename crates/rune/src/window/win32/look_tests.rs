use super::{AGAIN, AGAIN_MAX, FRESH, Look, LookEffect, LookEvent};
use crate::window::state::{Cover, Life};

#[test]
fn hidden_window_drops_covered_look_wake() {
    let t0 = crate::time::now();
    let mut life = Life::Unopened;
    assert!(life.show());
    let mut look = Look::default();

    // A shown window with a running animation loop is covered by another window.
    let (next, effect) = look.step(LookEvent::Frame { now: t0 });
    assert_eq!(effect, LookEffect::Look);
    let (next, effect) = next.step(LookEvent::Found { seen: false, moving: true, now: t0 });
    assert_eq!(effect, LookEffect::None);
    look = next;
    life.cover(Cover::Windows, true);
    assert!(life.covered());
    assert_eq!(look.deadline(life), Some(t0 + AGAIN), "covered moving window arms a re-look wake");

    // Hiding the window leaves `Life::Hidden` (`!life.covered()`). Even if `Look` still holds
    // `Look::Covered { again: Some(..) }`, `deadline` must return `None` so `Host::wake_deadline`
    // never arms an expired `WaitUntil` that `new_events` won't consume.
    life.hide(|| 1);
    assert!(!life.covered());
    assert_eq!(look.deadline(life), None, "a hidden window must not arm a covered look wake");

    // Uncovering via `show()` without a prior look also clears the covered wake immediately,
    // and non-shown states never arm one either.
    let mut uncovered = life;
    assert!(uncovered.show());
    assert_eq!(look.deadline(uncovered), None, "an uncovered window must not arm a covered look wake");
    assert_eq!(look.deadline(Life::Unopened), None);
    let mut finished = uncovered;
    finished.finish();
    assert_eq!(look.deadline(finished), None);
}

#[test]
fn covered_look_backs_off_and_stale_resets_wait() {
    let t0 = crate::time::now();
    let mut life = Life::Unopened;
    life.show();
    life.cover(Cover::Windows, true);

    let (look, _) = Look::Due.step(LookEvent::Found { seen: false, moving: true, now: t0 });
    let t1 = t0 + AGAIN;
    assert_eq!(look.deadline(life), Some(t1));

    // When the wake fires at `t1`, stepping `Frame` asks to look again and doubles the wait.
    let (look, effect) = look.step(LookEvent::Frame { now: t1 });
    assert_eq!(effect, LookEffect::Look);
    let (look, _) = look.step(LookEvent::Found { seen: false, moving: true, now: t1 });
    let t2 = t1 + AGAIN * 2;
    assert_eq!(look.deadline(life), Some(t2));

    // Further covered looks cap at `AGAIN_MAX`.
    let (look, _) = look.step(LookEvent::Found { seen: false, moving: true, now: t2 });
    let (look, _) = look.step(LookEvent::Found { seen: false, moving: true, now: t2 + AGAIN_MAX });
    assert_eq!(look.deadline(life), Some(t2 + AGAIN_MAX + AGAIN_MAX));

    // Marking stale (as `Host::wind_down` does on hide) clears the deadline and resets the wait.
    let (look, effect) = look.step(LookEvent::Stale);
    assert_eq!((look, effect), (Look::Due, LookEffect::None));
    assert_eq!(look.deadline(life), None);
    let t3 = t2 + AGAIN_MAX * 2;
    let (look, _) = look.step(LookEvent::Found { seen: false, moving: true, now: t3 });
    assert_eq!(look.deadline(life), Some(t3 + AGAIN), "after stale, re-look wait starts at AGAIN");

    // A covered window with no running animation loop arms no wake at all.
    let (idle, _) = look.step(LookEvent::Found { seen: false, moving: false, now: t3 });
    assert_eq!(idle.deadline(life), None);

    // Once seen again, no wake is armed and frames within `FRESH` skip looking.
    let (seen, _) = look.step(LookEvent::Found { seen: true, moving: true, now: t3 });
    assert_eq!(seen.deadline(life), None);
    assert_eq!(seen.step(LookEvent::Frame { now: t3 + FRESH / 2 }).1, LookEffect::None);
    assert_eq!(seen.step(LookEvent::Frame { now: t3 + FRESH }).1, LookEffect::Look);
}
