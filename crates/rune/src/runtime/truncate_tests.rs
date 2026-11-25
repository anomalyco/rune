//! `El::truncate_middle` and `El::truncate_head`: the ellipsis replaces the middle or the start,
//! cuts fall between graphemes, a resize refits the cut, and copying and tools keep the whole
//! string.
use super::*;
use crate::el::Elide;
use crate::{Color, Cx, El, Env, Key, KeyEvent, Link, Mods, Span, TextStyle, col, rich};
use unicode_segmentation::UnicodeSegmentation;

const PATH: &str = "~/Projects/rune/crates/rune-kit/src/components/settings/appearance_panel.rs";

fn style() -> TextStyle { TextStyle::new(13.0, 400.0).lh(16.0) }

/// Content, how it's cut, and a byte range that is both an inline-code span and a link.
type S = (&'static str, Elide, Option<(usize, usize)>);

fn pill() -> Color { crate::color::hex("#e6339a").unwrap() }

/// `content` in a `width` pt window, cut in the middle (or at its head).
fn runtime(content: &'static str, head: bool, width: f32) -> Runtime<S> {
    cut_at(content, if head { Elide::Head } else { Elide::Middle }, None, width)
}

fn cut_at(content: &'static str, elide: Elide, marked: Option<(usize, usize)>, width: f32) -> Runtime<S> {
    fn view(s: &S, _: &mut Cx<S>) -> El {
        let spans = s.2.map(|(a, b)| Span { mono: true, bg: Some(pill()), ..Span::new(a, b) }).into_iter().collect();
        let mut t = rich(s.0, spans).key("t").font(style()).selectable(true);
        if let Some((a, b)) = s.2 { t = t.link(Link::new(a..b, "marked")).on_link(|_: &mut S, _, _| {}); }
        col().w_full().child(match s.1 { Elide::End => t.truncate(), Elide::Middle => t.truncate_middle(), Elide::Head => t.truncate_head() })
    }
    let mut rt = Runtime::new((content, elide, marked), view, (width, 40.0), Env::new(true, 2.0));
    rt.set_srgb_blend(false);
    rt.frame(0.0);
    rt
}

fn node<'a>(rt: &'a Runtime<S>) -> &'a Node {
    rt.tree.nodes.iter().find(|n| n.key.as_deref() == Some("t")).unwrap()
}

/// The string node "t" draws, and its cut.
fn drawn(rt: &Runtime<S>) -> (String, Option<Elision>) {
    let n = node(rt);
    let content = &n.text.as_ref().unwrap().content;
    match n.elided {
        Some(e) => ([&content[..e.head], "…", &content[e.tail..]].concat(), Some(e)),
        None => (content.clone(), None),
    }
}

fn fits(rt: &Runtime<S>) -> bool {
    let n = node(rt);
    let w = (n.target[2] - n.target[0]) * 2.0;
    n.text_layout.as_ref().and_then(crate::text::TextLayout::single).unwrap().full_width() <= w + 0.5
}

#[test]
fn middle_truncation_keeps_both_ends_and_refits_on_resize() {
    let mut rt = runtime(PATH, false, 200.0);
    let (shown, e) = drawn(&rt);
    let e = e.expect("the path overflows 200 pt");
    assert!(fits(&rt), "{shown}");
    assert!(shown.starts_with("~/Projects") && shown.ends_with("panel.rs") && shown.contains('…'), "{shown}");
    let (head, tail) = (PATH[..e.head].graphemes(true).count(), PATH[e.tail..].graphemes(true).count());
    assert!(head.abs_diff(tail) <= 1, "both ends keep half: {shown}");
    let el = rt.elements().into_iter().find(|e| e.key.as_deref() == Some("t")).unwrap();
    assert_eq!(el.text.as_deref(), Some(PATH), "tools and accessibility read the whole path");

    rt.set_viewport((320.0, 40.0), 2.0);
    rt.frame(0.1);
    let (wider, _) = drawn(&rt);
    assert!(fits(&rt) && wider.len() > shown.len(), "a wider box keeps more: {wider}");

    rt.set_viewport((900.0, 40.0), 2.0);
    rt.frame(0.2);
    assert_eq!(drawn(&rt), (PATH.to_owned(), None), "room for it all: no cut");

    rt.set_viewport((200.0, 40.0), 2.0);
    rt.frame(0.3);
    assert_eq!(drawn(&rt).0, shown, "narrowing again cuts as before");
}

