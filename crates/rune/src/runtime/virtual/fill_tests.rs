//! The fill machine against origin/dev's counter, and the wait that resumes a starved fill.
use super::{FILL_FRAMES, Fill, FillEffect, FillEvent, RESUME_LIMIT, State};
use crate::machine::Attempt;

/// origin/dev's `Runtime::virtual_filling`, transcribed as it was: `Runtime.virtual_fill: u32`
/// counted one more for each frame that found an index-built list wanting its window and reset
/// to zero for each frame that found none, and it answered `filling && virtual_fill <= 12` —
/// `VIRTUAL_FILL_FRAMES` at `runtime/virtual/window.rs:8`. It is never edited to fit the branch:
/// a walk that fails means the branch differs from dev.
struct Dev {
    virtual_fill: u32,
}

impl Dev {
    fn frame(&mut self, filling: bool) -> bool {
        self.virtual_fill = if filling { self.virtual_fill + 1 } else { 0 };
        filling && self.virtual_fill <= 12
    }
}

fn sequence(bits: u32, len: usize) -> String {
    (0..len).map(|i| if bits >> i & 1 == 1 { 'F' } else { '.' }).collect()
}

/// Runs `fill` to the bound with `missing` rows unfilled, returning the wait it asked for.
fn run_to_bound(fill: &mut Fill, now: f64, missing: usize) -> Option<(f64, Attempt)> {
    for _ in 0..FILL_FRAMES + 2 {
        let count = if fill.at_bound() { missing } else { 0 };
        for effect in fill.step(FillEvent::Frame { now, wanted: true, missing: count }) {
            if let FillEffect::WakeAt(due) = effect {
                return fill.due(due).map(|attempt| (due, attempt));
            }
        }
        if fill.state() == State::Idle { return None; }
    }
    None
}

/// Every sequence of frames up to one past the bound, each driven through `step` and its answer
/// compared with dev's after every frame. Past the bound the branch departs from dev on purpose
/// (see the tests below), so only the Draw answer is compared here.
#[test]
fn every_run_of_frames_draws_what_dev_draws() {
    let longest = FILL_FRAMES as usize + 1;
    for len in 1..=longest {
        for bits in 0..1u32 << len {
            let walk = sequence(bits, len);
            let (mut fill, mut dev) = (Fill::default(), Dev { virtual_fill: 0 });
            for (i, wanted) in (0..len).map(|i| (i, bits >> i & 1 == 1)) {
                let missing = if fill.at_bound() { 1 } else { 0 };
                let drew = fill.step(FillEvent::Frame { now: 0.0, wanted, missing }).into_iter().any(|e| e == FillEffect::Draw);
                assert_eq!(
                    drew,
                    dev.frame(wanted),
                    "frame {i} of {walk}: wanted {wanted}, state {:?}",
                    fill.state()
                );
            }
        }
    }
}

/// One frame's answer is all the runtime keeps of the fill, so a settled list asks again at once,
/// while a run keeps asking until the bound and not after it.
#[test]
fn a_settled_frame_forgets_the_run_and_the_bound_stops_it() {
    let mut fill = Fill::default();
    let frame = |fill: &mut Fill, wanted: bool| {
        let missing = if fill.at_bound() { 8 } else { 0 };
        fill.step(FillEvent::Frame { now: 0.0, wanted, missing }).into_iter().any(|e| e == FillEffect::Draw)
    };
    let drew: Vec<bool> = (0..FILL_FRAMES as usize + 1).map(|_| frame(&mut fill, true)).collect();
    assert!(drew[..FILL_FRAMES as usize].iter().all(|d| *d), "the first {FILL_FRAMES} frames draw: {drew:?}");
    assert!(!drew[FILL_FRAMES as usize], "the bound stops drawing: {drew:?}");
    // // differs from dev: the bound leaves a wait behind, not a spent count.
    assert!(matches!(fill.state(), State::Waiting { .. }), "the bound waits: {:?}", fill.state());
    assert!(fill.step(FillEvent::Frame { now: 0.0, wanted: false, missing: 0 }).is_empty());
    assert_eq!(fill.state(), State::Idle, "a frame with nothing to fill settles it");
    assert!(frame(&mut fill, true), "a settled list asks again at once");
}

