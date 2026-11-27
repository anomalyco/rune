use std::cell::Cell;
use std::rc::Rc;

use crate::{Align, Cx, El, Env, GesturePhase, Key, KeyEvent, Mods, Runtime, ScrollSpec, Side, WheelEvent, col, el, row};

fn scroll_offset<S: 'static>(rt: &Runtime<S>, key: &str) -> (f64, f64) {
    let id = rt.tree.nodes.iter().find(|n| n.key.as_deref() == Some(key))
        .unwrap_or_else(|| panic!("missing scroll viewport {key}")).id;
    let st = &rt.scroll.scrolls[&id];
    (st.offset, st.x.offset)
}

fn find_a11y<'a>(node: &'a serde_json::Value, key: &str) -> Option<&'a serde_json::Value> {
    if node["key"] == key { return Some(node); }
    node["children"].as_array()?.iter().find_map(|c| find_a11y(c, key))
}

#[test]
fn outside_wheel_dismisses_modal_and_scrolls_viewport_beneath() {
    #[derive(Default)]
    struct S { open: bool, dismisses: usize, use_tx: bool }

    fn view(s: &S, _: &mut Cx<S>) -> El {
        let mut root = col().key("root").w_full().h_full()
            .child(
                col().key("list").absolute(0.0, 0.0).size(200.0, 200.0)
                    .scroll_xy()
                    .child(el().size(1000.0, 1000.0)),
            );
        if s.open {
            let pop = col().key("popover").absolute(220.0, 20.0).size(140.0, 120.0).role("dialog").label("Popover");
            let pop = if s.use_tx {
                pop.modal_tx(true, |s: &mut S, tx| { s.open = false; s.dismisses += 1; tx.instant(); })
            } else {
                pop.modal(true, |s: &mut S| { s.open = false; s.dismisses += 1; })
            };
            root = root.child(pop.modal_wheel_outside(true));
        }
        root
    }

    // 1. Vertical wheel_event dismisses and scrolls list beneath.
    let mut rt = Runtime::new(S { open: true, dismisses: 0, use_tx: false }, view, (400.0, 240.0), Env::new(true, 1.0));
    rt.frame(0.0);
    rt.wheel_event(WheelEvent::new(0.0, -45.0).at(50.0, 80.0));
    rt.frame(0.016);
    assert!(!rt.state.open);
    assert_eq!(rt.state.dismisses, 1);
    assert_eq!(scroll_offset(&rt, "list"), (45.0, 0.0));

    // 2. Horizontal wheel_xy with modal_tx dismisses and scrolls horizontally.
    rt.update(|s, _| { s.open = true; s.use_tx = true; });
    rt.frame(0.016);
    rt.wheel_xy(50.0, 80.0, -60.0, 0.0);
    rt.frame(0.016);
    assert!(!rt.state.open);
    assert_eq!(rt.state.dismisses, 2);
    assert_eq!(scroll_offset(&rt, "list"), (45.0, 60.0));

    // 3. Direct rt.wheel(x, y, dy) also dismisses and scrolls.
    rt.update(|s, _| s.open = true);
    rt.frame(0.016);
    rt.wheel(50.0, 80.0, -30.0);
    rt.frame(0.016);
    assert!(!rt.state.open);
    assert_eq!(rt.state.dismisses, 3);
    assert_eq!(scroll_offset(&rt, "list"), (75.0, 60.0));

    // 4. Two outside wheels in the same frame dismiss once and both deltas scroll.
    rt.update(|s, _| s.open = true);
    rt.frame(0.016);
    rt.wheel_event(WheelEvent::new(0.0, -20.0).at(50.0, 80.0));
    rt.wheel_event(WheelEvent::new(0.0, -15.0).at(50.0, 80.0));
    rt.frame(0.016);
    assert!(!rt.state.open);
    assert_eq!(rt.state.dismisses, 4, "same-frame outside wheels must not fire dismiss twice");
    assert_eq!(scroll_offset(&rt, "list"), (110.0, 60.0), "both same-frame wheel deltas scroll beneath");

    // 5. Notched mouse wheel eases to its target after dismissing.
    rt.update(|s, _| s.open = true);
    rt.frame(0.2);
    rt.wheel_event(WheelEvent::new(0.0, -100.0).at(50.0, 80.0).notch(true));
    for _ in 0..40 { rt.frame(0.016); }
    assert!(!rt.state.open);
    assert_eq!(rt.state.dismisses, 5);
    assert!((scroll_offset(&rt, "list").0 - 210.0).abs() < 1.0, "notched wheel reaches target after dismiss: {:?}", scroll_offset(&rt, "list"));
}

