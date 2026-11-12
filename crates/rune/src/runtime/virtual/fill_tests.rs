//! The fill machine against origin/dev's counter, and the wait that resumes a starved fill.
use super::{FILL_FRAMES, Fill, FillEffect, FillEvent, RESUME_LIMIT, State};
use crate::machine::{Attempt, Effects};

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
        for effect in fill.step(FillEvent::Frame { now, wanted: true, missing: count, moved: false }) {
            if let FillEffect::WakeAt(due) = effect
                && let State::Waiting { attempt, .. } = fill.state()
            {
                return Some((due, attempt));
            }
        }
        if matches!(fill.state(), State::Idle | State::Stalled) { return None; }
    }
    None
}

/// One frame as `Runtime::virtual_filling` runs it: the fill's own deadline draws for itself, a
/// stale one draws nothing, and any other frame counts as a fill frame. Returns whether it drew.
fn frame(fill: &mut Fill, now: f64, missing: usize, deadlines: &mut Vec<f64>) -> bool {
    match fill.wake(now) {
        Some(attempt) => !fill.step(FillEvent::Due { attempt, now }).is_empty(),
        None => {
            let count = if fill.at_bound() { missing } else { 0 };
            let effects = fill.step(FillEvent::Frame { now, wanted: true, missing: count, moved: false });
            let drew = effects.clone().into_iter().any(|e| e == FillEffect::Draw);
            for effect in effects {
                if let FillEffect::WakeAt(due) = effect { deadlines.push(due); }
            }
            drew
        }
    }
}

