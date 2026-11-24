//! Tests for `Runtime::text_offset_at`, `Tx::text_offset_at`, and `Cx::field_selection_rects`.

use std::cell::RefCell;

use rune::{
    Cx, El, ElementInfo, Env, Ime, Key, KeyEvent, Mods, Runtime, ScrollSpec, TextEdit, TextStyle,
    Transition, col, el, memo, text, text_input,
};

fn elem<S: 'static>(rt: &Runtime<S>, key: &str) -> ElementInfo {
    rt.elements()
        .into_iter()
        .find(|e| e.key.as_deref() == Some(key))
        .unwrap_or_else(|| panic!("element {key:?} not found"))
}

// ── Runtime::text_offset_at & Tx::text_offset_at ───────────────────────────────────────────────

#[test]
fn text_offset_at_plain_wrapped_utf8_and_rtl_text() {
    struct S {
        plain: String,
        wrapped: String,
        rtl: String,
        empty: String,
    }
    fn view(s: &S, _: &mut Cx<S>) -> El {
        let style = TextStyle::new(14.0, 400.0).lh(20.0);
        col().w_full().h_full().padding(10.0).gap(8.0)
            .child(text(&s.plain).key("plain").font(style).w_fit())
            .child(text(&s.wrapped).key("wrapped").font(style).w(120.0).wrap())
            .child(text(&s.rtl).key("rtl").font(style).w(200.0))
            .child(text(&s.empty).key("empty").font(style).w(100.0).h(20.0))
            .child(text("inert words").key("inert").font(style).inert())
            .child(text("secret label").key("secret").font(style).secure())
            .child(el().key("box").size(40.0, 20.0))
    }
    let state = S {
        plain: "alpha 👋🏽 omega".into(),
        wrapped: "first line wraps onto a second line and a third".into(),
        rtl: "مرحبا بالعالم".into(),
        empty: String::new(),
    };
    let mut rt = Runtime::new(state, view, (320.0, 320.0), Env::new(true, 2.0));
    rt.frame(0.0);

    // Plain text: left edge hits start, middle hits interior char boundary, right edge hits end.
    let p = elem(&rt, "plain").rect;
    let mid_y = p[1] + p[3] * 0.5;
    let start = rt.text_offset_at("plain", p[0] + 0.5, mid_y).unwrap();
    let mid = rt.text_offset_at("plain", p[0] + p[2] * 0.5, mid_y).unwrap();
    let end = rt.text_offset_at("plain", p[0] + p[2] - 0.5, mid_y).unwrap();
    assert_eq!(start, 0);
    assert!(mid > 0 && mid < rt.state.plain.len(), "mid={mid}");
    assert!(rt.state.plain.is_char_boundary(mid));
    assert_eq!(end, rt.state.plain.len());

    // Wrapped multiline text: hitting line 2 returns a strictly larger offset than line 1.
    let w = elem(&rt, "wrapped").rect;
    assert!(w[3] > 35.0, "expected wrapped text to span multiple lines: {w:?}");
    let line1 = rt.text_offset_at("wrapped", w[0] + 20.0, w[1] + 10.0).unwrap();
    let line2 = rt.text_offset_at("wrapped", w[0] + 20.0, w[1] + 30.0).unwrap();
    assert!(line2 > line1, "line1={line1}, line2={line2}");
    assert!(rt.state.wrapped.is_char_boundary(line1) && rt.state.wrapped.is_char_boundary(line2));

    // RTL text: right edge hits start (small offset), left edge hits later bytes.
    let r = elem(&rt, "rtl").rect;
    let ry = r[1] + r[3] * 0.5;
    let rtl_right = rt.text_offset_at("rtl", r[0] + r[2] - 1.0, ry).unwrap();
    let rtl_left = rt.text_offset_at("rtl", r[0] + 1.0, ry).unwrap();
    assert!(rtl_right < rtl_left, "RTL right ({rtl_right}) < left ({rtl_left})");
    assert!(rt.state.rtl.is_char_boundary(rtl_right) && rt.state.rtl.is_char_boundary(rtl_left));

    // Empty text returns Some(0).
    let e = elem(&rt, "empty").rect;
    assert_eq!(rt.text_offset_at("empty", e[0] + 5.0, e[1] + 5.0), Some(0));

    // Inert text is still queryable.
    let (ix, iy) = rt.find("inert").unwrap();
    assert!(rt.text_offset_at("inert", ix, iy).is_some());

    // Missing, empty key, non-text, secure, and non-finite coordinates all return None.
    assert_eq!(rt.text_offset_at("missing", 10.0, 10.0), None);
    assert_eq!(rt.text_offset_at("", 10.0, 10.0), None);
    let (bx, by) = rt.find("box").unwrap();
    assert_eq!(rt.text_offset_at("box", bx, by), None);
    let (sx, sy) = rt.find("secret").unwrap();
    assert_eq!(rt.text_offset_at("secret", sx, sy), None);
    assert_eq!(rt.text_offset_at("plain", f32::NAN, mid_y), None);
    assert_eq!(rt.text_offset_at("plain", p[0], f32::INFINITY), None);
}