#[test]
fn outside_wheel_dismisses_modal_and_routes_to_on_wheel_beneath() {
    struct S {
        open: bool,
        dismisses: usize,
        received: Vec<WheelEvent>,
        live_only: bool,
        views: Rc<Cell<usize>>,
    }

    fn view(s: &S, _: &mut Cx<S>) -> El {
        s.views.set(s.views.get() + 1);
        let mut root = col().key("root").w_full().h_full()
            .child(
                el().key("canvas").absolute(20.0, 30.0).size(160.0, 140.0)
                    .on_wheel(|s: &mut S, ev, tx| {
                        s.received.push(ev);
                        if s.live_only { tx.live_only(); }
                        true
                    }),
            );
        if s.open {
            root = root.child(
                col().key("popover").absolute(220.0, 20.0).size(140.0, 120.0)
                    .modal(true, |s: &mut S| { s.open = false; s.dismisses += 1; })
                    .modal_wheel_outside(true),
            );
        }
        root
    }

    let views = Rc::new(Cell::new(0));
    let mut rt = Runtime::new(
        S { open: true, dismisses: 0, received: Vec::new(), live_only: false, views: Rc::clone(&views) },
        view,
        (400.0, 220.0),
        Env::new(true, 1.0),
    );
    rt.frame(0.0);
    let mods = Mods { alt: true, ..Mods::default() };
    rt.wheel_event(WheelEvent::new(12.0, -24.0).at(60.0, 80.0).mods(mods));
    rt.frame(0.016);
    assert!(!rt.state.open);
    assert_eq!(rt.state.dismisses, 1);
    assert_eq!(rt.state.received.len(), 1);
    let ev = rt.state.received[0];
    assert_eq!((ev.x, ev.y), (40.0, 50.0), "reposted on_wheel receives target-local coordinates");
    assert_eq!((ev.dx, ev.dy), (12.0, -24.0));
    assert_eq!(ev.mods, mods);

    // Reposted on_wheel calling tx.live_only() skips a second view rebuild after the dismiss commit.
    rt.update(|s, _| { s.open = true; s.live_only = true; });
    rt.frame(0.016);
    let views_before = views.get();
    rt.wheel_event(WheelEvent::new(0.0, -10.0).at(60.0, 80.0));
    rt.frame(0.016);
    assert!(!rt.state.open);
    assert_eq!(rt.state.received.len(), 2);
    assert_eq!(views.get() - views_before, 1, "dismiss commits once and live_only on_wheel skips a second rebuild");
}

