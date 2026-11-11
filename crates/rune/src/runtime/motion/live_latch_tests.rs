//! The live wake against a literal transcription of its origin/dev logic: `surface_visible`,
//! `live_dirty` and the shared `Wake`'s `pending` atomic, with dev's branches in dev's order.
//! The walk drives the production lane one step at a time and lets the publishing thread run
//! between every step, then compares what a window sees after each: whether it is visible,
//! whether a value waits, whether a take found one, whether a latch read, and how often the
//! wake reached the host. A lost or doubled wake is a failed assertion, not a missed frame.
use super::*;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

/// Origin/dev's shared wake: `pending` set by a publisher, `visible` by the host, and a callback
/// that fires once when a hidden host with a wake shows.
struct DevWake {
    pending: AtomicBool,
    visible: AtomicBool,
    fired: Arc<AtomicUsize>,
}

impl DevWake {
    fn new(fired: Arc<AtomicUsize>) -> Self {
        Self { pending: AtomicBool::new(false), visible: AtomicBool::new(true), fired }
    }
    fn visible(&self, visible: bool) {
        let was = self.visible.swap(visible, Ordering::AcqRel);
        if visible && !was && self.pending.load(Ordering::Acquire) { self.fired.fetch_add(1, Ordering::SeqCst); }
    }
    fn notify(&self) {
        if !self.pending.swap(true, Ordering::AcqRel) && self.visible.load(Ordering::Acquire) {
            self.fired.fetch_add(1, Ordering::SeqCst);
        }
    }
}

/// Origin/dev's three gates for "may this window wake", transcribed.
struct Dev {
    visible: bool,
    dirty: bool,
    subs: bool,
    wake: DevWake,
}

impl Dev {
    fn new(fired: Arc<AtomicUsize>) -> Self {
        Self { visible: true, dirty: false, subs: true, wake: DevWake::new(fired) }
    }
    fn set_visible(&mut self, visible: bool) {
        self.wake.visible(visible);
        self.visible = visible;
    }
    fn publish(&self) { self.wake.notify(); }
    fn pending(&self) -> bool { self.visible && (self.dirty || self.wake.pending.load(Ordering::Acquire)) }
    fn subscribe(&mut self, sampled: bool) { self.dirty = sampled; }
    fn prune(&mut self, sampled: bool) {
        let dirty = self.dirty;
        self.subscribe(sampled);
        self.dirty &= dirty;
    }
    fn set(&mut self) { self.dirty = true; }
    fn take(&mut self) -> bool { self.visible && self.wake.pending.swap(false, Ordering::AcqRel) }
    fn latch_begin(&mut self, pending: bool) -> bool {
        if !self.visible || !self.subs { return false; }
        if !pending && !self.dirty { return false; }
        self.dirty = false;
        true
    }
    fn latch_end(&mut self, active: bool) -> bool {
        self.dirty |= active;
        self.dirty
    }
}

#[derive(Clone, Copy, Debug)]
enum Ev {
    Publish,
    Show,
    Hide,
    Subscribe(bool),
    Prune(bool),
    Set,
    Take,
    LatchBegin,
    LatchEnd(bool),
    Subs(bool),
}

