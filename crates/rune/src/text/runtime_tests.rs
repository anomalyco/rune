//! Text integration tests that exercise `Runtime`, `El`, and the GPU renderer alongside `rune-text`.

use crate::color::Color;
use crate::text::{FontSmoothing, Run, Span};
use crate::tok::TextStyle;

#[test]
fn advance_in_off_grapheme_draws_as_plain_text() {
    use crate::{Cx, El, Env, Runtime};
    fn view(_: &(), _: &mut Cx<()>) -> El {
        crate::el::row()
            .child(crate::el::text("y").advance_in(&Run::new("x\u{301}y"), 2))
            .child(crate::el::text("y").advance_in(&Run::new("y"), 99))
    }
    let mut rt = Runtime::new((), view, (200.0, 100.0), Env::new(true, 2.0));
    rt.frame(0.0);
    assert_eq!(rt.instances.iter().filter(|i| i.params[1] == crate::render::KIND_GLYPH).count(), 2);
}

/// A window narrowing re-breaks its wrapped texts without shaping them, and draws what a
/// window opened at that width draws.
#[test]
fn resizing_a_window_rebreaks_wrapped_text_and_draws_as_a_fresh_one() {
    use crate::{Cx, El, Env, Runtime, col};
    fn view(_: &(), _: &mut Cx<()>) -> El {
        let para = "Paragraph text long enough to wrap over several lines as the window narrows, with words of varied length.";
        col().w_full().padding(12.0).gap(8.0).children((0..6).map(|i| {
            let t = crate::el::text(format!("{i}. {para}")).w_full().wrap();
            if i % 2 == 0 { t.align(crate::Align::Center) } else { t }
        }))
    }
    let glyphs = |rt: &Runtime<()>| {
        rt.instances.iter().filter(|i| i.params[1] == crate::render::KIND_GLYPH).map(|i| i.rect.map(f32::to_bits)).collect::<Vec<_>>()
    };
    let mut rt = Runtime::new((), view, (700.0, 900.0), Env::new(true, 2.0));
    rt.frame(0.0);
    for w in [640.0, 517.5, 380.0, 700.0, 233.0] {
        rt.set_viewport((w, 900.0), 2.0);
        rt.frame(1.0 / 120.0);
        assert_eq!(rt.stats.shaped, 0, "at {w}");
        let mut fresh = Runtime::new((), view, (w, 900.0), Env::new(true, 2.0));
        fresh.frame(0.0);
        assert_eq!(glyphs(&rt), glyphs(&fresh), "at {w}");
    }
}

/// `Tx::set_font_smoothing` switches a running window: the next frame rasterises the new
/// mode's glyphs and draws them; asking for the current mode asks for no frame.
#[test]
fn a_handler_switches_font_smoothing_at_run_time() {
    use crate::{Cx, El, Env, Runtime};
    fn view(_: &(), _: &mut Cx<()>) -> El { crate::el::text("Hamburgefonstiv").size(13.0, 400.0) }
    let mut rt = Runtime::new((), view, (300.0, 100.0), Env::new(false, 2.0));
    rt.set_font_smoothing(FontSmoothing::On);
    rt.frame(0.0);
    let glyphs = |rt: &Runtime<()>| rt.instances.iter().filter(|i| i.params[1] == crate::render::KIND_GLYPH).map(|i| (i.rect, i.uv)).collect::<Vec<_>>();
    let before = (glyphs(&rt), rt.text.atlas.rasterized());
    assert!(!rt.update_if(|_, tx| { tx.set_font_smoothing(FontSmoothing::On); false }), "the current mode asks for no frame");
    assert!(rt.update_if(|_, tx| { tx.set_font_smoothing(FontSmoothing::Off); false }), "a new mode asks for a frame");
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.font_smoothing(), FontSmoothing::Off);
    assert_eq!(rt.text.apple_pipeline(), crate::text::ApplePipeline::UNSMOOTHED);
    assert!(rt.text.atlas.rasterized() > before.1, "Off rasterises its own glyphs");
    assert_ne!(glyphs(&rt), before.0, "and draws them");
    let srgb = rt.srgb_blend();
    assert!(rt.update_if(|_, tx| { tx.set_font_smoothing(FontSmoothing::On); false }));
    assert_eq!(rt.srgb_blend(), srgb, "blending is separate from the mode");
}

