//! Headless Runtime and CPU-boot pixel tests for `El::align_y` and `El::set_align_y`.

use std::cell::RefCell;

use super::*;
use crate::color::Color;
use crate::el::{col, el, text, text_input};
use crate::render::cpu_boot::{self, Scene, Target};
use crate::{Align, Direction, Env, Ime, Link, TextEdit, TextStyle};

fn render_cpu<S: 'static>(rt: &Runtime<S>, w: u32, h: u32) -> Vec<u8> {
    let mut px = vec![0u8; (w * h * 4) as usize];
    let scene = Scene::new(&rt.instances, &rt.text.atlas, &rt.images, (w, h), [0.0; 4]);
    cpu_boot::paint(&scene, &mut Target::new(&mut px, w, h), 1).expect("cpu_boot paints");
    px
}

fn ink_in_rect(px: &[u8], w: u32, [x0, y0, x1, y1]: [u32; 4]) -> usize {
    let mut lit = 0usize;
    for y in y0..y1 {
        for x in x0..x1 {
            let i = ((y * w + x) * 4) as usize;
            if px[i] > 16 || px[i + 1] > 16 || px[i + 2] > 16 {
                lit += 1;
            }
        }
    }
    lit
}

fn rect_of<S: 'static>(rt: &Runtime<S>, key: &str) -> [f32; 4] {
    rt.elements().into_iter().find(|e| e.key.as_deref() == Some(key)).expect("element").rect
}

#[test]
fn el_align_y_builder_and_setter_keep_default_cold_empty() {
    let plain = text("plain");
    assert!(plain.text.as_ref().unwrap().cold.is_empty());
    assert_eq!(plain.text.as_ref().unwrap().cold.align_y, Align::Start);

    // Setting Align::Start on a fresh element leaves `cold` unallocated.
    let explicit_start = text("start").align_y(Align::Start);
    assert!(explicit_start.text.as_ref().unwrap().cold.is_empty());
    assert_eq!(explicit_start.text.as_ref().unwrap().cold.align_y, Align::Start);

    let edit = TextEdit::new("hello");
    let field_default = text_input(&edit, TextStyle::new(14.0, 400.0).lh(20.0)).single_line();
    assert_eq!(field_default.text.as_ref().unwrap().cold.align_y, Align::Start);

    let mut field = field_default.align_y(Align::Center);
    assert_eq!(field.text.as_ref().unwrap().cold.align_y, Align::Center);
    field.set_align_y(Align::End);
    assert_eq!(field.text.as_ref().unwrap().cold.align_y, Align::End);
    field.set_align_y(Align::Start);
    assert_eq!(field.text.as_ref().unwrap().cold.align_y, Align::Start);

    // Non-text elements ignore align_y cleanly.
    let box_el = el().align_y(Align::Center);
    assert!(box_el.text.is_none());
}

