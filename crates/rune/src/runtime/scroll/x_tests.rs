use super::*;
use winit::dpi::PhysicalPosition;
use winit::event::MouseScrollDelta::{LineDelta, PixelDelta};

#[test]
fn a_gesture_begun_as_one_ends_is_its_momentum_once() {
    use crate::{GesturePhase, WheelEvent};
    fn view(_: &(), _: &mut Cx<()>) -> El {
        crate::col().w_full().h_full().child(crate::col().w_full().h(300.0)
            .scroll(crate::ScrollSpec { stick_bottom: false }).child(crate::el().w_full().h(2000.0)))
    }
    let mut rt = Runtime::new((), view, (300.0, 300.0), Env::default());
    rt.frame(0.0);
    let wheel = |rt: &mut Runtime<()>, phase| rt.wheel_event(WheelEvent::new(0.0, -10.0).at(100.0, 100.0).phase(phase));
    wheel(&mut rt, GesturePhase::Began);
    let first = rt.scroll.wheel_lock.gesture;
    assert_eq!((rt.scroll.wheel_lock.owner().clone(), rt.scroll.wheel_lock.momentum()), (WheelOwner::Vertical, false));
    wheel(&mut rt, GesturePhase::Ended);
    wheel(&mut rt, GesturePhase::Began);
    assert!(rt.scroll.wheel_lock.momentum(), "began right as one ended: its momentum");
    assert_eq!((rt.scroll.wheel_lock.gesture, rt.scroll.wheel_lock.owner().clone()), (first, WheelOwner::Vertical), "keeping the gesture");
    wheel(&mut rt, GesturePhase::Ended);
    wheel(&mut rt, GesturePhase::Began);
    assert!(!rt.scroll.wheel_lock.momentum(), "a finger caught the momentum: a new gesture");
    let second = rt.scroll.wheel_lock.gesture;
    assert_ne!(second, first);

    rt.wheel_lost();
    assert_ne!(rt.scroll.wheel_lock.gesture, second);
    assert_eq!((rt.scroll.wheel_lock.owner().clone(), rt.scroll.wheel_lock.momentum()), (WheelOwner::Unpicked, false));
    rt.wheel_xy(100.0, 100.0, 0.0, -10.0);
    let wheel_gesture = rt.scroll.wheel_lock.gesture;
    rt.wheel_xy(100.0, 100.0, -10.0, 0.0);
    assert_ne!(rt.scroll.wheel_lock.gesture, wheel_gesture, "without phases, a delta wholly on the other axis starts another");
}

/// A viewport whose content fits is passed over: the wheel over a section that is its own
/// viewport, as tall as its rows, scrolls the list around it, on either axis.
#[test]
fn the_wheel_over_a_viewport_with_nothing_to_scroll_scrolls_the_one_around_it() {
    fn view(_: &(), _: &mut Cx<()>) -> El {
        let fits = crate::col().key("section").w_full().h(200.0).scroll(crate::ScrollSpec { stick_bottom: false }).child(crate::el().w_full().h(200.0));
        let fits_x = crate::row().key("chips").w_full().h(40.0).scroll_x().child(crate::el().w(100.0).h(40.0));
        let list = crate::col().w_full().child(fits_x).child(fits).child(crate::el().w_full().h(2000.0));
        crate::col().key("list").w_full().h(300.0).scroll(crate::ScrollSpec { stick_bottom: false }).child(list)
    }
    let mut rt = Runtime::new((), view, (300.0, 300.0), Env::default());
    rt.frame(0.0);
    rt.frame(1.0 / 60.0);
    let id = |rt: &Runtime<()>, key: &str| rt.tree.nodes.iter().find(|n| n.key.as_deref() == Some(key)).unwrap().id;
    let (list, section) = (id(&rt, "list"), id(&rt, "section"));
    // The strip that fits sideways is no horizontal viewport for the wheel, and over it the list scrolls.
    assert_eq!(rt.scroll_at_axis(100.0, 20.0, true), None);
    assert_eq!(rt.scroll_at(100.0, 20.0), Some(list));
    rt.wheel_xy(100.0, 100.0, 0.0, -30.0);
    assert_eq!((rt.scroll.scrolls[&list].offset, rt.scroll.scrolls[&section].offset), (30.0, 0.0));
}

