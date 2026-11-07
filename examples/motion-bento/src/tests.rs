use motion_bento_chaos::schedule::{TAP_LEAD, schedule};
use motion_bento_chaos::{Config, Model};
use rune::{Env, Runtime};

use super::{State, view};
use motion_bento_chaos::look::grid;

/// Taps every scheduled action's key the way the headless script does, and checks the window's
/// model after each tap against the model the schedule simulated. A tap that lands on the wrong
/// element, or on none, shows up as the first action where the two part.
#[test]
fn script_taps_reach_the_same_state_as_the_schedule() {
    for seed in [1, 2, 3] {
        let cfg = Config { seed, duration: 12.0, ..Config::default() };
        let mut rt = Runtime::new(State::new(), view, (grid::WIDTH, grid::HEIGHT), Env { system_dark: true, scale: 1.0 });
        rt.state.player = None;
        rt.state.m = Model::new(seed);
        let mut want = Model::new(seed);
        let dt = 1.0 / 60.0;
        let mut t = 0.0;
        rt.frame(0.0);
        for (n, e) in schedule(&cfg).into_iter().enumerate() {
            while t + dt <= e.t - TAP_LEAD {
                t += dt;
                rt.frame(dt);
            }
            let key = e.action.key();
            let (x, y) = rt.find(&key).unwrap_or_else(|| panic!("seed {seed}: event {n} at {:.3}: no element {key}", e.t));
            rt.pointer_move(x, y);
            rt.pointer_down(x, y);
            t += dt;
            rt.frame(dt);
            rt.pointer_up(x, y);
            rt.frame(0.0);
            want.apply(e.action);
            assert_eq!(
                format!("{:?}", rt.state.m),
                format!("{want:?}"),
                "seed {seed}: event {n} at {:.3}: tapping {key} ({x:.0}, {y:.0}) for {:?}",
                e.t,
                e.action
            );
        }
    }
}