#[test]
fn head_truncation_keeps_the_end() {
    let rt = runtime(PATH, true, 160.0);
    let (shown, e) = drawn(&rt);
    assert_eq!(e.map(|e| e.head), Some(0));
    assert!(fits(&rt) && shown.starts_with('…') && shown.ends_with("appearance_panel.rs"), "{shown}");
}

#[test]
fn cuts_fall_between_graphemes() {
    const MIXED: &str = "👩‍👩‍👧‍👦 cafe\u{301} שלום עולם 🇯🇵 नमस्ते e\u{301}e\u{301}e\u{301} 👩‍👩‍👧‍👦 done";
    let mut rt = runtime(MIXED, false, 60.0);
    let bounds: Vec<usize> = MIXED.grapheme_indices(true).map(|(i, _)| i).chain([MIXED.len()]).collect();
    let mut cuts = 0;
    for w in (60..400).step_by(7) {
        rt.set_viewport((w as f32, 40.0), 2.0);
        rt.frame(1.0);
        let (shown, e) = drawn(&rt);
        let Some(e) = e else { continue };
        cuts += 1;
        assert!(fits(&rt), "{w}: {shown}");
        assert!(bounds.contains(&e.head) && bounds.contains(&e.tail), "{w}: {e:?} splits a grapheme: {shown}");
    }
    assert!(cuts > 10, "most widths cut: {cuts}");
}

#[test]
fn copying_a_cut_text_copies_all_of_it() {
    let mut rt = runtime(PATH, false, 200.0);
    let r = node(&rt).target;
    let (y, left, right) = ((r[1] + r[3]) / 2.0, r[0] + 0.5, r[2] - 0.5);

    // A drag across the ellipsis selects what it hides too.
    rt.pointer_move(left, y);
    rt.pointer_down(left, y);
    rt.frame(1.0 / 60.0);
    rt.pointer_move(right, y);
    rt.frame(2.0 / 60.0);
    rt.pointer_up(right, y);
    rt.frame(3.0 / 60.0);
    assert_eq!(rt.selected_text().as_deref(), Some(PATH));

    // So does Select All.
    rt.pointer_move(left + 20.0, y);
    rt.pointer_down(left + 20.0, y);
    rt.pointer_up(left + 20.0, y);
    rt.frame(1.0);
    rt.key(KeyEvent { key: Key::Command('a'), mods: Mods { cmd: true, ..Mods::default() } });
    rt.frame(1.1);
    assert_eq!(rt.selected_text().as_deref(), Some(PATH));
}

/// Instances filled with the code span's pill colour.
fn pills(rt: &Runtime<S>) -> usize {
    let want = pill().premul(1.0);
    rt.instances.iter().filter(|i| i.fill.iter().zip(want).all(|(a, b)| (a - b).abs() < 1e-3)).count()
}

#[test]
fn a_cut_hides_the_spans_and_links_inside_it() {
    // A link that is also an inline-code span, at the end, in the middle and at the start.
    let cases = [
        ("See the docs", Elide::End, (8, 12)),
        ("Open the config file in your editor today", Elide::Middle, (9, 15)),
        ("docs are here for you to read", Elide::Head, (0, 4)),
    ];
    for (content, elide, (a, b)) in cases {
        let mut rt = cut_at(content, elide, Some((a, b)), 20.0);
        let mut hidden = 0;
        for w in (20..300).step_by(3) {
            rt.set_viewport((w as f32, 40.0), 2.0);
            rt.frame(1.0);
            let (shown, e) = drawn(&rt);
            let gone = e.is_some_and(|e| a >= e.head && b <= e.tail);
            hidden += usize::from(gone);
            let link = rt.elements().iter().any(|e| e.path.ends_with("#link-0"));
            assert_eq!(link, !gone, "{w}: {shown}: the link is pressable only while it shows");
            assert_eq!(pills(&rt) > 0, !gone, "{w}: {shown}: the code pill draws only while it shows");
        }
        assert!(hidden > 0, "{content}: some width hides the link");
    }
}

