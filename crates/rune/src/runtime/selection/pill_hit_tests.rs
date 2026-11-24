use super::*;
use crate::{Cx, El, Env, Runtime, TextEdit, TextStyle, col, text_input};

struct PillDoc {
    edit: TextEdit,
    single_line: bool,
    menus: Vec<String>,
}

fn make_pill_doc(pill_count: usize, single_line: bool) -> PillDoc {
    let mut edit = TextEdit::new("").rich();
    for i in 0..pill_count {
        if i > 0 {
            if !single_line && i == 25 {
                edit.insert("\n\n");
            } else {
                edit.insert(" ");
            }
        }
        let (id, label) = if !single_line && i == 15 {
            ("wrapped".to_owned(), "long_wrapped_pill_phrase across_two_lines".to_owned())
        } else {
            (format!("p{i}"), format!("pill{i:02}"))
        };
        edit.insert_pill(id, &label);
    }
    edit.set_caret(0);
    PillDoc { edit, single_line, menus: Vec::new() }
}

fn pill_doc_view(s: &PillDoc, _: &mut Cx<PillDoc>) -> El {
    let mut field = text_input(&s.edit, TextStyle::new(14.0, 400.0).lh(20.0))
        .key("field")
        .label("Pills")
        .on_edit(|s: &mut PillDoc| &mut s.edit)
        .on_pill_secondary(|s: &mut PillDoc, id, _, _| {
            s.menus.push(id.to_owned());
            true
        });
    field = if s.single_line {
        field.single_line().size(180.0, 28.0)
    } else {
        field.w_full().h(500.0)
    };
    col().w(180.0).h(520.0).child(field)
}

fn settle_pill_doc(rt: &mut Runtime<PillDoc>) {
    for _ in 0..60 {
        if !rt.frame(1.0 / 60.0) {
            return;
        }
    }
    panic!("never settled");
}

fn field_node_index(rt: &Runtime<PillDoc>) -> usize {
    rt.tree.nodes.iter().position(|n| n.input.is_some()).expect("input node")
}

fn reference_input_pill_at(core: &RuntimeCore, i: usize, x: f32, y: f32) -> Option<String> {
    let n = &core.tree.nodes[i];
    let input = n.input.as_deref()?;
    if input.edit.is_secure() || input.edit.preedit().is_some() { return None; }
    let pills = input.edit.display_pills();
    if pills.is_empty() { return None; }
    let (rect, ux, uy) = core.unturned_box_and_point(i, x, y);
    let z = core.paint.drawn_scale.get(i).copied().unwrap_or(1.0);
    let scroll_x = if input.single_line { core.edit.input_scroll.get(&n.path).copied().unwrap_or(0.0) * z } else { 0.0 };
    core.span_boxes(n, rect, z, pills.iter().map(|&(s, e, _)| (s, e)))
        .into_iter()
        .find(|(_, r)| contains([r[0] - scroll_x, r[1], r[2] - scroll_x, r[3]], ux, uy))
        .map(|(k, _)| pills[k].2.to_owned())
}

fn all_pill_boxes(core: &RuntimeCore, i: usize) -> Vec<(usize, [f32; 4])> {
    let n = &core.tree.nodes[i];
    let input = n.input.as_deref().unwrap();
    let pills = input.edit.display_pills();
    let z = core.paint.drawn_scale.get(i).copied().unwrap_or(1.0);
    let scroll_x = if input.single_line { core.edit.input_scroll.get(&n.path).copied().unwrap_or(0.0) * z } else { 0.0 };
    core.span_boxes(n, core.drawn_box(i), z, pills.iter().map(|&(s, e, _)| (s, e)))
        .into_iter()
        .map(|(k, r)| (k, [r[0] - scroll_x, r[1], r[2] - scroll_x, r[3]]))
        .collect()
}

