//! `El::max_lines`: wrapped text stops after n lines with "…" ending the last, measures n lines
//! tall, cuts between graphemes, refits on resize, and copying and tools keep the whole string.
use super::*;
use crate::{Color, Cx, El, Env, Key, KeyEvent, Link, Mods, Span, TextStyle, col, rich};
use unicode_segmentation::UnicodeSegmentation;

const TITLE: &str = "Move the sidebar's search into the toolbar, keep its results in the list, and let the \
    filter chips wrap onto a second row when the window is narrow enough that they would overflow";

fn style() -> TextStyle { TextStyle::new(13.0, 400.0).lh(16.0) }

/// Content, the line limit, and a byte range that is both a coloured span and a link.
type S = (&'static str, usize, Option<(usize, usize)>);

fn mark() -> Color { crate::color::hex("#e6339a").unwrap() }

fn clamp(content: &'static str, lines: usize, marked: Option<(usize, usize)>, width: f32) -> Runtime<S> {
    fn view(s: &S, _: &mut Cx<S>) -> El {
        let spans = s.2.map(|(a, b)| Span { color: Some(mark()), ..Span::new(a, b) }).into_iter().collect();
        let mut t = rich(s.0, spans).key("t").font(style()).selectable(true).max_lines(s.1);
        if let Some((a, b)) = s.2 { t = t.link(Link::new(a..b, "marked")).on_link(|_: &mut S, _, _| {}); }
        col().w_full().child(t)
    }
    let mut rt = Runtime::new((content, lines, marked), view, (width, 400.0), Env::new(true, 2.0));
    rt.frame(0.0);
    rt
}

fn node<'a>(rt: &'a Runtime<S>) -> &'a Node {
    rt.tree.nodes.iter().find(|n| n.key.as_deref() == Some("t")).unwrap()
}

/// The string node "t" draws, its cut, its line count and its height in points.
fn drawn(rt: &Runtime<S>) -> (String, Option<Elision>, usize, f32) {
    let n = node(rt);
    let content = &n.text.as_ref().unwrap().content;
    let shown = match n.elided {
        Some(e) => [&content[..e.head], "…", &content[e.tail..]].concat(),
        None => content.clone(),
    };
    (shown, n.elided, n.text_layout.as_ref().and_then(crate::text::TextLayout::single).unwrap().len(), n.target[3] - n.target[1])
}

#[test]
fn stops_after_n_lines_with_an_ellipsis_and_measures_n_lines() {
    let mut rt = clamp(TITLE, 3, None, 200.0);
    let (shown, e, lines, h) = drawn(&rt);
    let e = e.expect("the title runs past three lines at 200 pt");
    assert!(shown.ends_with('…') && !shown[..shown.len() - "…".len()].ends_with(' '), "{shown}");
    assert_eq!(e.tail, TITLE.len(), "everything after the cut is hidden");
    assert_eq!((lines, h), (3, 48.0), "three 16 pt lines: {shown}");
    let el = rt.elements().into_iter().find(|e| e.key.as_deref() == Some("t")).unwrap();
    assert_eq!(el.text.as_deref(), Some(TITLE), "tools and accessibility read the whole title");

    rt.set_viewport((320.0, 400.0), 2.0);
    rt.frame(0.1);
    let (wider, _, lines, h) = drawn(&rt);
    assert!(wider.len() > shown.len() && (lines, h) == (3, 48.0), "a wider box keeps more: {wider}");

    rt.set_viewport((2000.0, 400.0), 2.0);
    rt.frame(0.2);
    let (all, e, lines, h) = drawn(&rt);
    assert_eq!((all.as_str(), e, lines, h), (TITLE, None, 1, 16.0), "room for it all: no cut, one line tall");

    rt.set_viewport((200.0, 400.0), 2.0);
    rt.frame(0.3);
    assert_eq!(drawn(&rt).0, shown, "narrowing again cuts as before");
}

