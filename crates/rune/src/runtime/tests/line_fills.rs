//! Headless Runtime tests for `LineFill` and `El::line_fills` / `El::line_fill`.

use super::*;
use crate::color::Color;
use crate::el::{col, text, text_input};
use crate::{Env, Ime, LineFill, TextEdit, TextStyle};

const CODE_BG: Color = Color { r: 0.15, g: 0.25, b: 0.45, a: 1.0 };
const QUOTE_BAR: Color = Color { r: 0.85, g: 0.45, b: 0.15, a: 1.0 };

fn approx_color(inst: &[f32; 4], c: Color) -> bool {
    (inst[0] - c.r).abs() < 0.03
        && (inst[1] - c.g).abs() < 0.03
        && (inst[2] - c.b).abs() < 0.03
        && (inst[3] - c.a).abs() < 0.03
}

#[test]
fn line_fills_paint_full_width_bg_and_left_bar_under_glyphs() {
    struct S {
        edit: TextEdit,
        fills: Vec<LineFill>,
        width: f32,
    }
    let doc = "intro\n```\nlet x = 1;\n\nlet y = 2;\n```\n> quoted note that wraps across lines when narrow";
    let code_start = doc.find("```").unwrap();
    let code_end = doc.rfind("```").unwrap() + 3;
    let quote_start = doc.find("> quoted").unwrap();
    let quote_end = doc.len();

    let view = |s: &S, _: &mut Cx<S>| {
        let mut field = text_input(&s.edit, TextStyle::new(14.0, 400.0).lh(20.0))
            .key("field")
            .w(s.width)
            .min_h(160.0)
            .padding(8.0);
        if let Some((first, rest)) = s.fills.split_first() {
            field = field.line_fill(*first).line_fills(rest.iter().copied());
        }
        col().key("root").padding(10.0).child(field)
    };

    let mut rt_plain = Runtime::new(
        S { edit: TextEdit::new(doc), fills: Vec::new(), width: 320.0 },
        view,
        (400.0, 260.0),
        Env::new(true, 1.0),
    );
    rt_plain.frame(0.0);
    assert!(!rt_plain.instances.iter().any(|q| q.params[1] == crate::render::KIND_RECT && approx_color(&q.fill, CODE_BG)));
    assert!(!rt_plain.instances.iter().any(|q| q.params[1] == crate::render::KIND_RECT && approx_color(&q.fill, QUOTE_BAR)));
    let a11y_plain = rt_plain.accessibility_tree();

    let fills = vec![
        LineFill::new(code_start, code_end).bg(CODE_BG).radius(8.0),
        LineFill::new(quote_start, quote_end).left_bar(3.0, QUOTE_BAR),
    ];
    let mut rt = Runtime::new(
        S { edit: TextEdit::new(doc), fills, width: 320.0 },
        view,
        (400.0, 260.0),
        Env::new(true, 1.0),
    );
    rt.frame(0.0);

    assert!(rt.state.edit.is_plain());
    assert_eq!(rt.state.edit.history_len(), (0, 0));
    let a11y_filled = rt.accessibility_tree();
    let val = |t: &accesskit::TreeUpdate| {
        t.nodes.iter().find(|(_, n)| n.role() == accesskit::Role::MultilineTextInput || n.role() == accesskit::Role::TextInput)
            .and_then(|(_, n)| n.value().map(str::to_owned))
    };
    assert_eq!(val(&a11y_filled), val(&a11y_plain));

    let bg_idx = rt.instances.iter().position(|q| q.params[1] == crate::render::KIND_RECT && approx_color(&q.fill, CODE_BG))
        .expect("code block LineFill::bg emits a rect");
    let bar_idx = rt.instances.iter().position(|q| q.params[1] == crate::render::KIND_RECT && approx_color(&q.fill, QUOTE_BAR))
        .expect("quote LineFill::left_bar emits a bar rect");
    let first_glyph_idx = rt.instances.iter().position(|q| q.params[1] == crate::render::KIND_GLYPH)
        .expect("field emits text glyphs");

    assert!(bg_idx < first_glyph_idx, "LineFill::bg ({bg_idx}) must paint under glyphs ({first_glyph_idx})");
    assert!(bar_idx < first_glyph_idx, "LineFill::left_bar ({bar_idx}) must paint under glyphs ({first_glyph_idx})");

    let bg = rt.instances[bg_idx];
    let bar = rt.instances[bar_idx];
    assert!((bg.rect[2] - 304.0).abs() < 1.5, "LineFill::bg spans full field inner width: {:?}", bg.rect);
    assert!((bg.radii[0] - 8.0).abs() < 0.01, "custom radius preserved: {:?}", bg.radii);
    assert!(bg.rect[3] >= 90.0, "multi-line code block height: {:?}", bg.rect);
    assert!((bar.rect[0] - bg.rect[0]).abs() < 0.5, "left_bar aligns with fill left edge: {:?} vs {:?}", bar.rect, bg.rect);
    assert!((bar.rect[2] - 3.0).abs() < 0.5, "left_bar width is 3pt: {:?}", bar.rect);
    assert!(bar.rect[1] >= bg.rect[1] + bg.rect[3] - 1.0, "quote bar starts below code block");

    rt.update(|s, _| s.width = 160.0);
    rt.frame(0.0);
    let tall_bar = rt.instances.iter().find(|q| q.params[1] == crate::render::KIND_RECT && approx_color(&q.fill, QUOTE_BAR)).unwrap();
    assert!(tall_bar.rect[3] > bar.rect[3] + 15.0, "wrapped quote bar grows vertically: {} -> {}", bar.rect[3], tall_bar.rect[3]);
}

