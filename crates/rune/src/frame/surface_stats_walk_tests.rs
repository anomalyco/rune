//! Every short sequence of busy, spinning, calm and quiet seconds through a [`SurfaceMeter`].
//! The real meter and its spin [`step`] run beside `Old`, a transcription of the fields and
//! branches they replaced (`hot`, `warned`), which calls none of them; both must publish the
//! same stats and warn at the same roll with the same words.

use super::*;

// The old code.

#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct OldCounts {
    redraws: u64,
    unpaced: u64,
    presents: u64,
    skipped: u64,
    occluded: u64,
    timeouts: u64,
    outdated: u64,
    turns: u64,
    wakes: u64,
}

impl OldCounts {
    fn failed(&self) -> u64 { self.occluded + self.timeouts + self.outdated }

    fn since(&self, earlier: &OldCounts) -> OldCounts {
        OldCounts {
            redraws: self.redraws - earlier.redraws,
            unpaced: self.unpaced - earlier.unpaced,
            presents: self.presents - earlier.presents,
            skipped: self.skipped - earlier.skipped,
            occluded: self.occluded - earlier.occluded,
            timeouts: self.timeouts - earlier.timeouts,
            outdated: self.outdated - earlier.outdated,
            turns: self.turns - earlier.turns,
            wakes: self.wakes - earlier.wakes,
        }
    }

    fn per(&self, secs: f64) -> OldCounts {
        let r = |n: u64| (n as f64 / secs).round() as u64;
        OldCounts {
            redraws: r(self.redraws), unpaced: r(self.unpaced), presents: r(self.presents), skipped: r(self.skipped),
            occluded: r(self.occluded), timeouts: r(self.timeouts), outdated: r(self.outdated), turns: r(self.turns), wakes: r(self.wakes),
        }
    }
}

fn old_pacing(pacing: Pacing) -> String {
    match pacing {
        Pacing::Idle => "idle".into(),
        Pacing::DisplayLink => "display link".into(),
        Pacing::Unpaced => "unpaced".into(),
        Pacing::Vsync => "vsync".into(),
        Pacing::Failed(status) => format!("display link failed ({status})"),
    }
}

#[derive(Clone, Debug, Default)]
struct OldStats {
    name: String,
    totals: OldCounts,
    per_second: OldCounts,
    pacing: Pacing,
    published: Option<Instant>,
}

impl OldStats {
    fn summary(&self) -> String {
        let (t, r) = (&self.totals, &self.per_second);
        format!(
            "{}: {} redraws/s ({} unpaced), {} presents/s, {} failed acquires/s (occluded {}, timeout {}, outdated {}), {} skipped/s, {} loop turns/s ({} wakes); totals {} presents, {} failed, {} skipped; pacing: {}",
            self.name, r.redraws, r.unpaced, r.presents, r.failed(), r.occluded, r.timeouts, r.outdated, r.skipped, r.turns, r.wakes,
            t.presents, t.failed(), t.skipped, old_pacing(self.pacing),
        )
    }
}

struct Old {
    totals: OldCounts,
    base: OldCounts,
    started: Instant,
    pacing: Pacing,
    waited: bool,
    hot: u32,
    warned: bool,
    shared: OldStats,
}

impl Old {
    fn new(name: &str, started: Instant) -> Self {
        let shared = OldStats { name: name.to_owned(), ..OldStats::default() };
        Old { totals: OldCounts::default(), base: OldCounts::default(), started, pacing: Pacing::Idle, waited: false, hot: 0, warned: false, shared }
    }

    fn redraw(&mut self, paced: bool) {
        self.totals.redraws += 1;
        if !paced && !self.waited { self.totals.unpaced += 1; }
        self.waited = false;
        self.tick(Instant::now());
    }

    fn waited(&mut self) { self.waited = true; }

    fn presented(&mut self) {
        self.totals.presents += 1;
        self.tick(Instant::now());
    }

    fn skipped(&mut self) {
        self.totals.skipped += 1;
        self.tick(Instant::now());
    }

    fn failed(&mut self, missed: Missed) {
        match missed {
            Missed::Occluded => self.totals.occluded += 1,
            Missed::Timeout => self.totals.timeouts += 1,
            Missed::Outdated | Missed::Unconfigured => self.totals.outdated += 1,
        }
        self.tick(Instant::now());
    }

    fn turn(&mut self, woke: bool, waited: bool) {
        self.totals.turns += 1;
        if woke { self.totals.wakes += 1; }
        self.waited |= waited;
        self.tick(Instant::now());
    }

    fn set_pacing(&mut self, pacing: Pacing) {
        self.pacing = pacing;
        self.shared.pacing = pacing;
    }

    fn tick(&mut self, now: Instant) -> Option<String> {
        let elapsed = now.saturating_duration_since(self.started);
        let summary = {
            let stats = &mut self.shared;
            stats.totals = self.totals;
            if elapsed < WINDOW { return None; }
            stats.per_second = self.totals.since(&self.base).per(elapsed.as_secs_f64());
            stats.published = Some(now);
            let rates = stats.per_second;
            self.hot = if rates.failed() > SPIN_RATE || rates.unpaced > SPIN_RATE { self.hot + 1 } else { 0 };
            stats.summary()
        };
        self.base = self.totals;
        self.started = now;
        if self.hot < SPIN_SECONDS || std::mem::replace(&mut self.warned, true) { return None; }
        let warning = format!("rune: surface has spun for {} s, a bug in Rune: {summary}", self.hot);
        Some(warning)
    }
}