#[test]
fn text_within_the_limit_is_untouched() {
    let rt = clamp("Short title", 3, None, 200.0);
    assert_eq!(drawn(&rt), ("Short title".to_owned(), None, 1, 16.0));
    // 0 shows every line.
    let rt = clamp(TITLE, 0, None, 200.0);
    let (shown, e, lines, h) = drawn(&rt);
    assert!(shown == TITLE && e.is_none() && lines > 3 && h == 16.0 * lines as f32);
}

#[test]
fn a_line_break_before_the_cut_is_dropped() {
    let rt = clamp("first\nsecond\nthird", 2, None, 400.0);
    assert_eq!(drawn(&rt).0, "first\nsecond…");
    assert_eq!(drawn(&rt).3, 32.0);
}

#[test]
fn cuts_fall_between_graphemes() {
    const MIXED: &str = "👩‍👩‍👧‍👦 cafe\u{301} שלום עולם 🇯🇵 नमस्ते e\u{301}e\u{301}e\u{301} 👩‍👩‍👧‍👦 done and some more words to wrap 👩‍👩‍👧‍👦";
    let bounds: Vec<usize> = MIXED.grapheme_indices(true).map(|(i, _)| i).chain([MIXED.len()]).collect();
    let mut rt = clamp(MIXED, 2, None, 60.0);
    let mut cuts = 0;
    for w in (60..400).step_by(7) {
        rt.set_viewport((w as f32, 400.0), 2.0);
        rt.frame(1.0);
        let (shown, e, lines, _) = drawn(&rt);
        let Some(e) = e else { continue };
        cuts += 1;
        assert!(lines <= 2, "{w}: {shown}");
        assert!(bounds.contains(&e.head), "{w}: {e:?} splits a grapheme: {shown}");
    }
    assert!(cuts > 10, "most widths cut: {cuts}");
}

#[test]
fn copying_a_clamped_text_copies_all_of_it() {
    let mut rt = clamp(TITLE, 2, None, 200.0);
    let r = node(&rt).target;
    let (x, y) = (r[0] + 20.0, r[1] + 8.0);
    rt.pointer_move(x, y);
    rt.pointer_down(x, y);
    rt.pointer_up(x, y);
    rt.frame(1.0);
    rt.key(KeyEvent { key: Key::Command('a'), mods: Mods { cmd: true, ..Mods::default() } });
    rt.frame(1.1);
    assert_eq!(rt.selected_text().as_deref(), Some(TITLE));
}

#[test]
fn spans_and_links_past_the_cut_are_hidden() {
    let at = TITLE.find("overflow").unwrap();
    let marked = (at, at + "overflow".len());
    let coloured = |rt: &Runtime<S>| {
        let want = mark().premul(1.0);
        rt.instances.iter().filter(|i| i.fill.iter().zip(want).all(|(a, b)| (a - b).abs() < 1e-3)).count()
    };
    let rt = clamp(TITLE, 2, Some(marked), 200.0);
    assert!(drawn(&rt).1.is_some_and(|e| e.head < marked.0));
    assert!(!rt.elements().iter().any(|e| e.path.ends_with("#link-0")), "a link past the cut isn't pressable");
    assert_eq!(coloured(&rt), 0);
    let rt = clamp(TITLE, 0, Some(marked), 200.0);
    assert!(rt.elements().iter().any(|e| e.path.ends_with("#link-0")));
    assert!(coloured(&rt) > 0, "shown, the span draws its colour");
}

/// One word with no break opportunity, wider than the box.
const TOKEN: &str = "line_clamp_measures_the_whole_identifier_before_it_decides_where_the_ellipsis_falls";

/// The drawn last line's width and the element's, in points.
fn widths(rt: &Runtime<S>) -> (f32, f32) {
    let n = node(rt);
    (last_line_width(n.text_layout.as_ref().and_then(crate::text::TextLayout::single).unwrap(), n.text.as_ref().unwrap().max_lines.max(1)) / 2.0, n.target[2] - n.target[0])
}

