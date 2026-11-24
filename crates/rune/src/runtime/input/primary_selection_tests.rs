use crate::{ClipboardContent, Cx, El, Env, PointerButton, Runtime, ScrollSpec, TextEdit, TextStyle, col, el, row, text_input};

fn style() -> TextStyle {
    TextStyle::new(14.0, 400.0).lh(20.0)
}

fn rect_of<S: 'static>(rt: &Runtime<S>, key: &str) -> [f32; 4] {
    rt.elements().into_iter().find(|e| e.key.as_deref() == Some(key)).unwrap_or_else(|| panic!("missing element {key}")).rect
}

fn path_of<S: 'static>(rt: &Runtime<S>, key: &str) -> String {
    rt.elements().into_iter().find(|e| e.key.as_deref() == Some(key)).unwrap_or_else(|| panic!("missing element {key}")).path
}

fn linux_middle_click<S: 'static>(rt: &mut Runtime<S>, x: f32, y: f32) {
    {
        let (core, mut app) = rt.split_mut();
        core.button_down_with_primary(&mut app, x, y, PointerButton::Middle, true);
    }
    rt.button_up(x, y, PointerButton::Middle);
    rt.frame(1.0 / 60.0);
}

#[test]
fn middle_click_on_textbox_wrapper_padding_focuses_queues_paste_and_stops_ancestors() {
    struct State {
        a: TextEdit,
        b: TextEdit,
        middle_presses: usize,
        button_presses: usize,
    }
    fn view(s: &State, _: &mut Cx<State>) -> El {
        col().w_full().h_full().gap(12.0).padding(8.0)
            .child(
                row().key("middle-ancestor").w_full().padding(4.0)
                    .on_middle_press(|s: &mut State, _| s.middle_presses += 1)
                    .child(
                        row().key("box-a").role("textbox").w(260.0).h(44.0).px(20.0).py(10.0)
                            .child(text_input(&s.a, style()).single_line().key("input-a").w(180.0).on_edit(|s: &mut State| &mut s.a)),
                    ),
            )
            .child(
                row().key("button-ancestor").w_full().padding(4.0)
                    .pointer_buttons([PointerButton::Middle])
                    .on_press(|s: &mut State| s.button_presses += 1)
                    .child(
                        row().key("box-b").role("textbox").w(260.0).h(44.0).px(20.0).py(10.0)
                            .child(text_input(&s.b, style()).single_line().key("input-b").w(180.0).on_edit(|s: &mut State| &mut s.b)),
                    ),
            )
    }

    let mut rt = Runtime::new(
        State { a: TextEdit::new("hello"), b: TextEdit::new("world"), middle_presses: 0, button_presses: 0 },
        view,
        (360.0, 200.0),
        Env::new(true, 1.0),
    );
    rt.frame(0.0);

    // Middle-clicking the left padding of box-a focuses input-a at start, queues paste, and blocks on_middle_press.
    let box_a = rect_of(&rt, "box-a");
    let input_a_path = path_of(&rt, "input-a");
    linux_middle_click(&mut rt, box_a[0] + 4.0, box_a[1] + box_a[3] * 0.5);
    assert!(rt.state.a.focused, "middle-clicking wrapper left padding must focus inner input");
    assert_eq!(rt.state.a.range(), (0, 0), "left padding places caret at start");
    assert_eq!(rt.state.middle_presses, 0, "ancestor on_middle_press must not fire");
    let queued = rt.core_mut().edit.primary.paste.take();
    assert_eq!(queued.as_deref(), Some(input_a_path.as_str()), "primary paste must target the resolved inner input");

    // Delivering the queued primary selection pastes at the caret placed by the middle click.
    rt.paste_result(queued, ClipboardContent::new("say "));
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.state.a.text, "say hello");

    // Middle-clicking the right padding of box-b focuses input-b at end and blocks pointer_buttons([Middle]).
    let box_b = rect_of(&rt, "box-b");
    let input_b_path = path_of(&rt, "input-b");
    linux_middle_click(&mut rt, box_b[0] + box_b[2] - 4.0, box_b[1] + box_b[3] * 0.5);
    assert!(rt.state.b.focused, "middle-clicking wrapper right padding must focus inner input");
    assert_eq!(rt.state.b.range(), (5, 5), "right padding places caret at end");
    assert_eq!(rt.state.button_presses, 0, "ancestor pointer_buttons([Middle]) handler must not fire");
    assert_eq!(rt.core_mut().edit.primary.paste.take().as_deref(), Some(input_b_path.as_str()));
}

