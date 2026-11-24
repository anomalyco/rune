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

#[test]
fn modal_trap_blocks_background_ignores_escape_and_restores_focus() {
    #[derive(Default)]
    struct S { open: bool, behind_presses: usize, root_escapes: usize }

    fn view(s: &S, _: &mut Cx<S>) -> El {
        let mut root = col()
            .on_key(|s: &mut S, ev, _| {
                if ev.key == Key::Escape { s.root_escapes += 1; return true; }
                false
            })
            .child(
                button("opener").on_press(|s: &mut S| { s.behind_presses += 1; s.open = true; }),
            );
        if s.open {
            root = root.child(
                col().key("trap").absolute(120.0, 0.0).size(160.0, 120.0)
                    .modal_trap()
                    .child(button("stay"))
                    .child(button("done").on_press(|s: &mut S| s.open = false)),
            );
        }
        root
    }

    let mut rt = Runtime::new(S::default(), view, (400.0, 300.0), Env::new(false, 1.0));
    rt.set_full_keyboard_access(true);
    rt.frame(0.0);

    press(&mut rt, Key::Tab);
    assert_eq!(focused_element(&rt).map(|(k, _)| k), Some("opener".into()));
    press(&mut rt, Key::Text(" ".into()));
    assert!(rt.state.open);
    assert_eq!(focused_element(&rt).map(|(k, _)| k), Some("stay".into()));

    // Escape neither closes the modal, blurs focus inside it, nor leaks to the root handler.
    press(&mut rt, Key::Escape);
    assert!(rt.state.open);
    assert_eq!(focused_element(&rt).map(|(k, _)| k), Some("stay".into()));
    assert_eq!(rt.state.root_escapes, 0);

    // Outside click does not reach the inert background.
    let (ox, oy) = element_center(&rt, "opener");
    rt.pointer_down(ox, oy);
    rt.pointer_up(ox, oy);
    rt.frame(0.016);
    assert!(rt.state.open);
    assert_eq!(rt.state.behind_presses, 1);

    // Tab stays trapped inside the modal, and its own button closes it and returns focus.
    press(&mut rt, Key::Tab);
    assert_eq!(focused_element(&rt).map(|(k, _)| k), Some("done".into()));
    press(&mut rt, Key::Text(" ".into()));
    assert!(!rt.state.open);
    assert_eq!(focused_element(&rt).map(|(k, _)| k), Some("opener".into()));
}

#[test]
fn outside_secondary_policy_table_matches_chrome_per_os() {
    use crate::runtime::input::secondary::{OutsideSecondary, SecondaryPlatform, outside_secondary_policy};
    use OutsideSecondary::{DismissRepost, DismissSwallow, Swallow};
    use SecondaryPlatform::{Linux, MacOs, Web, Windows};

    let cases = [
        // Non-dismissible modals (click_outside == false or modal_trap) swallow everywhere.
        (Windows, false, false, Swallow),
        (Windows, true, false, Swallow),
        (MacOs, false, false, Swallow),
        (MacOs, true, false, Swallow),
        (Linux, false, false, Swallow),
        (Linux, true, false, Swallow),
        (Web, false, false, Swallow),
        (Web, true, false, Swallow),
        // Non-menu modals (dialogs, selects) dismiss and swallow everywhere.
        (Windows, false, true, DismissSwallow),
        (MacOs, false, true, DismissSwallow),
        (Linux, false, true, DismissSwallow),
        (Web, false, true, DismissSwallow),
        // Menu modals dismiss and repost on Windows, and dismiss and swallow on macOS, Linux and web.
        (Windows, true, true, DismissRepost),
        (MacOs, true, true, DismissSwallow),
        (Linux, true, true, DismissSwallow),
        (Web, true, true, DismissSwallow),
    ];
    for (platform, is_menu, can_dismiss_outside, expected) in cases {
        assert_eq!(
            outside_secondary_policy(platform, is_menu, can_dismiss_outside),
            expected,
            "platform={platform:?} is_menu={is_menu} can_dismiss_outside={can_dismiss_outside}",
        );
    }
}