#[test]
fn inside_wheel_scrolls_modal_and_anchored_popover_without_dismissing() {
    struct S { open: bool, dismisses: usize }

    fn view(s: &S, _: &mut Cx<S>) -> El {
        let mut root = col().key("root").w_full().h_full()
            .child(
                col().key("bg-list").absolute(0.0, 0.0).size(400.0, 260.0)
                    .scroll(ScrollSpec { stick_bottom: false })
                    .child(el().w_full().h(1000.0)),
            );
        if s.open {
            root = root
                .child(
                    col().key("modal").absolute(200.0, 10.0).size(160.0, 120.0)
                        .modal(true, |s: &mut S| { s.open = false; s.dismisses += 1; })
                        .modal_wheel_outside(true)
                        .child(el().key("modal-anchor").size(160.0, 20.0))
                        .child(
                            col().key("modal-list").w_full().h(100.0)
                                .scroll(ScrollSpec { stick_bottom: false })
                                .child(el().w_full().h(600.0)),
                        ),
                )
                .child(
                    col().key("sub-pop").size(100.0, 60.0)
                        .anchored("modal-anchor", Side::Left, Align::Start, 6.0)
                        .scroll(ScrollSpec { stick_bottom: false })
                        .child(el().w_full().h(300.0)),
                );
        }
        root
    }

    let mut rt = Runtime::new(S { open: false, dismisses: 0 }, view, (400.0, 260.0), Env::new(true, 1.0));
    rt.frame(0.0);

    // Frame 0 of entrance fade (dt = 0.0): inside wheel does not dismiss.
    rt.update(|s, _| s.open = true);
    rt.frame(0.0);
    rt.wheel_event(WheelEvent::new(0.0, -25.0).at(260.0, 70.0));
    rt.frame(0.016);
    assert!(rt.state.open && rt.state.dismisses == 0, "inside wheel on frame 0 must not dismiss");
    assert_eq!(scroll_offset(&rt, "bg-list").0, 0.0);

    // Once visible, wheel inside the modal scrolls modal-list without dismissing.
    rt.frame(0.2);
    rt.wheel_event(WheelEvent::new(0.0, -25.0).at(260.0, 70.0));
    rt.frame(0.016);
    assert!(rt.state.open && rt.state.dismisses == 0, "inside wheel scrolls modal without dismissing");
    assert_eq!(scroll_offset(&rt, "modal-list").0, 25.0);
    assert_eq!(scroll_offset(&rt, "bg-list").0, 0.0);

    // Wheel inside a popover anchored inside the modal stays inside ModalScope and does not dismiss.
    let (sx, sy) = rt.find("sub-pop").unwrap();
    rt.wheel_event(WheelEvent::new(0.0, -30.0).at(sx, sy));
    rt.frame(0.016);
    assert!(rt.state.open && rt.state.dismisses == 0, "wheel inside modal-anchored popover must not dismiss");
    assert_eq!(scroll_offset(&rt, "sub-pop").0, 30.0);
    assert_eq!(scroll_offset(&rt, "bg-list").0, 0.0);

    // A phased trackpad gesture that began inside the modal and drifted outside mid-gesture must not dismiss.
    rt.frame(0.2);
    rt.wheel_event(WheelEvent::new(0.0, -10.0).at(260.0, 70.0).phase(GesturePhase::Began).precise(true));
    rt.wheel_event(WheelEvent::new(0.0, -10.0).at(30.0, 200.0).phase(GesturePhase::Moved).precise(true));
    rt.wheel_event(WheelEvent::new(0.0, 0.0).at(30.0, 200.0).phase(GesturePhase::Ended).precise(true));
    rt.frame(0.016);
    assert!(rt.state.open && rt.state.dismisses == 0, "phased gesture started inside modal stays with modal");
    assert_eq!(scroll_offset(&rt, "bg-list").0, 0.0);

    // An unphased inside mouse wheel followed immediately (16ms) by an outside mouse wheel dismisses and scrolls beneath.
    rt.frame(0.2);
    rt.wheel_event(WheelEvent::new(0.0, -15.0).at(260.0, 70.0));
    rt.frame(0.016);
    assert!(rt.state.open);
    rt.wheel_event(WheelEvent::new(0.0, -40.0).at(30.0, 200.0));
    rt.frame(0.016);
    assert!(!rt.state.open && rt.state.dismisses == 1, "unphased outside wheel immediately after inside wheel dismisses");
    assert_eq!(scroll_offset(&rt, "bg-list").0, 40.0);
}

#[test]
fn default_modal_wheel_outside_false_and_modal_trap_block_outside_wheels() {
    #[derive(Clone, Copy)]
    enum Mode { DefaultFalse, ToggledOff, Trap, WheelOnlyNoClick }
    struct S { mode: Mode, open: bool, dismisses: usize }

    fn view(s: &S, _: &mut Cx<S>) -> El {
        let mut root = col().key("root").w_full().h_full()
            .child(
                col().key("bg").absolute(0.0, 0.0).size(200.0, 200.0)
                    .scroll(ScrollSpec { stick_bottom: false })
                    .child(el().w_full().h(1000.0)),
            );
        if s.open {
            let m = col().key("modal").absolute(220.0, 20.0).size(140.0, 120.0);
            let m = match s.mode {
                Mode::DefaultFalse => m.modal(true, |s: &mut S| { s.open = false; s.dismisses += 1; }),
                Mode::ToggledOff => m.modal(true, |s: &mut S| { s.open = false; s.dismisses += 1; })
                    .modal_wheel_outside(true)
                    .modal_wheel_outside(false),
                Mode::Trap => m.modal_trap().modal_wheel_outside(true),
                Mode::WheelOnlyNoClick => m.modal(false, |s: &mut S| { s.open = false; s.dismisses += 1; })
                    .modal_wheel_outside(true),
            };
            root = root.child(m);
        }
        root
    }

    for mode in [Mode::DefaultFalse, Mode::ToggledOff, Mode::Trap] {
        let mut rt = Runtime::new(S { mode, open: true, dismisses: 0 }, view, (400.0, 220.0), Env::new(true, 1.0));
        rt.frame(0.0);
        rt.wheel_event(WheelEvent::new(0.0, -50.0).at(50.0, 50.0));
        rt.frame(0.016);
        assert!(rt.state.open && rt.state.dismisses == 0);
        assert_eq!(scroll_offset(&rt, "bg").0, 0.0);
    }

    // modal(false, ..).modal_wheel_outside(true): outside click stays blocked, outside wheel dismisses and scrolls.
    let mut rt = Runtime::new(S { mode: Mode::WheelOnlyNoClick, open: true, dismisses: 0 }, view, (400.0, 220.0), Env::new(true, 1.0));
    rt.frame(0.0);
    rt.pointer_down(50.0, 50.0);
    rt.pointer_up(50.0, 50.0);
    rt.frame(0.016);
    assert!(rt.state.open && rt.state.dismisses == 0, "click_outside=false still blocks outside clicks");
    rt.wheel_event(WheelEvent::new(0.0, -50.0).at(50.0, 50.0));
    rt.frame(0.016);
    assert!(!rt.state.open && rt.state.dismisses == 1, "wheel_outside=true still dismisses when click_outside=false");
    assert_eq!(scroll_offset(&rt, "bg").0, 50.0);
}