#[test]
fn trimming_spaces_keeps_graphemes_whole() {
    // A space that begins a grapheme (before a combining accent) or ends one (after a
    // prepended mark) stays with it rather than being trimmed off beside the ellipsis.
    const SPACED: &str = "ab\u{600} cd \u{301}ef gh \u{301}ij";
    let bounds: Vec<usize> = SPACED.grapheme_indices(true).map(|(i, _)| i).chain([SPACED.len()]).collect();
    for elide in [Elide::End, Elide::Middle, Elide::Head] {
        let mut rt = cut_at(SPACED, elide, None, 10.0);
        for w in 10..140 {
            rt.set_viewport((w as f32, 40.0), 2.0);
            rt.frame(1.0);
            let (shown, e) = drawn(&rt);
            let Some(e) = e else { continue };
            assert!(bounds.contains(&e.head) && bounds.contains(&e.tail), "{elide:?} {w}: {shown:?}");
        }
    }
}

#[test]
fn elision_maps_ranges_across_the_cut() {
    // "abcdefgh" drawn "ab…gh".
    let e = Elision { head: 2, tail: 6 };
    assert_eq!(e.content(0), 0);
    assert_eq!(e.content(2), 2, "before the ellipsis");
    assert_eq!(e.content(5), 6, "after it: where the hidden middle ends");
    assert_eq!(e.content(7), 8);

    // Selections reaching into the cut highlight the ellipsis.
    assert_eq!(e.range(1, 4), (1, 5));
    assert_eq!(e.range(3, 8), (2, 7));
    assert_eq!(e.range(6, 8), (5, 7));

    // Links: hidden ones are empty, edges clamp, and only one showing on both sides takes "…".
    let empty = |(a, b): (usize, usize)| a == b;
    assert!(empty(e.mark(3, 5, true)) && empty(e.mark(2, 6, true)));
    assert_eq!(e.mark(1, 4, true), (1, 2));
    assert_eq!(e.mark(4, 8, true), (5, 7));
    assert_eq!(e.mark(1, 7, true), (1, 6));
    assert_eq!((e.mark(0, 2, true), e.mark(6, 8, true)), ((0, 2), (5, 7)));

    // Spans: also hidden when wholly cut, but "…" takes the style of the text it replaces
    // when that span shows beside it.
    assert!(empty(e.mark(3, 5, false)) && empty(e.mark(2, 6, false)));
    assert_eq!(e.mark(1, 4, false), (1, 5));
    assert_eq!(e.mark(2, 8, false), (2, 7));
    assert_eq!(e.mark(4, 8, false), (5, 7), "starting after the cut's first byte: not the ellipsis");
    assert_eq!(e.mark(1, 7, false), (1, 6));
}