#[test]
fn outside_secondary_press_runtime_repost_and_swallow_branches_all_platforms() {
    use crate::runtime::input::secondary::SecondaryPlatform::{Linux, MacOs, Web, Windows};

    #[derive(Clone, Copy, Debug)]
    enum ModalKind { MenuSelf, MenuChild, Dialog, SelectListbox, MenuLocked, MenuTrap }
    struct S { kind: ModalKind, open: bool, closes: usize, bg_hits: usize, bg_other_hits: usize }

    fn view(s: &S, _: &mut Cx<S>) -> El {
        let mut root = col().w_full().h_full()
            .child(el().key("bg").absolute(0.0, 0.0).size(120.0, 60.0)
                .on_secondary_press(|s: &mut S, _, _| s.bg_hits += 1))
            .child(el().key("bg-other").absolute(0.0, 80.0).size(120.0, 60.0)
                .on_secondary_press(|s: &mut S, _, _| s.bg_other_hits += 1));
        if s.open {
            let m = col().key("modal").absolute(200.0, 0.0).size(140.0, 120.0);
            let modal = match s.kind {
                ModalKind::MenuSelf => m.role("menu").modal(true, |s: &mut S| { s.open = false; s.closes += 1; }),
                ModalKind::MenuChild => m.modal(true, |s: &mut S| { s.open = false; s.closes += 1; })
                    .child(col().key("inner-menu").role("menu").size(140.0, 120.0)),
                ModalKind::Dialog => m.role("dialog").modal(true, |s: &mut S| { s.open = false; s.closes += 1; }),
                ModalKind::SelectListbox => m.modal(true, |s: &mut S| { s.open = false; s.closes += 1; })
                    .child(col().key("listbox").role("listbox").size(140.0, 120.0)),
                ModalKind::MenuLocked => m.role("menu").modal(false, |s: &mut S| { s.open = false; s.closes += 1; }),
                ModalKind::MenuTrap => m.role("menu").modal_trap(),
            };
            root = root.child(modal);
        }
        root
    }

    for platform in [Windows, MacOs, Linux, Web] {
        for (kind, should_close, should_repost_on_win) in [
            (ModalKind::MenuSelf, true, true),
            (ModalKind::MenuChild, true, true),
            (ModalKind::Dialog, true, false),
            (ModalKind::SelectListbox, true, false),
            (ModalKind::MenuLocked, false, false),
            (ModalKind::MenuTrap, false, false),
        ] {
            let repost = platform == Windows && should_repost_on_win;
            // Same-frame secondary down + up.
            let mut rt = Runtime::new(
                S { kind, open: true, closes: 0, bg_hits: 0, bg_other_hits: 0 },
                view,
                (400.0, 200.0),
                Env::new(true, 1.0),
            );
            rt.frame(0.0);
            rt.core_mut().secondary_down_for(20.0, 20.0, platform);
            if !repost {
                assert_eq!(rt.secondary_claim(), "app", "swallowed modal press suppresses browser context menu ({platform:?}, {kind:?})");
            }
            rt.secondary_up(20.0, 20.0);
            rt.frame(0.016);
            assert_eq!(rt.state.open, !should_close, "same-frame open ({platform:?}, {kind:?})");
            assert_eq!(rt.state.closes, usize::from(should_close));
            assert_eq!(rt.state.bg_hits, usize::from(repost), "same-frame repost ({platform:?}, {kind:?})");

            // Cross-frame secondary down -> frame -> secondary up -> frame.
            let mut rt = Runtime::new(
                S { kind, open: true, closes: 0, bg_hits: 0, bg_other_hits: 0 },
                view,
                (400.0, 200.0),
                Env::new(true, 1.0),
            );
            rt.frame(0.0);
            rt.core_mut().secondary_down_for(20.0, 20.0, platform);
            rt.frame(0.016);
            assert_eq!(rt.state.open, !should_close, "cross-frame dismiss on down ({platform:?}, {kind:?})");
            assert_eq!(rt.state.bg_hits, 0, "repost waits for release");
            rt.secondary_up(20.0, 20.0);
            rt.frame(0.016);
            assert_eq!(rt.state.bg_hits, usize::from(repost), "cross-frame repost ({platform:?}, {kind:?})");
        }
    }

    // Dragging off onto another target before release drops the Windows repost (both same-frame and cross-frame).
    for cross_frame in [false, true] {
        let mut rt = Runtime::new(
            S { kind: ModalKind::MenuSelf, open: true, closes: 0, bg_hits: 0, bg_other_hits: 0 },
            view,
            (400.0, 200.0),
            Env::new(true, 1.0),
        );
        rt.frame(0.0);
        rt.core_mut().secondary_down_for(20.0, 20.0, Windows);
        if cross_frame { rt.frame(0.016); }
        rt.secondary_up(20.0, 100.0);
        rt.frame(0.016);
        assert!(!rt.state.open);
        assert_eq!((rt.state.bg_hits, rt.state.bg_other_hits), (0, 0), "drag-off cancels repost (cross_frame={cross_frame})");
    }
}

