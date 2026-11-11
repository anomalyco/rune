//! The commit intent walk: every order of the `Tx` calls that set motion intent, checked after
//! every call against `origin/dev`'s two flags and two motion slots, transcribed below.

use super::*;
use rune_motion::{Curve, Easing};

use crate::tok::motion;

/// The spring of the node being laid out, for reads that fall back to it.
const NODE: Spring = Spring::new(0.4, 0.1);
/// The spring of a matched flight, for reads that fall back to it.
const MATCHED: Spring = Spring::new(0.2, 0.0);

fn tween() -> Anim {
    Anim { delay: 0.05, ..Anim::new(Curve::tween(0.35, Easing::cubic(0.16, 1.0, 0.3, 1.0))) }
}

fn glide() -> Anim {
    Anim::new(Curve::linear(0.2))
}

/// `origin/dev`'s `Tx` for one handler, transcribed: the two flags any handler may set, the two
/// motion slots, and the `live_only` bit, with dev's own setters and merge.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct Dev {
    instant: bool,
    settle: bool,
    motion: Option<Spring>,
    motion_curve: Option<Anim>,
    live_only: bool,
}

impl Dev {
    fn instant(&mut self) {
        self.instant = true;
    }

    fn animated(&mut self) {
        self.instant = false;
    }

    fn settle(&mut self) {
        self.instant = true;
        self.settle = true;
    }

    fn motion(&mut self, spring: Spring) {
        self.motion = Some(spring);
        self.motion_curve = None;
    }

    fn motion_curve(&mut self, anim: Anim) {
        self.motion = Some(match anim.curve {
            Curve::Spring(s) => s,
            c => Spring::new(c.logical_duration().max(0.01), 0.0),
        });
        self.motion_curve = Some(anim);
    }

    fn live_only(&mut self) {
        self.live_only = true;
    }

    /// `merge(&mut Tx, Tx)`'s three lines for these fields: either handler's snap or settle is
    /// taken, and the later handler's motion only when it set one.
    fn merge(&mut self, one: &Dev) {
        self.instant |= one.instant;
        self.settle |= one.settle;
        if one.motion.is_some() {
            self.motion = one.motion;
            self.motion_curve = one.motion_curve;
        }
    }
}

/// The reads every motion-intent site makes, and nothing else.
#[derive(Debug, PartialEq)]
struct Seen {
    /// `apply`: this change's own layout applies at once.
    snaps: bool,
    /// `apply`: this change lands every running track.
    lands: bool,
    /// `apply`: a growth row's duration scale.
    scale: f32,
    /// `apply`: the node's layout springs.
    layout: (Spring, Anim),
    /// `flights`: the spring a matched flight takes.
    flight: Spring,
    /// `latch_requests`: the window frame's spring, `None` on an instant change.
    window: Option<Spring>,
}

/// What the runtime makes of a change, read the way the sites read it.
fn seen(tx: &Tx) -> Seen {
    Seen {
        snaps: tx.commit.snaps(),
        lands: tx.commit.lands(),
        scale: tx.motion.map_or(0.0, |m| m.spring().duration * 1.15),
        layout: tx.motion.map_or((NODE, NODE.into()), |m| (m.spring(), m.anim())),
        flight: tx.motion.map(MotionSource::spring).or(Some(MATCHED)).unwrap_or(motion::REFLOW),
        window: (!tx.commit.snaps()).then(|| tx.motion.map_or(motion::DEFAULT, MotionSource::spring)),
    }
}

/// The same reads, from dev's fields.
fn seen_dev(dev: &Dev) -> Seen {
    let spring_l = dev.motion.unwrap_or(NODE);
    let anim_l: Anim = dev.motion_curve.unwrap_or_else(|| spring_l.into());
    Seen {
        snaps: dev.instant,
        lands: dev.settle,
        scale: dev.motion.map_or(0.0, |m| m.duration * 1.15),
        layout: (spring_l, anim_l),
        flight: dev.motion.or(Some(MATCHED)).unwrap_or(motion::REFLOW),
        window: (!dev.instant).then(|| dev.motion.unwrap_or(motion::DEFAULT)),
    }
}

/// One call a handler can make that sets commit intent.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Op {
    Instant,
    Animated,
    Settle,
    Motion,
    Curve,
    Tween,
    LiveOnly,
}

const OPS: [Op; 7] = [Op::Instant, Op::Animated, Op::Settle, Op::Motion, Op::Curve, Op::Tween, Op::LiveOnly];

impl Op {
    fn call(self, tx: &mut Tx) {
        match self {
            Op::Instant => tx.instant(),
            Op::Animated => tx.animated(),
            Op::Settle => tx.settle(),
            Op::Motion => tx.motion(MATCHED),
            Op::Curve => tx.motion_curve(glide()),
            Op::Tween => tx.motion_curve(tween()),
            Op::LiveOnly => tx.live_only(),
        }
    }

