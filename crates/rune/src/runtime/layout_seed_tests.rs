//! `App::layout_seed`: a first commit whose view is given the rects an earlier runtime's view
//! read runs the view once when the layout agrees with them, and ends at the layout no seed
//! ends at whatever the seed says.
use std::cell::Cell;

use super::*;
use crate::{Cx, El, Env, LayoutSeed, col, el, row, text};

thread_local! {
    /// Runs of `view` on this test's thread.
    static RUNS: Cell<u32> = const { Cell::new(0) };
}

struct S {
    /// The side pane's width.
    side: f32,
    zoom: f32,
}

fn view(s: &S, cx: &mut Cx<S>) -> El {
    RUNS.with(|r| r.set(r.get() + 1));
    cx.zoom(s.zoom);
    let wide = cx.layout_with("main", |r| r[2] >= 300.0).unwrap_or(false);
    let side = cx.layout_size("side").map_or(0.0, |[w, _]| w);
    row().w_full().h_full()
        .child(el().key("side").w(s.side).h_full())
        .child(col().key("main").grow(1.0).h_full().child(text(if wide { format!("wide beside {side:.0}") } else { "narrow".to_owned() }).key("label")))
}

/// A runtime at `size`, given `seed`, after its first frame; and how often that frame ran the view.
fn first_frame(side: f32, zoom: f32, size: (f32, f32), seed: Option<LayoutSeed>) -> (Runtime<S>, u32) {
    let mut rt = Runtime::new(S { side, zoom }, view, size, Env::new(true, 1.0));
    if let Some(seed) = seed { rt.core_mut().offer_layout_seed(seed); }
    let before = RUNS.with(Cell::get);
    rt.frame(0.0);
    (rt, RUNS.with(Cell::get) - before)
}

fn label(rt: &Runtime<S>) -> [f32; 4] {
    rt.presented("label").expect("the label is laid out").0
}

#[test]
fn a_first_frame_given_its_own_reads_runs_the_view_once() {
    let (plain, runs) = first_frame(200.0, 1.0, (800.0, 600.0), None);
    assert!(runs >= 2, "with no seed the view runs again once layout knows what it read: {runs}");
    let seed = plain.core().layout_seed().expect("the view reads layout");
    assert_eq!(seed.viewport, [800.0, 600.0]);
    assert_eq!(seed.rects.iter().map(|(k, _)| k.as_str()).collect::<Vec<_>>(), ["main", "side"], "ordered by key");

    let (seeded, runs) = first_frame(200.0, 1.0, (800.0, 600.0), Some(seed.clone()));
    assert_eq!(runs, 1, "the seed's rects are the layout's, so the view runs once");
    assert_eq!(seeded.core().layout_seed(), Some(seed), "and reads the same rects");
    assert_eq!(label(&seeded), label(&plain), "at the same layout");
}

#[test]
fn a_seed_from_another_state_ends_at_the_layout_no_seed_ends_at() {
    let (old, _) = first_frame(200.0, 1.0, (800.0, 600.0), None);
    let stale = old.core().layout_seed().unwrap();
    let (plain, unseeded) = first_frame(120.0, 1.0, (800.0, 600.0), None);
    let (seeded, runs) = first_frame(120.0, 1.0, (800.0, 600.0), Some(stale));
    assert!(runs >= 2, "a rect the layout disagrees with runs the view again: {runs}");
    assert!(runs <= unseeded, "and costs no more than no seed: {runs} against {unseeded}");
    assert_eq!(seeded.core().layout_seed(), plain.core().layout_seed());
    assert_eq!(label(&seeded), label(&plain));
}

#[test]
fn a_seed_for_another_viewport_is_not_given_to_the_view() {
    let (old, _) = first_frame(200.0, 1.0, (800.0, 600.0), None);
    let seed = old.core().layout_seed().unwrap();
    let (plain, unseeded) = first_frame(200.0, 1.0, (700.0, 600.0), None);
    let (seeded, runs) = first_frame(200.0, 1.0, (700.0, 600.0), Some(seed));
    assert_eq!(runs, unseeded, "as with no seed");
    assert_eq!(seeded.core().layout_seed(), plain.core().layout_seed());
    assert_eq!(label(&seeded), label(&plain));
}

#[test]
fn a_seeded_key_the_view_does_not_read_goes() {
    let (old, _) = first_frame(200.0, 1.0, (800.0, 600.0), None);
    let seed = old.core().layout_seed().unwrap();
    let mut more = seed.clone();
    more.rects.push(("gone".to_owned(), Some([1.0, 2.0, 3.0, 4.0])));
    let (seeded, runs) = first_frame(200.0, 1.0, (800.0, 600.0), Some(more));
    assert_eq!(runs, 1, "a key nothing reads moves nothing");
    assert_eq!(seeded.core().layout_seed(), Some(seed));
}

#[test]
fn a_zoomed_view_given_its_seed_runs_once_at_its_zoom() {
    let (plain, unseeded) = first_frame(200.0, 1.25, (800.0, 600.0), None);
    assert!(unseeded >= 2, "{unseeded}");
    let seed = plain.core().layout_seed().unwrap();
    assert_eq!(seed.zoom, 1.25);
    assert_eq!(seed.viewport, [640.0, 480.0], "the view's points: the window's over the zoom");
    let (seeded, runs) = first_frame(200.0, 1.25, (800.0, 600.0), Some(seed.clone()));
    assert_eq!(runs, 1, "laid out at the seed's zoom from the first run");
    assert_eq!(seeded.zoom(), 1.25);
    assert_eq!(seeded.core().layout_seed(), Some(seed));
    assert_eq!(label(&seeded), label(&plain));
}

#[test]
fn the_first_frames_reads_stay_what_the_first_frame_read() {
    let (mut rt, _) = first_frame(200.0, 1.0, (800.0, 600.0), None);
    let first = rt.core().first_layout_seed().cloned().expect("the first view reads layout");
    assert_eq!(Some(&first), rt.core().layout_seed().as_ref(), "after the first frame they are the current reads");
    // The side pane narrows, as content arriving after a launch's first frame moves things.
    rt.update(|s, _| s.side = 120.0);
    rt.frame(1.0 / 60.0);
    assert_ne!(rt.core().layout_seed(), Some(first.clone()), "the current reads moved");
    assert_eq!(rt.core().first_layout_seed(), Some(&first), "the first frame's did not");
    // And they are what a new launch in the first frame's state settles at in one run.
    let (_, runs) = first_frame(200.0, 1.0, (800.0, 600.0), Some(first));
    assert_eq!(runs, 1);
}

#[test]
fn a_seed_offered_after_the_first_commit_does_nothing() {
    let (mut rt, _) = first_frame(200.0, 1.0, (800.0, 600.0), None);
    let seed = rt.core().layout_seed().unwrap();
    let mut wrong = seed.clone();
    wrong.rects[0].1 = Some([0.0, 0.0, 10.0, 10.0]);
    rt.core_mut().offer_layout_seed(wrong);
    rt.update(|s, _| s.side = 201.0);
    rt.frame(1.0 / 60.0);
    let (plain, _) = first_frame(201.0, 1.0, (800.0, 600.0), None);
    assert_eq!(rt.core().layout_seed(), plain.core().layout_seed());
}
