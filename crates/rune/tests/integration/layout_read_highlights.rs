//! Layout reads of text highlights and keyed elements stay right while a list scrolls (frames
//! that reuse what a lookup found before) and after a commit moves what a key names.
use std::cell::RefCell;

use rune::{Color, Cx, El, Env, Runtime, ScrollSpec, TextHighlight, TextStyle, col, text};

struct S {
    hit_row: usize,
    /// Element keys read besides the highlight, so lookups go through an index.
    extra_reads: usize,
    seen: RefCell<Option<[f32; 4]>>,
    seen_rows: RefCell<Vec<Option<[f32; 4]>>>,
}

fn view(s: &S, cx: &mut Cx<S>) -> El {
    *s.seen.borrow_mut() = cx.window_layout(&format!("row{}/mark", s.hit_row));
    *s.seen_rows.borrow_mut() = (0..s.extra_reads).map(|i| cx.window_layout(&format!("row{i}"))).collect();
    let rows = (0..30).fold(col().w_full(), |c, i| {
        // Every row's text has a highlight keyed `mark`: the scoped read picks one row's.
        let body = text(format!("row {i} text with a mark in it"))
            .font(TextStyle::new(14.0, 400.0).lh(20.0))
            .text_highlight(TextHighlight::new(18..22, Color::WHITE).key("mark"));
        c.child(col().key(format!("row{i}")).w_full().h(40.0).py(10.0).child(body))
    });
    col().w_full().h_full().child(col().key("list").w_full().h(200.0).scroll(ScrollSpec { stick_bottom: false }).child(rows))
}

fn settle(rt: &mut Runtime<S>) {
    for _ in 0..600 { if !rt.frame(1.0 / 60.0) { return; } }
    panic!("never settled");
}

/// The highlight `mark` in row `row` as `[x, y, w, h]`, and row `i`s rect, from the tree.
fn mark_of(rt: &Runtime<S>, row: usize) -> [f32; 4] {
    let scope = format!("/row{row}/");
    let e = rt.elements().into_iter()
        .find(|e| e.key.as_deref() == Some("mark") && e.path.contains(&scope))
        .unwrap_or_else(|| panic!("mark in row {row}"));
    e.rect
}

fn row_of(rt: &Runtime<S>, i: usize) -> [f32; 4] {
    let key = format!("row{i}");
    rt.elements().into_iter().find(|e| e.key.as_deref() == Some(key.as_str())).expect("row").rect
}

fn close(a: [f32; 4], b: [f32; 4]) -> bool {
    a.iter().zip(b).all(|(x, y)| (x - y).abs() < 0.75)
}

fn check(rt: &Runtime<S>, when: &str) {
    let seen = rt.state.seen.borrow().unwrap_or_else(|| panic!("{when}: read saw nothing"));
    let mark = mark_of(rt, rt.state.hit_row);
    assert!(close(seen, mark), "{when}: read {seen:?}, mark at {mark:?}");
    for (i, seen) in rt.state.seen_rows.borrow().iter().enumerate() {
        let seen = seen.unwrap_or_else(|| panic!("{when}: row{i} read saw nothing"));
        let row = row_of(rt, i);
        assert!(close(seen, row), "{when}: row{i} read {seen:?}, row at {row:?}");
    }
}

fn reads_follow_scrolls_and_commits(extra_reads: usize) {
    let s = S { hit_row: 6, extra_reads, seen: RefCell::new(None), seen_rows: RefCell::new(Vec::new()) };
    let mut rt = Runtime::new(s, view, (300.0, 300.0), Env::new(true, 1.0));
    rt.frame(0.0);
    settle(&mut rt);
    check(&rt, "first layout");

    for dy in [-120.0, -160.0, 200.0] {
        let before = mark_of(&rt, rt.state.hit_row)[1];
        rt.wheel(100.0, 100.0, dy);
        settle(&mut rt);
        assert_ne!(mark_of(&rt, rt.state.hit_row)[1], before, "the wheel scrolled");
        check(&rt, &format!("wheeled {dy}"));
    }

    // A commit names another row's highlight; frames after it look it up in the new tree.
    rt.update(|s, _| s.hit_row = 2);
    settle(&mut rt);
    check(&rt, "after the commit");
    rt.wheel(100.0, 100.0, 400.0);
    settle(&mut rt);
    check(&rt, "wheeled back after the commit");
}

#[test]
fn a_scoped_highlight_read_follows_scrolls_and_commits() {
    reads_follow_scrolls_and_commits(0);
}

#[test]
fn many_reads_follow_scrolls_and_commits() {
    reads_follow_scrolls_and_commits(12);
}

#[test]
fn a_read_answers_its_own_highlights_among_many_in_one_text() {
    struct Multi {
        body: String,
        spans: Vec<(usize, usize, String)>,
        reads: Vec<String>,
        seen: RefCell<Vec<Option<[f32; 4]>>>,
    }
    fn multi_view(s: &Multi, cx: &mut Cx<Multi>) -> El {
        *s.seen.borrow_mut() = s.reads.iter().map(|k| cx.window_layout(k)).collect();
        let mut t = text(s.body.clone()).key("doc").font(TextStyle::new(14.0, 400.0).lh(20.0)).wrap().w_full();
        for (a, b, k) in &s.spans {
            t = t.text_highlight(TextHighlight::new(*a..*b, Color::WHITE).key(k.as_str()));
        }
        col().w(180.0).h(400.0).child(t)
    }

    let mut body = String::new();
    let mut spans = Vec::new();
    for i in 0..32 {
        if i > 0 { body.push(' '); }
        let a = body.len();
        body.push_str(&format!("term{i:02}"));
        let b = body.len();
        let key = if i == 1 || i == 29 { "pair".to_owned() } else { format!("k{i}") };
        spans.push((a, b, key));
    }
    let reads = vec!["k0".to_owned(), "k15".to_owned(), "doc/k28".to_owned(), "pair".to_owned()];
    let mut rt = Runtime::new(
        Multi { body, spans, reads: reads.clone(), seen: RefCell::new(Vec::new()) },
        multi_view,
        (180.0, 400.0),
        Env::new(true, 1.0),
    );
    rt.frame(0.0);
    for _ in 0..60 { if !rt.frame(1.0 / 60.0) { break; } }

    let els = rt.elements();
    let seen = rt.state.seen.borrow().clone();
    for (key, got) in reads.iter().zip(seen) {
        let got = got.unwrap_or_else(|| panic!("{key}: read saw nothing"));
        let leaf = key.rsplit('/').next().unwrap();
        let u = els.iter()
            .filter(|e| e.role == "mark" && e.key.as_deref() == Some(leaf))
            .map(|e| [e.rect[0], e.rect[1], e.rect[0] + e.rect[2], e.rect[1] + e.rect[3]])
            .fold(None, |u: Option<[f32; 4]>, r| {
                Some(u.map_or(r, |u| [u[0].min(r[0]), u[1].min(r[1]), u[2].max(r[2]), u[3].max(r[3])]))
            })
            .unwrap_or_else(|| panic!("{key}: no mark in tree"));
        let want = [u[0], u[1], u[2] - u[0], u[3] - u[1]];
        assert!(close(got, want), "{key}: read {got:?}, mark union {want:?}");
        if key == "pair" {
            assert!(got[3] > 25.0, "pair should span multiple lines: {got:?}");
        }
    }
}
