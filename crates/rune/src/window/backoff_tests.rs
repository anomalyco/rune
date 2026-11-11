use super::*;

const MS: Duration = Duration::from_millis(1);

#[test]
fn an_occluded_miss_backs_off_until_a_frame_presents() {
    let t0 = Instant::now();
    let mut backoff = Backoff::default();
    assert_eq!(backoff.deadline(), None, "nothing waits");
    let miss = |backoff: &mut Backoff, missed, now| {
        let effect = backoff.step(BackoffEvent::Missed { missed, paced: false, now });
        (backoff.deadline(), effect.is_empty())
    };
    assert_eq!(miss(&mut backoff, Missed::Occluded, t0), (Some(t0 + OCCLUDED_RETRY), false), "an occluded frame re-checks after the first wait");
    assert!(backoff.waiting(t0) && !backoff.waiting(t0 + OCCLUDED_RETRY), "until it is due");
    assert_eq!(miss(&mut backoff, Missed::Occluded, t0), (Some(t0 + OCCLUDED_RETRY * 2), false), "each miss doubles it");
    for _ in 0..8 { miss(&mut backoff, Missed::Occluded, t0); }
    assert_eq!(backoff.deadline(), Some(t0 + OCCLUDED_MAX), "up to a re-check every two seconds");
    backoff.step(BackoffEvent::Lapsed); // The draw clears the wait before it acquires.
    backoff.step(BackoffEvent::Presented);
    assert_eq!(backoff.deadline(), None, "a frame that presents waits for nothing");
    assert_eq!(miss(&mut backoff, Missed::Occluded, t0).0, Some(t0 + OCCLUDED_RETRY), "and the wait starts over");
}

#[test]
fn a_display_link_draws_a_timeout_on_its_next_tick() {
    let now = Instant::now();
    let mut backoff = Backoff::default();
    assert!(backoff.step(BackoffEvent::Missed { missed: Missed::Timeout, paced: true, now }).is_empty());
    assert_eq!(backoff.deadline(), None, "a running link retries on its tick");
    backoff.step(BackoffEvent::Missed { missed: Missed::Timeout, paced: false, now });
    assert_eq!(backoff.deadline(), Some(now + TIMEOUT_RETRY), "an unpaced one waits about a frame");
    backoff.step(BackoffEvent::Missed { missed: Missed::Outdated, paced: false, now });
    assert_eq!(backoff.deadline(), Some(now + OCCLUDED_RETRY), "a lost surface is reconfigured on a slow retry");
}

#[test]
fn a_wait_that_lapses_keeps_its_backoff_until_a_frame_presents() {
    let now = Instant::now();
    let mut backoff = Backoff::default();
    backoff.step(BackoffEvent::Missed { missed: Missed::Occluded, paced: false, now });
    backoff.step(BackoffEvent::Missed { missed: Missed::Occluded, paced: false, now });
    backoff.step(BackoffEvent::Lapsed);
    assert_eq!(backoff.deadline(), None, "the retry came due");
    backoff.step(BackoffEvent::Missed { missed: Missed::Occluded, paced: false, now });
    assert_eq!(backoff.deadline(), Some(now + OCCLUDED_RETRY * 4), "nothing showed the surface can draw, so it carries on");
}

#[test]
fn an_uncovered_window_starts_the_occluded_wait_over_but_keeps_the_hidden_one() {
    let now = Instant::now();
    let mut backoff = Backoff::seeded(OCCLUDED_RETRY * 4, TIMEOUT_RETRY * 4);
    backoff.step(BackoffEvent::At(now + TIMER_FRAME));
    assert_eq!(backoff.deadline(), Some(now + TIMER_FRAME), "a timer paces the next frame");
    backoff.step(BackoffEvent::Uncovered);
    assert_eq!(backoff.deadline(), None, "an occlusion change waits for nothing");
    backoff.step(BackoffEvent::Missed { missed: Missed::Occluded, paced: false, now });
    assert_eq!(backoff.deadline(), Some(now + OCCLUDED_RETRY), "the occluded wait starts over");
    backoff.step(BackoffEvent::HiddenMissed { preparing: false, now });
    assert_eq!(backoff.deadline(), Some(now + TIMEOUT_RETRY * 4), "the hidden one carries on");
}

#[test]
fn a_hidden_refresh_backs_off_but_leaves_its_retry_to_the_first_frame_deadline() {
    let now = Instant::now();
    let mut backoff = Backoff::default();
    backoff.step(BackoffEvent::HiddenMissed { preparing: true, now });
    assert_eq!(backoff.deadline(), None, "a prepared first frame retries it itself");
    backoff.step(BackoffEvent::HiddenMissed { preparing: false, now });
    assert_eq!(backoff.deadline(), Some(now + TIMEOUT_RETRY));
    backoff.step(BackoffEvent::HiddenMissed { preparing: false, now });
    assert_eq!(backoff.deadline(), Some(now + TIMEOUT_RETRY * 2), "and doubles");
    for _ in 0..10 { backoff.step(BackoffEvent::HiddenMissed { preparing: false, now }); }
    assert_eq!(backoff.deadline(), Some(now + OCCLUDED_MAX), "up to the same cap");
    backoff.step(BackoffEvent::Presented);
    backoff.step(BackoffEvent::HiddenMissed { preparing: false, now });
    assert_eq!(backoff.deadline(), Some(now + TIMEOUT_RETRY), "a presented frame starts it over");
}

