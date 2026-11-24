//! Headless Runtime and CPU-boot pixel tests for `SpanBox` (`El::span_box` / `El::span_boxes`)
//! and `El::overline` / `El::overlines`.

use super::*;
use crate::color::Color;
use crate::el::{col, rich, text, text_input};
use crate::render::cpu_boot::{self, Scene, Target};
use crate::{Direction, Env, Ime, Key, KeyEvent, Mods, Span, SpanBox, TextCase, TextEdit, TextStyle};

const KEY_BG: Color = Color { r: 0.22, g: 0.28, b: 0.42, a: 1.0 };
const VAL_BG: Color = Color { r: 0.14, g: 0.46, b: 0.82, a: 1.0 };
const RING_C: Color = Color { r: 0.55, g: 0.78, b: 1.0, a: 1.0 };
const OVER_C: Color = Color { r: 0.92, g: 0.22, b: 0.22, a: 1.0 };
const STRIKE_C: Color = Color { r: 0.2, g: 0.82, b: 0.32, a: 1.0 };
const UNDER_C: Color = Color { r: 0.22, g: 0.45, b: 0.95, a: 1.0 };

fn approx_color(inst: &[f32; 4], c: Color) -> bool {
    (inst[0] - c.r).abs() < 0.03
        && (inst[1] - c.g).abs() < 0.03
        && (inst[2] - c.b).abs() < 0.03
        && (inst[3] - c.a).abs() < 0.03
}

fn render_cpu<S: 'static>(rt: &Runtime<S>, w: u32, h: u32) -> Vec<u8> {
    let mut px = vec![0u8; (w * h * 4) as usize];
    let scene = Scene::new(&rt.instances, &rt.text.atlas, &rt.images, (w, h), [0.0; 4]);
    cpu_boot::paint(&scene, &mut Target::new(&mut px, w, h), 1).expect("cpu_boot paints");
    px
}

