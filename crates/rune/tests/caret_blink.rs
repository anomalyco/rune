//! The focused field's caret blinks while its window is key, wakes the host only at each on
//! and off, and shows again the moment the user types.
use rune::{Color, El, Env, Ime, Runtime, TextEdit, TextStyle, col, text_input};

struct S { draft: TextEdit }

fn ink() -> Color { rune::color::hex("#ff0077").unwrap() }

fn view(s: &S, _: &mut rune::Cx<S>) -> El {
    col().w_full().h_full().padding(20.0).child(
        text_input(&s.draft, TextStyle::new(14.0, 400.0).lh(20.0)).key("input").w_full()
            .caret(ink(), ink().alpha(0.3)).on_edit(|s: &mut S| &mut s.draft),
    )
}

fn runtime() -> Runtime<S> {
    let mut draft = TextEdit::new("hello");
    draft.focused = true;
    let mut rt = Runtime::new(S { draft }, view, (300.0, 200.0), Env::new(true, 2.0));
    rt.window_focus(true);
    rt.frame(0.0);
    rt
}

fn caret_drawn(rt: &Runtime<S>) -> bool {
    let want = ink().premul(1.0);
    rt.instances.iter().any(|i| i.fill.iter().zip(want).all(|(a, b)| (a - b).abs() < 1e-3))
}

/// Steps idle frames to `t` seconds, as a host does when it sleeps until the wake.
fn sleep_until(rt: &mut Runtime<S>, t: &mut f64, to: f64) {
    rt.frame(to - *t);
    *t = to;
}

#[test]
fn blinks_on_and_off_each_half_second() {
    let mut rt = runtime();
    let mut t = 0.0;
    assert!(caret_drawn(&rt), "on when focused");
    let wake = rt.wake_deadline().expect("asks to wake for the first off");
    assert!((wake - 0.5).abs() < 1e-6, "{wake}");
    sleep_until(&mut rt, &mut t, 0.5);
    assert!(!caret_drawn(&rt), "off after half a second");
    assert!((rt.wake_deadline().unwrap() - 1.0).abs() < 1e-6, "then wakes to show it");
    sleep_until(&mut rt, &mut t, 1.0);
    assert!(caret_drawn(&rt), "back on");
}

#[test]
fn typing_shows_the_caret_and_restarts_the_cycle() {
    let mut rt = runtime();
    let mut t = 0.0;
    sleep_until(&mut rt, &mut t, 0.6);
    assert!(!caret_drawn(&rt));
    rt.ime(Ime::Commit("!".into()));
    sleep_until(&mut rt, &mut t, 0.7);
    assert!(caret_drawn(&rt), "typing shows it at once");
    sleep_until(&mut rt, &mut t, 1.1);
    assert!(caret_drawn(&rt), "and holds it for a full half second");
    sleep_until(&mut rt, &mut t, 1.2);
    assert!(!caret_drawn(&rt));
}

#[test]
fn holds_still_when_it_should() {
    let mut rt = runtime();
    let mut t = 0.0;
    rt.window_focus(false);
    sleep_until(&mut rt, &mut t, 0.6);
    assert!(caret_drawn(&rt), "an inactive window's caret doesn't blink");
    assert_eq!(rt.wake_deadline(), None, "and doesn't wake the host");

    rt.window_focus(true);
    rt.reduced_motion = true;
    sleep_until(&mut rt, &mut t, 1.2);
    assert!(caret_drawn(&rt), "reduced motion holds it on");

    rt.reduced_motion = false;
    sleep_until(&mut rt, &mut t, 1.3);
    sleep_until(&mut rt, &mut t, 100.0);
    assert!(caret_drawn(&rt), "on after a minute without a change");
    assert_eq!(rt.wake_deadline(), None, "and the window rests");
}