/// Glyph extents per drawn line, in device pixels, for one text element 300 pt wide at 2×.
fn line_spans(content: &str, align: crate::Align, justify: bool) -> Vec<(f32, f32)> {
    use crate::{Cx, El, Env, Runtime};
    type S = (String, crate::Align, bool);
    fn view(s: &S, _: &mut Cx<S>) -> El {
        let t = crate::el::text(s.0.clone()).w(300.0).wrap().align(s.1);
        if s.2 { t.justify() } else { t }
    }
    let mut rt = Runtime::new((content.to_string(), align, justify), view, (300.0, 400.0), Env::new(true, 2.0));
    rt.frame(0.0);
    let mut quads: Vec<[f32; 4]> =
        rt.instances.iter().filter(|i| i.params[1] == crate::render::KIND_GLYPH).map(|i| i.rect).collect();
    quads.sort_by(|a, b| (a[1] + a[3] / 2.0).total_cmp(&(b[1] + b[3] / 2.0)));
    let mut lines: Vec<(f32, f32, f32)> = Vec::new();
    for q in quads {
        let cy = q[1] + q[3] / 2.0;
        match lines.last_mut() {
            Some(l) if cy - l.0 < 16.0 => { l.1 = l.1.min(q[0]); l.2 = l.2.max(q[0] + q[2]); }
            _ => lines.push((cy, q[0], q[0] + q[2])),
        }
    }
    lines.into_iter().map(|(_, l, r)| (l, r)).collect()
}

#[test]
fn justified_text_fills_each_wrapped_line_but_the_last() {
    use crate::Align;
    let content = "the quick brown fox jumps over the lazy dog while five boxing wizards jump quickly and pack my box with five dozen liquor jugs";
    let justified = line_spans(content, Align::Start, true);
    assert!(justified.len() >= 3, "{justified:?}");
    for &(l, r) in &justified[..justified.len() - 1] {
        assert!(l < 8.0 && r > 592.0, "justified line spans {l}..{r}");
    }
    let start = line_spans(content, Align::Start, false);
    assert_eq!(start.len(), justified.len(), "justifying never rewraps");
    assert!(start[..start.len() - 1].iter().any(|&(_, r)| r < 592.0), "start lines stay ragged: {start:?}");
}

#[test]
fn wrapped_right_to_left_text_aligns_and_justifies_from_the_right() {
    use crate::Align;
    let hebrew = "שלום עולם זהו טקסט ארוך שנשבר לכמה שורות כדי לבדוק יישור מימין לשמאל בתוך רוחב קבוע של האלמנט";
    let start = line_spans(hebrew, Align::Start, false);
    if start.is_empty() {
        eprintln!("skipping: no Hebrew glyphs");
        return;
    }
    assert!(start.len() >= 2, "{start:?}");
    assert!(start.iter().all(|&(_, r)| r > 592.0), "wrapped RTL start hugs the right edge: {start:?}");
    let end = line_spans(hebrew, Align::End, false);
    assert!(end.iter().all(|&(l, _)| l < 8.0), "wrapped RTL end hugs the left edge: {end:?}");
    let justified = line_spans(hebrew, Align::Start, true);
    let last = justified.len() - 1;
    assert!(justified[..last].iter().all(|&(l, r)| l < 8.0 && r > 592.0), "{justified:?}");
    assert!(justified[last].1 > 592.0, "justified RTL last line starts at the right: {justified:?}");
}

