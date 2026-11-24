use super::*;
use crate::{Key, KeyEvent, Link, Mods, TextEdit, TextStyle, Transition, col, el, row, text, text_input};
use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

fn tab<S: 'static>(rt: &mut Runtime<S>, shift: bool) {
    rt.key(KeyEvent { key: Key::Tab, mods: Mods { shift, ..Mods::default() } });
    rt.frame(1.0 / 60.0);
}

fn click<S: 'static>(rt: &mut Runtime<S>, x: f32, y: f32) {
    rt.pointer_down(x, y);
    rt.pointer_up(x, y);
    rt.frame(1.0 / 60.0);
}

fn center<S: 'static>(rt: &Runtime<S>, key: &str) -> (f32, f32) {
    rt.find(key).unwrap_or_else(|| panic!("missing element {key:?}"))
}

#[test]
fn keyboard_tab_arrows_and_hierarchy_fire_focus_and_within_with_focus_visible() {
    #[derive(Default)]
    struct S {
        log: Vec<(&'static str, bool, bool)>,
    }

    fn view(_: &S, _: &mut Cx<S>) -> El {
        col().w_full().h_full().padding(8.0).gap(6.0)
            .child(
                el().key("before").size(80.0, 28.0).role("button").label("Before").focusable(true)
                    .on_focus_change(|s: &mut S, f, tx| s.log.push(("before", f, tx.focus_visible()))),
            )
            .child(
                col().key("card").size(220.0, 80.0).gap(4.0).focusable(true)
                    .on_focus_change(|s: &mut S, f, tx| s.log.push(("card-self", f, tx.focus_visible())))
                    .on_focus_within_change(|s: &mut S, w, tx| s.log.push(("card-within", w, tx.focus_visible())))
                    .child(
                        el().key("cancel").size(80.0, 24.0).role("button").label("Cancel").focusable(true)
                            .on_focus_change(|s: &mut S, f, tx| s.log.push(("cancel", f, tx.focus_visible()))),
                    )
                    .child(
                        el().key("ok").size(80.0, 24.0).role("button").label("OK").focusable(true)
                            .on_focus_change(|s: &mut S, f, tx| s.log.push(("ok", f, tx.focus_visible()))),
                    ),
            )
            .child(
                row().key("tabs").role("tablist").size(200.0, 30.0).focus_group()
                    .on_focus_within_change(|s: &mut S, w, tx| s.log.push(("tabs-within", w, tx.focus_visible())))
                    .child(
                        el().key("t1").size(80.0, 26.0).role("tab").label("One").focusable(true).focus_selected(true)
                            .on_focus_change(|s: &mut S, f, tx| s.log.push(("t1", f, tx.focus_visible()))),
                    )
                    .child(
                        el().key("t2").size(80.0, 26.0).role("tab").label("Two").focusable(true)
                            .on_focus_change(|s: &mut S, f, tx| s.log.push(("t2", f, tx.focus_visible()))),
                    ),
            )
    }

    let mut rt = Runtime::new(S::default(), view, (320.0, 260.0), Env::new(true, 1.0));
    rt.frame(0.0);
    assert!(rt.state.log.is_empty(), "initial unfocused mount fires nothing: {:?}", rt.state.log);

    // Tab 1: before
    tab(&mut rt, false);
    assert_eq!(rt.state.log, vec![("before", true, true)]);
    rt.state.log.clear();

    // Tab 2: card itself (leaves before, enters card-self and card-within)
    tab(&mut rt, false);
    assert_eq!(rt.state.log, vec![
        ("before", false, true),
        ("card-self", true, true),
        ("card-within", true, true),
    ]);
    rt.state.log.clear();

    // Tab 3: card -> cancel (card-self loses focus, cancel gains; card-within stays true!)
    tab(&mut rt, false);
    assert_eq!(rt.state.log, vec![
        ("card-self", false, true),
        ("cancel", true, true),
    ]);
    rt.state.log.clear();

    // Tab 4: cancel -> ok (siblings inside card; card-within does not re-fire)
    tab(&mut rt, false);
    assert_eq!(rt.state.log, vec![
        ("cancel", false, true),
        ("ok", true, true),
    ]);
    rt.state.log.clear();

    // Shift+Tab back to cancel, then Shift+Tab to card itself (card-within still does not re-fire)
    tab(&mut rt, true);
    tab(&mut rt, true);
    assert_eq!(rt.state.log, vec![
        ("ok", false, true),
        ("cancel", true, true),
        ("cancel", false, true),
        ("card-self", true, true),
    ]);
    rt.state.log.clear();

    // Jump into focus_group tablist (card-self and card-within both leave; t1 and tabs-within enter)
    tab(&mut rt, false);
    tab(&mut rt, false);
    tab(&mut rt, false);
    rt.state.log.clear();
    // Focus is now on t1; Right arrow moves to t2 inside tabs without firing tabs-within.
    rt.key(KeyEvent { key: Key::Right, mods: Mods::default() });
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.state.log, vec![
        ("t1", false, true),
        ("t2", true, true),
    ]);
}