#[test]
fn the_phase_carries_when_the_gesture_last_moved_and_when_it_ended() {
    let mut lock = WheelLock::default();
    let plain = crate::Mods::default();
    lock.delta(1.0, 0.0, -10.0, plain, None);
    assert_eq!(lock.phase, WheelPhase::Unphased { at: Some(1.0) }, "a delta starts it, timed");
    lock.delta(1.05, 0.0, -10.0, plain, None);
    assert_eq!(lock.phase, WheelPhase::Unphased { at: Some(1.05) }, "a pause under the idle keeps the gesture");
    let gesture = lock.gesture;
    lock.delta(1.05 + WHEEL_IDLE + 0.01, 0.0, -10.0, plain, None);
    assert_ne!(lock.gesture, gesture, "a longer pause starts another");
    assert_eq!(lock.phase, WheelPhase::Unphased { at: Some(1.05 + WHEEL_IDLE + 0.01) }, "timed from the new delta");
    lock.set_owner(lock.gesture, WheelOwner::Vertical);
    lock.ended_at(2.0);
    assert_eq!(lock.phase, WheelPhase::Ended { at: 2.0, from: GestureKind::Unphased }, "an unphased end keeps its phase");
    let ended = lock.gesture;
    lock.delta(2.001, 0.0, -10.0, plain, None);
    assert_ne!(lock.gesture, ended, "a delta after the end starts another gesture at once");
    lock.lost(None);
    assert_eq!(lock.phase, WheelPhase::Unphased { at: None }, "the loss forgets when it last moved");
}

#[test]
fn an_ended_gestures_momentum_has_the_gap_its_end_time_measures() {
    let plain = crate::Mods::default();
    let mut lock = WheelLock::default();
    lock.delta(0.0, 0.0, -10.0, plain, None);
    lock.set_owner(lock.gesture, WheelOwner::Vertical);
    lock.ended_at(1.0);
    let gesture = lock.gesture;
    let soon = 1.0 + MOMENTUM_GAP * 0.5;
    lock.began(soon, true, false, None);
    assert_eq!(lock.phase, WheelPhase::Momentum { at: soon }, "within the gap: its momentum");
    assert_eq!(lock.gesture, gesture, "keeping the gesture");
    assert!(lock.momentum());
    lock.ended_at(2.0);
    assert_eq!(lock.phase, WheelPhase::Ended { at: 2.0, from: GestureKind::Momentum });
    lock.began(2.0 + MOMENTUM_GAP * 0.5, true, false, None);
    assert!(matches!(lock.phase, WheelPhase::Phased { .. }), "one that ended as momentum is a new touch");
    assert!(!lock.momentum());

    let mut lock = WheelLock::default();
    lock.delta(0.0, 0.0, -10.0, plain, None);
    lock.set_owner(lock.gesture, WheelOwner::Vertical);
    lock.ended_at(1.0);
    let gesture = lock.gesture;
    lock.began(1.0 + MOMENTUM_GAP + 0.01, true, false, None);
    assert_ne!(lock.gesture, gesture, "after the gap it is its own gesture");
}