#[test]
fn span_ring_subranges_case_elision_and_cache_changes() {
    use crate::{Cx, El, Env, Link, LinkPaint, Runtime, TextCase, col, rich};
    use crate::render::KIND_RECT;

    fn view(color: &Color, _: &mut Cx<Color>) -> El {
        col().child(rich("aa Straße zz", vec![Span::new(0, 13).bg(Color::WHITE, 3.0)])
            .span_ring(3, 10, *color, 1.0).text_case(TextCase::Uppercase)
            .link(Link::new(0..13, "whole").hover(LinkPaint::new().bg(Color::BLACK)))
            .on_link(|_: &mut Color, _, _| {}))
    }
    let mut rt = Runtime::new(Color::BLACK, view, (300.0, 80.0), Env::new(false, 1.0));
    rt.frame(0.0);
    let ring = *rt.instances.iter().find(|i| i.params[1] == KIND_RECT && i.params[0] == 1.0).unwrap();
    let rects: Vec<_> = rt.instances.iter().filter(|i| i.params[1] == KIND_RECT).collect();
    assert_eq!(rects.len(), 3, "ring boundaries split one background into three pieces");
    assert!(ring.rect[0] > rects[0].rect[0] && ring.rect[2] < rects.iter().map(|i| i.rect[2]).sum());
    let bounds = rt.elements().into_iter().find(|e| e.key.as_deref() == Some("whole")).unwrap().rect;
    rt.pointer_move(bounds[0] + 2.0, bounds[1] + 2.0);
    rt.frame(0.01);
    let hovered: Vec<_> = rt.instances.iter().filter(|i| i.params[1] == KIND_RECT).collect();
    assert_eq!(hovered.len(), 3, "hover recolors split backgrounds in place");
    assert_eq!(hovered.iter().filter(|i| i.params[0] == 1.0).count(), 1, "hover does not extend the ring outside its range");
    assert_eq!(hovered.iter().find(|i| i.params[0] == 1.0).unwrap().rect, ring.rect);
    assert!(hovered.iter().all(|i| i.fill == Color::BLACK.premul(1.0)));
    rt.pointer_move(299.0, 79.0);
    rt.state = Color::WHITE;
    rt.invalidate();
    rt.settle_all();
    rt.frame(0.0);
    let changed = rt.instances.iter().find(|i| i.params[1] == KIND_RECT && i.params[0] == 1.0).unwrap();
    assert_eq!(changed.border, Color::WHITE.premul(1.0), "ring color invalidates cached paint");
    assert_eq!(changed.rect, ring.rect);

    let mut cased = rich("İ xy", vec![Span::new(0, 5).bg(Color::WHITE, 2.0)])
        .span_ring(3, 5, Color::BLACK, 1.0).text_case(TextCase::Lowercase).text.unwrap();
    cased.apply_case();
    assert_eq!(cased.content, "i\u{307} xy");
    assert_eq!((cased.cold.rings[0].0, cased.cold.rings[0].1), (4, 6));
    assert_eq!((cased.spans[1].start, cased.spans[1].end), (4, 6));

    fn truncated(_: &(), _: &mut Cx<()>) -> El {
        col().w(60.0).child(rich("prefix middle suffix", vec![Span::new(14, 20).bg(Color::WHITE, 2.0)])
            .w(60.0).truncate_head().span_ring(14, 20, Color::BLACK, 1.0))
    }
    let mut cut = Runtime::new((), truncated, (60.0, 40.0), Env::new(false, 1.0));
    cut.frame(0.0);
    let rings: Vec<_> = cut.instances.iter().filter(|i| i.params[1] == KIND_RECT && i.params[0] == 1.0).collect();
    assert_eq!(rings.len(), 1, "head truncation remaps the suffix ring");
    assert!(rings[0].rect[0] >= 0.0 && rings[0].rect[2] <= 64.0);
}

