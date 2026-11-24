//! HoverCard placement, gutter, width, surface hover handlers, role/label, and defaults.

use rune::{Cx, El, Env, Runtime, Tx, col, el, text};
use rune_kit::{Button, HoverCard, Kit, Placement, root};

fn settle<S: 'static>(rt: &mut Runtime<S>) {
    rt.frame(0.0);
    rt.settle_all();
    rt.frame(1.0 / 60.0);
}

#[test]
fn hover_card_defaults_stay_unchanged_and_closed_mounts_no_card() {
    struct State {
        open: bool,
        wide_body: bool,
    }
    fn view(s: &State, _: &mut Cx<State>) -> El {
        let k = Kit::neutral(true);
        let mut layer = Vec::new();
        let body = if s.wide_body {
            el().key("body-box").size(500.0, 24.0)
        } else {
            el().key("body-box").size(40.0, 24.0)
        };
        let trigger = HoverCard::new("preview")
            .trigger(el().key("anchor-box").size(60.0, 24.0))
            .child(body)
            .open(s.open)
            .build(&k, &mut layer);
        root(el().w_full().h_full().child(el().absolute(200.0, 100.0).w(60.0).child(trigger)), layer)
    }

    // Closed: no card mounted in the layer.
    let mut rt = Runtime::new(State { open: false, wide_body: false }, view, (600.0, 400.0), Env::new(true, 1.0));
    settle(&mut rt);
    assert!(rt.elements().iter().all(|e| e.key.as_deref() != Some("preview-card")));

    // Open with short body: min_w(200.0), 4 pt gutter below trigger, default group role, no label.
    rt.state.open = true;
    rt.update(|_, _| {});
    settle(&mut rt);
    let els = rt.elements();
    let trigger = els.iter().find(|e| e.key.as_deref() == Some("preview-trigger")).unwrap();
    let card = els.iter().find(|e| e.key.as_deref() == Some("preview-card")).unwrap();
    assert!((card.target[2] - 200.0).abs() < 0.5, "default min_width is 200, got {}", card.target[2]);
    assert!(
        (card.target[1] - (trigger.target[1] + trigger.target[3] + 4.0)).abs() < 0.5,
        "default gutter is 4 pt below trigger: card={:?} trigger={:?}",
        card.target,
        trigger.target,
    );
    assert_eq!(card.role, "group");
    assert_eq!(card.label, None);

    // Open with wide body: capped at max_w(320.0).
    rt.update(|s, _| s.wide_body = true);
    settle(&mut rt);
    let card = rt.elements().into_iter().find(|e| e.key.as_deref() == Some("preview-card")).unwrap();
    assert!((card.target[2] - 320.0).abs() < 0.5, "default max_width is 320, got {}", card.target[2]);
}

#[test]
fn hover_card_left_start_left_end_right_end_and_gutter_layout() {
    struct State {
        placement: Placement,
        gutter: f32,
        reduced: bool,
    }
    fn view(s: &State, cx: &mut Cx<State>) -> El {
        if s.reduced {
            cx.reduced_motion(true);
        }
        let k = Kit::neutral(true);
        let mut layer = Vec::new();
        let trigger = HoverCard::new("side")
            .trigger(el().key("anchor").size(80.0, 30.0))
            .child(el().size(100.0, 50.0))
            .placement(s.placement)
            .gutter(s.gutter)
            .width(120.0)
            .open(true)
            .build(&k, &mut layer);
        root(el().w_full().h_full().child(el().absolute(240.0, 140.0).w(80.0).child(trigger)), layer)
    }

    for reduced in [false, true] {
        for (placement, gutter) in [
            (Placement::LeftStart, 0.0),
            (Placement::LeftStart, 12.0),
            (Placement::LeftEnd, 8.0),
            (Placement::RightEnd, 14.0),
        ] {
            let mut rt = Runtime::new(State { placement, gutter, reduced }, view, (640.0, 400.0), Env::new(true, 1.0));
            if reduced {
                rt.frame(0.0);
                rt.frame(1.0 / 60.0);
            } else {
                settle(&mut rt);
            }
            let els = rt.elements();
            let t = els.iter().find(|e| e.key.as_deref() == Some("side-trigger")).unwrap().target;
            let c = els.iter().find(|e| e.key.as_deref() == Some("side-card")).unwrap().target;
            assert!((c[2] - 120.0).abs() < 0.5, "fixed width 120, got {}", c[2]);
            match placement {
                Placement::LeftStart => {
                    assert!((c[0] + c[2] + gutter - t[0]).abs() < 0.5, "LeftStart x: card={c:?} trigger={t:?} gutter={gutter}");
                    assert!((c[1] - t[1]).abs() < 0.5, "LeftStart y: card={c:?} trigger={t:?}");
                }
                Placement::LeftEnd => {
                    assert!((c[0] + c[2] + gutter - t[0]).abs() < 0.5, "LeftEnd x: card={c:?} trigger={t:?} gutter={gutter}");
                    assert!((c[1] + c[3] - (t[1] + t[3])).abs() < 0.5, "LeftEnd y: card={c:?} trigger={t:?}");
                }
                Placement::RightEnd => {
                    assert!((c[0] - (t[0] + t[2] + gutter)).abs() < 0.5, "RightEnd x: card={c:?} trigger={t:?} gutter={gutter}");
                    assert!((c[1] + c[3] - (t[1] + t[3])).abs() < 0.5, "RightEnd y: card={c:?} trigger={t:?}");
                }
                _ => unreachable!(),
            }
        }
    }
}