#[test]
fn a_word_wider_than_the_box_is_cut_to_fit() {
    let rt = clamp(TOKEN, 0, None, 200.0);
    assert_eq!(drawn(&rt).2, 1, "the word shapes as one overflowing line");
    let rt = clamp(TOKEN, 1, None, 200.0);
    let (shown, e, lines, h) = drawn(&rt);
    assert!(e.is_some() && shown.ends_with('…') && shown.len() > 10, "{shown}");
    assert_eq!((lines, h), (1, 16.0));
    let (line, element) = widths(&rt);
    assert!(line <= 200.25 && line > 150.0 && element <= 200.0, "{line} {element}: {shown}");
}

#[test]
fn a_wrapped_word_fills_the_last_line() {
    let content: &'static str = Box::leak(format!("See {TOKEN}").into_boxed_str());
    let rt = clamp(content, 1, None, 200.0);
    let (shown, _, lines, _) = drawn(&rt);
    assert!(shown.starts_with("See line_") && shown.ends_with('…') && lines == 1, "{shown}");
    let (line, element) = widths(&rt);
    assert!(line <= 200.25 && line > 150.0 && element <= 200.0, "{line} {element}: {shown}");

    // In two lines the word moves to the second and is cut there.
    let rt = clamp(content, 2, None, 200.0);
    let (shown, _, lines, _) = drawn(&rt);
    assert!(shown.starts_with("See\nline_") || shown.starts_with("See line_"), "{shown}");
    assert!(lines <= 2 && widths(&rt).0 <= 200.25, "{shown}");

    // With `wrap_anywhere`, an unbroken token wraps across lines 1..n and cuts at the end of line n.
    fn anywhere(_: &(), _: &mut Cx<()>) -> El {
        col().w_full().child(crate::text(TOKEN).key("t").font(style()).wrap_anywhere().max_lines(2))
    }
    let mut rt = Runtime::new((), anywhere, (200.0, 400.0), Env::new(true, 2.0));
    rt.frame(0.0);
    let n = rt.tree.nodes.iter().find(|n| n.key.as_deref() == Some("t")).unwrap();
    let e = n.elided.expect("the token wraps past two lines at 200 pt");
    let l = n.text_layout.as_ref().and_then(crate::text::TextLayout::single).unwrap();
    assert_eq!(l.len(), 2, "wraps onto two lines before cutting");
    assert!(e.head > 40 && l.full_width() <= 400.5, "both lines fill the 200 pt box: head={}, width={}", e.head, l.full_width());
}

/// A field's caret and selection index its own text, so it takes no cut.
#[test]
fn a_text_field_ignores_the_clamp_and_truncation() {
    fn view(e: &crate::TextEdit, _: &mut Cx<crate::TextEdit>) -> El {
        col().w_full()
            .child(crate::text_input(e, style()).key("t").w_full().max_lines(1))
            .child(crate::text_input(e, style()).key("u").w_full().truncate_middle())
    }
    let mut rt = Runtime::new(crate::TextEdit::new(TITLE), view, (200.0, 400.0), Env::new(true, 2.0));
    rt.frame(0.0);
    for key in ["t", "u"] {
        let n = rt.tree.nodes.iter().find(|n| n.key.as_deref() == Some(key)).unwrap();
        let t = n.text.as_ref().unwrap();
        assert!(n.elided.is_none() && t.max_lines == 0 && !t.truncate, "{key}");
        let r = n.target;
        let (x, y) = (r[2] - 4.0, r[1] + 8.0);
        rt.pointer_move(x, y);
        rt.pointer_down(x, y);
        rt.pointer_up(x, y);
        rt.frame(1.0);
        rt.key(KeyEvent { key: Key::Right, mods: Mods::default() });
        rt.frame(1.1);
    }
}