#[test]
fn only_a_timer_asking_leaves_a_quiet_frame() {
    let now = Instant::now();
    let mut frame = TimerFrame::default();
    assert!(!frame.quiet() && !frame.stalled(now));
    frame.asked_quiet(now);
    assert!(frame.quiet(), "only a timer asked for the frame");
    frame.drawn();
    assert!(frame.quiet(), "a draw that returns before its decision leaves it");
    assert!(frame.take_quiet() && !frame.quiet(), "the draw takes it once");
    frame.asked_quiet(now);
    frame.no_longer_quiet();
    assert!(!frame.quiet(), "a window event says something else asked");
    frame.asked_quiet(now + MS);
    assert!(frame.take_quiet(), "a later timer asks quietly again");
}

#[test]
fn a_stalled_frame_is_asked_for_again_from_now_and_stays_quiet() {
    let now = Instant::now();
    let mut frame = TimerFrame::default();
    frame.asked_quiet(now);
    assert!(!frame.stalled(now + TIMER_FRAME_STALL - MS) && frame.stalled(now + TIMER_FRAME_STALL));
    frame.stalled_again(now + TIMER_FRAME_STALL);
    assert!(!frame.stalled(now + TIMER_FRAME_STALL), "the next ask starts its bound over");
    assert!(frame.quiet(), "and only a timer asked");
    assert_eq!(frame.wait(Some(now), now + TIMER_FRAME_STALL), Some(now + 2 * TIMER_FRAME_STALL));
}

/// The fields this machine replaced, kept apart exactly as they were: `Win::retry`,
/// `occluded_wait` and `hidden_wait`, and `quiet` beside the frame a timer owed. Every sequence of
/// events fed to both must leave the same deadline, the same waits, and the same quiet frame.
#[derive(Clone, Copy, Debug)]
struct Apart {
    retry: Option<Instant>,
    occluded: Duration,
    hidden: Duration,
    quiet: bool,
    owed: Option<Instant>,
}

/// The retry rule as the window had it before this machine, copied here literally: the reference
/// must not call the production code it checks, or a change to the doubling would move both sides.
fn retry(missed: Missed, paced: bool, now: Instant, occluded_wait: &mut Duration) -> Option<Instant> {
    match missed {
        Missed::Occluded => {
            let at = now + (*occluded_wait).clamp(OCCLUDED_RETRY, OCCLUDED_MAX);
            *occluded_wait = (*occluded_wait * 2).min(OCCLUDED_MAX);
            Some(at)
        }
        Missed::Timeout if paced => None,
        Missed::Timeout => Some(now + TIMEOUT_RETRY),
        Missed::Outdated | Missed::Unconfigured => Some(now + OCCLUDED_RETRY),
    }
}

impl Apart {
    fn new() -> Self { Apart { retry: None, occluded: OCCLUDED_RETRY, hidden: TIMEOUT_RETRY, quiet: false, owed: None } }

    fn step(&mut self, step: Step, now: Instant) {
        match step {
            Step::Occluded => self.retry = retry(Missed::Occluded, false, now, &mut self.occluded),
            Step::Timeout => self.retry = retry(Missed::Timeout, false, now, &mut self.occluded),
            Step::PacedTimeout => self.retry = retry(Missed::Timeout, true, now, &mut self.occluded),
            Step::Outdated => self.retry = retry(Missed::Outdated, false, now, &mut self.occluded),
            Step::Unconfigured => self.retry = retry(Missed::Unconfigured, false, now, &mut self.occluded),
            Step::Hidden => {
                self.retry = Some(now + self.hidden);
                self.hidden = (self.hidden * 2).min(OCCLUDED_MAX);
            }
            Step::HiddenPreparing => self.retry = None,
            Step::At => self.retry = Some(now + TIMER_FRAME),
            Step::Lapsed => self.retry = None,
            Step::Uncovered => {
                self.retry = None;
                self.occluded = OCCLUDED_RETRY;
            }
            // A frame drawn: `draw` clears the wait before it takes its drawable.
            Step::Present => {
                self.retry = None;
                self.occluded = OCCLUDED_RETRY;
                self.hidden = TIMEOUT_RETRY;
            }
            Step::TimerAsk => {
                self.owed.get_or_insert(now);
                self.quiet = true;
            }
            Step::TimerStallAgain => self.owed = Some(now),
            Step::TimerDrawn => self.owed = None,
            Step::QuietCleared | Step::TakeQuiet => self.quiet = false,
        }
    }
}