/// The queued deltas of an unphased gesture that ended before the frame drained them still
/// pick each axis as they arrive: the end doesn't make the gesture phased.
#[test]
fn a_queued_cross_axis_delta_after_an_unphased_end_picks_its_axis() {
    fn view(_: &(), _: &mut Cx<()>) -> El {
        crate::col().size(200.0, 200.0)
            .child(crate::col().key("xy").size(200.0, 200.0).scroll_xy()
                .on_wheel(|_: &mut (), _: crate::WheelEvent, _: &mut Tx| false)
                .child(crate::el().key("content").w(1000.0).h(1000.0)))
    }
    use crate::WheelEvent;
    let mut rt = Runtime::new((), view, (200.0, 200.0), Env::default());
    rt.frame(0.0);
    rt.frame(1.0 / 60.0);
    rt.wheel_event(WheelEvent::new(-10.0, 0.0).at(100.0, 100.0));
    rt.wheel_event(WheelEvent::new(0.0, -20.0).at(100.0, 100.0));
    rt.wheel_ended();
    rt.frame(1.0 / 60.0);
    let id = rt.tree.nodes.iter().find(|n| n.key.as_deref() == Some("xy")).unwrap().id;
    assert_eq!((rt.scroll.scrolls[&id].x.offset, rt.scroll.scrolls[&id].offset), (10.0, 20.0), "each queued delta scrolls the axis it picks");
}

/// A phased gesture beginning while an unphased one's deltas wait keeps each delta's own
/// axis policy: the cross-axis one still picks its axis, rather than the successor's
/// phases holding the earlier gesture to its first axis.
#[test]
fn a_delta_queued_before_a_phased_gesture_began_keeps_its_own_axis_policy() {
    fn view(_: &(), _: &mut Cx<()>) -> El {
        crate::col().size(200.0, 200.0)
            .child(crate::col().key("xy").size(200.0, 200.0).scroll_xy()
                .on_wheel(|_: &mut (), _: crate::WheelEvent, _: &mut Tx| false)
                .child(crate::el().key("content").w(1000.0).h(1000.0)))
    }
    use crate::WheelEvent;
    let mut rt = Runtime::new((), view, (200.0, 200.0), Env::default());
    rt.frame(0.0);
    rt.frame(1.0 / 60.0);
    rt.wheel_event(WheelEvent::new(-10.0, 0.0).at(100.0, 100.0));
    rt.wheel_event(WheelEvent::new(0.0, -20.0).at(100.0, 100.0));
    rt.wheel_began();
    rt.frame(1.0 / 60.0);
    let id = rt.tree.nodes.iter().find(|n| n.key.as_deref() == Some("xy")).unwrap().id;
    assert_eq!((rt.scroll.scrolls[&id].x.offset, rt.scroll.scrolls[&id].offset), (10.0, 20.0), "the queued cross-axis delta keeps its own policy");
}

/// origin/dev's wheel lock, transcribed from this file: when its last delta came (`at`),
/// whether the gesture was phased and momentum, and when the platform ended it (`ended`),
/// beside the owner, in place of the phase that carries them.
#[derive(Default)]
struct DevLock {
    gesture: u32,
    run_gesture: u32,
    owner: DevOwner,
    at: Option<f64>,
    phased: bool,
    momentum: bool,
    ended: Option<f64>,
}

#[derive(Clone, Copy, Default, PartialEq, Debug)]
enum DevOwner {
    #[default]
    Unpicked,
    Vertical,
    Horizontal,
}

impl DevLock {
    fn restart(&mut self, queued: bool) {
        self.gesture += 1;
        self.phased = false;
        self.momentum = false;
        self.ended = None;
        if !queued {
            self.run_gesture = self.gesture;
            self.owner = DevOwner::Unpicked;
        }
    }

    fn delta(&mut self, now: f64, dx: f32, dy: f32, queued: bool) {
        let idle = if self.phased { PHASED_IDLE } else { WHEEL_IDLE };
        let flipped = !self.phased && self.run_gesture == self.gesture && match self.owner {
            DevOwner::Horizontal => dx == 0.0,
            DevOwner::Vertical => dy == 0.0,
            DevOwner::Unpicked => false,
        };
        let stale = self.ended.is_some() || self.at.is_none_or(|a| now - a > idle) || flipped;
        if stale { self.restart(queued); }
        self.at = Some(now);
    }