#[test]
fn clamped_elision_cache_reuses_cut_across_commits() {
    let mut rt = clamp(TITLE, 2, Some((9, 25)), 180.0);
    let (_, cut0, lines0, _) = drawn(&rt);
    assert_eq!(lines0, 2);
    assert!(cut0.is_some());
    assert_eq!(rt.text.cached_elision_count(), 1);
    let shaped_before = rt.text.shaped;
    rt.invalidate();
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.text.shaped, shaped_before, "repeat commit reuses cached clamped cut");
    assert_eq!(drawn(&rt).1, cut0);
}

#[test]
fn narrow_clamped_text_clips_to_box_and_draws_nothing_at_zero_width() {
    use crate::render::KIND_GLYPH;
    let zero = clamp("Hello world", 1, None, 0.0);
    assert_eq!(zero.instances.iter().filter(|i| i.params[1] == KIND_GLYPH).count(), 0);

    let narrow = clamp("Hello world", 1, None, 5.0);
    let n = node(&narrow);
    let (x0, x1) = (n.target[0] * 2.0, n.target[2] * 2.0);
    let glyphs: Vec<_> = narrow.instances.iter().filter(|i| i.params[1] == KIND_GLYPH).collect();
    assert!(!glyphs.is_empty());
    for g in glyphs {
        assert!(g.clip[0] >= x0 - 0.5 && g.clip[2] <= x1 + 0.5, "glyph clip {:?} overflows [{x0}, {x1}]", g.clip);
    }
}

const FILE_NAME: &str = "Quarterly report final revised version two of the budget.xlsx";