#[test]
fn hover_card_custom_min_max_and_fixed_widths() {
    struct State {
        min_w: Option<f32>,
        max_w: Option<f32>,
        fixed_w: Option<f32>,
        body_w: f32,
    }
    fn view(s: &State, _: &mut Cx<State>) -> El {
        let k = Kit::neutral(true);
        let mut layer = Vec::new();
        let mut card = HoverCard::new("sized")
            .trigger(el().size(60.0, 24.0))
            .child(el().size(s.body_w, 24.0))
            .open(true);
        if let Some(w) = s.min_w {
            card = card.min_width(w);
        }
        if let Some(w) = s.max_w {
            card = card.max_width(w);
        }
        if let Some(w) = s.fixed_w {
            card = card.width(w);
        }
        let trigger = card.build(&k, &mut layer);
        root(el().w_full().h_full().child(el().absolute(100.0, 60.0).w(60.0).child(trigger)), layer)
    }

    let width_of = |state: State| {
        let mut rt = Runtime::new(state, view, (1200.0, 400.0), Env::new(true, 1.0));
        settle(&mut rt);
        rt.elements().into_iter().find(|e| e.key.as_deref() == Some("sized-card")).unwrap().target[2]
    };

    // Custom min_width/max_width clamp narrow and wide content.
    assert!((width_of(State { min_w: Some(120.0), max_w: Some(180.0), fixed_w: None, body_w: 40.0 }) - 120.0).abs() < 0.5);
    assert!((width_of(State { min_w: Some(120.0), max_w: Some(180.0), fixed_w: None, body_w: 400.0 }) - 180.0).abs() < 0.5);

    // Fixed width overrides both min_width and max_width (both narrower and wider than defaults).
    assert!((width_of(State { min_w: Some(200.0), max_w: Some(320.0), fixed_w: Some(140.0), body_w: 400.0 }) - 140.0).abs() < 0.5);
    assert!((width_of(State { min_w: Some(200.0), max_w: Some(320.0), fixed_w: Some(440.0), body_w: 40.0 }) - 440.0).abs() < 0.5);

    // Edge cases: zero, negative (clamped to 0), and huge widths do not panic.
    assert!(width_of(State { min_w: None, max_w: None, fixed_w: Some(0.0), body_w: 40.0 }) <= 1.0);
    assert!(width_of(State { min_w: None, max_w: None, fixed_w: Some(-20.0), body_w: 40.0 }) <= 1.0);
    assert!((width_of(State { min_w: None, max_w: None, fixed_w: Some(900.0), body_w: 40.0 }) - 900.0).abs() < 0.5);
}

