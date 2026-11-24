use rune::{Cx, El, Env, PointerPress, Runtime, el};
use rune_kit::{Kit, MenuItem, context_menu, root};

#[derive(Default)]
struct State { point: Option<PointerPress> }

fn view(s: &State, _: &mut Cx<State>) -> El {
    let k = Kit::neutral(true);
    let trigger = el().size(300.0, 200.0).on_secondary_press(|s: &mut State, p, _| s.point = Some(p));
    let mut layer = Vec::new();
    let body = context_menu("context", trigger, s.point)
        .item(MenuItem::new("Copy").on_select(|s: &mut State| s.point = None))
        .on_close(|s: &mut State| s.point = None)
        .build(&k, &mut layer);
    root(body, layer)
}

#[test]
fn context_menu_opens_at_pointer_and_stays_inside_viewport() {
    let mut rt = Runtime::new(State::default(), view, (300.0, 200.0), Env::new(true, 1.0));
    rt.frame(0.0);
    rt.secondary_down(290.0, 190.0);
    rt.secondary_up(290.0, 190.0);
    rt.frame(0.0);
    for _ in 0..120 { rt.frame(1.0 / 120.0); }
    let menu = rt.elements().into_iter().find(|e| e.key.as_deref() == Some("context-menu")).unwrap();
    assert!(menu.rect[0] >= 0.0 && menu.rect[1] >= 0.0);
    assert!(menu.rect[0] + menu.rect[2] <= 300.01);
    assert!(menu.rect[1] + menu.rect[3] <= 200.01);
    let (x, y) = rt.find("context-item0").unwrap();
    rt.pointer_down(x, y);
    rt.pointer_up(x, y);
    rt.frame(0.0);
    assert!(rt.state.point.is_none());
}

mod keyboard {
    use rune::{Cx, El, Env, Key, KeyEvent, Mods, PointerPress, Runtime, col, el};
    use rune_kit::{Kit, MenuItem, context_menu, root};

    #[derive(Default)]
    struct State { point: Option<PointerPress>, row_keys: bool }

    fn view(s: &State, _: &mut Cx<State>) -> El {
        let k = Kit::neutral(true);
        let row = el().key("row").size(200.0, 40.0).focusable(true)
            .on_key(|s: &mut State, ev, _| s.row_keys && ev.key == Key::Function(10))
            .on_secondary_press(|s: &mut State, p, _| s.point = Some(p));
        let mut layer = Vec::new();
        let body = context_menu("context", row, s.point)
            .item(MenuItem::new("Copy").on_select(|s: &mut State| s.point = None))
            .on_close(|s: &mut State| s.point = None)
            .build(&k, &mut layer);
        root(col().padding(20.0).child(body).child(el().key("other").size(200.0, 40.0).focusable(true)), layer)
    }

    fn settle(rt: &mut Runtime<State>) {
        rt.frame(0.0);
        for _ in 0..60 { rt.frame(1.0 / 60.0); }
    }

    fn press(rt: &mut Runtime<State>, key: Key, shift: bool) {
        rt.key(KeyEvent { key, mods: Mods { shift, ..Mods::default() } });
        settle(rt);
    }

    fn focused(rt: &Runtime<State>) -> Option<String> {
        rt.elements().into_iter().find(|e| e.focused).and_then(|e| e.key)
    }

    fn menu_open(rt: &Runtime<State>) -> bool {
        rt.elements().iter().any(|e| e.key.as_deref() == Some("context-menu"))
    }

    fn focused_row() -> Runtime<State> {
        let mut rt = Runtime::new(State::default(), view, (400.0, 300.0), Env::new(true, 1.0));
        settle(&mut rt);
        press(&mut rt, Key::Tab, false);
        assert_eq!(focused(&rt).as_deref(), Some("row"));
        rt
    }

