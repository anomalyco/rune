use crate::{
    Anim, Color, Cx, El, Env, Key, KeyEvent, Link, Runtime, ScrollSpec, TouchPhase, Transition, Tx,
    col, el, text,
};

fn tab<S: 'static>(rt: &mut Runtime<S>) {
    rt.key(KeyEvent { key: Key::Tab, mods: crate::Mods::default() });
    rt.frame(1.0 / 60.0);
}

fn check_hover<S: 'static>(rt: &mut Runtime<S>, sel: &str) -> bool {
    let mut hit = false;
    rt.update(|_, tx| hit = tx.is_hovered(sel));
    hit
}

fn current_hovered<S: 'static>(rt: &mut Runtime<S>) -> Option<String> {
    let mut seen = None;
    rt.update(|_, tx| seen = tx.hovered().map(str::to_owned));
    seen
}

#[test]
fn default_tx_has_no_hover() {
    let tx = Tx::default();
    assert_eq!(tx.hovered(), None);
    assert!(!tx.is_hovered(""));
    assert!(!tx.is_hovered("card"));
}

#[test]
fn hover_or_focus_visible_keeps_lift_across_focus_loss_and_hover_leave() {
    #[derive(Default)]
    struct S {
        lifted: bool,
        events: Vec<(&'static str, bool, bool)>,
    }

    fn view(_: &S, _: &mut Cx<S>) -> El {
        col().w_full().h_full().gap(8.0)
            .child(
                col().key("card").size(160.0, 60.0)
                    .on_hover_change(|s: &mut S, inside, tx| {
                        s.lifted = inside || (tx.focus_within("card") && tx.focus_visible());
                        s.events.push(("hover", inside, tx.is_hovered("card")));
                    })
                    .on_focus_within_change(|s: &mut S, focused, tx| {
                        s.lifted = (focused && tx.focus_visible()) || tx.is_hovered("card");
                        s.events.push(("focus", focused, tx.is_hovered("card")));
                    })
                    .child(el().key("btn").size(80.0, 30.0).focusable(true).on_press(|_: &mut S| {})),
            )
            .child(el().key("other").size(80.0, 30.0).focusable(true).on_press(|_: &mut S| {}))
    }

    let mut rt = Runtime::new(S::default(), view, (240.0, 140.0), Env::new(true, 1.0));
    rt.frame(0.0);

    // 1. Focus enters card/btn via keyboard -> lifted.
    tab(&mut rt);
    assert!(rt.state.lifted);
    assert_eq!(rt.state.events.last(), Some(&("focus", true, false)));

    // 2. Pointer moves onto child button inside card -> still lifted.
    rt.pointer_move(40.0, 15.0);
    rt.frame(1.0 / 60.0);
    assert!(rt.state.lifted);
    assert_eq!(current_hovered(&mut rt).as_deref(), Some("btn"));
    assert!(check_hover(&mut rt, "card"));
    assert!(check_hover(&mut rt, "card/btn"));

    // 3. Focus leaves card via Tab while pointer is still inside -> stays lifted!
    tab(&mut rt);
    assert!(rt.state.lifted, "focus loss must keep lift while pointer is over card");
    assert_eq!(rt.state.events.last(), Some(&("focus", false, true)));

    // 4. Pointer leaves card (neither focused nor hovered) -> lift drops.
    rt.pointer_move(200.0, 120.0);
    rt.frame(1.0 / 60.0);
    assert!(!rt.state.lifted);
    assert_eq!(rt.state.events.last(), Some(&("hover", false, false)));

    // 5. Reverse order: pointer moves onto card outside button -> lifted; Tab back into card ->
    // pointer leaves while focused -> stays lifted until focus also leaves.
    rt.pointer_move(120.0, 45.0);
    rt.frame(1.0 / 60.0);
    assert!(rt.state.lifted);
    assert_eq!(current_hovered(&mut rt).as_deref(), Some("card"));
    assert!(check_hover(&mut rt, "card"));
    assert!(!check_hover(&mut rt, "btn"));

    tab(&mut rt);
    assert!(rt.state.lifted);

    rt.pointer_move(200.0, 120.0);
    rt.frame(1.0 / 60.0);
    assert!(rt.state.lifted, "hover leave must keep lift while focus-visible is within card");

    tab(&mut rt);
    assert!(!rt.state.lifted);
}

#[test]
fn handles_key_paths_unkeyed_descendants_escaped_keys_and_links() {
    #[derive(Default)]
    struct S;

    fn btn(key: Option<&'static str>, label: &'static str) -> El {
        let b = el().size(140.0, 22.0).on_press(|_: &mut S| {}).child(text(label));
        match key { Some(k) => b.key(k), None => b }
    }

    fn view(_: &S, _: &mut Cx<S>) -> El {
        col().w_full().h_full().padding(4.0).gap(4.0)
            .child(
                col().key("popover")
                    .child(btn(Some("close"), "Close popover"))
                    .child(btn(None, "Unkeyed in popover")),
            )
            .child(
                col().key("page")
                    .child(btn(Some("close"), "Close page")),
            )
            .child(
                col().key("list")
                    .child(col().key("row/0").child(btn(Some("edit"), "Edit row"))),
            )
            .child(
                col().key("article").child(
                    text("Read docs")
                        .key("body")
                        .size(140.0, 22.0)
                        .link(Link::new(0..9, "docs-link")),
                ),
            )
            .child(el().size(140.0, 22.0).on_press(|_: &mut S| {}))
    }

    let mut rt = Runtime::new(S, view, (300.0, 260.0), Env::new(true, 1.0));
    rt.frame(0.0);

    // Empty unkeyed background -> None.
    rt.pointer_move(280.0, 240.0);
    rt.frame(1.0 / 60.0);
    assert_eq!(current_hovered(&mut rt), None);
    assert!(!check_hover(&mut rt, "popover"));
    assert!(!check_hover(&mut rt, ""));

    // 1. popover/close (including over its unkeyed text label).
    let (x, y) = rt.find("popover/close").unwrap();
    rt.pointer_move(x, y);
    rt.frame(1.0 / 60.0);
    assert_eq!(current_hovered(&mut rt).as_deref(), Some("close"));
    assert!(check_hover(&mut rt, "popover"));
    assert!(check_hover(&mut rt, "close"));
    assert!(check_hover(&mut rt, "popover/close"));
    assert!(!check_hover(&mut rt, "page"));
    assert!(!check_hover(&mut rt, "page/close"));
    assert!(!check_hover(&mut rt, ""));

    // 2. Unkeyed hit button inside popover: nearest keyed element is "popover".
    rt.pointer_move(x, y + 22.0);
    rt.frame(1.0 / 60.0);
    assert_eq!(current_hovered(&mut rt).as_deref(), Some("popover"));
    assert!(check_hover(&mut rt, "popover"));
    assert!(!check_hover(&mut rt, "popover/close"));

    // 3. page/close (same leaf key, different ancestor).
    let (x, y) = rt.find("page/close").unwrap();
    rt.pointer_move(x, y);
    rt.frame(1.0 / 60.0);
    assert_eq!(current_hovered(&mut rt).as_deref(), Some("close"));
    assert!(check_hover(&mut rt, "page"));
    assert!(check_hover(&mut rt, "page/close"));
    assert!(!check_hover(&mut rt, "popover"));
    assert!(!check_hover(&mut rt, "popover/close"));

    // 4. Escaped key containing '/'.
    let (x, y) = rt.find("list/row/0/edit").unwrap();
    rt.pointer_move(x, y);
    rt.frame(1.0 / 60.0);
    assert_eq!(current_hovered(&mut rt).as_deref(), Some("edit"));
    assert!(check_hover(&mut rt, "list"));
    assert!(check_hover(&mut rt, "row/0"));
    assert!(check_hover(&mut rt, "list/row/0"));
    assert!(check_hover(&mut rt, "list/row/0/edit"));
    assert!(!check_hover(&mut rt, "popover/edit"));

    // 5. Hovered inline Link inside article/body.
    let (x, y) = rt.find("docs-link").unwrap();
    rt.pointer_move(x, y);
    rt.frame(1.0 / 60.0);
    assert_eq!(current_hovered(&mut rt).as_deref(), Some("docs-link"));
    assert!(check_hover(&mut rt, "docs-link"));
    assert!(check_hover(&mut rt, "body/docs-link"));
    assert!(check_hover(&mut rt, "article/body/docs-link"));
    assert!(check_hover(&mut rt, "body"));
    assert!(check_hover(&mut rt, "article"));
    assert!(!check_hover(&mut rt, "page/docs-link"));

    // 6. Completely unkeyed hit button falls back to its #n segment.
    let (_, by) = rt.find("body").unwrap();
    rt.pointer_move(40.0, by + 26.0);
    rt.frame(1.0 / 60.0);
    assert!(current_hovered(&mut rt).as_deref().is_some_and(|k| k.starts_with('#')));
}

#[test]
fn non_hit_elements_passthrough_overlays_occlusion_hover_below_and_overhang() {
    struct S {
        passes_below: bool,
        keyed_overlay: bool,
    }

    fn view(s: &S, _: &mut Cx<S>) -> El {
        let mut toast = col().absolute(0.0, 0.0).w_full().h_full();
        if s.keyed_overlay { toast = toast.key("toast-layer"); }

        let mut layer = el().key("layer").absolute(0.0, 120.0).size(120.0, 40.0).on_press(|_: &mut S| {});
        if s.passes_below { layer = layer.hover_passes_below(); }

        el().w_full().h_full()
            // Plain non-hit card with hover styling only (no hit handler).
            .child(
                col().key("card").absolute(0.0, 0.0).size(120.0, 40.0).hover(Color::WHITE.alpha(0.1))
                    .child(el().key("badge").size(40.0, 20.0)),
            )
            // Fully transparent overlay over card: must not steal hover.
            .child(el().key("invisible").absolute(0.0, 0.0).size(120.0, 40.0).opacity(0.0))
            // Interactive cover over right half of card: blocks card.
            .child(el().key("cover").absolute(80.0, 0.0).size(40.0, 40.0).hit())
            // Non-hit pass-through sibling covering window (e.g. toast container).
            .child(toast)
            // Overhanging hit child: bar-1 hangs over row-0.
            .child(col().key("row-0").absolute(0.0, 50.0).size(120.0, 30.0).hover(Color::WHITE.alpha(0.1)))
            .child(
                col().key("row-1").absolute(0.0, 80.0).size(120.0, 30.0)
                    .child(el().key("bar-1").absolute(60.0, -15.0).size(40.0, 20.0).hit()),
            )
            // hover_passes_below layer over tab, with menu above both.
            .child(el().key("tab").absolute(0.0, 120.0).size(100.0, 40.0).on_press(|_: &mut S| {}))
            .child(layer)
            .child(el().key("menu").absolute(60.0, 120.0).size(50.0, 40.0).on_press(|_: &mut S| {}))
    }

    for keyed_overlay in [false, true] {
        let mut rt = Runtime::new(
            S { passes_below: true, keyed_overlay },
            view,
            (200.0, 180.0),
            Env::new(true, 1.0),
        );
        rt.frame(0.0);

        // Non-hit card & badge under pass-through toast layer and invisible element.
        rt.pointer_move(20.0, 10.0);
        rt.frame(1.0 / 60.0);
        assert!(check_hover(&mut rt, "card"), "keyed_overlay={keyed_overlay}: card hovered under pass-through layer");
        assert!(check_hover(&mut rt, "badge"), "keyed_overlay={keyed_overlay}: badge hovered");
        assert!(!check_hover(&mut rt, "invisible"), "opacity 0 must not hover");
        if !keyed_overlay {
            assert_eq!(current_hovered(&mut rt).as_deref(), Some("badge"));
        }

        // Interactive cover blocks card underneath.
        rt.pointer_move(100.0, 20.0);
        rt.frame(1.0 / 60.0);
        assert!(check_hover(&mut rt, "cover"));
        assert!(!check_hover(&mut rt, "card"), "hit cover blocks non-hit card underneath");
        assert_eq!(current_hovered(&mut rt).as_deref(), Some("cover"));

        // Overhanging bar-1 keeps row-1 hovered and blocks row-0.
        rt.pointer_move(80.0, 72.0);
        rt.frame(1.0 / 60.0);
        assert!(check_hover(&mut rt, "bar-1"));
        assert!(check_hover(&mut rt, "row-1"));
        assert!(!check_hover(&mut rt, "row-0"));
        assert_eq!(current_hovered(&mut rt).as_deref(), Some("bar-1"));

        // hover_passes_below lets tab hover while menu above both wins.
        rt.pointer_move(20.0, 140.0);
        rt.frame(1.0 / 60.0);
        assert!(check_hover(&mut rt, "tab"));
        assert!(!check_hover(&mut rt, "layer"));
        assert_eq!(current_hovered(&mut rt).as_deref(), Some("tab"));

        rt.pointer_move(80.0, 140.0);
        rt.frame(1.0 / 60.0);
        assert!(check_hover(&mut rt, "menu"));
        assert!(!check_hover(&mut rt, "tab"));

        // Without hover_passes_below, layer blocks tab.
        rt.update(|s, _| s.passes_below = false);
        rt.frame(1.0 / 60.0);
        rt.pointer_move(20.0, 140.0);
        rt.frame(1.0 / 60.0);
        assert!(check_hover(&mut rt, "layer"));
        assert!(!check_hover(&mut rt, "tab"));
    }
}

#[test]
fn modals_inert_exiting_and_unmounted_clear_hover_even_mid_commit() {
    #[derive(Default)]
    struct S {
        show_card: bool,
        show_btn: bool,
        exiting_card: bool,
        modal_open: bool,
        inert_card: bool,
        focus_loss_saw_card: Option<bool>,
        focus_loss_saw_btn: Option<bool>,
    }

    fn view(s: &S, _: &mut Cx<S>) -> El {
        let mut root = el().w_full().h_full();
        if s.show_card {
            let mut card = col().key("card").size(160.0, 60.0)
                .on_focus_within_change(|st: &mut S, focused, tx| {
                    if !focused {
                        st.focus_loss_saw_card = Some(tx.is_hovered("card"));
                        st.focus_loss_saw_btn = Some(tx.is_hovered("btn"));
                    }
                });
            if s.inert_card { card = card.inert(); }
            if s.exiting_card { card = card.transition(Transition::opacity()); }
            if s.show_btn {
                card = card.child(el().key("btn").size(80.0, 30.0).focusable(true).on_press(|_: &mut S| {}));
            }
            root = root.child(card);
        }
        if s.modal_open {
            root = root.child(
                col().key("dialog").absolute(100.0, 0.0).size(100.0, 60.0).modal(false, |_: &mut S| {})
                    .child(el().key("modal-btn").size(80.0, 30.0).focusable(true).on_press(|_: &mut S| {})),
            );
        }
        root
    }

    // Case 1: unmounting only child btn keeps parent card hovered during mid-commit focus loss.
    let mut rt = Runtime::new(
        S { show_card: true, show_btn: true, ..S::default() },
        view,
        (240.0, 100.0),
        Env::new(true, 1.0),
    );
    rt.frame(0.0);
    tab(&mut rt);
    rt.pointer_move(40.0, 15.0);
    rt.frame(1.0 / 60.0);
    assert!(check_hover(&mut rt, "btn"));
    assert!(check_hover(&mut rt, "card"));

    rt.update(|s, _| s.show_btn = false);
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.state.focus_loss_saw_btn, Some(false), "unmounted child stops hovering immediately");
    assert_eq!(rt.state.focus_loss_saw_card, Some(true), "live parent stays hovered when only child unmounts");

    // Case 2: unmounting whole card clears card hover immediately in mid-commit focus loss.
    rt.update(|s, _| { s.show_btn = true; s.focus_loss_saw_card = None; s.focus_loss_saw_btn = None; });
    rt.frame(1.0 / 60.0);
    tab(&mut rt);
    rt.pointer_move(40.0, 15.0);
    rt.frame(1.0 / 60.0);
    rt.update(|s, _| s.show_card = false);
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.state.focus_loss_saw_card, Some(false));
    assert_eq!(rt.state.focus_loss_saw_btn, Some(false));
    assert!(!check_hover(&mut rt, "card"));

    // Case 3: exiting card (with opacity transition) stops hovering immediately.
    rt.update(|s, _| { s.show_card = true; s.exiting_card = true; });
    rt.frame(1.0 / 60.0);
    rt.pointer_move(40.0, 15.0);
    rt.frame(1.0 / 60.0);
    assert!(check_hover(&mut rt, "card"));
    rt.update(|s, _| s.show_card = false);
    rt.frame(1.0 / 60.0);
    assert!(!check_hover(&mut rt, "card"), "exiting card must not report hovered");

    // Case 4: inert and modal block hover.
    rt.update(|s, _| { s.show_card = true; s.exiting_card = false; s.inert_card = true; });
    rt.frame(1.0 / 60.0);
    assert!(!check_hover(&mut rt, "card"), "inert card must not hover");

    rt.update(|s, _| { s.inert_card = false; s.modal_open = true; });
    rt.frame(1.0 / 60.0);
    rt.pointer_move(40.0, 15.0);
    rt.frame(1.0 / 60.0);
    assert!(!check_hover(&mut rt, "card"), "modal blocks background hover");
    assert_eq!(current_hovered(&mut rt), None);

    rt.pointer_move(140.0, 15.0);
    rt.frame(1.0 / 60.0);
    assert!(check_hover(&mut rt, "dialog"));
    assert!(check_hover(&mut rt, "modal-btn"));
}