#[test]
fn middle_click_on_textbox_wrapper_padding_does_not_start_autoscroll() {
    struct State { edit: TextEdit }
    fn view(s: &State, _: &mut Cx<State>) -> El {
        col().key("scroller").size(300.0, 200.0).scroll(ScrollSpec { stick_bottom: false })
            .child(
                col().w_full().h(1000.0).padding(12.0)
                    .child(
                        row().key("box").role("textbox").w(240.0).h(44.0).px(20.0).py(10.0)
                            .child(text_input(&s.edit, style()).single_line().key("input").w(160.0).on_edit(|s: &mut State| &mut s.edit)),
                    ),
            )
    }

    let mut rt = Runtime::new(State { edit: TextEdit::new("notes") }, view, (300.0, 200.0), Env::new(true, 1.0));
    rt.frame(0.0);
    let scroller_id = rt.tree.nodes.iter().find(|n| n.key.as_deref() == Some("scroller")).unwrap().id;
    let box_r = rect_of(&rt, "box");
    let input_path = path_of(&rt, "input");

    {
        let (core, mut app) = rt.split_mut();
        core.button_down_with_primary(&mut app, box_r[0] + 4.0, box_r[1] + 4.0, PointerButton::Middle, true);
    }
    rt.pointer_move(box_r[0] + 4.0, box_r[1] + 120.0);
    rt.frame(1.0 / 60.0);
    rt.frame(1.0 / 60.0);
    rt.button_up(box_r[0] + 4.0, box_r[1] + 120.0, PointerButton::Middle);
    rt.frame(1.0 / 60.0);

    assert!(rt.core().autoscroll_cursor().is_none(), "middle-clicking wrapper padding must not start autoscroll");
    assert_eq!(rt.scroll.scrolls[&scroller_id].offset, 0.0, "scroll viewport must stay at 0");
    assert!(rt.state.edit.focused);
    assert_eq!(rt.core_mut().edit.primary.paste.take().as_deref(), Some(input_path.as_str()));
}

