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