/// A frame timer, a window event and a present arrive in any order; every sequence of these
/// steps through both the machine and the fields it replaced, checked at each one.
#[derive(Clone, Copy, Debug)]
enum Step {
    Occluded,
    Timeout,
    PacedTimeout,
    Outdated,
    Unconfigured,
    Hidden,
    HiddenPreparing,
    At,
    Lapsed,
    Uncovered,
    Present,
    TimerAsk,
    TimerStallAgain,
    TimerDrawn,
    QuietCleared,
    TakeQuiet,
}

fn every_sequence(alphabet: &[Step], max: usize, mut f: impl FnMut(&[Step])) {
    fn walk(alphabet: &[Step], seq: &mut Vec<Step>, max: usize, f: &mut impl FnMut(&[Step])) {
        f(seq);
        if seq.len() == max { return; }
        for step in alphabet {
            seq.push(*step);
            walk(alphabet, seq, max, f);
            seq.pop();
        }
    }
    walk(alphabet, &mut Vec::new(), max, &mut f);
}

fn holds_up(seq: &[Step]) {
    let t0 = Instant::now();
    let (mut backoff, mut frame, mut apart) = (Backoff::default(), TimerFrame::default(), Apart::new());
    for (k, step) in seq.iter().enumerate() {
        let now = t0 + MS * (11 * k as u32);
        match *step {
            Step::Occluded => { backoff.step(BackoffEvent::Missed { missed: Missed::Occluded, paced: false, now }); }
            Step::Timeout => { backoff.step(BackoffEvent::Missed { missed: Missed::Timeout, paced: false, now }); }
            Step::PacedTimeout => { backoff.step(BackoffEvent::Missed { missed: Missed::Timeout, paced: true, now }); }
            Step::Outdated => { backoff.step(BackoffEvent::Missed { missed: Missed::Outdated, paced: false, now }); }
            Step::Unconfigured => { backoff.step(BackoffEvent::Missed { missed: Missed::Unconfigured, paced: false, now }); }
            Step::Hidden => { backoff.step(BackoffEvent::HiddenMissed { preparing: false, now }); }
            Step::HiddenPreparing => { backoff.step(BackoffEvent::HiddenMissed { preparing: true, now }); }
            Step::At => { backoff.step(BackoffEvent::At(now + TIMER_FRAME)); }
            Step::Lapsed => { backoff.step(BackoffEvent::Lapsed); }
            Step::Uncovered => { backoff.step(BackoffEvent::Uncovered); }
            Step::Present => {
                backoff.step(BackoffEvent::Lapsed);
                backoff.step(BackoffEvent::Presented);
            }
            Step::TimerAsk => frame.asked_quiet(now),
            Step::TimerStallAgain => frame.stalled_again(now),
            Step::TimerDrawn => frame.drawn(),
            Step::QuietCleared => frame.no_longer_quiet(),
            Step::TakeQuiet => { frame.take_quiet(); }
        }
        apart.step(*step, now);
        assert_eq!(backoff.deadline(), apart.retry, "{seq:?} at {k}");
        assert_eq!(backoff.waiting(now), apart.retry.is_some_and(|at| at > now), "{seq:?} at {k}");
        // The waits behind the deadline, which no query shows: a miss of each kind now, on both.
        let mut missed = backoff;
        missed.step(BackoffEvent::Missed { missed: Missed::Occluded, paced: false, now });
        let mut wait = apart.occluded;
        assert_eq!(missed.deadline(), retry(Missed::Occluded, false, now, &mut wait), "the occluded wait after {seq:?} at {k}");
        let mut hidden = backoff;
        hidden.step(BackoffEvent::HiddenMissed { preparing: false, now });
        assert_eq!(hidden.deadline(), Some(now + apart.hidden), "the hidden wait after {seq:?} at {k}");
        // The quiet timer frame, as the loop waits on it.
        assert_eq!(frame.quiet(), apart.quiet, "quiet after {seq:?} at {k}");
        assert_eq!(frame.stalled(now), apart.owed.is_some_and(|at| now >= at + TIMER_FRAME_STALL), "{seq:?} at {k}");
        assert_eq!(frame.wait(Some(now), now), Some(apart.owed.map_or(now, |at| at + TIMER_FRAME_STALL)), "{seq:?} at {k}");
    }
}

#[test]
fn every_order_of_events_leaves_the_same_deadline_and_waits() {
    let all = [Step::Occluded, Step::Timeout, Step::PacedTimeout, Step::Outdated, Step::Unconfigured, Step::Hidden, Step::HiddenPreparing, Step::At,
        Step::Lapsed, Step::Uncovered, Step::Present, Step::TimerAsk, Step::TimerStallAgain, Step::TimerDrawn, Step::QuietCleared, Step::TakeQuiet];
    every_sequence(&all, 3, holds_up);
    let common = [Step::Occluded, Step::Unconfigured, Step::Hidden, Step::Present, Step::Uncovered, Step::TimerAsk, Step::TimerStallAgain,
        Step::QuietCleared, Step::TakeQuiet];
    every_sequence(&common, 6, holds_up);
}