/// Run one sequence through both, comparing after every step.
fn walk(events: &[Ev]) {
    let fired_dev = Arc::new(AtomicUsize::new(0));
    let fired_new = Arc::new(AtomicUsize::new(0));
    let mut dev = Dev::new(fired_dev.clone());
    let mut new = LiveLane::new({
        let fired = fired_new.clone();
        Arc::new(move || { fired.fetch_add(1, Ordering::SeqCst); })
    });
    let (mut woke_dev, mut woke_new) = (false, false);
    let (mut reading_dev, mut reading_new) = (false, false);
    for (i, &event) in events.iter().enumerate() {
        let (answer_dev, answer_new) = match event {
            Ev::Publish => { dev.publish(); new.publish(); (None, None) }
            Ev::Show => { dev.set_visible(true); new.set_visible(true); (None, None) }
            Ev::Hide => { dev.set_visible(false); new.set_visible(false); (None, None) }
            Ev::Subscribe(sampled) => { dev.subscribe(sampled); new.subscribed(sampled); (None, None) }
            Ev::Prune(sampled) => {
                let keep_dev = dev.dirty;
                dev.prune(sampled);
                let keep_new = new.dirty();
                new.subscribed(sampled);
                new.pruned(keep_new);
                debug_assert_eq!(keep_dev, keep_new);
                (None, None)
            }
            Ev::Set => { dev.set(); new.set(); (None, None) }
            Ev::Take => {
                let (a, b) = (dev.take(), new.take());
                woke_dev |= a;
                woke_new |= b;
                (Some(a), Some(b))
            }
            Ev::LatchBegin => {
                let a = dev.latch_begin(woke_dev || dev.pending());
                let b = new.latch_begin(woke_new || new.pending(), dev.subs);
                reading_dev = a;
                reading_new = b;
                woke_dev = false;
                woke_new = false;
                (Some(a), Some(b))
            }
            Ev::LatchEnd(active) => {
                let a = reading_dev.then(|| dev.latch_end(active));
                let b = reading_new.then(|| new.latch_end(active));
                (a, b)
            }
            Ev::Subs(has) => { dev.subs = has; (None, None) }
        };
        assert_eq!(answer_dev, answer_new, "answer differed after {event:?} at step {i} of {events:?}");
        assert_eq!(dev.visible, new.visible(), "visible differed after {event:?} at step {i} of {events:?}");
        assert_eq!(dev.pending(), new.pending(), "pending differed after {event:?} at step {i} of {events:?}");
        assert_eq!(
            fired_dev.load(Ordering::SeqCst),
            fired_new.load(Ordering::SeqCst),
            "wake count differed after {event:?} at step {i} of {events:?}"
        );
    }
}

/// Every sequence up to length 6 of the events a frame, a commit, a host and a publisher send.
/// Length 6 reaches every interleaving the frame has: a publish before, between or after a
/// take and a latch, a show or hide around either, and two publishes before one frame.
#[test]
fn every_short_sequence_matches_dev() {
    const EVENTS: [Ev; 11] = [
        Ev::Publish,
        Ev::Show,
        Ev::Hide,
        Ev::Subscribe(true),
        Ev::Subscribe(false),
        Ev::Prune(true),
        Ev::Prune(false),
        Ev::Set,
        Ev::Take,
        Ev::LatchBegin,
        Ev::LatchEnd(true),
    ];
    walk(&[]);
    let n = EVENTS.len();
    for len in 1..=6 {
        let mut sequence = vec![Ev::Publish; len];
        for mut code in 0..n.pow(len as u32) {
            for slot in &mut sequence {
                *slot = EVENTS[code % n];
                code /= n;
            }
            walk(&sequence);
        }
    }
}

/// A publish that lands while the frame reads keeps a wake for the next frame; the atomic is
/// taken before the read and never cleared after it.
#[test]
fn a_publish_while_the_frame_reads_survives() {
    let mut lane = LiveLane::new(Arc::new(|| {}));
    lane.subscribed(true);
    lane.publish();
    assert!(lane.take(), "the frame takes the publish");
    lane.publish();
    assert!(lane.latch_begin(true, true), "the frame reads");
    assert!(!lane.latch_end(false), "nothing is still moving");
    assert!(lane.pending(), "the publish that landed during the read keeps a wake");
    assert!(lane.take(), "and the next frame takes exactly it");
    assert!(!lane.pending());
}

/// A hidden window keeps one coalesced wake for show, and draws nothing in between.
#[test]
fn a_hidden_window_keeps_one_wake_for_show() {
    let fired = Arc::new(AtomicUsize::new(0));
    let mut lane = LiveLane::new({
        let fired = fired.clone();
        Arc::new(move || { fired.fetch_add(1, Ordering::SeqCst); })
    });
    lane.set_visible(false);
    lane.publish();
    lane.publish();
    assert_eq!(fired.load(Ordering::SeqCst), 0, "a hidden window is not woken");
    assert!(!lane.pending(), "a hidden window has nothing to draw");
    assert!(!lane.take(), "and the take leaves the wake for show");
    assert!(!lane.latch_begin(true, true), "so no latch runs");
    lane.set_visible(true);
    assert_eq!(fired.load(Ordering::SeqCst), 1, "showing delivers the one coalesced wake");
    assert!(lane.take());
    assert!(!lane.take(), "and it is delivered once");
    assert_eq!(fired.load(Ordering::SeqCst), 1);
}