#[test]
fn zero_delta_and_momentum_wheels_do_not_dismiss_modal() {
    struct S { open: bool, dismisses: usize }

    fn view(s: &S, _: &mut Cx<S>) -> El {
        let mut root = col().key("root").w_full().h_full()
            .child(
                col().key("bg").absolute(0.0, 0.0).size(200.0, 200.0)
                    .scroll(ScrollSpec { stick_bottom: false })
                    .child(el().w_full().h(1000.0)),
            );
        if s.open {
            root = root.child(
                col().key("modal").absolute(220.0, 20.0).size(140.0, 120.0)
                    .modal(true, |s: &mut S| { s.open = false; s.dismisses += 1; })
                    .modal_wheel_outside(true)
                    .scroll(ScrollSpec { stick_bottom: false })
                    .child(el().w_full().h(600.0)),
            );
        }
        root
    }

    let mut rt = Runtime::new(S { open: true, dismisses: 0 }, view, (400.0, 220.0), Env::new(true, 1.0));
    rt.frame(0.0);

    // 1. Zero-delta wheel events across all phases never dismiss.
    for phase in [GesturePhase::Began, GesturePhase::Moved, GesturePhase::Ended] {
        rt.wheel_event(WheelEvent::new(0.0, 0.0).at(50.0, 50.0).phase(phase));
        rt.frame(0.016);
        assert!(rt.state.open && rt.state.dismisses == 0, "zero-delta wheel ({phase:?}) must not dismiss");
    }

    // 2. Explicit WheelEvent::momentum(true) never dismisses.
    rt.frame(0.2);
    rt.wheel_event(WheelEvent::new(0.0, -40.0).at(50.0, 50.0).momentum(true));
    rt.frame(0.016);
    assert!(rt.state.open && rt.state.dismisses == 0, "explicit momentum wheel must not dismiss");
    assert_eq!(scroll_offset(&rt, "bg").0, 0.0);

    // 3. Phased trackpad momentum following an inside gesture does not dismiss even if pointer moves outside.
    rt.frame(0.2);
    rt.wheel_event(WheelEvent::new(0.0, -20.0).at(260.0, 60.0).phase(GesturePhase::Began).precise(true));
    rt.wheel_event(WheelEvent::new(0.0, 0.0).at(260.0, 60.0).phase(GesturePhase::Ended).precise(true));
    rt.frame(0.016); // within MOMENTUM_GAP (50ms)
    rt.wheel_event(WheelEvent::new(0.0, -30.0).at(50.0, 50.0).phase(GesturePhase::Began).precise(true));
    rt.wheel_event(WheelEvent::new(0.0, -15.0).at(50.0, 50.0).phase(GesturePhase::Moved).precise(true));
    rt.wheel_event(WheelEvent::new(0.0, 0.0).at(50.0, 50.0).phase(GesturePhase::Ended).precise(true));
    rt.frame(0.016);
    assert!(rt.state.open && rt.state.dismisses == 0, "trackpad momentum must not dismiss");
    assert_eq!(scroll_offset(&rt, "bg").0, 0.0);

    // 4. A fresh finger touch after momentum ends (even immediately) DOES dismiss and scroll.
    rt.wheel_event(WheelEvent::new(0.0, -35.0).at(50.0, 50.0).phase(GesturePhase::Began).precise(true));
    rt.wheel_event(WheelEvent::new(0.0, 0.0).at(50.0, 50.0).phase(GesturePhase::Ended).precise(true));
    rt.frame(0.016);
    assert!(!rt.state.open && rt.state.dismisses == 1, "new touch after momentum dismisses");
    assert_eq!(scroll_offset(&rt, "bg").0, 35.0);
}

