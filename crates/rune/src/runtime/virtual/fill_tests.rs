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

/// `Runtime::virtual_filling` after it read the lists (`runtime/virtual/window.rs`), through the
/// fill's own methods in the same order, the host timer aside: whether it drew, the deadlines it
/// armed, and whether it counted the unfilled rows.
fn virtual_filling(fill: &mut Fill, now: f64, wanted: bool, moved: bool, sources: u64, unfilled: usize) -> (bool, Vec<f64>, bool) {
    if !wanted {
        fill.step(FillEvent::Frame { now, wanted: false, missing: 0, moved: false });
        return (false, vec![], false);
    }
    fill.lists(now, sources);
    if let Some(attempt) = fill.wake(now) {
        return (!fill.step(FillEvent::Due { attempt, now }).is_empty(), vec![], false);
    }
    let counted = fill.at_bound();
    let missing = if counted { unfilled } else { 0 };
    let (mut draw, mut wakes) = (false, vec![]);
    for effect in fill.step(FillEvent::Frame { now, wanted, missing, moved }) {
        match effect {
            FillEffect::Draw => draw = true,
            FillEffect::WakeAt(due) => wakes.push(due),
        }
    }
    (draw, wakes, counted)
}

/// r374: lists asking under other sources retire the run whatever it was doing, filling,
/// waiting or stalled, and hand back the host timer only a wait owns. The same sources change
/// nothing.
#[test]
fn other_sources_retire_the_run() {
    let mut fill = Fill::default();
    assert_eq!(fill.lists(0.0, 0), None, "no sources named, as before: nothing changes");
    assert_eq!(fill.lists(0.0, 7), None);
    for _ in 0..3 { fill.step(FillEvent::Frame { now: 0.0, wanted: true, missing: 0, moved: false }); }
    assert_eq!(fill.lists(0.0, 7), None, "the same sources keep the run");
    assert!(matches!(fill.state(), State::Filling { frames: 3, .. }), "{:?}", fill.state());
    assert_eq!(fill.lists(0.0, 8), None, "a run that isn't waiting owns no timer");
    assert_eq!(fill.state(), State::Idle, "filling: other sources retire it");

    let (due, _) = run_to_bound(&mut fill, 0.0, 8).expect("the bound asks for a wake");
    fill.own_timer(Some(due));
    assert_eq!(fill.lists(0.01, 8), None, "the same sources keep the wait");
    assert_eq!(fill.lists(0.01, 9), Some(due), "waiting: other sources retire it and take its timer");
    assert_eq!(fill.state(), State::Idle);
    assert!(!fill.resuming(due), "and its deadline resumes nothing");

    let (due, _) = run_to_bound(&mut fill, 0.0, 8).expect("the bound asks for a wake");
    fill.own_timer(Some(due));
    fill.timer_shared(due);
    assert_eq!(fill.lists(0.01, 10), None, "a timer another shares is left armed");

    let mut now = 0.0;
    while let Some((due, attempt)) = run_to_bound(&mut fill, now, 8) {
        now = due;
        fill.step(FillEvent::Due { attempt, now });
    }
    assert_eq!(fill.state(), State::Stalled);
    assert_eq!(fill.lists(now, 11), None);
    assert_eq!(fill.state(), State::Idle, "stalled: other sources retire it");
}

/// origin/dev's fill (`runtime/virtual/fill.rs`) and the half of `Runtime::virtual_filling`
/// after it read the lists (`runtime/virtual/window.rs`), transcribed as they were, with plain
/// numbers for attempts and its constants written out. It is never edited to fit the branch: a
/// walk that fails means the branch differs from dev. Each `// differs from dev:` marks an
/// approved change.
mod reference {
    #[derive(Clone, Copy, Debug, PartialEq)]
    pub(super) enum State {
        Idle,
        Filling { frames: u16, best: usize, resumes: u8 },
        Waiting { attempt: u32, due: f64, best: usize, resumes: u8 },
        Stalled,
    }

    #[derive(Clone, Copy)]
    enum Event {
        Frame { now: f64, wanted: bool, missing: usize, moved: bool },
        Due { attempt: u32, now: f64 },
    }

    #[derive(Clone, Copy, PartialEq)]
    enum Effect {
        Draw,
        WakeAt(f64),
    }

    pub(super) struct Dev {
        pub(super) state: State,
        seq: u32,
        pub(super) wake: Option<(u32, f64)>,
        // differs from dev: the sum of the named sources the last wanted frame found asking.
        sources: u64,
    }

    impl Dev {
        pub(super) fn new() -> Self {
            Dev { state: State::Idle, seq: 0, wake: None, sources: 0 }
        }

        fn at_bound(&self) -> bool {
            matches!(self.state, State::Filling { frames, .. } if frames >= 12)
        }

        fn wake(&mut self, now: f64) -> Option<u32> {
            let (attempt, due) = self.wake?;
            if now < due { return None; }
            self.wake = None;
            Some(attempt)
        }

        fn step(&mut self, event: Event) -> Vec<Effect> {
            let effects = self.transition(event);
            if !matches!(self.state, State::Waiting { .. }) { self.wake = None; }
            effects
        }

