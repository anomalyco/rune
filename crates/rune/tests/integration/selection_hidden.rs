//! A drag in a selection group reaches only text that shows. Text clipped out of sight but
//! laid out under the words (a folded part above them) took a drag in a paragraph to its hidden
//! text and selected everything between, in a zoomed page (t973).
use rune::{Cx, El, Env, Runtime, TextStyle, col, text};

const SEEN: &str = "the words you can see and drag across";

struct S {
    zoom: f32,
}

fn view(s: &S, _: &mut Cx<S>) -> El {
    let style = TextStyle::new(14.0, 400.0).lh(20.0);
    let t = |content: &str, key: &str| text(content.to_owned()).key(key).font(style).w_full().selectable(true);
    let folded = col().key("folded").w_full().h(0.0).clip_rounded(0.0)
        .child(t("hidden earlier note one", "h0"))
        .child(t("hidden earlier note two", "h1"))
        .child(t("hidden earlier note three", "h2"));
    col().w_full().h_full().padding(10.0).child(
        col().key("page").w(400.0).scale(s.zoom).selection_group(true)
            .child(folded)
            .child(t("first line above", "a"))
            .child(t(SEEN, "seen"))
            .child(t("last line below", "b")),
    )
}

fn drag_in_seen(zoom: f32) -> Option<String> {
    let mut rt = Runtime::new(S { zoom }, view, (600.0, 400.0), Env::new(true, 2.0));
    rt.frame(0.0);
    let e = rt.elements().into_iter().find(|e| e.key.as_deref() == Some("seen")).unwrap();
    let y = e.rect[1] + e.rect[3] / 2.0;
    let (x0, x1) = (e.rect[0] + e.rect[2] * 0.2, e.rect[0] + e.rect[2] * 0.5);
    rt.pointer_move(x0, y);
    rt.pointer_down(x0, y);
    rt.frame(1.0 / 60.0);
    for k in 1..=4 {
        rt.pointer_move(x0 + (x1 - x0) * k as f32 / 4.0, y + 2.0);
        rt.frame(1.0 / 60.0);
    }
    rt.pointer_up(x1, y + 2.0);
    rt.frame(1.0 / 60.0);
    rt.selected_text()
}

#[test]
fn a_drag_skips_text_clipped_out_of_sight() {
    for zoom in [1.0, 1.3] {
        let sel = drag_in_seen(zoom).expect("the drag selects");
        assert!(!sel.contains("hidden") && !sel.contains('\n') && SEEN.contains(sel.as_str()), "at {zoom}: only the words dragged over: {sel:?}");
    }
}

const LONG: &str = "a long note folded to its first lines, whose hidden lines run on far below the fold and under the words that come after it, as a long prompt folded in a chat does, line after line after line after line after line after line after line after line after line after line after line after line after line after line after line after line after line";

fn partly_folded(_: &S, _: &mut Cx<S>) -> El {
    let style = TextStyle::new(14.0, 400.0).lh(20.0);
    let t = |content: &str, key: &str| text(content.to_owned()).key(key).font(style).w_full().selectable(true);
    col().w_full().h_full().padding(10.0).child(
        col().key("page").w(400.0).selection_group(true)
            .child(col().key("fold").w_full().h(40.0).clip_rounded(0.0).child(t(LONG, "long").wrap()))
            .child(t("first line above", "a"))
            .child(t(SEEN, "seen"))
            .child(t("last line below", "b")),
    )
}

#[test]
fn a_drag_skips_the_hidden_lines_of_partly_clipped_text() {
    let mut rt = Runtime::new(S { zoom: 1.0 }, partly_folded, (600.0, 600.0), Env::new(true, 2.0));
    rt.frame(0.0);
    let e = rt.elements().into_iter().find(|e| e.key.as_deref() == Some("seen")).unwrap();
    let long = rt.elements().into_iter().find(|e| e.key.as_deref() == Some("long")).unwrap();
    assert!(long.rect[1] + long.rect[3] > e.rect[1] + e.rect[3], "the folded text runs on under the words: {:?} {:?}", long.rect, e.rect);
    let y = e.rect[1] + e.rect[3] / 2.0;
    let (x0, x1) = (e.rect[0] + e.rect[2] * 0.2, e.rect[0] + e.rect[2] * 0.5);
    rt.pointer_move(x0, y);
    rt.pointer_down(x0, y);
    rt.frame(1.0 / 60.0);
    for k in 1..=4 {
        rt.pointer_move(x0 + (x1 - x0) * k as f32 / 4.0, y + 2.0);
        rt.frame(1.0 / 60.0);
    }
    rt.pointer_up(x1, y + 2.0);
    rt.frame(1.0 / 60.0);
    let sel = rt.selected_text().expect("the drag selects");
    assert!(!sel.contains('\n') && SEEN.contains(sel.as_str()), "only the words dragged over: {sel:?}");
}


