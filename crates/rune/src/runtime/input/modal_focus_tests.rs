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

fn find_a11y<'a>(node: &'a serde_json::Value, key: &str) -> Option<&'a serde_json::Value> {
    if node["key"] == key { return Some(node); }
    node["children"].as_array()?.iter().find_map(|c| find_a11y(c, key))
}

fn element_center<S: 'static>(rt: &Runtime<S>, key: &str) -> (f32, f32) {
    let e = rt.elements().into_iter().find(|e| e.key.as_deref() == Some(key)).unwrap();
    (e.rect[0] + e.rect[2] * 0.5, e.rect[1] + e.rect[3] * 0.5)
}

#[test]
fn open_modal_makes_post_modal_siblings_inert_to_focus_a11y_and_pointer() {
    use crate::{Align, Side, row};

    #[derive(Default)]
    struct S {
        open: bool,
        click_outside: bool,
        after_presses: usize,
        after_hovers: usize,
        after_moves: usize,
        after_secondary: usize,
        pop_presses: usize,
        bg_pop_presses: usize,
    }
    fn view(s: &S, _: &mut Cx<S>) -> El {
        let mut root = row().key("root").size(600.0, 300.0).gap(16.0)
            .child(button("before"));
        if s.open {
            root = root.child(
                col().key("dialog").size(160.0, 140.0).role("dialog").label("Dialog")
                    .modal(s.click_outside, |s: &mut S| s.open = false)
                    .child(button("first"))
                    .child(button("last"))
                    .child(button("pop-trigger")),
            );
        }
        root = root.child(
            button("after")
                .on_press(|s: &mut S| s.after_presses += 1)
                .on_hover_change(|s: &mut S, on, _| if on { s.after_hovers += 1; })
                .on_pointer_move(|s: &mut S, _| s.after_moves += 1)
                .on_secondary_press(|s: &mut S, _, _| s.after_secondary += 1),
        );
        if s.open {
            // A popover anchored inside the modal stays live even when mounted after the modal,
            // while a popover anchored to a background element stays inert.
            root = root
                .child(col().key("pop-layer").child(
                    col().key("pop").size(120.0, 40.0).anchored("pop-trigger", Side::Bottom, Align::Start, 4.0)
                        .child(button("pop-item").on_press(|s: &mut S| s.pop_presses += 1)),
                ))
                .child(col().key("bg-pop").size(120.0, 40.0).anchored("before", Side::Bottom, Align::Start, 4.0)
                    .child(button("bg-pop-item").on_press(|s: &mut S| s.bg_pop_presses += 1)));
        }
        root
    }

    let mut rt = Runtime::new(S { open: true, click_outside: false, ..S::default() }, view, (600.0, 300.0), Env::new(true, 1.0));
    rt.set_full_keyboard_access(true);
    rt.frame(0.0);
    rt.frame(0.016);

    let els = rt.elements();
    let is_inert = |k: &str| els.iter().find(|e| e.key.as_deref() == Some(k)).map(|e| e.inert);
    assert_eq!(is_inert("before"), Some(true), "sibling before modal is inert");
    assert_eq!(is_inert("after"), Some(true), "sibling after modal must be inert");
    assert_eq!(is_inert("bg-pop-item"), Some(true), "popover anchored outside modal must be inert");
    assert_eq!(is_inert("first"), Some(false));
    assert_eq!(is_inert("last"), Some(false));
    assert_eq!(is_inert("pop-item"), Some(false), "popover anchored inside modal stays live");

    let a11y = rt.accessibility_json();
    assert!(find_a11y(&a11y, "before").is_none());
    assert!(find_a11y(&a11y, "after").is_none(), "sibling after modal must leave accessibility tree");
    assert!(find_a11y(&a11y, "bg-pop-item").is_none(), "background popover must leave accessibility tree");
    assert!(find_a11y(&a11y, "first").is_some());
    assert!(find_a11y(&a11y, "last").is_some());
    assert!(find_a11y(&a11y, "pop-item").is_some(), "modal-anchored popover stays in accessibility tree");

    let web_a11y = rt.a11y_json();
    assert!(!web_a11y.contains(r#""after""#), "sibling after modal must leave web a11y mirror");
    assert!(!web_a11y.contains(r#""bg-pop-item""#), "background popover must leave web a11y mirror");
    assert!(web_a11y.contains(r#""first""#) && web_a11y.contains(r#""pop-item""#), "modal and anchored popover stay in web a11y mirror");

    // Tab and Shift+Tab wrap strictly inside the modal and its anchored popover.
    assert_eq!(focused_element(&rt).map(|(k, _)| k), Some("first".into()));
    let mut visited = vec!["first".to_owned()];
    for _ in 0..4 {
        press(&mut rt, Key::Tab);
        visited.push(focused_element(&rt).map(|(k, _)| k).unwrap_or_default());
    }
    assert_eq!(visited, ["first", "last", "pop-trigger", "pop-item", "first"]);
    rt.key(KeyEvent { key: Key::Tab, mods: Mods { shift: true, ..Mods::default() } });
    rt.frame(0.016);
    assert_eq!(focused_element(&rt).map(|(k, _)| k), Some("pop-item".into()));

    // Pointer hover, move, primary click and secondary click must not reach the post-modal sibling.
    let (ax, ay) = element_center(&rt, "after");
    rt.pointer_move(ax, ay);
    rt.frame(0.016);
    rt.pointer_down(ax, ay);
    rt.pointer_up(ax, ay);
    rt.frame(0.016);
    rt.secondary_down(ax, ay);
    rt.secondary_up(ax, ay);
    rt.frame(0.016);
    assert_eq!(
        (rt.state.after_presses, rt.state.after_hovers, rt.state.after_moves, rt.state.after_secondary),
        (0, 0, 0, 0),
        "post-modal sibling must ignore pointer move, hover, primary press and secondary press"
    );
    assert!(rt.state.open, "modal with click_outside=false stays open");
    assert_eq!(focused_element(&rt).map(|(k, _)| k), Some("pop-item".into()));

    // Pointer clicks still reach the popover anchored inside the modal.
    let (px, py) = element_center(&rt, "pop-item");
    rt.pointer_down(px, py);
    rt.pointer_up(px, py);
    rt.frame(0.016);
    assert_eq!(rt.state.pop_presses, 1, "popover anchored inside modal accepts clicks");

    // With click_outside enabled, clicking on the post-modal sibling dismisses the modal without pressing it.
    rt.update(|s, _| s.click_outside = true);
    rt.frame(0.016);
    rt.pointer_down(ax, ay);
    rt.pointer_up(ax, ay);
    rt.frame(0.016);
    assert!(!rt.state.open, "clicking outside on post-modal sibling dismisses a click_outside modal");
    assert_eq!(rt.state.after_presses, 0, "dismissing click must not also press the post-modal sibling");
}

#[test]
fn playground_stage_modal_makes_side_column_inert_and_traps_tab() {
    use std::sync::{Arc, atomic::{AtomicUsize, Ordering}};
    use crate::{App, AppSpec, Playground, Story, text};

    struct Root;
    struct S { side_presses: Arc<AtomicUsize> }

    fn root_view(_: &Root, _: &mut Cx<Root>) -> El { el() }
    fn stage_btn(key: &'static str) -> El {
        el().key(key).role("button").label(key).focusable(true).size(90.0, 24.0).child(text(key))
    }
    fn stage(_: &S, _: &mut Cx<S>) -> El {
        col()
            .child(stage_btn("before"))
            .child(
                col().key("dialog").role("dialog").label("Dialog")
                    .modal(false, |_: &mut S| {})
                    .child(stage_btn("first"))
                    .child(stage_btn("last")),
            )
    }
    fn side(_: &S, _: &mut Cx<S>) -> El {
        col().child(stage_btn("side-button").on_press(|s: &mut S| { s.side_presses.fetch_add(1, Ordering::SeqCst); }))
    }

    let side_presses = Arc::new(AtomicUsize::new(0));
    let side_presses_story = Arc::clone(&side_presses);
    let spec = AppSpec::new("repro", (1000.0, 600.0), || Root, root_view);
    let playground = Playground::<Root>::new()
        .enabled(true)
        .story(Story::new("Repro/Modal", move || S { side_presses: Arc::clone(&side_presses_story) }, stage).side(side).variant("Only", |_| {}));
    let mut rt = App::new(spec).playground(playground).headless_runtime((1000.0, 600.0), Env::new(false, 1.0));
    rt.frame(0.0);
    rt.open_playground("Repro/Modal", None);
    rt.set_full_keyboard_access(true);
    rt.update(|_, tx| tx.focus("last", true));
    rt.frame(0.016);
    rt.frame(0.016);

    let els = rt.elements();
    let is_inert = |k: &str| els.iter().find(|e| e.key.as_deref() == Some(k)).map(|e| e.inert);
    let a11y = rt.accessibility_json();
    for k in ["playground-item-0", "before", "side-button", "playground-variant-0"] {
        assert_eq!(is_inert(k), Some(true), "{k} must be inert while stage modal is open");
        assert!(find_a11y(&a11y, k).is_none(), "{k} must leave the accessibility tree while stage modal is open");
    }
    for k in ["first", "last"] {
        assert_eq!(is_inert(k), Some(false), "{k} stays live");
        assert!(find_a11y(&a11y, k).is_some(), "{k} stays in the accessibility tree");
    }

    press(&mut rt, Key::Tab);
    assert_eq!(focused_element(&rt).map(|(k, _)| k), Some("first".into()), "Tab wraps inside the dialog instead of reaching the side column");
    press(&mut rt, Key::Tab);
    assert_eq!(focused_element(&rt).map(|(k, _)| k), Some("last".into()));

    let (sx, sy) = element_center(&rt, "side-button");
    rt.pointer_down(sx, sy);
    rt.pointer_up(sx, sy);
    rt.frame(0.016);
    assert_eq!(side_presses.load(Ordering::SeqCst), 0, "side column button must ignore clicks while stage modal is open");
}