#[test]
fn single_line_input_align_y_centres_glyphs_caret_ime_selection_and_hit_test() {
    struct S {
        centered: TextEdit,
        default: TextEdit,
        end: TextEdit,
        auto_h: TextEdit,
        manual_py: TextEdit,
        asym_pad: TextEdit,
        short_box: TextEdit,
        zero_box: TextEdit,
        scaled: TextEdit,
        watch: &'static str,
    }
    thread_local! {
        static SEEN_CARET: RefCell<Option<[f32; 4]>> = const { RefCell::new(None) };
        static SEEN_SEL: RefCell<Vec<[f32; 4]>> = const { RefCell::new(Vec::new()) };
    }
    fn style() -> TextStyle {
        TextStyle::new(14.0, 400.0).lh(20.0)
    }
    fn view(s: &S, cx: &mut Cx<S>) -> El {
        let st = style();
        SEEN_CARET.with_borrow_mut(|c| *c = cx.caret(s.watch));
        SEEN_SEL.with_borrow_mut(|b| *b = cx.field_selection_rects(s.watch));
        col().w_full().h_full().padding(10.0).gap(6.0)
            .child(
                text_input(&s.centered, st).single_line().key("centered").w(260.0).h(56.0).px(12.0)
                    .align_y(Align::Center).color(Color::WHITE).caret(Color::WHITE, Color { r: 0.2, g: 0.5, b: 1.0, a: 0.4 })
                    .on_edit(|s: &mut S| &mut s.centered),
            )
            .child(
                text_input(&s.default, st).single_line().key("default").w(260.0).h(56.0).px(12.0)
                    .color(Color::WHITE).caret(Color::WHITE, Color { r: 0.2, g: 0.5, b: 1.0, a: 0.4 })
                    .on_edit(|s: &mut S| &mut s.default),
            )
            .child(
                text_input(&s.end, st).single_line().key("end").w(260.0).h(56.0).px(12.0)
                    .align_y(Align::End).color(Color::WHITE).caret(Color::WHITE, Color { r: 0.2, g: 0.5, b: 1.0, a: 0.4 })
                    .on_edit(|s: &mut S| &mut s.end),
            )
            .child(
                text_input(&s.auto_h, st).single_line().key("auto").w(260.0).px(12.0)
                    .align_y(Align::Center).color(Color::WHITE)
                    .on_edit(|s: &mut S| &mut s.auto_h),
            )
            .child(
                text_input(&s.manual_py, st).single_line().key("manual-py").w(260.0).px(12.0).py(18.0)
                    .align_y(Align::Center).color(Color::WHITE)
                    .on_edit(|s: &mut S| &mut s.manual_py),
            )
            .child(
                text_input(&s.asym_pad, st).single_line().key("asym").w(260.0).h(56.0).px(12.0).pt(8.0).pb(4.0)
                    .align_y(Align::Center).color(Color::WHITE)
                    .on_edit(|s: &mut S| &mut s.asym_pad),
            )
            .child(
                text_input(&s.short_box, st).single_line().key("short").w(260.0).h(10.0).px(12.0)
                    .align_y(Align::Center).color(Color::WHITE)
                    .on_edit(|s: &mut S| &mut s.short_box),
            )
            .child(
                text_input(&s.zero_box, st).single_line().key("zero").w(260.0).h(0.0).px(12.0)
                    .align_y(Align::Center).color(Color::WHITE)
                    .on_edit(|s: &mut S| &mut s.zero_box),
            )
            .child(
                col().key("scaled-wrap").scale(1.5).origin(0.0, 0.0).child(
                    text_input(&s.scaled, st).single_line().key("scaled").w(160.0).h(56.0).px(12.0)
                        .align_y(Align::Center).color(Color::WHITE)
                        .on_edit(|s: &mut S| &mut s.scaled),
                ),
            )
    }

    for dpi in [1.0f32, 2.0] {
        let mut centered = TextEdit::new("Hello centered");
        centered.focused = true;
        centered.set_caret(5);
        let state = S {
            centered,
            default: TextEdit::new("Hello default"),
            end: TextEdit::new("Hello bottom"),
            auto_h: TextEdit::new("Auto height"),
            manual_py: TextEdit::new("Manual py"),
            asym_pad: TextEdit::new("Asymmetric"),
            short_box: TextEdit::new("Short"),
            zero_box: TextEdit::new("Zero"),
            scaled: TextEdit::new("Scaled input"),
            watch: "centered",
        };
        let (vw, vh) = (360.0, 560.0);
        let mut rt = Runtime::new(state, view, (vw, vh), Env::new(true, dpi));
        rt.frame(0.0);
        rt.frame(1.0 / 60.0);

        let cr = rect_of(&rt, "centered");
        let dr = rect_of(&rt, "default");
        let er = rect_of(&rt, "end");
        let ar = rect_of(&rt, "auto");
        let mr = rect_of(&rt, "manual-py");
        let asr = rect_of(&rt, "asym");
        let sr = rect_of(&rt, "short");
        let zr = rect_of(&rt, "zero");

        // Verify glyph vertical placement in rendered pixels at 1x.
        if dpi == 1.0 {
            let px = render_cpu(&rt, vw as u32, vh as u32);
            // Centered: 56pt box, 20pt line -> 18pt top/bottom margin, glyphs in [y+18, y+38].
            let c_top = ink_in_rect(&px, vw as u32, [cr[0] as u32 + 12, cr[1] as u32 + 1, (cr[0] + 150.0) as u32, cr[1] as u32 + 16]);
            let c_mid = ink_in_rect(&px, vw as u32, [cr[0] as u32 + 12, cr[1] as u32 + 18, (cr[0] + 150.0) as u32, cr[1] as u32 + 38]);
            let c_bot = ink_in_rect(&px, vw as u32, [cr[0] as u32 + 12, cr[1] as u32 + 40, (cr[0] + 150.0) as u32, cr[1] as u32 + 55]);
            assert_eq!(c_top, 0, "centered input must have no ink in top 16pt margin");
            assert!(c_mid > 30, "centered input must draw glyphs in middle 20pt band (ink={c_mid})");
            assert_eq!(c_bot, 0, "centered input must have no ink in bottom 16pt margin");

            // Default (Align::Start): unchanged top-aligned output.
            let d_top = ink_in_rect(&px, vw as u32, [dr[0] as u32 + 12, dr[1] as u32, (dr[0] + 150.0) as u32, dr[1] as u32 + 20]);
            let d_bot = ink_in_rect(&px, vw as u32, [dr[0] as u32 + 12, dr[1] as u32 + 24, (dr[0] + 150.0) as u32, dr[1] as u32 + 55]);
            assert!(d_top > 30, "default single-line input stays top-aligned (ink={d_top})");
            assert_eq!(d_bot, 0, "default single-line input draws nothing in bottom region");

            // Align::End: 36pt top margin, glyphs in [y+36, y+56].
            let e_top = ink_in_rect(&px, vw as u32, [er[0] as u32 + 12, er[1] as u32 + 1, (er[0] + 150.0) as u32, er[1] as u32 + 34]);
            let e_bot = ink_in_rect(&px, vw as u32, [er[0] as u32 + 12, er[1] as u32 + 36, (er[0] + 150.0) as u32, er[1] as u32 + 56]);
            assert_eq!(e_top, 0, "end-aligned input must have no ink in top 34pt");
            assert!(e_bot > 30, "end-aligned input must draw glyphs in bottom 20pt band (ink={e_bot})");
        }

        // Caret and IME area on centered field sit at y + 18pt.
        let caret = SEEN_CARET.with_borrow(|c| *c).expect("Cx::caret for centered field");
        let ime = rt.ime_area.expect("ime_area for centered field");
        assert!((caret[1] - (cr[1] + 18.0)).abs() < 0.75, "dpi={dpi}: caret={caret:?}, box={cr:?}");
        assert!((ime[1] - (cr[1] + 18.0)).abs() < 0.75, "dpi={dpi}: ime={ime:?}, box={cr:?}");
        assert!((caret[3] - 20.0).abs() < 0.75, "dpi={dpi}: caret height={caret:?}");

        // IME preedit target clause candidate area also sits at y + 18pt.
        rt.ime(Ime::Preedit("nihao".into(), Some((0, 5)), Vec::new()));
        rt.frame(1.0 / 60.0);
        let pre_ime = rt.ime_area.expect("ime_area during preedit");
        assert!((pre_ime[1] - (cr[1] + 18.0)).abs() < 0.75, "dpi={dpi}: preedit ime={pre_ime:?}, box={cr:?}");
        rt.ime(Ime::Preedit(String::new(), None, Vec::new()));
        rt.frame(1.0 / 60.0);

        // Selection rects (Cx::field_selection_rects) sit at y + 18pt.
        rt.update(|s, _| s.centered.set_selection(0, 5));
        rt.frame(1.0 / 60.0);
        let sel = SEEN_SEL.with_borrow(Clone::clone);
        assert_eq!(sel.len(), 1, "dpi={dpi}: single-line selection has 1 box: {sel:?}");
        assert!((sel[0][1] - (cr[1] + 18.0)).abs() < 0.75, "dpi={dpi}: sel={sel:?}, box={cr:?}");

        // Auto-height and manual-.py() fields get 0 extra shift (origin == top + inset_top).
        let origin_of = |key: &str| -> [f32; 2] {
            let i = rt.tree.nodes.iter().position(|n| n.key.as_deref() == Some(key)).unwrap();
            let n = &rt.tree.nodes[i];
            let spec = n.text.as_ref().unwrap();
            let flow = n.text_layout.as_ref().unwrap().view();
            links::origin(n, spec, flow, rt.drawn_box(i), dpi, 1.0)
        };
        assert!((origin_of("auto")[1] - ar[1]).abs() < 0.5, "auto-height field has 0 shift");
        assert!((origin_of("manual-py")[1] - (mr[1] + 18.0)).abs() < 0.5, "manual .py(18) field has 0 extra shift");
        // Asymmetric padding: content box [asr[1]+8, asr[1]+52] -> content_h 44, line 20 -> extra 24 -> +12 -> asr[1]+20.
        assert!((origin_of("asym")[1] - (asr[1] + 20.0)).abs() < 0.75, "asymmetric padding centres inside content box");
        // Short / zero height: clamped to 0 shift (never negative).
        assert!((origin_of("short")[1] - sr[1]).abs() < 0.5, "short box clamps to 0 shift");
        assert!((origin_of("zero")[1] - zr[1]).abs() < 0.5, "zero-height box clamps to 0 shift");

        // Hit-testing and text_offset_at work across the centered field (band and vertical margins).
        let mid_x = cr[0] + 40.0;
        let off_band = rt.text_offset_at("centered", mid_x, cr[1] + 28.0).unwrap();
        let off_top = rt.text_offset_at("centered", mid_x, cr[1] + 4.0).unwrap();
        let off_bot = rt.text_offset_at("centered", mid_x, cr[1] + 52.0).unwrap();
        assert!(off_band > 0 && off_band < rt.state.centered.text.len(), "off_band={off_band}");
        assert_eq!(off_top, off_band, "clicking top margin of single-line input hits same char");
        assert_eq!(off_bot, off_band, "clicking bottom margin of single-line input hits same char");

        // Real pointer click on the centered band moves caret to off_band.
        rt.pointer_move(mid_x, cr[1] + 28.0);
        rt.pointer_down(mid_x, cr[1] + 28.0);
        rt.pointer_up(mid_x, cr[1] + 28.0);
        rt.frame(1.0 / 60.0);
        assert_eq!(rt.state.centered.focus, off_band);

        // Scaled subtree (1.5x): text_offset_at hits interior cleanly.
        let (sc_rect, _) = rt.presented("scaled").unwrap();
        let sc_start = rt.text_offset_at("scaled", sc_rect[0] + 18.0, sc_rect[1] + sc_rect[3] * 0.5).unwrap();
        let sc_end = rt.text_offset_at("scaled", sc_rect[0] + sc_rect[2] - 2.0, sc_rect[1] + sc_rect[3] * 0.5).unwrap();
        assert_eq!(sc_start, 0);
        assert_eq!(sc_end, rt.state.scaled.text.len());
    }
}

