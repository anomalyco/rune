use std::cell::RefCell;

use super::*;
use crate::{Color, Cx, El, Env, Runtime, TextHighlight, TextStyle, col, text};

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