#[test]
fn pointer_leave_cancel_pointer_touch_and_held_press_capture() {
    #[derive(Default)]
    struct S {
        cancel_on_press: bool,
    }

    fn view(s: &S, _: &mut Cx<S>) -> El {
        let cancel = s.cancel_on_press;
        col().size(240.0, 160.0)
            .child(
                col().key("pill").size(160.0, 60.0)
                    .child(
                        el().key("btn").size(100.0, 40.0)
                            .press_scale(0.8)
                            .on_press_tx(move |_: &mut S, tx| {
                                if cancel { tx.cancel_pointer(); }
                            }),
                    ),
            )
            .child(el().key("other").size(100.0, 40.0).on_press(|_: &mut S| {}))
    }

    let mut rt = Runtime::new(S::default(), view, (240.0, 160.0), Env::new(true, 1.0));
    rt.frame(0.0);

    // Move onto btn edge and press down: press_scale shrinks drawn rect, but capture inside keeps hover.
    rt.pointer_move(3.0, 20.0);
    rt.frame(1.0 / 60.0);
    assert!(check_hover(&mut rt, "btn"));
    assert!(check_hover(&mut rt, "pill"));

    rt.pointer_down(3.0, 20.0);
    rt.frame(1.0 / 60.0);
    assert!(check_hover(&mut rt, "btn"));
    assert!(check_hover(&mut rt, "pill"));

    // Drag outside btn but still inside pill: btn leaves, pill stays hovered, other stays blocked.
    rt.pointer_move(130.0, 30.0);
    rt.frame(1.0 / 60.0);
    assert!(!check_hover(&mut rt, "btn"));
    assert!(check_hover(&mut rt, "pill"));
    assert!(!check_hover(&mut rt, "other"));

    // Drag onto unrelated sibling "other" while captured: capture blocks "other" and clears "pill".
    rt.pointer_move(40.0, 80.0);
    rt.frame(1.0 / 60.0);
    assert!(!check_hover(&mut rt, "btn"));
    assert!(!check_hover(&mut rt, "pill"));
    assert!(!check_hover(&mut rt, "other"));
    assert_eq!(current_hovered(&mut rt), None);

    // Release over "other" -> hover returns to "other".
    rt.pointer_up(40.0, 80.0);
    rt.frame(1.0 / 60.0);
    assert!(check_hover(&mut rt, "other"));

    // pointer_leave clears hover.
    rt.pointer_leave();
    rt.frame(1.0 / 60.0);
    assert_eq!(current_hovered(&mut rt), None);
    assert!(!check_hover(&mut rt, "other"));

    // tx.cancel_pointer() clears hover.
    rt.update(|s, _| s.cancel_on_press = true);
    rt.pointer_move(40.0, 20.0);
    rt.frame(1.0 / 60.0);
    assert!(check_hover(&mut rt, "btn"));
    rt.pointer_down(40.0, 20.0);
    rt.pointer_up(40.0, 20.0);
    rt.frame(1.0 / 60.0);
    assert_eq!(current_hovered(&mut rt), None);
    assert!(!check_hover(&mut rt, "btn"));

    // Touch tap has no hover.
    rt.touch(1, TouchPhase::Started, 40.0, 20.0, 0.0);
    assert_eq!(current_hovered(&mut rt), None);
    assert!(!check_hover(&mut rt, "btn"));
    rt.touch(1, TouchPhase::Ended, 40.0, 20.0, 0.05);
    rt.frame(1.0 / 60.0);
    assert_eq!(current_hovered(&mut rt), None);
    assert!(!check_hover(&mut rt, "btn"));
}