#[test]
fn truncate_fade_clips_and_fades_selection_and_marks_to_box() {
    use rune::render::{KIND_GLYPH, KIND_RECT, unpack_fade};
    use rune::{Color, Key, KeyEvent, Mods, Span, TextMark, row, rich};

    let hl = rune::color::hex("#3080ff").unwrap().alpha(0.35);
    let bg = rune::color::hex("#222222").unwrap();
    let pill = rune::color::hex("#e6339a").unwrap();
    let msg = "Upstream request failed: [service_unavailable] model is currently experiencing high demand, spikes in traffic";

    struct FadeState {
        msg: &'static str,
        hl: Color,
        bg: Color,
        pill: Color,
        box_w: f32,
        mark_path: Option<String>,
    }

    fn fade_view(s: &FadeState, cx: &mut Cx<FadeState>) -> El {
        if let Some(p) = &s.mark_path {
            cx.mark_texts([TextMark::new(p.clone(), 0..s.msg.len(), s.msg)]);
        }
        let style = TextStyle::new(14.0, 400.0).lh(20.0);
        let spans = vec![Span { bg: Some(s.pill), underline: Some(s.pill), ..Span::new(0, s.msg.len()) }];
        row().w(600.0).h(80.0).padding(16.0).child(
            rich(s.msg, spans)
                .key("err")
                .font(style)
                .w(s.box_w)
                .fill(s.bg)
                .truncate_fade()
                .selectable(true)
                .selection_color(s.hl),
        )
    }

    let scale = 2.0;
    let mut rt = Runtime::new(
        FadeState { msg, hl, bg, pill, box_w: 180.0, mark_path: None },
        fade_view,
        (600.0, 80.0),
        Env::new(true, scale),
    );
    rt.set_srgb_blend(false);
    rt.frame(0.0);

    let err_el = rt.elements().into_iter().find(|e| e.key.as_deref() == Some("err")).unwrap();
    let box_right_px = (err_el.rect[0] + err_el.rect[2]) * scale;

    // Select all of the truncated text.
    rt.key(KeyEvent { key: Key::Command('a'), mods: Mods { cmd: true, ..Mods::default() } });
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.selected_text().as_deref(), Some(msg), "selecting overflowing truncate_fade still copies full text");

    let want_hl = hl.premul(1.0);
    let want_bg = bg.premul(1.0);
    let want_pill = pill.premul(1.0);
    let approx = |a: [f32; 4], b: [f32; 4]| a.iter().zip(b).all(|(x, y)| (x - y).abs() < 1e-3);

    let sel_insts: Vec<_> = rt.instances.iter()
        .filter(|i| i.params[1] == KIND_RECT && approx(i.fill, want_hl))
        .collect();
    assert!(!sel_insts.is_empty(), "selection highlight must paint for visible part");
    let glyph_fade = rt.instances.iter()
        .filter(|i| i.params[1] == KIND_GLYPH)
        .map(|i| unpack_fade(i.fade).0[2])
        .fold(0.0_f32, f32::max);
    assert!(glyph_fade > 0.0, "overflowing truncate_fade glyphs must fade on the right");

    for s_inst in &sel_insts {
        assert!(
            s_inst.clip[2] <= box_right_px + 0.5,
            "selection clip {:?} must stop at box right edge {box_right_px} (not bleed over hidden text)",
            s_inst.clip,
        );
        let sel_fade = unpack_fade(s_inst.fade).0[2];
        assert!(
            (sel_fade - glyph_fade).abs() <= 1.0,
            "selection right fade {sel_fade} must match overflowing text fade {glyph_fade}",
        );
    }

    // Decorations (span pill bg / underline) must also clip to the text box.
    for d_inst in rt.instances.iter().filter(|i| i.params[1] == KIND_RECT && approx(i.fill, want_pill)) {
        assert!(
            d_inst.clip[2] <= box_right_px + 0.5,
            "span decoration clip {:?} must stop at box right edge {box_right_px}",
            d_inst.clip,
        );
    }

    // The element's own background fill must NOT inherit the text-only truncate_fade fade.
    let bg_inst = rt.instances.iter()
        .find(|i| i.params[1] == KIND_RECT && approx(i.fill, want_bg))
        .expect("element background fill must paint");
    assert_eq!(unpack_fade(bg_inst.fade).0[2], 0.0, "box fill stays unfaded");

    // Text marks (Cx::mark_texts) must also clip and fade to the text box.
    let err_path = err_el.path.clone();
    rt.update(move |s, _| s.mark_path = Some(err_path));
    rt.key(KeyEvent { key: Key::Escape, mods: Mods::default() });
    rt.frame(2.0 / 60.0);
    let mark_insts: Vec<_> = rt.instances.iter()
        .filter(|i| i.params[1] == KIND_RECT && approx(i.fill, want_hl))
        .collect();
    assert!(!mark_insts.is_empty(), "text mark must paint for visible part");
    for m_inst in &mark_insts {
        assert!(
            m_inst.clip[2] <= box_right_px + 0.5,
            "mark clip {:?} must stop at box right edge {box_right_px}",
            m_inst.clip,
        );
        assert!(unpack_fade(m_inst.fade).0[2] > 0.0, "mark must fade with overflowing truncate_fade");
    }

    // Zero-width truncate_fade draws neither glyphs nor selection highlight.
    rt.update(|s, tx| { s.box_w = 0.0; s.mark_path = None; tx.instant(); });
    rt.frame(3.0 / 60.0);
    assert_eq!(rt.instances.iter().filter(|i| i.params[1] == KIND_GLYPH).count(), 0);
    assert_eq!(
        rt.instances.iter().filter(|i| i.params[1] == KIND_RECT && approx(i.fill, want_hl)).count(),
        0,
        "zero-width truncate_fade must emit zero selection rects",
    );
}