#[test]
fn secure_textbox_wrapper_never_takes_or_gives_primary_selection() {
    struct State {
        masked: TextEdit,
        revealed: TextEdit,
        wrapped_secure: TextEdit,
        plain: TextEdit,
    }
    fn view(s: &State, _: &mut Cx<State>) -> El {
        col().w_full().h_full().gap(8.0).padding(8.0)
            .child(
                row().key("masked-box").role("textbox").w(240.0).h(40.0).px(16.0).py(8.0)
                    .child(text_input(&s.masked, style()).single_line().key("masked").w(160.0).on_edit(|s: &mut State| &mut s.masked)),
            )
            .child(
                row().key("revealed-box").role("textbox").w(240.0).h(40.0).px(16.0).py(8.0)
                    .child(text_input(&s.revealed, style()).single_line().key("revealed").w(160.0).on_edit(|s: &mut State| &mut s.revealed)),
            )
            .child(
                row().key("secure-wrap").role("textbox").secure().w(240.0).h(40.0).px(16.0).py(8.0)
                    .child(text_input(&s.wrapped_secure, style()).single_line().key("wrapped").w(160.0).on_edit(|s: &mut State| &mut s.wrapped_secure)),
            )
            .child(
                row().key("plain-box").role("textbox").w(240.0).h(40.0).px(16.0).py(8.0)
                    .child(text_input(&s.plain, style()).single_line().key("plain").w(160.0).on_edit(|s: &mut State| &mut s.plain)),
            )
    }

    let mut rt = Runtime::new(
        State {
            masked: TextEdit::new("hunter2").secure(),
            revealed: TextEdit::new("topsecret").secure().revealed(true),
            wrapped_secure: TextEdit::new("wrappedpw"),
            plain: TextEdit::new("public"),
        },
        view,
        (300.0, 240.0),
        Env::new(true, 1.0),
    );
    rt.frame(0.0);

    // None of the secure wrappers accept primary paste on padding or input middle-click.
    for (box_key, inp_key) in [("masked-box", "masked"), ("revealed-box", "revealed"), ("secure-wrap", "wrapped")] {
        let br = rect_of(&rt, box_key);
        let ir = rect_of(&rt, inp_key);
        for (x, y) in [(br[0] + 4.0, br[1] + 4.0), (ir[0] + 10.0, ir[1] + ir[3] * 0.5)] {
            linux_middle_click(&mut rt, x, y);
            assert!(rt.core().edit.primary.paste.is_none(), "{box_key} must never queue primary paste");
        }
    }
    assert!(!rt.state.masked.focused && !rt.state.revealed.focused && !rt.state.wrapped_secure.focused);

    // Selecting text inside any secure field never publishes it to the primary selection.
    let note_release = |rt: &mut Runtime<State>| {
        rt.core_mut().primary_released();
        let (core, app) = rt.split_mut();
        core.note_primary_selection(&app);
    };
    for (box_key, select_fn) in [
        ("masked-box", (|s: &mut State| s.masked.set_selection(0, s.masked.text.len())) as fn(&mut State)),
        ("revealed-box", |s: &mut State| s.revealed.set_selection(0, s.revealed.text.len())),
        ("secure-wrap", |s: &mut State| s.wrapped_secure.set_selection(0, s.wrapped_secure.text.len())),
    ] {
        let br = rect_of(&rt, box_key);
        rt.pointer_down(br[0] + 4.0, br[1] + 4.0);
        rt.pointer_up(br[0] + 4.0, br[1] + 4.0);
        rt.frame(1.0 / 60.0);
        rt.update(|s, _| select_fn(s));
        rt.frame(1.0 / 60.0);
        note_release(&mut rt);
        assert!(rt.core().edit.primary.write.is_none(), "{box_key} must never write to primary selection");
    }

    // By contrast, selecting in the plain field writes to primary selection, and middle-clicking its padding queues paste.
    let pr = rect_of(&rt, "plain-box");
    rt.pointer_down(pr[0] + 4.0, pr[1] + 4.0);
    rt.pointer_up(pr[0] + 4.0, pr[1] + 4.0);
    rt.frame(1.0 / 60.0);
    rt.update(|s, _| s.plain.set_selection(0, s.plain.text.len()));
    rt.frame(1.0 / 60.0);
    note_release(&mut rt);
    assert_eq!(rt.core_mut().edit.primary.write.take().as_deref(), Some("public"));
    linux_middle_click(&mut rt, pr[0] + 4.0, pr[1] + 4.0);
    assert_eq!(rt.core_mut().edit.primary.paste.take(), Some(path_of(&rt, "plain")));
}