#[test]
fn span_ring_without_background_is_empty_and_reveal_fades_the_chip() {
    use crate::{Cx, El, Env, Runtime, col, rich, text};
    use crate::render::KIND_RECT;
    fn plain(_: &(), _: &mut Cx<()>) -> El { text("plain").span_ring(0, 5, Color::BLACK, 1.0) }
    let mut plain = Runtime::new((), plain, (100.0, 40.0), Env::new(false, 1.0));
    plain.frame(0.0);
    assert!(!plain.instances.iter().any(|i| i.params[1] == KIND_RECT && i.params[0] > 0.0));

    fn view(s: &String, _: &mut Cx<String>) -> El {
        col().key("root").streaming(true).child(rich(s, vec![Span::new(0, s.len()).bg(Color::WHITE, 3.0)])
            .key("body").span_ring(0, s.len(), Color::BLACK, 1.0).reveal(0.2))
    }
    let mut rt = Runtime::new("one".to_string(), view, (300.0, 80.0), Env::new(false, 1.0));
    rt.frame(0.0);
    rt.state.push_str(" two");
    rt.invalidate();
    rt.frame(0.01);
    let initial = rt.instances.iter().find(|i| i.params[1] == KIND_RECT && i.params[0] == 1.0).unwrap().border[3];
    assert!(initial < 0.2, "new chip is not opaque before its final word: {initial}");
    rt.frame(0.11);
    let middle = rt.instances.iter().find(|i| i.params[1] == KIND_RECT && i.params[0] == 1.0).unwrap();
    assert!(middle.border[3] > initial && middle.border[3] < 1.0);
    assert_eq!(middle.border[3], middle.fill[3]);
    rt.frame(1.0);
    assert_eq!(rt.instances.iter().find(|i| i.params[1] == KIND_RECT && i.params[0] == 1.0).unwrap().border[3], 1.0);
}