#[test]
fn text_offset_at_fields_single_line_scrolled_multiline_pills_scaled_exiting_and_secure() {
    struct S {
        single: TextEdit,
        multi: TextEdit,
        secure: TextEdit,
        scaled: TextEdit,
        show_temp: bool,
    }
    fn view(s: &S, _: &mut Cx<S>) -> El {
        let style = TextStyle::new(15.0, 400.0).lh(20.0);
        let mut root = col().w_full().h_full().padding(10.0).gap(8.0)
            .child(
                text_input(&s.single, style).single_line().key("single").size(100.0, 26.0)
                    .on_edit(|s: &mut S| &mut s.single),
            )
            .child(
                col().key("panel").child(
                    text_input(&s.multi, style).key("multi").w(140.0).min_h(50.0)
                        .on_edit(|s: &mut S| &mut s.multi),
                ),
            )
            .child(
                text_input(&s.secure, style).single_line().key("pw").size(140.0, 26.0)
                    .on_edit(|s: &mut S| &mut s.secure),
            )
            .child(
                col().key("scaled-wrap").scale(2.0).origin(0.0, 0.0).child(
                    text_input(&s.scaled, style).single_line().key("scaled").size(120.0, 26.0)
                        .on_edit(|s: &mut S| &mut s.scaled),
                ),
            );
        if s.show_temp {
            root = root.child(text("leaving soon").key("temp").font(style).transition(Transition::opacity()));
        }
        root
    }

    let mut single = TextEdit::new("a very long single-line query that scrolls past the right edge");
    single.focused = true;
    single.set_selection(single.text.len(), single.text.len());

    let mut multi = TextEdit::new("hello ");
    multi.insert_pill("file:main.rs", "main.rs");
    let _ = multi.insert("\nsecond line here");

    let mut secure = TextEdit::new("hunter2").secure();
    secure.focused = false;

    let scaled = TextEdit::new("abcdefghij");

    let mut rt = Runtime::new(
        S { single, multi, secure, scaled, show_temp: true },
        view,
        (400.0, 400.0),
        Env::new(true, 2.0),
    );
    rt.frame(0.0);

    // Horizontally scrolled single-line field: clicking near its left edge hits scrolled text (> 10), not 0.
    let sr = elem(&rt, "single").rect;
    let scrolled_offset = rt.text_offset_at("single", sr[0] + 2.0, sr[1] + sr[3] * 0.5).unwrap();
    assert!(scrolled_offset > 10, "expected horizontally scrolled offset > 10, got {scrolled_offset}");
    assert!(rt.state.single.text.is_char_boundary(scrolled_offset));

    // Multiline field resolved by key path ("panel/multi"): line 2 > line 1, valid model offset with pill.
    let mr = elem(&rt, "multi").rect;
    let m1 = rt.text_offset_at("panel/multi", mr[0] + 5.0, mr[1] + 10.0).unwrap();
    let m2 = rt.text_offset_at("panel/multi", mr[0] + 5.0, mr[1] + 30.0).unwrap();
    assert!(m2 > m1, "multiline line 2 ({m2}) > line 1 ({m1})");
    assert!(rt.state.multi.text.is_char_boundary(m1) && rt.state.multi.text.is_char_boundary(m2));

    // Secure text field returns None.
    let (px, py) = rt.find("pw").unwrap();
    assert_eq!(rt.text_offset_at("pw", px, py), None);

    // Scaled subtree (2x): left edge hits start, middle hits interior, right edge hits end.
    let (sc_rect, _) = rt.presented("scaled").unwrap();
    assert!(sc_rect[2] > 200.0, "expected 2x scaled width: {sc_rect:?}");
    let sc_y = sc_rect[1] + sc_rect[3] * 0.5;
    let sc_start = rt.text_offset_at("scaled", sc_rect[0] + 2.0, sc_y).unwrap();
    let sc_mid = rt.text_offset_at("scaled", sc_rect[0] + 40.0, sc_y).unwrap();
    let sc_end = rt.text_offset_at("scaled", sc_rect[0] + sc_rect[2] - 2.0, sc_y).unwrap();
    assert_eq!(sc_start, 0);
    assert!(sc_mid > 0 && sc_mid < sc_end, "start={sc_start}, mid={sc_mid}, end={sc_end}");
    assert_eq!(sc_end, rt.state.scaled.text.len());

    // Exiting element returns None once removal begins its exit transition.
    let (tx, ty) = rt.find("temp").unwrap();
    assert!(rt.text_offset_at("temp", tx, ty).is_some());
    rt.update(|s, _| s.show_temp = false);
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.text_offset_at("temp", tx, ty), None);
}