#[test]
fn span_box_grows_layout_emits_per_corner_radii_and_keeps_copy_bytes() {
    const BODY: &str = "search from:ada now";
    struct S {
        boxed: bool,
    }

    fn view(s: &S, _: &mut Cx<S>) -> El {
        let k0 = BODY.find("from:").unwrap();
        let k1 = k0 + "from:".len();
        let v1 = k1 + "ada".len();
        let mut t = text(BODY)
            .key("chips")
            .font(TextStyle::new(14.0, 400.0).lh(20.0))
            .color(Color::WHITE)
            .selectable(true);
        if s.boxed {
            t = t
                .span_box(k0..k1, SpanBox::new().bg(KEY_BG).pad_x(6.0, 4.0).corners([5.0, 0.0, 0.0, 5.0]))
                .span_boxes([(
                    k1..v1,
                    SpanBox::new()
                        .bg(VAL_BG)
                        .pad_x(4.0, 6.0)
                        .corners([0.0, 5.0, 5.0, 0.0])
                        .border(RING_C, 1.0),
                )]);
        }
        crate::el::row().key("root").padding(12.0).child(t)
    }

    let mut plain = Runtime::new(S { boxed: false }, view, (360.0, 80.0), Env::new(true, 1.0));
    plain.frame(0.0);
    let plain_w = plain.elements().into_iter().find(|e| e.key.as_deref() == Some("chips")).unwrap().rect[2];
    assert!(!plain.instances.iter().any(|q| q.params[1] == crate::render::KIND_RECT && approx_color(&q.fill, KEY_BG)));

    let mut rt = Runtime::new(S { boxed: true }, view, (360.0, 80.0), Env::new(true, 1.0));
    rt.frame(0.0);
    let boxed_w = rt.elements().into_iter().find(|e| e.key.as_deref() == Some("chips")).unwrap().rect[2];
    assert!(
        (boxed_w - plain_w - 20.0).abs() < 2.0,
        "layout width must grow by pad_left + pad_right (20 pt): plain={plain_w}, boxed={boxed_w}",
    );

    let key_inst = rt
        .instances
        .iter()
        .copied()
        .find(|q| q.params[1] == crate::render::KIND_RECT && approx_color(&q.fill, KEY_BG))
        .expect("key SpanBox rect emitted");
    let val_inst = rt
        .instances
        .iter()
        .copied()
        .find(|q| q.params[1] == crate::render::KIND_RECT && approx_color(&q.fill, VAL_BG))
        .expect("value SpanBox rect emitted");
    assert_eq!(key_inst.radii, [5.0, 0.0, 0.0, 5.0]);
    assert_eq!(val_inst.radii, [0.0, 5.0, 5.0, 0.0]);
    assert!((val_inst.params[0] - 1.0).abs() < 0.01, "inset border width=1px: {:?}", val_inst.params);
    assert!(approx_color(&val_inst.border, RING_C), "inset border color preserved");
    assert!(
        (key_inst.rect[0] + key_inst.rect[2] - val_inst.rect[0]).abs() < 0.01,
        "adjacent chips meet without overlapping: key={:?} val={:?}",
        key_inst.rect,
        val_inst.rect,
    );

    // Accessibility name and role stay unchanged (only the laid-out bounds widen by 20 pt).
    let a11y_plain = plain.accessibility_json();
    let a11y_boxed = rt.accessibility_json();
    assert_eq!(a11y_plain["children"][0]["children"][0]["name"], BODY);
    assert_eq!(a11y_boxed["children"][0]["children"][0]["name"], BODY);
    assert_eq!(a11y_boxed["children"][0]["children"][0]["role"], a11y_plain["children"][0]["children"][0]["role"]);

    // Selection + Cmd/Ctrl+C copies the exact authored bytes with zero extra padding chars.
    let rect = rt.elements().into_iter().find(|e| e.key.as_deref() == Some("chips")).unwrap().rect;
    let cy = rect[1] + rect[3] * 0.5;
    rt.pointer_down(rect[0] + 1.0, cy);
    rt.pointer_move(rect[0] + rect[2] - 1.0, cy);
    rt.pointer_up(rect[0] + rect[2] - 1.0, cy);
    rt.frame(0.0);
    assert_eq!(rt.selected_text().as_deref(), Some(BODY));
    rt.key(KeyEvent { key: Key::Command('c'), mods: Mods { cmd: true, ..Mods::default() } });
    rt.frame(0.0);
    let copied = rt.copy_request.as_ref().map(|w| w.content().text().to_owned());
    assert_eq!(copied.as_deref(), Some(BODY));

    // Off-screen CPU-boot render produces non-blank pixels inside both chip rects.
    let px = render_cpu(&rt, 360, 80);
    let sample_at = |x: f32, y: f32| -> [u8; 4] {
        let (ix, iy) = (x.round() as usize, y.round() as usize);
        let o = (iy * 360 + ix) * 4;
        [px[o], px[o + 1], px[o + 2], px[o + 3]]
    };
    let k_px = sample_at(key_inst.rect[0] + 2.0, key_inst.rect[1] + key_inst.rect[3] * 0.5);
    let v_px = sample_at(val_inst.rect[0] + val_inst.rect[2] - 3.0, val_inst.rect[1] + val_inst.rect[3] * 0.5);
    assert!(k_px[3] > 200, "key SpanBox pixels painted: {k_px:?}");
    assert!(v_px[3] > 200, "val SpanBox pixels painted: {v_px:?}");
    assert_ne!(k_px, v_px, "key and value chips paint distinct colours");
}