/// A fill that hits its bound with rows still unfilled waits, the wake resumes it, and a frame
/// drawn for another cause retires the wait so its wake does nothing.
#[test]
fn the_bound_waits_and_the_wake_resumes_the_fill() {
    let mut fill = Fill::default();
    let (due, attempt) = run_to_bound(&mut fill, 1.0, 4).expect("the bound asks for a wake");
    assert!(matches!(fill.state(), State::Waiting { .. }));
    assert!(!fill.resuming(due - 0.001), "nothing resumes before the wake is due");
    assert!(fill.resuming(due));

    // A stale wake: an attempt the state never issued changes nothing.
    let stale = fill.step(FillEvent::Due { attempt: Attempt(999), now: 2.0 });
    assert!(stale.is_empty(), "a stale wake does nothing: {stale:?}");
    assert!(matches!(fill.state(), State::Waiting { .. }));

    // A frame for another cause (a scroll) retires the wait; the wake it left does nothing.
    assert!(!fill.step(FillEvent::Frame { now: 1.5, wanted: true, missing: 4 }).is_empty());
    assert!(matches!(fill.state(), State::Filling { .. }), "the frame starts a fresh run: {:?}", fill.state());
    assert!(fill.step(FillEvent::Due { attempt, now: due }).is_empty(), "the retired wake resumes nothing");
}

/// The wake resumes the fill where the turn's own frames left it.
#[test]
fn the_wake_resumes_the_fill() {
    let mut fill = Fill::default();
    let (due, attempt) = run_to_bound(&mut fill, 0.0, 2).expect("the bound asks for a wake");
    let effects = fill.step(FillEvent::Due { attempt, now: due });
    assert!(effects.into_iter().any(|e| e == FillEffect::Draw), "the wake draws again");
    assert!(matches!(fill.state(), State::Filling { frames: 1, .. }), "{:?}", fill.state());
}

/// A list whose heights never settle resumes with a longer wait each time, then stops after
/// [`RESUME_LIMIT`] resumes without the unfilled range shrinking.
#[test]
fn a_starved_fill_stops_after_its_resumes_without_progress() {
    let mut fill = Fill::default();
    let mut now = 0.0;
    let mut waits = Vec::new();
    while let Some((due, attempt)) = run_to_bound(&mut fill, now, 8) {
        waits.push(due - now);
        now = due;
        assert!(waits.len() <= RESUME_LIMIT as usize, "resumes are bounded: {waits:?}");
        assert!(!fill.step(FillEvent::Due { attempt, now }).is_empty(), "the wake resumes it");
    }
    assert_eq!(waits.len(), RESUME_LIMIT as usize, "resumes are bounded: {waits:?}");
    assert!(waits.windows(2).all(|w| w[1] > w[0]), "each resume backs off: {waits:?}");
    assert_eq!(fill.state(), State::Idle, "and the fill stops asking");
    assert!(!fill.resuming(now + 100.0), "nothing is left armed");
}

/// A resume that finds the unfilled range smaller counts as progress and resets the bound, so a
/// slow fill that is still shrinking keeps going.
#[test]
fn a_resume_that_finds_progress_gets_a_fresh_bound() {
    let mut fill = Fill::default();
    let mut now = 0.0;
    let mut missing = 40;
    let mut wakes = 0;
    while let Some((due, attempt)) = run_to_bound(&mut fill, now, missing) {
        wakes += 1;
        now = due;
        missing = missing.saturating_sub(6);
        fill.step(FillEvent::Due { attempt, now });
        assert!(wakes <= 8, "progress keeps it going: {wakes}");
    }
    assert!(wakes > RESUME_LIMIT as usize, "the shrinking range earned more resumes: {wakes}");
}
