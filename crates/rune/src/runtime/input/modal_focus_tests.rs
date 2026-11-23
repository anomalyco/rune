use crate::{Cx, El, Env, Key, KeyEvent, Mods, Runtime, col, el};

fn button(key: &str) -> El {
    el().key(key).size(120.0, 30.0).role("button").label(key).focusable(true)
}

fn press<S: 'static>(rt: &mut Runtime<S>, key: Key) {
    rt.key(KeyEvent { key, mods: Mods::default() });
    for _ in 0..30 { rt.frame(0.016); }
}

fn focused_element<S: 'static>(rt: &Runtime<S>) -> Option<(String, bool)> {
    rt.elements().into_iter().find(|e| e.focused)
        .map(|e| (e.key.unwrap_or_default(), e.focusable))
}

#[test]
fn modal_whose_action_disables_opener_does_not_return_focus_to_disabled_opener() {
    #[derive(Default)]
    struct S { open: bool, reset_done: bool }
    fn view(s: &S, _: &mut Cx<S>) -> El {
        let mut root = col().w_full().h_full()
            .child(button("before").on_press(|_: &mut S| {}))
            .child(button("reset-all").disabled(s.reset_done)
                .on_press(|s: &mut S| if !s.reset_done { s.open = true; }));
        if s.open {
            root = root.child(col().key("confirm").size(200.0, 100.0).role("dialog").label("Reset everything?")
                .modal(true, |s: &mut S| s.open = false)
                .child(button("reset").on_press(|s: &mut S| { s.reset_done = true; s.open = false; })));
        }
        root
    }

    let mut rt = Runtime::new(S::default(), view, (400.0, 300.0), Env::new(true, 1.0));
    rt.frame(0.016);
    press(&mut rt, Key::Tab);
    press(&mut rt, Key::Tab);
    assert_eq!(focused_element(&rt), Some(("reset-all".into(), true)));
    press(&mut rt, Key::Text(" ".into()));
    assert_eq!(focused_element(&rt), Some(("reset".into(), true)));

    press(&mut rt, Key::Text(" ".into()));
    assert!(!rt.state.open && rt.state.reset_done);
    assert_eq!(focused_element(&rt), None, "disabled opener must not receive restored focus");
    assert!(!rt.has_focus());

    // Keyboard navigation still reaches remaining enabled controls cleanly.
    press(&mut rt, Key::Tab);
    assert_eq!(focused_element(&rt), Some(("before".into(), true)));
}

#[test]
fn modal_whose_action_makes_opener_or_ancestor_inert_does_not_return_focus_to_it() {
    #[derive(Default)]
    struct S { open: bool, inert_self: bool, inert_parent: bool }
    fn view(s: &S, _: &mut Cx<S>) -> El {
        let mut root = col().w_full().h_full()
            .child(button("before").on_press(|_: &mut S| {}))
            .child({
                let mut opener = button("opener").on_press(|s: &mut S| s.open = true);
                if s.inert_self { opener = opener.inert(); }
                let mut section = col().key("section").child(opener);
                if s.inert_parent { section = section.inert(); }
                section
            });
        if s.open {
            root = root.child(col().key("confirm").size(200.0, 100.0).role("dialog")
                .modal(true, |s: &mut S| s.open = false)
                .child(button("close-self").on_press(|s: &mut S| { s.inert_self = true; s.open = false; }))
                .child(button("close-parent").on_press(|s: &mut S| { s.inert_parent = true; s.open = false; })));
        }
        root
    }

    for ancestor in [false, true] {
        let mut rt = Runtime::new(S::default(), view, (400.0, 300.0), Env::new(true, 1.0));
        rt.frame(0.016);
        press(&mut rt, Key::Tab);
        press(&mut rt, Key::Tab);
        assert_eq!(focused_element(&rt), Some(("opener".into(), true)));
        press(&mut rt, Key::Text(" ".into()));
        assert_eq!(focused_element(&rt), Some(("close-self".into(), true)));
        if ancestor {
            press(&mut rt, Key::Tab);
            assert_eq!(focused_element(&rt), Some(("close-parent".into(), true)));
        }
        press(&mut rt, Key::Text(" ".into()));
        assert!(!rt.state.open);
        assert_eq!(focused_element(&rt), None, "inert opener (ancestor: {ancestor}) must not receive restored focus");
        assert!(!rt.has_focus());
    }
}

#[test]
fn nested_modal_whose_action_disables_opener_falls_back_inside_outer_modal() {
    struct S { outer: bool, inner: bool, disabled: bool }
    fn view(s: &S, _: &mut Cx<S>) -> El {
        let mut root = col().w_full().h_full().child(button("page"));
        if s.outer {
            root = root.child(col().key("outer").size(300.0, 200.0).role("dialog")
                .modal(true, |s: &mut S| s.outer = false)
                .child(button("open-inner").disabled(s.disabled).on_press(|s: &mut S| s.inner = true))
                .child(button("other").on_press(|_: &mut S| {})));
        }
        if s.inner {
            root = root.child(col().key("inner").size(200.0, 100.0).role("dialog")
                .modal(true, |s: &mut S| s.inner = false)
                .child(button("confirm").on_press(|s: &mut S| { s.disabled = true; s.inner = false; })));
        }
        root
    }

    let mut rt = Runtime::new(S { outer: true, inner: false, disabled: false }, view, (400.0, 300.0), Env::new(true, 1.0));
    rt.frame(0.016);
    assert_eq!(focused_element(&rt), Some(("open-inner".into(), true)));
    press(&mut rt, Key::Text(" ".into()));
    assert_eq!(focused_element(&rt), Some(("confirm".into(), true)));

    press(&mut rt, Key::Text(" ".into()));
    assert!(rt.state.outer && !rt.state.inner && rt.state.disabled);
    assert_eq!(
        focused_element(&rt),
        Some(("other".into(), true)),
        "closing a nested modal whose opener is now disabled must keep focus on a focusable control inside the outer modal"
    );
}
