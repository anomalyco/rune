use std::cell::RefCell;

use super::*;
use crate::{Color, Cx, El, Env, Link, Runtime, TextHighlight, TextStyle, Tx, col, text};

struct Doc {
    words: Vec<(usize, usize, String)>,
    body: String,
    read_keys: Vec<String>,
    seen: RefCell<Vec<Option<[f32; 4]>>>,
    rev: usize,
}

fn make_doc(word_count: usize, read_keys: Vec<String>) -> Doc {
    let mut body = String::new();
    let mut words = Vec::with_capacity(word_count);
    for i in 0..word_count {
        if i > 0 {
            body.push(' ');
        }
        let start = body.len();
        body.push_str(&format!("word{i:02}"));
        let end = body.len();
        let key = if i == 2 || i == word_count - 3 {
            "shared".to_owned()
        } else {
            format!("w{i}")
        };
        words.push((start, end, key));
    }
    Doc {
        words,
        body,
        read_keys,
        seen: RefCell::new(Vec::new()),
        rev: 0,
    }
}

fn doc_view(s: &Doc, cx: &mut Cx<Doc>) -> El {
    *s.seen.borrow_mut() = s.read_keys.iter().map(|k| cx.window_layout(k)).collect();
    let mut t = text(s.body.clone())
        .key("doc")
        .font(TextStyle::new(14.0, 400.0).lh(20.0))
        .wrap()
        .w_full();
    for (start, end, key) in &s.words {
        t = t.text_highlight(TextHighlight::new(*start..*end, Color::WHITE).key(key.as_str()));
    }
    col().w(180.0).h(400.0).child(t)
}

fn settle(rt: &mut Runtime<Doc>) {
    for _ in 0..60 {
        if !rt.frame(1.0 / 60.0) {
            return;
        }
    }
    panic!("never settled");
}

fn union_rects(rects: impl IntoIterator<Item = [f32; 4]>) -> Option<[f32; 4]> {
    rects.into_iter().fold(None, |u: Option<[f32; 4]>, r| {
        Some(u.map_or(r, |u| [u[0].min(r[0]), u[1].min(r[1]), u[2].max(r[2]), u[3].max(r[3])]))
    })
}

fn close(a: [f32; 4], b: [f32; 4]) -> bool {
    a.iter().zip(b).all(|(x, y)| (x - y).abs() < 0.01)
}

fn text_node_index(rt: &Runtime<Doc>) -> usize {
    rt.tree.nodes.iter().position(|n| n.text.is_some()).expect("text node")
}

#[test]
fn keyed_highlight_target_lays_out_only_matching_highlights() {
    let mut rt = Runtime::new(make_doc(40, vec![]), doc_view, (180.0, 400.0), Env::new(true, 1.0));
    rt.frame(0.0);
    settle(&mut rt);

    let core = rt.core();
    let i = text_node_index(&rt);
    let all_boxes = core.highlight_boxes_in(&core.tree.nodes, i, core.tree.nodes[i].target, 1.0);
    let expected_for = |key: &str| {
        let spec = core.tree.nodes[i].text.as_ref().unwrap();
        let wanted: Vec<usize> = spec.cold.highlights.iter().enumerate()
            .filter_map(|(k, h)| (h.key.as_deref() == Some(key)).then_some(k))
            .collect();
        union_rects(all_boxes.iter().filter(|(k, _)| wanted.contains(k)).map(|&(_, r)| r)).unwrap()
    };

    let (single, single_queries) = count_span_box_queries(|| core.highlight_target_in(&core.tree.nodes, None, "w10"));
    let (node_single, rect_single) = single.expect("w10 target");
    assert_eq!(node_single, i);
    assert!(close(rect_single, expected_for("w10")), "got {rect_single:?}");
    assert_eq!(single_queries, 1, "single-key highlight read should query only its 1 span");

    let (shared, shared_queries) = count_span_box_queries(|| core.highlight_target_in(&core.tree.nodes, None, "shared"));
    let (node_shared, rect_shared) = shared.expect("shared target");
    assert_eq!(node_shared, i);
    assert!(close(rect_shared, expected_for("shared")), "got {rect_shared:?}");
    assert!(rect_shared[3] - rect_shared[1] > 25.0, "shared highlights should span multiple lines: {rect_shared:?}");
    assert_eq!(shared_queries, 2, "two-highlight shared key should query only its 2 spans");

    let (missing, missing_queries) = count_span_box_queries(|| core.highlight_target_in(&core.tree.nodes, None, "missing"));
    assert!(missing.is_none());
    assert_eq!(missing_queries, 0, "missing key should query 0 spans");
}

#[test]
fn keyed_highlight_inline_target_lays_out_only_matching_highlights() {
    let mut rt = Runtime::new(make_doc(40, vec![]), doc_view, (180.0, 400.0), Env::new(true, 1.0));
    rt.frame(0.0);
    settle(&mut rt);

    let core = rt.core();
    let i = text_node_index(&rt);
    let n = &core.tree.nodes[i];
    let spec = n.text.as_ref().unwrap();
    let all_inline = core.span_inline_boxes(n, n.target, 1.0, spec.cold.highlights.iter().map(|h| (h.start, h.end)));
    let expected_for = |key: &str| {
        let wanted: Vec<usize> = spec.cold.highlights.iter().enumerate()
            .filter_map(|(k, h)| (h.key.as_deref() == Some(key)).then_some(k))
            .collect();
        union_rects(all_inline.iter().filter(|(k, _)| wanted.contains(k)).map(|&(_, r)| r)).unwrap()
    };

    let (single, single_queries) = count_span_box_queries(|| core.highlight_inline_target_in(&core.tree.nodes, None, "w10"));
    let (node_single, rect_single) = single.expect("w10 inline target");
    assert_eq!(node_single, i);
    assert!(close(rect_single, expected_for("w10")), "got {rect_single:?}");
    assert_eq!(single_queries, 1, "single-key inline read should query only its 1 span");

    let (shared, shared_queries) = count_span_box_queries(|| core.highlight_inline_target_in(&core.tree.nodes, None, "shared"));
    let (node_shared, rect_shared) = shared.expect("shared inline target");
    assert_eq!(node_shared, i);
    assert!(close(rect_shared, expected_for("shared")), "got {rect_shared:?}");
    assert_eq!(shared_queries, 2, "two-highlight shared inline key should query only its 2 spans");

    let (missing, missing_queries) = count_span_box_queries(|| core.highlight_inline_target_in(&core.tree.nodes, None, "missing"));
    assert!(missing.is_none());
    assert_eq!(missing_queries, 0, "missing inline key should query 0 spans");
}

