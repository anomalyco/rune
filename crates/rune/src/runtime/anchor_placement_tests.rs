//! `Cx::anchor_placement` and `memo::anchor_placement`: resolved `(Side, Align)` after
//! flipping and RTL mirroring, chained submenus, scroll stability, memo invalidation, and
//! resize settling.
use std::cell::Cell;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use super::*;
use crate::el::{Align, Direction, ScrollSpec, Side};
use crate::{Cx, El, Env, col, el, memo, text};

#[test]
fn unflipped_placement_reports_authored_side_and_align() {
    #[derive(Default)]
    struct S {
        seen: Cell<Option<(Side, Align)>>,
    }
    fn view(s: &S, cx: &mut Cx<S>) -> El {
        s.seen.set(cx.anchor_placement("pop"));
        col().size(400.0, 300.0)
            .child(el().key("trigger").absolute(40.0, 40.0).size(80.0, 24.0))
            .child(col().key("pop").size(100.0, 60.0).anchored("trigger", Side::Bottom, Align::Start, 4.0))
    }

    let mut rt = Runtime::new(S::default(), view, (400.0, 300.0), Env::new(true, 1.0));
    rt.frame(0.0);
    assert_eq!(rt.state.seen.get(), Some((Side::Bottom, Align::Start)));
}

#[test]
fn side_flips_report_resolved_side() {
    struct Case {
        pos: (f32, f32),
        side: Side,
        expected: Side,
    }
    let cases = [
        Case { pos: (40.0, 250.0), side: Side::Bottom, expected: Side::Top },
        Case { pos: (40.0, 10.0), side: Side::Top, expected: Side::Bottom },
        Case { pos: (310.0, 80.0), side: Side::Right, expected: Side::Left },
        Case { pos: (10.0, 80.0), side: Side::Left, expected: Side::Right },
    ];
    for c in cases {
        struct S {
            pos: (f32, f32),
            side: Side,
            seen: Cell<Option<(Side, Align)>>,
        }
        fn view(s: &S, cx: &mut Cx<S>) -> El {
            s.seen.set(cx.anchor_placement("pop"));
            col().size(400.0, 300.0)
                .child(el().key("trigger").absolute(s.pos.0, s.pos.1).size(60.0, 24.0))
                .child(col().key("pop").size(100.0, 80.0).anchored("trigger", s.side, Align::Start, 4.0))
        }
        let mut rt = Runtime::new(S { pos: c.pos, side: c.side, seen: Cell::new(None) }, view, (400.0, 300.0), Env::new(true, 1.0));
        rt.frame(0.0);
        assert_eq!(rt.state.seen.get(), Some((c.expected, Align::Start)), "authored {:?} at {:?}", c.side, c.pos);
    }
}

#[test]
fn align_flips_report_resolved_align() {
    struct Case {
        pos: (f32, f32),
        side: Side,
        align: Align,
        expected: (Side, Align),
    }
    let cases = [
        // Bottom/Start near right edge flips align to End.
        Case { pos: (340.0, 40.0), side: Side::Bottom, align: Align::Start, expected: (Side::Bottom, Align::End) },
        // Bottom/End near left edge flips align to Start.
        Case { pos: (10.0, 40.0), side: Side::Bottom, align: Align::End, expected: (Side::Bottom, Align::Start) },
        // Right/Start near bottom edge flips align to End.
        Case { pos: (80.0, 260.0), side: Side::Right, align: Align::Start, expected: (Side::Right, Align::End) },
        // Right/End near top edge flips align to Start.
        Case { pos: (80.0, 10.0), side: Side::Right, align: Align::End, expected: (Side::Right, Align::Start) },
    ];
    for c in cases {
        struct S {
            pos: (f32, f32),
            side: Side,
            align: Align,
            seen: Cell<Option<(Side, Align)>>,
        }
        fn view(s: &S, cx: &mut Cx<S>) -> El {
            s.seen.set(cx.anchor_placement("pop"));
            col().size(400.0, 300.0)
                .child(el().key("trigger").absolute(s.pos.0, s.pos.1).size(40.0, 24.0))
                .child(col().key("pop").size(120.0, 80.0).anchored("trigger", s.side, s.align, 4.0))
        }
        let mut rt = Runtime::new(S { pos: c.pos, side: c.side, align: c.align, seen: Cell::new(None) }, view, (400.0, 300.0), Env::new(true, 1.0));
        rt.frame(0.0);
        assert_eq!(rt.state.seen.get(), Some(c.expected), "authored ({:?}, {:?}) at {:?}", c.side, c.align, c.pos);
    }
}