#[test]
fn span_ring_draws_inset_border_on_light_page_merges_runs_wraps_and_stays_on_hover() {
    use crate::render::{Gpu, KIND_RECT, OFFSCREEN_FORMAT, Offscreen, Renderer};
    use crate::{Cx, El, Env, Link, LinkPaint, Runtime, col, rich};

    const PAGE: Color = Color { r: 0.96, g: 0.96, b: 0.97, a: 1.0 };
    const FILL: Color = Color { r: 0.95, g: 0.95, b: 0.96, a: 1.0 };
    const HOVER: Color = Color { r: 0.88, g: 0.90, b: 0.96, a: 1.0 };
    const RING: Color = Color { r: 0.12, g: 0.38, b: 0.88, a: 1.0 };

    fn view(alpha: &f32, _: &mut Cx<f32>) -> El {
        let content = " Try  bold code  now";
        let (a, b) = (1, content.find(" now").unwrap());
        let bold_a = content.find("bold").unwrap();
        let code_a = content.find("code").unwrap();
        let spans = vec![
            Span::new(a, b).bg(FILL, 6.0),
            Span::new(bold_a, bold_a + 4).bold(),
            Span::new(code_a, code_a + 4).mono(),
        ];
        col()
            .size(200.0, 44.0)
            .fill(PAGE)
            .padding(8.0)
            .child(
                rich(content, spans)
                    .font(TextStyle::new(14.0, 400.0))
                    .color(Color::BLACK)
                    .opacity(*alpha)
                    .span_ring(a, b, RING, 1.0)
                    .link(Link::new(a..b, "chip").hover(LinkPaint::new().bg(HOVER)))
                    .on_link(|_: &mut f32, _, _| {}),
            )
    }

    let mut rt = Runtime::new(1.0f32, view, (200.0, 44.0), Env::new(false, 2.0));
    rt.frame(0.0);
    let chips: Vec<_> = rt.instances.iter().filter(|i| i.params[1] == KIND_RECT && i.params[0] > 0.0).copied().collect();
    assert_eq!(chips.len(), 1, "plain, bold and mono runs merge into one ringed chip: {chips:?}");
    let chip = chips[0];
    assert_eq!((chip.radii, chip.params[0]), ([8.0; 4], 2.0), "4 pt radius and 1 pt ring at 2x");
    assert_eq!((chip.fill, chip.border), (FILL.premul(1.0), RING.premul(1.0)));

    // Offscreen GPU readback in a light theme where fill ≈ page colour: the ring is visible
    // at the inset edge while the interior shows the fill.
    let gpu = Gpu::headless();
    let size = (400u32, 88u32);
    let mut renderer = Renderer::new(&gpu.device, OFFSCREEN_FORMAT);
    let target = Offscreen::new(&gpu.device, size.0, size.1);
    renderer.prepare(&gpu.device, &gpu.queue, &mut rt.text.atlas, &rt.instances, size);
    let mut encoder = gpu.device.create_command_encoder(&Default::default());
    renderer.draw(&mut encoder, &target.view, [0.0; 4]);
    target.copy(&mut encoder);
    gpu.queue.submit([encoder.finish()]);
    let out = target.read(&gpu.device);
    let px = |x: usize, y: usize| -> [u8; 4] { out[(y * size.0 as usize + x) * 4..][..4].try_into().unwrap() };

    let [x0, y0, w, h] = chip.rect.map(|v| v as usize);
    let mid_x = x0 + w / 2;
    let mid_y = y0 + h / 2;
    let outside = px(mid_x, y0 - 2);
    let top_ring = px(mid_x, y0 + 1);
    let bot_ring = px(mid_x, y0 + h - 2);
    let left_ring = px(x0 + 1, mid_y);
    // Inside the left padding (past the 2 px ring, before the first glyph): fill ≈ page, not ring.
    let inside_fill = px(x0 + 5, mid_y);
    assert!(outside[0] > 235 && outside[2] > 235 && (outside[0] as i32 - outside[2] as i32).abs() <= 5, "page outside: {outside:?}");
    for (edge, p) in [("top", top_ring), ("bottom", bot_ring), ("left", left_ring)] {
        assert!(
            (p[0] as i32 - 97).abs() <= 2 && (p[1] as i32 - 166).abs() <= 2 && (p[2] as i32 - 241).abs() <= 2 && p[3] == 255,
            "{edge} inset ring pixel is ring-coloured: {p:?}"
        );
    }
    assert!(inside_fill[0] > 230 && inside_fill[2] > 230 && (inside_fill[2] as i32 - inside_fill[0] as i32) < 10, "interior pixel is fill, not ring: {inside_fill:?}");
    // Outside the 8 px rounded corner radius, the corner pixel stays on the page background.
    let corner_out = px(x0, y0);
    assert!(corner_out[0] > 235 && (corner_out[0] as i32 - corner_out[2] as i32).abs() <= 5, "rounded corner cuts outside pixel: {corner_out:?}");

    // A partial-range ring at fractional scale keeps its right edge even though the
    // following unringed background overlaps it by bg_pad.
    fn partial(_: &(), _: &mut Cx<()>) -> El {
        col().fill(PAGE).padding(8.0).child(
            rich("aa ring zz", vec![Span::new(0, 10).bg(FILL, 6.0)])
                .span_ring(3, 7, RING, 1.0),
        )
    }
    let mut partial_rt = Runtime::new((), partial, (200.0, 44.0), Env::new(false, 1.5));
    partial_rt.frame(0.0);
    let partial_chip = partial_rt.instances.iter().find(|i| i.params[1] == KIND_RECT && i.params[0] > 0.0).unwrap();
    renderer.prepare(&gpu.device, &gpu.queue, &mut partial_rt.text.atlas, &partial_rt.instances, size);
    let mut encoder = gpu.device.create_command_encoder(&Default::default());
    renderer.draw(&mut encoder, &target.view, [0.0; 4]);
    target.copy(&mut encoder);
    gpu.queue.submit([encoder.finish()]);
    let partial_out = target.read(&gpu.device);
    let x = (partial_chip.rect[0] + partial_chip.rect[2] - 1.0) as usize;
    let y = (partial_chip.rect[1] + partial_chip.rect[3] * 0.5) as usize;
    let edge = &partial_out[(y * size.0 as usize + x) * 4..][..4];
    assert!(edge[2] as i32 - edge[0] as i32 > 50, "fractional right ring survives adjacent fill: {edge:?}");

    // Hover replaces the fill with HOVER while keeping the resting ring on the same rect.
    let link_el = rt.elements().into_iter().find(|e| e.key.as_deref() == Some("chip")).unwrap();
    rt.pointer_move(link_el.rect[0] + 4.0, link_el.rect[1] + 4.0);
    rt.frame(1.0 / 60.0);
    let hovered = rt.instances.iter().find(|i| i.params[1] == KIND_RECT && i.params[0] > 0.0).unwrap();
    assert_eq!((hovered.rect, hovered.fill, hovered.border, hovered.params[0]), (chip.rect, HOVER.premul(1.0), RING.premul(1.0), 2.0));

    // Fading the text element scales both the fill and the ring border alpha.
    rt.pointer_move(0.0, 0.0);
    rt.state = 0.5;
    rt.invalidate();
    rt.settle_all();
    rt.frame(0.0);
    let faded = rt.instances.iter().find(|i| i.params[1] == KIND_RECT && i.params[0] > 0.0).unwrap();
    assert!((faded.fill[3] - FILL.premul(0.5)[3]).abs() < 1e-3 && (faded.border[3] - RING.premul(0.5)[3]).abs() < 1e-3);

    // Fractional scale (1.5x) and wrapping across lines: each wrapped line gets its own ringed chip.
    fn wrapped(_: &(), _: &mut Cx<()>) -> El {
        let s = "one long ringed link that wraps across lines";
        col().w(90.0).child(
            rich(s, vec![Span::new(4, s.len() - 13).bg(FILL, 3.0)])
                .font(TextStyle::new(14.0, 400.0))
                .span_rings([(4, s.len() - 13, RING, 1.0)]),
        )
    }
    let mut wrap_rt = Runtime::new((), wrapped, (90.0, 120.0), Env::new(false, 1.5));
    wrap_rt.frame(0.0);
    let lines: Vec<_> = wrap_rt.instances.iter().filter(|i| i.params[1] == KIND_RECT && i.params[0] > 0.0).collect();
    assert!(lines.len() >= 2, "wrapped span splits into one ringed rect per line: {lines:?}");
    for d in &lines {
        assert_eq!((d.radii, d.params[0], d.border), ([6.0; 4], 1.5, RING.premul(1.0)));
    }
}