#[test]
fn find_highlight_lays_out_only_first_matching_highlight() {
    let mut rt = Runtime::new(make_doc(40, vec![]), doc_view, (180.0, 400.0), Env::new(true, 1.0));
    rt.frame(0.0);
    settle(&mut rt);

    let core = rt.core();
    let i = text_node_index(&rt);
    let drawn_boxes = core.highlight_boxes(i, core.drawn_box(i));
    let expected_center = |key: &str| {
        let spec = core.tree.nodes[i].text.as_ref().unwrap();
        let k = spec.cold.highlights.iter().position(|h| h.key.as_deref() == Some(key)).unwrap();
        let (_, r) = drawn_boxes.iter().find(|(j, _)| *j == k).unwrap();
        ((r[0] + r[2]) * 0.5, (r[1] + r[3]) * 0.5)
    };

    let (single, single_queries) = count_span_box_queries(|| core.find_highlight("w10"));
    let (sx, sy) = single.expect("w10 center");
    let (ex, ey) = expected_center("w10");
    assert!((sx - ex).abs() < 0.01 && (sy - ey).abs() < 0.01, "got ({sx}, {sy}), want ({ex}, {ey})");
    assert_eq!(single_queries, 1, "find_highlight should query only the matching highlight");

    let (shared, shared_queries) = count_span_box_queries(|| core.find_highlight("shared"));
    let (sx, sy) = shared.expect("shared center");
    let (ex, ey) = expected_center("shared");
    assert!((sx - ex).abs() < 0.01 && (sy - ey).abs() < 0.01, "got ({sx}, {sy}), want ({ex}, {ey})");
    assert_eq!(shared_queries, 1, "find_highlight should query only the first matching highlight");

    let (missing, missing_queries) = count_span_box_queries(|| core.find_highlight("missing"));
    assert!(missing.is_none());
    assert_eq!(missing_queries, 0, "missing find_highlight should query 0 spans");
}

#[test]
fn window_layout_reads_scale_with_reads_not_total_highlights() {
    let keys = vec!["w3".to_owned(), "w17".to_owned(), "w33".to_owned(), "shared".to_owned()];
    let mut rt = Runtime::new(make_doc(40, keys.clone()), doc_view, (180.0, 400.0), Env::new(true, 1.0));
    rt.frame(0.0);
    settle(&mut rt);

    // Trigger a commit that re-runs layout read settling for the 4 keyed reads (5 matching highlights
    // out of 40 total highlights in the text).
    let ((), queries) = count_span_box_queries(|| {
        rt.update(|s, _| s.rev += 1);
        settle(&mut rt);
    });

    // Without the fix, each settle pass lays out all 40 highlights per read (4 * 40 = 160 queries
    // per pass). With the fix, each pass lays out only the 5 matching highlights (1 + 1 + 1 + 2).
    assert!(queries > 0, "expected layout reads to run during settle");
    assert!(queries <= 15, "expected queries proportional to matched highlights (5/pass), got {queries}");

    let els = rt.elements();
    let seen = rt.state.seen.borrow().clone();
    for (key, got) in keys.iter().zip(seen) {
        let got = got.unwrap_or_else(|| panic!("missing window_layout for {key}"));
        let mark_rects: Vec<[f32; 4]> = els.iter()
            .filter(|e| e.role == "mark" && e.key.as_deref() == Some(key.as_str()))
            .map(|e| [e.rect[0], e.rect[1], e.rect[0] + e.rect[2], e.rect[1] + e.rect[3]])
            .collect();
        let u = union_rects(mark_rects).expect("marks in tree");
        let want = [u[0], u[1], u[2] - u[0], u[3] - u[1]];
        assert!(
            got.iter().zip(want).all(|(a, b)| (a - b).abs() < 0.75),
            "{key}: window_layout {got:?} != mark union {want:?}"
        );
    }
}

struct LinkDoc {
    links: Vec<(usize, usize, String)>,
    body: String,
    with_secondary: bool,
    menus: Vec<(String, f32, f32)>,
}

fn make_link_doc(link_count: usize, with_secondary: bool) -> LinkDoc {
    let mut body = String::new();
    let mut links = Vec::with_capacity(link_count);
    for i in 0..link_count {
        if i > 0 {
            body.push(' ');
        }
        let start = body.len();
        let (label, id) = if i == 15 {
            ("long_wrapped_link_phrase across_two_lines".to_owned(), "wrapped".to_owned())
        } else if i == 2 || i == link_count - 3 {
            (format!("link{i:02}"), "dup".to_owned())
        } else {
            (format!("link{i:02}"), format!("l{i}"))
        };
        body.push_str(&label);
        let end = body.len();
        links.push((start, end, id));
    }
    LinkDoc {
        links,
        body,
        with_secondary,
        menus: Vec::new(),
    }
}

fn link_doc_view(s: &LinkDoc, _: &mut Cx<LinkDoc>) -> El {
    let mut t = text(s.body.clone())
        .key("doc")
        .font(TextStyle::new(14.0, 400.0).lh(20.0))
        .wrap()
        .w_full();
    for (start, end, id) in &s.links {
        t = t.link(Link::new(*start..*end, id.as_str()));
    }
    if s.with_secondary {
        t = t.on_link_secondary(|s: &mut LinkDoc, id, ev, _| {
            s.menus.push((id.to_owned(), ev.x, ev.y));
        });
    }
    col().w(180.0).h(500.0).child(t)
}

fn settle_link_doc(rt: &mut Runtime<LinkDoc>) {
    for _ in 0..60 {
        if !rt.frame(1.0 / 60.0) {
            return;
        }
    }
    panic!("never settled");
}

fn link_text_node_index(rt: &Runtime<LinkDoc>) -> usize {
    rt.tree.nodes.iter().position(|n| n.text.is_some()).expect("text node")
}