#[test]
fn rtl_mirrored_anchor_reports_physical_resolved_placement() {
    struct S {
        /// Trigger left inset in RTL (mirrored from right edge of 400pt root).
        inset: f32,
        side: Side,
        align: Align,
        seen: Cell<Option<(Side, Align)>>,
    }
    fn view(s: &S, cx: &mut Cx<S>) -> El {
        s.seen.set(cx.anchor_placement("pop"));
        col().dir(Direction::Rtl).size(400.0, 300.0)
            .child(el().key("trigger").absolute(s.inset, 80.0).size(40.0, 24.0))
            .child(col().key("pop").size(120.0, 60.0).anchored("trigger", s.side, s.align, 4.0))
    }

    // RTL inset 40 -> physical x = 400 - 40 - 40 = 320. Authored Right mirrors to physical Left (fits at x = 196).
    let mut rt = Runtime::new(S { inset: 40.0, side: Side::Right, align: Align::Start, seen: Cell::new(None) }, view, (400.0, 300.0), Env::new(true, 1.0));
    rt.frame(0.0);
    assert_eq!(rt.state.seen.get(), Some((Side::Left, Align::Start)));

    // RTL inset 340 -> physical x = 400 - 340 - 40 = 20. Authored Right mirrors to physical Left, overflows left edge, flips to physical Right!
    let mut rt = Runtime::new(S { inset: 340.0, side: Side::Right, align: Align::Start, seen: Cell::new(None) }, view, (400.0, 300.0), Env::new(true, 1.0));
    rt.frame(0.0);
    assert_eq!(rt.state.seen.get(), Some((Side::Right, Align::Start)));

    // Bottom/Start in RTL mirrors align to physical End; near left edge (x = 20) flips back to physical Start.
    let mut rt = Runtime::new(S { inset: 40.0, side: Side::Bottom, align: Align::Start, seen: Cell::new(None) }, view, (400.0, 300.0), Env::new(true, 1.0));
    rt.frame(0.0);
    assert_eq!(rt.state.seen.get(), Some((Side::Bottom, Align::End)));

    let mut rt = Runtime::new(S { inset: 340.0, side: Side::Bottom, align: Align::Start, seen: Cell::new(None) }, view, (400.0, 300.0), Env::new(true, 1.0));
    rt.frame(0.0);
    assert_eq!(rt.state.seen.get(), Some((Side::Bottom, Align::Start)));
}

#[test]
fn chained_submenu_reads_parent_placement_and_follows_flip() {
    #[derive(Default)]
    struct S {
        parent_seen: Cell<Option<(Side, Align)>>,
        sub_seen: Cell<Option<(Side, Align)>>,
    }
    fn view(s: &S, cx: &mut Cx<S>) -> El {
        let parent_p = cx.anchor_placement("parent-menu");
        s.parent_seen.set(parent_p);
        s.sub_seen.set(cx.anchor_placement("submenu"));
        let sub_side = parent_p.map_or(Side::Right, |(side, _)| side);
        let parent_menu = col().key("parent-menu").size(120.0, 80.0)
            .anchored("btn", Side::Right, Align::Start, 4.0)
            .child(el().key("item").size(120.0, 30.0));
        let submenu = col().key("submenu").size(100.0, 50.0)
            .anchored("item", sub_side, Align::Start, 4.0);
        // Trigger near right edge of 500pt window: parent flips Right -> Left (x = 420 - 120 - 4 = 296),
        // submenu reads parent's Side::Left and opens to the left of parent (x = 296 - 100 - 4 = 192).
        col().size(500.0, 300.0)
            .child(el().key("btn").absolute(420.0, 60.0).size(60.0, 24.0))
            .child(parent_menu)
            .child(submenu)
    }

    let mut rt = Runtime::new(S::default(), view, (500.0, 300.0), Env::new(true, 1.0));
    rt.frame(0.0);
    assert_eq!(rt.state.parent_seen.get(), Some((Side::Left, Align::Start)));
    assert_eq!(rt.state.sub_seen.get(), Some((Side::Left, Align::Start)));
    assert_eq!(rt.presented("parent-menu").unwrap().0, [296.0, 60.0, 120.0, 80.0]);
    assert_eq!(rt.presented("submenu").unwrap().0, [192.0, 60.0, 100.0, 50.0]);
}

