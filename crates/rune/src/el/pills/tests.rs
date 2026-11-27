use crate::color::Color;
use crate::el::{PillStyle, rich, text, text_input};
use crate::text::{Span, TextSystem};
use parley::Alignment;
use crate::{TextEdit, TextStyle};

const INK: Color = Color { r: 1.0, g: 0.0, b: 0.0, a: 1.0 };
const CHIP: Color = Color { r: 0.0, g: 0.0, b: 1.0, a: 0.2 };

#[test]
fn pills_record_sorted_chips_and_skip_bad_ranges() {
    let body = "ask @code-reviewer about @src/main.rs — é";
    let at = |s: &str| body.find(s).unwrap();
    let agent = at("@code-reviewer")..at("@code-reviewer") + 14;
    let file = at("@src/main.rs")..at("@src/main.rs") + 12;
    let e = text(body).pills([
        (file.clone(), PillStyle::new().bg(CHIP)),
        (agent.clone(), PillStyle::new().fg(INK)),
        (agent.start + 1..agent.end + 2, PillStyle::new()),
        (5..5, PillStyle::new()),
        (body.len() - 1..body.len(), PillStyle::new()),
        (0..body.len() + 1, PillStyle::new()),
    ]);
    let t = e.text.as_ref().unwrap();
    let got: Vec<(u32, u32, Option<Color>, Option<Color>)> = t.cold.pills.iter().map(|p| (p.start, p.end, p.bg, p.fg)).collect();
    assert_eq!(got, [(agent.start as u32, agent.end as u32, None, Some(INK)), (file.start as u32, file.end as u32, Some(CHIP), None)]);
}

#[test]
fn a_pill_colours_its_label_and_keeps_it_through_later_spans() {
    let body = "ask @reviewer to look";
    let e = rich(body, vec![Span::new(0, 3).bold()]).pill(4..13, PillStyle::new().fg(INK)).spans(vec![Span::new(14, 16).italic()]);
    let t = e.text.as_ref().unwrap();
    let ink: Vec<(u32, u32)> = t.spans.iter().filter(|s| s.color == Some(INK)).map(|s| (s.start, s.end)).collect();
    assert_eq!(ink, [(4, 13)]);
    assert!(t.spans.iter().any(|s| s.italic && (s.start, s.end) == (14, 16)));
}

#[test]
fn a_field_ignores_static_pills() {
    let e = text_input(&TextEdit::new("ask @reviewer"), TextStyle::new(14.0, 400.0)).pill(4..13, PillStyle::new().fg(INK));
    assert!(e.text.as_ref().unwrap().cold.pills.is_empty());
}

#[test]
fn a_pill_label_stays_on_one_line_and_takes_room() {
    let body = "please ask @recursive-self-improvement about it";
    let at = body.find('@').unwrap();
    let end = at + "@recursive-self-improvement".len();
    let mut ts = TextSystem::default();
    let plain = text(body).wrap();
    let pilled = text(body).wrap().pill(at..end, PillStyle::new());
    let mut width = |e: &crate::El, w| ts.layout_for_spec(&e.text.as_ref().unwrap().as_text_input(1.0, false), 1.0, w, Alignment::Start).full_width();
    let (wide_plain, wide_pill) = (width(&plain, None), width(&pilled, None));
    assert!(wide_pill > wide_plain + 4.0, "the chip's padding takes room: {wide_plain} vs {wide_pill}");
    // Narrow enough that plain text breaks at the label's hyphens, wide enough for the pill.
    let layout = ts.layout_for_spec(&pilled.text.as_ref().unwrap().as_text_input(1.0, false), 1.0, Some(210.0), Alignment::Start);
    let split = layout.lines().any(|l| {
        let r = l.text_range();
        r.start > at && r.start < end
    });
    assert!(!split, "a label that fits a line never splits across lines");
    let narrow = ts.layout_for_spec(&pilled.text.as_ref().unwrap().as_text_input(1.0, false), 1.0, Some(110.0), Alignment::Start);
    assert!(narrow.width() <= 110.5, "one wider than the line wraps inside it: {}", narrow.width());
}

fn image(w: u32, h: u32) -> crate::Pixels {
    let p = crate::Pixels::new();
    p.set(w, h, vec![255; (w * h * 4) as usize]);
    p
}