    #[test]
    fn shift_f10_opens_the_focused_rows_menu_and_escape_returns_focus() {
        let mut rt = focused_row();
        press(&mut rt, Key::Function(10), true);
        let p = rt.state.point.expect("the row's menu opened");
        let row = rt.elements().into_iter().find(|e| e.key.as_deref() == Some("row")).unwrap();
        assert_eq!((p.x, p.y), (row.rect[0], row.rect[1] + row.rect[3]), "at the row's bottom-left");
        assert!(menu_open(&rt));
        press(&mut rt, Key::Escape, false);
        assert!(!menu_open(&rt) && rt.state.point.is_none());
        assert_eq!(focused(&rt).as_deref(), Some("row"), "focus returns to the row");

        press(&mut rt, Key::ContextMenu, false);
        assert!(rt.state.point.is_some() && menu_open(&rt), "the Menu key opens it too");
    }

    #[test]
    fn a_key_handler_that_takes_shift_f10_keeps_it_and_nothing_focused_is_a_no_op() {
        let mut rt = focused_row();
        rt.state.row_keys = true;
        press(&mut rt, Key::Function(10), true);
        assert!(rt.state.point.is_none());
        rt.state.row_keys = false;
        press(&mut rt, Key::Function(10), false);
        assert!(rt.state.point.is_none(), "F10 without Shift");

        let mut rt = Runtime::new(State::default(), view, (400.0, 300.0), Env::new(true, 1.0));
        settle(&mut rt);
        press(&mut rt, Key::ContextMenu, false);
        assert!(rt.state.point.is_none() && focused(&rt).is_none());
    }
}

mod outside_secondary {
    use rune::{Cx, El, Env, PointerPress, Runtime, col, el};
    use rune_kit::{Dialog, Kit, MenuItem, Popover, PopupItem, PopupList, Select, context_menu, root};