#[test]
fn elision_cache_reuses_cut_evicts_superseded_and_bypasses_secure() {
    fn view(s: &String, _: &mut Cx<String>) -> El {
        col().w_full().child(crate::text(s.as_str()).key("t").font(style()).truncate_middle())
    }
    let mut rt = Runtime::new(PATH.to_owned(), view, (140.0, 40.0), Env::new(true, 2.0));
    rt.frame(0.0);
    let cut0 = rt.tree.nodes.iter().find(|n| n.key.as_deref() == Some("t")).unwrap().elided;
    assert!(cut0.is_some());
    assert_eq!(rt.text.cached_elision_count(), 1);
    let shaped_before = rt.text.shaped;
    rt.invalidate();
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.text.shaped, shaped_before, "repeat commit reuses cached truncated cut");
    assert_eq!(rt.tree.nodes.iter().find(|n| n.key.as_deref() == Some("t")).unwrap().elided, cut0);

    // Editing the string supersedes the old shaping and drops its cached cut on end_commit.
    rt.update(|s, _| s.push_str("/extra_leaf_module.rs"));
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.text.cached_elision_count(), 1, "superseded cut is replaced rather than accumulating");

    // Redacted (`secure`) text never enters the elision cache.
    fn secure_view(s: &String, _: &mut Cx<String>) -> El {
        col().w_full().child(crate::text(s.as_str()).key("t").font(style()).secure().revealed(true).truncate())
    }
    let mut sec = Runtime::new(PATH.to_owned(), secure_view, (140.0, 40.0), Env::new(true, 2.0));
    sec.frame(0.0);
    assert!(sec.tree.nodes.iter().find(|n| n.key.as_deref() == Some("t")).unwrap().elided.is_some());
    assert_eq!(sec.text.cached_elision_count(), 0, "redacted text bypasses elision cache");
}

