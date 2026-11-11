use super::*;
use rune_motion::{AnimationStore, NodeId, Prop, PropClass};
use std::time::Duration;

#[derive(Clone, Copy, Debug)]
enum Input<'a> {
    Hold(Instant),
    Advance(Instant, bool, &'a [TrackId]),
    Present(Instant),
}

#[derive(Debug, PartialEq)]
enum Output {
    None,
    Frame(Step),
    Delay(Vec<TrackId>, f64),
}

// Keep the previous clock's fields and branches independent of the machine, including split.
struct Reference {
    last: Instant,
    starts: Vec<TrackId>,
}

impl Reference {
    fn hold(&mut self, at: Instant) {
        self.last = self.last.max(at);
        self.starts.clear();
    }

    fn apply(&mut self, input: Input<'_>) -> Output {
        match input {
            Input::Hold(at) => { self.hold(at); Output::None }
            Input::Advance(at, running, started) => {
                let gap = at.saturating_duration_since(self.last).as_secs_f64();
                self.hold(at);
                let step = if !running { Step { catch_up: 0.0, dt: 1.0 / 120.0 } } else {
                    let total = gap.clamp(0.0, 0.25);
                    let dt = total.min(1.0 / 120.0);
                    Step { catch_up: total - dt, dt }
                };
                self.starts.clear();
                self.starts.extend_from_slice(started);
                Output::Frame(step)
            }
            Input::Present(done) => {
                let late = done.saturating_duration_since(self.last).as_secs_f64().min(0.25);
                let effect = if late > 0.0 && !self.starts.is_empty() {
                    Output::Delay(self.starts.clone(), late)
                } else { Output::None };
                self.starts.clear();
                effect
            }
        }
    }
}

#[test]
fn clock_walk_matches_previous_frames() {
    let start = Instant::now();
    let at = |ms| start + Duration::from_millis(ms);
    let mut store = AnimationStore::new();
    let x = Prop::<f32>::custom(64, PropClass::Spatial);
    let a = store.set(NodeId(1), x, 0.0);
    let b = store.set(NodeId(2), x, 0.0);
    use Input::*;
    let alphabet = [Hold(at(1)), Hold(at(12)), Advance(at(4), true, &[a]),
        Advance(at(35), false, &[b, a]), Advance(at(300), true, &[]),
        Present(at(4)), Present(at(60)), Present(at(600))];
    for sequence in 0..alphabet.len().pow(5) {
        let mut clock = FrameClock::new(start);
        let mut reference = Reference { last: start, starts: Vec::new() };
        let mut n = sequence;
        for _ in 0..5 {
            let input = alphabet[n % alphabet.len()];
            n /= alphabet.len();
            let expected = reference.apply(input);
            let got = match input {
                Hold(at) => { clock.hold(at); Output::None }
                Advance(at, running, started) => {
                    let (step, starts) = clock.begin(at, running);
                    starts.extend_from_slice(started);
                    Output::Frame(step)
                }
                Present(done) => match clock.step(Event::Present(done)) {
                    Effects::None => Output::None,
                    Effects::One(Effect::Delay(late, starts)) => Output::Delay(starts.to_vec(), late),
                    _ => panic!("unexpected presentation effect"),
                },
            };
            assert_eq!(got, expected, "sequence {sequence}, input {input:?}");
            assert_eq!(clock.last(), reference.last, "sequence {sequence}, input {input:?}");
            let pending = match &clock.phase {
                Phase::Stepped { starts, .. } => starts.as_slice(),
                _ => &[],
            };
            assert_eq!(pending, reference.starts, "sequence {sequence}, input {input:?}");
        }
    }
}