#[test]
fn find_link_lays_out_only_first_matching_link() {
    let mut rt = Runtime::new(make_link_doc(40, false), link_doc_view, (180.0, 500.0), Env::new(true, 1.0));
    rt.frame(0.0);
    settle_link_doc(&mut rt);

    let core = rt.core();
    let i = link_text_node_index(&rt);
    let drawn_boxes = core.link_boxes(i, core.drawn_box(i));
    let link_index = |id: &str| {
        core.tree.nodes[i].text.as_ref().unwrap().cold.links.iter().position(|l| l.id == id).unwrap()
    };
    let expected_center = |id: &str| {
        let k = link_index(id);
        let (_, r) = drawn_boxes.iter().find(|(j, _)| *j == k).unwrap();
        ((r[0] + r[2]) * 0.5, (r[1] + r[3]) * 0.5)
    };

    let wrapped_k = link_index("wrapped");
    let wrapped_boxes = drawn_boxes.iter().filter(|(j, _)| *j == wrapped_k).count();
    assert!(wrapped_boxes >= 2, "expected wrapped link to span at least 2 line boxes, got {wrapped_boxes}");

    let (single, single_queries) = count_span_box_queries(|| core.find_link("l10"));
    let (sx, sy) = single.expect("l10 center");
    let (ex, ey) = expected_center("l10");
    assert!((sx - ex).abs() < 0.01 && (sy - ey).abs() < 0.01, "got ({sx}, {sy}), want ({ex}, {ey})");
    assert_eq!(single_queries, 1, "find_link should query only the matching link");

    let (dup, dup_queries) = count_span_box_queries(|| core.find_link("dup"));
    let (sx, sy) = dup.expect("dup center");
    let (ex, ey) = expected_center("dup");
    assert!((sx - ex).abs() < 0.01 && (sy - ey).abs() < 0.01, "got ({sx}, {sy}), want ({ex}, {ey})");
    assert_eq!(dup_queries, 1, "find_link should query only the first matching link");

    let (wrapped, wrapped_queries) = count_span_box_queries(|| core.find_link("wrapped"));
    let (sx, sy) = wrapped.expect("wrapped center");
    let (ex, ey) = expected_center("wrapped");
    assert!((sx - ex).abs() < 0.01 && (sy - ey).abs() < 0.01, "got ({sx}, {sy}), want ({ex}, {ey})");
    assert_eq!(wrapped_queries, 1, "find_link on a wrapped link should query only that link");

    let (missing, missing_queries) = count_span_box_queries(|| core.find_link("missing"));
    assert!(missing.is_none());
    assert_eq!(missing_queries, 0, "missing find_link should query 0 spans");
}

#[test]
fn focused_link_menu_lays_out_only_focused_link() {
    let mut rt = Runtime::new(make_link_doc(40, true), link_doc_view, (180.0, 500.0), Env::new(true, 1.0));
    rt.frame(0.0);
    settle_link_doc(&mut rt);

    let i = link_text_node_index(&rt);
    let drawn_boxes = rt.core().link_boxes(i, rt.core().drawn_box(i));
    let expected_bottom_left = |rt: &Runtime<LinkDoc>, id: &str| {
        let k = rt.core().tree.nodes[i].text.as_ref().unwrap().cold.links.iter().position(|l| l.id == id).unwrap();
        let (_, r) = drawn_boxes.iter().find(|(j, _)| *j == k).unwrap();
        (r[0], r[3])
    };

    for id in ["l10", "wrapped"] {
        rt.update(|_, tx| tx.focus(id, true));
        settle_link_doc(&mut rt);
        assert_eq!(rt.focused_key().as_deref(), Some(id));
        let (ex, ey) = expected_bottom_left(&rt, id);

        let (handled, queries) = count_span_box_queries(|| {
            let (core, mut app) = rt.split_mut();
            let mut tx = Tx::default();
            core.focused_link_menu(&mut app, i, &mut tx)
        });
        assert!(handled, "focused_link_menu should handle {id}");
        assert_eq!(queries, 1, "focused_link_menu should query only the focused link ({id})");
        let (got_id, gx, gy) = rt.state.menus.pop().expect("on_link_secondary hit");
        assert_eq!(got_id, id);
        assert!((gx - ex).abs() < 0.01 && (gy - ey).abs() < 0.01, "{id}: got ({gx}, {gy}), want ({ex}, {ey})");
    }

    let mut no_menu = Runtime::new(make_link_doc(40, false), link_doc_view, (180.0, 500.0), Env::new(true, 1.0));
    no_menu.frame(0.0);
    settle_link_doc(&mut no_menu);
    let j = link_text_node_index(&no_menu);
    no_menu.update(|_, tx| tx.focus("l10", true));
    settle_link_doc(&mut no_menu);
    let (handled, queries) = count_span_box_queries(|| {
        let (core, mut app) = no_menu.split_mut();
        let mut tx = Tx::default();
        core.focused_link_menu(&mut app, j, &mut tx)
    });
    assert!(!handled);
    assert_eq!(queries, 0, "focused_link_menu without on_link_secondary should query 0 spans");
}

