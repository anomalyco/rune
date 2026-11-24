use crate::{
    Cx, Direction, El, Env, GrowthSpec, Ime, Key, KeyEvent, Mods, Runtime, ScrollSpec, TextEdit,
    TextStyle, col, el, text_input,
};

fn lines_text(count: usize) -> String {
    (0..count).map(|i| format!("line {i}")).collect::<Vec<_>>().join("\n")
}

fn style() -> TextStyle {
    TextStyle::new(13.0, 400.0).lh(20.0)
}

fn key(rt: &mut Runtime<FocusState>, key: Key) {
    rt.key(KeyEvent { key, mods: Mods::default() });
    rt.frame(1.0 / 120.0);
}

struct FocusState {
    edit: TextEdit,
    reveal: bool,
    margin: f32,
    reduced_motion: bool,
    extra_below: f32,
}

fn focus_view(s: &FocusState, cx: &mut Cx<FocusState>) -> El {
    if s.reduced_motion {
        cx.reduced_motion(true);
    }
    let mut vp = col()
        .key("viewport")
        .w_full()
        .h(160.0)
        .scroll(ScrollSpec::default())
        .scroll_margin(s.margin);
    let mut field = text_input(&s.edit, style())
        .key("field")
        .label("Notes")
        .w_full()
        .on_edit(|s: &mut FocusState| &mut s.edit);
    if s.reveal {
        field = field.reveal_caret_on_focus(true);
    }
    vp = vp.child(field);
    if s.extra_below > 0.0 {
        vp = vp.child(el().w_full().h(s.extra_below));
    }
    col()
        .w_full()
        .h_full()
        .child(el().key("before").label("Before").size(120.0, 24.0).focusable(true))
        .child(vp)
}

fn scroll_y<S: 'static>(rt: &Runtime<S>, key: &str) -> f64 {
    let id = rt.tree.nodes.iter().find(|n| n.key.as_deref() == Some(key)).expect(key).id;
    rt.scroll.scrolls[&id].offset
}

fn scroll_x<S: 'static>(rt: &Runtime<S>, key: &str) -> f64 {
    let id = rt.tree.nodes.iter().find(|n| n.key.as_deref() == Some(key)).expect(key).id;
    rt.scroll.scrolls[&id].x.offset
}

fn caret_and_viewport<S: 'static>(rt: &Runtime<S>, vp_key: &str, field_key: &str) -> ([f32; 4], [f32; 4]) {
    let els = rt.elements();
    let vp = els.iter().find(|e| e.key.as_deref() == Some(vp_key)).expect(vp_key).rect;
    let caret = els
        .iter()
        .find(|e| e.key.as_deref() == Some(field_key))
        .expect(field_key)
        .caret
        .expect("focused caret");
    (vp, caret)
}

fn assert_caret_in_y(rt: &Runtime<FocusState>, margin: f32, step: &str) {
    let (vp, caret) = caret_and_viewport(rt, "viewport", "field");
    let top = vp[1];
    let bottom = vp[1] + vp[3];
    assert!(
        caret[1] >= top - 0.5 && caret[1] + caret[3] <= bottom + 0.5,
        "{step}: caret [{}, {}] outside viewport [{top}, {bottom}]",
        caret[1],
        caret[1] + caret[3]
    );
    if margin > 0.0 {
        let y = scroll_y(rt, "viewport");
        let max = {
            let id = rt.tree.nodes.iter().find(|n| n.key.as_deref() == Some("viewport")).unwrap().id;
            rt.scroll.scrolls[&id].max
        };
        if y > 0.5 && y < max - 0.5 {
            assert!(
                caret[1] >= top + margin - 0.5 && caret[1] + caret[3] <= bottom - margin + 0.5,
                "{step}: caret [{}, {}] did not clear margin {margin} inside [{top}, {bottom}]",
                caret[1],
                caret[1] + caret[3]
            );
        }
    }
}