        fn transition(&mut self, event: Event) -> Vec<Effect> {
            match (self.state, event) {
                (_, Event::Frame { wanted: false, .. }) => {
                    self.state = State::Idle;
                    vec![]
                }
                (_, Event::Frame { wanted: true, moved: true, .. }) => {
                    self.state = State::Filling { frames: 1, best: usize::MAX, resumes: 0 };
                    vec![Effect::Draw]
                }
                (State::Idle, Event::Frame { wanted: true, .. }) => {
                    self.state = State::Filling { frames: 1, best: usize::MAX, resumes: 0 };
                    vec![Effect::Draw]
                }
                (State::Filling { frames, best, resumes }, Event::Frame { now, wanted: true, missing, .. }) => {
                    let frames = frames.saturating_add(1).min(12 + 1);
                    if frames <= 12 {
                        self.state = State::Filling { frames, best, resumes };
                        return vec![Effect::Draw];
                    }
                    let (best, resumes) = if missing < best { (missing, 0) } else { (best, resumes) };
                    if missing == 0 {
                        self.state = State::Idle;
                        return vec![];
                    }
                    if resumes >= 3 {
                        self.state = State::Stalled;
                        return vec![];
                    }
                    self.seq = self.seq.wrapping_add(1);
                    let attempt = self.seq;
                    let resumes = resumes + 1;
                    let due = now + 3.0 / 60.0 * f64::from(1u32 << (resumes - 1));
                    self.state = State::Waiting { attempt, due, best, resumes };
                    self.wake = Some((attempt, due));
                    vec![Effect::WakeAt(due)]
                }
                (State::Waiting { .. } | State::Stalled, Event::Frame { wanted: true, .. }) => vec![],
                (State::Waiting { attempt, due, best, resumes }, Event::Due { attempt: a, now }) if attempt == a && now >= due => {
                    self.state = State::Filling { frames: 1, best, resumes };
                    vec![Effect::Draw]
                }
                _ => vec![],
            }
        }

        /// `Runtime::virtual_filling` after `virtual_demand`: whether it drew, the deadlines it
        /// armed, and whether it counted the unfilled rows.
        pub(super) fn virtual_filling(&mut self, now: f64, wanted: bool, moved: bool, sources: u64, unfilled: usize) -> (bool, Vec<f64>, bool) {
            if !wanted {
                self.step(Event::Frame { now, wanted: false, missing: 0, moved: false });
                return (false, vec![], false);
            }
            // differs from dev: lists asking under other named sources retire the run, as a
            // frame with nothing wanted between would.
            if sources != self.sources {
                self.step(Event::Frame { now, wanted: false, missing: 0, moved: false });
                self.sources = sources;
            }
            if let Some(attempt) = self.wake(now) {
                return (!self.step(Event::Due { attempt, now }).is_empty(), vec![], false);
            }
            let counted = self.at_bound();
            let missing = if counted { unfilled } else { 0 };
            let (mut draw, mut wakes) = (false, vec![]);
            for effect in self.step(Event::Frame { now, wanted, missing, moved }) {
                match effect {
                    Effect::Draw => draw = true,
                    Effect::WakeAt(due) => wakes.push(due),
                }
            }
            (draw, wakes, counted)
        }
    }
}

/// One frame of a walk.
#[derive(Clone, Copy)]
struct Step {
    wanted: bool,
    moved: bool,
    sources: u64,
    /// Jump the clock to the armed deadline first (a second on, when none is armed).
    to_deadline: bool,
}

/// What a walk's letter stands for: the frames it draws, and whether it shrinks the unfilled
/// count first.
fn letter(c: char) -> (Vec<Step>, bool) {
    let run = |sources, to_deadline| {
        (0..13).map(|i| Step { wanted: true, moved: false, sources, to_deadline: to_deadline && i == 0 }).collect()
    };
    match c {
        // A run's worth of frames for lists naming no source, naming one, or naming another.
        'a' => (run(0, false), false),
        'b' => (run(1, false), false),
        'c' => (run(2, false), false),
        // The armed deadline comes due in a frame for unnamed lists, or named ones, then a
        // run's worth more.
        'w' => (run(0, true), false),
        'x' => (run(1, true), false),
        // A frame with nothing to fill.
        'i' => (vec![Step { wanted: false, moved: false, sources: 0, to_deadline: false }], false),
        // A frame whose list's viewport moved.
        'm' => (vec![Step { wanted: true, moved: true, sources: 0, to_deadline: false }], false),
        // The unfilled range shrinks by a row, then a run's worth of frames.
        'p' => (run(0, false), true),
        _ => unreachable!(),
    }
}

/// Every walk up to six letters long over frames for unnamed lists and two named sources,
/// deadlines that come due in a frame for either, idle frames, moved viewports and progress,
/// each frame driven through the fill's methods and compared with the reference after every
/// frame: what it drew, the deadline it armed and holds, whether it counted the unfilled rows,
/// and where the run stands.
#[test]
fn every_walk_over_named_and_unnamed_lists_matches_the_reference() {
    const LETTERS: [char; 8] = ['a', 'b', 'c', 'w', 'x', 'i', 'm', 'p'];
    const LONGEST: u32 = 6;
    let mut walks = 0;
    for len in 1..=LONGEST {
        for n in 0..LETTERS.len().pow(len) {
            let walk: String = (0..len).map(|i| LETTERS[n / LETTERS.len().pow(i) % LETTERS.len()]).collect();
            check_walk(&walk);
            walks += 1;
        }
    }
    assert!(walks > 200_000, "{walks} walks");
}