#[test]
fn wheel_repost_guards_refused_replaced_nested_and_inert() {
    #[derive(Clone, Copy)]
    enum Mode { RefuseClose, OpenNewModal, NestedInOuter, InertTarget }
    struct S {
        mode: Mode,
        outer_open: bool,
        inner_open: bool,
        new_modal: bool,
        dismisses: usize,
        bg_wheels: usize,
        new_modal_wheels: usize,
    }

    fn view(s: &S, _: &mut Cx<S>) -> El {
        let mut bg = col().key("bg").absolute(0.0, 0.0).size(120.0, 200.0)
            .scroll(ScrollSpec { stick_bottom: false })
            .on_wheel(|s: &mut S, _, _| { s.bg_wheels += 1; false })
            .child(el().w_full().h(1000.0));
        if matches!(s.mode, Mode::InertTarget) {
            bg = bg.inert();
        }
        let mut root = col().key("root").w_full().h_full().child(bg);
        if matches!(s.mode, Mode::NestedInOuter) && s.outer_open {
            root = root.child(
                col().key("outer").absolute(140.0, 0.0).size(140.0, 200.0).role("dialog")
                    .modal(false, |s: &mut S| s.outer_open = false)
                    .child(
                        col().key("outer-list").w_full().h_full()
                            .scroll(ScrollSpec { stick_bottom: false })
                            .child(el().w_full().h(1000.0)),
                    ),
            );
        }
        if s.inner_open {
            root = root.child(
                col().key("inner").absolute(300.0, 20.0).size(90.0, 100.0).role("dialog")
                    .modal(true, |s: &mut S| {
                        s.dismisses += 1;
                        match s.mode {
                            Mode::RefuseClose => {}
                            Mode::OpenNewModal => { s.inner_open = false; s.new_modal = true; }
                            _ => s.inner_open = false,
                        }
                    })
                    .modal_wheel_outside(true),
            );
        }
        if s.new_modal {
            root = root.child(
                col().key("confirm").absolute(0.0, 0.0).size(120.0, 200.0).role("dialog")
                    .modal(true, |s: &mut S| s.new_modal = false)
                    .on_wheel(|s: &mut S, _, _| { s.new_modal_wheels += 1; true }),
            );
        }
        root
    }

    let make = |mode| S {
        mode,
        outer_open: true,
        inner_open: true,
        new_modal: false,
        dismisses: 0,
        bg_wheels: 0,
        new_modal_wheels: 0,
    };

    // 1. Refused dismiss drops the repost.
    let mut rt = Runtime::new(make(Mode::RefuseClose), view, (420.0, 220.0), Env::new(true, 1.0));
    rt.frame(0.0);
    rt.wheel_event(WheelEvent::new(0.0, -40.0).at(40.0, 40.0));
    rt.frame(0.016);
    assert!(rt.state.inner_open && rt.state.dismisses == 1);
    assert_eq!(rt.state.bg_wheels, 0);
    assert_eq!(scroll_offset(&rt, "bg").0, 0.0);

    // 2. Replaced modal drops the repost (neither background nor replacement modal receives it).
    let mut rt = Runtime::new(make(Mode::OpenNewModal), view, (420.0, 220.0), Env::new(true, 1.0));
    rt.frame(0.0);
    rt.wheel_event(WheelEvent::new(0.0, -40.0).at(40.0, 40.0));
    rt.frame(0.016);
    assert!(!rt.state.inner_open && rt.state.new_modal && rt.state.dismisses == 1);
    assert_eq!((rt.state.bg_wheels, rt.state.new_modal_wheels), (0, 0));
    assert_eq!(scroll_offset(&rt, "bg").0, 0.0);

    // 3. Nested modal: wheel outside outer modal closes inner modal without piercing inert background.
    let mut rt = Runtime::new(make(Mode::NestedInOuter), view, (420.0, 220.0), Env::new(true, 1.0));
    rt.frame(0.0);
    rt.wheel_event(WheelEvent::new(0.0, -40.0).at(40.0, 40.0));
    rt.frame(0.016);
    assert!(!rt.state.inner_open && rt.state.outer_open);
    assert_eq!(rt.state.bg_wheels, 0);
    assert_eq!(scroll_offset(&rt, "bg").0, 0.0);

    // 4. Nested modal: wheel inside outer modal (outside inner) closes inner and scrolls outer's list.
    rt.update(|s, _| s.inner_open = true);
    rt.frame(0.2);
    rt.wheel_event(WheelEvent::new(0.0, -55.0).at(180.0, 60.0));
    rt.frame(0.016);
    assert!(!rt.state.inner_open && rt.state.outer_open);
    assert_eq!(scroll_offset(&rt, "outer-list").0, 55.0);

    // 5. Inert background target does not scroll or fire on_wheel after modal closes.
    let mut rt = Runtime::new(make(Mode::InertTarget), view, (420.0, 220.0), Env::new(true, 1.0));
    rt.frame(0.0);
    rt.wheel_event(WheelEvent::new(0.0, -40.0).at(40.0, 40.0));
    rt.frame(0.016);
    assert!(!rt.state.inner_open && rt.state.dismisses == 1);
    assert_eq!(rt.state.bg_wheels, 0);
    assert_eq!(scroll_offset(&rt, "bg").0, 0.0);
}