#[test]
fn default_off_focus_gain_leaves_scroll_unchanged() {
    // Frame-0 model focus with default reveal_caret_on_focus(false) leaves scroll at 0.
    let mut initial = TextEdit::new(lines_text(40));
    initial.focused = true;
    let mut rt0 = Runtime::new(
        FocusState { edit: initial, reveal: false, margin: 0.0, reduced_motion: false, extra_below: 0.0 },
        focus_view,
        (300.0, 240.0),
        Env::new(true, 2.0),
    );
    rt0.frame(0.0);
    assert!(rt0.state.edit.focused);
    assert_eq!(scroll_y(&rt0, "viewport"), 0.0, "default-off frame-0 focus does not scroll");

    // Tab, Tx::focus, edit.focused = true, and AccessKit Focus all leave scroll at 0 by default.
    let mut rt = Runtime::new(
        FocusState { edit: TextEdit::new(lines_text(40)), reveal: false, margin: 0.0, reduced_motion: false, extra_below: 0.0 },
        focus_view,
        (300.0, 240.0),
        Env::new(true, 2.0),
    );
    rt.set_tab_visits_controls(true);
    rt.frame(0.0);
    key(&mut rt, Key::Tab);
    key(&mut rt, Key::Tab);
    assert!(rt.state.edit.focused);
    assert_eq!(scroll_y(&rt, "viewport"), 0.0, "default-off Tab does not jump to offscreen caret");

    rt.update(|_, tx| tx.blur());
    rt.frame(1.0 / 120.0);
    rt.update(|_, tx| tx.focus("field", false));
    rt.frame(1.0 / 120.0);
    assert!(rt.state.edit.focused);
    assert_eq!(scroll_y(&rt, "viewport"), 0.0, "default-off Tx::focus does not scroll");

    rt.update(|_, tx| tx.blur());
    rt.frame(1.0 / 120.0);
    rt.update(|s, _| s.edit.focused = true);
    rt.frame(1.0 / 120.0);
    assert!(rt.state.edit.focused);
    assert_eq!(scroll_y(&rt, "viewport"), 0.0, "default-off edit.focused = true does not scroll");

    rt.update(|_, tx| tx.blur());
    rt.frame(1.0 / 120.0);
    assert!(rt.accessibility_action("field", accesskit::Action::Focus, None));
    rt.frame(1.0 / 120.0);
    assert!(rt.state.edit.focused);
    assert_eq!(scroll_y(&rt, "viewport"), 0.0, "default-off AccessKit Focus does not scroll");
}

#[test]
fn tab_tx_focus_model_focus_and_a11y_reveal_offscreen_caret() {
    let mut rt = Runtime::new(
        FocusState { edit: TextEdit::new(lines_text(40)), reveal: true, margin: 16.0, reduced_motion: false, extra_below: 0.0 },
        focus_view,
        (300.0, 240.0),
        Env::new(true, 2.0),
    );
    rt.set_tab_visits_controls(true);
    rt.frame(0.0);
    assert!(!rt.state.edit.focused);
    assert_eq!(scroll_y(&rt, "viewport"), 0.0);

    // 1. Tab into the field brings caret at line 39 into view.
    key(&mut rt, Key::Tab);
    assert!(!rt.state.edit.focused);
    key(&mut rt, Key::Tab);
    assert!(rt.state.edit.focused);
    assert!(scroll_y(&rt, "viewport") > 500.0);
    assert_caret_in_y(&rt, 16.0, "Tab into field");

    // Accessibility tree reflects focused MultilineTextInput.
    let tree = rt.accessibility_tree();
    let (field_id, field_node) = tree
        .nodes
        .iter()
        .find(|(_, n)| n.author_id() == Some("field"))
        .expect("a11y field");
    assert_eq!(field_node.role(), accesskit::Role::MultilineTextInput);
    assert_eq!(tree.focus, *field_id);

    // 2. Tx::focus reveals offscreen caret in the middle (respecting scroll_margin).
    let mid = rt.state.edit.text.find("line 20").unwrap();
    rt.update(|s, tx| {
        s.edit.set_caret(mid);
        tx.blur();
        tx.scroll_to_y("viewport", 0.0);
    });
    rt.frame(1.0 / 120.0);
    assert_eq!(scroll_y(&rt, "viewport"), 0.0);
    rt.update(|_, tx| tx.focus("field", false));
    rt.frame(1.0 / 120.0);
    assert!(rt.state.edit.focused);
    assert_caret_in_y(&rt, 16.0, "Tx::focus mid-document");

    // 3. edit.focused = true reveals offscreen caret at top when viewport is at bottom.
    rt.update(|s, tx| {
        s.edit.set_caret(0);
        tx.blur();
        tx.scroll_to_y("viewport", 9999.0);
    });
    rt.frame(1.0 / 120.0);
    assert!(scroll_y(&rt, "viewport") > 500.0);
    rt.update(|s, _| s.edit.focused = true);
    rt.frame(1.0 / 120.0);
    assert!(rt.state.edit.focused);
    assert_eq!(scroll_y(&rt, "viewport"), 0.0, "caret at line 0 scrolls back to top");
    assert_caret_in_y(&rt, 16.0, "edit.focused = true");

    // 4. AccessKit Action::Focus reveals offscreen caret at end.
    let end = rt.state.edit.text.len();
    rt.update(|s, tx| {
        s.edit.set_caret(end);
        tx.blur();
        tx.scroll_to_y("viewport", 0.0);
    });
    rt.frame(1.0 / 120.0);
    assert_eq!(scroll_y(&rt, "viewport"), 0.0);
    assert!(rt.accessibility_action("field", accesskit::Action::Focus, None));
    rt.frame(1.0 / 120.0);
    assert!(rt.state.edit.focused);
    assert_caret_in_y(&rt, 16.0, "AccessKit Focus");

    // 5. Frame-0 initial edit.focused = true reveals caret immediately on frame 0.
    let mut initial = TextEdit::new(lines_text(40));
    initial.focused = true;
    let mut rt0 = Runtime::new(
        FocusState { edit: initial, reveal: true, margin: 0.0, reduced_motion: false, extra_below: 0.0 },
        focus_view,
        (300.0, 240.0),
        Env::new(true, 2.0),
    );
    rt0.frame(0.0);
    assert!(rt0.state.edit.focused);
    assert!(scroll_y(&rt0, "viewport") > 500.0);
    assert_caret_in_y(&rt0, 0.0, "frame-0 initial focus");
}