/// Every sequence of frames up to two past the bound, each driven through `step` and its answer
/// compared with dev's after every frame. The wake is not a frame's answer, so the draw answers
/// still match dev exactly: past the bound both stop drawing until a frame finds nothing wanted.
#[test]
fn every_run_of_frames_draws_what_dev_draws() {
    let longest = FILL_FRAMES as usize + 2;
    for len in 1..=longest {
        for bits in 0..1u32 << len {
            let walk = sequence(bits, len);
            let (mut fill, mut dev) = (Fill::default(), Dev { virtual_fill: 0 });
            for (i, wanted) in (0..len).map(|i| (i, bits >> i & 1 == 1)) {
                let missing = if fill.at_bound() { 1 } else { 0 };
                let drew = fill.step(FillEvent::Frame { now: 0.0, wanted, missing, moved: false }).into_iter().any(|e| e == FillEffect::Draw);
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
        fill.step(FillEvent::Frame { now: 0.0, wanted, missing, moved: false }).into_iter().any(|e| e == FillEffect::Draw)
    };
    let drew: Vec<bool> = (0..FILL_FRAMES as usize + 1).map(|_| frame(&mut fill, true)).collect();
    assert!(drew[..FILL_FRAMES as usize].iter().all(|d| *d), "the first {FILL_FRAMES} frames draw: {drew:?}");
    assert!(!drew[FILL_FRAMES as usize], "the bound stops drawing: {drew:?}");
    // // differs from dev: the bound leaves a wait behind, not a spent count.
    assert!(matches!(fill.state(), State::Waiting { .. }), "the bound waits: {:?}", fill.state());
    assert!(fill.step(FillEvent::Frame { now: 0.0, wanted: false, missing: 0, moved: false }).is_empty());
    assert_eq!(fill.state(), State::Idle, "a frame with nothing to fill settles it");
    assert!(frame(&mut fill, true), "a settled list asks again at once");
}

/// A fill that hits its bound with rows still unfilled waits, and only the wake it holds resumes
/// it. A stale attempt, a pre-due frame and a redrawn frame all change nothing.
#[test]
fn the_bound_waits_and_the_wake_resumes_the_fill() {
    let mut fill = Fill::default();
    let (due, attempt) = run_to_bound(&mut fill, 1.0, 4).expect("the bound asks for a wake");
    let waiting = fill.state();
    assert!(matches!(waiting, State::Waiting { .. }));
    assert!(!fill.resuming(due - 0.001), "nothing resumes before the wake is due");
    assert!(fill.resuming(due));

    // A stale wake: an attempt the state never issued changes nothing.
    assert!(fill.step(FillEvent::Due { attempt: Attempt(999), now: 2.0 }).is_empty());
    assert_eq!(fill.state(), waiting);

    // A frame drawn for another cause while the fill waits: no draw, no reset.
    assert!(fill.step(FillEvent::Frame { now: 1.5, wanted: true, missing: 4, moved: false }).is_empty());
    assert_eq!(fill.state(), waiting, "a redraw changes nothing");

    // Its own wake resumes the run it was waiting in.
    let effects = fill.step(FillEvent::Due { attempt, now: due });
    assert!(effects.into_iter().any(|e| e == FillEffect::Draw), "the wake draws again");
    assert!(matches!(fill.state(), State::Filling { frames: 1, resumes: 1, .. }), "{:?}", fill.state());
}

/// A list whose heights never settle resumes with a longer wait each time, then stalls: nothing
/// after that asks for a frame, whatever else draws.
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
    assert_eq!(fill.state(), State::Stalled, "and the fill stops asking");
    assert!(!fill.resuming(now + 100.0), "nothing is left armed");
    // Redraws and stale deadlines after it change nothing.
    for _ in 0..5 {
        assert!(fill.step(FillEvent::Frame { now: now + 1.0, wanted: true, missing: 8, moved: false }).is_empty());
        assert!(fill.step(FillEvent::Due { attempt: Attempt(999), now: now + 1.0 }).is_empty());
    }
    assert_eq!(fill.state(), State::Stalled);
}

/// The reviewer's replay, 36,001 ticks at 359.94 Hz: wanted true with the same eight rows missing
/// forever, one frame per draw and one per deadline, and extra deadlines left in the set. The
/// fill must stop drawing within its bound and leave nothing armed.
#[test]
fn stale_deadlines_never_restart_the_fill() {
    const TICKS: usize = 36_001;
    let dt = 1.0 / 359.94;
    let mut fill = Fill::default();
    // Deadlines nobody's attempt stands for, as a retired wait leaves behind.
    let mut deadlines: Vec<f64> = (0..20).map(|i| dt * f64::from(i * 7 + 3)).collect();
    let (mut now, mut draw, mut frames, mut drawn) = (0.0, true, 0, 0);
    for _ in 0..TICKS {
        now += dt;
        let due = deadlines.iter().any(|d| *d <= now);
        if !draw && !due { continue; }
        frames += 1;
        deadlines.retain(|d| *d > now);
        draw = frame(&mut fill, now, 8, &mut deadlines);
        if draw { drawn += 1; }
    }
    assert!(drawn <= 4 * FILL_FRAMES as usize + 4, "the fill stopped drawing: {drawn} frames of {frames}");
    assert_eq!(fill.state(), State::Stalled, "and it stays stalled");
    assert!(deadlines.is_empty(), "no deadline is left armed: {deadlines:?}");
}

/// r367: a due wake after the list finished or unmounted draws nothing, and starts nothing.
#[test]
fn a_wake_after_the_list_is_gone_draws_nothing() {
    let mut fill = Fill::default();
    let (due, attempt) = run_to_bound(&mut fill, 0.0, 4).expect("the bound asks for a wake");
    assert!(matches!(fill.state(), State::Waiting { .. }));

    // The list finishes or unmounts: the wait retires with it, and its deadline is forgotten.
    assert!(fill.step(FillEvent::Frame { now: 0.1, wanted: false, missing: 0, moved: false }).is_empty());
    assert_eq!(fill.state(), State::Idle);
    assert!(!fill.resuming(due), "a retired deadline resumes nothing");
    assert_eq!(fill.wake(due), None, "the deadline it left is no longer the fill's");
    assert!(fill.step(FillEvent::Due { attempt, now: due }).is_empty(), "and draws nothing");
    assert_eq!(fill.state(), State::Idle);
    // A list that replaces it starts exactly as one that was never here: its first frame draws.
    assert!(fill.step(FillEvent::Frame { now: due + 1.0, wanted: true, missing: 8, moved: false })
        .into_iter().any(|e| e == FillEffect::Draw));
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
        assert!(!fill.step(FillEvent::Due { attempt, now }).is_empty());
        assert!(wakes <= 8, "progress keeps it going: {wakes}");
    }
    assert!(wakes > RESUME_LIMIT as usize, "the shrinking range earned more resumes: {wakes}");
}

/// A viewport that moved invalidates the window the fill was chosen for, so even a stalled fill
/// starts again; a redraw alone does not.
#[test]
fn a_moved_viewport_starts_the_fill_again() {
    let mut fill = Fill::default();
    let mut now = 0.0;
    while let Some((due, attempt)) = run_to_bound(&mut fill, now, 8) {
        now = due;
        fill.step(FillEvent::Due { attempt, now });
    }
    assert_eq!(fill.state(), State::Stalled);
    assert!(fill.step(FillEvent::Frame { now: now + 1.0, wanted: true, missing: 8, moved: false }).is_empty());
    assert_eq!(fill.state(), State::Stalled, "a redraw leaves it stalled");
    let effects: Effects<FillEffect> = fill.step(FillEvent::Frame { now: now + 2.0, wanted: true, missing: 8, moved: true });
    assert!(effects.into_iter().any(|e| e == FillEffect::Draw), "a scroll starts it again");
    assert!(matches!(fill.state(), State::Filling { frames: 1, resumes: 0, .. }), "{:?}", fill.state());
}