#[test]
fn span_box_validation_text_case_isolates_paragraphs_and_field_ignore() {
    // Editable fields ignore SpanBox like El::pill.
    let field = text_input(&TextEdit::new("from:ada"), TextStyle::new(14.0, 400.0))
        .span_box(0..5, SpanBox::new().bg(KEY_BG).pad(4.0));
    assert!(field.text.as_ref().unwrap().cold.span_boxes.is_empty());

    // Invalid ranges (empty, out-of-bounds, non-char-boundary, overlapping, no-op style) are skipped.
    let utf8 = "é from:ada";
    let e = text(utf8).span_boxes([
        (0..1, SpanBox::new().bg(KEY_BG)),                  // non-char boundary inside 'é'
        (3..3, SpanBox::new().bg(KEY_BG)),                  // empty
        (3..100, SpanBox::new().bg(KEY_BG)),                // out of bounds
        (3..8, SpanBox::new()),                             // no fill, no border, no pad -> no-op
        (3..8, SpanBox::new().bg(KEY_BG).pad(4.0)),         // valid
        (6..10, SpanBox::new().bg(VAL_BG).pad(4.0)),        // overlaps 3..8 -> skipped
        (8..utf8.len(), SpanBox::new().bg(VAL_BG).pad(4.0)), // adjacent -> valid
    ]);
    let boxes = &e.text.as_ref().unwrap().cold.span_boxes;
    assert_eq!(boxes.len(), 2);
    assert_eq!((boxes[0].start, boxes[0].end), (3, 8));
    assert_eq!((boxes[1].start, boxes[1].end), (8, utf8.len() as u32));

    // TextCase ("ß" -> "SS"), isolates, by_line, dir_by_paragraph, and truncate all remap cleanly.
    let view = |_: &(), _: &mut Cx<()>| {
        col()
            .gap(4.0)
            .child(
                text("ß from:ada")
                    .key("cased")
                    .text_case(TextCase::Uppercase)
                    .isolate(3..11, Direction::Ltr)
                    .span_box(3..8, SpanBox::new().bg(KEY_BG).pad(4.0).corners([4.0, 0.0, 0.0, 4.0])),
            )
            .child(
                text("from:ada\nمرحبا")
                    .key("bidi")
                    .w(240.0)
                    .wrap()
                    .span_box(0..5, SpanBox::new().bg(KEY_BG).pad(4.0))
                    .dir_by_paragraph(),
            )
            .child(
                text("line one\nfrom:ada line two")
                    .key("by-line")
                    .w(240.0)
                    .wrap()
                    .by_line()
                    .span_box(9..14, SpanBox::new().bg(VAL_BG).pad(4.0)),
            )
            .child(
                text("from:ada very long truncated label")
                    .key("trunc")
                    .w(110.0)
                    .truncate()
                    .span_box(0..5, SpanBox::new().bg(KEY_BG).pad(4.0)),
            )
    };
    let mut rt = Runtime::new((), view, (320.0, 220.0), Env::new(true, 1.0));
    rt.frame(0.0);
    let key_rects = rt.instances.iter().filter(|q| q.params[1] == crate::render::KIND_RECT && approx_color(&q.fill, KEY_BG)).count();
    let val_rects = rt.instances.iter().filter(|q| q.params[1] == crate::render::KIND_RECT && approx_color(&q.fill, VAL_BG)).count();
    assert_eq!(key_rects, 3, "cased + bidi + trunc all emit KEY_BG rects");
    assert_eq!(val_rects, 1, "by_line emits VAL_BG rect");
}

#[test]
fn overline_paints_above_strike_and_underline_wraps_and_renders_pixels() {
    const SRC: &str = "OVER plain\nwrap this overlined text at narrow widths";
    fn view(_: &(), _: &mut Cx<()>) -> El {
        col().padding(12.0).child(
            rich(SRC, vec![Span::new(0, 4).underline(UNDER_C)])
                .key("ov")
                .font(TextStyle::new(14.0, 400.0).lh(22.0))
                .color(Color::WHITE)
                .w(140.0)
                .wrap()
                .strike(0, 4, Some(STRIKE_C))
                .overline(0, 4, Some(OVER_C))
                .overlines([(11, SRC.len(), Some(OVER_C)), (5, 5, Some(OVER_C)), (9, 6, Some(OVER_C))]),
        )
    }
    let mut rt = Runtime::new((), view, (240.0, 140.0), Env::new(true, 1.0));
    rt.frame(0.0);

    let overs: Vec<_> = rt.instances.iter().filter(|q| q.params[1] == crate::render::KIND_RECT && approx_color(&q.fill, OVER_C)).collect();
    let strikes: Vec<_> = rt.instances.iter().filter(|q| q.params[1] == crate::render::KIND_RECT && approx_color(&q.fill, STRIKE_C)).collect();
    let unders: Vec<_> = rt.instances.iter().filter(|q| q.params[1] == crate::render::KIND_RECT && approx_color(&q.fill, UNDER_C)).collect();

    assert!(overs.len() >= 3, "OVER + >=2 wrapped overline segments, got {}", overs.len());
    assert_eq!(strikes.len(), 1);
    assert_eq!(unders.len(), 1);
    assert!(
        overs[0].rect[1] < strikes[0].rect[1] && strikes[0].rect[1] < unders[0].rect[1],
        "expected overline y={} < strike y={} < underline y={}",
        overs[0].rect[1],
        strikes[0].rect[1],
        unders[0].rect[1],
    );

    // Off-screen CPU-boot pixel render proves the overline draws red pixels at its y row.
    let px = render_cpu(&rt, 240, 140);
    let ox = (overs[0].rect[0] + overs[0].rect[2] * 0.5).round() as usize;
    let oy = overs[0].rect[1].round() as usize;
    let idx = (oy * 240 + ox) * 4;
    // cpu_boot writes BGRA8: red channel sits at +2.
    assert!(px[idx + 2] > 180 && px[idx + 3] > 200, "overline pixel drawn at ({ox}, {oy}): {:?}", &px[idx..idx + 4]);
}