#[test]
fn input_pill_at_lays_out_only_candidate_pill_and_matches_full_scan() {
    let mut rt = Runtime::new(make_pill_doc(40, false), pill_doc_view, (180.0, 520.0), Env::new(true, 1.0));
    rt.frame(0.0);
    settle_pill_doc(&mut rt);

    let i = field_node_index(&rt);
    let drawn_boxes = all_pill_boxes(rt.core(), i);
    let pill_boxes_of = |k: usize| -> Vec<[f32; 4]> {
        drawn_boxes.iter().filter(|(j, _)| *j == k).map(|&(_, r)| r).collect()
    };
    let mid = |r: [f32; 4]| ((r[0] + r[2]) * 0.5, (r[1] + r[3]) * 0.5);

    let wrapped_rects = pill_boxes_of(15);
    assert!(wrapped_rects.len() >= 2, "expected wrapped pill on 2+ lines, got {}", wrapped_rects.len());

    let hits = [
        ("first", "p0", mid(pill_boxes_of(0)[0])),
        ("middle", "p10", mid(pill_boxes_of(10)[0])),
        ("wrapped line 1", "wrapped", mid(wrapped_rects[0])),
        ("wrapped line 2", "wrapped", mid(wrapped_rects[1])),
        ("after paragraph break", "p25", mid(pill_boxes_of(25)[0])),
        ("last", "p39", mid(pill_boxes_of(39)[0])),
    ];
    for (label, want, (x, y)) in hits {
        let (got, queries) = count_span_box_queries(|| rt.core().input_pill_at(i, x, y));
        assert_eq!(got.as_deref(), Some(want), "{label}: input_pill_at mismatch");
        assert!(queries <= 1, "{label}: expected <= 1 span-box query, got {queries}");
    }

    // Points over no pill: space between pill 0 and pill 1, right margin, above/below text, NaN.
    let (r0, r1) = (pill_boxes_of(0)[0], pill_boxes_of(1)[0]);
    assert!((r0[1] - r1[1]).abs() < 0.01 && r1[0] > r0[2] + 1.0, "pill 0 and 1 should sit on one line with a space");
    let misses = [
        ("space between pills", ((r0[2] + r1[0]) * 0.5, (r0[1] + r0[3]) * 0.5)),
        ("right margin", (175.0, (r0[1] + r0[3]) * 0.5)),
        ("above text", (r0[0] + 2.0, r0[1] - 5.0)),
        ("below text", (20.0, 510.0)),
        ("nan", (f32::NAN, r0[1])),
    ];
    for (label, (x, y)) in misses {
        let (got, queries) = count_span_box_queries(|| rt.core().input_pill_at(i, x, y));
        assert_eq!(got, None, "{label}: expected miss");
        assert_eq!(queries, 0, "{label}: miss should query 0 spans, got {queries}");
    }

    // Exhaustive equivalence against full span_boxes scan across every pill's interior and edges.
    for &(k, r) in &drawn_boxes {
        let cy = (r[1] + r[3]) * 0.5;
        let cx = (r[0] + r[2]) * 0.5;
        for (px, py) in [
            (cx, cy),
            (r[0], r[1]),
            (r[0] - 0.1, cy),
            (r[2] - 0.01, r[3] - 0.01),
            (r[2], cy),
            (cx, r[3]),
        ] {
            let want = reference_input_pill_at(rt.core(), i, px, py);
            let (got, queries) = count_span_box_queries(|| rt.core().input_pill_at(i, px, py));
            assert_eq!(got, want, "pill {k} point ({px}, {py}) diverged from full scan");
            assert!(queries <= 2, "pill {k} point ({px}, {py}) queried {queries} spans");
        }
    }

    // Secondary press path: secondary_claim on down and field_menu on up stay bounded.
    let (p39_x, p39_y) = mid(pill_boxes_of(39)[0]);
    rt.pointer_move(p39_x, p39_y);
    rt.secondary_down(p39_x, p39_y);
    let (claim, claim_q) = count_span_box_queries(|| rt.core().secondary_claim());
    assert_eq!(claim, "app");
    assert!(claim_q <= 1, "secondary_claim over pill should query <= 1 span, got {claim_q}");
    let (_, up_q) = count_span_box_queries(|| {
        rt.secondary_up(p39_x, p39_y);
        rt.frame(1.0 / 60.0);
    });
    assert!(up_q <= 1, "field_menu on_pill_secondary should query <= 1 span, got {up_q}");
    assert_eq!(rt.state.menus, vec!["p39"]);
}

