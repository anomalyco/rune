//! `scrim_tx`, `Popover::on_close_tx`, `Select::on_toggle_tx`, and `Select::on_close_tx`.

use rune::{Cx, El, Env, Key, KeyEvent, Mods, Runtime, Tx, col, text};
use rune_kit::{Button, Kit, Popover, Select, SelectAppearance, root, scrim_tx};

fn settle<S: 'static>(rt: &mut Runtime<S>) {
    rt.frame(0.0);
    rt.settle_all();
    rt.frame(1.0 / 60.0);
}

fn focused_key<S: 'static>(rt: &Runtime<S>) -> Option<String> {
    rt.elements().into_iter().find(|e| e.focused).and_then(|e| e.key)
}

#[test]
fn scrim_tx_fires_with_tx_on_primary_and_secondary_press_and_stays_hidden_from_a11y() {
    #[derive(Default)]
    struct State {
        open: bool,
        closes: usize,
    }
    fn view(s: &State, _: &mut Cx<State>) -> El {
        let k = Kit::neutral(true);
        let mut layer = Vec::new();
        if s.open {
            layer.push(scrim_tx("custom-scrim", |s: &mut State, tx: &mut Tx| {
                s.open = false;
                s.closes += 1;
                tx.focus("after", true);
            }));
        }
        let body = col().w_full().h_full().padding(20.0)
            .child(Button::new("After").build(&k).key("after"));
        root(body, layer)
    }

    // Primary press.
    let mut rt = Runtime::new(State { open: true, closes: 0 }, view, (400.0, 300.0), Env::new(true, 1.0));
    settle(&mut rt);
    let tree = rt.accessibility_json();
    assert!(!tree.to_string().contains("custom-scrim"), "scrim_tx stays role none and out of a11y tree");

    rt.pointer_down(300.0, 200.0);
    rt.pointer_up(300.0, 200.0);
    rt.frame(0.0);
    assert!(!rt.state.open);
    assert_eq!(rt.state.closes, 1);
    assert_eq!(focused_key(&rt).as_deref(), Some("after"), "scrim_tx focuses target in the same commit");

    // Secondary press.
    let mut rt = Runtime::new(State { open: true, closes: 0 }, view, (400.0, 300.0), Env::new(true, 1.0));
    settle(&mut rt);
    rt.secondary_down(300.0, 200.0);
    rt.secondary_up(300.0, 200.0);
    rt.frame(0.0);
    assert!(!rt.state.open);
    assert_eq!(rt.state.closes, 1);
    assert_eq!(focused_key(&rt).as_deref(), Some("after"), "scrim_tx secondary press focuses target in same commit");
}