#[test]
fn an_inline_image_replaces_its_bytes_on_the_baseline_and_grows_the_line() {
    let body = "so $e^x$ grows";
    let style = TextStyle::new(14.0, 400.0).lh(20.0);
    let mut ts = TextSystem::default();
    let mut lay = |e: &crate::El, w| ts.layout_for_spec(&e.text.as_ref().unwrap().as_text_input(2.0, false), 2.0, w, Alignment::Start);
    let plain = lay(&text(body).font(style), None);
    let flat = text(body).font(style).inline_image(3..8, crate::InlineImage::new(image(40, 20), 20.0, 8.0, 2.0));
    let with = lay(&flat, None);
    let without_label = lay(&text("so  grows").font(style), None);
    // The bytes take no room: only the image's 20 points (40 px at 2x) replace them.
    let grew = with.full_width() - without_label.full_width();
    assert!((grew - 40.0).abs() < 1.5, "{grew}");
    assert!(with.full_width() < plain.full_width() + 40.0);
    // Short enough to sit inside the line: its height stays 20 points.
    assert_eq!(with.height(), plain.height());
    let mut boxes = Vec::new();
    let t = flat.text.as_ref().unwrap();
    crate::text::bare_icons(&with, &t.cold.pills, 2.0, [0.0, 0.0], &mut boxes);
    let baseline = with.lines().next().unwrap().metrics().baseline;
    let [(0, at, size)] = boxes[..] else { panic!("{boxes:?}") };
    assert_eq!(size, [40.0, 20.0]);
    // Its top is its ascent above the baseline.
    assert!((at[1] + 16.0 - baseline).abs() <= 1.0, "{at:?} vs baseline {baseline}");
    // A tall, deep one grows the line both ways.
    let tall = lay(&text(body).font(style).inline_image(3..8, crate::InlineImage::new(image(40, 60), 20.0, 18.0, 12.0)), None);
    assert!(tall.height() >= 60.0, "{}", tall.height());
}

#[test]
fn an_inline_image_too_wide_for_the_rest_of_a_line_moves_whole_to_the_next() {
    let body = "some words then $\\frac{a}{b} + c$ after";
    let at = body.find('$').unwrap();
    let end = body.rfind('$').unwrap() + 1;
    let style = TextStyle::new(14.0, 400.0).lh(20.0);
    let mut ts = TextSystem::default();
    let e = text(body).font(style).wrap().inline_image(at..end, crate::InlineImage::new(image(120, 40), 60.0, 14.0, 6.0));
    let w = ts.layout_for_spec(&e.text.as_ref().unwrap().as_text_input(1.0, false), 1.0, None, Alignment::Start).full_width();
    let narrow = ts.layout_for_spec(&e.text.as_ref().unwrap().as_text_input(1.0, false), 1.0, Some(w - 50.0), Alignment::Start);
    let starts: Vec<usize> = narrow.lines().map(|l| l.text_range().start).collect();
    assert!(starts.contains(&at), "the image starts a line: {starts:?}");
    assert!(!starts.iter().any(|&s| s > at && s < end), "never split: {starts:?}");
}

// In a running app: selection, copy, wrapping and streaming over pilled text.

mod live {
    use std::ops::Range;

    use crate::el::{PillStyle, col, text};
    use crate::{Cx, El, Env, Key, KeyEvent, Mods, Runtime, TextStyle};

    struct S {
        body: String,
        pills: Vec<Range<usize>>,
        width: f32,
    }

    fn view(s: &S, _: &mut Cx<S>) -> El {
        let t = text(s.body.clone()).key("t").font(TextStyle::new(14.0, 400.0).lh(20.0)).w(s.width).wrap().selectable(true);
        col().key("root").padding(10.0).child(t.pills(s.pills.iter().map(|r| (r.clone(), PillStyle::new()))))
    }

    fn runtime(body: &str, pills: &[Range<usize>], width: f32) -> Runtime<S> {
        let s = S { body: body.into(), pills: pills.to_vec(), width };
        let mut rt = Runtime::new(s, view, (600.0, 400.0), Env::new(true, 1.0));
        rt.frame(0.0);
        rt
    }

    fn rect(rt: &Runtime<S>) -> [f32; 4] {
        rt.elements().into_iter().find(|e| e.key.as_deref() == Some("t")).unwrap().rect
    }

    /// Each line's byte range in the text's layout.
    fn lines(rt: &Runtime<S>) -> Vec<Range<usize>> {
        let n = rt.tree.nodes.iter().find(|n| n.key.as_deref() == Some("t")).unwrap();
        n.text_layout.as_ref().and_then(crate::text::TextLayout::single).unwrap().lines().map(|l| l.text_range()).collect()
    }

    fn drag(rt: &mut Runtime<S>, from: (f32, f32), to: (f32, f32)) {
        rt.pointer_down(from.0, from.1);
        rt.pointer_move(to.0, to.1);
        rt.pointer_up(to.0, to.1);
        rt.frame(0.0);
    }

    #[test]
    fn a_selection_and_copy_across_a_pill_give_its_label_text() {
        let body = "ask @reviewer to look";
        let mut rt = runtime(body, &[4..13], 400.0);
        let r = rect(&rt);
        let y = r[1] + r[3] * 0.5;
        drag(&mut rt, (r[0] + 1.0, y), (r[0] + r[2] - 1.0, y));
        assert_eq!(rt.selected_text().as_deref(), Some(body));
        // The highlight follows the selected range, which covers the whole label.
        let (_, range) = rt.text_selection().unwrap();
        assert!(range.start <= 4 && range.end >= 13, "{range:?}");
        rt.key(KeyEvent { key: Key::Command('c'), mods: Mods { cmd: true, ..Mods::default() } });
        rt.frame(0.0);
        let copied = rt.copy_request.as_ref().map(|w| w.content().text().to_owned());
        assert_eq!(copied.as_deref(), Some(body));
    }