#[test]
fn max_lines_elide_middle_keeps_extension_measures_n_lines_and_is_order_independent() {
    use crate::text;
    fn view(width: &f32, _: &mut Cx<f32>) -> El {
        col().w_full().child(
            col().w(*width)
                .child(text(FILE_NAME).key("end").font(style()).selectable(true).max_lines(2))
                .child(text(FILE_NAME).key("mid_after").font(style()).selectable(true).max_lines(2).elide_middle())
                .child(text(FILE_NAME).key("mid_before").font(style()).selectable(true).elide_middle().max_lines(2))
                .child(text(FILE_NAME).key("one_line").font(style()).max_lines(2).truncate_middle()),
        )
    }
    let mut rt = Runtime::new(120.0f32, view, (400.0, 400.0), Env::new(true, 2.0));
    rt.frame(0.0);
    let shown_of = |rt: &Runtime<f32>, key: &str| {
        let n = rt.tree.nodes.iter().find(|n| n.key.as_deref() == Some(key)).unwrap();
        let content = &n.text.as_ref().unwrap().content;
        let shown = match n.elided {
            Some(e) => [&content[..e.head], "…", &content[e.tail..]].concat(),
            None => content.clone(),
        };
        let l = n.text_layout.as_ref().and_then(crate::text::TextLayout::single).unwrap();
        let max_line_w = l.lines().map(|line| (line.metrics().advance - line.metrics().hanging_advance) / 2.0).fold(0.0f32, f32::max);
        (shown, n.elided, l.len(), n.target[3] - n.target[1], max_line_w)
    };

    let (end_shown, end_e, end_lines, end_h, _) = shown_of(&rt, "end");
    assert_eq!((end_lines, end_h), (2, 32.0));
    assert_eq!(end_e.map(|e| e.tail), Some(FILE_NAME.len()), "default max_lines still end-elides: {end_shown}");

    let (one_shown, _, one_lines, one_h, _) = shown_of(&rt, "one_line");
    assert_eq!((one_lines, one_h), (1, 16.0), "truncate_middle still forces one line: {one_shown}");

    let (ma_shown, ma_e, ma_lines, ma_h, ma_w) = shown_of(&rt, "mid_after");
    let (mb_shown, mb_e, mb_lines, mb_h, mb_w) = shown_of(&rt, "mid_before");
    assert_eq!((ma_shown.as_str(), ma_e, ma_lines, ma_h), (mb_shown.as_str(), mb_e, mb_lines, mb_h), "order independent");
    let e = ma_e.expect("long file name overflows 2 lines at 120 pt");
    assert_eq!((ma_lines, ma_h), (2, 32.0), "two 16 pt lines: {ma_shown}");
    assert!(ma_w <= 120.25 && mb_w <= 120.25, "every line fits 120 pt box: {ma_w} / {mb_w}: {ma_shown}");
    assert!(ma_shown.starts_with("Quarterly") && ma_shown.ends_with(".xlsx") && ma_shown.contains('…'), "{ma_shown}");
    assert!(!FILE_NAME[..e.head].ends_with(' ') && !FILE_NAME[e.tail..].starts_with(' '), "no spaces beside ellipsis: {ma_shown}");

    // ElementInfo::text and AccessKit keep the full authored string.
    let el = rt.elements().into_iter().find(|e| e.key.as_deref() == Some("mid_after")).unwrap();
    assert_eq!(el.text.as_deref(), Some(FILE_NAME));
    let tree = rt.accessibility_tree();
    let ak = &tree.nodes.iter().find(|(_, n)| n.author_id() == Some("mid_after")).expect("a11y node").1;
    assert_eq!(ak.value().or(ak.label()), Some(FILE_NAME));

    // Selecting all copies the whole string across the ellipsis.
    let r = rt.tree.nodes.iter().find(|n| n.key.as_deref() == Some("mid_after")).unwrap().target;
    rt.pointer_move(r[0] + 10.0, r[1] + 8.0);
    rt.pointer_down(r[0] + 10.0, r[1] + 8.0);
    rt.pointer_up(r[0] + 10.0, r[1] + 8.0);
    rt.frame(0.1);
    rt.key(KeyEvent { key: Key::Command('a'), mods: Mods { cmd: true, ..Mods::default() } });
    rt.frame(0.2);
    assert_eq!(rt.selected_text().as_deref(), Some(FILE_NAME));

    // Refits wider, clears elision when all of it fits, and refits when narrowed again.
    rt.update(|w, _| *w = 220.0);
    rt.frame(0.3);
    let (wider, _, wider_lines, wider_h, wider_w) = shown_of(&rt, "mid_after");
    assert!(wider.len() > ma_shown.len() && wider_lines <= 2 && wider_h <= 32.0 && wider_w <= 220.25, "{wider}");

    rt.update(|w, _| *w = 800.0);
    rt.frame(0.4);
    let (full, full_e, full_lines, full_h, _) = shown_of(&rt, "mid_after");
    assert_eq!((full.as_str(), full_e, full_lines, full_h), (FILE_NAME, None, 1, 16.0));

    rt.update(|w, _| *w = 120.0);
    rt.frame(0.5);
    assert_eq!(shown_of(&rt, "mid_after").0, ma_shown);
}

#[test]
fn max_lines_elide_head_keeps_end_and_is_order_independent() {
    use crate::text;
    const PATH: &str = "~/Projects/rune/crates/rune-kit/src/components/settings/appearance_panel.rs";
    fn view(_: &(), _: &mut Cx<()>) -> El {
        col().w_full().child(
            col().w(140.0)
                .child(text(PATH).key("ha").font(style()).wrap_anywhere().max_lines(2).elide_head())
                .child(text(PATH).key("hb").font(style()).wrap_anywhere().elide_head().max_lines(2)),
        )
    }
    let mut rt = Runtime::new((), view, (300.0, 200.0), Env::new(true, 2.0));
    rt.frame(0.0);
    let info = |key: &str| {
        let n = rt.tree.nodes.iter().find(|n| n.key.as_deref() == Some(key)).unwrap();
        let content = &n.text.as_ref().unwrap().content;
        let e = n.elided.expect("overflowing path elides");
        let shown = [&content[..e.head], "…", &content[e.tail..]].concat();
        let l = n.text_layout.as_ref().and_then(crate::text::TextLayout::single).unwrap();
        let max_line_w = l.lines().map(|line| (line.metrics().advance - line.metrics().hanging_advance) / 2.0).fold(0.0f32, f32::max);
        (shown, e, l.len(), n.target[3] - n.target[1], max_line_w)
    };
    let (sa, ea, la, ha, wa) = info("ha");
    let (sb, eb, lb, hb, wb) = info("hb");
    assert_eq!((sa.as_str(), ea, la, ha), (sb.as_str(), eb, lb, hb));
    assert_eq!(ea.head, 0);
    assert_eq!((la, ha), (2, 32.0));
    assert!(wa <= 140.25 && wb <= 140.25, "{wa} / {wb}: {sa}");
    assert!(sa.starts_with('…') && sa.ends_with("appearance_panel.rs"), "{sa}");
}