#[test]
fn hover_card_on_hover_fires_on_surface_keeps_descendants_active_and_leaves_on_exit() {
    #[derive(Default)]
    struct State {
        open: bool,
        events: Vec<bool>,
        action_clicks: usize,
    }
    fn view(s: &State, _: &mut Cx<State>) -> El {
        let k = Kit::neutral(true);
        let mut layer = Vec::new();
        let trigger = HoverCard::new("info")
            .trigger(el().key("trig").size(80.0, 28.0))
            .placement(Placement::RightStart)
            .gutter(6.0)
            .width(200.0)
            .role("dialog")
            .label("Details")
            .child(
                col().gap(6.0)
                    .child(text("Summary"))
                    .child(Button::new("Inspect").on_press(|s: &mut State| s.action_clicks += 1).build(&k).key("inspect")),
            )
            .open(s.open)
            .on_hover(|s: &mut State, over| s.events.push(over))
            .build(&k, &mut layer);
        root(el().w_full().h_full().child(el().absolute(40.0, 80.0).w(80.0).child(trigger)), layer)
    }

    let mut rt = Runtime::new(State { open: true, ..State::default() }, view, (600.0, 400.0), Env::new(true, 1.0));
    settle(&mut rt);

    // Verify role & label on ElementInfo.
    let card = rt.elements().into_iter().find(|e| e.key.as_deref() == Some("info-card")).unwrap();
    assert_eq!(card.role, "dialog");
    assert_eq!(card.label.as_deref(), Some("Details"));

    // Pointer enters the card surface.
    let card_x = card.target[0] + 6.0;
    let card_y = card.target[1] + 6.0;
    rt.pointer_move(card_x, card_y);
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.state.events, vec![true], "entering surface reports true");

    // Moving onto the button inside the card does not report a leave, and clicking it works.
    let (bx, by) = rt.find("inspect").unwrap();
    rt.pointer_move(bx, by);
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.state.events, vec![true], "moving onto descendant button stays inside card");
    rt.pointer_down(bx, by);
    rt.pointer_up(bx, by);
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.state.action_clicks, 1, "child button inside card remains clickable");

    // Moving off the card reports false.
    rt.pointer_move(580.0, 380.0);
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.state.events, vec![true, false], "leaving surface reports false");

    // Re-enter then close (unmount) while hovered: owed leave fires.
    rt.pointer_move(card_x, card_y);
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.state.events, vec![true, false, true]);
    rt.update(|s, _| s.open = false);
    settle(&mut rt);
    assert_eq!(rt.state.events, vec![true, false, true, false], "unmounting hovered card delivers owed leave");
}

#[test]
fn hover_card_on_hover_tx_keeps_card_open_and_receives_tx() {
    #[derive(Default)]
    struct State {
        trigger_over: bool,
        card_over: bool,
        events: Vec<bool>,
    }
    fn view(s: &State, _: &mut Cx<State>) -> El {
        let k = Kit::neutral(true);
        let mut layer = Vec::new();
        let trigger_el = el()
            .key("trig")
            .size(80.0, 28.0)
            .on_hover_change(|s: &mut State, over, _| s.trigger_over = over);
        let open = s.trigger_over || s.card_over;
        let card = HoverCard::new("stay")
            .trigger(trigger_el)
            .placement(Placement::RightStart)
            .gutter(2.0)
            .width(180.0)
            .child(Button::new("Inside").build(&k).key("inside-btn"))
            .open(open)
            .on_hover_tx(|s: &mut State, over, tx: &mut Tx| {
                s.card_over = over;
                s.events.push(over);
                if over {
                    tx.focus("inside-btn", true);
                }
            })
            .build(&k, &mut layer);
        root(el().w_full().h_full().child(el().absolute(40.0, 80.0).w(80.0).child(card)), layer)
    }

    let mut rt = Runtime::new(State::default(), view, (600.0, 400.0), Env::new(true, 1.0));
    settle(&mut rt);

    // Hover trigger -> card opens.
    let (tx, ty) = rt.find("trig").unwrap();
    rt.pointer_move(tx, ty);
    settle(&mut rt);
    assert!(rt.state.trigger_over);
    let card = rt.elements().into_iter().find(|e| e.key.as_deref() == Some("stay-card")).expect("card mounted");

    // Move directly from trigger onto the card surface -> trigger leaves, card stays open, tx.focus runs.
    rt.pointer_move(card.target[0] + 10.0, card.target[1] + 10.0);
    settle(&mut rt);
    assert!(!rt.state.trigger_over);
    assert!(rt.state.card_over);
    assert_eq!(rt.state.events, vec![true]);
    assert!(rt.elements().iter().any(|e| e.key.as_deref() == Some("stay-card")), "card stays open while hovered");
    let focused = rt.elements().into_iter().find(|e| e.focused).and_then(|e| e.key);
    assert_eq!(focused.as_deref(), Some("inside-btn"), "on_hover_tx's Tx::focus ran in the same commit");

    // Move away -> card closes.
    rt.pointer_move(550.0, 350.0);
    settle(&mut rt);
    assert!(!rt.state.card_over);
    assert_eq!(rt.state.events, vec![true, false]);
    assert!(rt.elements().iter().all(|e| e.key.as_deref() != Some("stay-card")));
}