#[test]
fn memo_and_view_invalidate_only_when_placement_flips_across_scroll() {
    struct S {
        cx_builds: Arc<AtomicUsize>,
        memo_builds: Arc<AtomicUsize>,
        seen: Rc<Cell<Option<(Side, Align)>>>,
        tick: u32,
    }
    fn view(s: &S, cx: &mut Cx<S>) -> El {
        let cx_builds = s.cx_builds.clone();
        let memo_builds = s.memo_builds.clone();
        let seen = s.seen.clone();
        let rows = col().key("rows").children((0..30).map(|i| {
            let mut r = el().key(format!("row-{i}")).w(200.0).h(30.0);
            if i == 8 { r = r.key("trigger"); }
            r
        }));
        let scroller = col().key("scroller").size(220.0, 300.0).scroll(ScrollSpec { stick_bottom: false }).child(rows);
        let pop = col().key("pop").size(100.0, 80.0).anchored("trigger", Side::Bottom, Align::Start, 4.0);
        // Memo reading via Cx::anchor_placement
        let cx_badge = memo("cx-badge", 0u32, || {
            cx_builds.fetch_add(1, Ordering::Relaxed);
            let p = cx.anchor_placement("pop");
            seen.set(p);
            text(format!("{p:?}"))
        });
        // Memo reading via memo::anchor_placement (without Cx)
        let free_badge = memo("free-badge", 0u32, || {
            memo_builds.fetch_add(1, Ordering::Relaxed);
            let p = crate::memo::anchor_placement("pop");
            text(format!("{p:?}"))
        });
        col().size(400.0, 300.0).child(scroller).child(pop).child(cx_badge).child(free_badge)
            .child(text(format!("{}", s.tick)))
    }

    let state = S {
        cx_builds: Arc::new(AtomicUsize::new(0)),
        memo_builds: Arc::new(AtomicUsize::new(0)),
        seen: Rc::new(Cell::new(None)),
        tick: 0,
    };
    let mut rt = Runtime::new(state, view, (400.0, 300.0), Env::new(true, 1.0));
    rt.frame(0.0);
    // Row 8 is at y = 240..270 in a 300pt window: 80pt popup below would end at 354 > 292,
    // so it flips to Top (240 - 80 - 4 = 156 >= 8).
    assert_eq!(rt.state.seen.get(), Some((Side::Top, Align::Start)));
    let commits_0 = rt.stats.commits;
    let cx_b0 = rt.state.cx_builds.load(Ordering::Relaxed);
    let memo_b0 = rt.state.memo_builds.load(Ordering::Relaxed);

    // Unrelated state commit does NOT rebuild either memo when placement is unchanged.
    rt.state.tick += 1;
    rt.invalidate();
    rt.frame(0.0);
    assert_eq!(rt.state.cx_builds.load(Ordering::Relaxed), cx_b0, "cx memo reused on unchanged placement");
    assert_eq!(rt.state.memo_builds.load(Ordering::Relaxed), memo_b0, "free memo reused on unchanged placement");
    let commits_1 = rt.stats.commits;

    // Small wheel scroll (20pt down): trigger moves to y = 220..250 -> below still overflows (334 > 292), stays Top.
    rt.wheel(100.0, 100.0, -20.0);
    for _ in 0..4 { rt.frame(1.0 / 120.0); }
    assert_eq!(rt.stats.commits, commits_1, "non-flipping scroll causes zero commits");
    assert_eq!(rt.state.cx_builds.load(Ordering::Relaxed), cx_b0, "non-flipping scroll causes zero memo rebuilds");
    assert_eq!(rt.state.memo_builds.load(Ordering::Relaxed), memo_b0);

    // Larger wheel scroll (140pt more down -> 160pt total): trigger moves to y = 80..110 -> below fits, flips to Bottom!
    rt.wheel(100.0, 100.0, -140.0);
    for _ in 0..4 { rt.frame(1.0 / 120.0); }
    assert_eq!(rt.state.seen.get(), Some((Side::Bottom, Align::Start)));
    assert_eq!(rt.stats.commits, commits_1 + 1, "flipping scroll triggers exactly one commit");
    assert_eq!(rt.state.cx_builds.load(Ordering::Relaxed), cx_b0 + 1, "cx memo rebuilt once on flip");
    assert_eq!(rt.state.memo_builds.load(Ordering::Relaxed), memo_b0 + 1, "free memo rebuilt once on flip");
    assert!(rt.stats.commits > commits_0);

    // Further wheel scroll staying on Bottom (20pt more down -> 180pt total) causes zero additional commits or rebuilds.
    let commits_2 = rt.stats.commits;
    rt.wheel(100.0, 100.0, -20.0);
    for _ in 0..4 { rt.frame(1.0 / 120.0); }
    assert_eq!(rt.stats.commits, commits_2, "staying on Bottom after scroll causes zero commits");
    assert_eq!(rt.state.cx_builds.load(Ordering::Relaxed), cx_b0 + 1);
    assert_eq!(rt.state.memo_builds.load(Ordering::Relaxed), memo_b0 + 1);
}