#[test]
fn already_visible_caret_scrolls_zero_and_focused_wheel_stays_put() {
    // Give extra space below the 40-line field so the viewport can sit scrolled near its bottom
    // with the field's top above the viewport and bottom inside it while the caret is visible.
    let mut edit = TextEdit::new(lines_text(40));
    let line36 = edit.text.find("line 36").unwrap();
    edit.set_caret(line36);
    let mut rt = Runtime::new(
        FocusState { edit, reveal: true, margin: 0.0, reduced_motion: false, extra_below: 200.0 },
        focus_view,
        (300.0, 240.0),
        Env::new(true, 2.0),
    );
    rt.set_tab_visits_controls(true);
    rt.frame(0.0);

    // Position viewport at y = 700 (shows [700, 860]): field spans [0, 800], caret at [720, 740] is visible.
    rt.update(|_, tx| tx.scroll_to_y("viewport", 700.0));
    rt.frame(1.0 / 120.0);
    assert_eq!(scroll_y(&rt, "viewport"), 700.0);

    // Tabbing into the field must change scroll offset by exactly 0.0 (not pull top into view).
    key(&mut rt, Key::Tab);
    key(&mut rt, Key::Tab);
    assert!(rt.state.edit.focused);
    assert_eq!(scroll_y(&rt, "viewport"), 700.0, "already-visible caret moves scroll by 0 on Tab");

    // Same for Tx::focus when already visible.
    rt.update(|_, tx| tx.blur());
    rt.frame(1.0 / 120.0);
    rt.update(|_, tx| tx.focus("field", false));
    rt.frame(1.0 / 120.0);
    assert!(rt.state.edit.focused);
    assert_eq!(scroll_y(&rt, "viewport"), 700.0, "already-visible caret moves scroll by 0 on Tx::focus");

    // Wheel-scroll away from the caret while focused: stays put across idle frames, rebuilds, and empty IME.
    rt.wheel(150.0, 100.0, 600.0);
    rt.frame(1.0 / 120.0);
    let scrolled = scroll_y(&rt, "viewport");
    assert!(scrolled < 400.0, "wheel moved viewport away from caret: {scrolled}");

    let commits = rt.stats.commits;
    for _ in 0..3 {
        rt.frame(1.0 / 120.0);
    }
    assert_eq!(scroll_y(&rt, "viewport"), scrolled, "idle frames do not yank scroll");
    assert_eq!(rt.stats.commits, commits, "idle frames do not recommit");
    assert!(!rt.animating, "settled idle window asks for no frames");

    rt.ime(Ime::Preedit(String::new(), None, Vec::new()));
    rt.frame(1.0 / 120.0);
    assert_eq!(scroll_y(&rt, "viewport"), scrolled, "empty IME preedit does not yank scroll");

    rt.ime(Ime::Commit(String::new()));
    rt.frame(1.0 / 120.0);
    assert_eq!(scroll_y(&rt, "viewport"), scrolled, "empty IME commit does not yank scroll");

    // View rebuild keeping focus on the same field leaves scroll untouched.
    rt.update(|s, _| s.margin = 0.0);
    rt.frame(1.0 / 120.0);
    assert_eq!(scroll_y(&rt, "viewport"), scrolled, "view rebuild while already focused does not yank scroll");
}