#[test]
fn single_line_placeholder_and_placeholder_font_centre_vertically() {
    struct S {
        same_font: TextEdit,
        small_font: TextEdit,
        default_top: TextEdit,
    }
    fn view(s: &S, _: &mut Cx<S>) -> El {
        let st = TextStyle::new(16.0, 400.0).lh(20.0);
        let small = TextStyle::new(11.0, 400.0).lh(14.0);
        col().w_full().h_full().padding(10.0).gap(8.0)
            .child(
                text_input(&s.same_font, st).single_line().key("same").w(260.0).h(56.0).px(12.0)
                    .placeholder("Search notes…", Color::WHITE).align_y(Align::Center),
            )
            .child(
                text_input(&s.small_font, st).single_line().key("small").w(260.0).h(56.0).px(12.0)
                    .placeholder("Compact hint", Color::WHITE).placeholder_font(small).align_y(Align::Center),
            )
            .child(
                text_input(&s.default_top, st).single_line().key("top").w(260.0).h(56.0).px(12.0)
                    .placeholder("Top default", Color::WHITE),
            )
    }

    let state = S { same_font: TextEdit::new(""), small_font: TextEdit::new(""), default_top: TextEdit::new("") };
    let (vw, vh) = (320.0, 220.0);
    let mut rt = Runtime::new(state, view, (vw, vh), Env::new(true, 1.0));
    rt.frame(0.0);

    let px = render_cpu(&rt, vw as u32, vh as u32);
    let r_same = rect_of(&rt, "same");
    assert_eq!(ink_in_rect(&px, vw as u32, [r_same[0] as u32 + 12, r_same[1] as u32 + 1, (r_same[0] + 150.0) as u32, r_same[1] as u32 + 16]), 0);
    assert!(ink_in_rect(&px, vw as u32, [r_same[0] as u32 + 12, r_same[1] as u32 + 18, (r_same[0] + 150.0) as u32, r_same[1] as u32 + 38]) > 20);
    assert_eq!(ink_in_rect(&px, vw as u32, [r_same[0] as u32 + 12, r_same[1] as u32 + 40, (r_same[0] + 150.0) as u32, r_same[1] as u32 + 55]), 0);

    // Smaller placeholder_font (lh 14 in 56pt box -> offset 21pt, drawn in [y+21, y+35]).
    let r_small = rect_of(&rt, "small");
    assert_eq!(ink_in_rect(&px, vw as u32, [r_small[0] as u32 + 12, r_small[1] as u32 + 1, (r_small[0] + 150.0) as u32, r_small[1] as u32 + 19]), 0);
    assert!(ink_in_rect(&px, vw as u32, [r_small[0] as u32 + 12, r_small[1] as u32 + 21, (r_small[0] + 150.0) as u32, r_small[1] as u32 + 35]) > 15);
    assert_eq!(ink_in_rect(&px, vw as u32, [r_small[0] as u32 + 12, r_small[1] as u32 + 37, (r_small[0] + 150.0) as u32, r_small[1] as u32 + 55]), 0);

    // Default placeholder without align_y stays top-aligned.
    let r_top = rect_of(&rt, "top");
    assert!(ink_in_rect(&px, vw as u32, [r_top[0] as u32 + 12, r_top[1] as u32, (r_top[0] + 150.0) as u32, r_top[1] as u32 + 20]) > 20);
    assert_eq!(ink_in_rect(&px, vw as u32, [r_top[0] as u32 + 12, r_top[1] as u32 + 24, (r_top[0] + 150.0) as u32, r_top[1] as u32 + 55]), 0);
}