    #[test]
    fn every_pill_label_stays_on_one_line() {
        let body = "ping @recursive-self-improvement and @code-review-standards-and-spec today";
        let at = |s: &str| body.find(s).unwrap()..body.find(s).unwrap() + s.len();
        let pills = [at("@recursive-self-improvement"), at("@code-review-standards-and-spec")];
        let rt = runtime(body, &pills, 260.0);
        let ls = lines(&rt);
        assert!(ls.len() >= 2, "narrow enough to wrap: {ls:?}");
        let n = rt.tree.nodes.iter().find(|n| n.key.as_deref() == Some("t")).unwrap();
        let t = n.text.as_ref().unwrap();
        for (authored, chip) in pills.iter().zip(&t.cold.pills) {
            // Layout ranges include each pill's bidi isolate marks.
            let p = chip.start as usize..chip.end as usize;
            assert_eq!(&t.content[p.clone()], &body[authored.clone()]);
            assert!(!ls.iter().any(|l| l.start > p.start && l.start < p.end), "{p:?} splits across {ls:?}");
        }
    }

    #[test]
    fn pills_survive_streaming_appends_and_bad_ranges_are_ignored() {
        let mut rt = runtime("ask @reviewer", &[4..13], 400.0);
        let pilled = |rt: &Runtime<S>| {
            let n = rt.tree.nodes.iter().find(|n| n.key.as_deref() == Some("t")).unwrap();
            let t = n.text.as_ref().unwrap();
            t.cold.pills.iter().map(|p| (p.start..p.end, t.content[p.start as usize..p.end as usize].to_owned())).collect::<Vec<_>>()
        };
        let mut w = rect(&rt)[2];
        for more in [" to", " look at", " the é diff"] {
            rt.update(|s, _| s.body.push_str(more));
            rt.frame(1.0 / 60.0);
            assert_eq!(pilled(&rt), [(7..16, "@reviewer".into())], "after {more:?}");
            assert!(rect(&rt)[2] >= w);
            w = rect(&rt)[2];
        }
        // Past the end, inside "é", or empty: skipped, and the frame still draws.
        let e = rt.state.body.find('é').unwrap();
        rt.update(|s, _| s.pills = vec![4..13, e + 1..e + 2, 30..400, 20..20]);
        rt.frame(2.0 / 60.0);
        assert_eq!(pilled(&rt), [(7..16, "@reviewer".into())]);
        rt.update(|s, _| s.body.truncate(8));
        rt.frame(3.0 / 60.0);
        assert!(pilled(&rt).is_empty(), "a range the text no longer reaches is dropped");
    }

    fn animated_apng(first: [u8; 4], second: [u8; 4], delay_ms: u16) -> crate::Pixels {
        let mut out = Vec::new();
        let mut enc = png::Encoder::new(&mut out, 2, 2);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        enc.set_animated(2, 0).unwrap();
        enc.set_frame_delay(delay_ms, 1000).unwrap();
        let mut w = enc.write_header().unwrap();
        w.write_image_data(&first.repeat(4)).unwrap();
        w.write_image_data(&second.repeat(4)).unwrap();
        w.finish().unwrap();
        let cache = crate::ImageCache::new(std::sync::Arc::new(|| {}));
        let img = cache.load(out);
        img.wait().unwrap();
        img.pixels().clone()
    }

