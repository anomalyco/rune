//! A growth row's public kind switch, as an app uses it: `whole` asks for the spring alone, with
//! none of the stack's speed caps.

use rune::growth::GrowTrack;
use rune::{GROWTH, edge};

/// A whole row takes the spring's own speed, so it opens far past the carrying-tail cap that
/// holds a capped row back, and still lands exactly.
#[test]
fn an_app_asks_for_an_uncapped_whole_glide() {
    let mut t = GrowTrack::settled(0.0);
    t.whole = true;
    t.retarget(900.0);
    let (mut prev, mut fastest) = (0.0, 0.0f32);
    for _ in 0..(2.0 / rune::growth::SUBSTEP) as usize {
        t.step(GROWTH, &edge::FEATHER, true);
        fastest = fastest.max((t.h - prev) / rune::growth::SUBSTEP);
        prev = t.h;
    }
    // A capped row carrying a tail is held to 7.5 × 60 pt/s; a whole glide peaks far above it.
    assert!(fastest > 7.5 * 60.0, "the whole glide keeps the spring's speed: {fastest} pt/s at peak");
    assert!(!t.moving() && (t.h - 900.0).abs() < 1e-3, "and lands on its target: {} pt", t.h);
}