#[test]
fn pointer_press_and_blur_report_actual_focus_with_focus_visible_false() {
    #[derive(Default)]
    struct S {
        log: Vec<(&'static str, bool, bool)>,
    }

    fn view(_: &S, _: &mut Cx<S>) -> El {
        col().w_full().h_full().padding(12.0).gap(10.0)
            .child(
                col().key("panel").size(160.0, 44.0)
                    .on_focus_within_change(|s: &mut S, w, tx| s.log.push(("panel", w, tx.focus_visible())))
                    .child(
                        el().key("a").size(120.0, 32.0).role("button").label("A").focusable(true)
                            .on_focus_change(|s: &mut S, f, tx| s.log.push(("a", f, tx.focus_visible()))),
                    ),
            )
            .child(
                el().key("b").size(120.0, 32.0).role("button").label("B").focusable(true)
                    .on_focus_change(|s: &mut S, f, tx| s.log.push(("b", f, tx.focus_visible()))),
            )
    }

    let mut rt = Runtime::new(S::default(), view, (300.0, 200.0), Env::new(true, 1.0));
    rt.frame(0.0);

    let (ax, ay) = center(&rt, "a");
    click(&mut rt, ax, ay);
    assert_eq!(rt.state.log, vec![("a", true, false), ("panel", true, false)]);
    rt.state.log.clear();

    // Clicking b moves focus: leaves fire before enters, modality stays pointer (false).
    let (bx, by) = center(&rt, "b");
    click(&mut rt, bx, by);
    assert_eq!(rt.state.log, vec![
        ("a", false, false),
        ("panel", false, false),
        ("b", true, false),
    ]);
    rt.state.log.clear();

    // Clicking empty space blurs b.
    click(&mut rt, 280.0, 180.0);
    assert_eq!(rt.state.log, vec![("b", false, false)]);
}

#[test]
fn text_field_blur_fires_same_frame_with_edit_unfocused() {
    struct S {
        edit: TextEdit,
        log: Vec<(&'static str, bool, bool, bool)>,
    }

    fn view(s: &S, _: &mut Cx<S>) -> El {
        col().w_full().h_full().padding(12.0).gap(10.0)
            .child(
                col().key("wrap").size(220.0, 40.0)
                    .on_focus_within_change(|s: &mut S, w, tx| {
                        s.log.push(("wrap", w, s.edit.focused, tx.focus_visible()));
                    })
                    .child(
                        text_input(&s.edit, TextStyle::new(14.0, 400.0))
                            .key("field")
                            .size(200.0, 32.0)
                            .on_edit(|s: &mut S| &mut s.edit)
                            .on_focus_change(|s: &mut S, f, tx| {
                                s.log.push(("field", f, s.edit.focused, tx.focus_visible()));
                            }),
                    ),
            )
            .child(el().key("other").size(80.0, 28.0).role("button").label("Other").focusable(true))
    }

    // Initial mount with TextEdit::focused == true fires on frame 0.
    let mut edit = TextEdit::new("hello");
    edit.focused = true;
    let mut rt = Runtime::new(S { edit, log: Vec::new() }, view, (300.0, 160.0), Env::new(true, 1.0));
    rt.frame(0.0);
    assert_eq!(rt.state.log, vec![("field", true, true, false), ("wrap", true, true, false)]);
    rt.state.log.clear();

    // Escape blurs the text field and notifies in the SAME frame with edit.focused == false.
    rt.key(KeyEvent { key: Key::Escape, mods: Mods::default() });
    rt.frame(1.0 / 60.0);
    assert!(!rt.state.edit.focused);
    assert_eq!(rt.state.log, vec![("field", false, false, false), ("wrap", false, false, false)]);
    rt.state.log.clear();

    // Pointer click focuses the field, then click outside blurs it in the same frame.
    let (fx, fy) = center(&rt, "field");
    click(&mut rt, fx, fy);
    assert_eq!(rt.state.log, vec![("field", true, true, false), ("wrap", true, true, false)]);
    rt.state.log.clear();

    click(&mut rt, 280.0, 140.0);
    assert!(!rt.state.edit.focused);
    assert_eq!(rt.state.log, vec![("field", false, false, false), ("wrap", false, false, false)]);
}

#[test]
fn assistive_technology_focus_fires_with_focus_visible_and_preserves_a11y_tree() {
    #[derive(Default)]
    struct S {
        log: Vec<(&'static str, bool, bool)>,
    }

    fn view(_: &S, _: &mut Cx<S>) -> El {
        col().w_full().h_full().padding(10.0)
            .child(
                col().key("observer").size(180.0, 60.0).label("Observer")
                    .on_focus_change(|s: &mut S, f, tx| s.log.push(("obs-self", f, tx.focus_visible())))
                    .on_focus_within_change(|s: &mut S, w, tx| s.log.push(("obs-within", w, tx.focus_visible())))
                    .child(
                        el().key("btn").size(100.0, 32.0).role("button").label("Action").focusable(true)
                            .on_focus_change(|s: &mut S, f, tx| s.log.push(("btn", f, tx.focus_visible()))),
                    ),
            )
    }

    let mut rt = Runtime::new(S::default(), view, (240.0, 120.0), Env::new(true, 1.0));
    rt.frame(0.0);

    // Non-focusable observer must not advertise Focus action in the accessibility tree.
    let tree = rt.accessibility_tree();
    let obs = &tree.nodes.iter().find(|(_, n)| n.author_id() == Some("observer")).unwrap().1;
    assert!(!obs.supports_action(accesskit::Action::Focus), "observer container must stay non-focusable in a11y tree");

    assert!(rt.accessibility_action("btn", accesskit::Action::Focus, None));
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.state.log, vec![("btn", true, true), ("obs-within", true, true)]);
}

#[test]
fn programmatic_focus_blur_and_focus_when_shown() {
    #[derive(Default)]
    struct S {
        show_late: bool,
        log: Vec<(&'static str, bool, bool)>,
    }

    fn view(s: &S, _: &mut Cx<S>) -> El {
        let mut root = col().w_full().h_full().padding(10.0).gap(8.0)
            .child(
                el().key("a").size(100.0, 28.0).role("button").label("A").focusable(true)
                    .on_focus_change(|s: &mut S, f, tx| s.log.push(("a", f, tx.focus_visible()))),
            )
            .child(
                el().key("b").size(100.0, 28.0).role("button").label("B").focusable(true)
                    .on_focus_change(|s: &mut S, f, tx| s.log.push(("b", f, tx.focus_visible()))),
            );
        if s.show_late {
            root = root.child(
                el().key("late").size(100.0, 28.0).role("button").label("Late").focusable(true)
                    .on_focus_change(|s: &mut S, f, tx| s.log.push(("late", f, tx.focus_visible()))),
            );
        }
        root
    }

    let mut rt = Runtime::new(S::default(), view, (240.0, 160.0), Env::new(true, 1.0));
    rt.frame(0.0);

    rt.update(|_, tx| tx.focus("a", true));
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.state.log, vec![("a", true, true)]);
    rt.state.log.clear();

    rt.update(|_, tx| tx.focus("b", false));
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.state.log, vec![("a", false, false), ("b", true, false)]);
    rt.state.log.clear();

    rt.update(|_, tx| tx.blur());
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.state.log, vec![("b", false, false)]);
    rt.state.log.clear();

    rt.update(|_, tx| tx.focus_when_shown("late", true));
    rt.frame(1.0 / 60.0);
    assert!(rt.state.log.is_empty());

    rt.update(|s, _| s.show_late = true);
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.state.log, vec![("late", true, true)]);
}

