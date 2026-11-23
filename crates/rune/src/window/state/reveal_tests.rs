use super::{Found, Reveal, RevealEffect::*};

fn effects(reveal: Reveal, found: Found) -> Vec<super::RevealEffect> { reveal.effects(found).into_iter().collect() }

const HIDDEN: Found = Found::Hidden { minimized: false };
const SHOWN: Found = Found::Shown { covered: false, minimized: false };
const COVERED: Found = Found::Shown { covered: true, minimized: false };
const MINIMIZED: Found = Found::Shown { covered: false, minimized: true };

#[test]
fn show_and_open_bring_a_hidden_window_back_alike_but_for_focus() {
    let common = [ReadSystem, Build, Place, OrderIn { focus: false }, Settle, Shown { live: true }, CancelTimerFrame, Number];
    assert_eq!(effects(Reveal::Show, HIDDEN), [&common[..], &[Redraw]].concat());
    assert_eq!(effects(Reveal::Open, HIDDEN), [&common[..], &[Focus, Redraw]].concat());
}

#[test]
fn an_open_reads_the_system_style_before_it_builds() {
    // A hidden window may have missed a new accent's or scroller style's redraw. `show` re-read
    // them before building what changed; `open` built with the stale ones.
    for found in [HIDDEN, MINIMIZED, Found::Missing] {
        let steps = effects(Reveal::Open, found);
        assert_eq!(steps.first(), Some(&ReadSystem), "{found:?}");
    }
}

#[test]
fn a_cached_reveal_cancels_the_timer_frame_as_every_show_does() {
    assert_eq!(effects(Reveal::Cached, HIDDEN), [ApplyFrame, Place, OrderIn { focus: true }, Shown { live: true }, CancelTimerFrame, Number, RedrawIfMoving]);
    assert_eq!(effects(Reveal::Cached, SHOWN), [OrderIn { focus: true }, Shown { live: true }, CancelTimerFrame, Number, RedrawIfMoving]);
    for reveal in [Reveal::Show, Reveal::Open] {
        assert!(effects(reveal, SHOWN).contains(&CancelTimerFrame), "{reveal:?}");
    }
    assert!(effects(Reveal::Cached, Found::Missing).is_empty(), "a launcher reveal never creates");
}

#[test]
fn a_hidden_cached_reveal_applies_pending_frame_and_placement_before_ordering_in() {
    let steps = effects(Reveal::Cached, HIDDEN);
    let apply = steps.iter().position(|e| *e == ApplyFrame).expect("applies pending window_frame");
    let place = steps.iter().position(|e| *e == Place).expect("re-places at the new height");
    let order = steps.iter().position(|e| *e == OrderIn { focus: true }).expect("orders in");
    assert!(apply < place && place < order, "frame and placement precede order-in: {steps:?}");
    assert!(!effects(Reveal::Cached, SHOWN).contains(&ApplyFrame), "an already-shown launcher keeps its frame animation");
}

#[test]
fn showing_a_covered_window_settles_however_it_is_asked() {
    // Its occlusion event then finds it uncovered already and doesn't settle.
    for reveal in [Reveal::Show, Reveal::Open] {
        assert!(effects(reveal, COVERED).contains(&Settle), "{reveal:?}");
    }
}

#[test]
fn showing_a_window_already_on_screen_lands_no_motion() {
    for reveal in [Reveal::Show, Reveal::Open] {
        let steps = effects(reveal, SHOWN);
        assert!(!steps.contains(&Settle) && !steps.contains(&Build) && !steps.iter().any(|e| matches!(e, OrderIn { .. })), "{reveal:?}");
    }
}

#[test]
fn only_open_restores_a_minimized_window() {
    assert_eq!(effects(Reveal::Open, MINIMIZED),
        [ReadSystem, Build, Place, Unminimize, Settle, Shown { live: true }, CancelTimerFrame, Number, Focus, Redraw]);
    let show = effects(Reveal::Show, MINIMIZED);
    assert!(!show.contains(&Unminimize) && !show.contains(&Settle));
    assert!(show.contains(&Shown { live: cfg!(target_os = "macos") }), "it stays minimised, and isn't live but on macOS");
}

#[test]
fn a_missing_window_is_created() {
    assert_eq!(effects(Reveal::Show, Found::Missing), [ReadSystem, Create]);
    assert_eq!(effects(Reveal::Open, Found::Missing), [ReadSystem, Create]);
}