/// Hiding and showing between a publish and its frame neither loses the wake nor doubles it.
#[test]
fn hiding_and_showing_between_a_publish_and_its_frame_keeps_one_wake() {
    let fired = Arc::new(AtomicUsize::new(0));
    let mut lane = LiveLane::new({
        let fired = fired.clone();
        Arc::new(move || { fired.fetch_add(1, Ordering::SeqCst); })
    });
    lane.publish();
    assert_eq!(fired.load(Ordering::SeqCst), 1);
    lane.set_visible(false);
    lane.set_visible(true);
    assert_eq!(fired.load(Ordering::SeqCst), 2, "the show finds the wake still waiting");
    assert!(lane.take());
    assert!(!lane.take());
    assert_eq!(fired.load(Ordering::SeqCst), 2);
}

/// Two publishes before one frame coalesce into one wake and one take.
#[test]
fn two_publishes_before_one_frame_coalesce() {
    let fired = Arc::new(AtomicUsize::new(0));
    let mut lane = LiveLane::new({
        let fired = fired.clone();
        Arc::new(move || { fired.fetch_add(1, Ordering::SeqCst); })
    });
    lane.publish();
    lane.publish();
    assert_eq!(fired.load(Ordering::SeqCst), 1);
    assert!(lane.take());
    assert!(!lane.take());
    assert_eq!(fired.load(Ordering::SeqCst), 1);
}

/// A value an app thread publishes still wakes an idle window through the real `Live` source,
/// and a hidden one holds that wake until it shows.
#[test]
fn an_app_thread_set_wakes_an_idle_window() {
    let fired = Arc::new(AtomicUsize::new(0));
    let mut lane = LiveLane::new({
        let fired = fired.clone();
        Arc::new(move || { fired.fetch_add(1, Ordering::SeqCst); })
    });
    let value = crate::live::Live::new(0.0f32);
    let _sub = value.source.subscribe(lane.waker());
    assert!(!lane.pending());
    assert!(value.set(1.0));
    assert_eq!(fired.load(Ordering::SeqCst), 1, "an idle window is woken");
    assert!(lane.pending());
    assert!(lane.take());

    lane.set_visible(false);
    assert!(value.set(2.0));
    assert_eq!(fired.load(Ordering::SeqCst), 1, "a hidden window is not");
    assert!(!lane.pending());
    lane.set_visible(true);
    assert_eq!(fired.load(Ordering::SeqCst), 2, "showing delivers the held wake");
    assert!(lane.take());
    assert!(!lane.take(), "once");
    assert_eq!(fired.load(Ordering::SeqCst), 2);
}

/// A visible frame with nothing pending does dev's one atomic swap and nothing else: the state is
/// not stepped or written. Only a frame with work goes through `step`.
#[test]
fn an_empty_frame_leaves_the_state_untouched() {
    let mut lane = LiveLane::new(Arc::new(|| {}));
    assert_eq!(lane.state, LiveWake::Idle);
    assert!(!lane.take(), "nothing published");
    assert_eq!(lane.state, LiveWake::Idle, "a take with nothing pending changes nothing");
    assert!(!lane.latch_begin(false, true), "nothing to read");
    assert_eq!(lane.state, LiveWake::Idle, "a clean latch changes nothing");

    // The scroll path's second latch, with nothing left to read.
    lane.state = LiveWake::Latching { dirty: false };
    assert!(!lane.latch_begin(false, true));
    assert_eq!(lane.state, LiveWake::Latching { dirty: false });

    // A take with work waiting records nothing either: the work is the dirty bit it keeps.
    lane.state = LiveWake::Pending;
    assert!(!lane.take());
    assert_eq!(lane.state, LiveWake::Pending);

    // No bound sources is dev's early return, before any transition.
    assert!(!lane.latch_begin(true, false));
    assert_eq!(lane.state, LiveWake::Pending);
}

/// A live-only event without a commit keeps the frame due, and a commit that samples a value
/// keeps it due until the latch reads.
#[test]
fn a_live_only_set_keeps_the_frame_due() {
    let mut lane = LiveLane::new(Arc::new(|| {}));
    assert!(!lane.pending());
    lane.set();
    assert!(lane.pending(), "a live-only publish asks for a frame");
    assert!(!lane.take(), "with no shared wake");
    assert!(lane.latch_begin(false, true), "but the latch still reads");
    assert!(!lane.latch_end(false));
    assert!(!lane.pending(), "and rests once read");
}

/// A window with no bound sources never reads, and a hidden one never draws.
#[test]
fn nothing_reads_without_sources_or_while_hidden() {
    let mut lane = LiveLane::new(Arc::new(|| {}));
    lane.subscribed(true);
    assert!(!lane.latch_begin(true, false), "no sources, no read");
    lane.set_visible(false);
    assert!(!lane.latch_begin(true, true), "hidden, no read");
}