#[test]
fn max_lines_elide_middle_and_head_preserve_visible_spans_and_links() {
    let head_mark = (0, "Quarterly".len());
    let mid_at = FILE_NAME.find("revised").unwrap();
    let mid_mark = (mid_at, mid_at + "revised".len());
    let tail_at = FILE_NAME.find("budget.xlsx").unwrap();
    let tail_mark = (tail_at, tail_at + "budget.xlsx".len());

    fn view(head_elide: &bool, _: &mut Cx<bool>) -> El {
        let head_mark = (0, "Quarterly".len());
        let mid_at = FILE_NAME.find("revised").unwrap();
        let mid_mark = (mid_at, mid_at + "revised".len());
        let tail_at = FILE_NAME.find("budget.xlsx").unwrap();
        let tail_mark = (tail_at, tail_at + "budget.xlsx".len());
        let spans = vec![
            Span { color: Some(mark()), ..Span::new(head_mark.0, head_mark.1) },
            Span { color: Some(mark()), ..Span::new(mid_mark.0, mid_mark.1) },
            Span { color: Some(mark()), ..Span::new(tail_mark.0, tail_mark.1) },
        ];
        let t = rich(FILE_NAME, spans)
            .key("t")
            .font(style())
            .link(Link::new(head_mark.0..head_mark.1, "head"))
            .link(Link::new(mid_mark.0..mid_mark.1, "mid"))
            .link(Link::new(tail_mark.0..tail_mark.1, "tail"))
            .on_link(|_: &mut bool, _, _| {})
            .max_lines(2);
        let t = if *head_elide { t.elide_head() } else { t.elide_middle() };
        col().w_full().child(col().w(120.0).child(t))
    }

    let mut mid_rt = Runtime::new(false, view, (300.0, 200.0), Env::new(true, 2.0));
    mid_rt.frame(0.0);
    let n = mid_rt.tree.nodes.iter().find(|n| n.key.as_deref() == Some("t")).unwrap();
    let e = n.elided.expect("middle elided");
    assert!(e.head >= head_mark.1 && e.head <= mid_mark.0 && e.tail >= mid_mark.1 && e.tail <= tail_mark.0, "{e:?}");
    let paths: Vec<_> = mid_rt.elements().into_iter().map(|e| e.path).collect();
    assert!(paths.iter().any(|p| p.ends_with("#link-0")), "head link stays pressable");
    assert!(!paths.iter().any(|p| p.ends_with("#link-1")), "cut middle link is hidden");
    assert!(paths.iter().any(|p| p.ends_with("#link-2")), "tail link stays pressable");

    let mut head_rt = Runtime::new(true, view, (300.0, 200.0), Env::new(true, 2.0));
    head_rt.frame(0.0);
    let paths: Vec<_> = head_rt.elements().into_iter().map(|e| e.path).collect();
    assert!(!paths.iter().any(|p| p.ends_with("#link-0")), "cut head link is hidden");
    assert!(paths.iter().any(|p| p.ends_with("#link-2")), "tail link stays pressable in head elide");
}