#[test]
fn windows_menu_secondary_repost_guards_refuse_new_modal_inert_and_inside_non_bubbling() {
    use crate::runtime::input::secondary::SecondaryPlatform::Windows;

    #[derive(Clone, Copy)]
    enum Mode { RefuseClose, OpenNewModal, InertTarget, NestedInDialog, InsideNonBubbling }
    struct S {
        mode: Mode,
        outer_open: bool,
        menu_open: bool,
        new_modal: bool,
        dismisses: usize,
        bg_hits: usize,
        inner_hits: usize,
        parent_hits: usize,
    }

    fn view(s: &S, _: &mut Cx<S>) -> El {
        let mut bg = el().key("bg").absolute(0.0, 0.0).size(120.0, 80.0)
            .on_secondary_press(|s: &mut S, _, _| s.bg_hits += 1);
        if matches!(s.mode, Mode::InertTarget) {
            bg = bg.inert();
        }
        let mut root = col().w_full().h_full().child(bg);
        if matches!(s.mode, Mode::NestedInDialog) && s.outer_open {
            root = root.child(
                col().key("outer").absolute(150.0, 0.0).size(200.0, 200.0).role("dialog")
                    .modal(false, |s: &mut S| s.outer_open = false)
                    .child(el().key("inner").size(100.0, 60.0).on_secondary_press(|s: &mut S, _, _| s.inner_hits += 1)),
            );
        }
        if s.menu_open {
            let menu = col().key("menu").absolute(260.0, 120.0).size(120.0, 60.0).role("menu")
                .modal(true, |s: &mut S| {
                    s.dismisses += 1;
                    match s.mode {
                        Mode::RefuseClose => {}
                        Mode::OpenNewModal => { s.menu_open = false; s.new_modal = true; }
                        _ => s.menu_open = false,
                    }
                });
            if matches!(s.mode, Mode::InsideNonBubbling) {
                root = root.child(
                    col().key("parent").absolute(0.0, 0.0).w_full().h_full()
                        .on_secondary_press(|s: &mut S, _, _| s.parent_hits += 1)
                        .child(menu),
                );
            } else {
                root = root.child(menu);
            }
        }
        if s.new_modal {
            root = root.child(
                col().key("confirm").absolute(0.0, 0.0).size(120.0, 80.0).role("dialog")
                    .modal(true, |s: &mut S| s.new_modal = false)
                    .on_secondary_press(|s: &mut S, _, _| s.inner_hits += 1),
            );
        }
        root
    }

    let make = |mode| S { mode, outer_open: true, menu_open: true, new_modal: false, dismisses: 0, bg_hits: 0, inner_hits: 0, parent_hits: 0 };

    // 1. Refusing to close drops the repost (both same-frame and cross-frame).
    for cross in [false, true] {
        let mut rt = Runtime::new(make(Mode::RefuseClose), view, (400.0, 220.0), Env::new(true, 1.0));
        rt.frame(0.0);
        rt.core_mut().secondary_down_for(20.0, 20.0, Windows);
        if cross { rt.frame(0.016); }
        rt.secondary_up(20.0, 20.0);
        rt.frame(0.016);
        assert!(rt.state.menu_open && rt.state.dismisses == 1 && rt.state.bg_hits == 0, "refuse close drops repost (cross={cross})");
    }

    // 2. Dismissing menu opens a new modal under the pointer: repost is dropped.
    for cross in [false, true] {
        let mut rt = Runtime::new(make(Mode::OpenNewModal), view, (400.0, 220.0), Env::new(true, 1.0));
        rt.frame(0.0);
        rt.core_mut().secondary_down_for(20.0, 20.0, Windows);
        if cross { rt.frame(0.016); }
        rt.secondary_up(20.0, 20.0);
        rt.frame(0.016);
        assert!(!rt.state.menu_open && rt.state.new_modal && rt.state.bg_hits == 0 && rt.state.inner_hits == 0, "new modal drops repost (cross={cross})");
    }

    // 3. Inert target behind menu does not receive the reposted secondary press.
    let mut rt = Runtime::new(make(Mode::InertTarget), view, (400.0, 220.0), Env::new(true, 1.0));
    rt.frame(0.0);
    rt.core_mut().secondary_down_for(20.0, 20.0, Windows);
    rt.secondary_up(20.0, 20.0);
    rt.frame(0.016);
    assert!(!rt.state.menu_open && rt.state.bg_hits == 0);

    // 4. Menu nested in outer modal: outside outer modal never pierces inert background; inside outer modal reposts.
    let mut rt = Runtime::new(make(Mode::NestedInDialog), view, (400.0, 220.0), Env::new(true, 1.0));
    rt.frame(0.0);
    rt.core_mut().secondary_down_for(20.0, 20.0, Windows);
    rt.secondary_up(20.0, 20.0);
    rt.frame(0.016);
    assert!(!rt.state.menu_open && rt.state.outer_open && rt.state.bg_hits == 0);

    rt.update(|s, _| s.menu_open = true);
    rt.frame(0.016);
    rt.core_mut().secondary_down_for(180.0, 20.0, Windows);
    rt.secondary_up(180.0, 20.0);
    rt.frame(0.016);
    assert!(!rt.state.menu_open && rt.state.outer_open && rt.state.inner_hits == 1);

    // 5. Right-clicking inside the menu does not dismiss it or bubble to an ancestor outside the modal.
    let mut rt = Runtime::new(make(Mode::InsideNonBubbling), view, (400.0, 220.0), Env::new(true, 1.0));
    rt.frame(0.0);
    rt.core_mut().secondary_down_for(300.0, 140.0, Windows);
    rt.secondary_up(300.0, 140.0);
    rt.frame(0.016);
    assert!(rt.state.menu_open && rt.state.dismisses == 0 && rt.state.parent_hits == 0);
}

