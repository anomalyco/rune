use super::*;
use crate::machine::Effects;

#[derive(Clone, Copy, Default)]
struct Old { idle_from: f64, idle_frames: u32, woken: [bool; 3] }

impl Old {
    fn label(&self) -> &'static str {
        match self.woken {
            [true, _, true] => "a `wake_after` loop whose `poll` returns true",
            [true, false, false] => "a `wake_after` loop",
            [true, true, false] => "a `wake_after` loop and a live value",
            [false, ..] => "a live value",
        }
    }

    fn end(&mut self, r: Reading) -> Option<(&'static str, f64)> {
        if !r.quiet || r.animating || r.same == Some(false) {
            self.idle_frames = 0;
            return None;
        }
        if r.same.is_none() || !(r.timer || r.live) { return None; }
        if self.idle_frames == 0 || r.clock - self.idle_from > 2.0 {
            (self.idle_from, self.idle_frames, self.woken) = (r.clock, 0, [false; 3]);
        }
        self.idle_frames += 1;
        for (woke, by) in self.woken.iter_mut().zip([r.timer, r.live, r.polled]) { *woke |= by; }
        if self.idle_frames == 120 { Some((self.label(), r.clock - self.idle_from)) } else { None }
    }
}

fn check(s: &mut Streak, old: &mut Old, r: Reading) {
    let expected = old.end(r);
    let got = match step(s, r) {
        Effects::None => None,
        Effects::One((woke, secs)) => Some((woke.label(), secs)),
        _ => panic!("extra warning"),
    };
    assert_eq!(got, expected);
    match *s {
        Streak::Watching => assert_eq!(old.idle_frames, 0),
        Streak::Counting { from, frames, woke } => assert_eq!((from, frames, woke.label()), (old.idle_from, old.idle_frames, old.label())),
    }
}

#[test]
fn idle_readings_walk() {
    let live = Reading { clock: 0.0, quiet: true, animating: false, same: Some(true), timer: false, live: true, polled: false };
    let events = [live, Reading { live: false, timer: true, ..live }, Reading { polled: true, ..live },
        Reading { live: false, polled: true, ..live }, Reading { quiet: false, ..live }, Reading { animating: true, ..live },
        Reading { same: None, ..live }, Reading { same: Some(false), ..live }, Reading { clock: 2.0, ..live },
        Reading { clock: 2.01, ..live }, Reading { live: true, timer: true, ..live }];
    fn walk(s: Streak, old: Old, events: &[Reading], depth: u8) {
        if depth == 0 { return; }
        for &r in events {
            let (mut s, mut old) = (s, old);
            check(&mut s, &mut old, r);
            walk(s, old, events, depth - 1);
        }
    }
    let (mut s, mut old) = (Streak::Watching, Old::default());
    walk(s, old, &events, 5);
    // Start just below the real threshold too; don't substitute a smaller test limit.
    for seed in [live, Reading { live: false, timer: true, ..live }] {
        check(&mut s, &mut old, Reading { quiet: false, ..live });
        for _ in 0..118 { check(&mut s, &mut old, seed); }
        walk(s, old, &events, 5);
    }
}