    fn settle<S: 'static>(rt: &mut Runtime<S>) {
        rt.frame(0.0);
        for _ in 0..30 { rt.frame(0.016); }
    }

    #[test]
    fn outside_secondary_press_on_context_menu_dismisses_and_follows_os_repost_policy() {
        #[derive(Default)]
        struct S { point: Option<PointerPress>, on: Option<&'static str>, closes: usize }
        fn row(key: &'static str, y: f32) -> El {
            el().key(key).absolute(0.0, y).size(400.0, 40.0)
                .on_secondary_press(move |s: &mut S, p, _| { s.point = Some(p); s.on = Some(key); })
        }
        fn view(s: &S, _: &mut Cx<S>) -> El {
            let k = Kit::neutral(true);
            let mut layer = Vec::new();
            let body = col().w_full().h_full().child(
                context_menu("menu", el().w_full().h_full().child(row("a", 0.0)).child(row("b", 200.0)), s.point)
                    .item(MenuItem::new("Open"))
                    .on_close(|s: &mut S| { s.point = None; s.closes += 1; })
                    .build(&k, &mut layer),
            );
            root(body, layer)
        }

        // Same-frame secondary down + up on row B while row A's menu is open.
        let mut rt = Runtime::new(S::default(), view, (400.0, 300.0), Env::new(true, 1.0));
        rt.frame(0.0);
        rt.secondary_down(20.0, 20.0);
        rt.secondary_up(20.0, 20.0);
        settle(&mut rt);
        assert!(rt.state.point.is_some() && rt.state.on == Some("a"));

        rt.secondary_down(20.0, 220.0);
        rt.secondary_up(20.0, 220.0);
        rt.frame(0.016);
        assert_eq!(rt.state.closes, 1, "outside secondary press must dismiss row A's menu");
        if cfg!(target_os = "windows") {
            assert!(rt.state.point.is_some(), "Windows reposts outside secondary press to row B");
            assert_eq!(rt.state.on, Some("b"));
        } else {
            assert!(rt.state.point.is_none(), "non-Windows consumes outside secondary press");
            assert_eq!(rt.state.on, Some("a"));
        }

        // Cross-frame secondary down (frame) + secondary up on row B while row A's menu is open.
        let mut rt = Runtime::new(S::default(), view, (400.0, 300.0), Env::new(true, 1.0));
        rt.frame(0.0);
        rt.secondary_down(20.0, 20.0);
        rt.secondary_up(20.0, 20.0);
        settle(&mut rt);
        assert!(rt.state.point.is_some() && rt.state.on == Some("a"));

        rt.secondary_down(20.0, 220.0);
        rt.frame(0.016);
        assert_eq!(rt.state.closes, 1, "dismiss commits on down");
        assert!(rt.state.point.is_none());
        rt.secondary_up(20.0, 220.0);
        rt.frame(0.016);
        if cfg!(target_os = "windows") {
            assert!(rt.state.point.is_some(), "Windows cross-frame release opens row B's menu");
            assert_eq!(rt.state.on, Some("b"));
        } else {
            assert!(rt.state.point.is_none(), "non-Windows cross-frame release stays consumed");
            assert_eq!(rt.state.on, Some("a"));
        }
    }

    #[test]
    fn outside_secondary_press_on_dialog_and_select_never_reaches_background() {
        #[derive(Clone, Copy, Debug, PartialEq, Eq)]
        enum Kind { DialogOutside, DialogLocked, DialogNoClose, Select }
        struct S { kind: Kind, open: bool, closes: usize, bg_secondary: usize }
        fn view(s: &S, _: &mut Cx<S>) -> El {
            let k = Kit::neutral(true);
            let mut layer = Vec::new();
            let bg = el().key("bg").absolute(0.0, 0.0).size(400.0, 300.0)
                .on_secondary_press(|s: &mut S, _, _| s.bg_secondary += 1);
            let body = match s.kind {
                Kind::DialogOutside => {
                    Dialog::new("dlg", "Confirm").open(s.open).width(160.0).fit().click_outside(true)
                        .on_close(|s: &mut S| { s.open = false; s.closes += 1; })
                        .build(&k, &mut layer);
                    bg
                }
                Kind::DialogLocked => {
                    Dialog::new("dlg", "Confirm").open(s.open).width(160.0).fit().click_outside(false)
                        .on_close(|s: &mut S| { s.open = false; s.closes += 1; })
                        .build(&k, &mut layer);
                    bg
                }
                Kind::DialogNoClose => {
                    Dialog::new("dlg", "Confirm").open(s.open).width(160.0).fit()
                        .build(&k, &mut layer);
                    bg
                }
                Kind::Select => {
                    let sel = Select::new("sel", &["One", "Two"])
                        .open(s.open)
                        .on_close(|s: &mut S| { s.open = false; s.closes += 1; })
                        .build(&k, &mut layer);
                    el().w_full().h_full().child(bg).child(el().absolute(120.0, 100.0).child(sel))
                }
            };
            root(body, layer)
        }

        for (kind, should_close) in [
            (Kind::DialogOutside, true),
            (Kind::DialogLocked, false),
            (Kind::DialogNoClose, false),
            (Kind::Select, true),
        ] {
            let mut rt = Runtime::new(S { kind, open: true, closes: 0, bg_secondary: 0 }, view, (400.0, 300.0), Env::new(true, 1.0));
            settle(&mut rt);
            rt.secondary_down(10.0, 10.0);
            rt.secondary_up(10.0, 10.0);
            settle(&mut rt);
            assert_eq!(rt.state.open, !should_close, "open state after outside secondary press for {kind:?}");
            assert_eq!(rt.state.closes, usize::from(should_close));
            assert_eq!(rt.state.bg_secondary, 0, "background must never receive secondary press for {kind:?}");
        }
    }

    #[test]
    fn outside_secondary_press_on_menu_nested_in_dialog_respects_outer_dialog_inertness() {
        struct S { dialog: bool, menu: Option<PointerPress>, menu_on: Option<&'static str>, menu_closes: usize, bg_secondary: usize }
        fn view(s: &S, _: &mut Cx<S>) -> El {
            let k = Kit::neutral(true);
            let mut layer = Vec::new();
            let bg = el().key("bg").absolute(0.0, 0.0).size(400.0, 300.0)
                .on_secondary_press(|s: &mut S, _, _| s.bg_secondary += 1);
            let inside = col().w_full().gap(8.0)
                .child(el().key("row-a").size(140.0, 32.0).on_secondary_press(|s: &mut S, p, _| { s.menu = Some(p); s.menu_on = Some("row-a"); }))
                .child(el().key("row-b").size(140.0, 32.0).on_secondary_press(|s: &mut S, p, _| { s.menu = Some(p); s.menu_on = Some("row-b"); }));
            Dialog::new("dlg", "Nested").open(s.dialog).width(200.0).fit().click_outside(false)
                .on_close(|s: &mut S| s.dialog = false)
                .child(inside)
                .build(&k, &mut layer);
            // Menu mounts after Dialog in the overlay layer so it is the active modal while open.
            let _ = context_menu("nested", el().size(0.0, 0.0), s.menu)
                .item(MenuItem::new("Action"))
                .on_close(|s: &mut S| { s.menu = None; s.menu_closes += 1; })
                .build(&k, &mut layer);
            root(bg, layer)
        }

        let mut rt = Runtime::new(
            S { dialog: true, menu: None, menu_on: None, menu_closes: 0, bg_secondary: 0 },
            view,
            (400.0, 300.0),
            Env::new(true, 1.0),
        );
        settle(&mut rt);

        // Open menu from row-a inside the dialog.
        let (ax, ay) = rt.find("row-a").unwrap();
        rt.secondary_down(ax, ay);
        rt.secondary_up(ax, ay);
        settle(&mut rt);
        assert!(rt.state.menu.is_some() && rt.state.menu_on == Some("row-a"));

        // Right-clicking outside the dialog dismisses the menu without piercing the dialog's inert background.
        rt.secondary_down(5.0, 5.0);
        rt.secondary_up(5.0, 5.0);
        settle(&mut rt);
        assert!(rt.state.menu.is_none(), "menu dismissed");
        assert!(rt.state.dialog, "outer dialog stays open");
        assert_eq!(rt.state.menu_closes, 1);
        assert_eq!(rt.state.bg_secondary, 0, "outer dialog keeps background inert even on Windows");

        // Re-open on row-a, then right-click row-b INSIDE the dialog.
        rt.secondary_down(ax, ay);
        rt.secondary_up(ax, ay);
        settle(&mut rt);
        let (bx, by) = rt.find("row-b").unwrap();
        rt.secondary_down(bx, by);
        rt.secondary_up(bx, by);
        settle(&mut rt);
        assert!(rt.state.dialog);
        assert_eq!(rt.state.menu_closes, 2);
        if cfg!(target_os = "windows") {
            assert!(rt.state.menu.is_some() && rt.state.menu_on == Some("row-b"), "Windows reposts to sibling row inside outer dialog");
        } else {
            assert!(rt.state.menu.is_none());
        }
    }

    #[test]
    fn outside_secondary_press_on_non_modal_scrim_dismisses_and_consumes() {
        #[derive(Clone, Copy)]
        enum Kind { Popover, PopupList }
        struct S { kind: Kind, open: bool, closes: usize, bg_secondary: usize }
        fn view(s: &S, _: &mut Cx<S>) -> El {
            let k = Kit::neutral(true);
            let mut layer = Vec::new();
            let bg = el().key("bg").absolute(0.0, 0.0).size(400.0, 300.0)
                .on_secondary_press(|s: &mut S, _, _| s.bg_secondary += 1);
            let body = match s.kind {
                Kind::Popover => {
                    let pop = Popover::new("pop")
                        .trigger(el().key("trig").size(60.0, 24.0))
                        .title("Info")
                        .open(s.open)
                        .on_close(|s: &mut S| { s.open = false; s.closes += 1; })
                        .build(&k, &mut layer);
                    el().w_full().h_full().child(bg).child(el().absolute(120.0, 100.0).child(pop))
                }
                Kind::PopupList => {
                    PopupList::new("list", "anchor")
                        .open(s.open)
                        .items(vec![PopupItem::new("one", "One")])
                        .on_close(|s: &mut S| { s.open = false; s.closes += 1; })
                        .build(&k, &mut layer);
                    el().w_full().h_full().child(bg).child(el().key("anchor").absolute(120.0, 200.0).size(80.0, 24.0))
                }
            };
            root(body, layer)
        }

        for kind in [Kind::Popover, Kind::PopupList] {
            let mut rt = Runtime::new(S { kind, open: true, closes: 0, bg_secondary: 0 }, view, (400.0, 300.0), Env::new(true, 1.0));
            settle(&mut rt);
            rt.secondary_down(10.0, 10.0);
            rt.secondary_up(10.0, 10.0);
            settle(&mut rt);
            assert!(!rt.state.open, "non-modal scrim must dismiss on outside secondary press");
            assert_eq!(rt.state.closes, 1);
            assert_eq!(rt.state.bg_secondary, 0, "non-modal scrim must consume outside secondary press on every OS");
        }
    }

    #[test]
    fn context_menu_item_opening_dialog_in_same_commit_returns_focus_to_opener() {
        use rune::{Key, KeyEvent, Mods, TextEdit, TextStyle, text_input};
        use rune_kit::Button;

        #[derive(Clone, Copy, Debug, PartialEq, Eq)]
        enum DialogKind { Default, FocusedField, NoOnClose }
        struct S { kind: DialogKind, point: Option<PointerPress>, dialog: bool, field: TextEdit }

        fn view(s: &S, _: &mut Cx<S>) -> El {
            let k = Kit::neutral(true);
            let mut layer = Vec::new();
            let row = el().key("row").size(200.0, 40.0).focusable(true)
                .on_secondary_press(|s: &mut S, p, _| s.point = Some(p));
            let body = context_menu("context", row, s.point)
                .item(MenuItem::new("Customize").on_select(|s: &mut S| {
                    s.point = None;
                    s.dialog = true;
                    if s.kind == DialogKind::FocusedField { s.field.focused = true; }
                }))
                .on_close(|s: &mut S| s.point = None)
                .build(&k, &mut layer);
            let mut dlg = Dialog::new("custom", "Customize").fit().open(s.dialog);
            if s.kind != DialogKind::NoOnClose {
                dlg = dlg.on_close(|s: &mut S| { s.dialog = false; s.field.focused = false; });
            }
            if s.kind == DialogKind::FocusedField {
                dlg = dlg.child(text_input(&s.field, TextStyle::new(14.0, 400.0)).key("dlg-input").size(180.0, 28.0)
                    .on_edit(|s: &mut S| &mut s.field));
            }
            dlg = dlg.child(Button::new("Done").on_press(|s: &mut S| { s.dialog = false; s.field.focused = false; }).build(&k).key("done"));
            dlg.build(&k, &mut layer);
            root(col().padding(20.0).child(body).child(el().key("other").size(200.0, 40.0).focusable(true)), layer)
        }

        let press = |rt: &mut Runtime<S>, key: Key, shift: bool| {
            rt.key(KeyEvent { key, mods: Mods { shift, ..Mods::default() } });
            settle(rt);
        };
        let focused = |rt: &Runtime<S>| rt.elements().into_iter().find(|e| e.focused).and_then(|e| e.key);

        for kind in [DialogKind::Default, DialogKind::FocusedField, DialogKind::NoOnClose] {
            let mut rt = Runtime::new(
                S { kind, point: None, dialog: false, field: TextEdit::new("") },
                view,
                (500.0, 400.0),
                Env::new(true, 1.0),
            );
            rt.set_full_keyboard_access(true);
            settle(&mut rt);

            press(&mut rt, Key::Tab, false);
            assert_eq!(focused(&rt).as_deref(), Some("row"));
            press(&mut rt, Key::Function(10), true);
            assert!(rt.state.point.is_some());
            press(&mut rt, Key::Down, false);
            press(&mut rt, Key::Enter, false);
            assert!(rt.state.point.is_none() && rt.state.dialog);
            if kind == DialogKind::FocusedField {
                assert_eq!(focused(&rt).as_deref(), Some("dlg-input"));
            }

            if kind == DialogKind::NoOnClose {
                press(&mut rt, Key::Text(" ".into()), false);
            } else {
                press(&mut rt, Key::Escape, false);
            }
            assert!(!rt.state.dialog);
            assert_eq!(
                focused(&rt).as_deref(),
                Some("row"),
                "closing dialog opened from context menu must return focus to original row ({kind:?})"
            );
        }
    }
}