    fn began(&mut self, now: f64, active: bool, queued: bool, any_queued: bool) {
        let momentum = !self.momentum && (active || queued) && self.ended.is_some_and(|e| now - e <= MOMENTUM_GAP);
        if !momentum { self.restart(any_queued); }
        self.phased = true;
        self.momentum = momentum;
        self.ended = None;
        self.at = Some(now);
    }

    fn lost(&mut self, queued: bool) {
        self.restart(queued);
        self.at = None;
    }

    fn horizontal(&self) -> Option<bool> {
        match self.owner {
            DevOwner::Unpicked => None,
            DevOwner::Vertical => Some(false),
            DevOwner::Horizontal => Some(true),
        }
    }

    fn scroll(&mut self, dx: f32, dy: f32, across: bool) -> bool {
        match self.horizontal() {
            Some(h) if self.phased || if h { dx != 0.0 } else { dy != 0.0 } => h,
            _ => {
                self.owner = if across { DevOwner::Horizontal } else { DevOwner::Vertical };
                across
            }
        }
    }
}

/// The gesture dev's `run_gesture` mirrors: the older one whose queued deltas still wait,
/// else the arriving one.
fn running(lock: &WheelLock) -> Attempt {
    match &lock.run {
        WheelRun::Waiting { gesture, .. } => *gesture,
        WheelRun::Arriving(_) => lock.gesture,
    }
}

/// Who dev's single `owner` mirrors: the owner of the gesture it runs.
fn running_owner(lock: &WheelLock) -> &WheelOwner {
    match &lock.run {
        WheelRun::Waiting { owner, .. } => owner,
        WheelRun::Arriving(owner) => owner,
    }
}

fn owner_kind(owner: &WheelOwner) -> Option<DevOwner> {
    match owner {
        WheelOwner::Unpicked => Some(DevOwner::Unpicked),
        WheelOwner::Vertical => Some(DevOwner::Vertical),
        WheelOwner::Horizontal(_) => Some(DevOwner::Horizontal),
        // A handler owns the gesture only after `run_wheel` took a delta, which this walk
        // never drives.
        WheelOwner::Handler(_) => None,
    }
}

#[derive(Clone, Copy, Debug)]
enum WAct {
    Delta { dx: f32, dy: f32, gap: f64, queued: bool },
    Began { gap: f64, queued: bool },
    Ended { gap: f64 },
    Lost { queued: bool },
    Scroll { dx: f32, dy: f32, across: bool },
}

/// Drives every sequence through the lock and through dev's fields, one event at a time,
/// comparing the phase's facts, the owner and the axis each delta scrolls after every event.
fn wheel_walk(acts: &[WAct]) {
    let (mut lock, mut dev, mut now) = (WheelLock::default(), DevLock::default(), 0.0);
    for (i, act) in acts.iter().enumerate() {
        let before = lock.gesture;
        let dev_before = dev.gesture;
        match act {
            WAct::Delta { dx, dy, gap, queued } => {
                now += gap;
                let pending = (*queued).then_some(running(&lock));
                lock.delta(now, *dx, *dy, crate::Mods::default(), pending);
                dev.delta(now, *dx, *dy, *queued);
            }
            WAct::Began { gap, queued } => {
                now += gap;
                let active = lock.owner() != &WheelOwner::Unpicked;
                let pending = (*queued).then_some(running(&lock));
                lock.began(now, active, *queued, pending);
                let dev_active = dev.run_gesture == dev.gesture && dev.owner != DevOwner::Unpicked;
                dev.began(now, dev_active, *queued, *queued);
            }
            WAct::Ended { gap } => {
                now += gap;
                lock.ended_at(now);
                dev.ended = Some(now);
            }
            WAct::Lost { queued } => {
                let pending = (*queued).then_some(running(&lock));
                lock.lost(pending);
                dev.lost(*queued);
            }
            WAct::Scroll { dx, dy, across } => {
                // A delta of the gesture dev runs: the older one while its deltas wait, else
                // the arriving one. It carries that gesture's own policy.
                let gesture = running(&lock);
                let h = match lock.keeps_axis(gesture, *dx, *dy, lock.phased()) {
                    Some(h) => h,
                    None => lock.repick(gesture, across.then_some(NodeId(1))),
                };
                assert_eq!(h, dev.scroll(*dx, *dy, *across), "{acts:?}, event {i}: axis");
            }
        }
        let at = format!("{acts:?}, event {i} ({act:?})");
        assert_eq!((lock.gesture != before), (dev.gesture != dev_before), "gesture: {at}");
        assert_eq!(lock.phase.phased(), dev.phased, "phased: {at}");
        assert_eq!(lock.phase.momentum(), dev.momentum, "momentum: {at}");
        // An ended phase keeps its end time where dev kept a separate `ended`: the last
        // delta's time is no longer read, so only a live phase's is compared.
        if dev.ended.is_none() {
            assert_eq!(lock.phase.at(), dev.at, "at: {at}");
        }
        assert_eq!(lock.phase.ended(), dev.ended, "ended: {at}");
        assert_eq!(owner_kind(running_owner(&lock)), Some(dev.owner), "owner: {at}");
    }
}