#[test]
fn modal_autofocus_open_and_close_update_view_same_frame() {
    #[derive(Default)]
    struct S {
        open: bool,
        opener_focused: bool,
        confirm_focused: bool,
        log: Vec<(&'static str, bool)>,
    }

    fn view(s: &S, _: &mut Cx<S>) -> El {
        let mut root = col().w_full().h_full().padding(10.0).gap(8.0)
            .child(
                el().key("opener").size(120.0, 28.0).role("button").label("Open").focusable(true)
                    .on_focus_change(|s: &mut S, f, _| {
                        s.opener_focused = f;
                        s.log.push(("opener", f));
                    })
                    .child(text(if s.opener_focused { "opener-on" } else { "opener-off" }).key("opener-lbl")),
            );
        if s.open {
            root = root.child(
                col().key("dialog").size(200.0, 100.0)
                    .modal(true, |s: &mut S| s.open = false)
                    .on_focus_within_change(|s: &mut S, w, _| s.log.push(("dialog-within", w)))
                    .child(
                        el().key("confirm").size(100.0, 28.0).role("button").label("Confirm").focusable(true).autofocus()
                            .on_focus_change(|s: &mut S, f, _| {
                                s.confirm_focused = f;
                                s.log.push(("confirm", f));
                            })
                            .on_press(|s: &mut S| s.open = false)
                            .child(text(if s.confirm_focused { "confirm-on" } else { "confirm-off" }).key("confirm-lbl")),
                    ),
            );
        }
        root
    }

    let mut rt = Runtime::new(S::default(), view, (320.0, 200.0), Env::new(true, 1.0));
    rt.frame(0.0);

    tab(&mut rt, false);
    assert!(rt.state.opener_focused);
    rt.state.log.clear();

    // Open modal: opener loses focus, autofocus confirm gains focus, and view updates same frame.
    rt.update(|s, _| s.open = true);
    rt.frame(1.0 / 60.0);
    assert!(!rt.state.opener_focused);
    assert!(rt.state.confirm_focused);
    assert_eq!(rt.state.log, vec![
        ("opener", false),
        ("confirm", true),
        ("dialog-within", true),
    ]);
    let confirm_text = rt.elements().into_iter().find(|e| e.key.as_deref() == Some("confirm-lbl")).and_then(|e| e.text);
    assert_eq!(confirm_text.as_deref(), Some("confirm-on"), "view rebuilt in same frame as modal autofocus");
    rt.state.log.clear();

    // Close modal via its button: confirm and dialog leave, opener regains focus in same frame.
    let (cx, cy) = center(&rt, "confirm");
    click(&mut rt, cx, cy);
    assert!(rt.state.opener_focused);
    assert!(!rt.state.confirm_focused);
    assert_eq!(rt.state.log, vec![
        ("confirm", false),
        ("dialog-within", false),
        ("opener", true),
    ]);
    let opener_text = rt.elements().into_iter().find(|e| e.key.as_deref() == Some("opener-lbl")).and_then(|e| e.text);
    assert_eq!(opener_text.as_deref(), Some("opener-on"), "view rebuilt in same frame as modal restore");
}

#[test]
fn unmount_transition_and_disable_while_focused_fire_false() {
    #[derive(Clone, Copy, PartialEq, Eq)]
    enum Mode { Mounted, Unmounted, DisabledDropHandler, Inert }

    struct S {
        mode: Mode,
        with_transition: bool,
        log: Vec<(&'static str, bool)>,
    }

    fn view(s: &S, _: &mut Cx<S>) -> El {
        let mut container = col().key("box").size(200.0, 80.0)
            .on_focus_within_change(|s: &mut S, w, _| s.log.push(("box-within", w)));
        match s.mode {
            Mode::Mounted => {
                let mut btn = el().key("target").size(100.0, 28.0).role("button").label("Go").focusable(true)
                    .on_focus_change(|s: &mut S, f, _| s.log.push(("target", f)));
                if s.with_transition { btn = btn.transition(Transition::opacity()); }
                container = container.child(btn);
            }
            Mode::Unmounted => {}
            Mode::DisabledDropHandler => {
                // Disabled AND no longer attaches on_focus_change; old handler must still hear false.
                container = container.child(el().key("target").size(100.0, 28.0).role("button").label("Go").focusable(false));
            }
            Mode::Inert => {
                container = container.child(
                    el().key("target").size(100.0, 28.0).role("button").label("Go").focusable(true).inert()
                        .on_focus_change(|s: &mut S, f, _| s.log.push(("target", f))),
                );
            }
        }
        col().w_full().h_full().padding(10.0).child(container)
    }

    for (next_mode, with_transition) in [
        (Mode::Unmounted, false),
        (Mode::Unmounted, true),
        (Mode::DisabledDropHandler, false),
        (Mode::Inert, false),
    ] {
        let mut rt = Runtime::new(
            S { mode: Mode::Mounted, with_transition, log: Vec::new() },
            view,
            (260.0, 140.0),
            Env::new(true, 1.0),
        );
        rt.frame(0.0);
        tab(&mut rt, false);
        assert_eq!(rt.state.log, vec![("target", true), ("box-within", true)]);
        rt.state.log.clear();

        rt.update(|s, _| s.mode = next_mode);
        rt.frame(1.0 / 60.0);
        assert_eq!(rt.state.log, vec![("target", false), ("box-within", false)], "transition={with_transition}");
        // Settling an exit transition must not double-fire.
        rt.frame(1.0);
        assert_eq!(rt.state.log.len(), 2, "no duplicate leave after exit completes");
    }
}

#[test]
fn same_frame_blur_then_refocus_fires_sequentially_without_coalescing() {
    #[derive(Default)]
    struct S {
        self_log: Vec<bool>,
        within_log: Vec<bool>,
    }

    fn view(_: &S, _: &mut Cx<S>) -> El {
        col().w_full().h_full().padding(10.0)
            .child(
                col().key("host").size(140.0, 60.0)
                    .on_focus_within_change(|s: &mut S, w, _| s.within_log.push(w))
                    .child(
                        el().key("tree").size(120.0, 40.0).role("tree").label("Files").focusable(true)
                            .on_focus_change(|s: &mut S, f, _| s.self_log.push(f)),
                    ),
            )
    }

    let mut rt = Runtime::new(S::default(), view, (300.0, 200.0), Env::new(true, 1.0));
    rt.frame(0.0);
    tab(&mut rt, false);
    assert_eq!(rt.state.self_log, vec![true]);
    assert_eq!(rt.state.within_log, vec![true]);
    rt.state.self_log.clear();
    rt.state.within_log.clear();

    // Queue a pointer blur on empty space AND Shift+Tab back before a single frame.
    rt.pointer_down(280.0, 180.0);
    rt.pointer_up(280.0, 180.0);
    rt.key(KeyEvent { key: Key::Tab, mods: Mods { shift: true, ..Mods::default() } });
    rt.frame(1.0 / 60.0);

    assert_eq!(rt.state.self_log, vec![false, true], "leave and re-enter must not coalesce");
    assert_eq!(rt.state.within_log, vec![false, true], "within leave and re-enter must not coalesce");
}

#[test]
fn focus_callbacks_do_not_create_pointer_hit_target() {
    #[derive(Default)]
    struct S {
        pressed: usize,
        focus_events: usize,
    }

    fn view(_: &S, _: &mut Cx<S>) -> El {
        el().w_full().h_full()
            .child(
                el().key("under").absolute(20.0, 20.0).size(120.0, 40.0)
                    .role("button").label("Under")
                    .on_press(|s: &mut S| s.pressed += 1),
            )
            // Overlapping non-interactive container with both focus callbacks: must NOT steal hits.
            .child(
                el().key("overlay").absolute(0.0, 0.0).size(200.0, 100.0)
                    .on_focus_change(|s: &mut S, _, _| s.focus_events += 1)
                    .on_focus_within_change(|s: &mut S, _, _| s.focus_events += 1),
            )
    }

    let mut rt = Runtime::new(S::default(), view, (240.0, 120.0), Env::new(true, 1.0));
    rt.frame(0.0);

    click(&mut rt, 60.0, 40.0);
    assert_eq!(rt.state.pressed, 1, "click passes straight through focus-observer overlay");
    assert_eq!(rt.state.focus_events, 0);
}

#[test]
fn tx_focus_within_handles_duplicate_keys_unkeyed_descendants_escaped_keys_and_links() {
    struct S {
        poll_pop: bool,
        poll_page: bool,
        last_cb_within_pop: bool,
    }

    fn btn(key: Option<&'static str>, label: &'static str) -> El {
        let b = el().role("button").label(label).focusable(true).size(140.0, 22.0)
            .on_focus_change(|s: &mut S, _, tx| s.last_cb_within_pop = tx.focus_within("popover"));
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
                    .child(btn(Some("close"), "Close page"))
                    .child(btn(None, "Unkeyed on page")),
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
                        .focusable(true)
                        .link(Link::new(0..9, "docs-link")),
                ),
            )
    }

    fn poll_focus(s: &mut S, _: f64, tx: &mut Tx) -> bool {
        s.poll_pop = tx.focus_within("popover");
        s.poll_page = tx.focus_within("page");
        false
    }
    let mut rt = Runtime::new(
        S { poll_pop: false, poll_page: false, last_cb_within_pop: false },
        view,
        (300.0, 260.0),
        Env::new(true, 1.0),
    );
    rt.poll = Some(poll_focus);
    rt.frame(0.0);

    let check = |rt: &mut Runtime<S>, sel: &str| {
        let mut hit = false;
        rt.update(|_, tx| hit = tx.focus_within(sel));
        hit
    };

    assert!(!check(&mut rt, "popover"));
    assert!(!check(&mut rt, ""));

    // 1. popover/close
    tab(&mut rt, false);
    assert!(check(&mut rt, "popover"));
    assert!(check(&mut rt, "close"));
    assert!(check(&mut rt, "popover/close"));
    assert!(!check(&mut rt, "page"));
    assert!(!check(&mut rt, "page/close"));
    assert!(!check(&mut rt, ""));
    assert!(rt.state.last_cb_within_pop);

    // 2. unkeyed button in popover
    tab(&mut rt, false);
    assert!(check(&mut rt, "popover"));
    assert!(!check(&mut rt, "page"));
    assert!(!check(&mut rt, "popover/close"));
    assert!(rt.state.last_cb_within_pop);

    // 3. page/close (same leaf key "close", different ancestor!)
    tab(&mut rt, false);
    assert!(!check(&mut rt, "popover"));
    assert!(!check(&mut rt, "popover/close"));
    assert!(check(&mut rt, "page"));
    assert!(check(&mut rt, "close"));
    assert!(check(&mut rt, "page/close"));
    assert!(!rt.state.last_cb_within_pop);

    // Verify poll reads Tx::focus_within too.
    rt.frame(1.0 / 60.0);
    assert!(!rt.state.poll_pop);
    assert!(rt.state.poll_page);

    // 4. unkeyed button on page
    tab(&mut rt, false);
    assert!(!check(&mut rt, "popover"));
    assert!(check(&mut rt, "page"));

    // 5. escaped key containing '/'
    tab(&mut rt, false);
    assert!(check(&mut rt, "list"));
    assert!(check(&mut rt, "row/0"));
    assert!(check(&mut rt, "list/row/0"));
    assert!(check(&mut rt, "list/row/0/edit"));
    assert!(!check(&mut rt, "popover/edit"));

    // 6. focused inline link inside article/body
    tab(&mut rt, false);
    assert!(check(&mut rt, "article"));
    assert!(check(&mut rt, "article/body"));
    assert!(check(&mut rt, "docs-link"));
    assert!(check(&mut rt, "body/docs-link"));
    assert!(check(&mut rt, "article/body/docs-link"));
    assert!(!check(&mut rt, "page/docs-link"));
}