#[test]
fn entering_modal_at_zero_opacity_does_not_dismiss_on_inside_primary_or_secondary_press() {
    use crate::runtime::input::secondary::SecondaryPlatform::{Linux, MacOs, Web, Windows};

    #[derive(Default)]
    struct S { open: bool, dismisses: usize, bg_secondary: usize }
    fn view(s: &S, _: &mut Cx<S>) -> El {
        let mut root = col().key("root").w_full().h_full()
            .on_secondary_press(|s: &mut S, _, _| s.bg_secondary += 1)
            .child(button("opener").on_press(|s: &mut S| s.open = true));
        if s.open {
            root = root.child(
                col().key("modal").absolute(100.0, 0.0).size(100.0, 120.0)
                    .modal(true, |s: &mut S| { s.open = false; s.dismisses += 1; })
                    .child(button("one")),
            );
        }
        root
    }

    for platform in [Windows, MacOs, Linux, Web] {
        let mut rt = Runtime::new(S::default(), view, (300.0, 300.0), Env::new(true, 2.0));
        rt.frame(0.0);
        rt.update(|s, _| s.open = true);
        // dt = 0.0 keeps the newly mounted modal at entrance opacity 0.0 (not yet in hit_tests).
        rt.frame(0.0);
        let (x, y) = rt.find("one").unwrap();

        rt.core_mut().secondary_down_for(x, y, platform);
        rt.secondary_up(x, y);
        rt.frame(0.0);
        assert!(rt.state.open, "inside secondary press on frame 0 must not dismiss entering modal ({platform:?})");
        assert_eq!((rt.state.dismisses, rt.state.bg_secondary), (0, 0));

        rt.pointer_down(x, y);
        rt.pointer_up(x, y);
        rt.frame(0.0);
        assert!(rt.state.open, "inside primary press on frame 0 must not dismiss entering modal ({platform:?})");
        assert_eq!(rt.state.dismisses, 0);

        // An actual outside press on frame 0 still dismisses the modal.
        rt.core_mut().secondary_down_for(5.0, 280.0, platform);
        rt.secondary_up(5.0, 280.0);
        rt.frame(0.0);
        assert!(!rt.state.open, "outside secondary press on frame 0 still dismisses ({platform:?})");
        assert_eq!((rt.state.dismisses, rt.state.bg_secondary), (1, 0));
    }
}