#[test]
fn hover_hold_keeps_initial_row_hovered_during_programmatic_glide() {
    #[derive(Default)]
    struct S;

    fn view(_: &S, _: &mut Cx<S>) -> El {
        let rows = (0..40).map(|i| {
            el().key(format!("row-{i}")).w_full().h(30.0)
                .hover(Color::WHITE.alpha(0.1))
                .hover_motion(Anim::INSTANT, Anim::INSTANT)
                .on_press(|_: &mut S| {})
        });
        col().key("list").w_full().h_full().scroll(ScrollSpec { stick_bottom: false }).children(rows)
    }

    let mut rt = Runtime::new(S, view, (200.0, 120.0), Env::new(true, 1.0));
    rt.frame(0.0);
    rt.pointer_move(50.0, 45.0); // row-1 (y = 30..60)
    rt.frame(1.0 / 60.0);
    assert!(check_hover(&mut rt, "row-1"));

    // Start a programmatic scroll glide under the stationary pointer.
    rt.update(|_, tx| tx.scroll_to_y("list", 300.0));
    let mut saw_held = false;
    for _ in 0..10 {
        rt.frame(1.0 / 60.0);
        if rt.core().held_hover().is_some() {
            saw_held = true;
            assert!(check_hover(&mut rt, "row-1"), "held glide keeps initial row hovered");
            assert!(!check_hover(&mut rt, "row-5"), "passing rows stay unhovered while held");
            break;
        }
    }
    assert!(saw_held, "glide should enter HoverHold");

    // Moving the pointer releases the hold and picks up the row now under it.
    rt.pointer_move(51.0, 45.0);
    rt.frame(1.0 / 60.0);
    assert!(!check_hover(&mut rt, "row-1"));
}