#[test]
fn narrow_truncated_text_clips_to_box_and_draws_nothing_at_zero_width() {
    use crate::render::KIND_GLYPH;
    use crate::row;

    type RowState = (&'static str, Elide, f32, bool);
    fn row_view(s: &RowState, _: &mut Cx<RowState>) -> El {
        let spans = if s.3 { vec![Span { bg: Some(pill()), underline: Some(pill()), ..Span::new(0, s.0.len()) }] } else { Vec::new() };
        let t = rich(s.0, spans).key("t").font(style()).w(s.2);
        let t = match s.1 { Elide::End => t.truncate(), Elide::Middle => t.truncate_middle(), Elide::Head => t.truncate_head() };
        row().w(200.0).child(crate::el().w(10.0).h(16.0)).child(t)
    }

    let scale = 2.0;
    for elide in [Elide::End, Elide::Middle, Elide::Head] {
        // Zero-width box: draws no glyphs or decorations, keeps full a11y text.
        let mut zero = Runtime::new(("Hello world", elide, 0.0, true), row_view, (200.0, 40.0), Env::new(true, scale));
        zero.frame(0.0);
        let glyphs = zero.instances.iter().filter(|i| i.params[1] == KIND_GLYPH).count();
        assert_eq!(glyphs, 0, "{elide:?}: zero-width truncated text must draw no glyphs");
        let want_pill = pill().premul(1.0);
        let deco = zero.instances.iter().filter(|i| i.fill.iter().zip(want_pill).all(|(a, b)| (a - b).abs() < 1e-3)).count();
        assert_eq!(deco, 0, "{elide:?}: zero-width truncated text must draw no span decorations");
        let el = zero.elements().into_iter().find(|e| e.key.as_deref() == Some("t")).unwrap();
        assert_eq!(el.text.as_deref(), Some("Hello world"));

        // Box narrower than the ellipsis (5 pt < ~11 pt): every glyph and decoration clips to the box.
        let mut narrow = Runtime::new(("Hello world", elide, 5.0, true), row_view, (200.0, 40.0), Env::new(true, scale));
        narrow.frame(0.0);
        let n = narrow.tree.nodes.iter().find(|n| n.key.as_deref() == Some("t")).unwrap();
        let (x0, x1) = (n.target[0] * scale, n.target[2] * scale);
        let glyph_list: Vec<_> = narrow.instances.iter().filter(|i| i.params[1] == KIND_GLYPH).collect();
        assert!(!glyph_list.is_empty(), "{elide:?}: 5 pt box should paint clipped content");
        for g in &glyph_list {
            assert!(g.clip[0] >= x0 - 0.5 && g.clip[2] <= x1 + 0.5,
                "{elide:?}: glyph clip {:?} overflows box [{x0}, {x1}]", g.clip);
        }
        for d in narrow.instances.iter().filter(|i| i.fill.iter().zip(want_pill).all(|(a, b)| (a - b).abs() < 1e-3)) {
            assert!(d.clip[0] >= x0 - 0.5 && d.clip[2] <= x1 + 0.5,
                "{elide:?}: decoration clip {:?} overflows box [{x0}, {x1}]", d.clip);
        }
    }

    // End truncation keeps the first grapheme clipped rather than replacing it with "…" (CSS Overflow 3 / Blink).
    let mut end_narrow = Runtime::new(("Hello world", Elide::End, 5.0, false), row_view, (200.0, 40.0), Env::new(true, scale));
    end_narrow.frame(0.0);
    let n = end_narrow.tree.nodes.iter().find(|n| n.key.as_deref() == Some("t")).unwrap();
    let e = n.elided.expect("overflowing multi-grapheme text is elided");
    assert_eq!(e.head, 1, "first grapheme 'H' is kept before the ellipsis");
    assert_eq!(e.tail, "Hello world".len());

    // Single grapheme narrower than its own width stays un-ellipsized and clipped to the box.
    let mut single = Runtime::new(("W", Elide::End, 4.0, false), row_view, (200.0, 40.0), Env::new(true, scale));
    single.frame(0.0);
    let n = single.tree.nodes.iter().find(|n| n.key.as_deref() == Some("t")).unwrap();
    assert!(n.elided.is_none(), "single grapheme is clipped rather than replaced by an ellipsis");
    assert!(n.text_cut(scale), "clipped single grapheme still counts as truncated for tooltips");
    let (x0, x1) = (n.target[0] * scale, n.target[2] * scale);
    let glyphs: Vec<_> = single.instances.iter().filter(|i| i.params[1] == KIND_GLYPH).collect();
    assert!(!glyphs.is_empty());
    for g in glyphs {
        assert!(g.clip[0] >= x0 - 0.5 && g.clip[2] <= x1 + 0.5, "single glyph clip {:?} overflows [{x0}, {x1}]", g.clip);
    }
}

#[test]
fn shrink_preserves_explicit_min_w_in_either_order_and_leaves_min_h_alone() {
    use crate::{el, row};

    assert_eq!(std::mem::size_of::<rune_taffy::CompactStyle>(), 64, "CompactStyle keeps its 64-byte size");

    fn row_widths(row_w: f32) -> [f32; 5] {
        let mut rt = Runtime::new(
            row_w,
            |&w: &f32, _: &mut Cx<f32>| {
                col().size(400.0, 400.0)
                    .child(row().w(w).child(el().key("min_then_shrink").w(300.0).h(20.0).min_w(160.0).shrink()))
                    .child(row().w(w).child(el().key("shrink_then_min").w(300.0).h(20.0).shrink().min_w(160.0)))
                    .child(row().w(w).child(el().key("shrink_auto").h(20.0).shrink().child(el().w(300.0).h(20.0))))
                    .child(row().w(w).child(el().key("override_zero").w(300.0).h(20.0).min_w(160.0).shrink().min_w(0.0)))
                    .child(row().w(w).child({
                        let mut e = el().key("in_place").w(300.0).h(20.0);
                        e.set_min_w(160.0);
                        e.set_shrink();
                        e
                    }))
            },
            (400.0, 400.0),
            Env::new(true, 1.0),
        );
        rt.frame(0.0);
        let w_of = |k: &str| rt.presented(k).expect(k).0[2];
        [
            w_of("min_then_shrink"),
            w_of("shrink_then_min"),
            w_of("shrink_auto"),
            w_of("override_zero"),
            w_of("in_place"),
        ]
    }

    // In a 100 pt row: both orders stop at 160 pt; auto min_w and explicit min_w(0.0) shrink to 100 pt.
    let tight = row_widths(100.0);
    assert_eq!(tight, [160.0, 160.0, 100.0, 100.0, 160.0], "100 pt row: {tight:?}");

    // In a 200 pt row: 300 pt items with min_w(160) shrink to 200 pt in both orders.
    let medium = row_widths(200.0);
    assert_eq!(medium, [200.0, 200.0, 200.0, 200.0, 200.0], "200 pt row: {medium:?}");

    // min_h is untouched by shrink: vertical columns still need explicit .min_h(0.0), and explicit min_h stays.
    let mut rt_h = Runtime::new(
        (),
        |_: &(), _: &mut Cx<()>| {
            row().size(400.0, 200.0)
                .child(col().w(100.0).h(100.0).child(col().key("no_min_h").shrink().child(el().w(40.0).h(300.0))))
                .child(col().w(100.0).h(100.0).child(col().key("zero_min_h").shrink().min_h(0.0).child(el().w(40.0).h(300.0))))
                .child(col().w(100.0).h(100.0).child(col().key("kept_min_h").min_h(160.0).shrink().child(el().w(40.0).h(300.0))))
        },
        (400.0, 200.0),
        Env::new(true, 1.0),
    );
    rt_h.frame(0.0);
    assert_eq!(rt_h.presented("no_min_h").unwrap().0[3], 300.0, "shrink leaves auto min_h alone");
    assert_eq!(rt_h.presented("zero_min_h").unwrap().0[3], 100.0, "explicit min_h(0.0) allows vertical shrink");
    assert_eq!(rt_h.presented("kept_min_h").unwrap().0[3], 160.0, "explicit min_h is preserved");
}

#[test]
fn single_line_truncation_preserves_explicit_min_w_in_either_order() {
    use crate::row;

    const LONG: &str = "A very long single-line label that exceeds three hundred points when laid out at fourteen points";

    #[derive(Clone, Copy)]
    enum Kind { End, SetEnd, Middle, Head, Fade, FadeWidth }

    fn apply(t: El, kind: Kind) -> El {
        match kind {
            Kind::End => t.truncate(),
            Kind::SetEnd => {
                let mut t = t;
                t.set_truncate();
                t
            }
            Kind::Middle => t.truncate_middle(),
            Kind::Head => t.truncate_head(),
            Kind::Fade => t.truncate_fade(),
            Kind::FadeWidth => t.truncate_fade_width(2.0),
        }
    }

    for kind in [Kind::End, Kind::SetEnd, Kind::Middle, Kind::Head, Kind::Fade, Kind::FadeWidth] {
        for (row_w, want_min) in [(100.0_f32, 160.0_f32), (200.0_f32, 200.0_f32)] {
            let mut rt = Runtime::new(
                (row_w, kind),
                |&(w, k): &(f32, Kind), _: &mut Cx<(f32, Kind)>| {
                    let mk = |key: &'static str| crate::text(LONG).key(key).font(style());
                    col().size(500.0, 200.0)
                        .child(row().w(w).child(apply(mk("min_first").min_w(160.0), k)))
                        .child(row().w(w).child(apply(mk("trunc_first"), k).min_w(160.0)))
                        .child(row().w(w).child(apply(mk("auto_min"), k)))
                        .child(row().w(w).child(apply(mk("override_zero").min_w(160.0), k).min_w(0.0)))
                },
                (500.0, 200.0),
                Env::new(true, 1.0),
            );
            rt.frame(0.0);
            let w_of = |key: &str| rt.presented(key).expect(key).0[2];
            assert_eq!(w_of("min_first"), want_min, "row_w={row_w}: min_w then truncate");
            assert_eq!(w_of("trunc_first"), want_min, "row_w={row_w}: truncate then min_w");
            assert_eq!(w_of("auto_min"), row_w, "row_w={row_w}: auto min_w still shrinks to row");
            assert_eq!(w_of("override_zero"), row_w, "row_w={row_w}: trailing min_w(0.0) overrides earlier min_w");
        }
    }
}