#[test]
fn link_fx_decos_scans_each_lines_end_once_and_matches_underline_edges() {
    let mut ts = crate::text::TextSystem::default();
    let scale = 2.0;
    let origin = [11.25, 23.5];

    let build = |ts: &mut crate::text::TextSystem, n: usize| {
        let mut src = String::new();
        let mut pills = Vec::with_capacity(n);
        for i in 0..n {
            src.push_str("a ");
            let start = src.len() as u32;
            src.push_str("p");
            let end = src.len() as u32;
            pills.push(crate::text::PillChip { start, end, icon: None, image: None, bg: None, fg: None, ring: None, radius: None, pad_x: None, margin_x: None, bare: None });
            if i + 1 < n {
                src.push(' ');
            }
        }
        let layout = ts.layout_blocks(&src, &[], &[], &pills, TextStyle::new(14.0, 400.0), scale, None, parley::Alignment::Start);
        (src, layout)
    };

    let measure = |ts: &mut crate::text::TextSystem, n: usize| -> (usize, usize, usize, Vec<PaintedDeco>, String, std::rc::Rc<parley::Layout<crate::text::Ink>>) {
        let (src, layout) = build(ts, n);
        let line = layout.get(0).expect("single unwrapped line");
        let run_count = line.items().filter(|it| matches!(it, parley::PositionedLayoutItem::GlyphRun(_))).count();
        let link_boxes = boxes(&layout, src.len(), 0, src.len() as u32);
        let fx = LinkFx(vec![
            (0, src.len() as u32, crate::text::LinkPaint::new().underline(Color::WHITE), 2.0, false),
            (0, src.len() as u32, crate::text::LinkPaint::new().underline(Color::BLACK), 2.0, false),
        ]);
        let _ = take_link_line_end_visits();
        let mut out = Vec::new();
        fx.decos(&layout, src.len(), scale, origin, &mut out);
        let visits = take_link_line_end_visits();
        (visits, run_count, link_boxes.len(), out, src, layout)
    };

    let (v40, runs40, boxes40, ..) = measure(&mut ts, 40);
    let (v80, runs80, boxes80, actual, src, layout) = measure(&mut ts, 80);
    assert!(boxes40 >= 80 && boxes80 >= 160, "pills should split the link into many boxes (got {boxes40}, {boxes80})");
    assert!(
        v80 <= runs80,
        "LinkFx::decos should compute line_end once per line, not per box (got {v80} visits for {runs80} runs and {boxes80} boxes; v40={v40}, runs40={runs40})"
    );

    // Every underline rect must match the per-box underline_edges calculation identically.
    let line = layout.get(0).unwrap();
    let y = (origin[1] + line.metrics().baseline + 2.0 * scale).round();
    let expected: Vec<_> = boxes(&layout, src.len(), 0, src.len() as u32)
        .into_iter()
        .map(|(bb, _)| {
            let (x0, x1) = crate::text::underline_edges(&line, origin[0], bb.x0 as f32, bb.x1 as f32);
            ([x0, y, x1 - x0, scale.max(1.0)], Color::BLACK, 0.0f32)
        })
        .collect();
    let got: Vec<_> = actual.iter().map(|d| (d.rect, d.color, d.radius)).collect();
    assert_eq!(got, expected, "LinkFx::decos underlines must match underline_edges identically");
}

fn reference_link_decos(
    fx: &LinkFx,
    layout: &parley::Layout<crate::text::Ink>,
    len: usize,
    s: f32,
    origin: [f32; 2],
    out: &mut Vec<PaintedDeco>,
) {
    for &(a, b, p, bg_pad, has_rings) in &fx.0 {
        for (bb, line) in boxes(layout, len, a, b) {
            let (x0, w) = (origin[0] + bb.x0 as f32, (bb.x1 - bb.x0) as f32);
            if p.bg.is_some() || p.ring.is_some() {
                let pad = bg_pad * s;
                let (mut y0, mut y1) = ((origin[1] + bb.y0 as f32).round(), (origin[1] + bb.y1 as f32).round());
                if let Some(l) = layout.get(line) {
                    let base = origin[1] + l.metrics().baseline;
                    let (mut asc, mut desc) = (0.0f32, 0.0f32);
                    for run in l.runs() {
                        let r = run.text_range();
                        if r.start < b as usize && (a as usize) < r.end {
                            let fm = run.font_metrics();
                            (asc, desc) = (asc.max(fm.ascent), desc.max(fm.descent.abs()));
                        }
                    }
                    if asc > 0.0 || desc > 0.0 {
                        y0 = (base - asc - 1.0 * s).round();
                        y1 = (base + desc + 1.0 * s).round();
                    }
                }
                let rect = [x0 - pad, y0, w + 2.0 * pad, y1 - y0];
                let ring = p.ring.map(|(c, rw)| (c, rw * s));
                let contained = |d: &&PaintedDeco| {
                    d.radius > 0.0
                        && d.rect[0] >= rect[0] - 1.5
                        && d.rect[0] + d.rect[2] <= rect[0] + rect[2] + 1.5
                        && d.rect[1] < y1
                        && y0 < d.rect[1] + d.rect[3]
                };
                let split = has_rings
                    && out.iter().filter(contained).any(|d| d.ring.is_some())
                    && out.iter().filter(contained).any(|d| (d.rect[0] - rect[0]).abs() < 1.5)
                    && out.iter().filter(contained).any(|d| (d.rect[0] + d.rect[2] - rect[0] - rect[2]).abs() < 1.5);
                let mut matched = false;
                for d in out.iter_mut().filter(|d| {
                    d.radius > 0.0
                        && d.rect[0] >= rect[0] - 1.5
                        && d.rect[0] + d.rect[2] <= rect[0] + rect[2] + 1.5
                        && (split || ((d.rect[0] - rect[0]).abs() < 1.5 && (d.rect[2] - rect[2]).abs() < 1.5))
                        && d.rect[1] < y1
                        && y0 < d.rect[1] + d.rect[3]
                }) {
                    matched = true;
                    if let Some(c) = p.bg {
                        d.color = c;
                    }
                    if ring.is_some() {
                        d.ring = ring;
                    }
                }
                if !matched {
                    out.push(PaintedDeco {
                        deco: Deco { rect, color: p.bg.unwrap_or(Color::TRANSPARENT), radius: 4.0 * s },
                        ring,
                        corners: None,
                        byte: b.saturating_sub(1),
                    });
                }
            }
            if let (Some(c), Some(l)) = (p.underline, layout.get(line)) {
                let y = (origin[1] + l.metrics().baseline + 2.0 * s).round();
                let (x0, x1) = crate::text::underline_edges(&l, origin[0], bb.x0 as f32, bb.x1 as f32);
                let w = x1 - x0;
                if let Some(d) = out.iter_mut().find(|d| {
                    d.radius == 0.0
                        && (d.rect[0] - x0).abs() < 1.5
                        && (d.rect[2] - w).abs() < 1.5
                        && (d.rect[1] - y).abs() < 1.5
                }) {
                    d.color = c;
                } else {
                    out.push(PaintedDeco {
                        deco: Deco { rect: [x0, y, w, s.max(1.0)], color: c, radius: 0.0 },
                        ring: None,
                        corners: None,
                        byte: 0,
                    });
                }
            }
        }
    }
}