    fn placed_rgba<T: 'static>(rt: &Runtime<T>) -> Option<[u8; 4]> {
        let inst = rt.instances.iter().find(|q| q.params[1] == crate::render::KIND_IMAGE)?;
        let page = &rt.images.pages[inst.params[0] as usize];
        let (x, y) = (inst.uv[0] as usize, inst.uv[1] as usize);
        let at = (y * page.width as usize + x) * 4;
        Some(page.data[at..at + 4].try_into().unwrap())
    }

    #[test]
    fn an_animated_inline_image_advances_frames_and_stops_waking_off_screen() {
        let red = [255, 0, 0, 255];
        let green = [0, 255, 0, 255];
        let px = animated_apng(red, green, 100);

        struct Anim {
            px: crate::Pixels,
            as_icon: bool,
            prefix_lines: usize,
            cached: bool,
            mounted: bool,
        }
        fn anim_view(s: &Anim, _: &mut Cx<Anim>) -> El {
            let mut body = "line
".repeat(s.prefix_lines);
            let at = body.len();
            body.push_str("go");
            let style = TextStyle::new(14.0, 400.0).lh(20.0);
            let t = if s.as_icon {
                text(body).key("t").font(style).inline_icon(at, crate::InlineIcon::image(s.px.clone()).size(12.0))
            } else {
                text(body).key("t").font(style).inline_image(at..at + 2, crate::InlineImage::new(s.px.clone(), 12.0, 10.0, 2.0))
            };
            let mut inner = col().key("inner").w(200.0);
            if s.mounted {
                inner = inner.child(t);
            }
            if s.cached {
                inner = inner.cache();
            }
            col().key("scroll").size(200.0, 40.0).scroll(crate::ScrollSpec { stick_bottom: false }).child(inner)
        }

        // Both El::inline_image and El::inline_icon(InlineIcon::image) advance frames and arm wakes.
        for as_icon in [false, true] {
            let mut rt = Runtime::new(
                Anim { px: px.clone(), as_icon, prefix_lines: 0, cached: false, mounted: true },
                anim_view,
                (200.0, 40.0),
                Env::new(true, 1.0),
            );
            rt.frame(0.0);
            assert_eq!(placed_rgba(&rt), Some(red), "as_icon={as_icon}: frame 0");
            let wake0 = rt.wake_deadline().expect("animated inline image arms a wake for frame 1");
            assert!((wake0 - 0.1).abs() < 1e-4, "as_icon={as_icon}: wake0={wake0}");

            rt.frame(0.1);
            assert_eq!(placed_rgba(&rt), Some(green), "as_icon={as_icon}: frame 1");
            let wake1 = rt.wake_deadline().expect("animated inline image arms a wake for next loop");
            assert!((wake1 - 0.2).abs() < 1e-4, "as_icon={as_icon}: wake1={wake1}");

            // Hidden window asks for no frames.
            rt.set_live_visible(false);
            rt.frame(0.1);
            assert_eq!(rt.wake_deadline(), None, "as_icon={as_icon}: hidden window must not wake");
            rt.set_live_visible(true);

            // Clipped off screen inside a still-visible tall text element: draws and wakes nothing.
            rt.update(|s, _| s.prefix_lines = 10);
            rt.frame(0.0);
            assert_eq!(placed_rgba(&rt), None, "as_icon={as_icon}: clipped inline image draws nothing");
            assert_eq!(rt.wake_deadline(), None, "as_icon={as_icon}: off-screen inline image must not wake");

            // Back in view it wakes again; with its text unmounted it asks for nothing.
            rt.update(|s, _| s.prefix_lines = 0);
            rt.frame(0.0);
            assert!(rt.wake_deadline().is_some(), "as_icon={as_icon}: back in view, it wakes again");
            rt.update(|s, _| s.mounted = false);
            // Its exit fade may still draw it; once that ends, nothing is left.
            for _ in 0..30 { rt.frame(0.1); }
            assert_eq!(placed_rgba(&rt), None, "as_icon={as_icon}: unmounted inline image draws nothing");
            assert_eq!(rt.wake_deadline(), None, "as_icon={as_icon}: unmounted inline image must not wake");
        }

        // Under El::cache(), an animated inline image draws live instead of freezing on frame 0's raster.
        let mut rt = Runtime::new(
            Anim { px, as_icon: false, prefix_lines: 0, cached: true, mounted: true },
            anim_view,
            (200.0, 40.0),
            Env::new(true, 1.0),
        );
        rt.frame(0.0);
        assert!(rt.instances.iter().all(|q| q.params[1] != crate::render::KIND_LAYER), "cached ancestor must draw live for inline images");
        assert_eq!(placed_rgba(&rt), Some(red), "cached: frame 0");
        rt.frame(0.1);
        assert_eq!(placed_rgba(&rt), Some(green), "cached: advances to frame 1");
        assert!(rt.wake_deadline().is_some(), "cached: keeps arming wakes");
    }

    #[test]
    fn pill_style_stays_copy_and_const_and_paints_ring_radius_and_logo() {
        const BORDER: crate::Color = crate::Color { r: 0.2, g: 0.5, b: 0.9, a: 1.0 };
        const BG: crate::Color = crate::Color { r: 0.2, g: 0.5, b: 0.9, a: 0.15 };
        const FG: crate::Color = crate::Color { r: 0.1, g: 0.3, b: 0.8, a: 1.0 };
        const CAPSULE: PillStyle = PillStyle::new().bg(BG).fg(FG).ring(BORDER, 1.0).radius(999.0);
        fn assert_copy<T: Copy>(_: T) {}
        assert_copy(CAPSULE);
        let copy_a = CAPSULE;
        let copy_b = copy_a;
        assert_eq!(copy_b.ring, Some((BORDER, 1.0)));
        assert_eq!(copy_b.radius, Some(999.0));
        assert_eq!(PillStyle::new().ring(BORDER, 0.0).ring, None);
        assert_eq!(PillStyle::new().ring(BORDER, -1.0).ring, None);
        assert_eq!(PillStyle::new().ring(BORDER, f32::NAN).ring, None);
        assert_eq!(PillStyle::new().radius(-4.0).radius, Some(0.0));
        assert_eq!(PillStyle::new().radius(f32::NAN).radius, Some(0.0));
        assert_eq!(PillStyle::new().radius(0.0).radius, Some(0.0));

        let logo = super::image(16, 16);
        let static_el = text("open @Gmail today").pill_with_image(5..11, CAPSULE, logo.clone());
        let chips = &static_el.text.as_ref().unwrap().cold.pills;
        assert_eq!(chips.len(), 1);
        assert_eq!(chips[0].ring, Some((BORDER, 1.0)));
        assert_eq!(chips[0].radius, Some(999.0));
        assert!(chips[0].image.is_some());
        assert!(chips[0].icon.is_none());

        let ignored_on_field = crate::el::text_input(&crate::TextEdit::new("open @Gmail"), TextStyle::new(14.0, 400.0))
            .pill_with_image(5..11, CAPSULE, logo.clone());
        assert!(ignored_on_field.text.as_ref().unwrap().cold.pills.is_empty());

        struct FieldState {
            edit: crate::TextEdit,
            logo: crate::Pixels,
            use_style: bool,
        }
        let mut edit = crate::TextEdit::new("send via ").rich();
        edit.insert_pill("app:gmail", "@Gmail");
        edit.insert(" now");
        let view = |s: &FieldState, _: &mut Cx<FieldState>| {
            let mut field = crate::el::text_input(&s.edit, TextStyle::new(14.0, 400.0).lh(20.0))
                .key("field")
                .w(320.0)
                .h(40.0);
            if s.use_style {
                let px = s.logo.clone();
                field = field
                    .pill_style(|_| CAPSULE)
                    .pill_image(move |id| (id == "app:gmail").then(|| px.clone()));
            }
            col().key("root").padding(8.0).child(field)
        };
        let mut rt_plain = Runtime::new(
            FieldState { edit: edit.clone(), logo: logo.clone(), use_style: false },
            view,
            (360.0, 80.0),
            Env::new(true, 1.0),
        );
        rt_plain.frame(0.0);
        let a11y_plain = rt_plain.accessibility_tree();

        let mut rt = Runtime::new(
            FieldState { edit, logo: logo.clone(), use_style: true },
            view,
            (360.0, 80.0),
            Env::new(true, 1.0),
        );
        rt.frame(0.0);
        let a11y_styled = rt.accessibility_tree();

        let field_value = |tree: &accesskit::TreeUpdate| {
            tree.nodes.iter().find(|(_, n)| n.role() == accesskit::Role::MultilineTextInput || n.role() == accesskit::Role::TextInput)
                .and_then(|(_, n)| n.value().map(str::to_owned))
        };
        assert_eq!(field_value(&a11y_styled), field_value(&a11y_plain));

        let ringed = rt.instances.iter().find(|q| q.params[1] == crate::render::KIND_RECT && q.params[0] > 0.5 && q.border[3] > 0.5)
            .expect("pill with PillStyle::ring emits a bordered rect instance");
        assert!((ringed.params[0] - 1.0).abs() < 0.01);
        assert!((ringed.radii[0] - ringed.rect[3] * 0.5).abs() < 0.5, "capsule radius clamped to half height: {:?}", ringed);

        let img = rt.instances.iter().find(|q| q.params[1] == crate::render::KIND_IMAGE).expect("pill_image emits a KIND_IMAGE instance");
        assert!(
            img.rect[0] >= ringed.rect[0] - 0.5
                && img.rect[0] + img.rect[2] <= ringed.rect[0] + ringed.rect[2] + 0.5
                && img.rect[1] >= ringed.rect[1] - 0.5
                && img.rect[1] + img.rect[3] <= ringed.rect[1] + ringed.rect[3] + 0.5,
            "logo {:?} sits inside pill chip {:?}", img.rect, ringed.rect
        );

        let mut rt_static = Runtime::new(
            logo,
            |px: &crate::Pixels, _: &mut Cx<crate::Pixels>| {
                col().key("root").padding(8.0).child(
                    text("open @Gmail now")
                        .font(TextStyle::new(14.0, 400.0).lh(20.0))
                        .pill_with_image(5..11, CAPSULE, px.clone()),
                )
            },
            (320.0, 60.0),
            Env::new(true, 1.0),
        );
        rt_static.frame(0.0);
        assert!(rt_static.instances.iter().any(|q| q.params[1] == crate::render::KIND_RECT && q.params[0] > 0.5 && q.border[3] > 0.5));
        assert!(rt_static.instances.iter().any(|q| q.params[1] == crate::render::KIND_IMAGE));
    }

    #[test]
    fn pill_style_pad_x_and_margin_x_stay_const_copy_and_control_chip_geometry() {
        const TIGHT: PillStyle = PillStyle::new().pad_x(2.0).margin_x(0.0).radius(3.0);
        fn assert_copy<T: Copy>(_: T) {}
        assert_copy(TIGHT);
        assert_eq!(PillStyle::new().pad_x, None);
        assert_eq!(PillStyle::new().margin_x, None);
        assert_eq!(TIGHT.pad_x, Some(2.0));
        assert_eq!(TIGHT.margin_x, Some(0.0));
        assert_eq!(PillStyle::new().pad_x(-3.0).pad_x, Some(0.0));
        assert_eq!(PillStyle::new().pad_x(f32::NAN).pad_x, Some(0.0));
        assert_eq!(PillStyle::new().margin_x(-1.0).margin_x, Some(0.0));
        assert_eq!(PillStyle::new().margin_x(f32::INFINITY).margin_x, Some(0.0));

        const BG: crate::Color = crate::Color { r: 0.2, g: 0.5, b: 0.9, a: 0.25 };
        const STYLE: TextStyle = TextStyle::new(15.0, 400.0).lh(22.0);
        let chip_rect = |rt: &Runtime<PillStyle>| -> [f32; 4] {
            rt.instances
                .iter()
                .find(|q| q.params[1] == crate::render::KIND_RECT && q.fill[3] > 0.1)
                .expect("pill chip rect")
                .rect
        };
        let static_view = |ps: &PillStyle, _: &mut Cx<PillStyle>| {
            col().key("root").child(text("hi @alex there").key("t").font(STYLE).pill(3..8, *ps))
        };

        let mut rt_def = Runtime::new(PillStyle::new().bg(BG), static_view, (320.0, 60.0), Env::new(true, 1.0));
        rt_def.frame(0.0);
        let def_rect = chip_rect(&rt_def);
        let def_w = rt_def.tree.nodes.iter().find(|n| n.key.as_deref() == Some("t")).unwrap().text_layout.as_ref().unwrap().view().full_width();

        let mut rt_tight = Runtime::new(TIGHT.bg(BG), static_view, (320.0, 60.0), Env::new(true, 1.0));
        rt_tight.frame(0.0);
        let tight_rect = chip_rect(&rt_tight);
        let tight_w = rt_tight.tree.nodes.iter().find(|n| n.key.as_deref() == Some("t")).unwrap().text_layout.as_ref().unwrap().view().full_width();

        // Default at 15pt: pad=5.4, margin=1.8 (14.4pt total). Tight: pad=2.0, margin=0.0 (4.0pt total).
        let expected_chip_shrink = 2.0 * ((15.0f32 * 0.36).max(3.0) - 2.0);
        assert!(
            ((def_rect[2] - tight_rect[2]) - expected_chip_shrink).abs() <= 1.5,
            "chip width shrunk by pad delta: def={def_rect:?} tight={tight_rect:?}",
        );
        let expected_total_shrink = 2.0 * (((15.0f32 * 0.36).max(3.0) + (15.0f32 * 0.12).max(1.5)) - 2.0);
        assert!(
            ((def_w - tight_w) - expected_total_shrink).abs() <= 2.0,
            "text advance shrunk by pad+margin delta: def_w={def_w} tight_w={tight_w}",
        );

        // Dynamic text_input: changing pill_style updates chip rect and keeps hit-testing accurate.
        struct InputState {
            edit: crate::TextEdit,
            tight: bool,
            pressed: Vec<String>,
        }
        let mut edit = crate::TextEdit::new("hi ").rich();
        edit.insert_pill("user:alex", "@alex");
        edit.insert(" there");
        let input_view = |s: &InputState, _: &mut Cx<InputState>| {
            let ps = if s.tight { TIGHT.bg(BG) } else { PillStyle::new().bg(BG) };
            col().key("root").child(
                crate::el::text_input(&s.edit, STYLE)
                    .key("field")
                    .w(280.0)
                    .h(36.0)
                    .on_edit(|s: &mut InputState| &mut s.edit)
                    .pill_style(move |_| ps)
                    .on_pill_press(|s: &mut InputState, id, _, _| {
                        s.pressed.push(id.to_owned());
                        true
                    }),
            )
        };
        let mut rt = Runtime::new(
            InputState { edit, tight: false, pressed: Vec::new() },
            input_view,
            (320.0, 60.0),
            Env::new(true, 1.0),
        );
        rt.frame(0.0);
        let find_chip = |rt: &Runtime<InputState>| -> [f32; 4] {
            rt.instances
                .iter()
                .find(|q| q.params[1] == crate::render::KIND_RECT && q.fill[3] > 0.1)
                .expect("input pill chip rect")
                .rect
        };
        let input_def = find_chip(&rt);

        rt.update(|s, _| s.tight = true);
        rt.frame(0.0);
        let input_tight = find_chip(&rt);
        assert!(
            ((input_def[2] - input_tight[2]) - expected_chip_shrink).abs() <= 1.5,
            "dynamic pill_style switch re-shapes and repaints chip: def={input_def:?} tight={input_tight:?}",
        );

        // Clicking the center of the tighter pill fires on_pill_press with its id.
        let cx = input_tight[0] + input_tight[2] * 0.5;
        let cy = input_tight[1] + input_tight[3] * 0.5;
        rt.pointer_move(cx, cy);
        rt.pointer_down(cx, cy);
        rt.pointer_up(cx, cy);
        rt.frame(0.0);
        assert_eq!(rt.state.pressed, ["user:alex"]);
    }

    #[test]
    fn pill_inline_image_replaces_pill_label_while_keeping_atomic_edit_copy_hit_and_a11y() {
        const BG: crate::Color = crate::Color { r: 0.2, g: 0.5, b: 0.9, a: 0.25 };
        const STYLE: TextStyle = TextStyle::new(14.0, 400.0).lh(20.0);

        struct ChatState {
            edit: crate::TextEdit,
            emoji: Option<crate::Pixels>,
            pressed: Vec<String>,
            secondary: Vec<String>,
            inline_first: bool,
        }

        let emoji_px = super::image(32, 32);
        let mut edit = crate::TextEdit::new("oi ").rich();
        edit.insert_pill("emoji:gato", ":gato:");
        edit.insert(" ");
        edit.insert_pill("user:ada", "@ada");
        edit.insert(" tudo");

        let view = |s: &ChatState, _: &mut Cx<ChatState>| {
            let emoji = s.emoji.clone();
            let mut field = crate::el::text_input(&s.edit, STYLE)
                .key("field")
                .w(320.0)
                .h(80.0)
                .on_edit(|s: &mut ChatState| &mut s.edit)
                .on_pill_press(|s: &mut ChatState, id, _, _| {
                    s.pressed.push(id.to_owned());
                    true
                })
                .on_pill_secondary(|s: &mut ChatState, id, _, _| {
                    s.secondary.push(id.to_owned());
                    true
                });
            if s.inline_first {
                field = field
                    .pill_inline_image(move |id| {
                        (id == "emoji:gato").then(|| emoji.as_ref().map(|px| crate::InlineImage::new(px.clone(), 20.0, 16.0, 4.0))).flatten()
                    })
                    .pill_style(|_| PillStyle::new().bg(BG));
            } else {
                field = field
                    .pill_style(|_| PillStyle::new().bg(BG))
                    .pill_inline_image(move |id| {
                        (id == "emoji:gato").then(|| emoji.as_ref().map(|px| crate::InlineImage::new(px.clone(), 20.0, 16.0, 4.0))).flatten()
                    });
            }
            col().key("root").padding(8.0).child(field)
        };

        // 1. Both builder orders produce the same replaced emoji + styled mention pill.
        for inline_first in [false, true] {
            let mut rt = Runtime::new(
                ChatState { edit: edit.clone(), emoji: Some(emoji_px.clone()), pressed: Vec::new(), secondary: Vec::new(), inline_first },
                view,
                (360.0, 100.0),
                Env::new(true, 1.0),
            );
            rt.frame(0.0);

            // Exactly one KIND_IMAGE instance for `:gato:` at ~20x20, and only ONE chip rect (for `@ada`).
            let images: Vec<_> = rt.instances.iter().filter(|q| q.params[1] == crate::render::KIND_IMAGE).collect();
            assert_eq!(images.len(), 1, "inline_first={inline_first}: expected 1 KIND_IMAGE for :gato:");
            let img = images[0].rect;
            assert!((img[2] - 20.0).abs() <= 1.0 && (img[3] - 20.0).abs() <= 1.0, "emoji rect={img:?}");

            let chip_rects: Vec<_> = rt.instances.iter().filter(|q| q.params[1] == crate::render::KIND_RECT && q.fill[3] > 0.1).collect();
            assert_eq!(chip_rects.len(), 1, "inline_first={inline_first}: only @ada draws a chip rect, not :gato:");
            assert!(chip_rects[0].rect[0] > img[0] + img[2], "@ada chip sits to the right of :gato: image");

            // Accessibility value keeps the full shortcode text and adds no spurious Image child.
            let tree = rt.accessibility_tree();
            let field_node = tree.nodes.iter().find(|(_, n)| {
                n.role() == accesskit::Role::MultilineTextInput || n.role() == accesskit::Role::TextInput
            }).expect("field a11y node");
            assert_eq!(field_node.1.value(), Some("oi :gato: @ada tudo"));
            assert!(!tree.nodes.iter().any(|(_, n)| n.role() == accesskit::Role::Image), "replaced field pill emits no extra Image a11y node");

            // Primary and secondary clicks on the emoji image hit `emoji:gato`.
            let (ex, ey) = (img[0] + img[2] * 0.5, img[1] + img[3] * 0.5);
            rt.pointer_move(ex, ey);
            assert_eq!(rt.cursor(), crate::Cursor::Pointer, "hovering replaced pill with on_pill_press shows Pointer");
            rt.pointer_down(ex, ey);
            rt.pointer_up(ex, ey);
            rt.frame(0.0);
            assert_eq!(rt.state.pressed, ["emoji:gato"]);

            rt.secondary_down(ex, ey);
            rt.secondary_up(ex, ey);
            rt.frame(0.0);
            assert_eq!(rt.state.secondary, ["emoji:gato"]);
        }

        // 2. Dynamic update: emoji pixels arriving later switch `:gato:` from chip to replaced image
        //    even in a multi-paragraph field, and without `on_pill_press`, clicking left/right halves
        //    places the caret before/after `:gato:` while Backspace deletes the pill atomically.
        let mut multi = crate::TextEdit::new("line 1\noi ").rich();
        let pill_start = multi.text.len();
        multi.insert_pill("emoji:gato", ":gato:");
        let pill_end = multi.text.len();
        multi.insert(" tudo");
        multi.set_caret(0);

        let edit_view = |s: &ChatState, _: &mut Cx<ChatState>| {
            let emoji = s.emoji.clone();
            col().key("root").padding(8.0).child(
                crate::el::text_input(&s.edit, STYLE)
                    .key("field")
                    .w(320.0)
                    .h(120.0)
                    .on_edit(|s: &mut ChatState| &mut s.edit)
                    .pill_style(|_| PillStyle::new().bg(BG))
                    .pill_inline_image(move |id| {
                        (id == "emoji:gato").then(|| emoji.as_ref().map(|px| crate::InlineImage::new(px.clone(), 24.0, 22.0, 6.0))).flatten()
                    }),
            )
        };
        let mut rt = Runtime::new(
            ChatState { edit: multi, emoji: None, pressed: Vec::new(), secondary: Vec::new(), inline_first: false },
            edit_view,
            (360.0, 140.0),
            Env::new(true, 1.0),
        );
        rt.frame(0.0);
        assert!(!rt.instances.iter().any(|q| q.params[1] == crate::render::KIND_IMAGE), "no image before emoji loads");
        let h_before = rt.tree.nodes.iter().find(|n| n.key.as_deref() == Some("field")).unwrap().text_layout.as_ref().unwrap().view().height();

        // Emoji finishes loading: switches from chip to taller 24x28 replaced image.
        rt.update(|s, _| s.emoji = Some(emoji_px.clone()));
        rt.frame(0.0);
        let img = rt.instances.iter().find(|q| q.params[1] == crate::render::KIND_IMAGE).expect("emoji image drawn").rect;
        let h_after = rt.tree.nodes.iter().find(|n| n.key.as_deref() == Some("field")).unwrap().text_layout.as_ref().unwrap().view().height();
        assert!(h_after > h_before + 4.0, "taller inline emoji grew multi-paragraph height: before={h_before} after={h_after}");
        assert!(!rt.instances.iter().any(|q| q.params[1] == crate::render::KIND_RECT && q.fill[3] > 0.1), "chip rect disappears once replaced by image");

        // Click left half -> caret before `:gato:`; click right half -> caret after `:gato:`.
        let mid_y = img[1] + img[3] * 0.5;
        let left_x = img[0] + img[2] * 0.25;
        let right_x = img[0] + img[2] * 0.75;
        rt.pointer_move(left_x, mid_y);
        rt.pointer_down(left_x, mid_y);
        rt.pointer_up(left_x, mid_y);
        rt.frame(0.0);
        assert_eq!(rt.state.edit.range(), (pill_start, pill_start), "left-half click puts caret before pill");

        rt.pointer_move(right_x, mid_y);
        rt.pointer_down(right_x, mid_y);
        rt.pointer_up(right_x, mid_y);
        rt.frame(0.0);
        assert_eq!(rt.state.edit.range(), (pill_end, pill_end), "right-half click puts caret after pill");

        // Left/Right arrows step atomically over the replaced pill; Shift+Left selects it whole
        // with its shortcode text, and Backspace removes it whole.
        rt.key(crate::KeyEvent { key: crate::Key::Left, mods: crate::Mods::default() });
        rt.frame(0.0);
        assert_eq!(rt.state.edit.range(), (pill_start, pill_start), "Left arrow steps atomically before pill");
        rt.key(crate::KeyEvent { key: crate::Key::Right, mods: crate::Mods::default() });
        rt.frame(0.0);
        assert_eq!(rt.state.edit.range(), (pill_end, pill_end), "Right arrow steps atomically past pill");

        rt.key(crate::KeyEvent { key: crate::Key::Left, mods: crate::Mods { shift: true, ..crate::Mods::default() } });
        rt.frame(0.0);
        assert_eq!(rt.state.edit.range(), (pill_start, pill_end));
        assert_eq!(rt.state.edit.selected(), ":gato:");
        assert!(
            rt.instances.iter().any(|q| q.params[1] == crate::render::KIND_RECT && q.rect[2] >= img[2] - 1.5 && (q.rect[0] - img[0]).abs() <= 2.0),
            "selection highlight covers replaced emoji image",
        );

        rt.key(crate::KeyEvent { key: crate::Key::Right, mods: crate::Mods::default() });
        rt.key(crate::KeyEvent { key: crate::Key::Backspace, mods: crate::Mods::default() });
        rt.frame(0.0);
        assert_eq!(rt.state.edit.text, "line 1\noi  tudo");
        assert!(rt.state.edit.pills().next().is_none());
    }
}
