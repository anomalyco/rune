use super::*;
use crate::el::GrowthSpec;

/// A growth stack whose middle row is 600 pt open or 40 pt shut, with `El::fold_glide` or else
/// `El::arrives_whole`.
fn stack(open: bool, fold: bool) -> El {
    let mut row = el().key("group").h(if open { 600.0 } else { 40.0 }).w_full();
    row = if fold { row.fold_glide(rune_motion::Spring::new(0.18, 0.0)) } else { row.arrives_whole() };
    col().key("root").child(col().key("stack").growth(GrowthSpec::default()).children([
        el().key("above").h(100.0).w_full(),
        row,
        el().key("below").h(100.0).w_full(),
    ]))
}

/// Seconds until the row shut from 600 pt is within half a point of shut, at 120 Hz.
fn shut_secs(fold: bool) -> f32 {
    fn folded(s: &(bool, bool), _: &mut Cx<(bool, bool)>) -> El {
        stack(s.0, s.1)
    }
    let mut rt = Runtime::new((true, fold), folded, (400.0, 900.0), Env::new(true, 2.0));
    rt.frame(0.0);
    rt.state.0 = false;
    rt.dirty = true;
    for k in 1..240 {
        rt.frame(1.0 / 120.0);
        if fold && k < 12 {
            assert!(rt.row_mask.iter().any(|&m| m >= 0.0), "shutting, the row clips what it held");
        }
        let below = rt.elements().into_iter().find(|e| e.key.as_deref() == Some("below")).unwrap();
        if (below.rect[1] - 140.0).abs() < 0.5 {
            return k as f32 / 120.0;
        }
    }
    panic!("still moving after 2 s");
}

#[test]
fn a_fold_glide_row_folds_on_its_own_spring() {
    let own = shut_secs(true);
    let stack = shut_secs(false);
    assert!(own < 0.3, "the row's own 0.18 s spring: {own} s");
    assert!(stack > own + 0.2, "the stack's 0.32 s spring, stretched for the distance: {stack} s");
}

/// The row's drawn top and height each frame of opening, in a 300 pt window with the stack
/// drawn at `zoom`.
fn opening(zoom: f32) -> Vec<(f32, f32)> {
    fn folded(s: &(bool, f32), _: &mut Cx<(bool, f32)>) -> El {
        col().key("zoom").scale(s.1).child(stack(s.0, true))
    }
    let mut rt = Runtime::new((false, zoom), folded, (400.0, 300.0), Env::new(true, 2.0));
    rt.frame(0.0);
    rt.frame(1.0 / 120.0);
    rt.state.0 = true;
    rt.dirty = true;
    (0..60).map(|_| {
        rt.frame(1.0 / 120.0);
        let row = rt.elements().into_iter().find(|e| e.key.as_deref() == Some("group")).unwrap();
        (row.rect[1], row.rect[3])
    }).collect()
}

#[test]
fn a_zoomed_fold_moves_only_what_shows_of_it() {
    let frames = opening(1.5);
    let (top, full) = (frames[0].0, 600.0 * 1.5);
    assert!(top < 300.0, "{frames:?}");
    assert!(frames.iter().all(|&(y, h)| y + h <= 300.5 || h == full), "glides to the window's edge, then lands: {frames:?}");
    assert!(frames[20].0 + frames[20].1 > 290.0 && frames[59].1 == full, "{frames:?}");
}

#[test]
fn a_fold_moves_only_what_shows_of_it() {
    fn folded(open: &bool, _: &mut Cx<bool>) -> El {
        stack(*open, true)
    }
    // The row starts 100 pt down a 300 pt window, so 200 pt of it shows.
    let mut rt = Runtime::new(false, folded, (400.0, 300.0), Env::new(true, 2.0));
    rt.frame(0.0);
    rt.frame(1.0 / 120.0);
    rt.state = true;
    rt.dirty = true;
    let mut heights = Vec::new();
    for _ in 0..60 {
        rt.frame(1.0 / 120.0);
        let row = rt.elements().into_iter().find(|e| e.key.as_deref() == Some("group")).unwrap();
        heights.push(row.rect[3]);
    }
    assert!(heights.iter().all(|&h| h <= 200.5 || h == 600.0), "glides to the window's edge, then lands: {heights:?}");
    assert!(heights[0] < 100.0 && heights[20] > 190.0 && heights[59] == 600.0, "{heights:?}");
}

/// Rows `a` (a fold, 600 pt open or 40 pt shut) and `b` (100 pt), in either order, between a
/// 100 pt row above and one below.
fn reordering(s: &(bool, bool), _: &mut Cx<(bool, bool)>) -> El {
    let a = el().key("a").w_full().h(if s.0 { 600.0 } else { 40.0 }).fold_glide(rune_motion::Spring::new(0.18, 0.0));
    let b = el().key("b").w_full().h(100.0);
    let (first, second) = if s.1 { (b, a) } else { (a, b) };
    col().key("root").child(col().key("stack").w_full().growth(GrowthSpec::default()).children([
        el().key("above").h(100.0).w_full(),
        first,
        second,
        el().key("below").h(100.0).w_full(),
    ]))
}