fn deco_snapshot(out: &[PaintedDeco]) -> Vec<([f32; 4], Color, f32, Option<(Color, f32)>, u32)> {
    out.iter().map(|d| (d.rect, d.color, d.radius, d.ring, d.byte)).collect()
}

fn clone_decos(out: &[PaintedDeco]) -> Vec<PaintedDeco> {
    out.iter().map(|d| PaintedDeco { deco: d.deco, ring: d.ring, corners: d.corners, byte: d.byte }).collect()
}

#[test]
fn link_fx_decos_bg_and_underline_scans_scale_linearly_with_boxes() {
    let mut ts = crate::text::TextSystem::default();
    let scale = 2.0;
    let origin = [11.25, 23.5];
    let bg_rest = Color { r: 0.2, g: 0.3, b: 0.4, a: 1.0 };
    let bg_hover = Color { r: 0.5, g: 0.6, b: 0.7, a: 1.0 };
    let ring_col = Color { r: 0.1, g: 0.4, b: 0.9, a: 1.0 };

    // Case 1: Single link split by n pills on one line (many boxes + many runs on line 0,
    // with pre-existing pill chips, resting backgrounds, rings and underlines in `out`).
    let measure_single_line = |ts: &mut crate::text::TextSystem, n: usize| {
        let mut src = String::new();
        let mut pills = Vec::with_capacity(n);
        let mut spans = Vec::with_capacity(n);
        let mut rings = Vec::with_capacity(n);
        for i in 0..n {
            let ws = src.len();
            src.push_str("ab");
            let we = src.len();
            spans.push(crate::Span::new(ws, we).bg(bg_rest, 2.0).underline(Color::WHITE));
            if i % 3 == 0 {
                rings.push((ws as u32, we as u32, ring_col, 1.0));
            }
            src.push(' ');
            let ps = src.len() as u32;
            src.push('p');
            let pe = src.len() as u32;
            pills.push(crate::text::PillChip { start: ps, end: pe, icon: None, image: None, bg: None, fg: None, ring: None, radius: None, pad_x: None, margin_x: None, bare: None });
            if i + 1 < n {
                src.push(' ');
            }
        }
        let layout = ts.layout_blocks(&src, &spans, &[], &pills, TextStyle::new(14.0, 400.0), scale, None, parley::Alignment::Start);
        let mut base_out = Vec::new();
        let mut icons = Vec::new();
        crate::text::pill_chips(&layout, &pills, 14.0, scale, origin, bg_rest, &mut base_out, &mut icons);
        ts.decorations_with_rings(&layout, &spans, &rings, 4.0, scale, origin, &mut base_out);
        let fx = LinkFx(vec![(
            0,
            src.len() as u32,
            crate::text::LinkPaint::new().bg(bg_hover).underline(Color::BLACK),
            2.0,
            true,
        )]);
        let mut got = clone_decos(&base_out);
        let _ = take_link_deco_visits();
        fx.decos(&layout, src.len(), scale, origin, &mut got);
        let visits = take_link_deco_visits();
        let mut expected = base_out;
        reference_link_decos(&fx, &layout, src.len(), scale, origin, &mut expected);
        assert_eq!(deco_snapshot(&got), deco_snapshot(&expected), "single-line link decos must match reference");
        visits
    };

    let (r40, b40, u40) = measure_single_line(&mut ts, 40);
    let (r80, b80, u80) = measure_single_line(&mut ts, 80);

    // Case 2: Single wrapped link spanning n lines (no pills), with both bg and underline.
    let measure_multiline = |ts: &mut crate::text::TextSystem, n: usize| {
        let src = (0..n).map(|i| format!("line{i:03}")).collect::<Vec<_>>().join("\n");
        let spans = vec![crate::Span::new(0, src.len()).bg(bg_rest, 2.0).underline(Color::WHITE)];
        let layout = ts.layout_rich(&src, &spans, TextStyle::new(14.0, 400.0), scale, None);
        let mut base_out = Vec::new();
        ts.decorations_with_rings(&layout, &spans, &[], 4.0, scale, origin, &mut base_out);
        let fx = LinkFx(vec![(
            0,
            src.len() as u32,
            crate::text::LinkPaint::new().bg(bg_hover).underline(Color::BLACK),
            2.0,
            false,
        )]);
        let mut got = clone_decos(&base_out);
        let _ = take_link_deco_visits();
        fx.decos(&layout, src.len(), scale, origin, &mut got);
        let visits = take_link_deco_visits();
        let mut expected = base_out;
        reference_link_decos(&fx, &layout, src.len(), scale, origin, &mut expected);
        assert_eq!(deco_snapshot(&got), deco_snapshot(&expected), "multi-line link decos must match reference");
        visits
    };

    let (mr40, mb40, mu40) = measure_multiline(&mut ts, 40);
    let (mr80, mb80, mu80) = measure_multiline(&mut ts, 80);

    // Case 3: n distinct links on a single line in LinkFx, with resting spans and underlines.
    let measure_many_links = |ts: &mut crate::text::TextSystem, n: usize| {
        let mut src = String::new();
        let mut spans = Vec::with_capacity(n);
        let mut fx_items = Vec::with_capacity(n);
        for i in 0..n {
            if i > 0 {
                src.push(' ');
            }
            let a = src.len();
            src.push_str(&format!("lnk{i:03}"));
            let b = src.len();
            let sp = crate::Span::new(a, b).bg(bg_rest, 2.0).underline(Color::WHITE);
            spans.push(if i % 2 == 0 { sp.bold() } else { sp });
            fx_items.push((
                a as u32,
                b as u32,
                crate::text::LinkPaint::new().bg(bg_hover).underline(Color::BLACK),
                2.0,
                false,
            ));
        }
        let layout = ts.layout_rich(&src, &spans, TextStyle::new(14.0, 400.0), scale, None);
        let mut base_out = Vec::new();
        ts.decorations_with_rings(&layout, &spans, &[], 4.0, scale, origin, &mut base_out);
        let fx = LinkFx(fx_items);
        let mut got = clone_decos(&base_out);
        let _ = take_link_deco_visits();
        fx.decos(&layout, src.len(), scale, origin, &mut got);
        let visits = take_link_deco_visits();
        let mut expected = base_out;
        reference_link_decos(&fx, &layout, src.len(), scale, origin, &mut expected);
        assert_eq!(deco_snapshot(&got), deco_snapshot(&expected), "many-links decos must match reference");
        visits
    };

    let (lr40, lb40, lu40) = measure_many_links(&mut ts, 40);
    let (lr80, lb80, lu80) = measure_many_links(&mut ts, 80);
    assert!(r80 > 0 && b80 > 0 && u80 > 0 && mb80 > 0 && mu80 > 0 && lr80 > 0 && lb80 > 0 && lu80 > 0);
    assert!(
        r80 * 10 <= r40 * 25,
        "bg run visits on one line should scale ~2x when doubling n, not ~4x (r40={r40}, r80={r80})"
    );
    assert!(
        b80 * 10 <= b40 * 25,
        "bg out visits on one line should scale ~2x when doubling n, not ~4x (b40={b40}, b80={b80})"
    );
    assert!(
        u80 * 10 <= u40 * 25,
        "underline out visits on one line should scale ~2x when doubling n, not ~4x (u40={u40}, u80={u80})"
    );
    assert!(
        mr80 * 10 <= mr40 * 25,
        "multi-line bg run visits should scale ~2x (mr40={mr40}, mr80={mr80})"
    );
    assert!(
        mb80 * 10 <= mb40 * 25,
        "multi-line bg out visits should scale ~2x (mb40={mb40}, mb80={mb80})"
    );
    assert!(
        mu80 * 10 <= mu40 * 25,
        "multi-line underline out visits should scale ~2x (mu40={mu40}, mu80={mu80})"
    );
    assert!(
        lr80 * 10 <= lr40 * 25,
        "many-links bg run visits should scale ~2x (lr40={lr40}, lr80={lr80})"
    );
    assert!(
        lb80 * 10 <= lb40 * 25,
        "many-links bg out visits should scale ~2x (lb40={lb40}, lb80={lb80})"
    );
    assert!(
        lu80 * 10 <= lu40 * 25,
        "many-links underline out visits should scale ~2x (lu40={lu40}, lu80={lu80})"
    );
}