#[test]
fn text_input_overlines_persist_across_rebuilds_and_shift_across_ime_preedit() {
    struct App {
        edit: TextEdit,
        secure: bool,
    }
    let view = |s: &App, _: &mut Cx<App>| {
        let mut field = text_input(&s.edit, TextStyle::new(14.0, 400.0).lh(20.0))
            .key("field")
            .w(240.0)
            .overline(2, 8, Some(OVER_C))
            .overlines([(8, 11, None)])
            .spans(vec![Span::new(8, 11).color(VAL_BG)])
            .color(Color::WHITE)
            .font(TextStyle::new(14.0, 400.0).lh(20.0));
        if s.secure {
            field = field.secure();
        }
        col().child(field)
    };

    let mut rt = Runtime::new(
        App { edit: TextEdit::new("hello world"), secure: false },
        view,
        (300.0, 80.0),
        Env::new(true, 1.0),
    );
    rt.frame(0.0);

    let overlines_of = |rt: &Runtime<App>| {
        rt.tree.nodes.iter().find(|n| n.key.as_deref() == Some("field")).unwrap()
            .text.as_ref().unwrap().cold.overlines.clone()
    };
    assert_eq!(overlines_of(&rt), vec![(2, 8, Some(OVER_C)), (8, 11, None)]);
    assert!(rt.instances.iter().any(|q| q.params[1] == crate::render::KIND_RECT && approx_color(&q.fill, OVER_C)));

    // Preedit before the range shifts it right by 3 bytes ("abc").
    rt.update(|s, _| {
        s.edit.focused = true;
        s.edit.set_caret(0);
        s.edit.ime(&Ime::Preedit("abc".into(), Some((3, 3)), Vec::new()));
    });
    rt.frame(0.0);
    assert_eq!(overlines_of(&rt), vec![(5, 11, Some(OVER_C)), (11, 14, None)]);

    // Preedit inside 2..8 splits it cleanly around the preedit.
    rt.update(|s, _| {
        s.edit.ime(&Ime::Preedit(String::new(), None, Vec::new()));
        s.edit.set_caret(5);
        s.edit.ime(&Ime::Preedit("abc".into(), Some((3, 3)), Vec::new()));
    });
    rt.frame(0.0);
    assert_eq!(overlines_of(&rt), vec![(2, 5, Some(OVER_C)), (8, 11, Some(OVER_C)), (11, 14, None)]);

    // Secure fields conceal overlines.
    rt.update(|s, _| {
        s.edit.ime(&Ime::Preedit(String::new(), None, Vec::new()));
        s.secure = true;
    });
    rt.frame(0.0);
    assert!(overlines_of(&rt).is_empty());
    assert!(!rt.instances.iter().any(|q| q.params[1] == crate::render::KIND_RECT && approx_color(&q.fill, OVER_C)));
}