/// Underline rects drawn at 2× for `words`, each its own underlined text element side by side
/// in a row; device px `(left, right)`, left to right.
fn underlines(words: &[&str]) -> Vec<(f32, f32)> {
    use crate::{Cx, El, Env, Runtime};
    fn view(s: &Vec<String>, _: &mut Cx<Vec<String>>) -> El {
        let line = crate::oklcha(0.6, 0.15, 260.0, 0.4);
        crate::row().children(s.iter().map(|w| crate::el::rich(w.clone(), vec![Span::new(0, w.len()).underline(line)])))
    }
    let mut rt = Runtime::new(words.iter().map(|w| w.to_string()).collect(), view, (600.0, 100.0), Env::new(true, 2.0));
    rt.frame(0.0);
    let mut out: Vec<(f32, f32)> = rt.instances.iter()
        .filter(|i| i.params[1] == crate::render::KIND_RECT && i.rect[3] == 2.0)
        .map(|i| (i.rect[0], i.rect[0] + i.rect[2]))
        .collect();
    out.sort_by(|a, b| a.0.total_cmp(&b.0));
    out
}

/// A link flowed a word an element underlines as one line: each word's line
/// meets the next one's at the space, with no gap where an element's width rounds up (t942).
#[test]
fn underlined_words_in_a_row_meet_without_gaps() {
    let lines = underlines(&["Watch ", "Kaizen ", "× ", "OpenCode ", "(4:12, ", "1080p60) ", "→"]);
    assert_eq!(lines.len(), 7, "{lines:?}");
    for w in lines.windows(2) {
        assert_eq!(w[0].1, w[1].0, "each line meets the next: {lines:?}");
    }
}