#[test]
fn link_fx_decos_matches_reference_for_split_rings_inline_code_and_overlaps() {
    let mut ts = crate::text::TextSystem::default();
    let scale = 2.0;
    let origin = [7.5, 13.0];
    let bg_rest = Color { r: 0.9, g: 0.9, b: 0.9, a: 1.0 };
    let bg_code = Color { r: 0.8, g: 0.8, b: 0.85, a: 1.0 };
    let bg_hover = Color { r: 0.3, g: 0.5, b: 0.9, a: 1.0 };
    let bg_focus = Color { r: 0.2, g: 0.7, b: 0.4, a: 1.0 };
    let ring_col = Color { r: 0.1, g: 0.2, b: 0.8, a: 1.0 };

    // 1. Ring splits one uniform link background into three pieces; hover recolors all three
    //    in place, while a second link with only an isolated inline-code span inside it pushes
    //    a fresh hover background rather than mistaking the code chip for the whole link.
    let src = "aa middle zz  and  pre code post";
    let (l1a, l1b) = (0usize, 12usize);
    let (l2a, l2b) = (19usize, src.len());
    let code_a = src.find("code").unwrap();
    let spans = vec![
        crate::Span::new(l1a, l1b).bg(bg_rest, 3.0).underline(Color::WHITE),
        crate::Span::new(code_a, code_a + 4).mono().bg(bg_code, 2.0),
    ];
    let rings = vec![(3u32, 9u32, ring_col, 1.0)];
    let layout = ts.layout_rich(src, &spans, TextStyle::new(14.0, 400.0), scale, None);
    let mut base_out = Vec::new();
    ts.decorations_with_rings(&layout, &spans, &rings, 4.0, scale, origin, &mut base_out);

    // Exercise both single-entry (fast path) and multi-entry (indexed path), including duplicate
    // overlapping link paints that recolor a newly-pushed background/underline in place.
    let cases = vec![
        LinkFx(vec![(l1a as u32, l1b as u32, crate::text::LinkPaint::new().bg(bg_hover).underline(Color::BLACK), 3.0, true)]),
        LinkFx(vec![(l2a as u32, l2b as u32, crate::text::LinkPaint::new().bg(bg_hover).underline(Color::BLACK), 2.0, true)]),
        LinkFx(vec![
            (l1a as u32, l1b as u32, crate::text::LinkPaint::new().bg(bg_hover).underline(Color::BLACK), 3.0, true),
            (l2a as u32, l2b as u32, crate::text::LinkPaint::new().bg(bg_hover).underline(Color::BLACK), 2.0, true),
            (l2a as u32, l2b as u32, crate::text::LinkPaint::new().bg(bg_focus).ring(ring_col, 1.0).underline(Color::WHITE), 2.0, true),
        ]),
    ];
    for (idx, fx) in cases.iter().enumerate() {
        let mut got = clone_decos(&base_out);
        fx.decos(&layout, src.len(), scale, origin, &mut got);
        let mut expected = clone_decos(&base_out);
        reference_link_decos(fx, &layout, src.len(), scale, origin, &mut expected);
        assert_eq!(deco_snapshot(&got), deco_snapshot(&expected), "case {idx} must match reference");
    }
}
fn reference_link_at(core: &RuntimeCore, i: usize, x: f32, y: f32) -> Option<usize> {
    if core.tree.nodes[i].text.as_ref().is_none_or(|t| t.cold.links.is_empty()) { return None; }
    let (rect, ux, uy) = core.unturned_box_and_point(i, x, y);
    core.link_boxes(i, rect).into_iter()
        .find(|(_, r)| contains(*r, ux, uy)).map(|(k, _)| k)
}

