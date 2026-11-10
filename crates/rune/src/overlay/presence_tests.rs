use std::time::Duration;

use super::{Effect, Event, Presence, Release};
use crate::time::Instant;

const FADE: Duration = Duration::from_millis(200);
const AFTER: Duration = Duration::from_secs(5);

fn effects(presence: &mut Presence, event: Event, release: Release) -> Vec<Effect> {
    presence.step(event, release).into_iter().collect()
}

fn shown() -> Presence { Presence::Shown { occluded: false } }

#[test]
fn a_fade_out_keeps_drawing_without_input_until_it_ends() {
    let t0 = Instant::now();
    let mut p = shown();
    assert_eq!(effects(&mut p, Event::Hide { fade: FADE, now: t0 }, Release::Now), [Effect::HideWindow { fade: FADE }],
        "nothing stops and nothing is released while the exit plays");
    assert!(!p.shown() && p.drawing() && p.fading());
    assert_eq!(p.deadline(), Some(t0 + FADE));
    assert!(p.step(Event::Due { now: t0 + FADE / 2 }, Release::Now).is_empty());
    assert_eq!(effects(&mut p, Event::Due { now: t0 + FADE }, Release::Now),
        [Effect::StopDrawing, Effect::HideWindow { fade: Duration::ZERO }, Effect::ReleaseGpu]);
    assert_eq!(p, Presence::Hidden { release_at: None });
}

#[test]
fn hiding_at_once_stops_drawing_then_releases_by_policy() {
    let t0 = Instant::now();
    let hide = Event::Hide { fade: Duration::ZERO, now: t0 };
    let mut keep = shown();
    assert_eq!(effects(&mut keep, hide, Release::Keep), [Effect::StopDrawing, Effect::HideWindow { fade: Duration::ZERO }]);
    assert_eq!(keep, Presence::Hidden { release_at: None });
    let mut later = shown();
    assert_eq!(effects(&mut later, hide, Release::After(AFTER)), [Effect::StopDrawing, Effect::HideWindow { fade: Duration::ZERO }]);
    assert_eq!(later, Presence::Hidden { release_at: Some(t0 + AFTER) });
    assert!(later.step(Event::Due { now: t0 + AFTER / 2 }, Release::After(AFTER)).is_empty());
    assert_eq!(effects(&mut later, Event::Due { now: t0 + AFTER }, Release::After(AFTER)), [Effect::ReleaseGpu]);
    assert_eq!(later.deadline(), None, "released once");
}

#[test]
fn showing_cancels_a_fade_and_its_end_is_stale() {
    let t0 = Instant::now();
    let mut p = shown();
    p.step(Event::Hide { fade: FADE, now: t0 }, Release::Now);
    assert!(p.step(Event::Show, Release::Now).is_empty());
    assert_eq!(p, shown());
    assert!(p.step(Event::Due { now: t0 + FADE * 2 }, Release::Now).is_empty(), "the cancelled fade's end hides nothing");
    assert!(p.shown());
}

#[test]
fn showing_cancels_a_pending_release_and_its_time_is_stale() {
    let t0 = Instant::now();
    let release = Release::After(AFTER);
    let mut p = shown();
    p.step(Event::Hide { fade: Duration::ZERO, now: t0 }, release);
    p.step(Event::Show, release);
    assert_eq!(p.deadline(), None);
    let t1 = t0 + AFTER / 2;
    p.step(Event::Hide { fade: Duration::ZERO, now: t1 }, release);
    assert!(p.step(Event::Due { now: t0 + AFTER }, release).is_empty(), "the first hide's release time is past for nothing");
    assert_eq!(effects(&mut p, Event::Due { now: t1 + AFTER }, release), [Effect::ReleaseGpu]);
}

#[test]
fn fading_again_restarts_the_fade() {
    let t0 = Instant::now();
    let mut p = shown();
    p.step(Event::Hide { fade: FADE, now: t0 }, Release::Keep);
    let t1 = t0 + FADE / 2;
    assert_eq!(effects(&mut p, Event::Hide { fade: FADE * 2, now: t1 }, Release::Keep), [Effect::HideWindow { fade: FADE * 2 }]);
    assert_eq!(p, Presence::FadingOut { until: t1 + FADE * 2, length: FADE * 2, occluded: false });
    assert!(p.step(Event::Due { now: t0 + FADE }, Release::Keep).is_empty(), "the first fade's end is stale");
}

#[test]
fn hiding_mid_fade_without_one_ends_it_now() {
    let t0 = Instant::now();
    let mut p = shown();
    p.step(Event::Hide { fade: FADE, now: t0 }, Release::After(AFTER));
    let t1 = t0 + FADE / 2;
    assert_eq!(effects(&mut p, Event::Hide { fade: Duration::ZERO, now: t1 }, Release::After(AFTER)),
        [Effect::StopDrawing, Effect::HideWindow { fade: Duration::ZERO }]);
    assert_eq!(p, Presence::Hidden { release_at: Some(t1 + AFTER) });
}

#[test]
fn hiding_a_hidden_overlay_leaves_its_release_alone() {
    let t0 = Instant::now();
    let mut p = Presence::Hidden { release_at: Some(t0 + AFTER) };
    assert_eq!(effects(&mut p, Event::Hide { fade: FADE, now: t0 + AFTER / 2 }, Release::After(AFTER)),
        [Effect::StopDrawing, Effect::HideWindow { fade: FADE }], "nothing to fade, and no new release");
    assert_eq!(p, Presence::Hidden { release_at: Some(t0 + AFTER) });
    let mut never = Presence::default();
    assert_eq!(effects(&mut never, Event::Hide { fade: Duration::ZERO, now: t0 }, Release::Now),
        [Effect::StopDrawing, Effect::HideWindow { fade: Duration::ZERO }], "a never-shown overlay keeps its prewarmed surface");
}

#[test]
fn a_fading_panel_never_becomes_covered_but_one_that_was_stays_so() {
    let t0 = Instant::now();
    let mut p = shown();
    p.step(Event::Hide { fade: FADE, now: t0 }, Release::Keep);
    assert!(!p.set_occluded(true) && !p.occluded(), "a panel fading toward alpha zero isn't occluded");
    let mut covered = Presence::Shown { occluded: true };
    covered.step(Event::Hide { fade: FADE, now: t0 }, Release::Keep);
    assert!(covered.occluded());
    assert!(covered.set_occluded(false) && !covered.occluded(), "uncovered mid-fade, it draws its exit");
}

#[test]
fn a_hidden_overlay_keeps_no_occlusion_and_a_show_reads_it_again() {
    let mut p = Presence::default();
    assert!(p.set_occluded(true), "the reading still applies its side effects");
    assert!(!p.occluded());
    let mut covered = Presence::Shown { occluded: true };
    covered.step(Event::Hide { fade: Duration::ZERO, now: Instant::now() }, Release::Keep);
    covered.step(Event::Show, Release::Keep);
    assert_eq!(covered, shown());
}

#[test]
fn release_follows_the_platform() {
    assert_eq!(Release::new(None), Release::Keep);
    assert_eq!(Release::new(Some(Duration::ZERO)), Release::Now);
    let after = Release::new(Some(AFTER));
    if cfg!(target_os = "macos") { assert_eq!(after, Release::Keep) } else { assert_eq!(after, Release::After(AFTER)) }
}