#[test]
fn hidden_text_by_opacity_layer_opacity_live_opacity_and_collapsed_clip_cannot_select_paint_or_copy() {
    use rune::render::KIND_RECT;
    use rune::{Color, Cursor, Key, KeyEvent, Live, Mods};

    let hl = rune::color::hex("#3080ff").unwrap().alpha(0.35);

    struct HiddenState {
        live_op: Live<f32>,
        hl: Color,
        seen_rects: std::rc::Rc<std::cell::RefCell<Vec<[f32; 4]>>>,
    }

    fn hidden_view(s: &HiddenState, cx: &mut Cx<HiddenState>) -> El {
        *s.seen_rects.borrow_mut() = cx.selection_rects();
        let style = TextStyle::new(14.0, 400.0).lh(20.0);
        let t = |content: &str, key: &str| {
            text(content.to_owned()).key(key).font(style).w_full().selectable(true).selection_color(s.hl)
        };
        col().size(400.0, 400.0).padding(10.0).selection_group(true)
            .child(t("visible top", "v_top"))
            .child(col().key("op0").opacity(0.0).child(t("secret opacity zero", "h_op")))
            .child(col().key("lop0").layer_opacity(0.0).child(t("secret layer opacity zero", "h_lop")))
            .child(col().key("lop0_round").layer_opacity(0.0).clip_rounded(8.0).child(t("secret layer opacity rounded", "h_lop_r")))
            .child(col().key("live0").live_opacity(&s.live_op).child(t("secret live opacity", "h_live")))
            .child(col().key("fold0").w_full().h(0.0).clip_soft(0.0).child(t("secret collapsed soft", "h_soft")))
            .child(col().key("fold0_r").w_full().h(0.0).clip_rounded(0.0).child(t("secret collapsed rounded", "h_round")))
            .child(t("visible bottom", "v_bot"))
    }

    let live_op = Live::new(0.0);
    let seen_rects = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
    let clip = rune::clipboard::memory();
    let mut rt = Runtime::new(
        HiddenState { live_op: live_op.clone(), hl, seen_rects: seen_rects.clone() },
        hidden_view,
        (400.0, 400.0),
        Env::new(true, 1.0),
    );
    rt.use_clipboard(&clip);
    rt.set_srgb_blend(false);
    rt.frame(0.0);

    // 1. Direct pointer click/hover on layer_opacity(0.0) and opacity(0.0) texts must NOT select or show I-beam.
    for key in ["h_op", "h_lop", "h_lop_r", "h_live"] {
        let r = rt.elements().into_iter().find(|e| e.key.as_deref() == Some(key)).unwrap().rect;
        let (x, y) = (r[0] + 10.0, r[1] + r[3] * 0.5);
        rt.pointer_move(x, y);
        rt.frame(1.0 / 60.0);
        assert_ne!(rt.cursor(), Cursor::Text, "{key}: hidden text must not show I-beam cursor");
        rt.pointer_down(x, y);
        rt.pointer_move(x + 40.0, y);
        rt.frame(2.0 / 60.0);
        rt.pointer_up(x + 40.0, y);
        rt.frame(3.0 / 60.0);
        assert_eq!(rt.selected_text(), None, "{key}: dragging on hidden text must select nothing");
    }

    // 2. Select All (Cmd+A) must select ONLY the two visible texts, skipping all hidden ones.
    rt.key(KeyEvent { key: Key::Command('a'), mods: Mods { cmd: true, ..Mods::default() } });
    rt.frame(4.0 / 60.0);
    assert_eq!(rt.selected_text().as_deref(), Some("visible top\nvisible bottom"));

    // Cmd+C copies only the visible texts.
    rt.key(KeyEvent { key: Key::Command('c'), mods: Mods { cmd: true, ..Mods::default() } });
    rt.frame(5.0 / 60.0);
    assert_eq!(clip.text().as_deref().map(String::as_str), Some("visible top\nvisible bottom"));

    // Cx::selection_rects returns only the 2 visible line boxes, none for hidden texts.
    assert_eq!(seen_rects.borrow().len(), 2, "selection_rects must exclude hidden texts: {:?}", *seen_rects.borrow());

    // Selection highlight instances: only 2 rects (for v_top and v_bot), none for hidden texts.
    let want_hl = hl.premul(1.0);
    let hl_count = rt.instances.iter()
        .filter(|i| i.params[1] == KIND_RECT && i.fill.iter().zip(want_hl).all(|(a, b)| (a - b).abs() < 1e-3))
        .count();
    assert_eq!(hl_count, 2, "only the 2 visible texts should paint selection highlights");

    // 3. Selecting a live_opacity(1.0) text and then hiding it via live_opacity.set(0.0) hides both
    // its painted selection and its selected/copied text.
    live_op.set(1.0);
    rt.frame(6.0 / 60.0);
    let r = rt.elements().into_iter().find(|e| e.key.as_deref() == Some("h_live")).unwrap().rect;
    let y = r[1] + r[3] * 0.5;
    rt.pointer_move(r[0] + 2.0, y);
    rt.pointer_down(r[0] + 2.0, y);
    rt.pointer_move(r[0] + 80.0, y);
    rt.pointer_up(r[0] + 80.0, y);
    rt.frame(7.0 / 60.0);
    assert!(rt.selected_text().as_deref().is_some_and(|s| s.contains("secret")), "visible live_opacity text selects");

    live_op.set(0.0);
    rt.frame(8.0 / 60.0);
    assert_eq!(rt.selected_text(), None, "hiding selected text via live_opacity(0.0) clears selected_text");
    let hl_after = rt.instances.iter()
        .filter(|i| i.params[1] == KIND_RECT && i.fill.iter().zip(want_hl).all(|(a, b)| (a - b).abs() < 1e-3))
        .count();
    assert_eq!(hl_after, 0, "hiding selected text via live_opacity(0.0) paints zero selection rects");
}