#[test]
fn link_at_lays_out_only_candidate_link_and_matches_full_scan() {
    let mut rt = Runtime::new(make_link_doc(40, false), link_doc_view, (180.0, 500.0), Env::new(true, 1.0));
    rt.frame(0.0);
    settle_link_doc(&mut rt);

    let i = link_text_node_index(&rt);
    let drawn_boxes = rt.core().link_boxes(i, rt.core().drawn_box(i));
    let link_boxes_of = |k: usize| -> Vec<[f32; 4]> {
        drawn_boxes.iter().filter(|(j, _)| *j == k).map(|&(_, r)| r).collect()
    };
    let mid = |r: [f32; 4]| ((r[0] + r[2]) * 0.5, (r[1] + r[3]) * 0.5);

    // First, middle, last, and both lines of the wrapped link (index 15).
    let wrapped_rects = link_boxes_of(15);
    assert!(wrapped_rects.len() >= 2, "expected wrapped link on 2+ lines, got {}", wrapped_rects.len());
    let hits = [
        ("first", 0usize, mid(link_boxes_of(0)[0])),
        ("middle", 10usize, mid(link_boxes_of(10)[0])),
        ("wrapped line 1", 15usize, mid(wrapped_rects[0])),
        ("wrapped line 2", 15usize, mid(wrapped_rects[1])),
        ("last", 39usize, mid(link_boxes_of(39)[0])),
    ];
    for (label, want, (x, y)) in hits {
        let (got, queries) = count_span_box_queries(|| rt.core().link_at(i, x, y));
        assert_eq!(got, Some(want), "{label}: link_at mismatch");
        assert!(queries <= 1, "{label}: expected <= 1 span-box query, got {queries}");
    }

    // Points over no link: space between link 0 and link 1, right of line, and below text.
    let (r0, r1) = (link_boxes_of(0)[0], link_boxes_of(1)[0]);
    assert!((r0[1] - r1[1]).abs() < 0.01 && r1[0] > r0[2] + 1.0, "link 0 and 1 should sit on one line with a space");
    let misses = [
        ("space between links", ((r0[2] + r1[0]) * 0.5, (r0[1] + r0[3]) * 0.5)),
        ("right margin", (175.0, (r0[1] + r0[3]) * 0.5)),
        ("above text", (r0[0] + 2.0, r0[1] - 5.0)),
        ("below text", (20.0, 490.0)),
        ("nan", (f32::NAN, r0[1])),
    ];
    for (label, (x, y)) in misses {
        let (got, queries) = count_span_box_queries(|| rt.core().link_at(i, x, y));
        assert_eq!(got, None, "{label}: expected miss");
        assert_eq!(queries, 0, "{label}: miss should query 0 spans, got {queries}");
    }

    // Exhaustive equivalence against full link_boxes scan across every link's interior and edges.
    for &(k, r) in &drawn_boxes {
        let cy = (r[1] + r[3]) * 0.5;
        let cx = (r[0] + r[2]) * 0.5;
        for (px, py) in [
            (cx, cy),
            (r[0], r[1]),
            (r[0] - 0.1, cy),
            (r[2] - 0.01, r[3] - 0.01),
            (r[2], cy),
            (cx, r[3]),
        ] {
            let want = reference_link_at(rt.core(), i, px, py);
            let got = rt.core().link_at(i, px, py);
            assert_eq!(got, want, "link {k} point ({px}, {py}) diverged from full scan");
        }
    }

    // Pointer moves run observe_pointer -> hovered_link() -> link_at() twice per move;
    // moving across links and plain text must stay bounded, not 2 * 40 queries per move.
    let (p0, p39) = (mid(link_boxes_of(0)[0]), mid(link_boxes_of(39)[0]));
    let gap = ((r0[2] + r1[0]) * 0.5, (r0[1] + r0[3]) * 0.5);
    let (_, q_enter) = count_span_box_queries(|| rt.observe_pointer(p0.0, p0.1));
    let (_, q_stay) = count_span_box_queries(|| rt.observe_pointer(p0.0 + 1.0, p0.1));
    let (_, q_gap) = count_span_box_queries(|| rt.observe_pointer(gap.0, gap.1));
    let (_, q_last) = count_span_box_queries(|| rt.observe_pointer(p39.0, p39.1));
    assert!(q_enter <= 2 && q_stay <= 2 && q_gap <= 2 && q_last <= 2,
        "pointer moves should stay O(1) queries, got enter={q_enter} stay={q_stay} gap={q_gap} last={q_last}");
}