#[test]
fn max_lines_elide_middle_and_head_unicode_graphemes_and_rtl() {
    use crate::text;
    const MIXED: &str = "👩‍👩‍👧‍👦 cafe\u{301} שלום עולם 🇯🇵 नमस्ते e\u{301}e\u{301} 👩‍👩‍👧‍👦 end_file_name.xlsx";
    const RTL: &str = "שלום עולם בדיקה ארוכה של טקסט בעברית לשתי שורות עם סיומת קובץ ארוכה.txt";
    let mixed_bounds: Vec<usize> = MIXED.grapheme_indices(true).map(|(i, _)| i).chain([MIXED.len()]).collect();
    let rtl_bounds: Vec<usize> = RTL.grapheme_indices(true).map(|(i, _)| i).chain([RTL.len()]).collect();

    fn view(w: &f32, _: &mut Cx<f32>) -> El {
        col().w_full().child(
            col().w(*w)
                .child(text(MIXED).key("m_mid").font(style()).max_lines(2).elide_middle())
                .child(text(MIXED).key("m_head").font(style()).max_lines(2).elide_head())
                .child(text(RTL).key("r_mid").dir(crate::Direction::Rtl).font(style()).max_lines(2).elide_middle())
                .child(text(RTL).key("r_head").dir(crate::Direction::Rtl).font(style()).max_lines(2).elide_head()),
        )
    }
    let mut rt = Runtime::new(60.0f32, view, (400.0, 400.0), Env::new(true, 2.0));
    let mut cuts = 0usize;
    for w in (60..280).step_by(19) {
        rt.update(move |s, _| *s = w as f32);
        rt.frame(0.1);
        for (key, bounds) in [("m_mid", &mixed_bounds), ("m_head", &mixed_bounds), ("r_mid", &rtl_bounds), ("r_head", &rtl_bounds)] {
            let n = rt.tree.nodes.iter().find(|n| n.key.as_deref() == Some(key)).unwrap();
            let l = n.text_layout.as_ref().and_then(crate::text::TextLayout::single).unwrap();
            assert!(l.len() <= 2, "{key} at {w}: {} lines", l.len());
            if let Some(e) = n.elided {
                cuts += 1;
                assert!(bounds.contains(&e.head) && bounds.contains(&e.tail), "{key} at {w}: {e:?} splits a grapheme");
            }
        }
    }
    assert!(cuts > 15, "expected multiple cuts across widths, got {cuts}");
}

#[test]
fn max_lines_elide_cache_separates_end_middle_and_head() {
    use crate::text;
    fn view(_: &(), _: &mut Cx<()>) -> El {
        col().w_full().child(
            col().w(120.0)
                .child(text(FILE_NAME).key("end").font(style()).max_lines(2))
                .child(text(FILE_NAME).key("mid").font(style()).max_lines(2).elide_middle())
                .child(text(FILE_NAME).key("head").font(style()).max_lines(2).elide_head()),
        )
    }
    let mut rt = Runtime::new((), view, (300.0, 300.0), Env::new(true, 2.0));
    rt.frame(0.0);
    let cut_of = |rt: &Runtime<()>, key: &str| {
        rt.tree.nodes.iter().find(|n| n.key.as_deref() == Some(key)).unwrap().elided.expect(key)
    };
    let (e_end, e_mid, e_head) = (cut_of(&rt, "end"), cut_of(&rt, "mid"), cut_of(&rt, "head"));
    assert_eq!(e_end.tail, FILE_NAME.len());
    assert!(e_mid.head > 0 && e_mid.tail < FILE_NAME.len());
    assert_eq!(e_head.head, 0);
    assert_ne!(e_end, e_mid);
    assert_ne!(e_mid, e_head);
    assert_eq!(rt.text.cached_elision_count(), 3, "End, Middle and Head occupy distinct cache entries");

    let shaped_before = rt.text.shaped;
    rt.invalidate();
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.text.shaped, shaped_before, "repeat commit reuses all three cached elisions");
    assert_eq!((cut_of(&rt, "end"), cut_of(&rt, "mid"), cut_of(&rt, "head")), (e_end, e_mid, e_head));
}

