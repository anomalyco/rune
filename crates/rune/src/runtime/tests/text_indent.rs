//! Headless Runtime and CPU-boot pixel tests for `El::text_indent` and `El::set_text_indent`.

use super::*;
use crate::color::Color;
use crate::el::{col, text};
use crate::memo;
use crate::render::cpu_boot::{self, Scene, Target};
use crate::{Direction, Env, Span, TextStyle};

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

#[test]
fn el_text_indent_builder_and_setter_keep_plain_cold_empty_and_ignore_non_finite() {
    let plain = text("plain");
    assert!(plain.text.as_ref().unwrap().cold.is_empty());
    assert_eq!(plain.text.as_ref().unwrap().cold.text_indent, 0.0);

    // Setting 0.0 or -0.0 or non-finite leaves `cold` unallocated.
    let zero = text("zero").text_indent(0.0).text_indent(-0.0).text_indent(f32::NAN).text_indent(f32::INFINITY).text_indent(f32::NEG_INFINITY);
    assert!(zero.text.as_ref().unwrap().cold.is_empty());
    assert_eq!(zero.text.as_ref().unwrap().cold.text_indent, 0.0);

    // Builder and in-place setter store finite values and ignore subsequent non-finite writes.
    let mut el = text("indented").text_indent(18.5);
    assert_eq!(el.text.as_ref().unwrap().cold.text_indent, 18.5);
    el.set_text_indent(f32::NAN);
    assert_eq!(el.text.as_ref().unwrap().cold.text_indent, 18.5);
    el.set_text_indent(-42.0);
    assert_eq!(el.text.as_ref().unwrap().cold.text_indent, -42.0);
    el.set_text_indent(-0.0);
    assert_eq!(el.text.as_ref().unwrap().cold.text_indent.to_bits(), 0.0f32.to_bits());
}

#[test]
fn hanging_indent_with_pl_draws_into_padding_hits_selection_and_survives_max_lines() {
    const MSG: &str = "10:42 Ada: Compact chat message whose body wraps cleanly under the 56pt column without clipping.";
    struct S {
        max_lines: usize,
    }
    fn view(s: &S, _: &mut Cx<S>) -> El {
        let mut t = text(MSG)
            .key("msg")
            .w(240.0)
            .pl(56.0)
            .font(TextStyle::new(14.0, 400.0).lh(20.0))
            .color(Color::WHITE)
            .wrap()
            .selectable(true)
            .text_indent(-56.0);
        if s.max_lines > 0 {
            t = t.max_lines(s.max_lines);
        }
        col().key("root").padding(12.0).child(t)
    }

    let mut rt = Runtime::new(S { max_lines: 0 }, view, (320.0, 140.0), Env::new(true, 1.0));
    rt.frame(0.0);

    let msg_el = rt.elements().into_iter().find(|e| e.key.as_deref() == Some("msg")).unwrap();
    let [bx, by, _, _] = msg_el.rect;

    // Verify rendered pixels: ink appears inside the left padding strip on line 0 (`10:42`),
    // and stays absent in that left strip on line 1.
    let px = render_cpu(&rt, 320, 140);
    let line0_pad_ink = ink_in_rect(&px, 320, [bx as u32, by as u32, (bx + 44.0) as u32, (by + 20.0) as u32]);
    let line1_pad_ink = ink_in_rect(&px, 320, [bx as u32, (by + 20.0) as u32, (bx + 44.0) as u32, (by + 40.0) as u32]);
    let line1_body_ink = ink_in_rect(&px, 320, [(bx + 56.0) as u32, (by + 20.0) as u32, (bx + 180.0) as u32, (by + 40.0) as u32]);
    assert!(line0_pad_ink > 20, "line 0 hanging prefix must draw inside left padding (ink={line0_pad_ink})");
    assert_eq!(line1_pad_ink, 0, "line 1 must stay indented past left padding (ink={line1_pad_ink})");
    assert!(line1_body_ink > 20, "line 1 body must draw starting at pl(56) (ink={line1_body_ink})");

    // Drag across the hanging prefix inside the left padding strip to select "10:42".
    rt.pointer_down(bx + 1.0, by + 10.0);
    rt.pointer_move(bx + 42.0, by + 10.0);
    rt.pointer_up(bx + 42.0, by + 10.0);
    rt.frame(1.0 / 60.0);
    let (sel_path, sel_range) = rt.text_selection().expect("dragging across hanging prefix must select text");
    assert!(sel_path.ends_with("msg"), "expected msg selection path, got {sel_path}");
    let sel_text = &MSG[sel_range];
    assert!(
        sel_text.starts_with("10:42"),
        "expected selection starting with hanging timestamp '10:42', got {:?}",
        sel_text,
    );

    // Combining `max_lines(2)` with `pl(56).text_indent(-56)` must elide without clipping the hanging prefix.
    rt.state.max_lines = 2;
    rt.invalidate();
    rt.frame(2.0 / 60.0);
    let msg_node = rt.tree.nodes.iter().find(|n| n.key.as_deref() == Some("msg")).unwrap();
    assert!(msg_node.elided.is_some(), "expected 3+ line message to elide under max_lines(2)");
    let px_clamped = render_cpu(&rt, 320, 140);
    let clamped_pad_ink = ink_in_rect(&px_clamped, 320, [bx as u32, by as u32, (bx + 44.0) as u32, (by + 20.0) as u32]);
    assert!(clamped_pad_ink > 20, "max_lines must not clip hanging prefix in padding (ink={clamped_pad_ink})");
}