#[test]
fn hover_change_poll_and_multiple_runtimes_stay_consistent_and_isolated() {
    #[derive(Default)]
    struct S {
        changes: Vec<(&'static str, bool, bool, bool, Option<String>)>,
        poll_hovered: Option<String>,
        poll_over_b: bool,
    }

    fn view(_: &S, _: &mut Cx<S>) -> El {
        col().w_full().h_full()
            .child(
                el().key("a").size(100.0, 40.0).on_hover_change(|s: &mut S, inside, tx| {
                    s.changes.push(("a", inside, tx.is_hovered("a"), tx.is_hovered("b"), tx.hovered().map(str::to_owned)));
                }),
            )
            .child(
                el().key("b").size(100.0, 40.0).on_hover_change(|s: &mut S, inside, tx| {
                    s.changes.push(("b", inside, tx.is_hovered("a"), tx.is_hovered("b"), tx.hovered().map(str::to_owned)));
                }),
            )
    }

    fn poll(s: &mut S, _: f64, tx: &mut Tx) -> bool {
        s.poll_hovered = tx.hovered().map(str::to_owned);
        s.poll_over_b = tx.is_hovered("b");
        false
    }

    let mut rt1 = Runtime::new(S::default(), view, (200.0, 100.0), Env::new(true, 1.0));
    let mut rt2 = Runtime::new(S::default(), view, (200.0, 100.0), Env::new(true, 1.0));
    rt1.poll = Some(poll);
    rt1.frame(0.0);
    rt2.frame(0.0);

    rt1.pointer_move(20.0, 20.0);
    rt1.frame(1.0 / 60.0);
    assert_eq!(rt1.state.changes, [("a", true, true, false, Some("a".into()))]);
    assert_eq!(rt1.state.poll_hovered.as_deref(), Some("a"));
    assert!(!rt1.state.poll_over_b);
    // Second runtime on the same thread stays completely unaffected.
    assert_eq!(current_hovered(&mut rt2), None);
    assert!(!check_hover(&mut rt2, "a"));

    // Move directly from a to b: both handlers see the new hover target b.
    rt1.state.changes.clear();
    rt1.pointer_move(20.0, 60.0);
    rt1.frame(1.0 / 60.0);
    assert_eq!(
        rt1.state.changes,
        [
            ("a", false, false, true, Some("b".into())),
            ("b", true, false, true, Some("b".into())),
        ]
    );
    assert_eq!(rt1.state.poll_hovered.as_deref(), Some("b"));
    assert!(rt1.state.poll_over_b);
    assert!(!check_hover(&mut rt2, "b"));
}
