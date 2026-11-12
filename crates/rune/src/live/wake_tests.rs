//! The host wake, driven one step at a time: a publisher's `PENDING` store and its callback are
//! separate steps, so the walk can hide, show or take the wake in between, as another thread
//! would. Every ordering of those steps is checked against a small reference for what the wake
//! owes — one wake per stretch of a visible window with pending work, none while hidden — and
//! the two places the reference departs from the dev protocol are marked.
use super::*;
use std::sync::atomic::{AtomicBool, AtomicUsize};

/// A wake and a count of the callbacks it made, with the visibility each callback saw. A callback
/// while hidden is a bug the count alone would hide, so it is counted on its own.
struct Watched {
    wake: Arc<Wake>,
    fired: Arc<AtomicUsize>,
    fired_hidden: Arc<AtomicUsize>,
    visible: Arc<AtomicBool>,
}

impl Watched {
    fn new(visible: bool) -> Self {
        let (fired, fired_hidden) = (Arc::new(AtomicUsize::new(0)), Arc::new(AtomicUsize::new(0)));
        let shown = Arc::new(AtomicBool::new(visible));
        let wake = Wake::new({
            let (fired, fired_hidden, shown) = (fired.clone(), fired_hidden.clone(), shown.clone());
            Arc::new(move || {
                if !shown.load(Ordering::SeqCst) { fired_hidden.fetch_add(1, Ordering::SeqCst); }
                fired.fetch_add(1, Ordering::SeqCst);
            })
        });
        if !visible { wake.visible(false); }
        Self { wake, fired, fired_hidden, visible: shown }
    }

    fn show(&self, visible: bool) {
        self.visible.store(visible, Ordering::SeqCst);
        self.wake.visible(visible);
    }
}

#[test]
fn hiding_during_publication_stays_silent_until_the_show() {
    let w = Watched::new(true);
    // The publisher makes the wake pending and is paused before it calls back.
    let stored = w.wake.set_pending();
    // A hide lands and finishes in that gap.
    w.show(false);
    // The publisher resumes.
    w.wake.wake_after_publish(stored);
    assert_eq!(w.fired.load(Ordering::SeqCst), 0, "a hidden window is never woken");
    assert_eq!(w.fired_hidden.load(Ordering::SeqCst), 0);
    assert!(w.wake.probe().1, "the publish still waits for the show");
    // The show delivers exactly that one wake.
    w.show(true);
    assert_eq!(w.fired.load(Ordering::SeqCst), 1);
}

#[test]
fn showing_during_publication_wakes_once() {
    for publisher_first in [true, false] {
        let w = Watched::new(false);
        // Pause the first writer immediately after its atomic change. Let the other
        // operation finish (including its callback) before the first writer resumes.
        if publisher_first {
            let stored = w.wake.set_pending();
            w.show(true);
            w.wake.wake_after_publish(stored);
        } else {
            w.visible.store(true, Ordering::SeqCst);
            let stored = w.wake.set_visible(true);
            w.wake.notify();
            w.wake.wake_after_visibility(true, stored);
        }
        assert_eq!(w.fired.load(Ordering::SeqCst), 1, "publisher first: {publisher_first}");
        assert_eq!(w.fired_hidden.load(Ordering::SeqCst), 0);
    }
}

/// One step of the protocol, as the walk plays it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Step {
    PublishStore,
    PublishWake,
    HideStore,
    HideWake,
    ShowStore,
    ShowWake,
    Take,
}

const PUBLISH: [Step; 2] = [Step::PublishStore, Step::PublishWake];
const HIDE: [Step; 2] = [Step::HideStore, Step::HideWake];
const SHOW: [Step; 2] = [Step::ShowStore, Step::ShowWake];
/// The host's own run: a hide and a later show. Its steps are one thread's, so only a
/// publisher's can land between them, never another hide or show.
const HOST: [Step; 4] = [Step::HideStore, Step::HideWake, Step::ShowStore, Step::ShowWake];

/// What the wake owes: a pending publish is delivered by the publisher that finds the window
/// visible, or else by the show that brings it back, and a hidden window is never woken. Each
/// store's answer is kept for its own wake step, as the real one keeps it.
struct Expected {
    visible: bool,
    pending: bool,
    /// What each publish's store saw: no pending work, a visible window, and the wakes
    /// delivered before it.
    fresh: Vec<bool>,
    was_visible: Vec<bool>,
    delivered: Vec<u8>,
    /// What the show's store saw: a hidden window with work waiting.
    armed: bool,
    /// Wakes delivered so far.
    wakes: u8,
    fired: usize,
}

impl Expected {
    fn new(visible: bool, programs: usize) -> Self {
        Self { visible, pending: false, fresh: vec![false; programs], was_visible: vec![visible; programs], delivered: vec![0; programs], armed: false, wakes: 0, fired: 0 }
    }

    fn play(&mut self, program: usize, step: Step) {
        match step {
            Step::PublishStore => {
                self.fresh[program] = !self.pending;
                self.was_visible[program] = self.visible;
                self.delivered[program] = self.wakes;
                self.pending = true;
            }
            // differs from dev: dev woke when its pending swap found no work before it and the
            // window was visible by the time it looked, so a publisher that began while hidden
            // woke the host after a show had already delivered that wake. Only a publisher that
            // found an idle, visible window wakes; a hidden one leaves the wake to the show.
            Step::PublishWake => {
                let unclaimed = self.wakes == self.delivered[program];
                if self.fresh[program] && self.was_visible[program] && self.visible && unclaimed { self.deliver(); }
            }
            Step::HideStore => self.visible = false,
            Step::HideWake => {}
            Step::ShowStore => {
                self.armed = !self.visible && self.pending;
                self.delivered[program] = self.wakes;
                self.visible = true;
            }
            // differs from dev: dev read the pending bit after its swap, so a publish landing in
            // between woke through the show too. The show delivers only the work its own swap
            // saw; a publish that lands after it wakes itself.
            Step::ShowWake => {
                if self.armed && self.wakes == self.delivered[program] { self.deliver(); }
            }
            Step::Take => {
                if self.visible { self.pending = false; }
            }
        }
    }