#[test]
fn tx_text_offset_at_from_secondary_press_and_nested_query() {
    struct S {
        edit: TextEdit,
        log: Vec<Option<usize>>,
    }
    fn view(s: &S, _: &mut Cx<S>) -> El {
        let style = TextStyle::new(14.0, 400.0).lh(20.0);
        col().w_full().h_full().padding(10.0).child(
            text_input(&s.edit, style).key("editor").w(260.0).h(40.0)
                .on_edit(|s: &mut S| &mut s.edit)
                .on_secondary_press(|_: &mut S, p, tx| {
                    let (wx, wy) = (p.x, p.y);
                    tx.text_offset_at("editor", wx, wy, move |s: &mut S, at, tx| {
                        s.log.push(at);
                        // Nested query waits until the next frame (matches Tx::element_at).
                        tx.text_offset_at("missing", wx, wy, |s: &mut S, next, _| {
                            s.log.push(next);
                        });
                    });
                }),
        )
    }
    let mut rt = Runtime::new(
        S { edit: TextEdit::new("spellcheck this misspelling please"), log: Vec::new() },
        view,
        (300.0, 120.0),
        Env::new(true, 2.0),
    );
    rt.frame(0.0);

    let r = elem(&rt, "editor").rect;
    let (x, y) = (r[0] + 60.0, r[1] + 10.0);
    let expected = rt.text_offset_at("editor", x, y);
    assert!(expected.is_some_and(|o| o > 0 && o < rt.state.edit.text.len()), "{expected:?}");

    rt.secondary_down(x, y);
    rt.secondary_up(x, y);
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.state.log, vec![expected], "first callback runs in same frame before commit");

    rt.frame(1.0 / 60.0);
    assert_eq!(rt.state.log, vec![expected, None], "nested query runs on the next frame");
}

// ── Cx::field_selection_rects ──────────────────────────────────────────────────────────────────

thread_local! {
    static FIELD_RECTS: RefCell<Vec<Vec<[f32; 4]>>> = const { RefCell::new(Vec::new()) };
    static MEMO_BUILDS: RefCell<usize> = const { RefCell::new(0) };
}

fn last_field_rects() -> Vec<[f32; 4]> {
    FIELD_RECTS.with_borrow(|v| v.last().cloned().unwrap_or_default())
}