#[test]
fn rtl_secure_disabled_pills_and_static_text_honour_align_y() {
    struct S {
        rtl: TextEdit,
        secret: TextEdit,
        disabled: TextEdit,
        pilled: TextEdit,
        link_clicks: usize,
    }
    fn view(s: &S, _: &mut Cx<S>) -> El {
        let st = TextStyle::new(14.0, 400.0).lh(20.0);
        col().w_full().h_full().padding(10.0).gap(6.0)
            .child(
                text_input(&s.rtl, st).single_line().dir(Direction::Rtl).key("rtl").label("RTL").w(260.0).h(56.0).px(12.0)
                    .align_y(Align::Center).color(Color::WHITE)
                    .on_edit(|s: &mut S| &mut s.rtl),
            )
            .child(
                text_input(&s.secret, st).single_line().secure().key("secret").label("Secret").w(260.0).h(56.0).px(12.0)
                    .align_y(Align::Center).color(Color::WHITE)
                    .on_edit(|s: &mut S| &mut s.secret),
            )
            .child(
                text_input(&s.disabled, st).single_line().disabled(true).inert().key("disabled").label("Disabled").w(260.0).h(56.0).px(12.0)
                    .align_y(Align::Center).color(Color::WHITE)
                    .on_edit(|s: &mut S| &mut s.disabled),
            )
            .child(
                text_input(&s.pilled, st).single_line().key("pilled").label("Pilled").w(260.0).h(56.0).px(12.0)
                    .align_y(Align::Center).color(Color::WHITE)
                    .on_edit(|s: &mut S| &mut s.pilled),
            )
            .child(
                text("Click link here").font(st).key("static-link").w(260.0).h(56.0).px(12.0)
                    .align_y(Align::Center).color(Color::WHITE)
                    .link(Link::new(6..10, "go"))
                    .on_link(|s: &mut S, _, _| s.link_clicks += 1),
            )
    }

    let mut pilled = TextEdit::new("").rich();
    pilled.insert_pill("u:ada", "@ada");
    pilled.insert(" hello");
    let mut rtl = TextEdit::new("مرحبا بالعالم");
    rtl.focused = true;
    let state = S {
        rtl,
        secret: TextEdit::new("hunter2").secure(),
        disabled: TextEdit::new("Read only"),
        pilled,
        link_clicks: 0,
    };
    let (vw, vh) = (320.0, 340.0);
    let mut rt = Runtime::new(state, view, (vw, vh), Env::new(true, 1.0));
    rt.frame(0.0);

    let px = render_cpu(&rt, vw as u32, vh as u32);

    // RTL: glyphs centred vertically on the right side, IME/caret at y + 18.
    let rr = rect_of(&rt, "rtl");
    assert_eq!(ink_in_rect(&px, vw as u32, [rr[0] as u32, rr[1] as u32 + 1, (rr[0] + rr[2]) as u32, rr[1] as u32 + 16]), 0);
    assert!(ink_in_rect(&px, vw as u32, [(rr[0] + rr[2] * 0.5) as u32, rr[1] as u32 + 18, (rr[0] + rr[2]) as u32, rr[1] as u32 + 38]) > 15);
    assert!((rt.ime_area.unwrap()[1] - (rr[1] + 18.0)).abs() < 0.75);

    // Secure: masked bullets stay vertically centred (align_y not stripped by redaction).
    let sr = rect_of(&rt, "secret");
    assert_eq!(ink_in_rect(&px, vw as u32, [sr[0] as u32, sr[1] as u32 + 1, (sr[0] + sr[2]) as u32, sr[1] as u32 + 16]), 0);
    assert!(ink_in_rect(&px, vw as u32, [sr[0] as u32 + 12, sr[1] as u32 + 18, (sr[0] + 140.0) as u32, sr[1] as u32 + 38]) > 15);
    assert_eq!(ink_in_rect(&px, vw as u32, [sr[0] as u32, sr[1] as u32 + 40, (sr[0] + sr[2]) as u32, sr[1] as u32 + 55]), 0);

    // Disabled input renders centred and ignores pointer focus.
    let dr = rect_of(&rt, "disabled");
    assert!(ink_in_rect(&px, vw as u32, [dr[0] as u32 + 12, dr[1] as u32 + 18, (dr[0] + 140.0) as u32, dr[1] as u32 + 38]) > 15);
    rt.pointer_move(dr[0] + 24.0, dr[1] + 28.0);
    rt.pointer_down(dr[0] + 24.0, dr[1] + 28.0);
    rt.pointer_up(dr[0] + 24.0, dr[1] + 28.0);
    rt.frame(1.0 / 60.0);
    assert!(!rt.state.disabled.focused, "disabled input must not take focus");

    // Pill hit-testing aligns with the centred line: hits middle band, misses empty top margin.
    let pr = rect_of(&rt, "pilled");
    let pi = rt.tree.nodes.iter().position(|n| n.key.as_deref() == Some("pilled")).unwrap();
    assert_eq!(rt.core().input_pill_at(pi, pr[0] + 24.0, pr[1] + 28.0).as_deref(), Some("u:ada"));
    assert_eq!(rt.core().input_pill_at(pi, pr[0] + 24.0, pr[1] + 4.0), None);

    // Static text with inline link: clicking top margin misses, clicking centred band activates link.
    let lr = rect_of(&rt, "static-link");
    let li = rt.tree.nodes.iter().position(|n| n.key.as_deref() == Some("static-link")).unwrap();
    let link_box = rt.core().link_boxes(li, [lr[0], lr[1], lr[0] + lr[2], lr[1] + lr[3]])[0].1;
    let lx = (link_box[0] + link_box[2]) * 0.5;
    let ly = (link_box[1] + link_box[3]) * 0.5;
    assert!((link_box[1] - (lr[1] + 18.0)).abs() < 1.0, "link box centred: {link_box:?} vs {lr:?}");
    rt.pointer_move(lx, lr[1] + 4.0);
    rt.pointer_down(lx, lr[1] + 4.0);
    rt.pointer_up(lx, lr[1] + 4.0);
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.state.link_clicks, 0, "clicking empty top margin must not trigger link");
    rt.pointer_move(lx, ly);
    rt.pointer_down(lx, ly);
    rt.pointer_up(lx, ly);
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.state.link_clicks, 1, "clicking centred link glyphs must trigger link");
}