// The walk.

#[derive(Clone, Copy, Debug)]
enum Ev {
    /// Over `SPIN_RATE` failed acquisitions, then a one-second roll.
    Failing,
    /// Over `SPIN_RATE` unpaced redraws, then a one-second roll.
    Unpaced,
    /// A 60 Hz second, then a one-second roll.
    Calm,
    /// Redraws after an event, a timer still ahead and a poll; no roll.
    Waits,
    /// A tick half a second after the last roll: too soon to roll.
    Early,
    /// Three seconds with nothing, then a roll.
    Quiet,
    /// The display link failed.
    Pace,
}

const ALPHABET: [Ev; 7] = [Ev::Failing, Ev::Unpaced, Ev::Calm, Ev::Waits, Ev::Early, Ev::Quiet, Ev::Pace];

fn counts(c: &SurfaceCounts) -> [u64; 9] {
    [c.redraws, c.unpaced, c.presents, c.skipped, c.occluded, c.timeouts, c.outdated, c.turns, c.wakes]
}

fn old_counts(c: &OldCounts) -> [u64; 9] {
    [c.redraws, c.unpaced, c.presents, c.skipped, c.occluded, c.timeouts, c.outdated, c.turns, c.wakes]
}

fn walk(events: &[Ev]) {
    // Rolls happen only at the walk's times: every count's own tick lands before `start`.
    let start = Instant::now() + Duration::from_secs(3600);
    let mut new = SurfaceMeter::new("main", Arc::default());
    new.started = start;
    let mut old = Old::new("main", start);
    let mut at = start;
    for (k, &ev) in events.iter().enumerate() {
        let (n, o) = (&mut new, &mut old);
        let tick = match ev {
            Ev::Failing => {
                for _ in 0..=SPIN_RATE { n.failed(Missed::Occluded); o.failed(Missed::Occluded); }
                n.failed(Missed::Timeout); o.failed(Missed::Timeout);
                n.failed(Missed::Outdated); o.failed(Missed::Outdated);
                Some(Duration::from_secs(1))
            }
            Ev::Unpaced => {
                for _ in 0..=SPIN_RATE { n.redraw(false); o.redraw(false); }
                Some(Duration::from_secs(1))
            }
            Ev::Calm => {
                for _ in 0..60 { n.redraw(true); o.redraw(true); n.presented(); o.presented(); }
                n.skipped(); o.skipped();
                n.turn(true, false); o.turn(true, false);
                Some(Duration::from_secs(1))
            }
            Ev::Waits => {
                n.waited(); o.waited();
                n.redraw(false); o.redraw(false);
                n.turn(true, true); o.turn(true, true);
                n.redraw(false); o.redraw(false);
                n.turn(false, false); o.turn(false, false);
                n.redraw(false); o.redraw(false);
                n.waited(); o.waited();
                None
            }
            Ev::Early => {
                let early = at + Duration::from_millis(500);
                assert_eq!(n.tick(early), o.tick(early), "early tick after {:?}", &events[..=k]);
                None
            }
            Ev::Quiet => Some(Duration::from_secs(3)),
            Ev::Pace => {
                n.set_pacing(Pacing::Failed(-6661)); o.set_pacing(Pacing::Failed(-6661));
                None
            }
        };
        if let Some(dt) = tick {
            at += dt;
            assert_eq!(new.tick(at), old.tick(at), "roll after {:?}", &events[..=k]);
        }
        let stats = new.shared.lock().unwrap().clone();
        let o = &old.shared;
        assert_eq!(
            (stats.name.as_str(), counts(&stats.totals), counts(&stats.per_second), stats.pacing, stats.published),
            (o.name.as_str(), old_counts(&o.totals), old_counts(&o.per_second), o.pacing, o.published),
            "stats after {:?}", &events[..=k],
        );
        assert_eq!(counts(&new.totals()), old_counts(&old.totals), "totals after {:?}", &events[..=k]);
    }
}

#[test]
fn every_sequence_of_seconds_publishes_and_warns_as_the_old_meter_did() {
    const LEN: u32 = 5;
    let mut seq = Vec::with_capacity(LEN as usize);
    for len in 1..=LEN {
        for n in 0..ALPHABET.len().pow(len) {
            seq.clear();
            let mut n = n;
            for _ in 0..len {
                seq.push(ALPHABET[n % ALPHABET.len()]);
                n /= ALPHABET.len();
            }
            walk(&seq);
        }
    }
}

#[test]
fn spinning_warns_once_on_the_second_hot_second_in_a_row() {
    let mut spin = Spin::Calm;
    let roll = |spin: &mut Spin, spinning| step(spin, Rolled { spinning });
    assert!(roll(&mut spin, true).is_empty());
    assert_eq!(spin, Spin::Hot { seconds: 1 });
    assert!(roll(&mut spin, false).is_empty());
    assert_eq!(spin, Spin::Calm, "a calm second ends the streak");
    roll(&mut spin, true);
    assert_eq!(roll(&mut spin, true), Effects::one(SpinEffect::Warn { seconds: 2 }));
    assert_eq!(spin, Spin::Warned);
    assert!(roll(&mut spin, true).is_empty() && roll(&mut spin, false).is_empty() && roll(&mut spin, true).is_empty());
    assert_eq!(spin, Spin::Warned);
}
