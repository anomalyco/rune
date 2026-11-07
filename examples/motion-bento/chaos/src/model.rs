//! The bento's logical state and the actions that change it.
//!
//! An [`Action`] carries only indices. Whatever randomness an action needs (a fader's new level,
//! the puck's next spot) comes from that tile's own [`Rng`] inside the model, so two builds that
//! apply the same actions in the same order hold the same state, whatever they draw.

use crate::rng::Rng;

pub const SCENES: [&str; 4] = ["Focus", "Relax", "Away", "Night"];
pub const LIGHTS: [&str; 4] = ["Studio", "Desk lamp", "Hallway", "Porch"];
pub const FADERS: [&str; 5] = ["Kick", "Bass", "Keys", "Vox", "FX"];
pub const SCENE_NOTES: [&str; 4] = [
    "Cool, bright light. Notifications held.",
    "Warm light and something slow playing.",
    "Lights off, cameras armed, heat eased.",
    "Dim amber light. Quiet hours.",
];
pub const DOCK: [&str; 5] = ["Energy", "Genres", "Orb", "Transfers", "Visitors"];
/// Each dock tile's key.
pub const DOCK_KEYS: [&str; 5] = ["energy", "chips", "orb", "transfers", "visitors"];
pub const CHIPS: [&str; 8] = ["Ambient", "Lo-fi", "Jazz", "Rain", "Synth", "Piano", "Cafe", "Forest"];
/// Chips shown while the chip tile is collapsed.
pub const CHIPS_FOLDED: usize = 4;
pub const TRACKS: [(&str, &str); 8] = [
    ("Night Drive", "Lumen"),
    ("Paper Moons", "Hollow Coast"),
    ("Glasshouse", "Ines Varga"),
    ("Slow Orbit", "Kites"),
    ("Low Tide", "Marlowe"),
    ("Afterglow", "Sable Youth"),
    ("Neon Rain", "Oda"),
    ("Soft Static", "Fen"),
];
pub const QUEUE_LEN: usize = 4;
pub const FEED_KINDS: [&str; 8] = [
    "Front door unlocked",
    "Motion in the hallway",
    "Washer finished",
    "Package delivered",
    "Living room at 21°",
    "Garage closed",
    "Doorbell rang",
    "Air purifier on",
];
pub const FEED_LEN: usize = 4;
pub const FILES: [&str; 3] = ["mix-final.wav", "photos-oct.zip", "backup.tar"];
pub const ENERGY_BARS: usize = 7;
/// Recent visitor arrivals the visitors tile charts.
pub const VISITS: usize = 12;
/// The most visitors one arrival brings.
pub const VISIT_MAX: u32 = 48;
pub const TEMP_MIN: f32 = 16.0;
pub const TEMP_MAX: f32 = 28.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Action {
    Scene(u8),
    TempUp,
    TempDown,
    Light(u8),
    Fader(u8),
    Puck,
    Queue,
    Feed,
    Focus(u8),
    Energy,
    ChipsMore,
    Chip(u8),
    Orb,
    Transfer(u8),
    Visitors,
}