#[test]
fn edges_empty_huge_rtl_nested_reduced_motion_inert_and_virtual() {
    // 1. Empty field scrolled below viewport + reduced motion.
    let mut rt_empty = Runtime::new(
        TextEdit::new(""),
        |e: &TextEdit, cx: &mut Cx<TextEdit>| {
            cx.reduced_motion(true);
            col().w_full().h_full().child(
                col()
                    .key("vp")
                    .w_full()
                    .h(120.0)
                    .scroll(ScrollSpec::default())
                    .child(el().w_full().h(300.0))
                    .child(
                        text_input(e, style())
                            .key("empty")
                            .label("Empty field")
                            .w_full()
                            .reveal_caret_on_focus(true)
                            .on_edit(|s: &mut TextEdit| s),
                    ),
            )
        },
        (240.0, 160.0),
        Env::new(true, 1.0),
    );
    rt_empty.frame(0.0);
    assert_eq!(scroll_y(&rt_empty, "vp"), 0.0);
    rt_empty.update(|_, tx| tx.focus("empty", false));
    rt_empty.frame(1.0 / 120.0);
    assert!(rt_empty.state.focused);
    assert!(scroll_y(&rt_empty, "vp") >= 199.0);
    let (vp, caret) = caret_and_viewport(&rt_empty, "vp", "empty");
    assert!(caret[1] >= vp[1] - 0.5 && caret[1] + caret[3] <= vp[1] + vp[3] + 0.5);
    assert!(!rt_empty.animating, "reduced motion lands immediately");

    // 2. Huge 200-line field with caret at the end.
    let mut rt_huge = Runtime::new(
        FocusState { edit: TextEdit::new(lines_text(200)), reveal: true, margin: 0.0, reduced_motion: true, extra_below: 0.0 },
        focus_view,
        (300.0, 240.0),
        Env::new(true, 1.0),
    );
    rt_huge.set_tab_visits_controls(true);
    rt_huge.frame(0.0);
    key(&mut rt_huge, Key::Tab);
    key(&mut rt_huge, Key::Tab);
    assert!(rt_huge.state.edit.focused);
    let huge_y = scroll_y(&rt_huge, "viewport");
    assert!(huge_y > 3500.0);
    assert_caret_in_y(&rt_huge, 0.0, "huge 200-line field");
    let huge_commits = rt_huge.stats.commits;
    rt_huge.frame(1.0 / 120.0);
    assert_eq!(scroll_y(&rt_huge, "viewport"), huge_y);
    assert_eq!(rt_huge.stats.commits, huge_commits, "settled huge field does not recommit");
    assert!(!rt_huge.core().scrolling(), "focus caret reveal lands without a scroll glide");

    // 3. RTL unwrapped multi-line field in scroll_xy scrolls horizontally to offscreen caret.
    let rtl_line = "مرحبا بالعالم هذا سطر طويل جدا يتجاوز عرض نافذة التمرير بكثير لاختبار المؤشر";
    let rtl_edit = TextEdit::new(format!("{rtl_line}\n{rtl_line}"));
    let mut rt_rtl = Runtime::new(
        rtl_edit,
        |e: &TextEdit, _: &mut Cx<TextEdit>| {
            col().w_full().h_full().dir(Direction::Rtl).child(
                col()
                    .key("vp")
                    .size(160.0, 80.0)
                    .dir(Direction::Rtl)
                    .scroll_xy()
                    .child(
                        text_input(e, style())
                            .key("rtl")
                            .label("RTL field")
                            .dir(Direction::Rtl)
                            .nowrap()
                            .reveal_caret_on_focus(true)
                            .on_edit(|s: &mut TextEdit| s),
                    ),
            )
        },
        (200.0, 120.0),
        Env::new(true, 2.0),
    );
    rt_rtl.frame(0.0);
    assert_eq!(scroll_x(&rt_rtl, "vp"), 0.0);
    rt_rtl.update(|_, tx| tx.focus("rtl", false));
    rt_rtl.frame(1.0 / 120.0);
    assert!(rt_rtl.state.focused);
    assert!(scroll_x(&rt_rtl, "vp") > 10.0, "RTL horizontal scroll moved: {}", scroll_x(&rt_rtl, "vp"));
    let (vp, caret) = caret_and_viewport(&rt_rtl, "vp", "rtl");
    assert!(
        caret[0] >= vp[0] - 1.0 && caret[0] + caret[2] <= vp[0] + vp[2] + 2.0,
        "RTL caret {caret:?} inside viewport {vp:?}"
    );

    // 4. Nested scroll viewports: inner scrolled editor positioned below outer page viewport.
    let mut rt_nested = Runtime::new(
        TextEdit::new(lines_text(30)),
        |e: &TextEdit, _: &mut Cx<TextEdit>| {
            col()
                .key("outer")
                .size(280.0, 180.0)
                .scroll(ScrollSpec::default())
                .child(el().key("spacer").w_full().h(320.0))
                .child(
                    col()
                        .key("inner")
                        .w_full()
                        .h(100.0)
                        .scroll(ScrollSpec::default())
                        .child(
                            text_input(e, style())
                                .key("nested")
                                .label("Nested field")
                                .w_full()
                                .reveal_caret_on_focus(true)
                                .on_edit(|s: &mut TextEdit| s),
                        ),
                )
        },
        (280.0, 180.0),
        Env::new(true, 1.0),
    );
    rt_nested.frame(0.0);
    assert_eq!((scroll_y(&rt_nested, "outer"), scroll_y(&rt_nested, "inner")), (0.0, 0.0));
    rt_nested.update(|_, tx| tx.focus("nested", false));
    rt_nested.frame(1.0 / 120.0);
    assert!(rt_nested.state.focused);
    assert!(scroll_y(&rt_nested, "inner") > 400.0, "inner viewport scrolled to caret");
    assert!(scroll_y(&rt_nested, "outer") > 200.0, "outer viewport scrolled to inner caret");
    let (outer, caret) = caret_and_viewport(&rt_nested, "outer", "nested");
    let (inner, _) = caret_and_viewport(&rt_nested, "inner", "nested");
    assert!(caret[1] >= inner[1] - 0.5 && caret[1] + caret[3] <= inner[1] + inner[3] + 0.5);
    assert!(caret[1] >= outer[1] - 0.5 && caret[1] + caret[3] <= outer[1] + outer[3] + 0.5);

    // 5. Inert subtree cannot take focus or move scroll.
    let mut rt_inert = Runtime::new(
        TextEdit::new(lines_text(40)),
        |e: &TextEdit, _: &mut Cx<TextEdit>| {
            col().key("vp").size(240.0, 120.0).scroll(ScrollSpec::default()).child(
                col().inert().child(
                    text_input(e, style())
                        .key("blocked")
                        .label("Blocked field")
                        .w_full()
                        .reveal_caret_on_focus(true)
                        .on_edit(|s: &mut TextEdit| s),
                ),
            )
        },
        (240.0, 120.0),
        Env::new(true, 1.0),
    );
    rt_inert.frame(0.0);
    rt_inert.update(|s, tx| {
        s.focused = true;
        tx.focus("blocked", true);
    });
    rt_inert.frame(1.0 / 120.0);
    assert!(!rt_inert.state.focused, "inert field does not gain focus");
    assert_eq!(scroll_y(&rt_inert, "vp"), 0.0, "inert field does not scroll");

    // 6. Unmounted virtualized input with reveal_caret_on_focus(true) mounts and reveals its caret.
    let blocks: Vec<TextEdit> = (0..120).map(|i| TextEdit::new(format!("row {i}\nline b\nline c"))).collect();
    let mut rt_virt = Runtime::new(
        blocks,
        |s: &Vec<TextEdit>, _: &mut Cx<Vec<TextEdit>>| {
            col().key("vp").size(280.0, 160.0).scroll(ScrollSpec::default()).child(
                col()
                    .key("rows")
                    .w_full()
                    .growth(GrowthSpec::default())
                    .virtualize(60.0)
                    .children(s.iter().enumerate().map(|(i, e)| {
                        text_input(e, style())
                            .key(format!("r{i}"))
                            .label(format!("Row {i}"))
                            .w_full()
                            .reveal_caret_on_focus(true)
                            .on_edit(move |s: &mut Vec<TextEdit>| &mut s[i])
                    })),
            )
        },
        (280.0, 160.0),
        Env::new(true, 1.0),
    );
    rt_virt.frame(0.0);
    assert!(rt_virt.find("r95").is_none(), "r95 starts unmounted");
    rt_virt.update(|s, _| s[95].focused = true);
    rt_virt.frame(1.0 / 120.0);
    assert!(rt_virt.state[95].focused);
    let (vp, caret) = caret_and_viewport(&rt_virt, "vp", "r95");
    assert!(
        caret[1] >= vp[1] - 0.5 && caret[1] + caret[3] <= vp[1] + vp[3] + 0.5,
        "virtual input r95 caret {caret:?} inside viewport {vp:?}"
    );
}
