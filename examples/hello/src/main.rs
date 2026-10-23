//! The smallest Rune app: one piece of state, a view of it, and a switch that flips it on a spring.
//! `cargo run -p hello` opens the window; `-- --snap hello.png --scene on` renders it headless.

use rune::tok::{self, motion, type_};
use rune::{AppSpec, Cx, El, col, el, text};

#[derive(Default)]
struct State {
    on: bool,
}

fn view(s: &State, _: &mut Cx<State>) -> El {
    let t = tok::theme(true);
    let label = text(if s.on { "Lights on" } else { "Lights off" })
        .font(type_::TITLE).color(t.text);
    let knob = el().key("knob").size(18.0, 18.0).radius(9.0).fill(t.knob)
        .absolute(if s.on { 18.0 } else { 2.0 }, 2.0);
    let switch = el().key("switch").role("switch").size(38.0, 22.0).radius(11.0)
        .fill(if s.on { tok::accent(0, true) } else { t.control_off })
        .motion(motion::SNAPPY)
        .on_press(|s: &mut State| s.on = !s.on)
        .child(knob);
    col().w_full().h_full().center().gap(16.0).fill(t.bg).child(label).child(switch)
}

fn main() {
    rune::run(AppSpec::<State> {
        scenes: &[("on", |s| s.on = true)],
        ..AppSpec::new("Hello", (320.0, 200.0), State::default, view)
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use rune::{Env, Runtime};

    #[test]
    fn a_tap_flips_the_switch_and_the_knob_springs_across() {
        let env = Env { system_dark: true, scale: 2.0 };
        let mut rt = Runtime::new(State::default(), view, (320.0, 200.0), env);
        rt.frame(0.0);
        let knob_x = |rt: &Runtime<State>| rt.presented("knob").unwrap().0[0];
        let off = knob_x(&rt);

        let (x, y) = rt.find("switch").unwrap();
        rt.pointer_down(x, y);
        rt.pointer_up(x, y);
        for _ in 0..6 { rt.frame(1.0 / 60.0); }
        assert!(rt.state.on);
        let mid = knob_x(&rt);
        for _ in 0..60 { rt.frame(1.0 / 60.0); }
        let on = knob_x(&rt);
        assert!(off < mid && mid < on, "the knob travels: {off} → {mid} → {on}");
        assert!((on - off - 16.0).abs() < 0.01, "and lands 16 points over");
    }
}
