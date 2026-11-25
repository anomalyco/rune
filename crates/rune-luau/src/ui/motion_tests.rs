//! Tests for `enter`/`exit` strings.

use rune::{Effect, Spring, Vec2};

use super::{Presence, parse};

/// The effect and spring of an animated side.
fn animated(s: &str, exit: bool) -> (Effect, Option<Spring>) {
    match parse(s, exit).unwrap() {
        Presence::Animated { effect, spring } => (effect, spring),
        Presence::None => panic!("{s:?} parsed as none"),
    }
}

#[test]
fn parts_combine_with_their_amounts() {
    let (effect, spring) = animated("fade slide-up:24", false);
    assert_eq!(effect.opacity, 0.0);
    assert_eq!(effect.offset, Vec2::new(0.0, 24.0), "an entrance sliding up starts below");
    assert_eq!(spring, None);

    let (effect, _) = animated("slide-up:24", true);
    assert_eq!(effect.offset, Vec2::new(0.0, -24.0), "an exit sliding up ends above");

    let (effect, _) = animated("slide-right", false);
    assert_eq!(effect.offset, Vec2::new(-8.0, 0.0));

    let (effect, _) = animated("scale:0.9 fade:0.5 blur", false);
    assert_eq!((effect.scale, effect.opacity, effect.blur), (0.9, 0.5, 1.5));
}

#[test]
fn spring_times_the_side() {
    assert_eq!(animated("fade spring:0.2", false).1, Some(Spring::new(0.2, 0.0)));
    assert_eq!(animated("spring:0.4:0.3 scale", true).1, Some(Spring::new(0.4, 0.3)));
    assert_eq!(parse(" none ", false).unwrap(), Presence::None);
}

#[test]
fn bad_strings_say_what_is_wrong() {
    for (s, wanted) in [
        ("", "at least one part"),
        ("wobble", "unknown part"),
        ("slide-up:far", "takes a number"),
        ("fade:1:2", "one amount"),
        ("spring", "spring:<secs>"),
        ("spring:0", "spring:<secs>"),
        ("spring:0.3:1", "spring:<secs>"),
        ("spring:0.3:0.1:2", "spring:<secs>"),
        ("scale:-0.5", "scale must be above 0"),
        ("scale:0", "scale must be above 0"),
        ("fade none", "none can't be combined"),
        ("none spring:0.3", "none can't be combined"),
        ("spring:0.3 none", "none can't be combined"),
    ] {
        let err = parse(s, false).unwrap_err();
        assert!(err.starts_with("enter") && err.contains(wanted), "{s:?}: {err}");
    }
}