#[test]
fn rtl_hanging_indent_with_pl_draws_into_right_padding() {
    const ARABIC: &str = "مرحبا بالعالم هذه رسالة عربية طويلة تلتف إلى السطر الثاني مع مسافة بادئة معلقة";
    fn view(_: &(), _: &mut Cx<()>) -> El {
        col().key("root").padding(12.0).child(
            text(ARABIC)
                .key("rtl-msg")
                .dir(Direction::Rtl)
                .w(240.0)
                // In Rune, `pl` is logical inline-start padding and mirrors to physical right in RTL.
                .pl(44.0)
                .font(TextStyle::new(14.0, 400.0).lh(20.0))
                .color(Color::WHITE)
                .wrap()
                .text_indent(-44.0),
        )
    }

    let mut rt = Runtime::new((), view, (320.0, 140.0), Env::new(true, 1.0));
    rt.frame(0.0);
    let msg_el = rt.elements().into_iter().find(|e| e.key.as_deref() == Some("rtl-msg")).unwrap();
    let [bx, by, bw, _] = msg_el.rect;
    let right_pad_x0 = (bx + bw - 42.0) as u32;
    let right_pad_x1 = (bx + bw) as u32;

    let px = render_cpu(&rt, 320, 140);
    let line0_rpad = ink_in_rect(&px, 320, [right_pad_x0, by as u32, right_pad_x1, (by + 20.0) as u32]);
    let line1_rpad = ink_in_rect(&px, 320, [right_pad_x0, (by + 20.0) as u32, right_pad_x1, (by + 40.0) as u32]);
    assert!(line0_rpad > 15, "RTL hanging line 0 must draw into right padding (ink={line0_rpad})");
    assert_eq!(line1_rpad, 0, "RTL line 1 must stop before right padding (ink={line1_rpad})");
}

#[test]
fn memo_invalidation_and_dynamic_indent_change_update_layout_and_settle_idle() {
    const PROSE: &str = "Dynamic indent test paragraph that wraps cleanly across two lines.";
    struct S {
        indent: f32,
    }
    fn view(s: &S, _: &mut Cx<S>) -> El {
        let indent = s.indent;
        col().key("root").padding(12.0).gap(8.0)
            .child(
                memo("memo-box", indent.to_bits(), move || {
                    text(PROSE)
                        .w(220.0)
                        .pl(36.0)
                        .font(TextStyle::new(14.0, 400.0).lh(20.0))
                        .color(Color::WHITE)
                        .wrap()
                        .text_indent(indent)
                }),
            )
            .child(
                memo("memo-root-mod", 1u32, || {
                    text(PROSE)
                        .w(220.0)
                        .pl(36.0)
                        .font(TextStyle::new(14.0, 400.0).lh(20.0))
                        .color(Color::WHITE)
                        .wrap()
                })
                .text_indent(indent),
            )
    }

    let mut rt = Runtime::new(S { indent: 0.0 }, view, (300.0, 160.0), Env::new(true, 1.0));
    rt.frame(0.0);
    let first_line_offset = |rt: &Runtime<S>, key: &str| {
        let n = rt.tree.nodes.iter().find(|n| n.key.as_deref() == Some(key)).unwrap();
        n.text_layout.as_ref().unwrap().view().pieces().next().unwrap().layout.get(0).unwrap().metrics().offset
    };
    assert_eq!(first_line_offset(&rt, "memo-box"), 0.0);
    assert_eq!(first_line_offset(&rt, "memo-root-mod"), 0.0);

    rt.state.indent = 28.0;
    rt.invalidate();
    rt.frame(1.0 / 60.0);
    assert!((first_line_offset(&rt, "memo-box") - 28.0).abs() < 0.5);
    assert!((first_line_offset(&rt, "memo-root-mod") - 28.0).abs() < 0.5);

    rt.state.indent = -36.0;
    rt.invalidate();
    rt.frame(2.0 / 60.0);
    assert!((first_line_offset(&rt, "memo-box") - (-36.0)).abs() < 0.5);
    assert!((first_line_offset(&rt, "memo-root-mod") - (-36.0)).abs() < 0.5);

    rt.state.indent = 0.0;
    rt.invalidate();
    rt.frame(3.0 / 60.0);
    assert_eq!(first_line_offset(&rt, "memo-box"), 0.0);
    assert_eq!(first_line_offset(&rt, "memo-root-mod"), 0.0);
    assert!(!rt.frame(4.0 / 60.0), "idle window with settled text_indent must ask for no frame");
}