#[test]
fn modal_replacing_modal_in_same_commit_returns_focus_to_original_opener() {
    use crate::{TextEdit, TextStyle, text_input};

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    enum Mode { InitialFirst, InitialAutofocus, SelfFocusField, SelfFocusTx, TrapModal }
    #[derive(Clone, Copy, PartialEq, Eq)]
    enum Active { None, A, B }
    struct S { mode: Mode, active: Active, field: TextEdit }

    fn view(s: &S, _: &mut Cx<S>) -> El {
        let mut root = col().key("root").w_full().h_full()
            .child(button("before").on_press(|_: &mut S| {}))
            .child(button("opener").on_press(|s: &mut S| s.active = Active::A));
        match s.active {
            Active::None => {}
            Active::A => {
                root = root.child(
                    col().key("modal-a").size(200.0, 120.0).role("menu")
                        .modal(true, |s: &mut S| s.active = Active::None)
                        .child(button("pick").on_press_tx(|s: &mut S, tx| {
                            s.active = Active::B;
                            match s.mode {
                                Mode::SelfFocusField => s.field.focused = true,
                                Mode::SelfFocusTx => tx.focus("b-second", true),
                                _ => {}
                            }
                        })),
                );
            }
            Active::B => {
                let b_first = button("b-first").on_press(|s: &mut S| s.active = Active::None);
                let mut b_second = button("b-second").on_press(|s: &mut S| s.active = Active::None);
                if s.mode == Mode::InitialAutofocus { b_second = b_second.autofocus(); }
                let mut modal = col().key("modal-b").size(200.0, 120.0).role("dialog")
                    .child(b_first)
                    .child(b_second);
                if s.mode == Mode::SelfFocusField {
                    modal = modal.child(
                        text_input(&s.field, TextStyle::new(14.0, 400.0)).key("b-field").size(160.0, 28.0)
                            .on_edit(|s: &mut S| &mut s.field),
                    );
                }
                let modal = if s.mode == Mode::TrapModal {
                    modal.modal_trap()
                } else {
                    modal.modal(true, |s: &mut S| { s.active = Active::None; s.field.focused = false; })
                };
                root = root.child(modal);
            }
        }
        root
    }

    for (mode, expected_b_focus) in [
        (Mode::InitialFirst, "b-first"),
        (Mode::InitialAutofocus, "b-second"),
        (Mode::SelfFocusField, "b-field"),
        (Mode::SelfFocusTx, "b-second"),
        (Mode::TrapModal, "b-first"),
    ] {
        let mut rt = Runtime::new(S { mode, active: Active::None, field: TextEdit::new("") }, view, (400.0, 300.0), Env::new(true, 1.0));
        rt.frame(0.016);
        press(&mut rt, Key::Tab);
        press(&mut rt, Key::Tab);
        assert_eq!(focused_element(&rt), Some(("opener".into(), true)));

        press(&mut rt, Key::Text(" ".into()));
        assert_eq!(focused_element(&rt), Some(("pick".into(), true)));

        // Close Modal A and open Modal B in the same commit.
        press(&mut rt, Key::Text(" ".into()));
        assert!(rt.state.active == Active::B);
        assert_eq!(focused_element(&rt).map(|(k, _)| k), Some(expected_b_focus.into()), "mode={mode:?}");

        // Closing Modal B returns focus to the original opener.
        if mode == Mode::TrapModal {
            press(&mut rt, Key::Text(" ".into()));
        } else {
            press(&mut rt, Key::Escape);
        }
        assert!(rt.state.active == Active::None);
        assert_eq!(
            focused_element(&rt),
            Some(("opener".into(), true)),
            "closing replacement modal must return focus to original opener (mode={mode:?})"
        );
    }
}