    fn dev(self, dev: &mut Dev) {
        match self {
            Op::Instant => dev.instant(),
            Op::Animated => dev.animated(),
            Op::Settle => dev.settle(),
            Op::Motion => dev.motion(MATCHED),
            Op::Curve => dev.motion_curve(glide()),
            Op::Tween => dev.motion_curve(tween()),
            Op::LiveOnly => dev.live_only(),
        }
    }
}

/// Every sequence of `OPS` up to `len`, in order.
fn sequences(len: usize) -> Vec<Vec<Op>> {
    let mut all = vec![Vec::new()];
    let mut frontier = vec![Vec::new()];
    for _ in 0..len {
        let mut next = Vec::new();
        for prefix in &frontier {
            for op in OPS {
                let mut seq = prefix.clone();
                seq.push(op);
                next.push(seq);
            }
        }
        all.extend(next.iter().cloned());
        frontier = next;
    }
    all
}

/// Runs `seq` to its end, checking the production `Tx` against dev after every call.
fn walk(seq: &[Op]) -> (Tx, Dev) {
    let mut tx = Tx::default();
    let mut dev = Dev::default();
    for (i, op) in seq.iter().enumerate() {
        op.call(&mut tx);
        op.dev(&mut dev);
        assert_eq!(seen(&tx), seen_dev(&dev), "after {i} of {seq:?}");
        assert_eq!(tx.live_only, dev.live_only, "live_only after {i} of {seq:?}");
    }
    (tx, dev)
}

/// Every order of the calls that set motion intent commits as dev's flags did, including which
/// call wins when two conflict (`settle` then `animated`, `motion` then `motion_curve`).
#[test]
fn every_order_of_commit_intent_commits_as_dev_did() {
    let mut reached = std::collections::BTreeSet::new();
    for seq in sequences(5) {
        let (tx, _) = walk(&seq);
        reached.insert(format!("{:?}", seen(&tx)));
    }
    // Every combination dev's flags and slots can hold, so the walk covered the whole state,
    // not a corner of it: four flag states, and no motion, a spring or one of two curves.
    assert_eq!(reached.len(), 4 * 4, "{reached:#?}");
}

/// Two handlers in one frame merge as dev's `|=` and last-`Some`-wins did, in either order.
#[test]
fn two_handlers_merge_as_dev_did() {
    let states = sequences(2);
    for a in &states {
        for b in &states {
            let (mut tx, mut dev) = walk(a);
            let (one, one_dev) = walk(b);
            crate::runtime::action::merge(&mut tx, one);
            dev.merge(&one_dev);
            assert_eq!(seen(&tx), seen_dev(&dev), "{a:?} then {b:?}");
        }
    }
}

/// The writes that set the intent directly rather than through a `Tx` setter: typing sets or
/// clears the snap and leaves running tracks landing (`keys.rs`), and a handler that declined
/// its event clears the whole intent (`action.rs`).
#[test]
fn a_direct_write_of_the_intent_matches_devs_flags() {
    let starts = [Commit::Animate, Commit::Instant, Commit::Settle, Commit::SettleAnimated];
    for start in starts {
        for animate_layout in [true, false] {
            let mut tx = Tx { commit: start, ..Tx::default() };
            let mut dev = Dev { instant: start.snaps(), settle: start.lands(), ..Dev::default() };
            tx.commit = tx.commit.snap(!animate_layout);
            dev.instant = !animate_layout;
            assert_eq!((tx.commit.snaps(), tx.commit.lands()), (dev.instant, dev.settle), "{start:?} typing");

            let mut tx = Tx { commit: start, ..Tx::default() };
            let mut dev = Dev { instant: start.snaps(), settle: start.lands(), ..Dev::default() };
            tx.commit = Commit::Animate;
            dev.instant = false;
            dev.settle = false;
            assert_eq!((tx.commit.snaps(), tx.commit.lands()), (dev.instant, dev.settle), "{start:?} declined");
        }
    }
}

/// A handler that declines its event takes back its `live_only`, and its commit intent stands.
#[test]
fn a_declined_handler_takes_back_only_live_only() {
    let mut tx = Tx::default();
    tx.settle();
    tx.live_only();
    assert!(!Tx::took(false, &mut tx));
    assert!(!tx.live_only);
    assert_eq!((tx.commit.snaps(), tx.commit.lands()), (true, true));

    let mut tx = Tx::default();
    tx.live_only();
    tx.instant();
    assert!(!Tx::took(false, &mut tx));
    assert_eq!((tx.commit.snaps(), tx.commit.lands()), (true, false));

    // A handler that took its event keeps both.
    let mut tx = Tx::default();
    tx.live_only();
    assert!(Tx::took(true, &mut tx));
    assert!(tx.live_only);
}
