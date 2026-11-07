//! The chaos schedule: a seeded list of timed actions, quiet stretches broken by bursts.
//!
//! The schedule plays the actions against its own [`Model`] as it picks them, so every action is
//! one a person could take at that moment (a chip that's showing, a scene that isn't already on).

use crate::config::Config;
use crate::model::{Action, CHIPS, DOCK, FADERS, Model, SCENES};
use crate::rng::Rng;

/// Chaos starts once the first frames have settled.
pub const START: f64 = 0.6;
/// A Rune script taps this long before the event's time and lifts at it, so the action lands at
/// `t` in a recording.
pub const TAP_LEAD: f64 = 0.01;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Event {
    pub t: f64,
    pub action: Action,
}

// Relative odds per kind of action.
const KINDS: [u32; 15] = [5, 4, 4, 9, 12, 8, 6, 8, 6, 6, 3, 8, 6, 8, 6];

fn pick(rng: &mut Rng, m: &Model) -> Action {
    let other = |rng: &mut Rng, n: usize, current: usize| ((current + 1 + rng.below(n - 1)) % n) as u8;
    match rng.weighted(&KINDS) {
        0 => Action::Scene(other(rng, SCENES.len(), m.scene)),
        1 => Action::TempUp,
        2 => Action::TempDown,
        3 => Action::Light(rng.below(4) as u8),
        4 => Action::Fader(rng.below(FADERS.len()) as u8),
        5 => Action::Puck,
        6 => Action::Queue,
        7 => Action::Feed,
        8 => Action::Focus(other(rng, DOCK.len(), m.focus)),
        9 => Action::Energy,
        10 => Action::ChipsMore,
        11 => Action::Chip(rng.below(m.chips_shown().min(CHIPS.len())) as u8),
        12 => Action::Orb,
        13 => Action::Transfer(rng.below(3) as u8),
        _ => Action::Visitors,
    }
}

/// The run's actions in time order, from [`START`] to `config.duration`.
pub fn schedule(config: &Config) -> Vec<Event> {
    let mut rng = Rng::new(config.seed);
    let mut model = Model::new(config.seed);
    let mut events = Vec::new();
    let (lo, hi) = config.gap();
    let mut t = START;
    let mut next_burst = START + rng.range(1.2, 2.2);
    while t < config.duration {
        let action = pick(&mut rng, &model);
        model.apply(action);
        events.push(Event { t, action });
        if t >= next_burst {
            // A burst: 6–12 actions 30–60 ms apart, then a breath.
            let n = 6 + rng.below(7);
            for _ in 0..n {
                t += rng.range(0.03, 0.06);
                if t >= config.duration {
                    break;
                }
                let action = pick(&mut rng, &model);
                model.apply(action);
                events.push(Event { t, action });
            }
            t += rng.range(0.35, 0.6);
            next_burst = t + rng.range(2.0, 3.0);
        } else {
            t += rng.range(lo, hi);
        }
    }
    events
}

/// The schedule as a Rune headless script: one tap per action at its key, and an `end` mark a
/// second after the last so the springs settle on camera.
pub fn script_json(config: &Config) -> String {
    let mut out = String::from("{\"events\": [\n");
    for e in schedule(config) {
        out.push_str(&format!(
            "  {{\"t\": {:.4}, \"type\": \"tap\", \"key\": \"{}\", \"hold\": {TAP_LEAD}}},\n",
            e.t - TAP_LEAD,
            e.action.key()
        ));
    }
    out.push_str(&format!("  {{\"t\": {:.4}, \"type\": \"mark\", \"name\": \"end\"}}\n]}}\n", config.duration + 1.0));
    out
}

/// Plays a schedule against a clock: hands back each action once its time has come.
#[derive(Clone, Debug)]
pub struct Player {
    events: Vec<Event>,
    next: usize,
}

impl Player {
    pub fn new(config: &Config) -> Player {
        Player { events: schedule(config), next: 0 }
    }

    /// The actions due by `t` seconds that haven't been handed out yet.
    pub fn due(&mut self, t: f64) -> impl Iterator<Item = Action> + '_ {
        let start = self.next;
        while self.next < self.events.len() && self.events[self.next].t <= t {
            self.next += 1;
        }
        self.events[start..self.next].iter().map(|e| e.action)
    }

    pub fn done(&self) -> bool {
        self.next == self.events.len()
    }

    /// The time of the next action, if any.
    pub fn next_at(&self) -> Option<f64> {
        self.events.get(self.next).map(|e| e.t)
    }

    pub fn len(&self) -> usize {
        self.events.len()
    }

    pub fn is_empty(&self) -> bool {
        self.events.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(seed: u64, duration: f64, intensity: f64) -> Config {
        Config { seed, duration, intensity, ..Config::default() }
    }

    #[test]
    fn a_seed_names_one_schedule() {
        assert_eq!(schedule(&config(5, 10.0, 1.0)), schedule(&config(5, 10.0, 1.0)));
        assert_ne!(schedule(&config(5, 10.0, 1.0)), schedule(&config(6, 10.0, 1.0)));
    }

    #[test]
    fn intensity_scales_the_rate_and_bursts_happen() {
        let calm = schedule(&config(1, 20.0, 1.0));
        let busy = schedule(&config(1, 20.0, 2.0));
        assert!(busy.len() as f64 > calm.len() as f64 * 1.4, "{} vs {}", busy.len(), calm.len());
        let tight = calm.windows(2).filter(|w| w[1].t - w[0].t < 0.07).count();
        assert!(tight >= 30, "bursts: {tight} tight gaps");
        assert!(calm.windows(2).all(|w| w[1].t > w[0].t && w[1].t - w[0].t >= 0.03 - 1e-9));
        assert!(calm.iter().all(|e| e.t >= START && e.t < 20.0));
    }

    #[test]
    fn every_action_is_takeable() {
        let mut m = Model::new(2);
        for e in schedule(&config(2, 30.0, 3.0)) {
            match e.action {
                Action::Scene(i) => assert_ne!(i as usize, m.scene),
                Action::Focus(i) => assert_ne!(i as usize, m.focus),
                Action::Chip(i) => assert!((i as usize) < m.chips_shown()),
                _ => {}
            }
            m.apply(e.action);
        }
    }

    #[test]
    fn the_player_hands_out_each_action_once() {
        let c = config(3, 6.0, 1.0);
        let mut p = Player::new(&c);
        let mut seen = 0;
        let mut t = 0.0;
        while !p.done() {
            t += 1.0 / 120.0;
            seen += p.due(t).count();
        }
        assert_eq!(seen, schedule(&c).len());
        assert!(script_json(&c).contains("\"key\": \""));
    }
}
