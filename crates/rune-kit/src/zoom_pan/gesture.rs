//! The press on a canvas's background: a drag pans, a double-click fits.

use rune::machine::Effects;

use super::View;

/// How far a press moves before it pans rather than clicks.
const DRAG_THRESHOLD: f32 = 3.0;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum Gesture {
    #[default]
    Idle,
    /// A press is down. `travel` is the pointer's travel since the press as far as it has
    /// panned; `moved` once it passed the threshold; `before` the view the press found.
    Pressed { travel: [f32; 2], moved: bool, before: View },
}

impl Gesture {
    pub fn panning(&self) -> bool { matches!(self, Gesture::Pressed { moved: true, .. }) }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Event {
    Down { view: View },
    /// The pointer's travel since the press, in points.
    Moved { travel: [f32; 2] },
    /// The press ended; `clicks` is which click of a run it was.
    Up { clicks: u32 },
    Cancel,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Effect {
    Pan([f32; 2]),
    Fit,
    /// A cancelled drag puts back the view it found.
    Restore(View),
}

pub fn step(state: &mut Gesture, event: Event) -> Effects<Effect> {
    match (*state, event) {
        (_, Event::Down { view }) => {
            *state = Gesture::Pressed { travel: [0.0, 0.0], moved: false, before: view };
            Effects::none()
        }
        (Gesture::Pressed { travel, moved, before }, Event::Moved { travel: to }) => {
            let delta = [to[0] - travel[0], to[1] - travel[1]];
            if delta == [0.0, 0.0] || (!moved && to[0].hypot(to[1]) < DRAG_THRESHOLD) { return Effects::none(); }
            *state = Gesture::Pressed { travel: to, moved: true, before };
            Effects::one(Effect::Pan(delta))
        }
        (Gesture::Pressed { moved, .. }, Event::Up { clicks }) => {
            *state = Gesture::Idle;
            if !moved && clicks == 2 { Effects::one(Effect::Fit) } else { Effects::none() }
        }
        (Gesture::Pressed { moved, before, .. }, Event::Cancel) => {
            *state = Gesture::Idle;
            if moved { Effects::one(Effect::Restore(before)) } else { Effects::none() }
        }
        (Gesture::Idle, _) => Effects::none(),
    }
}
