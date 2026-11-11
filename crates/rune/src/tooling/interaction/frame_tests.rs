use super::*;

// The old tracker's fields and branches, with readings supplied by the walk's clock.
#[derive(Default)]
struct Old {
    frame_start: Option<Instant>,
    began: Option<(Instant, Option<f64>)>,
    frame_why: u16,
    timed: Option<OldTimed>,
    times: [f64; PHASES],
    active: bool,
}

#[derive(Debug, PartialEq)]
struct OldTimed {
    arrived: Instant, frame_start: Instant, handler_ms: f64, rank: u8, why: u16, commits: u32,
    built: Option<(Instant, [f64; PHASES], u32)>, cpu: (Option<f64>, Option<f64>), stacks: Option<u32>,
}

type Report = (OldTimed, Instant, Instant);
impl Old {
    fn begin(&mut self, queued: bool, now: Instant, cpu: Option<f64>) -> Option<Report> {
        let left = self.timed.as_ref().and_then(|t| t.built).map(|(built, ..)| built);
        let finished = left.and_then(|built| self.finish(built, None));
        self.frame_start = queued.then_some(now);
        self.began = Some((now, if queued { cpu } else { None }));
        self.frame_why = 0;
        finished
    }

    fn noted(&mut self, rank: u8, at: Option<Instant>, started: Instant, handler: Duration) {
        let frame_start = *self.frame_start.get_or_insert(started);
        let arrived = at.unwrap_or(frame_start).min(frame_start);
        match &mut self.timed {
            Some(t) if t.built.is_none() => {
                t.arrived = t.arrived.min(arrived);
                t.handler_ms += handler.as_secs_f64() * 1000.0;
                if rank > t.rank { t.rank = rank; }
            }
            _ => {
                let cpu = (self.began.and_then(|b| b.1), None);
                self.timed = Some(OldTimed { arrived, frame_start, handler_ms: handler.as_secs_f64() * 1000.0,
                    rank, why: 0, commits: 0, built: None, cpu, stacks: None });
                self.times = [0.0; PHASES];
                self.active = true;
            }
        }
    }

    fn committed(&mut self, why: u16) {
        self.frame_why |= why;
        if let Some(t) = self.timed.as_mut().filter(|t| t.built.is_none()) {
            t.why |= why;
            t.commits += 1;
        }
    }

    fn build(&mut self, shaped: u32, now: Instant, cpu: Option<f64>, stacks: Option<u32>, slow_ms: f64) -> Option<(Instant, Instant, u16, u32)> {
        let why = self.frame_why;
        self.frame_start = None;
        let began = self.began.take();
        let Some(t) = self.timed.as_mut().filter(|t| t.built.is_none()) else {
            let (start, stacks) = (began?.0, stacks?);
            let work = now.saturating_duration_since(start).as_secs_f64() * 1000.0;
            return (work > slow_ms).then_some((start, now, why, stacks));
        };
        t.built = Some((now, std::mem::take(&mut self.times), shaped));
        t.cpu.1 = t.cpu.0.zip(cpu).map(|(from, to)| to - from);
        t.stacks = stacks;
        self.active = false;
        None
    }

    fn finish(&mut self, submitted: Instant, presents_at: Option<Instant>) -> Option<Report> {
        let timed = self.timed.take_if(|t| t.built.is_some())?;
        let (built, ..) = timed.built?;
        let submitted = submitted.max(built);
        let present = presents_at.map_or(submitted, |p| p.max(submitted));
        Some((timed, submitted, present))
    }
}

fn compare(got: Option<Finished>, expected: Option<Report>) {
    assert_eq!(got.is_some(), expected.is_some());
    if let (Some(f), Some((t, submitted, present))) = (got, expected) {
        assert_eq!((f.submitted, f.present), (submitted, present));
        assert_eq!(Some((f.built, f.phases, f.shaped)), t.built);
        assert_eq!((f.cpu_ms, f.stacks.map(|s| s.taken)), (t.cpu.1, t.stacks));
        let g = f.timed;
        assert_eq!((g.arrived, g.frame_start, g.handler_ms, g.note.rank, g.why, g.commits),
                   (t.arrived, t.frame_start, t.handler_ms, t.rank, t.why, t.commits));
    }
}