fn check_walk(walk: &str) {
    let (mut fill, mut dev) = (Fill::default(), reference::Dev::new());
    let (mut now, mut missing) = (0.0, 8usize);
    let mut frame_no = 0;
    for c in walk.chars() {
        let (steps, shrinks) = letter(c);
        if shrinks { missing = missing.saturating_sub(1).max(1); }
        for step in steps {
            frame_no += 1;
            now = match (step.to_deadline, dev.wake) {
                (true, Some((_, due))) => due,
                (true, None) => now + 1.0,
                (false, _) => now + 1.0 / 120.0,
            };
            let got = virtual_filling(&mut fill, now, step.wanted, step.moved, step.sources, missing);
            let want = dev.virtual_filling(now, step.wanted, step.moved, step.sources, missing);
            let at = format!("frame {frame_no} of walk {walk:?}: fill {:?}, reference {:?}", fill.state(), dev.state);
            assert_eq!(got, want, "draw, deadlines, counted at {at}");
            assert_eq!(fill.wake.map(|(_, due)| due), dev.wake.map(|(_, due)| due), "armed deadline at {at}");
            let same = match (fill.state(), dev.state) {
                (State::Idle, reference::State::Idle) | (State::Stalled, reference::State::Stalled) => true,
                (State::Filling { frames, best, resumes }, reference::State::Filling { frames: f, best: b, resumes: r }) =>
                    (frames, best, resumes) == (f, b, r),
                (State::Waiting { due, best, resumes, .. }, reference::State::Waiting { due: d, best: b, resumes: r, .. }) =>
                    (due, best, resumes) == (d, b, r),
                _ => false,
            };
            assert!(same, "state at {at}");
        }
    }
}

const REPLAY_FRAMES: u64 = 360;

/// Fill draws over three seconds at 120 Hz, as `Runtime::virtual_filling` reads them, for a list
/// at offset zero whose rows alternate between `a` and `b` every frame while eight rows stay
/// unfilled, named `source` (or nothing) on every frame.
fn draws_alternating(a: &[String], b: &[String], source: Option<u64>) -> usize {
    let mut v = crate::el::virtual_list::VirtualList::default();
    v.set_estimate(20.0);
    v.viewport = 100.0;
    let mut fill = Fill::default();
    (0..REPLAY_FRAMES).filter(|&frame| {
        v.set_keys(if frame % 2 == 0 { a } else { b }.to_vec(), frame, Some(0.0));
        v.source = source;
        // `virtual_demand`'s sum, for the one list asking.
        virtual_filling(&mut fill, frame as f64 / 120.0, true, false, v.source.unwrap_or(0), 8).0
    }).count()
}

/// The reference's draws over the same frames: dev reads no rows and no sources at all.
fn dev_draws() -> usize {
    let mut dev = reference::Dev::new();
    (0..REPLAY_FRAMES).filter(|&frame| dev.virtual_filling(frame as f64 / 120.0, true, false, 0, 8).0).count()
}

fn rows() -> Vec<String> {
    (0..50).map(|i| format!("r{i}")).collect()
}

/// `rows()` with the first `percent` of them replaced.
fn churned(percent: usize) -> Vec<String> {
    let mut rows = rows();
    let n = rows.len() * percent / 100;
    for (i, row) in rows.iter_mut().enumerate().take(n) { *row = format!("x{i}"); }
    rows
}

/// r435: one named source whose rows churn, 40%, 60% or all of them every frame, keeps its
/// budget and draws what dev draws.
#[test]
fn a_named_source_keeps_its_budget_whatever_its_rows_do() {
    assert_eq!(dev_draws(), 48, "dev: twelve frames, then three resumes of twelve");
    for percent in [40, 60, 100] {
        assert_eq!(draws_alternating(&rows(), &churned(percent), Some(0xfeed)), dev_draws(), "{percent}% of the rows changed");
    }
}

/// r434, r435: without a named source the fill is dev's, whatever the rows do: a row toggled far
/// below the reader, any one row toggled in turn, or most of them.
#[test]
fn rows_without_a_source_draw_what_dev_draws() {
    let mut tail = rows();
    tail.push("tail".into());
    assert_eq!(draws_alternating(&rows(), &tail, None), dev_draws(), "a row toggled far below");
    for i in 0..rows().len() {
        let mut toggled = rows();
        toggled[i] = format!("x{i}");
        assert_eq!(draws_alternating(&rows(), &toggled, None), dev_draws(), "row {i} toggled");
    }
    for percent in [40, 60, 100] {
        assert_eq!(draws_alternating(&rows(), &churned(percent), None), dev_draws(), "{percent}% of the rows changed");
    }
}