#[test]
fn input_pill_at_single_line_scrolled_matches_full_scan_with_bounded_queries() {
    let mut rt = Runtime::new(make_pill_doc(40, true), pill_doc_view, (180.0, 100.0), Env::new(true, 1.0));
    rt.frame(0.0);
    settle_pill_doc(&mut rt);

    let i = field_node_index(&rt);
    let path = rt.tree.nodes[i].path.clone();
    let unscrolled = all_pill_boxes(rt.core(), i);
    // Scroll horizontally so pill 20 sits near the left edge of the 180pt field.
    let scroll_x = unscrolled.iter().find(|(k, _)| *k == 20).unwrap().1[0] - 12.0;
    assert!(scroll_x > 100.0, "expected large horizontal scroll, got {scroll_x}");
    rt.edit.input_scroll.insert(path, scroll_x);

    let scrolled = all_pill_boxes(rt.core(), i);
    let r20 = scrolled.iter().find(|(k, _)| *k == 20).unwrap().1;
    let r21 = scrolled.iter().find(|(k, _)| *k == 21).unwrap().1;
    let mid = |r: [f32; 4]| ((r[0] + r[2]) * 0.5, (r[1] + r[3]) * 0.5);

    let (mx, my) = mid(r20);
    let (got, queries) = count_span_box_queries(|| rt.core().input_pill_at(i, mx, my));
    assert_eq!(got.as_deref(), Some("p20"));
    assert!(queries <= 1, "scrolled pill hit should query <= 1 span, got {queries}");

    let (gx, gy) = ((r20[2] + r21[0]) * 0.5, my);
    let (miss, miss_q) = count_span_box_queries(|| rt.core().input_pill_at(i, gx, gy));
    assert_eq!(miss, None);
    assert_eq!(miss_q, 0, "scrolled gap miss should query 0 spans, got {miss_q}");

    for &(k, r) in &scrolled {
        let cx = (r[0] + r[2]) * 0.5;
        let cy = (r[1] + r[3]) * 0.5;
        for (px, py) in [(cx, cy), (r[0], r[1]), (r[2] - 0.01, cy), (r[2], cy)] {
            let want = reference_input_pill_at(rt.core(), i, px, py);
            let (got, q) = count_span_box_queries(|| rt.core().input_pill_at(i, px, py));
            assert_eq!(got, want, "scrolled pill {k} at ({px}, {py}) diverged");
            assert!(q <= 1, "scrolled pill {k} at ({px}, {py}) queried {q} spans");
        }
    }
}