#[test]
fn modal_replacement_chain_of_three_and_nested_replacements_preserve_restore() {
    use crate::{TextEdit, TextStyle, text_input};

    #[derive(Clone, Copy, PartialEq, Eq)]
    enum Step { None, A, B, C }
    struct S { outer: bool, step: Step, field: TextEdit }

    fn view(s: &S, _: &mut Cx<S>) -> El {
        let mut root = col().key("root").w_full().h_full()
            .child(button("before").on_press(|_: &mut S| {}))
            .child(button("opener").on_press(|s: &mut S| s.step = Step::A))
            .child(button("open-outer").on_press(|s: &mut S| s.outer = true));
        if s.outer {
            root = root.child(
                col().key("outer").size(300.0, 200.0).role("dialog")
                    .modal(true, |s: &mut S| s.outer = false)
                    .child(button("outer-first").on_press(|_: &mut S| {}))
                    .child(button("open-a").on_press(|s: &mut S| s.step = Step::A)),
            );
        }
        match s.step {
            Step::None => {}
            Step::A => {
                root = root.child(
                    col().key("modal-a").size(200.0, 120.0).role("menu")
                        .modal(true, |s: &mut S| s.step = Step::None)
                        .child(button("to-b").on_press(|s: &mut S| { s.step = Step::B; s.field.focused = true; }))
                        .child(button("replace-both").on_press(|s: &mut S| { s.outer = false; s.step = Step::C; })),
                );
            }
            Step::B => {
                root = root.child(
                    col().key("modal-b").size(200.0, 120.0).role("dialog")
                        .modal(true, |s: &mut S| { s.step = Step::None; s.field.focused = false; })
                        .child(text_input(&s.field, TextStyle::new(14.0, 400.0)).key("b-field").size(160.0, 28.0)
                            .on_edit(|s: &mut S| &mut s.field))
                        .child(button("to-c").on_press(|s: &mut S| { s.field.focused = false; s.step = Step::C; })),
                );
            }
            Step::C => {
                root = root.child(
                    col().key("modal-c").size(200.0, 120.0).role("dialog")
                        .modal(true, |s: &mut S| s.step = Step::None)
                        .child(button("c-first"))
                        .child(button("c-auto").autofocus()),
                );
            }
        }
        root
    }

    // 1. Top-level chain of three: opener -> A -> B (self-focused field) -> C (autofocus) -> opener.
    let mut rt = Runtime::new(S { outer: false, step: Step::None, field: TextEdit::new("") }, view, (400.0, 300.0), Env::new(true, 1.0));
    rt.frame(0.016);
    press(&mut rt, Key::Tab);
    press(&mut rt, Key::Tab);
    assert_eq!(focused_element(&rt), Some(("opener".into(), true)));
    press(&mut rt, Key::Text(" ".into()));
    assert_eq!(focused_element(&rt), Some(("to-b".into(), true)));
    press(&mut rt, Key::Text(" ".into()));
    assert_eq!(focused_element(&rt).map(|(k, _)| k), Some("b-field".into()));
    press(&mut rt, Key::Tab);
    assert_eq!(focused_element(&rt), Some(("to-c".into(), true)));
    press(&mut rt, Key::Text(" ".into()));
    assert_eq!(focused_element(&rt), Some(("c-auto".into(), true)));
    press(&mut rt, Key::Escape);
    assert_eq!(focused_element(&rt), Some(("opener".into(), true)), "chain of three returns to original opener");

    // 2. Nested [Outer, A] -> [Outer, B] preserves Outer's opener (not outer-first), then Outer -> open-outer.
    let mut rt = Runtime::new(S { outer: false, step: Step::None, field: TextEdit::new("") }, view, (400.0, 300.0), Env::new(true, 1.0));
    rt.frame(0.016);
    press(&mut rt, Key::Tab);
    press(&mut rt, Key::Tab);
    press(&mut rt, Key::Tab);
    assert_eq!(focused_element(&rt), Some(("open-outer".into(), true)));
    press(&mut rt, Key::Text(" ".into()));
    press(&mut rt, Key::Tab);
    assert_eq!(focused_element(&rt), Some(("open-a".into(), true)));
    press(&mut rt, Key::Text(" ".into()));
    assert_eq!(focused_element(&rt), Some(("to-b".into(), true)));
    press(&mut rt, Key::Text(" ".into()));
    assert_eq!(focused_element(&rt).map(|(k, _)| k), Some("b-field".into()));
    press(&mut rt, Key::Escape);
    assert_eq!(focused_element(&rt), Some(("open-a".into(), true)), "closing nested replacement returns to inner opener");
    press(&mut rt, Key::Escape);
    assert_eq!(focused_element(&rt), Some(("open-outer".into(), true)), "closing outer returns to page opener");

    // 3. Nested [Outer, A] both close as C opens -> closing C returns to open-outer.
    press(&mut rt, Key::Text(" ".into()));
    press(&mut rt, Key::Tab);
    press(&mut rt, Key::Text(" ".into()));
    press(&mut rt, Key::Tab);
    assert_eq!(focused_element(&rt), Some(("replace-both".into(), true)));
    press(&mut rt, Key::Text(" ".into()));
    assert!(!rt.state.outer && rt.state.step == Step::C);
    assert_eq!(focused_element(&rt), Some(("c-auto".into(), true)));
    press(&mut rt, Key::Escape);
    assert_eq!(focused_element(&rt), Some(("open-outer".into(), true)), "replacing two popped modals carries outermost restore");
}