#[test]
fn link_at_matches_reference_for_clip_edges_overlaps_elided_and_paragraphs() {
    // 1. Clipped text: link straddling a clip edge (selectable so the node's hit rect is clipped,
    //    plus non-selectable link hits clipped by push_link_hits).
    struct ClipDoc { body: String, links: Vec<(usize, usize, String)> }
    let mut body = String::new();
    let mut links = Vec::new();
    for k in 0..12 {
        if k > 0 { body.push(' '); }
        let a = body.len();
        body.push_str(&format!("clippedlink{k:02}"));
        links.push((a, body.len(), format!("c{k}")));
    }
    fn clip_view(s: &ClipDoc, _: &mut Cx<ClipDoc>) -> El {
        let mut t = text(s.body.clone())
            .key("clipped")
            .font(TextStyle::new(14.0, 400.0).lh(20.0))
            .selectable(true)
            .w(260.0);
        for (a, b, id) in &s.links {
            t = t.link(Link::new(*a..*b, id.as_str()));
        }
        col().w(260.0).h(120.0).child(
            crate::el().w(45.0).h(26.0).clip_soft(0.0)
                .child(crate::el().absolute(0.0, 0.0).w(260.0).h(26.0).child(t))
        )
    }
    let mut rt = Runtime::new(ClipDoc { body, links }, clip_view, (260.0, 120.0), Env::new(true, 1.0));
    rt.frame(0.0);
    let ci = rt.tree.nodes.iter().position(|n| n.key.as_deref() == Some("clipped")).unwrap();
    let cboxes = rt.core().link_boxes(ci, rt.core().drawn_box(ci));
    let (_, straddle) = cboxes.iter().copied().find(|(_, r)| r[0] < 45.0 && r[2] > 45.0)
        .expect("a link should straddle the x=45 clip edge");
    let cy = (straddle[1] + straddle[3]) * 0.5;
    // Inside clip edge: hovered_link finds the straddling link with <= 1 query.
    rt.observe_pointer(44.0, cy);
    let (inside_hit, inside_q) = count_span_box_queries(|| rt.core().hovered_link());
    assert_eq!(inside_hit.map(|(_, k)| k), reference_link_at(rt.core(), ci, 44.0, cy));
    assert!(inside_hit.is_some() && inside_q <= 1, "inside clip edge: {inside_hit:?}, q={inside_q}");
    // Outside clip edge: clipped out of hover, while direct link_at still matches full scan.
    rt.observe_pointer(46.0, cy);
    assert_eq!(rt.core().hovered_link(), None, "outside clip edge should not hover");
    let (outside_direct, outside_q) = count_span_box_queries(|| rt.core().link_at(ci, 46.0, cy));
    assert_eq!(outside_direct, reference_link_at(rt.core(), ci, 46.0, cy));
    assert!(outside_direct.is_some() && outside_q <= 1, "direct link_at past clip: q={outside_q}");

    // 2. Overlapping and out-of-order links: lowest link index wins, matching link_boxes order.
    fn overlap_view(_: &(), _: &mut Cx<()>) -> El {
        let s = "alpha beta gamma delta";
        let t = text(s)
            .font(TextStyle::new(14.0, 400.0).lh(20.0))
            .link(Link::new(17..22, "delta"))   // k = 0 (out of byte order)
            .link(Link::new(6..16, "outer"))    // k = 1 (overlaps inner)
            .link(Link::new(6..10, "inner"))    // k = 2 (covered by k = 1)
            .link(Link::new(0..5, "alpha"));    // k = 3
        col().w(240.0).h(60.0).child(t)
    }
    let mut ort = Runtime::new((), overlap_view, (240.0, 60.0), Env::new(true, 1.0));
    ort.frame(0.0);
    let oi = ort.tree.nodes.iter().position(|n| n.text.is_some()).unwrap();
    let oboxes = ort.core().link_boxes(oi, ort.core().drawn_box(oi));
    for (_, r) in &oboxes {
        let (mx, my) = ((r[0] + r[2]) * 0.5, (r[1] + r[3]) * 0.5);
        let (got, q) = count_span_box_queries(|| ort.core().link_at(oi, mx, my));
        assert_eq!(got, reference_link_at(ort.core(), oi, mx, my));
        assert!(q <= 1, "overlap/unsorted link_at queried {q} spans");
    }
    // Specifically on "beta" (6..10), both k=1 and k=2 cover it; k=1 must win.
    let inner_r = oboxes.iter().find(|(k, _)| *k == 2).unwrap().1;
    assert_eq!(ort.core().link_at(oi, (inner_r[0] + inner_r[2]) * 0.5, (inner_r[1] + inner_r[3]) * 0.5), Some(1));

    // 3. Multi-paragraph (dir_by_paragraph) with a link spanning a newline and blank paragraph,
    //    plus an elided text node (max_lines(1)).
    fn para_elided_view(_: &(), _: &mut Cx<()>) -> El {
        let p_body = "first para

second link here
third para";
        let span_nl_a = p_body.find("para").unwrap();
        let span_nl_b = p_body.find("here").unwrap();
        let para = text(p_body)
            .key("para")
            .font(TextStyle::new(14.0, 400.0).lh(20.0))
            .dir_by_paragraph()
            .wrap()
            .w(200.0)
            .link(Link::new(0..5, "p0"))
            .link(Link::new(span_nl_a..span_nl_b, "cross_nl"))
            .link(Link::new(p_body.len() - 4..p_body.len(), "p2"));
        let e_body = "visible_one middle_cut_word_that_gets_elided_away tail_link";
        let elided = text(e_body)
            .key("elided")
            .font(TextStyle::new(14.0, 400.0).lh(20.0))
            .max_lines(1)
            .w(110.0)
            .link(Link::new(0..11, "e0"))
            .link(Link::new(12..49, "e1"))
            .link(Link::new(50..e_body.len(), "e2"));
        col().w(240.0).h(200.0).child(para).child(elided)
    }
    let mut prt = Runtime::new((), para_elided_view, (240.0, 200.0), Env::new(true, 1.0));
    prt.frame(0.0);
    for key in ["para", "elided"] {
        let ni = prt.tree.nodes.iter().position(|n| n.key.as_deref() == Some(key)).unwrap();
        let boxes = prt.core().link_boxes(ni, prt.core().drawn_box(ni));
        assert!(!boxes.is_empty(), "{key} should have link boxes");
        for (_, r) in &boxes {
            for (px, py) in [
                ((r[0] + r[2]) * 0.5, (r[1] + r[3]) * 0.5),
                (r[0], r[1]),
                (r[2] - 0.01, (r[1] + r[3]) * 0.5),
                (r[2] + 2.0, (r[1] + r[3]) * 0.5),
            ] {
                let want = reference_link_at(prt.core(), ni, px, py);
                let (got, q) = count_span_box_queries(|| prt.core().link_at(ni, px, py));
                assert_eq!(got, want, "{key}: ({px}, {py}) diverged from full scan");
                assert!(q <= 1, "{key}: ({px}, {py}) queried {q} spans");
            }
        }
    }
}


#[test]
fn a_selectable_texts_link_shows_the_hand_unless_link_cursor_names_another_shape() {
    use crate::Cursor;
    fn view(plain: &bool, _: &mut Cx<bool>) -> El {
        let t = text("read the docs before you start").key("words").selectable(true).link(Link::new(9..13, "docs"));
        col().size(300.0, 100.0).child(if *plain { t.link_cursor(Cursor::Default) } else { t })
    }
    for (plain, over_link) in [(false, Cursor::Pointer), (true, Cursor::Default)] {
        let mut rt = Runtime::new(plain, view, (300.0, 100.0), Env::new(true, 1.0));
        rt.frame(0.0);
        let (x, y) = rt.find("docs").expect("the link is laid out");
        rt.pointer_move(x, y);
        assert_eq!(rt.cursor(), over_link, "over the link, link_cursor set: {plain}");
        // Beside the link (inside "read", before it) the text is still selectable text.
        rt.pointer_move(x - 40.0, y);
        assert_eq!(rt.cursor(), Cursor::Text, "beside the link, link_cursor set: {plain}");
    }
}