#[test]
fn accessibility_tree_web_mirror_html_export_and_idle_frame_respect_align_y() {
    struct S {
        centered: TextEdit,
        plain: TextEdit,
    }
    fn view(s: &S, _: &mut Cx<S>) -> El {
        let st = TextStyle::new(14.0, 400.0).lh(20.0);
        col().key("root").w(320.0).padding(12.0).gap(8.0)
            .child(
                text_input(&s.centered, st).single_line().key("centered-field").label("Centered field")
                    .w(240.0).h(56.0).align_y(Align::Center)
                    .on_edit(|s: &mut S| &mut s.centered),
            )
            .child(
                text_input(&s.plain, st).single_line().key("plain-field").label("Plain field")
                    .w(240.0).h(56.0)
                    .on_edit(|s: &mut S| &mut s.plain),
            )
            .child(
                text("Centered label").font(st).key("centered-text").w(240.0).h(56.0).align_y(Align::Center),
            )
            .child(
                text("Bottom label").font(st).key("bottom-text").w(240.0).h(56.0).align_y(Align::End),
            )
            .child(
                text("Default label").font(st).key("default-text").w(240.0).h(56.0),
            )
    }

    let state = S { centered: TextEdit::new("Alpha"), plain: TextEdit::new("Beta") };
    let mut rt = Runtime::new(state, view, (360.0, 340.0), Env::new(true, 1.0));
    rt.frame(0.0);

    // AccessKit TextRun bounds shift down by 18pt on the centred input vs 0pt on the default input.
    let tree = rt.accessibility_tree();
    let run_y = |prefix: &str| -> f64 {
        tree.nodes.iter()
            .find(|(_, n)| n.role() == accesskit::Role::TextRun && n.value().is_some_and(|v| v.starts_with(prefix)))
            .and_then(|(_, n)| n.bounds())
            .expect("TextRun bounds")
            .y0
    };
    let cr = rect_of(&rt, "centered-field");
    let pr = rect_of(&rt, "plain-field");
    assert!((run_y("Alpha") - f64::from(cr[1] + 18.0)).abs() < 1.0, "centred input a11y run y0");
    assert!((run_y("Beta") - f64::from(pr[1])).abs() < 1.0, "default input a11y run y0");

    // Web a11y mirror shifts static text rect vertically for align_y != Start.
    let items = rt.web_a11y_items();
    let web_item = |t: &str| items.iter().find(|it| it.text == t).expect("web a11y item");
    let ct_rect = rect_of(&rt, "centered-text");
    let bt_rect = rect_of(&rt, "bottom-text");
    let dt_rect = rect_of(&rt, "default-text");
    assert!((web_item("Centered label").rect[1] - (ct_rect[1] + 18.0)).abs() < 1.0);
    assert!((web_item("Bottom label").rect[1] - (bt_rect[1] + 36.0)).abs() < 1.0);
    assert!((web_item("Default label").rect[1] - dt_rect[1]).abs() < 1.0);

    // HTML/CSS export emits align-content for Center/End and omits it for default Start.
    let mut export_rt = Runtime::new(
        S { centered: TextEdit::new("Alpha"), plain: TextEdit::new("Beta") },
        view,
        (360.0, 340.0),
        Env::default(),
    );
    export_rt.page_mode();
    export_rt.frame(0.0);
    let nodes = export_rt.export_nodes();
    let find_export = |s: &str| nodes.iter().find(|n| n.inner == s).expect("export node");
    assert!(find_export("Centered label").decls.contains(&("align-content", "center".into())));
    assert!(find_export("Bottom label").decls.contains(&("align-content", "end".into())));
    assert!(!find_export("Default label").decls.iter().any(|d| d.0 == "align-content"));

    // Settled idle window requests no extra frames.
    assert!(!rt.frame(1.0 / 60.0), "idle window with align_y must ask for no frame");
}