/// Every order of deltas, platform phases, losses and the deltas that later scroll, up to
/// five long, including an unphased gesture that ends with a cross-axis delta queued.
#[test]
fn the_phase_matches_devs_fields_over_every_order() {
    let acts = [
        WAct::Delta { dx: -10.0, dy: 0.0, gap: 0.0, queued: false },
        WAct::Delta { dx: -10.0, dy: 0.0, gap: 0.0, queued: true },
        WAct::Delta { dx: 0.0, dy: -20.0, gap: 0.0, queued: false },
        WAct::Delta { dx: 0.0, dy: -20.0, gap: 0.0, queued: true },
        WAct::Delta { dx: -10.0, dy: 0.0, gap: 0.2, queued: false },
        WAct::Ended { gap: 0.0 },
        WAct::Ended { gap: 0.2 },
        WAct::Began { gap: 0.02, queued: false },
        WAct::Began { gap: 0.02, queued: true },
        WAct::Lost { queued: false },
        WAct::Lost { queued: true },
        WAct::Scroll { dx: -10.0, dy: 0.0, across: true },
        WAct::Scroll { dx: 0.0, dy: -20.0, across: true },
        WAct::Scroll { dx: 0.0, dy: -20.0, across: false },
    ];
    for len in 0..=5 {
        wheel_walk_all(&acts, len, &mut Vec::new());
    }
}

fn wheel_walk_all(acts: &[WAct], len: usize, prefix: &mut Vec<WAct>) {
    if prefix.len() == len {
        wheel_walk(prefix);
        return;
    }
    for &act in acts {
        prefix.push(act);
        wheel_walk_all(acts, len, prefix);
        prefix.pop();
    }
}

#[test]
fn shift_turns_only_a_purely_vertical_wheel() {
    let wheel_delta = |delta, scale, shift| wheel_delta(delta, scale, shift, LINE_POINTS);
    assert_eq!(wheel_delta(LineDelta(0.0, -1.0), 2.0, false), (0.0, -LINE_POINTS));
    assert_eq!(wheel_delta(LineDelta(0.0, -1.0), 2.0, true), (-LINE_POINTS, 0.0), "Shift + mouse wheel");
    assert_eq!(wheel_delta(LineDelta(0.5, 0.0), 2.0, false), (LINE_POINTS / 2.0, 0.0), "a fractional horizontal scroll increment");
    let trackpad = PixelDelta(PhysicalPosition::new(-8.0, 20.0));
    assert_eq!(wheel_delta(trackpad, 2.0, true), (-4.0, 10.0), "a delta with a horizontal part stays as it is");
    assert!(SHIFT_TURNS_WHEEL != cfg!(target_os = "macos"));
}