#[test]
fn line_fills_shift_cleanly_across_ime_preedit_and_ignore_empty_or_secure_fields() {
    struct S {
        edit: TextEdit,
        code: (usize, usize),
        secure: bool,
    }
    let doc = "alpha\nline1\nline2\nomega";
    let code_start = doc.find("line1").unwrap();
    let code_end = doc.find("line2").unwrap() + "line2".len();

    let view = |s: &S, _: &mut Cx<S>| {
        let mut field = text_input(&s.edit, TextStyle::new(14.0, 400.0).lh(20.0))
            .key("field")
            .w(240.0)
            .line_fills([
                LineFill::new(s.code.0, s.code.1).bg(CODE_BG),
                LineFill::new(3, 3).bg(QUOTE_BAR),
                LineFill::new(8, 4).bg(QUOTE_BAR),
                LineFill::new(0, 5),
                LineFill::new(0, 5).left_bar(0.0, QUOTE_BAR),
            ]);
        if s.secure {
            field = field.secure();
        }
        col().key("root").child(field)
    };

    let mut rt = Runtime::new(
        S { edit: TextEdit::new(doc), code: (code_start, code_end), secure: false },
        view,
        (300.0, 160.0),
        Env::new(true, 1.0),
    );
    rt.frame(0.0);

    let fills_of = |rt: &Runtime<S>| -> Vec<LineFill> {
        rt.tree.nodes.iter().find(|n| n.key.as_deref() == Some("field")).unwrap().text.as_ref().unwrap().cold.line_fills.clone()
    };
    assert_eq!(fills_of(&rt).len(), 1, "only the valid visible LineFill is kept");
    assert_eq!(fills_of(&rt)[0].range(), code_start..code_end);

    rt.update(|s, _| {
        s.edit.focused = true;
        s.edit.set_caret(0);
        s.edit.ime(&Ime::Preedit("あ".into(), None, Vec::new()));
    });
    rt.frame(0.0);
    let shift = "あ".len();
    assert_eq!(fills_of(&rt)[0].range(), (code_start + shift)..(code_end + shift));

    rt.update(|s, _| {
        s.edit.ime(&Ime::Commit(String::new()));
        s.edit.set_caret(code_start + 2);
        s.edit.ime(&Ime::Preedit("漢字".into(), None, Vec::new()));
    });
    rt.frame(0.0);
    let inside_shift = "漢字".len();
    assert_eq!(fills_of(&rt).len(), 1, "IME preedit inside a LineFill keeps one contiguous fill");
    assert_eq!(fills_of(&rt)[0].range(), code_start..(code_end + inside_shift));
    let bg_count = rt.instances.iter().filter(|q| q.params[1] == crate::render::KIND_RECT && approx_color(&q.fill, CODE_BG)).count();
    assert_eq!(bg_count, 1, "one contiguous background rect during IME composition");

    rt.update(|s, _| {
        s.edit.ime(&Ime::Commit(String::new()));
        s.secure = true;
    });
    rt.frame(0.0);
    assert!(fills_of(&rt).is_empty(), "secure field must not retain line_fills");
    assert!(!rt.instances.iter().any(|q| q.params[1] == crate::render::KIND_RECT && approx_color(&q.fill, CODE_BG)));
}

#[test]
fn line_fills_work_on_static_and_rtl_text_and_empty_text_emits_nothing() {
    let view = |body: &String, _: &mut Cx<String>| {
        col().key("root").child(
            text(body.clone())
                .key("t")
                .font(TextStyle::new(14.0, 400.0).lh(20.0))
                .w(260.0)
                .wrap()
                .line_fill(LineFill::new(0, body.len()).bg(CODE_BG).left_bar(2.0, QUOTE_BAR)),
        )
    };
    let mut rt_empty = Runtime::new(String::new(), view, (300.0, 80.0), Env::new(true, 1.0));
    rt_empty.frame(0.0);
    assert!(!rt_empty.instances.iter().any(|q| q.params[1] == crate::render::KIND_RECT && approx_color(&q.fill, CODE_BG)));

    let mut rt_rtl = Runtime::new("שלום עולם\nשורה שנייה".into(), view, (300.0, 100.0), Env::new(true, 1.0));
    rt_rtl.frame(0.0);
    let bg = rt_rtl.instances.iter().find(|q| q.params[1] == crate::render::KIND_RECT && approx_color(&q.fill, CODE_BG)).unwrap();
    let bar = rt_rtl.instances.iter().find(|q| q.params[1] == crate::render::KIND_RECT && approx_color(&q.fill, QUOTE_BAR)).unwrap();
    assert!((bg.rect[2] - 260.0).abs() < 1.5);
    assert!(bg.rect[3] >= 35.0);
    assert!((bar.rect[2] - 2.0).abs() < 0.5);
}