#[test]
fn on_pill_press_consumes_primary_click_and_falls_through_when_false() {
    use crate::{Cursor, Key, KeyEvent, Mods, PointerPress};

    struct PressDoc {
        edit: TextEdit,
        plain_pills: TextEdit,
        disabled_edit: TextEdit,
        inert_edit: TextEdit,
        consume: bool,
        pressed: Vec<(String, PointerPress)>,
        sel_moves: Vec<(usize, usize)>,
    }

    fn make_pills() -> TextEdit {
        let mut e = TextEdit::new("Hi ").rich();
        e.insert_pill("p0", "@Ada");
        e.insert(" mid ");
        e.insert_pill("p1", "@Ben");
        e.insert(" ");
        e.insert_pill("rtl:1", "مرحبا");
        e.insert(" end");
        e.set_caret(0);
        e
    }

    fn view(s: &PressDoc, _: &mut Cx<PressDoc>) -> El {
        let style = TextStyle::new(14.0, 400.0).lh(20.0);
        let consume = s.consume;
        col()
            .w(360.0)
            .h(320.0)
            .gap(8.0)
            .child(
                text_input(&s.edit, style)
                    .key("field")
                    .label("Editable pills")
                    .w_full()
                    .h(48.0)
                    .on_edit(|s: &mut PressDoc| &mut s.edit)
                    .on_pill_press(move |s: &mut PressDoc, id, press, _| {
                        s.pressed.push((id.to_owned(), press));
                        consume
                    })
                    .on_input_selection(|s: &mut PressDoc, _| {
                        s.sel_moves.push((s.edit.anchor, s.edit.focus));
                    }),
            )
            .child(
                text_input(&s.plain_pills, style)
                    .key("no-handler")
                    .label("No pill press handler")
                    .w_full()
                    .h(48.0)
                    .on_edit(|s: &mut PressDoc| &mut s.plain_pills),
            )
            .child(
                text_input(&s.disabled_edit, style)
                    .key("disabled")
                    .label("Disabled pills")
                    .w_full()
                    .h(48.0)
                    .disabled(true)
                    .on_edit(|s: &mut PressDoc| &mut s.disabled_edit)
                    .on_pill_press(|s: &mut PressDoc, id, press, _| {
                        s.pressed.push((format!("dis:{id}"), press));
                        true
                    }),
            )
            .child(
                col().inert().child(
                    text_input(&s.inert_edit, style)
                        .key("inert")
                        .label("Inert pills")
                        .w_full()
                        .h(48.0)
                        .on_edit(|s: &mut PressDoc| &mut s.inert_edit)
                        .on_pill_press(|s: &mut PressDoc, id, press, _| {
                            s.pressed.push((format!("inert:{id}"), press));
                            true
                        }),
                ),
            )
    }

    let mut rt = Runtime::new(
        PressDoc {
            edit: make_pills(),
            plain_pills: make_pills(),
            disabled_edit: make_pills(),
            inert_edit: make_pills(),
            consume: true,
            pressed: Vec::new(),
            sel_moves: Vec::new(),
        },
        view,
        (360.0, 320.0),
        Env::new(true, 1.0),
    );
    rt.frame(0.0);

    let node_of = |rt: &Runtime<PressDoc>, key: &str| {
        rt.tree.nodes.iter().position(|n| n.key.as_deref() == Some(key)).expect(key)
    };
    let mid = |r: [f32; 4]| ((r[0] + r[2]) * 0.5, (r[1] + r[3]) * 0.5);

    let fi = node_of(&rt, "field");
    let boxes = all_pill_boxes(rt.core(), fi);
    let r_p0 = boxes.iter().find(|(k, _)| *k == 0).unwrap().1;
    let r_p1 = boxes.iter().find(|(k, _)| *k == 1).unwrap().1;
    let r_rtl = boxes.iter().find(|(k, _)| *k == 2).unwrap().1;
    let (p0x, p0y) = mid(r_p0);
    let (p1x, p1y) = mid(r_p1);
    let (rtlx, rtly) = mid(r_rtl);
    let plain_x = (r_p0[2] + r_p1[0]) * 0.5;
    let plain_y = p0y;

    // 1. Hover cursor: Pointer over pill with handler, Text over plain text, Text over pill without handler.
    rt.pointer_move(p0x, p0y);
    assert_eq!(rt.cursor(), Cursor::Pointer);
    rt.pointer_move(plain_x, plain_y);
    assert_eq!(rt.cursor(), Cursor::Text);

    let ni = node_of(&rt, "no-handler");
    let n_r0 = all_pill_boxes(rt.core(), ni).into_iter().find(|(k, _)| *k == 0).unwrap().1;
    let (nx, ny) = mid(n_r0);
    rt.pointer_move(nx, ny);
    assert_eq!(rt.cursor(), Cursor::Text, "field without on_pill_press keeps Cursor::Text over pills");

    // 2. Holding Shift suppresses Pointer cursor over pill.
    rt.pointer_move(p0x, p0y);
    assert_eq!(rt.cursor(), Cursor::Pointer);
    rt.physical_input(crate::PhysicalInput::Modifiers(Mods { shift: true, ..Mods::default() }));
    assert_eq!(rt.cursor(), Cursor::Text, "Shift+hover over pill shows Cursor::Text");
    rt.physical_input(crate::PhysicalInput::Modifiers(Mods::default()));

    // 3. Consuming primary press on p0: fires with id & window point, bounded <= 1 query,
    // does not focus, move caret, fire on_input_selection, or start a drag.
    rt.pointer_move(p0x, p0y);
    let (_, queries) = count_span_box_queries(|| {
        rt.pointer_down(p0x, p0y);
        rt.pointer_move(p1x, p1y);
        rt.pointer_up(p1x, p1y);
        rt.frame(1.0 / 60.0);
    });
    assert!(queries <= 1, "on_pill_press should query <= 1 span, got {queries}");
    assert_eq!(rt.state.pressed, vec![("p0".to_owned(), PointerPress { x: p0x, y: p0y })]);
    assert!(!rt.state.edit.focused, "consumed pill press should not focus field");
    assert_eq!((rt.state.edit.anchor, rt.state.edit.focus), (0, 0));
    assert!(rt.state.sel_moves.is_empty(), "consumed pill press should not fire on_input_selection");

    // Multi-frame drag after consumed pill press also must not select text.
    rt.state.pressed.clear();
    rt.pointer_move(rtlx, rtly);
    rt.pointer_down(rtlx, rtly);
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.state.pressed, vec![("rtl:1".to_owned(), PointerPress { x: rtlx, y: rtly })]);
    rt.pointer_move(p0x, p0y);
    rt.frame(1.0 / 60.0);
    rt.pointer_up(p0x, p0y);
    rt.frame(1.0 / 60.0);
    assert_eq!((rt.state.edit.anchor, rt.state.edit.focus), (0, 0));
    assert!(rt.state.sel_moves.is_empty());

    // 4. Click on plain text focuses field, moves caret, does not fire on_pill_press.
    rt.state.pressed.clear();
    rt.pointer_move(plain_x, plain_y);
    rt.pointer_down(plain_x, plain_y);
    rt.pointer_up(plain_x, plain_y);
    rt.frame(1.0 / 60.0);
    assert!(rt.state.pressed.is_empty());
    assert!(rt.state.edit.focused);
    assert!(rt.state.edit.focus > 0);
    assert_eq!(rt.state.sel_moves.len(), 1);
    let anchor_after_plain = rt.state.edit.focus;

    // 5. Shift+click on pill p1 bypasses on_pill_press and extends selection normally.
    rt.physical_input(crate::PhysicalInput::Modifiers(Mods { shift: true, ..Mods::default() }));
    rt.pointer_move(p1x, p1y);
    rt.pointer_down(p1x, p1y);
    rt.pointer_up(p1x, p1y);
    rt.frame(1.0 / 60.0);
    rt.physical_input(crate::PhysicalInput::Modifiers(Mods::default()));
    assert!(rt.state.pressed.is_empty(), "Shift+click must bypass on_pill_press");
    assert_eq!(rt.state.edit.anchor, anchor_after_plain);
    assert_ne!(rt.state.edit.focus, anchor_after_plain);

    // 6. Dragging a selection across a pill keeps Cursor::Text while dragging.
    rt.pointer_move(plain_x, plain_y);
    rt.pointer_down(plain_x, plain_y);
    rt.frame(1.0 / 60.0);
    rt.pointer_move(p0x, p0y);
    assert_eq!(rt.cursor(), Cursor::Text, "dragging across pill keeps Cursor::Text");
    rt.pointer_up(p0x, p0y);
    rt.frame(1.0 / 60.0);

    // 7. Returning false from on_pill_press falls through to normal caret placement & double-click selection.
    rt.update(|s, _| {
        s.consume = false;
        s.pressed.clear();
        s.sel_moves.clear();
        s.edit.set_caret(0);
    });
    rt.frame(1.0 / 60.0);
    rt.pointer_move(p1x, p1y);
    rt.pointer_down(p1x, p1y);
    rt.pointer_up(p1x, p1y);
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.state.pressed.len(), 1);
    assert_eq!(rt.state.pressed[0].0, "p1");
    assert!(rt.state.edit.focus > 0, "returning false falls through to caret placement");
    assert!(!rt.state.sel_moves.is_empty());

    // Second click within double-click interval selects the whole p1 pill.
    rt.pointer_down(p1x, p1y);
    rt.pointer_up(p1x, p1y);
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.state.edit.selected(), "@Ben");

    // 8. Accessibility tree stays intact with textbox role and name, and keyboard arrows collapse selection.
    let a11y = rt.accessibility_json().to_string();
    assert!(a11y.contains("Editable pills"));
    rt.key(KeyEvent { key: Key::Left, mods: Mods::default() });
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.state.edit.anchor, rt.state.edit.focus);

    // 9. Disabled and inert fields never fire on_pill_press or show Pointer cursor.
    rt.update(|s, _| {
        s.consume = true;
        s.pressed.clear();
    });
    rt.frame(1.0 / 60.0);
    for key in ["disabled", "inert"] {
        let idx = node_of(&rt, key);
        let r0 = all_pill_boxes(rt.core(), idx).into_iter().find(|(k, _)| *k == 0).unwrap().1;
        let (x, y) = mid(r0);
        rt.pointer_move(x, y);
        assert_ne!(rt.cursor(), Cursor::Pointer, "{key} field must not show Cursor::Pointer");
        rt.pointer_down(x, y);
        rt.pointer_up(x, y);
        rt.frame(1.0 / 60.0);
    }
    assert!(rt.state.pressed.is_empty(), "disabled/inert pills must not fire on_pill_press");
}