#[test]
fn field_selection_rects_single_multiline_scroll_rtl_ime_and_secure() {
    struct S {
        single: TextEdit,
        multi: TextEdit,
        rtl: TextEdit,
        secure: TextEdit,
        watch_key: &'static str,
    }
    fn view(s: &S, cx: &mut Cx<S>) -> El {
        let style = TextStyle::new(14.0, 400.0).lh(20.0);
        let boxes = cx.field_selection_rects(s.watch_key);
        FIELD_RECTS.with_borrow_mut(|v| v.push(boxes));
        col().w_full().h_full().padding(10.0).gap(8.0)
            .child(
                col().key("scroller").w(260.0).h(120.0).scroll(ScrollSpec { stick_bottom: false }).child(
                    col().w_full().gap(8.0)
                        .child(
                            text_input(&s.single, style).single_line().key("single").size(100.0, 26.0)
                                .on_edit(|s: &mut S| &mut s.single),
                        )
                        .child(
                            col().key("composer").child(
                                text_input(&s.multi, style).key("input").w(120.0)
                                    .on_edit(|s: &mut S| &mut s.multi),
                            ),
                        )
                        .child(
                            text_input(&s.rtl, style).single_line().key("rtl").size(180.0, 26.0)
                                .on_edit(|s: &mut S| &mut s.rtl),
                        )
                        .child(
                            text_input(&s.secure, style).single_line().key("pw").size(140.0, 26.0)
                                .on_edit(|s: &mut S| &mut s.secure),
                        )
                        .child(el().w_full().h(300.0)),
                ),
            )
    }

    let mut single = TextEdit::new("a very long single line string that overflows the narrow field");
    single.set_selection(0, 10); // unfocused initially

    let mut multi = TextEdit::new("alpha beta gamma delta epsilon zeta eta theta");
    multi.set_selection(0, multi.text.len());

    let mut rtl = TextEdit::new("مرحبا بالعالم");
    rtl.set_selection(0, "مرحبا".len());

    let mut secure = TextEdit::new("topsecret").secure();
    secure.set_selection(0, 5);

    FIELD_RECTS.with_borrow_mut(Vec::clear);
    let mut rt = Runtime::new(
        S { single, multi, rtl, secure, watch_key: "single" },
        view,
        (320.0, 240.0),
        Env::new(true, 2.0),
    );
    rt.frame(0.0);
    assert!(last_field_rects().is_empty(), "unfocused field returns empty");

    // Caret-only focused field returns empty.
    rt.update(|s, _| {
        s.single.focused = true;
        s.single.set_selection(5, 5);
    });
    rt.frame(1.0 / 60.0);
    assert!(last_field_rects().is_empty(), "caret-only returns empty");

    // Single-line scrolled selection: 1 rect, shifted by single_line_offset so the focused end sits in view.
    rt.update(|s, _| {
        let end = s.single.text.len();
        s.single.set_selection(end - 12, end);
    });
    rt.frame(1.0 / 60.0);
    let single_boxes = last_field_rects();
    assert_eq!(single_boxes.len(), 1, "single-line selection has 1 box: {single_boxes:?}");
    let sr = elem(&rt, "single").rect;
    assert!(single_boxes[0][2] > 10.0 && single_boxes[0][3] > 10.0);
    assert!(single_boxes[0][0] + single_boxes[0][2] <= sr[0] + sr[2] + 4.0, "scrolled box inside field: {:?} vs {sr:?}", single_boxes[0]);

    // IME preedit hides selection rects.
    rt.ime(Ime::Preedit("a".into(), Some((0, 1)), Vec::new()));
    rt.frame(1.0 / 60.0);
    assert!(last_field_rects().is_empty(), "IME preedit returns empty");
    rt.ime(Ime::Preedit(String::new(), None, Vec::new()));
    rt.frame(1.0 / 60.0);

    // Switch to multiline field matched via ancestor wrapper key ("composer") and key path ("composer/input").
    rt.update(|s, _| {
        s.single.focused = false;
        s.multi.focused = true;
        s.watch_key = "composer";
    });
    rt.frame(1.0 / 60.0);
    let multi_boxes = last_field_rects();
    assert!(multi_boxes.len() >= 2, "wrapped multiline selection has multiple lines: {multi_boxes:?}");
    assert!(multi_boxes.windows(2).all(|w| w[0][1] < w[1][1]), "reading order: {multi_boxes:?}");

    // Key path matching ("composer/input") returns the same boxes.
    rt.update(|s, _| s.watch_key = "composer/input");
    rt.frame(1.0 / 60.0);
    assert_eq!(last_field_rects(), multi_boxes);

    // Vertical scroll moves the boxes and re-runs the view.
    let before = last_field_rects();
    let sc = elem(&rt, "scroller").rect;
    rt.wheel(sc[0] + 10.0, sc[1] + 10.0, -40.0);
    for _ in 0..30 { rt.frame(1.0 / 60.0); }
    let after = last_field_rects();
    assert_eq!(after.len(), before.len());
    let dy = before[0][1] - after[0][1];
    assert!(dy > 5.0 && after.iter().zip(&before).all(|(a, b)| (b[1] - a[1] - dy).abs() < 0.5), "{before:?} -> {after:?}");

    // RTL focused field selection returns a valid positive-width box.
    rt.update(|s, _| {
        s.multi.focused = false;
        s.rtl.focused = true;
        s.watch_key = "rtl";
    });
    rt.frame(1.0 / 60.0);
    let rtl_boxes = last_field_rects();
    assert_eq!(rtl_boxes.len(), 1, "RTL selection box: {rtl_boxes:?}");
    assert!(rtl_boxes[0][2] > 5.0 && rtl_boxes[0][3] > 5.0);

    // Secure focused field always returns empty.
    rt.update(|s, _| {
        s.rtl.focused = false;
        s.secure.focused = true;
        s.watch_key = "pw";
    });
    rt.frame(1.0 / 60.0);
    assert!(last_field_rects().is_empty(), "secure field returns empty");
}