    fn deliver(&mut self) {
        self.wakes = self.wakes.wrapping_add(1);
        self.fired += 1;
    }
}

/// One sequence on the real wake, compared with the work owed after every step.
struct Run {
    w: Watched,
    expected: Expected,
    /// Each program's held store answers, until its wake step reads them.
    held: Vec<Option<(u8, u8)>>,
    steps: Vec<(usize, Step)>,
    /// Callbacks since the current stretch of pending work began.
    stretch: usize,
}

impl Run {
    fn new(visible: bool, programs: usize) -> Self {
        Self { w: Watched::new(visible), expected: Expected::new(visible, programs), held: vec![None; programs], steps: Vec::new(), stretch: 0 }
    }

    fn play(&mut self, program: usize, step: Step) {
        let where_ = format!("after {step:?} of program {program} at {:?}", self.steps);
        let was_visible = self.w.wake.probe().0;
        match step {
            Step::PublishStore => self.held[program] = Some(self.w.wake.set_pending()),
            Step::PublishWake => self.w.wake.wake_after_publish(self.held[program].take().expect(&where_)),
            Step::HideStore => {
                self.w.visible.store(false, Ordering::SeqCst);
                self.held[program] = Some(self.w.wake.set_visible(false));
            }
            Step::HideWake => self.w.wake.wake_after_visibility(false, self.held[program].take().expect(&where_)),
            Step::ShowStore => {
                self.w.visible.store(true, Ordering::SeqCst);
                self.held[program] = Some(self.w.wake.set_visible(true));
            }
            Step::ShowWake => self.w.wake.wake_after_visibility(true, self.held[program].take().expect(&where_)),
            // The host takes only a visible window's wake; a hidden one keeps it for show.
            Step::Take if self.w.wake.probe().0 => { self.w.wake.take_visible(); }
            Step::Take => {}
        }
        self.steps.push((program, step));
        self.expected.play(program, step);
        let fired = self.w.fired.load(Ordering::SeqCst);
        assert_eq!(fired, self.expected.fired, "wakes {where_}");
        assert_eq!(self.w.fired_hidden.load(Ordering::SeqCst), 0, "a hidden window woke {where_}");
        let (visible, pending) = self.w.wake.probe();
        assert_eq!((visible, pending), (self.expected.visible, self.expected.pending), "state {where_}");
        // A show, a take, or a publish that starts a new stretch of pending work resets the
        // stretch, which owes its own one wake.
        let shown = step == Step::ShowStore && !was_visible;
        if shown || step == Step::Take || (step == Step::PublishStore && self.expected.fresh[program]) {
            self.stretch = fired;
        }
        assert!(fired - self.stretch <= 1, "more than one wake in a stretch {where_}");
        // Work that waits with every writer at a resting point is owed a wake now, not later.
        if self.held.iter().all(Option::is_none) && visible && pending {
            assert!(fired > self.stretch, "pending work with no wake due {where_}");
        }
    }
}

/// Every interleaving of `programs`' steps, in each program's own order, with up to `takes` takes
/// inserted anywhere: each program is one thread's run of steps.
fn interleave(visible: bool, programs: &[&[Step]], takes: usize) {
    fn visit(programs: &[&[Step]], at: &mut [usize], takes: usize, path: &mut Vec<(usize, Step)>, out: &mut Vec<Vec<(usize, Step)>>) {
        if at.iter().zip(programs).all(|(&i, program)| i == program.len()) {
            out.push(path.clone());
            return;
        }
        if takes > 0 {
            path.push((0, Step::Take));
            visit(programs, at, takes - 1, path, out);
            path.pop();
        }
        for (p, program) in programs.iter().enumerate() {
            if at[p] == program.len() { continue; }
            path.push((p, program[at[p]]));
            at[p] += 1;
            visit(programs, at, takes, path, out);
            at[p] -= 1;
            path.pop();
        }
    }
    let mut out = Vec::new();
    visit(programs, &mut vec![0; programs.len()], takes, &mut Vec::new(), &mut out);
    for path in &out {
        let mut run = Run::new(visible, programs.len());
        for &(program, step) in path { run.play(program, step); }
    }
}

/// Every ordering of one publish, a hide and a show; of two publishes with each; and of a publish
/// with only one of them, each with up to two takes anywhere, from a visible and a hidden window.
#[test]
fn every_ordering_wakes_once_and_never_while_hidden() {
    let sets: [&[&[Step]]; 5] = [
        &[&PUBLISH, &HOST],
        &[&PUBLISH, &PUBLISH, &HOST],
        &[&PUBLISH, &SHOW],
        &[&PUBLISH, &HIDE],
        &[&PUBLISH, &PUBLISH, &SHOW],
    ];
    for programs in sets {
        for takes in 0..=2 {
            interleave(true, programs, takes);
            interleave(false, programs, takes);
        }
    }
}
