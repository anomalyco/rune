//! Every short sequence of frames, present reports, flushes and waits through [`Presents`],
//! since Metal reports presents in whatever order and at whatever time it likes. Beside the real
//! queue and its [`step`] runs `Old`, a transcription of the fields and branches they replaced,
//! which calls none of them; both must write the same bytes and answer `waiting` the same.

use super::*;
use std::sync::Mutex;

// The old code.

fn old_ms_since(start: Instant, at: Instant) -> f64 {
    let ms = match at.checked_duration_since(start) {
        Some(d) => d.as_secs_f64() * 1000.0,
        None => -start.duration_since(at).as_secs_f64() * 1000.0,
    };
    old_r3(ms)
}

fn old_r3(v: f64) -> f64 {
    (v * 1000.0).round() / 1000.0
}

struct OldPending {
    line: String,
    seq: Option<u64>,
    present: Option<Option<f64>>,
    continuous: bool,
    age: u32,
}

struct Old {
    reports: Vec<Presented>,
    start: Instant,
    pending: VecDeque<OldPending>,
    last_present: Option<f64>,
    scratch: Vec<Presented>,
}

impl Old {
    fn new(start: Instant) -> Self {
        Self { reports: Vec::new(), start, pending: VecDeque::new(), last_present: None, scratch: Vec::new() }
    }

    fn push(&mut self, line: String, seq: Option<u64>, continuous: bool, out: &mut impl std::io::Write) {
        for p in &mut self.pending { p.age += 1; }
        self.pending.push_back(OldPending { line, seq, present: None, continuous, age: 0 });
        self.poll();
        self.write_ready(out, false);
    }

    fn waiting(&mut self) -> bool {
        self.poll();
        self.pending.iter().any(|p| p.seq.is_some() && p.present.is_none())
    }

    fn flush(&mut self, out: &mut impl std::io::Write) {
        self.poll();
        self.write_ready(out, true);
    }

    fn poll(&mut self) {
        self.scratch.append(&mut self.reports);
        for (seq, at) in self.scratch.drain(..) {
            if let Some(p) = self.pending.iter_mut().find(|p| p.seq == Some(seq)) {
                p.present = Some(at.map(|at| old_ms_since(self.start, at)));
            }
        }
    }

    fn write_ready(&mut self, out: &mut impl std::io::Write, all: bool) {
        while let Some(p) = self.pending.front() {
            if !(all || p.seq.is_none() || p.present.is_some() || p.age >= WAIT_FRAMES) { break; }
            let p = self.pending.pop_front().unwrap();
            let line = self.finish(p);
            let _ = writeln!(out, "{line}");
        }
    }

    fn finish(&mut self, p: OldPending) -> String {
        let mut line = p.line;
        if !p.continuous { self.last_present = None; }
        match (p.seq, p.present) {
            (Some(_), Some(Some(at))) => {
                let _ = write!(line, r#","present_ms":{at}"#);
                if let Some(last) = self.last_present {
                    let _ = write!(line, r#","present_dt_ms":{}"#, old_r3(at - last));
                }
                self.last_present = Some(at);
            }
            (Some(_), Some(None)) => {
                line.push_str(r#","present_ms":null"#);
                self.last_present = None;
            }
            _ => self.last_present = None,
        }
        line.push('}');
        line
    }
}

// The walk.

#[derive(Default)]
struct Fake(Mutex<Vec<Presented>>);

impl Source for Fake {
    fn take(&self, out: &mut Vec<Presented>) { out.append(&mut self.0.lock().unwrap()); }
}

#[derive(Clone, Copy, Debug)]
enum Ev {
    Push { seq: Option<u64>, continuous: bool },
    /// Drawable `seq` landed (`Some`, at a time set by the event's place) or was skipped.
    Report { seq: u64, landed: bool },
    Flush,
    Waiting,
    /// `WAIT_FRAMES` frames without a drawable, so every line still waiting expires.
    Expire,
}

const ALPHABET: [Ev; 9] = [
    Ev::Push { seq: Some(1), continuous: true },
    Ev::Push { seq: Some(2), continuous: false },
    Ev::Push { seq: None, continuous: true },
    Ev::Report { seq: 1, landed: true },
    Ev::Report { seq: 2, landed: true },
    Ev::Report { seq: 1, landed: false },
    Ev::Flush,
    Ev::Waiting,
    Ev::Expire,
];

fn walk(events: &[Ev]) {
    let start = Instant::now();
    let fake = Arc::new(Fake::default());
    let mut new = Presents::new(fake.clone(), start);
    let mut old = Old::new(start);
    let (mut new_out, mut old_out) = (Vec::new(), Vec::new());
    let mut frame = 0;
    for (k, &ev) in events.iter().enumerate() {
        match ev {
            Ev::Push { seq, continuous } => {
                frame += 1;
                new.push(format!(r#"{{"t":{frame}"#), seq, continuous, &mut new_out);
                old.push(format!(r#"{{"t":{frame}"#), seq, continuous, &mut old_out);
            }
            Ev::Report { seq, landed } => {
                let at = landed.then(|| start + Duration::from_micros(7_333 * (k as u64 + 1)));
                fake.0.lock().unwrap().push((seq, at));
                old.reports.push((seq, at));
            }
            Ev::Flush => {
                new.flush(&mut new_out);
                old.flush(&mut old_out);
            }
            Ev::Waiting => assert_eq!(new.waiting(), old.waiting(), "waiting after {:?}", &events[..=k]),
            Ev::Expire => for _ in 0..WAIT_FRAMES {
                frame += 1;
                new.push(format!(r#"{{"t":{frame}"#), None, true, &mut new_out);
                old.push(format!(r#"{{"t":{frame}"#), None, true, &mut old_out);
            },
        }
        assert_eq!(String::from_utf8_lossy(&new_out), String::from_utf8_lossy(&old_out), "after {:?}", &events[..=k]);
    }
    new.flush(&mut new_out);
    old.flush(&mut old_out);
    assert_eq!(String::from_utf8_lossy(&new_out), String::from_utf8_lossy(&old_out), "flushed after {events:?}");
}

#[test]
fn every_sequence_of_frames_and_reports_writes_what_the_old_queue_wrote() {
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
fn an_interval_needs_two_landed_frames_drawn_back_to_back() {
    let mut cadence = Cadence::Broken;
    let landed = |at| Written { present: Present::Landed { seq: 1, at }, continuous: true };
    assert_eq!(step(&mut cadence, landed(10.0)), Effects::one(Field::At { at: 10.0, dt: None }));
    assert_eq!(step(&mut cadence, landed(18.5)), Effects::one(Field::At { at: 18.5, dt: Some(8.5) }));
    let idle = Written { present: Present::Landed { seq: 2, at: 500.0 }, continuous: false };
    assert_eq!(step(&mut cadence, idle), Effects::one(Field::At { at: 500.0, dt: None }));
    assert_eq!(step(&mut cadence, Written { present: Present::Skipped { seq: 3 }, continuous: true }), Effects::one(Field::Null));
    assert_eq!(cadence, Cadence::Broken);
    assert_eq!(step(&mut cadence, Written { present: Present::Waiting { seq: 4 }, continuous: true }), Effects::none());
}
