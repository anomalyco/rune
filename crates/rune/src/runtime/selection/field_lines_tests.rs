use crate::{Cx, El, Env, Runtime, TextEdit, TextStyle, col, text_input};

fn field(e: &TextEdit, _: &mut Cx<TextEdit>) -> El {
    col().size(120.0, 200.0).child(
        text_input(e, TextStyle::new(14.0, 400.0)).key("f").w_full().on_edit(|e: &mut TextEdit| e),
    )
}

fn nowrap(e: &TextEdit, _: &mut Cx<TextEdit>) -> El {
    col().size(120.0, 200.0).scroll_xy().child(
        text_input(e, TextStyle::new(14.0, 400.0)).key("f").nowrap().on_edit(|e: &mut TextEdit| e),
    )
}

fn run(text: &str, view: fn(&TextEdit, &mut Cx<TextEdit>) -> El) -> Runtime<TextEdit> {
    let mut rt = Runtime::new(TextEdit::new(text), view, (120.0, 200.0), Env::new(true, 2.0));
    rt.frame(0.0);
    rt
}

/// Walks every visual line from the first, as a gutter would.
fn lines(rt: &Runtime<TextEdit>) -> Vec<crate::TextLine> {
    let mut out = vec![rt.text_line("f", 0).unwrap()];
    while let Some(next) = out.last().unwrap().next {
        out.push(rt.text_line("f", next).unwrap());
        assert!(out.len() < 1000);
    }
    out
}

#[test]
fn hard_lines_give_their_starts_and_neighbours_stacked_down_the_field() {
    let rt = run("one\ntwo\n\nfour", field);
    let ls = lines(&rt);
    assert_eq!(ls.iter().map(|l| l.start).collect::<Vec<_>>(), [0, 4, 8, 9]);
    assert_eq!(ls.iter().map(|l| l.prev).collect::<Vec<_>>(), [None, Some(0), Some(4), Some(8)]);
    for w in ls.windows(2) {
        assert!(w[1].rect[1] >= w[0].rect[1] + w[0].rect[3] - 0.5, "{w:?}");
    }
    assert_eq!(rt.text_line("f", 6), Some(ls[1]));
    let field = rt.elements().into_iter().find(|e| e.key.as_deref() == Some("f")).unwrap();
    assert!(ls[0].rect[1] >= field.rect[1] && ls[0].rect[0] >= field.rect[0]);
    assert!(ls.iter().all(|l| l.rect[3] > 0.0));
}

#[test]
fn soft_wrapped_lines_count_as_visual_lines_and_nowrap_keeps_one_per_line() {
    let text = "alpha beta gamma delta epsilon zeta eta theta\nshort";
    let wrapped = lines(&run(text, field));
    assert!(wrapped.len() > 2, "{wrapped:?}");
    assert!(wrapped[1].start > 0 && wrapped[1].start < 46);
    let flat = lines(&run(text, nowrap));
    assert_eq!(flat.iter().map(|l| l.start).collect::<Vec<_>>(), [0, 46]);
    assert!(flat[0].rect[2] > 120.0);
}

#[test]
fn byte_at_point_round_trips_through_the_line_box() {
    let rt = run("one\ntwo\nthree", field);
    let l = rt.text_line("f", 9).unwrap();
    let at = rt.text_offset_at("f", l.rect[0] + 1.0, l.rect[1] + l.rect[3] * 0.5).unwrap();
    assert_eq!(rt.text_line("f", at).unwrap().start, l.start);
}

#[test]
fn missing_and_secure_fields_have_no_lines() {
    let rt = run("one", field);
    assert_eq!(rt.text_line("nope", 0), None);
    let mut e = TextEdit::new("secret");
    e.set_secure(true);
    let mut rt = Runtime::new(e, field, (120.0, 200.0), Env::new(true, 1.0));
    rt.frame(0.0);
    assert_eq!(rt.text_line("f", 0), None);
}

#[test]
fn a_composition_keeps_lines_in_the_committed_texts_bytes() {
    let mut e = TextEdit::new("one\ntwo");
    e.focused = true;
    e.anchor = 2;
    e.focus = 2;
    let mut rt = Runtime::new(e, field, (120.0, 200.0), Env::new(true, 1.0));
    rt.frame(0.0);
    rt.ime(crate::Ime::Preedit("xyz".into(), None, Vec::new()));
    rt.frame(0.0);
    assert!(rt.state.preedit().is_some());
    let first = rt.text_line("f", 0).unwrap();
    assert_eq!((first.start, first.next), (0, Some(4)));
    assert_eq!(rt.text_line("f", 5).unwrap().prev, Some(0));
}

/// Wraps onto a full line, a short soft-wrapped line (`"bb "`), and more lines in the bundled
/// default face at 14 pt inside the 120 pt field, with no system font needed.
const SHORT_MIDDLE: &str = "aaaaaaaaaaaaaaa bb ccccccccccccccc dd";

#[test]
fn a_byte_mid_line_sees_the_short_soft_wrapped_line_after_it() {
    let text = SHORT_MIDDLE;
    let rt = run(text, field);
    let starts: Vec<usize> = lines(&rt).iter().map(|l| l.start).collect();
    assert!(starts.len() >= 3, "{starts:?}");
    let short = starts[1];
    assert_eq!(&text[short..starts[2]], "bb ", "middle line should be the short one: {starts:?}");
    for b in 0..short {
        assert_eq!(rt.text_line("f", b).unwrap().next, Some(short), "byte {b}, lines {starts:?}");
    }
}