#[test]
fn on_pill_press_works_on_scrolled_single_line_and_empty_pills() {
    struct SingleDoc {
        scrolled: TextEdit,
        empty: TextEdit,
        pressed: Vec<String>,
    }

    fn view(s: &SingleDoc, _: &mut Cx<SingleDoc>) -> El {
        let style = TextStyle::new(14.0, 400.0).lh(20.0);
        col()
            .w(180.0)
            .h(120.0)
            .gap(8.0)
            .child(
                text_input(&s.scrolled, style)
                    .key("scrolled")
                    .single_line()
                    .size(180.0, 28.0)
                    .on_edit(|s: &mut SingleDoc| &mut s.scrolled)
                    .on_pill_press(|s: &mut SingleDoc, id, _, _| {
                        s.pressed.push(id.to_owned());
                        true
                    }),
            )
            .child(
                text_input(&s.empty, style)
                    .key("empty")
                    .single_line()
                    .size(180.0, 28.0)
                    .on_edit(|s: &mut SingleDoc| &mut s.empty)
                    .on_pill_press(|s: &mut SingleDoc, id, _, _| {
                        s.pressed.push(id.to_owned());
                        true
                    }),
            )
    }

    let doc = make_pill_doc(40, true);
    let mut rt = Runtime::new(
        SingleDoc { scrolled: doc.edit, empty: TextEdit::new("").rich(), pressed: Vec::new() },
        view,
        (180.0, 120.0),
        Env::new(true, 1.0),
    );
    rt.frame(0.0);

    let si = rt.tree.nodes.iter().position(|n| n.key.as_deref() == Some("scrolled")).unwrap();
    let path = rt.tree.nodes[si].path.clone();
    let unscrolled = all_pill_boxes(rt.core(), si);
    let scroll_x = unscrolled.iter().find(|(k, _)| *k == 20).unwrap().1[0] - 12.0;
    rt.edit.input_scroll.insert(path, scroll_x);
    let r20 = all_pill_boxes(rt.core(), si).into_iter().find(|(k, _)| *k == 20).unwrap().1;
    let (mx, my) = ((r20[0] + r20[2]) * 0.5, (r20[1] + r20[3]) * 0.5);

    let (_, q) = count_span_box_queries(|| {
        rt.pointer_down(mx, my);
        rt.pointer_up(mx, my);
        rt.frame(1.0 / 60.0);
    });
    assert!(q <= 1, "scrolled on_pill_press should query <= 1 span, got {q}");
    assert_eq!(rt.state.pressed, vec!["p20"]);

    // Clicking an empty field with on_pill_press queries 0 spans and never calls the handler.
    let (ex, ey) = rt.find("empty").unwrap();
    let (_, eq) = count_span_box_queries(|| {
        rt.pointer_down(ex, ey);
        rt.pointer_up(ex, ey);
        rt.frame(1.0 / 60.0);
    });
    assert_eq!(eq, 0);
    assert_eq!(rt.state.pressed, vec!["p20"]);
}