#[test]
fn accessibility_tree_html_export_and_dir_by_paragraph_respect_text_indent() {
    fn view(_: &(), _: &mut Cx<()>) -> El {
        col().key("root").w(320.0).padding(12.0).gap(8.0)
            .child(
                text("Indented first line of prose that wraps onto a second line.")
                    .key("indented")
                    .w(220.0)
                    .font(TextStyle::new(14.0, 400.0).lh(20.0))
                    .wrap()
                    .selectable(true)
                    .text_indent(24.0),
            )
            .child(
                text("Plain unindented prose.")
                    .key("plain")
                    .w(220.0)
                    .font(TextStyle::new(14.0, 400.0).lh(20.0))
                    .wrap()
                    .selectable(true),
            )
            .child(
                text("First English line
مرحبا بالعالم")
                    .key("bidi-paras")
                    .w(220.0)
                    .font(TextStyle::new(14.0, 400.0).lh(20.0))
                    .wrap()
                    .spans([Span::new(0, 5).bold()])
                    .text_indent(20.0)
                    .dir_by_paragraph(),
            )
    }

    let mut rt = Runtime::new((), view, (360.0, 240.0), Env::new(true, 1.0));
    rt.frame(0.0);

    // Accessibility tree keeps full text content and reflects the indented first run's x0.
    let tree = rt.accessibility_tree();
    let indented_runs: Vec<_> = tree.nodes.iter()
        .filter(|(_, n)| n.role() == accesskit::Role::TextRun && n.value().is_some_and(|v| v.starts_with("Indented")))
        .collect();
    let plain_runs: Vec<_> = tree.nodes.iter()
        .filter(|(_, n)| n.role() == accesskit::Role::TextRun && n.value().is_some_and(|v| v.starts_with("Plain")))
        .collect();
    assert!(!indented_runs.is_empty() && !plain_runs.is_empty());
    let ind_x0 = indented_runs[0].1.bounds().unwrap().x0;
    let pln_x0 = plain_runs[0].1.bounds().unwrap().x0;
    assert!((ind_x0 - pln_x0 - 24.0).abs() < 1.5, "a11y run x0 should shift by 24pt: indented={ind_x0}, plain={pln_x0}");

    // `dir_by_paragraph` keeps `text_indent` on paragraph 0 only; paragraph 1 stays 0.0.
    let p0 = rt.tree.nodes.iter().find(|n| n.key.as_deref() == Some("p0")).unwrap();
    let p1 = rt.tree.nodes.iter().find(|n| n.key.as_deref() == Some("p1")).unwrap();
    assert_eq!(p0.text.as_ref().unwrap().cold.text_indent, 20.0);
    assert_eq!(p1.text.as_ref().unwrap().cold.text_indent, 0.0);

    // HTML/CSS export emits `text-indent:24px` for indented text and omits it for plain text.
    let mut export_rt = Runtime::new((), view, (360.0, 240.0), Env::default());
    export_rt.page_mode();
    export_rt.frame(0.0);
    let nodes = export_rt.export_nodes();
    let ind_node = nodes.iter().find(|n| n.inner.starts_with("Indented")).unwrap();
    let pln_node = nodes.iter().find(|n| n.inner.starts_with("Plain")).unwrap();
    assert!(ind_node.decls.contains(&("text-indent", "24px".into())), "{:?}", ind_node.decls);
    assert!(!pln_node.decls.iter().any(|d| d.0 == "text-indent"), "{:?}", pln_node.decls);
}