#[test]
fn outside_wheel_dismissal_restores_focus_and_accessibility_tree() {
    #[derive(Default)]
    struct S { open: bool }

    fn view(s: &S, _: &mut Cx<S>) -> El {
        let mut root = row().key("root").w_full().h_full().gap(12.0)
            .child(
                col().key("list").size(180.0, 200.0).role("region").label("Files")
                    .scroll(ScrollSpec { stick_bottom: false })
                    .child(
                        el().key("opener").size(120.0, 32.0).role("button").label("Open details")
                            .focusable(true)
                            .on_press(|s: &mut S| s.open = true),
                    )
                    .child(el().w_full().h(800.0)),
            );
        if s.open {
            root = root.child(
                col().key("popover").size(160.0, 120.0).role("dialog").label("Details")
                    .modal(true, |s: &mut S| s.open = false)
                    .modal_wheel_outside(true)
                    .child(
                        el().key("close-btn").size(100.0, 32.0).role("button").label("Close")
                            .focusable(true)
                            .on_press(|s: &mut S| s.open = false),
                    ),
            );
        }
        root
    }

    let mut rt = Runtime::new(S::default(), view, (400.0, 220.0), Env::new(true, 1.0));
    rt.set_full_keyboard_access(true);
    rt.frame(0.0);

    // Focus opener and open the modal via keyboard.
    rt.key(KeyEvent { key: Key::Tab, mods: Mods::default() });
    rt.frame(0.016);
    rt.key(KeyEvent { key: Key::Text(" ".into()), mods: Mods::default() });
    rt.frame(0.016);
    assert!(rt.state.open);
    assert_eq!(rt.elements().into_iter().find(|e| e.focused).and_then(|e| e.key), Some("close-btn".into()));
    assert!(find_a11y(&rt.accessibility_json(), "opener").is_none(), "background hidden from a11y tree while modal open");
    assert!(!rt.a11y_json().contains(r#""Open details""#), "background hidden from web a11y mirror while modal open");
    assert!(rt.a11y_json().contains(r#""Close""#), "modal controls visible in web a11y mirror while modal open");

    // Outside wheel over the list dismisses modal, restores focus and a11y tree, and scrolls the list.
    rt.wheel_event(WheelEvent::new(0.0, -16.0).at(80.0, 120.0));
    rt.frame(0.016);
    assert!(!rt.state.open);
    assert_eq!(scroll_offset(&rt, "list").0, 16.0);
    assert_eq!(rt.elements().into_iter().find(|e| e.focused).and_then(|e| e.key), Some("opener".into()), "focus returns to opener after outside wheel dismiss");
    assert!(find_a11y(&rt.accessibility_json(), "opener").is_some(), "background restored to a11y tree");
    assert!(rt.a11y_json().contains(r#""Open details""#), "background restored to web a11y mirror");
}
