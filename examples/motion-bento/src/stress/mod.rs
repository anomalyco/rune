//! Stress mode's view: the wall of controls under a header with the load and, with `--hud`, the
//! frame-time HUD. What the wall shows comes from `motion_bento_chaos::stress`; this only draws it.
//!
//! By default the wall is built the way Rune is meant to be used ([`live`]): memoised cards and
//! live values, so a frame where only numbers, bars and colours move runs no view and commits
//! nothing. `--naive` ([`naive`]) builds everything every frame.

mod hud;
mod live;
mod naive;

use motion_bento_chaos::hud::grade;
use motion_bento_chaos::look::{Lch, color, lch};
use motion_bento_chaos::stress::Wall;
use rune::El;

pub use live::Bindings;

use crate::State;

const SWITCH_OFF: Lch = lch(0.44, 0.012, 258.0);
const KNOB: Lch = lch(0.98, 0.0, 0.0);
const OVER: Lch = lch(0.66, 0.2, 25.0);
const HUD_W: f32 = 660.0;
const GRAPH_W: f32 = 300.0;
const GRAPH_H: f32 = 92.0;

pub fn grade_color(ms: f32) -> Lch {
    [color::GOOD, color::WARM, OVER][grade(ms)]
}

pub fn view(s: &State, wall: &Wall) -> El {
    match &s.live {
        Some(b) => live::view(s, wall, b),
        None => naive::view(s, wall),
    }
}
