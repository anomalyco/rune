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

    // A visible frame with no bound sources leaves an idle lane untouched.
    lane.state = LiveWake::Idle;
    assert!(!lane.latch_begin(true, false));
    assert_eq!(lane.state, LiveWake::Idle);
}

/// A visible frame with no bound sources clears any pending wake so the window stops redrawing;
/// a hidden window still keeps its dirty bit until it shows.
#[test]
fn latch_begin_without_sources_clears_pending_wake() {
    let mut lane = LiveLane::new(Arc::new(|| {}));
    lane.set();
    assert!(lane.pending());
    assert!(!lane.latch_begin(true, false), "no sources, no read");
    assert_eq!(lane.state, LiveWake::Idle);
    assert!(!lane.pending());

    lane.set_visible(false);
    lane.set();
    assert!(!lane.latch_begin(true, false), "hidden frame does not clear");
    assert_eq!(lane.state, LiveWake::Hidden { dirty: true });
    lane.set_visible(true);
    assert!(lane.pending());
    assert!(!lane.latch_begin(true, false));
    assert_eq!(lane.state, LiveWake::Idle);
    assert!(!lane.pending());
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

/// `subscribe_live` looks up existing subscriptions by source id in O(1) per binding, so a
/// commit over N distinct bound `Live` sources stays linear instead of scanning all subscriptions.
#[test]
fn subscribe_live_scales_linearly_with_bound_sources() {
    struct State {
        rev: u32,
        sources: Vec<crate::Live<[f32; 2]>>,
    }
    fn view(s: &State, _: &mut crate::Cx<State>) -> crate::El {
        let mut root = crate::el().size(400.0, 400.0);
        for (i, src) in s.sources.iter().enumerate() {
            root = root.child(
                crate::el()
                    .key(format!("n{i}"))
                    .size(4.0, 4.0)
                    .live_offset(src),
            );
        }
        let _ = s.rev;
        root
    }
    let measure = |n: usize| -> (usize, usize) {
        let state = State {
            rev: 0,
            sources: (0..n).map(|i| crate::Live::new([i as f32, 0.0])).collect(),
        };
        let mut rt = crate::Runtime::new(state, view, (400.0, 400.0), crate::Env::new(true, 1.0));
        take_source_key_comparisons();
        rt.frame(0.0);
        let first_commit = take_source_key_comparisons();
        assert_eq!(rt.core().frame.live_subscriptions.len(), n);

        rt.update(|s, _| s.rev += 1);
        take_source_key_comparisons();
        rt.frame(0.0);
        let recommit = take_source_key_comparisons();
        assert_eq!(rt.core().frame.live_subscriptions.len(), n);
        (first_commit, recommit)
    };

    let n = 256;
    let (first_n, recommit_n) = measure(n);
    let (first_2n, recommit_2n) = measure(n * 2);

    assert!(first_n <= n * 2, "initial subscribe at n={n} was superlinear: {first_n} comparisons");
    assert!(first_2n <= n * 4, "initial subscribe at n={} was superlinear: {first_2n} comparisons", n * 2);
    assert!(recommit_n >= n, "counter did not observe lookups at n={n}: {recommit_n}");
    assert!(recommit_n <= n * 2, "recommit at n={n} was superlinear: {recommit_n} comparisons");
    assert!(recommit_2n >= n * 2, "counter did not observe lookups at n={}: {recommit_2n}", n * 2);
    assert!(recommit_2n <= n * 4, "recommit at n={} was superlinear: {recommit_2n} comparisons", n * 2);
    assert!(
        recommit_2n < recommit_n * 3,
        "doubling n from {n} to {} grew comparisons from {recommit_n} to {recommit_2n} (quadratic)",
        n * 2,
    );
}

/// Subscription deduplication, reuse, addition, pruning, re-addition, `set_live_waker`, and clear
/// across `Live`, `LiveText`, and `Pixels` sources.
#[test]
fn subscribe_live_reuses_adds_prunes_and_replaces_waker() {
    struct State {
        rev: u32,
        show_a: bool,
        show_b: bool,
        show_c: bool,
        show_text: bool,
        show_pixels: bool,
        a: crate::Live<f32>,
        b: crate::Live<f32>,
        c: crate::Live<f32>,
        label: crate::LiveText,
        pixels: crate::Pixels,
    }
    fn view(s: &State, _: &mut crate::Cx<State>) -> crate::El {
        let mut root = crate::el().size(200.0, 200.0);
        if s.show_a {
            // Bind `a` twice across two elements to verify deduplication.
            root = root
                .child(crate::el().key("a1").size(10.0, 10.0).live_opacity(&s.a).live_scale(&s.a))
                .child(crate::el().key("a2").size(10.0, 10.0).live_opacity(&s.a));
        }
        if s.show_b {
            root = root.child(crate::el().key("b").size(10.0, 10.0).live_opacity(&s.b));
        }
        if s.show_c {
            root = root.child(crate::el().key("c").size(10.0, 10.0).live_opacity(&s.c));
        }
        if s.show_text {
            root = root.child(crate::text("init").key("lbl").live_text(&s.label));
        }
        if s.show_pixels {
            root = root.child(crate::el().key("px").size(4.0, 4.0).pixels(&s.pixels));
        }
        let _ = s.rev;
        root
    }

    let a = crate::Live::new(1.0f32);
    let b = crate::Live::new(1.0f32);
    let c = crate::Live::new(1.0f32);
    let label = crate::LiveText::new("init");
    let pixels = crate::Pixels::new();
    pixels.set(2, 2, vec![255u8; 16]);
    let state = State {
        rev: 0,
        show_a: true,
        show_b: true,
        show_c: false,
        show_text: true,
        show_pixels: true,
        a: a.clone(),
        b: b.clone(),
        c: c.clone(),
        label: label.clone(),
        pixels: pixels.clone(),
    };
    let mut rt = crate::Runtime::new(state, view, (200.0, 200.0), crate::Env::new(true, 1.0));
    let woke_1 = Arc::new(AtomicUsize::new(0));
    rt.set_live_waker({
        let woke = woke_1.clone();
        Arc::new(move || { woke.fetch_add(1, Ordering::SeqCst); })
    });
    rt.frame(0.0);

    // Deduplicated to 4 distinct sources: a, b, label, pixels.
    assert_eq!(rt.core().frame.live_subscriptions.len(), 4);
    let sub_a = rt.core().frame.live_subscriptions[&SourceKey(a.source.id())].clone();
    let sub_b = rt.core().frame.live_subscriptions[&SourceKey(b.source.id())].clone();
    assert!(sub_a.active.load(Ordering::Acquire));
    assert!(sub_b.active.load(Ordering::Acquire));

    // Re-commit with the same bound sources reuses the exact same Arc<Subscription>s.
    rt.update(|s, _| s.rev += 1);
    rt.frame(0.0);
    assert_eq!(rt.core().frame.live_subscriptions.len(), 4);
    assert!(Arc::ptr_eq(&rt.core().frame.live_subscriptions[&SourceKey(a.source.id())], &sub_a));
    assert!(Arc::ptr_eq(&rt.core().frame.live_subscriptions[&SourceKey(b.source.id())], &sub_b));

    // Remove b, add c: b's subscription is pruned and deactivated; c is subscribed.
    rt.update(|s, _| { s.show_b = false; s.show_c = true; });
    rt.frame(0.0);
    assert_eq!(rt.core().frame.live_subscriptions.len(), 4);
    assert!(!rt.core().frame.live_subscriptions.contains_key(&SourceKey(b.source.id())));
    assert!(rt.core().frame.live_subscriptions.contains_key(&SourceKey(c.source.id())));
    assert!(!sub_b.active.load(Ordering::Acquire));

    // Drain any pending wake before probing publishers.
    rt.frame(1.0 / 120.0);
    let before = woke_1.load(Ordering::SeqCst);
    b.set(0.5);
    assert_eq!(woke_1.load(Ordering::SeqCst), before, "pruned source must not wake");
    c.set(0.5);
    assert_eq!(woke_1.load(Ordering::SeqCst), before + 1, "added source must wake");
    rt.frame(1.0 / 120.0);

    // Re-add b: its subscription becomes active and wakes again.
    rt.update(|s, _| s.show_b = true);
    rt.frame(0.0);
    rt.frame(1.0 / 120.0);
    assert!(rt.core().frame.live_subscriptions.contains_key(&SourceKey(b.source.id())));
    assert!(sub_b.active.load(Ordering::Acquire));
    let before = woke_1.load(Ordering::SeqCst);
    b.set(0.25);
    assert_eq!(woke_1.load(Ordering::SeqCst), before + 1, "re-added source must wake");
    rt.frame(1.0 / 120.0);

    // Replacing the waker deactivates old subscriptions and re-subscribes bound sources to the new waker.
    let woke_2 = Arc::new(AtomicUsize::new(0));
    rt.set_live_waker({
        let woke = woke_2.clone();
        Arc::new(move || { woke.fetch_add(1, Ordering::SeqCst); })
    });
    assert!(!sub_a.active.load(Ordering::Acquire));
    assert_eq!(rt.core().frame.live_subscriptions.len(), 5);
    rt.frame(1.0 / 120.0);
    let old_before = woke_1.load(Ordering::SeqCst);
    let new_before = woke_2.load(Ordering::SeqCst);
    a.set(0.4);
    assert_eq!(woke_1.load(Ordering::SeqCst), old_before, "old waker must not fire after replace");
    assert_eq!(woke_2.load(Ordering::SeqCst), new_before + 1, "new waker must fire");
    rt.frame(1.0 / 120.0);

    // LiveText and Pixels also wake through the replaced waker.
    let new_before = woke_2.load(Ordering::SeqCst);
    label.set("next");
    assert_eq!(woke_2.load(Ordering::SeqCst), new_before + 1, "LiveText must wake");
    rt.frame(1.0 / 120.0);
    let new_before = woke_2.load(Ordering::SeqCst);
    pixels.set(2, 2, vec![128u8; 16]);
    assert_eq!(woke_2.load(Ordering::SeqCst), new_before + 1, "Pixels must wake");
    rt.frame(1.0 / 120.0);

    // Removing all live sources drops `Live` bindings on commit and prunes exiting
    // `LiveText`/`Pixels` subscriptions once their exits finish.
    let sub_a_new = rt.core().frame.live_subscriptions[&SourceKey(a.source.id())].clone();
    rt.update(|s, _| {
        s.show_a = false;
        s.show_b = false;
        s.show_c = false;
        s.show_text = false;
        s.show_pixels = false;
    });
    rt.frame(0.0);
    assert!(!sub_a_new.active.load(Ordering::Acquire));
    for _ in 0..120 {
        if !rt.core().frame.has_live_subscriptions() { break; }
        rt.frame(1.0 / 60.0);
    }
    assert!(!rt.core().frame.has_live_subscriptions());
    assert!(rt.core().frame.live_subscriptions.is_empty());
    let new_before = woke_2.load(Ordering::SeqCst);
    a.set(0.1);
    label.set("gone");
    pixels.set(2, 2, vec![64u8; 16]);
    assert_eq!(woke_2.load(Ordering::SeqCst), new_before, "cleared sources must not wake");
}