#[test]
fn readonly_and_disabled_textbox_wrappers_never_paste_or_focus() {
    struct State {
        ro_plain: TextEdit,
        ro_focusable: TextEdit,
        ro_flag_input: TextEdit,
        ro_flag_wrap: TextEdit,
        disabled_kit: TextEdit,
        disabled_input: TextEdit,
        disabled_wrap: TextEdit,
    }
    fn view(s: &State, _: &mut Cx<State>) -> El {
        col().w_full().h_full().gap(6.0).padding(6.0)
            .child(
                row().key("ro-plain").role("textbox").w(240.0).h(32.0).px(16.0).py(6.0)
                    .child(text_input(&s.ro_plain, style()).single_line().w(160.0)),
            )
            .child(
                row().key("ro-focusable").role("textbox").w(240.0).h(32.0).px(16.0).py(6.0)
                    .child(text_input(&s.ro_focusable, style()).single_line().focusable(true).w(160.0)),
            )
            .child(
                row().key("ro-flag-input").role("textbox").w(240.0).h(32.0).px(16.0).py(6.0)
                    .child(text_input(&s.ro_flag_input, style()).single_line().key("ro-flag-input-inner").read_only(true).w(160.0).on_edit(|s: &mut State| &mut s.ro_flag_input)),
            )
            .child(
                row().key("ro-flag-wrap").role("textbox").read_only(true).w(240.0).h(32.0).px(16.0).py(6.0)
                    .child(text_input(&s.ro_flag_wrap, style()).single_line().key("ro-flag-wrap-inner").w(160.0).on_edit(|s: &mut State| &mut s.ro_flag_wrap)),
            )
            .child(
                row().key("disabled-kit").role("textbox").w(240.0).h(32.0).px(16.0).py(6.0)
                    .child(text_input(&s.disabled_kit, style()).single_line().disabled(true).w(160.0)),
            )
            .child(
                row().key("disabled-input").role("textbox").w(240.0).h(32.0).px(16.0).py(6.0)
                    .child(text_input(&s.disabled_input, style()).single_line().key("disabled-input-inner").disabled(true).w(160.0).on_edit(|s: &mut State| &mut s.disabled_input)),
            )
            .child(
                row().key("disabled-wrap").role("textbox").disabled(true).w(240.0).h(32.0).px(16.0).py(6.0)
                    .child(text_input(&s.disabled_wrap, style()).single_line().key("disabled-wrap-inner").w(160.0).on_edit(|s: &mut State| &mut s.disabled_wrap)),
            )
    }

    let mut rt = Runtime::new(
        State {
            ro_plain: TextEdit::new("a"),
            ro_focusable: TextEdit::new("b"),
            ro_flag_input: TextEdit::new("c"),
            ro_flag_wrap: TextEdit::new("d"),
            disabled_kit: TextEdit::new("e"),
            disabled_input: TextEdit::new("f"),
            disabled_wrap: TextEdit::new("g"),
        },
        view,
        (300.0, 320.0),
        Env::new(true, 1.0),
    );
    rt.frame(0.0);

    for key in [
        "ro-plain",
        "ro-focusable",
        "ro-flag-input",
        "ro-flag-input-inner",
        "ro-flag-wrap",
        "ro-flag-wrap-inner",
        "disabled-kit",
        "disabled-input",
        "disabled-input-inner",
        "disabled-wrap",
        "disabled-wrap-inner",
    ] {
        let r = rect_of(&rt, key);
        linux_middle_click(&mut rt, r[0] + 4.0, r[1] + 4.0);
        assert!(rt.core().edit.primary.paste.is_none(), "{key} must not queue primary paste");
    }
    assert!(
        !rt.state.ro_plain.focused
            && !rt.state.ro_focusable.focused
            && !rt.state.ro_flag_input.focused
            && !rt.state.ro_flag_wrap.focused
            && !rt.state.disabled_kit.focused
            && !rt.state.disabled_input.focused
            && !rt.state.disabled_wrap.focused,
        "read-only and disabled fields must not focus on middle-click"
    );
}

#[test]
fn middle_click_on_nested_button_or_ambiguous_textbox_does_not_paste() {
    struct State {
        single: TextEdit,
        first: TextEdit,
        second: TextEdit,
        btn_middle: usize,
    }
    fn view(s: &State, _: &mut Cx<State>) -> El {
        col().w_full().h_full().gap(12.0).padding(8.0)
            .child(
                row().key("with-btn").role("textbox").w(260.0).h(40.0).px(12.0).gap(8.0)
                    .child(text_input(&s.single, style()).single_line().key("single").w(160.0).on_edit(|s: &mut State| &mut s.single))
                    .child(
                        el().key("btn").role("button").hit().size(28.0, 28.0)
                            .on_middle_press(|s: &mut State, _| s.btn_middle += 1),
                    ),
            )
            .child(
                row().key("ambiguous").role("textbox").w(320.0).h(44.0).px(16.0).gap(8.0)
                    .child(text_input(&s.first, style()).single_line().key("first").w(100.0).on_edit(|s: &mut State| &mut s.first))
                    .child(text_input(&s.second, style()).single_line().key("second").w(100.0).on_edit(|s: &mut State| &mut s.second)),
            )
    }

    let mut rt = Runtime::new(
        State { single: TextEdit::new("one"), first: TextEdit::new("a"), second: TextEdit::new("b"), btn_middle: 0 },
        view,
        (360.0, 160.0),
        Env::new(true, 1.0),
    );
    rt.frame(0.0);

    // Middle-clicking a button inside a textbox wrapper reaches the button, not primary paste.
    let btn = rect_of(&rt, "btn");
    linux_middle_click(&mut rt, btn[0] + btn[2] * 0.5, btn[1] + btn[3] * 0.5);
    assert_eq!(rt.state.btn_middle, 1, "button inside textbox keeps its own middle press");
    assert!(!rt.state.single.focused);
    assert!(rt.core().edit.primary.paste.is_none());

    // Middle-clicking padding of a wrapper with two reachable editors pastes into neither.
    let amb = rect_of(&rt, "ambiguous");
    linux_middle_click(&mut rt, amb[0] + 4.0, amb[1] + 4.0);
    assert!(!rt.state.first.focused && !rt.state.second.focused);
    assert!(rt.core().edit.primary.paste.is_none());
}