#[test]
fn field_selection_rects_threshold_memo_invalidation_and_unrelated_field_isolation() {
    struct S {
        a: TextEdit,
        b: TextEdit,
        offset_y: f32,
        other: u32,
    }
    fn view(s: &S, cx: &mut Cx<S>) -> El {
        let style = TextStyle::new(14.0, 400.0).lh(20.0);
        let bar = memo("bar", 0, || {
            MEMO_BUILDS.with_borrow_mut(|n| *n += 1);
            let boxes = cx.field_selection_rects("a");
            FIELD_RECTS.with_borrow_mut(|v| v.push(boxes.clone()));
            text(format!("boxes:{}", boxes.len())).font(style)
        });
        col().w_full().h_full().padding(10.0)
            .child(bar)
            .child(text(format!("{}", s.other)).key("other").font(style))
            .child(el().h(s.offset_y))
            .child(
                text_input(&s.a, style).key("a").w_full().on_edit(|s: &mut S| &mut s.a),
            )
            .child(
                text_input(&s.b, style).key("b").w_full().on_edit(|s: &mut S| &mut s.b),
            )
    }

    let mut a = TextEdit::new("alpha beta gamma delta epsilon");
    a.focused = true;
    a.set_selection(0, 10);
    let b = TextEdit::new("unrelated field text");

    FIELD_RECTS.with_borrow_mut(Vec::clear);
    MEMO_BUILDS.with_borrow_mut(|n| *n = 0);
    let mut rt = Runtime::new(S { a, b, offset_y: 0.0, other: 0 }, view, (320.0, 240.0), Env::new(true, 2.0));
    rt.frame(0.0);
    assert_eq!(last_field_rects().len(), 1);

    // Idle frames do not recommit or rebuild the memo.
    let commits_idle = rt.stats.commits;
    let builds_idle = MEMO_BUILDS.with_borrow(|n| *n);
    assert!(!rt.frame(1.0 / 60.0));
    assert!(!rt.frame(1.0 / 60.0));
    assert_eq!(rt.stats.commits, commits_idle);
    assert_eq!(MEMO_BUILDS.with_borrow(|n| *n), builds_idle);

    // Unrelated state change reuses the memo because field a selection boxes did not move.
    rt.update(|s, _| s.other += 1);
    rt.frame(1.0 / 60.0);
    assert_eq!(MEMO_BUILDS.with_borrow(|n| *n), builds_idle, "memo reused when selection unchanged");

    // Moving field a by 0.5 pt (not > 0.5 pt threshold) keeps the memo cached.
    rt.update(|s, _| s.offset_y = 0.5);
    rt.frame(1.0 / 60.0);
    assert_eq!(MEMO_BUILDS.with_borrow(|n| *n), builds_idle, "0.5 pt shift stays within threshold");

    // Moving field a by 1.5 pt (> 0.5 pt threshold) invalidates and rebuilds the memo.
    rt.update(|s, _| s.offset_y = 1.5);
    rt.frame(1.0 / 60.0);
    assert_eq!(MEMO_BUILDS.with_borrow(|n| *n), builds_idle + 1, "1.5 pt shift crosses threshold");

    // Changing selection inside field a rebuilds the memo in the same frame.
    let builds_before_sel = MEMO_BUILDS.with_borrow(|n| *n);
    rt.update(|s, _| s.a.set_selection(0, 20));
    rt.frame(1.0 / 60.0);
    assert!(MEMO_BUILDS.with_borrow(|n| *n) > builds_before_sel, "selection change rebuilds memo");

    // Viewport resize (relayout) that wraps field a onto multiple lines rebuilds the memo.
    let builds_before_resize = MEMO_BUILDS.with_borrow(|n| *n);
    rt.set_viewport((90.0, 240.0), 2.0);
    rt.frame(1.0 / 60.0);
    assert!(MEMO_BUILDS.with_borrow(|n| *n) > builds_before_resize, "resize rewrap rebuilds memo");
    assert!(last_field_rects().len() >= 2, "wrapped after narrow resize: {:?}", last_field_rects());

    // Moving focus to unrelated field b clears field a boxes once, then further edits in b do NOT
    // rebuild the memo watching field a.
    rt.update(|s, _| {
        s.a.focused = false;
        s.b.focused = true;
        s.b.set_selection(0, 5);
    });
    rt.frame(1.0 / 60.0);
    assert!(last_field_rects().is_empty());
    let builds_on_b = MEMO_BUILDS.with_borrow(|n| *n);
    rt.key(KeyEvent { key: Key::Right, mods: Mods { shift: true, ..Mods::default() } });
    rt.frame(1.0 / 60.0);
    assert_eq!(MEMO_BUILDS.with_borrow(|n| *n), builds_on_b, "unrelated field b selection change must not rebuild memo watching a");
}