#[test]
fn scrolled_offscreen_clipped_cards_still_select_all_without_painting_offscreen_selection() {
    use rune::render::KIND_RECT;
    use rune::{Key, KeyEvent, Mods, ScrollSpec};

    let hl = rune::color::hex("#3080ff").unwrap().alpha(0.35);
    fn scroll_view(hl: &rune::Color, _: &mut Cx<rune::Color>) -> El {
        let style = TextStyle::new(14.0, 400.0).lh(20.0);
        col().size(300.0, 120.0).scroll(ScrollSpec::default()).selection_group(true)
            .child(col().h(60.0).clip_rounded(8.0).child(text("card one").font(style).selectable(true).selection_color(*hl)))
            .child(col().h(60.0).clip_rounded(8.0).child(text("card two").font(style).selectable(true).selection_color(*hl)))
            .child(col().h(60.0).clip_rounded(8.0).child(text("card three").font(style).selectable(true).selection_color(*hl)))
            .child(col().h(60.0).clip_rounded(8.0).child(text("card four").font(style).selectable(true).selection_color(*hl)))
    }
    let mut rt = Runtime::new(hl, scroll_view, (300.0, 120.0), Env::new(true, 1.0));
    rt.set_srgb_blend(false);
    rt.frame(0.0);
    rt.key(KeyEvent { key: Key::Command('a'), mods: Mods { cmd: true, ..Mods::default() } });
    rt.frame(1.0 / 60.0);
    assert_eq!(
        rt.selected_text().as_deref(),
        Some("card one\ncard two\ncard three\ncard four"),
        "Select All inside a scroll view keeps offscreen clip_rounded cards",
    );
    let want_hl = hl.premul(1.0);
    let hl_count = rt.instances.iter()
        .filter(|i| i.params[1] == KIND_RECT && i.fill.iter().zip(want_hl).all(|(a, b)| (a - b).abs() < 1e-3))
        .count();
    assert_eq!(hl_count, 2, "only the 2 visible cards inside the 120pt scroll viewport paint selection rects");
}

