use std::cell::Cell;

use rune::{Cx, El, Env, Runtime};
use rune_kit::{Kit, RollingText};

struct State {
    value: u32,
    reads: bool,
    views: Cell<usize>,
}

fn view(s: &State, cx: &mut Cx<State>) -> El {
    s.views.set(s.views.get() + 1);
    if s.reads {
        cx.layout_size("probe");
    } else {
        cx.layout_with("probe", |r| r[2]);
    }
    rune::col()
        .padding(20.0)
        .child(rune::el().key("probe").size(s.value as f32 * 10.0, 10.0))
        .child(RollingText::new("retry-count", s.value.to_string()).value(s.value as f64).build(&Kit::neutral(true)))
}

#[test]
fn layout_read_retries_keep_the_changed_glyphs_enter_and_direction() {
    for reads in [true, false] {
        let mut rt = Runtime::new(State { value: 9, reads, views: Cell::new(0) }, view, (400.0, 120.0), Env::new(true, 2.0));
        rt.frame(0.0);
        for value in [8, 9] {
            let before = rt.state.views.get();
            rt.update(|s, _| s.value = value);
            rt.frame(1.0 / 120.0);
            assert!(rt.state.views.get() >= before + 2, "the layout read retried the view");
            let digit = rt.elements().into_iter().find(|e| !e.exiting && e.key.as_deref().is_some_and(|k| k.starts_with("retry-count."))).unwrap();
            assert!(digit.opacity < 0.5, "the new digit fades in, even after a retry: {}", digit.opacity);
            let dy = digit.rect[1] - digit.target[1];
            assert!(if value == 8 { dy < -1.0 } else { dy > 1.0 }, "the digit rolls in the value's direction: {dy}");
            for _ in 0..120 { rt.frame(1.0 / 120.0); }
        }
    }
}

#[test]
fn rolling_text_retry_motion_gate() {
    use rune::motion_gate::{self, Run};

    motion_gate::gate_rows(vec![("rolling-text/layout-read-retry".into(), Box::new(|| {
        let mut r = Run::new(Runtime::new(State { value: 9, reads: true, views: Cell::new(0) }, view, (400.0, 120.0), Env::new(true, 2.0)), 120.0);
        r.idle(0.5);
        r.step();
        r.act("down", |rt| rt.update(|s, _| s.value = 8));
        r.play(0.8);
        r.act("up", |rt| rt.update(|s, _| s.value = 9));
        r.play(1.0);
        r.rec
    }))], &[]);
}