#[test]
fn max_lines_elide_edges_empty_zero_narrow_huge_live_and_text_input() {
    use crate::{LiveText, TextEdit, render::KIND_GLYPH, text, text_input};
    let huge = format!("start {} end.xlsx", "word ".repeat(600));
    struct State {
        huge: String,
        edit: TextEdit,
        live: LiveText,
    }
    fn view(s: &State, _: &mut Cx<State>) -> El {
        col().w_full().child(
            col().w(120.0)
                .child(text("").key("empty").font(style()).max_lines(2).elide_middle())
                .child(text(FILE_NAME).key("zero_lines").font(style()).max_lines(0).elide_middle())
                .child(text(FILE_NAME).key("zero_w").w(0.0).font(style()).max_lines(2).elide_middle())
                .child(text(FILE_NAME).key("narrow").w(5.0).font(style()).max_lines(2).elide_middle())
                .child(text(s.huge.as_str()).key("huge").font(style()).max_lines(2).elide_middle())
                .child(text("Superlongfirstword alpha beta gamma delta epsilon zeta.xlsx").key("wide_first").font(style()).max_lines(2).elide_middle())
                .child(text("init").key("live_mid").size(120.0, 32.0).font(style()).max_lines(2).elide_middle().live_text(&s.live))
                .child(text_input(&s.edit, style()).key("inp_mid").label("Mid").w_full().elide_middle())
                .child(text_input(&s.edit, style()).key("inp_head").label("Head").w_full().elide_head()),
        )
    }
    let state = State { huge: huge.clone(), edit: TextEdit::new(FILE_NAME), live: LiveText::new(FILE_NAME) };
    let mut rt = Runtime::new(state, view, (300.0, 600.0), Env::new(true, 2.0));
    rt.frame(0.0);

    fn find<'a>(rt: &'a Runtime<State>, key: &str) -> &'a Node {
        rt.tree.nodes.iter().find(|n| n.key.as_deref() == Some(key)).unwrap()
    }
    assert!(find(&rt, "empty").elided.is_none());
    assert!(find(&rt, "zero_lines").elided.is_none());
    assert_eq!(find(&rt, "huge").text_layout.as_ref().and_then(crate::text::TextLayout::single).unwrap().len(), 2);
    let huge_e = find(&rt, "huge").elided.expect("huge string elides");
    assert!(huge[huge_e.tail..].ends_with(".xlsx"));

    let wf = find(&rt, "wide_first");
    assert_eq!(wf.text_layout.as_ref().and_then(crate::text::TextLayout::single).unwrap().len(), 2, "wide first word still fills 2 lines");
    assert!(wf.elided.is_some());

    // text_input ignores elide_middle / elide_head.
    for key in ["inp_mid", "inp_head"] {
        let n = find(&rt, key);
        assert!(n.elided.is_none() && n.text.as_ref().unwrap().elide == crate::el::Elide::End, "{key}");
    }

    // live_text with max_lines(2).elide_middle() elides without polluting elision cache.
    let cached_before = rt.text.cached_elision_count();
    rt.state.live.set("Another very long streaming filename for live middle elision check.xlsx");
    rt.frame(0.1);
    let live_n = find(&rt, "live_mid");
    let live_e = live_n.elided.expect("live_text max_lines(2).elide_middle() elides");
    assert!(live_n.text.as_ref().unwrap().content[live_e.tail..].ends_with(".xlsx"));
    assert_eq!(rt.text.cached_elision_count(), cached_before, "live_text bypasses shared elision cache");

    // Zero-width elided text draws zero glyphs, narrow box clips glyphs.
    let mut zero = Runtime::new((), |_: &(), _: &mut Cx<()>| {
        col().w(0.0).child(text(FILE_NAME).key("z").font(style()).max_lines(2).elide_middle())
    }, (0.0, 200.0), Env::new(true, 2.0));
    zero.frame(0.0);
    assert_eq!(zero.instances.iter().filter(|i| i.params[1] == KIND_GLYPH).count(), 0);
}