#[test]
fn frame_phase_walk() {
    // A fixed threshold: the walk never reads `RUNE_PERF_SLOW_MS` from the environment. It sits
    // between the walk's 30 ms steps so both a slow and a quick untimed frame come up.
    const SLOW_MS: f64 = 75.0;
    let base = Instant::now();
    let (mut slow, mut quick) = (0, 0);
    for mut path in 0..8usize.pow(5) {
        let mut tracker = Tracker::default();
        let mut old = Old::default();
        TIMES.set([0.0; PHASES]);
        ACTIVE.set(false);
        for i in 1..=5 {
            let e = path % 8;
            path /= 8;
            let now = base + Duration::from_millis(i * 30);
            let cpu = (i % 3 != 0).then_some(i as f64);
            match e {
                0 | 1 => compare(tracker.begin_with(e == 1, SLOW_MS, || now, || cpu), old.begin(e == 1, now, cpu)),
                2 | 3 => {
                    let rank = e as u8;
                    let at = (e == 3).then_some(base);
                    let handler = Duration::from_micros(i);
                    old.noted(rank, at, now, handler);
                    let note = Note { rank, route: tracker.names.fixed("page"), control: tracker.names.fixed("button"), action: "press" };
                    tracker.noted(note, at, now, handler);
                }
                4 => {
                    old.committed(1 << i);
                    tracker.committed(1 << i);
                }
                5 => {
                    let taken = (i % 3 != 0).then_some(i as u32);
                    let untimed = !matches!(tracker.frame, Frame::Open { .. });
                    let expected = old.build(i as u32, now, cpu, taken, SLOW_MS);
                    let stacks = taken.map(|taken| Stacks { taken, waiting: taken, cpu_share: None, raw: vec![] });
                    let got = tracker.build_with(i as u32, 99, stacks, SLOW_MS, || now, || cpu);
                    assert_eq!(got.map(|f| (f.start, f.built, f.why, f.stacks.taken)), expected);
                    match (untimed, expected.is_some()) {
                        (true, true) => slow += 1,
                        (true, false) if taken.is_some() => quick += 1,
                        _ => (),
                    }
                }
                6 | 7 => {
                    let (sub, present) = if e == 6 { (base, Some(now)) } else { (now, Some(base)) };
                    compare(tracker.finish_with(sub, present, SLOW_MS), old.finish(sub, present));
                }
                _ => unreachable!(),
            }
            if old.active { old.times[Phase::Paint as usize] += i as f64; }
            if active() { add(Phase::Paint, i as f64); }
            assert_eq!((active(), TIMES.get()), (old.active, old.times));
            match (&tracker.frame, &old.timed) {
                (Frame::Idle, None) => assert!(old.began.is_none()),
                (Frame::Begun { at, queued, cpu, why }, None) => {
                    assert_eq!(Some((*at, *cpu)), old.began);
                    assert_eq!((queued.then_some(*at), *why), (old.frame_start, old.frame_why));
                }
                (Frame::Open { timed, input_start, cpu }, Some(t)) => {
                    assert!(t.built.is_none());
                    assert_eq!((*input_start, *cpu, timed.arrived, timed.frame_start, timed.note.rank, timed.handler_ms, timed.why, timed.commits),
                              (old.frame_start, t.cpu.0, t.arrived, t.frame_start, t.rank, t.handler_ms, t.why, t.commits));
                }
                (Frame::Built(b), Some(t)) => assert_eq!((Some((b.at, b.phases, b.shaped)), b.cpu_ms, b.stacks.as_ref().map(|s| s.taken)), (t.built, t.cpu.1, t.stacks)),
                _ => panic!("phase mismatch"),
            }
        }
        tracker.rolled_since = None; // Don't export the walk's synthetic samples at drop.
    }
    assert!(slow > 0 && quick > 0, "the walk covers a slow untimed frame ({slow}) and a quick one ({quick})");
}