#[test]
fn popover_on_close_tx_runs_on_close_button_and_scrim_presses_without_clone() {
    #[derive(Default)]
    struct State {
        open: bool,
        closes: usize,
    }
    struct NotClone(&'static str);

    fn view(s: &State, _: &mut Cx<State>) -> El {
        let k = Kit::neutral(true);
        let mut layer = Vec::new();
        // Capture a non-Clone token to prove Popover::on_close_tx does not require Clone.
        let target = NotClone("after");
        let trigger = Button::new("Open").on_press(|s: &mut State| s.open = true).build(&k).key("opener");
        let pop = Popover::new("share")
            .trigger(trigger)
            .title("Share")
            .child(text("Body"))
            .open(s.open)
            .on_close_tx(move |s: &mut State, tx: &mut Tx| {
                s.open = false;
                s.closes += 1;
                tx.focus(target.0, true);
            })
            .build(&k, &mut layer);
        let body = col().w_full().h_full().padding(40.0).gap(16.0)
            .child(pop)
            .child(Button::new("After").build(&k).key("after"));
        root(body, layer)
    }

    // 1. Header close button press.
    let mut rt = Runtime::new(State { open: true, closes: 0 }, view, (500.0, 400.0), Env::new(true, 1.0));
    settle(&mut rt);
    let (cx, cy) = rt.find("close").expect("close button mounted");
    rt.pointer_down(cx, cy);
    rt.pointer_up(cx, cy);
    rt.frame(0.0);
    assert!(!rt.state.open);
    assert_eq!(rt.state.closes, 1);
    assert_eq!(focused_key(&rt).as_deref(), Some("after"), "close button on_close_tx focuses target in same commit");

    // 2. Primary scrim press outside popover.
    let mut rt = Runtime::new(State { open: true, closes: 0 }, view, (500.0, 400.0), Env::new(true, 1.0));
    settle(&mut rt);
    rt.pointer_down(480.0, 380.0);
    rt.pointer_up(480.0, 380.0);
    rt.frame(0.0);
    assert!(!rt.state.open);
    assert_eq!(rt.state.closes, 1);
    assert_eq!(focused_key(&rt).as_deref(), Some("after"), "primary scrim press focuses target in same commit");

    // 3. Secondary scrim press outside popover.
    let mut rt = Runtime::new(State { open: true, closes: 0 }, view, (500.0, 400.0), Env::new(true, 1.0));
    settle(&mut rt);
    rt.secondary_down(480.0, 380.0);
    rt.secondary_up(480.0, 380.0);
    rt.frame(0.0);
    assert!(!rt.state.open);
    assert_eq!(rt.state.closes, 1);
    assert_eq!(focused_key(&rt).as_deref(), Some("after"), "secondary scrim press focuses target in same commit");
}

#[test]
fn select_on_toggle_tx_and_on_close_tx_run_in_same_commit_and_respect_disabled() {
    #[derive(Default)]
    struct State {
        open: bool,
        disabled: bool,
        toggles: usize,
        closes: usize,
        chosen: Option<String>,
    }

    fn view(s: &State, _: &mut Cx<State>) -> El {
        let k = Kit::neutral(true);
        let mut layer = Vec::new();
        let select = Select::new("fruit", &["Apple", "Banana", "Cherry"])
            .label("Fruit")
            .appearance(SelectAppearance::Field)
            .width(140.0)
            .current(s.chosen.as_deref())
            .open(s.open)
            .disabled(s.disabled)
            .on_toggle_tx(|s: &mut State, tx: &mut Tx| {
                s.open = !s.open;
                s.toggles += 1;
                if !s.open {
                    tx.focus("after", true);
                } else {
                    tx.instant();
                }
            })
            .on_close_tx(|s: &mut State, tx: &mut Tx| {
                s.open = false;
                s.closes += 1;
                tx.focus("after", true);
            })
            .on_choose(|s: &mut State, v, _| {
                s.chosen = Some(v.to_owned());
                s.open = false;
            })
            .build(&k, &mut layer);
        let body = col().w_full().h_full().padding(40.0).gap(16.0)
            .child(select)
            .child(Button::new("After").build(&k).key("after"));
        root(body, layer)
    }

    // 1. Trigger press runs on_toggle_tx.
    let mut rt = Runtime::new(State::default(), view, (500.0, 400.0), Env::new(true, 1.0));
    settle(&mut rt);
    let (tx, ty) = rt.find("fruit-trigger").unwrap();
    rt.pointer_down(tx, ty);
    rt.pointer_up(tx, ty);
    rt.frame(0.0);
    assert!(rt.state.open);
    assert_eq!(rt.state.toggles, 1);

    // 2. Escape runs on_close_tx and tx.focus("after") overrides default focus return to trigger.
    rt.key(KeyEvent { key: Key::Escape, mods: Mods::default() });
    rt.frame(0.0);
    assert!(!rt.state.open);
    assert_eq!(rt.state.closes, 1);
    assert_eq!(focused_key(&rt).as_deref(), Some("after"), "Escape on_close_tx focuses target in same commit");

    // 3. Re-open via keyboard (Tab back to trigger? Move focus to trigger and press Space),
    // then dismiss with primary press outside (scrim).
    rt.update(|_, tx| tx.focus("fruit-trigger", true));
    rt.frame(0.0);
    rt.key(KeyEvent { key: Key::Text(" ".into()), mods: Mods::default() });
    rt.frame(0.0);
    assert!(rt.state.open);
    assert_eq!(rt.state.toggles, 2);
    settle(&mut rt);

    rt.pointer_down(480.0, 380.0);
    rt.pointer_up(480.0, 380.0);
    rt.frame(0.0);
    assert!(!rt.state.open);
    assert_eq!(rt.state.closes, 2);
    assert_eq!(focused_key(&rt).as_deref(), Some("after"), "primary outside press focuses target in same commit");

    // 4. Re-open, then dismiss with secondary press outside.
    rt.update(|s, _| s.open = true);
    settle(&mut rt);
    rt.secondary_down(480.0, 380.0);
    rt.secondary_up(480.0, 380.0);
    rt.frame(0.0);
    assert!(!rt.state.open);
    assert_eq!(rt.state.closes, 3);
    assert_eq!(focused_key(&rt).as_deref(), Some("after"), "secondary outside press focuses target in same commit");

    // 5. Disabled select drops both on_toggle_tx and on_close_tx.
    let mut rt = Runtime::new(
        State { open: true, disabled: true, ..State::default() },
        view,
        (500.0, 400.0),
        Env::new(true, 1.0),
    );
    settle(&mut rt);
    assert!(rt.elements().iter().all(|e| e.key.as_deref() != Some("fruit-content")), "disabled select mounts no listbox");
    let (dx, dy) = rt.find("fruit-trigger").unwrap();
    rt.pointer_down(dx, dy);
    rt.pointer_up(dx, dy);
    rt.frame(0.0);
    assert_eq!((rt.state.toggles, rt.state.closes), (0, 0), "disabled select fires neither tx callback");
}

#[test]
fn select_trigger_alone_honors_on_toggle_tx() {
    #[derive(Default)]
    struct State {
        toggles: usize,
    }
    fn view(_: &State, _: &mut Cx<State>) -> El {
        let k = Kit::neutral(true);
        let trig = Select::new("picker", &["A", "B"])
            .placeholder("Pick")
            .on_toggle_tx(|s: &mut State, tx: &mut Tx| {
                s.toggles += 1;
                tx.focus("target", true);
            })
            .trigger(&k);
        root(col().padding(20.0).gap(12.0).child(trig).child(Button::new("Target").build(&k).key("target")), [])
    }

    let mut rt = Runtime::new(State::default(), view, (400.0, 200.0), Env::new(true, 1.0));
    settle(&mut rt);
    let (x, y) = rt.find("picker-trigger").unwrap();
    rt.pointer_down(x, y);
    rt.pointer_up(x, y);
    rt.frame(0.0);
    assert_eq!(rt.state.toggles, 1);
    assert_eq!(focused_key(&rt).as_deref(), Some("target"));
}