#[test]
fn modal_replacement_with_removed_disabled_or_inert_opener_keeps_focus_sensible() {
    #[derive(Clone, Copy, Debug)]
    enum OpenerLoss { DisabledAtReplace, RemovedAtReplace, InertAtReplace, DisabledAtClose, RemovedAtClose }
    #[derive(Clone, Copy, PartialEq, Eq)]
    enum Active { None, A, B }
    struct S { loss: OpenerLoss, outer: bool, active: Active, lost: bool }

    fn view(s: &S, _: &mut Cx<S>) -> El {
        let make_opener = |key: &str| -> Option<El> {
            let removed = s.lost && matches!(s.loss, OpenerLoss::RemovedAtReplace | OpenerLoss::RemovedAtClose);
            if removed { return None; }
            let disabled = s.lost && matches!(s.loss, OpenerLoss::DisabledAtReplace | OpenerLoss::DisabledAtClose);
            let mut b = button(key).disabled(disabled).on_press(|s: &mut S| s.active = Active::A);
            if s.lost && matches!(s.loss, OpenerLoss::InertAtReplace) { b = b.inert(); }
            Some(b)
        };
        let mut root = col().key("root").w_full().h_full()
            .child(button("before").on_press(|_: &mut S| {}));
        if !s.outer && let Some(op) = make_opener("opener") { root = root.child(op); }
        if s.outer {
            let mut outer = col().key("outer").size(300.0, 200.0).role("dialog")
                .modal(true, |s: &mut S| s.outer = false)
                .child(button("other").on_press(|_: &mut S| {}));
            if let Some(op) = make_opener("open-a") { outer = outer.child(op); }
            root = root.child(outer);
        }
        match s.active {
            Active::None => {}
            Active::A => {
                root = root.child(
                    col().key("modal-a").size(200.0, 120.0).role("menu")
                        .modal(true, |s: &mut S| s.active = Active::None)
                        .child(button("pick").on_press(|s: &mut S| {
                            if matches!(s.loss, OpenerLoss::DisabledAtReplace | OpenerLoss::RemovedAtReplace | OpenerLoss::InertAtReplace) {
                                s.lost = true;
                            }
                            s.active = Active::B;
                        })),
                );
            }
            Active::B => {
                root = root.child(
                    col().key("modal-b").size(200.0, 120.0).role("dialog")
                        .modal(true, |s: &mut S| {
                            if matches!(s.loss, OpenerLoss::DisabledAtClose | OpenerLoss::RemovedAtClose) {
                                s.lost = true;
                            }
                            s.active = Active::None;
                        })
                        .child(button("b-ok").on_press(|s: &mut S| {
                            if matches!(s.loss, OpenerLoss::DisabledAtClose | OpenerLoss::RemovedAtClose) {
                                s.lost = true;
                            }
                            s.active = Active::None;
                        })),
                );
            }
        }
        root
    }

    for loss in [
        OpenerLoss::DisabledAtReplace,
        OpenerLoss::RemovedAtReplace,
        OpenerLoss::InertAtReplace,
        OpenerLoss::DisabledAtClose,
        OpenerLoss::RemovedAtClose,
    ] {
        // Top-level: focus enters B while B is open, does not return to disabled/inert/removed opener on close,
        // and Tab cleanly lands on the remaining enabled control.
        let mut rt = Runtime::new(S { loss, outer: false, active: Active::None, lost: false }, view, (400.0, 300.0), Env::new(true, 1.0));
        rt.frame(0.016);
        press(&mut rt, Key::Tab);
        press(&mut rt, Key::Tab);
        assert_eq!(focused_element(&rt), Some(("opener".into(), true)));
        press(&mut rt, Key::Text(" ".into()));
        press(&mut rt, Key::Text(" ".into()));
        assert_eq!(focused_element(&rt), Some(("b-ok".into(), true)), "focus enters B ({loss:?})");
        press(&mut rt, Key::Escape);
        assert_eq!(focused_element(&rt), None, "lost opener must not regain focus ({loss:?})");
        assert!(!rt.has_focus());
        press(&mut rt, Key::Tab);
        assert_eq!(focused_element(&rt), Some(("before".into(), true)), "Tab reaches remaining control ({loss:?})");

        // Nested inside Outer: closing B falls back inside Outer to "other", never nowhere.
        let mut rt = Runtime::new(S { loss, outer: true, active: Active::None, lost: false }, view, (400.0, 300.0), Env::new(true, 1.0));
        rt.frame(0.016);
        press(&mut rt, Key::Tab);
        assert_eq!(focused_element(&rt), Some(("open-a".into(), true)));
        press(&mut rt, Key::Text(" ".into()));
        press(&mut rt, Key::Text(" ".into()));
        assert_eq!(focused_element(&rt), Some(("b-ok".into(), true)), "nested focus enters B ({loss:?})");
        press(&mut rt, Key::Escape);
        assert_eq!(focused_element(&rt), Some(("other".into(), true)), "nested close falls back inside Outer ({loss:?})");
    }
}