#[test]
fn every_byte_of_a_line_agrees_on_its_start_and_neighbours() {
    for (text, view) in [
        (SHORT_MIDDLE, field as fn(&TextEdit, &mut Cx<TextEdit>) -> El),
        ("alpha beta gamma delta epsilon zeta eta theta\nshort\n\nx y", field),
        ("alpha beta gamma delta epsilon zeta eta theta\nshort\n\nx y", nowrap),
    ] {
        let rt = run(text, view);
        let ls = lines(&rt);
        for (k, l) in ls.iter().enumerate() {
            let end = ls.get(k + 1).map_or(text.len() + 1, |n| n.start);
            for b in l.start..end {
                let got = rt.text_line("f", b).unwrap();
                assert_eq!((got.start, got.prev, got.next), (l.start, l.prev, l.next), "{text:?} byte {b}");
            }
        }
        let starts: Vec<usize> = ls.iter().map(|l| l.start).collect();
        let prevs: Vec<Option<usize>> = ls.iter().map(|l| l.prev).collect();
        let mut want = vec![None];
        want.extend(starts[..starts.len() - 1].iter().copied().map(Some));
        assert_eq!(prevs, want, "{text:?}");
    }
}

/// Walks a field forward from byte 0 and back from its last line, checking the starts move
/// strictly each way, the first line has no `prev` and both walks see the same lines.
fn walk_both_ways(rt: &Runtime<TextEdit>) -> Vec<usize> {
    let first = rt.text_line("f", 0).unwrap();
    assert_eq!((first.start, first.prev), (0, None));
    let mut down = vec![first.start];
    let mut l = first;
    while let Some(next) = l.next {
        assert!(next > l.start, "{l:?}");
        l = rt.text_line("f", next).unwrap();
        assert_eq!(l.start, next);
        down.push(l.start);
        assert!(down.len() < 1000);
    }
    let mut up = vec![l.start];
    while let Some(prev) = l.prev {
        assert!(prev < l.start, "{l:?}");
        l = rt.text_line("f", prev).unwrap();
        assert_eq!(l.start, prev);
        up.push(l.start);
        assert!(up.len() < 1000);
    }
    up.reverse();
    assert_eq!(down, up);
    down
}

fn composing(text: &str, at: usize) -> Runtime<TextEdit> {
    let mut e = TextEdit::new(text);
    e.focused = true;
    e.anchor = at;
    e.focus = at;
    let mut rt = Runtime::new(e, field, (120.0, 200.0), Env::new(true, 2.0));
    rt.frame(0.0);
    let long = "pre edit text that wraps over several lines in this field";
    rt.ime(crate::Ime::Preedit(long.into(), None, Vec::new()));
    rt.frame(0.0);
    assert!(rt.state.preedit().is_some());
    rt
}

#[test]
fn a_wrapping_composition_reads_as_one_line_and_walks_end() {
    let rt = composing("", 0);
    let first = rt.text_line("f", 0).unwrap();
    assert_eq!((first.start, first.prev, first.next), (0, None, None));
    assert!(first.rect[3] > 30.0, "the composition's lines make one tall box: {first:?}");
    assert_eq!(walk_both_ways(&rt), [0]);

    let text = "one\ntwo three\nfour";
    let rt = composing(text, 8);
    let starts = walk_both_ways(&rt);
    assert_eq!(starts, [0, 4, 14]);
    let mid = rt.text_line("f", 8).unwrap();
    assert_eq!((mid.start, mid.prev, mid.next), (4, Some(0), Some(14)));
    for b in 4..14 {
        assert_eq!(rt.text_line("f", b).unwrap().start, 4, "byte {b}");
    }

    let rt = composing("one\ntwo", 4);
    assert_eq!(walk_both_ways(&rt), [0, 4]);
    assert_eq!(rt.text_line("f", 4).unwrap().start, 4);
}

#[test]
fn tight_line_height_stacks_lines_without_overlap_or_negative_top() {
    fn tight(e: &TextEdit, _: &mut Cx<TextEdit>) -> El {
        col().size(120.0, 200.0).child(
            text_input(e, TextStyle::new(14.0, 400.0).lh(14.0)).key("f").w_full().on_edit(|e: &mut TextEdit| e),
        )
    }
    let rt = run("one\ntwo\n\nfour", tight);
    let ls = lines(&rt);
    assert_eq!(ls.iter().map(|l| l.start).collect::<Vec<_>>(), [0, 4, 8, 9]);
    let field_el = rt.elements().into_iter().find(|e| e.key.as_deref() == Some("f")).unwrap();
    assert!(
        ls[0].rect[1] >= field_el.rect[1],
        "first line must not protrude above field top: line={:?}, field={:?}",
        ls[0].rect,
        field_el.rect,
    );
    for w in ls.windows(2) {
        assert!(
            w[1].rect[1] >= w[0].rect[1] + w[0].rect[3] - 0.5,
            "adjacent lines must not overlap under negative half-leading: {w:?}",
        );
    }
    for l in &ls {
        assert!((l.rect[3] - 14.0).abs() <= 0.5, "line height should match 14 pt line box: {l:?}");
    }
    // Empty field keeps its line_px fallback height.
    let empty = run("", tight);
    let first = empty.text_line("f", 0).unwrap();
    assert!((first.rect[3] - 14.0).abs() <= 0.5, "empty line height: {first:?}");
}