#[test]
fn missing_plain_unresolved_and_cyclic_return_none_without_extra_passes() {
    #[derive(Default)]
    struct S {
        views: Cell<usize>,
        missing: Cell<Option<(Side, Align)>>,
        plain: Cell<Option<(Side, Align)>>,
        unresolved: Cell<Option<(Side, Align)>>,
        cyclic: Cell<Option<(Side, Align)>>,
    }
    fn view(s: &S, cx: &mut Cx<S>) -> El {
        s.views.set(s.views.get() + 1);
        s.missing.set(cx.anchor_placement("does-not-exist"));
        s.plain.set(cx.anchor_placement("plain-box"));
        s.unresolved.set(cx.anchor_placement("orphan"));
        s.cyclic.set(cx.anchor_placement("cyc-a"));
        col().size(400.0, 300.0)
            .child(el().key("plain-box").size(40.0, 20.0))
            .child(col().key("orphan").size(40.0, 20.0).anchored("no-anchor", Side::Bottom, Align::Start, 4.0))
            .child(col().key("cyc-a").size(40.0, 20.0).anchored("cyc-b", Side::Bottom, Align::Start, 4.0))
            .child(col().key("cyc-b").size(40.0, 20.0).anchored("cyc-a", Side::Bottom, Align::Start, 4.0))
    }

    let mut rt = Runtime::new(S::default(), view, (400.0, 300.0), Env::new(true, 1.0));
    rt.frame(0.0);
    assert_eq!(rt.state.missing.get(), None);
    assert_eq!(rt.state.plain.get(), None);
    assert_eq!(rt.state.unresolved.get(), None);
    assert_eq!(rt.state.cyclic.get(), None);
    assert_eq!(rt.state.views.get(), 1, "all-None anchor_placement reads settle in a single view pass");
}

#[test]
fn window_resize_flipping_overlay_wakes_reader() {
    #[derive(Default)]
    struct S {
        views: Cell<usize>,
        seen: Cell<Option<(Side, Align)>>,
    }
    fn view(s: &S, cx: &mut Cx<S>) -> El {
        s.views.set(s.views.get() + 1);
        s.seen.set(cx.anchor_placement("pop"));
        col().w_full().h_full()
            .child(el().key("trigger").absolute(300.0, 60.0).size(40.0, 24.0))
            .child(col().key("pop").size(120.0, 50.0).anchored("trigger", Side::Right, Align::Start, 4.0))
    }

    let mut rt = Runtime::new(S::default(), view, (600.0, 300.0), Env::new(true, 1.0));
    rt.frame(0.0);
    assert_eq!(rt.state.seen.get(), Some((Side::Right, Align::Start)));
    let views_after_open = rt.state.views.get();

    // Small resize that does NOT flip (600 -> 560): relayout succeeds without running view.
    rt.set_viewport((560.0, 300.0), 1.0);
    rt.frame(1.0 / 120.0);
    assert_eq!(rt.state.seen.get(), Some((Side::Right, Align::Start)));
    assert_eq!(rt.state.views.get(), views_after_open, "non-flipping resize runs no view");

    // Narrow resize that forces Right -> Left flip (560 -> 400: 344 + 120 = 464 > 392): view re-runs!
    rt.set_viewport((400.0, 300.0), 1.0);
    rt.frame(1.0 / 120.0);
    assert_eq!(rt.state.seen.get(), Some((Side::Left, Align::Start)));
    assert_eq!(rt.state.views.get(), views_after_open + 1, "flipping resize wakes reader");
}