#[test]
fn any_variants_live_only_wake_after_and_idle_zero_work() {
    let seen = Rc::new(RefCell::new(Vec::<(&'static str, bool)>::new()));
    let seen_a = seen.clone();
    let seen_w = seen.clone();

    struct S {
        seen_a: Rc<RefCell<Vec<(&'static str, bool)>>>,
        seen_w: Rc<RefCell<Vec<(&'static str, bool)>>>,
    }

    fn view(s: &S, _: &mut Cx<S>) -> El {
        let sa = s.seen_a.clone();
        let sw = s.seen_w.clone();
        col().w_full().h_full().padding(10.0)
            .child(
                col().key("box").size(160.0, 50.0)
                    .on_focus_within_change_any(move |within, tx| {
                        sw.borrow_mut().push(("within", within));
                        if !within { tx.wake_after(Duration::from_millis(50)); }
                        tx.live_only();
                    })
                    .child(
                        el().key("btn").size(100.0, 30.0).role("button").label("Btn").focusable(true)
                            .on_focus_change_any(move |focused, tx| {
                                sa.borrow_mut().push(("self", focused));
                                tx.live_only();
                            }),
                    ),
            )
    }

    let mut rt = Runtime::new(S { seen_a, seen_w }, view, (240.0, 120.0), Env::new(true, 1.0));
    rt.frame(0.0);
    let commits_after_mount = rt.stats.commits;

    // Programmatic focus with live_only callbacks commits once (for the focus change) and does not trigger an extra re-measure.
    rt.update(|_, tx| tx.focus("btn", true));
    rt.frame(1.0 / 60.0);
    assert_eq!(*seen.borrow(), vec![("self", true), ("within", true)]);
    assert_eq!(rt.stats.commits, commits_after_mount + 1);
    seen.borrow_mut().clear();

    // Idle frame after focus settles performs zero commits and does not animate.
    let commits_before_idle = rt.stats.commits;
    assert!(!rt.frame(1.0 / 60.0));
    assert_eq!(rt.stats.commits, commits_before_idle);

    // Blur schedules wake_after from on_focus_within_change_any(false).
    rt.update(|_, tx| tx.blur());
    rt.frame(1.0 / 60.0);
    assert_eq!(*seen.borrow(), vec![("self", false), ("within", false)]);
    assert!(rt.wake_deadline().is_some(), "wake_after from on_focus_within_change_any must be scheduled immediately");
}