impl Action {
    /// The key of the element a person would tap for this action. Both builds key their elements
    /// this way, and the Rune script taps them.
    pub fn key(self) -> String {
        match self {
            Action::Scene(i) => format!("scene/seg{i}"),
            Action::TempUp => "thermo/up".into(),
            Action::TempDown => "thermo/down".into(),
            Action::Light(i) => format!("lights/light{i}"),
            Action::Fader(i) => format!("mixer/fader{i}"),
            Action::Puck => "xy/pad".into(),
            Action::Queue => "queue/head/skip".into(),
            Action::Feed => "feed/head/test".into(),
            Action::Focus(i) => format!("{}/head", DOCK_KEYS[i as usize % DOCK_KEYS.len()]),
            Action::Energy => "energy/bars".into(),
            Action::ChipsMore => "chips/head/more".into(),
            Action::Chip(i) => format!("chips/chip{i}"),
            Action::Orb => "orb/orb".into(),
            Action::Transfer(i) => format!("transfers/row{i}"),
            Action::Visitors => "visitors/count".into(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FeedItem {
    /// Unique for the run: the element's identity.
    pub id: u32,
    pub kind: usize,
    /// Minutes past the hour, for the time label.
    pub minute: u32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Card {
    pub id: u32,
    pub track: usize,
}

/// One arrival of visitors: a bar in the visitors chart.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Visit {
    pub id: u32,
    pub count: u32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Orb {
    /// 0..1 of the largest size.
    pub size: f32,
    /// OKLCH hue in degrees.
    pub hue: f32,
    /// Corner radius as a fraction of the side: 0.5 is a circle.
    pub round: f32,
    /// Degrees, always growing so the spring turns one way.
    pub turn: f32,
}

#[derive(Clone, Debug)]
pub struct Model {
    /// Actions applied so far: the header's counter.
    pub events: u32,
    pub scene: usize,
    pub temp: f32,
    pub lights: [bool; 4],
    pub faders: [f32; 5],
    /// The XY pad's puck, 0..1 on each axis.
    pub puck: [f32; 2],
    /// Front card first.
    pub queue: Vec<Card>,
    /// Newest first.
    pub feed: Vec<FeedItem>,
    /// Which dock tile is wide.
    pub focus: usize,
    pub energy: [f32; ENERGY_BARS],
    /// Tenths of a kWh, so both builds print the same digits.
    pub kwh_tenths: u32,
    pub chips_open: bool,
    pub chips: [bool; 8],
    pub orb: Orb,
    pub transfers: [f32; 3],
    pub visitors: u32,
    /// Oldest first.
    pub visits: Vec<Visit>,
    next_id: u32,
    minute: u32,
    rng: [Rng; 8],
}

const R_FADER: usize = 0;
const R_PUCK: usize = 1;
const R_QUEUE: usize = 2;
const R_FEED: usize = 3;
const R_ENERGY: usize = 4;
const R_ORB: usize = 5;
const R_TRANSFER: usize = 6;
const R_VISITORS: usize = 7;

impl Model {
    pub fn new(seed: u64) -> Model {
        let rng = std::array::from_fn(|i| Rng::new(seed.wrapping_mul(0x2545_f491_4f6c_dd1d) ^ (i as u64 + 1)));
        let mut m = Model {
            events: 0,
            scene: 0,
            temp: 21.5,
            lights: [true, false, true, false],
            faders: [0.72, 0.58, 0.44, 0.66, 0.3],
            puck: [0.62, 0.38],
            queue: Vec::new(),
            feed: Vec::new(),
            focus: 0,
            energy: [0.42, 0.55, 0.38, 0.7, 0.62, 0.48, 0.8],
            kwh_tenths: 128,
            chips_open: false,
            chips: [true, false, true, false, false, false, false, false],
            orb: Orb { size: 0.8, hue: 245.0, round: 0.5, turn: 0.0 },
            transfers: [0.35, 0.7, 0.12],
            visitors: 1284,
            visits: Vec::new(),
            next_id: 0,
            minute: 2,
            rng,
        };
        for track in 0..QUEUE_LEN {
            let id = m.id();
            m.queue.push(Card { id, track });
        }
        for count in [12, 18, 9, 22, 30, 26, 14, 19, 33, 28, 21, 17] {
            let id = m.id();
            m.visits.push(Visit { id, count });
        }
        for kind in [2, 0, 5] {
            let id = m.id();
            m.minute += 3;
            m.feed.insert(0, FeedItem { id, kind, minute: m.minute });
        }
        m
    }

    fn id(&mut self) -> u32 {
        self.next_id += 1;
        self.next_id
    }

    pub fn chips_shown(&self) -> usize {
        if self.chips_open { CHIPS.len() } else { CHIPS_FOLDED }
    }

    pub fn energy_kwh(&self) -> String {
        format!("{}.{}", self.kwh_tenths / 10, self.kwh_tenths % 10)
    }

    pub fn temp_label(&self) -> String {
        format!("{:.1}°", self.temp)
    }

    /// 0..1 around the thermostat's ring.
    pub fn temp_fraction(&self) -> f32 {
        (self.temp - TEMP_MIN) / (TEMP_MAX - TEMP_MIN)
    }

    pub fn apply(&mut self, action: Action) {
        self.events += 1;
        match action {
            Action::Scene(i) => self.scene = i as usize % SCENES.len(),
            Action::TempUp => self.temp = (self.temp + 0.5).min(TEMP_MAX),
            Action::TempDown => self.temp = (self.temp - 0.5).max(TEMP_MIN),
            Action::Light(i) => {
                let l = &mut self.lights[i as usize % 4];
                *l = !*l;
            }
            Action::Fader(i) => {
                let r = &mut self.rng[R_FADER];
                let f = &mut self.faders[i as usize % FADERS.len()];
                // Move far enough to see.
                let mut next = r.rangef(0.12, 0.96);
                if (next - *f).abs() < 0.2 {
                    next = if *f > 0.5 { *f - 0.35 } else { *f + 0.35 };
                }
                *f = next;
            }
            Action::Puck => {
                let r = &mut self.rng[R_PUCK];
                self.puck = [r.rangef(0.08, 0.92), r.rangef(0.1, 0.9)];
            }
            Action::Queue => {
                self.queue.remove(0);
                let last = self.queue.last().map_or(0, |c| c.track);
                let track = (last + 1 + self.rng[R_QUEUE].below(TRACKS.len() - 1)) % TRACKS.len();
                let id = self.id();
                self.queue.push(Card { id, track });
            }
            Action::Feed => {
                let r = &mut self.rng[R_FEED];
                let kind = r.below(FEED_KINDS.len());
                self.minute = (self.minute + 1 + r.below(4) as u32) % 60;
                let id = self.id();
                self.feed.insert(0, FeedItem { id, kind, minute: self.minute });
                self.feed.truncate(FEED_LEN);
            }
            Action::Focus(i) => self.focus = i as usize % DOCK.len(),
            Action::Energy => {
                let r = &mut self.rng[R_ENERGY];
                for b in &mut self.energy {
                    *b = r.rangef(0.15, 1.0);
                }
                self.kwh_tenths += 3 + r.below(23) as u32;
            }
            Action::ChipsMore => self.chips_open = !self.chips_open,
            Action::Chip(i) => {
                let c = &mut self.chips[i as usize % CHIPS.len()];
                *c = !*c;
            }
            Action::Orb => {
                let r = &mut self.rng[R_ORB];
                self.orb = Orb {
                    size: r.rangef(0.55, 1.0),
                    hue: (self.orb.hue + r.rangef(40.0, 140.0)) % 360.0,
                    round: [0.5, 0.32, 0.22][r.below(3)],
                    turn: self.orb.turn + [45.0, 90.0, 135.0][r.below(3)],
                };
            }
            Action::Transfer(i) => {
                let step = self.rng[R_TRANSFER].rangef(0.15, 0.4);
                let t = &mut self.transfers[i as usize % 3];
                *t = if *t >= 1.0 { 0.0 } else { (*t + step).min(1.0) };
            }
            Action::Visitors => {
                let count = 1 + self.rng[R_VISITORS].below(VISIT_MAX as usize) as u32;
                self.visitors += count;
                let id = self.id();
                self.visits.remove(0);
                self.visits.push(Visit { id, count });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_same_actions_give_the_same_state() {
        let actions = [Action::Fader(2), Action::Puck, Action::Queue, Action::Feed, Action::Orb, Action::Energy];
        let run = |seed| {
            let mut m = Model::new(seed);
            for a in actions.iter().chain(&actions) {
                m.apply(*a);
            }
            format!("{:?}", (m.faders, m.puck, m.queue, m.feed, m.orb, m.energy, m.kwh_tenths))
        };
        assert_eq!(run(3), run(3));
        assert_ne!(run(3), run(4));
    }

    #[test]
    fn lists_keep_their_length() {
        let mut m = Model::new(1);
        for _ in 0..20 {
            m.apply(Action::Queue);
            m.apply(Action::Feed);
            m.apply(Action::Visitors);
        }
        assert_eq!(m.visits.len(), VISITS);
        assert_eq!(m.queue.len(), QUEUE_LEN);
        assert_eq!(m.feed.len(), FEED_LEN);
        let ids: std::collections::HashSet<u32> = m.queue.iter().map(|c| c.id).chain(m.feed.iter().map(|f| f.id)).collect();
        assert_eq!(ids.len(), QUEUE_LEN + FEED_LEN);
    }
}