/// Each key's (top, height) every frame for one second after `a` shuts and swaps with `b` in
/// one commit, in a window `window_h` tall.
fn shut_and_swap(window_h: f32) -> (Runtime<(bool, bool)>, Vec<HashMap<&'static str, (f32, f32)>>) {
    let mut rt = Runtime::new((true, false), reordering, (400.0, window_h), Env::new(true, 2.0));
    rt.frame(0.0);
    for _ in 0..4 { rt.frame(1.0 / 120.0); }
    rt.update(|s, _| *s = (false, true));
    let frames = (0..120).map(|_| {
        rt.frame(1.0 / 120.0);
        let els = rt.elements();
        ["above", "a", "b", "below"].into_iter().map(|k| {
            let e = els.iter().find(|e| e.key.as_deref() == Some(k) && !e.exiting).unwrap();
            (k, (e.rect[1], e.rect[3]))
        }).collect()
    }).collect();
    (rt, frames)
}

#[test]
fn a_fold_that_reorders_in_the_same_commit_slides_and_folds_at_once() {
    // All on screen: the fold shuts on its spring while it and `b` slide into their new places,
    // without a jump, and everything lands where the new order lays it out.
    let (rt, frames) = shut_and_swap(900.0);
    for w in frames.windows(2) {
        for k in ["above", "a", "b", "below"] {
            let ((y0, h0), (y1, h1)) = (w[0][k], w[1][k]);
            assert!((y1 - y0).abs() < 60.0 && (h1 - h0).abs() < 60.0, "{k} jumps from {y0}+{h0} to {y1}+{h1}: {frames:?}");
        }
        assert!(w[1]["a"].1 <= w[0]["a"].1 + 1e-3, "the fold only shuts: {frames:?}");
    }
    let last = frames.last().unwrap();
    assert_eq!((last["above"], last["b"], last["a"], last["below"]), ((0.0, 100.0), (100.0, 100.0), (200.0, 40.0), (240.0, 100.0)));
    assert!(frames[36]["a"].1 < 41.0, "the fold shuts on its own 0.18 s spring: {:?}", frames[36]["a"]);
    assert!(!rt.animating);
    // The fold past the window's bottom: what shows of it still shuts and slides without a jump,
    // and `b`, drawn below the window, comes in from its edge rather than from far below it.
    let (_, frames) = shut_and_swap(300.0);
    for w in frames.windows(2) {
        let ((y0, h0), (y1, h1)) = (w[0]["a"], w[1]["a"]);
        assert!((y1 - y0).abs() < 60.0 && ((y1 + h1).min(300.0) - (y0 + h0).min(300.0)).abs() < 60.0, "a jumps from {y0}+{h0} to {y1}+{h1}: {frames:?}");
        assert!((w[1]["b"].0 - w[0]["b"].0).abs() < 60.0, "b jumps: {frames:?}");
    }
    assert!(frames[0]["b"].0 <= 300.0, "{:?}", frames[0]["b"]);
    let last = frames.last().unwrap();
    assert_eq!((last["b"], last["a"]), ((100.0, 100.0), (200.0, 40.0)));
}

/// A section in a fold row: a 28 pt header, sticky to the top, over 1,120 pt of steps when open,
/// 200 pt down a 200 pt scroll whose rows below are 100 pt each.
fn section(open: &bool, _: &mut Cx<bool>) -> El {
    let mut group = col().key("group").w_full().child(el().key("hdr").w_full().h(28.0).sticky_top(0.0));
    if *open {
        group = group.child(col().key("steps").w_full().children((0..40).map(|k| el().key(format!("step-{k}")).w_full().h(28.0))));
    }
    let stack = col().key("stack").w_full().growth(GrowthSpec::default()).children([
        el().key("before").w_full().h(200.0),
        col().key("turn").w_full().fold_glide(rune_motion::Spring::new(0.18, 0.0)).child(group),
        el().key("after").w_full().h(100.0),
        el().key("tail").w_full().h(100.0),
    ]);
    col().key("scroll").size(300.0, 200.0).scroll(ScrollSpec { stick_bottom: true }).child(stack)
}

#[test]
fn keep_on_screen_holds_a_stuck_header_while_its_fold_shuts() {
    let mut rt = Runtime::new(true, section, (300.0, 200.0), Env::new(true, 1.0));
    rt.frame(0.0);
    rt.update(|_, tx| tx.scroll_to_instant("scroll", 0.0, 700.0));
    for _ in 0..4 { rt.frame(1.0 / 120.0); }
    let find = |rt: &Runtime<bool>, k: &str| rt.elements().into_iter().find(|e| e.key.as_deref() == Some(k) && !e.exiting).unwrap();
    assert!(find(&rt, "hdr").rect[1] == 0.0 && find(&rt, "hdr").stuck > 400.0, "{:?}", find(&rt, "hdr"));
    rt.update(|open, tx| { *open = false; tx.keep_on_screen("hdr"); });
    let mut after = Vec::new();
    for f in 0..120 {
        rt.frame(1.0 / 120.0);
        let hdr = find(&rt, "hdr").rect[1];
        assert!(hdr.abs() < 0.5, "frame {f}: the header moved to {hdr}");
        let a = find(&rt, "after").rect[1];
        // The row below never shows under the header, nor drops once it shows.
        assert!(a >= 28.0 - 0.5, "frame {f}: the row below rose to {a}, over the header");
        after.push(a);
    }
    let shown: Vec<f32> = after.iter().copied().skip_while(|&a| a >= 200.0).collect();
    assert!(shown.windows(2).all(|w| w[1] <= w[0] + 0.5), "the row below only rises once it shows: {after:?}");
    assert!(after[36] < 28.5, "it reaches the header on the fold's own spring: {after:?}");
    assert!(*after.last().unwrap() == 28.0 && rt.keeps.is_empty() && !rt.animating, "{after:?}");
}