#[test]
fn partly_clipped_multiline_text_culls_selection_rects_outside_clip() {
    use rune::render::KIND_RECT;
    use rune::{Color, Key, KeyEvent, Mods};

    let hl = rune::color::hex("#3080ff").unwrap().alpha(0.35);
    fn clipped_view(hl: &Color, _: &mut Cx<Color>) -> El {
        let style = TextStyle::new(14.0, 400.0).lh(20.0);
        col().size(400.0, 200.0).padding(10.0).child(
            col().key("fold").w(200.0).h(40.0).clip_rounded(0.0).child(
                text("line one\nline two\nline three\nline four\nline five")
                    .key("multi")
                    .font(style)
                    .w(200.0)
                    .selectable(true)
                    .selection_color(*hl),
            ),
        )
    }
    let mut rt = Runtime::new(hl, clipped_view, (400.0, 200.0), Env::new(true, 1.0));
    rt.set_srgb_blend(false);
    rt.frame(0.0);
    rt.key(KeyEvent { key: Key::Command('a'), mods: Mods { cmd: true, ..Mods::default() } });
    rt.frame(1.0 / 60.0);

    let want_hl = hl.premul(1.0);
    let sel_rects: Vec<_> = rt.instances.iter()
        .filter(|i| i.params[1] == KIND_RECT && i.fill.iter().zip(want_hl).all(|(a, b)| (a - b).abs() < 1e-3))
        .collect();
    assert_eq!(sel_rects.len(), 2, "only the 2 visible lines inside the 40pt clip emit selection rects: {sel_rects:?}");
    for r in &sel_rects {
        assert!(r.clip[3] <= 50.5, "selection rect clip bottom {:?} stays within fold bottom 50", r.clip);
    }
}

#[test]
fn truncate_fade_inside_clip_rounded_keeps_selection_clipped_without_corrupting_corners() {
    use rune::render::{KIND_RECT, unpack_fade};
    use rune::{Color, Key, KeyEvent, Mods};

    let hl = rune::color::hex("#3080ff").unwrap().alpha(0.35);
    let msg = "Upstream request failed: very long single line error that overflows inside a rounded card";
    fn rounded_view(s: &(&'static str, Color), _: &mut Cx<(&'static str, Color)>) -> El {
        let style = TextStyle::new(14.0, 400.0).lh(20.0);
        col().size(400.0, 120.0).padding(10.0).child(
            col().w(240.0).h(80.0).padding(20.0).clip_rounded(8.0).child(
                text(s.0)
                    .key("err")
                    .font(style)
                    .w(160.0)
                    .truncate_fade()
                    .selectable(true)
                    .selection_color(s.1),
            ),
        )
    }
    let mut rt = Runtime::new((msg, hl), rounded_view, (400.0, 120.0), Env::new(true, 1.0));
    rt.set_srgb_blend(false);
    rt.frame(0.0);
    let err_el = rt.elements().into_iter().find(|e| e.key.as_deref() == Some("err")).unwrap();
    let box_right = err_el.rect[0] + err_el.rect[2];
    rt.key(KeyEvent { key: Key::Command('a'), mods: Mods { cmd: true, ..Mods::default() } });
    rt.frame(1.0 / 60.0);

    let want_hl = hl.premul(1.0);
    let sel_rects: Vec<_> = rt.instances.iter()
        .filter(|i| i.params[1] == KIND_RECT && i.fill.iter().zip(want_hl).all(|(a, b)| (a - b).abs() < 1e-3))
        .collect();
    assert!(!sel_rects.is_empty());
    for r in &sel_rects {
        assert!(r.clip[2] <= box_right + 0.5, "clip {:?} stops at {box_right}", r.clip);
        assert!(unpack_fade(r.fade).0[2] > 0.0, "right fade applied");
        assert_eq!(r.fade[3], 0, "text_clip sitting clear of rounded corners must not receive mask corner radii");
    }
}
