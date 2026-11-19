use super::*;
use crate::el::{GrowthSpec, ScrollSpec, row};

fn fifty_rows_view(_: &(), _: &mut Cx<()>) -> El {
    col()
        .key("scroll")
        .size(200.0, 200.0)
        .scroll(ScrollSpec { stick_bottom: false })
        .children((0..50).map(|i| row().key(format!("r{i}")).w_full().h(30.0)))
}

#[test]
fn scroll_with_fifty_direct_row_children_scrolls_to_the_last_row() {
    let mut rt = Runtime::new((), fifty_rows_view, (200.0, 200.0), Env::new(true, 1.0));
    rt.frame(0.0);
    let last = rt.elements().into_iter().find(|e| e.key.as_deref() == Some("r49")).unwrap();
    assert_eq!(last.rect[1], 1470.0);

    rt.wheel(100.0, 100.0, -10_000.0);
    rt.frame(1.0 / 60.0);
    let last = rt.elements().into_iter().find(|e| e.key.as_deref() == Some("r49")).unwrap();
    assert_eq!(last.rect[1] + last.rect[3], 200.0, "scrolled flush to the 50th direct child");
    assert_eq!(rt.scroll_at_bottom("scroll"), Some(true));
}

#[test]
fn scroll_x_with_fifty_direct_children_scrolls_to_the_last_child() {
    fn view(_: &(), _: &mut Cx<()>) -> El {
        row()
            .key("strip")
            .size(200.0, 60.0)
            .scroll_x()
            .children((0..50).map(|i| el().key(format!("c{i}")).w(40.0).h(60.0)))
    }
    let mut rt = Runtime::new((), view, (200.0, 60.0), Env::new(true, 1.0));
    rt.frame(0.0);
    let last = rt.elements().into_iter().find(|e| e.key.as_deref() == Some("c49")).unwrap();
    assert_eq!(last.rect[0], 1960.0);

    rt.wheel_xy(100.0, 30.0, -10_000.0, 0.0);
    rt.frame(1.0 / 60.0);
    let last = rt.elements().into_iter().find(|e| e.key.as_deref() == Some("c49")).unwrap();
    assert_eq!(last.rect[0] + last.rect[2], 200.0, "scrolled flush to the 50th direct horizontal child");
}

#[test]
fn stick_bottom_with_several_direct_children_stays_pinned_as_rows_arrive() {
    fn view(count: &usize, _: &mut Cx<usize>) -> El {
        col()
            .key("scroll")
            .size(200.0, 150.0)
            .scroll(ScrollSpec { stick_bottom: true })
            .children((0..*count).map(|i| row().key(format!("r{i}")).w_full().h(40.0)))
    }
    let mut rt = Runtime::new(6usize, view, (200.0, 150.0), Env::new(true, 1.0));
    rt.frame(0.0);
    let last = rt.elements().into_iter().find(|e| e.key.as_deref() == Some("r5")).unwrap();
    assert_eq!(last.rect[1] + last.rect[3], 150.0, "opens pinned to the last direct child");

    for next in 7..=10 {
        rt.update(move |n, _| *n = next);
        for _ in 0..60 {
            rt.frame(1.0 / 120.0);
        }
        let key = format!("r{}", next - 1);
        let last = rt.elements().into_iter().find(|e| e.key.as_deref() == Some(key.as_str())).unwrap();
        assert!(
            (last.rect[1] + last.rect[3] - 150.0).abs() < 0.05,
            "after adding {key}, bottom was {} instead of 150.0",
            last.rect[1] + last.rect[3]
        );
        assert_eq!(rt.scroll_at_bottom("scroll"), Some(true));
    }
}

#[test]
fn scroll_into_view_reaches_the_fortieth_direct_child() {
    let mut rt = Runtime::new((), fifty_rows_view, (200.0, 200.0), Env::new(true, 1.0));
    rt.frame(0.0);
    rt.update(|_, tx| tx.scroll_into_view("r39"));
    rt.frame(1.0 / 60.0);
    let r39 = rt.elements().into_iter().find(|e| e.key.as_deref() == Some("r39")).unwrap();
    assert_eq!(r39.rect[1] + r39.rect[3], 200.0, "40th direct child brought into view at bottom edge");
}

#[test]
fn growth_stack_child_after_a_header_child_grows_and_stays_pinned() {
    fn view(count: &usize, _: &mut Cx<usize>) -> El {
        let stack = col()
            .key("stack")
            .w_full()
            .growth(GrowthSpec::default())
            .children((0..*count).map(|i| row().key(format!("r{i}")).w_full().h(50.0)));
        col()
            .key("scroll")
            .size(200.0, 200.0)
            .scroll(ScrollSpec { stick_bottom: true })
            .child(el().key("banner").w_full().h(60.0))
            .child(stack)
    }
    let mut rt = Runtime::new(5usize, view, (200.0, 200.0), Env::new(true, 1.0));
    rt.frame(0.0);
    let r4 = rt.elements().into_iter().find(|e| e.key.as_deref() == Some("r4")).unwrap();
    assert!((r4.rect[1] + r4.rect[3] - 200.0).abs() < 0.05);

    rt.update(|n, _| *n = 6);
    rt.frame(1.0 / 120.0);
    assert!(rt.animating, "growth stack child animates new row");
    for _ in 0..120 {
        rt.frame(1.0 / 120.0);
    }
    let r5 = rt.elements().into_iter().find(|e| e.key.as_deref() == Some("r5")).unwrap();
    assert!((r5.rect[1] + r5.rect[3] - 200.0).abs() < 0.05);
}

#[test]
fn scroll_xy_with_direct_children_scrolls_both_axes_and_warns_on_trailing_growth_sibling() {
    let _warnings = layout_diagnostics::scope();
    fn view(bad: &bool, _: &mut Cx<bool>) -> El {
        if *bad {
            return col()
                .key("scroll")
                .size(200.0, 200.0)
                .scroll(ScrollSpec { stick_bottom: false })
                .child(col().key("stack").w_full().growth(GrowthSpec::default()).child(el().h(50.0)))
                .child(el().key("after").w_full().h(50.0));
        }
        col()
            .key("scroll")
            .size(200.0, 200.0)
            .scroll_xy()
            .children((0..20).map(|i| row().key(format!("r{i}")).w(500.0).h(30.0)))
    }
    let mut rt = Runtime::new(false, view, (200.0, 200.0), Env::new(true, 1.0));
    rt.frame(0.0);
    rt.update(|_, tx| tx.scroll_to_instant("scroll", 9_999.0, 9_999.0));
    rt.frame(1.0 / 60.0);
    let r19 = rt.elements().into_iter().find(|e| e.key.as_deref() == Some("r19")).unwrap();
    assert_eq!((r19.rect[0] + r19.rect[2], r19.rect[1] + r19.rect[3]), (200.0, 200.0));
    assert!(!layout_diagnostics::warned_at("growth-sibling:"));

    rt.update(|bad, _| *bad = true);
    rt.frame(1.0 / 60.0);
    assert!(layout_diagnostics::warned_at("growth-sibling:"));
}

#[test]
fn animated_header_above_pinned_growth_stack_keeps_last_row_on_bottom_every_frame() {
    fn view(tall: &bool, _: &mut Cx<bool>) -> El {
        let stack = col()
            .key("stack")
            .w_full()
            .growth(GrowthSpec::default())
            .children((0..6).map(|i| row().key(format!("r{i}")).w_full().h(40.0)));
        col()
            .key("scroll")
            .size(200.0, 200.0)
            .scroll(ScrollSpec { stick_bottom: true })
            .child(el().key("header").w_full().h(if *tall { 120.0 } else { 40.0 }))
            .child(stack)
    }
    let mut rt = Runtime::new(false, view, (200.0, 200.0), Env::new(true, 1.0));
    rt.frame(0.0);
    rt.update(|tall, _| *tall = true);
    for frame in 0..90 {
        rt.frame(1.0 / 120.0);
        let r5 = rt.elements().into_iter().find(|e| e.key.as_deref() == Some("r5")).unwrap();
        assert!(
            (r5.rect[1] + r5.rect[3] - 200.0).abs() < 0.05,
            "frame {frame}: last row bottom {} left viewport bottom 200.0",
            r5.rect[1] + r5.rect[3]
        );
    }
}

#[test]
fn header_before_virtual_stack_reveals_unmounted_row_and_keeps_header_at_origin() {
    fn view(_: &(), _: &mut Cx<()>) -> El {
        let stack = col()
            .key("rows")
            .w_full()
            .growth(GrowthSpec::default())
            .virtualize(20.0)
            .children((0..100).map(|i| row().key(format!("r{i}")).w_full().h(20.0)));
        col()
            .key("scroll")
            .size(200.0, 200.0)
            .scroll(ScrollSpec { stick_bottom: false })
            .child(el().key("header").w_full().h(500.0))
            .child(stack)
    }
    let mut rt = Runtime::new((), view, (200.0, 200.0), Env::new(true, 1.0));
    rt.frame(0.0);
    assert!(rt.elements().iter().all(|e| e.key.as_deref() != Some("r35")), "r35 starts unmounted");

    // Scrolling near the bottom of the 500pt header must keep r0 mounted and visible.
    rt.wheel(100.0, 100.0, -450.0);
    rt.frame(1.0 / 60.0);
    let r0 = rt.elements().into_iter().find(|e| e.key.as_deref() == Some("r0")).expect("r0 stays mounted below header");
    assert!((r0.rect[1] - 50.0).abs() < 0.05, "r0 at y={} instead of 50.0", r0.rect[1]);

    // Revealing an initially unmounted row accounts for the header height and does not shift the header by `before`.
    rt.update(|_, tx| tx.scroll_into_view("r35"));
    rt.frame(1.0 / 60.0);
    assert!(rt.commit.virtuals["scroll/rows"].before > 0.0, "earlier virtual rows were culled");
    let r35 = rt.elements().into_iter().find(|e| e.key.as_deref() == Some("r35")).expect("r35 mounted and revealed");
    assert!((r35.rect[1] + r35.rect[3] - 200.0).abs() < 0.05, "r35 bottom {} != 200.0", r35.rect[1] + r35.rect[3]);
    let header = rt.elements().into_iter().find(|e| e.key.as_deref() == Some("header")).unwrap();
    assert!((header.rect[1] - (-1020.0)).abs() < 0.05, "header top {} moved with virtual culling", header.rect[1]);
}

#[test]
fn scroll_x_with_multiple_children_unwraps_w_full_text_column() {
    fn view(_: &(), _: &mut Cx<()>) -> El {
        row()
            .key("strip")
            .size(200.0, 80.0)
            .scroll_x()
            .child(el().key("badge").w(40.0).h(20.0))
            .child(col().key("wide").w_full().child(crate::el::text("unwrapped horizontal text ".repeat(12)).wrap()))
            .child(col().key("short").w_full().child(crate::el::text("ok")))
    }
    let mut rt = Runtime::new((), view, (200.0, 80.0), Env::new(true, 1.0));
    rt.frame(0.0);
    let badge = rt.elements().into_iter().find(|e| e.key.as_deref() == Some("badge")).unwrap();
    let wide = rt.elements().into_iter().find(|e| e.key.as_deref() == Some("wide")).unwrap();
    let short = rt.elements().into_iter().find(|e| e.key.as_deref() == Some("short")).unwrap();
    assert_eq!(badge.rect[2], 40.0);
    assert!(wide.rect[2] > 400.0, "w_full column in multi-child scroll_x unwraps past viewport width, got {}", wide.rect[2]);
    assert_eq!(short.rect[2], 200.0, "w_full child keeps min-width 100% of viewport when narrower");
}


#[test]
fn scroll_into_view_of_a_virtual_row_taller_than_the_viewport_lands_its_end_from_above() {
    // Short rows, one 500pt row in a 200pt viewport, and a varying tail after it.
    fn view(tail: &usize, _: &mut Cx<usize>) -> El {
        let n = 31 + *tail;
        let stack = col()
            .key("rows")
            .w_full()
            .growth(GrowthSpec::default())
            .virtual_rows(n, 20.0, |_: &usize, i| format!("r{i}").into(), |_: &usize, i| {
                row().w_full().h(if i == 30 { 500.0 } else { 20.0 })
            });
        col()
            .key("scroll")
            .size(200.0, 200.0)
            .scroll(ScrollSpec { stick_bottom: false })
            .child(stack)
    }
    for tail in [0usize, 1, 2, 5] {
        let mut rt = Runtime::new(tail, view, (200.0, 200.0), Env::new(true, 1.0));
        rt.frame(0.0);
        rt.update(|_, tx| tx.scroll_into_view("r30"));
        for _ in 0..30 {
            rt.frame(1.0 / 120.0);
        }
        let r30 = rt.elements().into_iter().find(|e| e.key.as_deref() == Some("r30")).expect("r30 revealed");
        assert!(
            (r30.rect[1] + r30.rect[3] - 200.0).abs() < 0.05,
            "tail {tail}: moving down, the row's end lands at the viewport's bottom, not its top: {:?}",
            r30.rect
        );
    }
}

#[test]
fn raised_and_anchored_overlays_occlude_horizontal_wheel_touch_fling_and_autoscroll() {
    fn view(_: &(), _: &mut Cx<()>) -> El {
        col().size(300.0, 200.0)
            .child(
                col().key("bg").size(300.0, 200.0)
                    .scroll(ScrollSpec { stick_bottom: false }).scroll_xy()
                    .child(el().size(1000.0, 1000.0)),
            )
            .child(el().key("panel").absolute(0.0, 0.0).size(150.0, 150.0).raise().fill(Color::WHITE))
            .child(el().key("anchor").absolute(160.0, 0.0).size(20.0, 20.0))
            .child(el().key("pop").size(80.0, 80.0).fill(Color::WHITE)
                .anchored("anchor", crate::Side::Bottom, crate::Align::Start, 0.0))
    }
    let mut rt = Runtime::new((), view, (300.0, 200.0), Env::new(true, 1.0));
    rt.frame(0.0);
    let bg = rt.tree.nodes.iter().find(|n| n.key.as_deref() == Some("bg")).unwrap().id;

    // Clear of overlays, bg is found on both axes.
    assert_eq!(rt.scroll_at_axis(260.0, 160.0, true), Some(bg));
    assert_eq!(rt.scroll_at_axis(260.0, 160.0, false), Some(bg));

    // Over the raised panel (50, 50) and anchored popover (180, 50), bg is occluded.
    for (x, y, label) in [(50.0, 50.0, "raised panel"), (180.0, 50.0, "anchored popover")] {
        assert_eq!(rt.scroll_at_axis(x, y, true), None, "horizontal scroll_at_axis over {label}");
        assert_eq!(rt.scroll_at_axis(x, y, false), None, "vertical scroll_at_axis over {label}");
        rt.wheel_xy(x, y, -60.0, 0.0);
        rt.fling(x, y, -400.0);
        rt.frame(1.0 / 60.0);
        assert_eq!(rt.scroll.scrolls[&bg].x.offset, 0.0, "horizontal wheel over {label} must not scroll bg");
        assert_eq!(rt.scroll.scrolls[&bg].offset, 0.0, "fling over {label} must not scroll bg");
    }

    // Touch pan over the raised panel must not scroll bg.
    rt.touch(1, crate::TouchPhase::Started, 50.0, 50.0, 0.0);
    rt.touch(1, crate::TouchPhase::Moved, 10.0, 50.0, 0.02);
    rt.touch(1, crate::TouchPhase::Moved, 10.0, 10.0, 0.04);
    rt.touch(1, crate::TouchPhase::Ended, 10.0, 10.0, 0.06);
    rt.frame(1.0 / 60.0);
    assert_eq!((rt.scroll.scrolls[&bg].x.offset, rt.scroll.scrolls[&bg].offset), (0.0, 0.0), "touch over panel must not scroll bg");

    // Middle-click autoscroll over the raised panel must not latch onto bg.
    if cfg!(any(target_os = "windows", target_os = "linux")) {
        rt.button_down(50.0, 50.0, crate::PointerButton::Middle);
        assert!(rt.core().autoscroll_cursor().is_none(), "autoscroll over raised panel must not start on occluded bg");
    }
}

#[test]
fn keep_on_screen_virtual_row_in_scaled_viewport_holds_its_position() {
    fn view(inserted: &usize, _: &mut Cx<usize>) -> El {
        let ins = *inserted;
        let n = 60 + ins;
        let stack = col().key("rows").w_full().growth(GrowthSpec::default())
            .virtual_rows(n, 50.0, move |_: &usize, i| {
                if i < 2 { format!("r{i}").into() }
                else if i < 2 + ins { format!("ins-{}", i - 2).into() }
                else { format!("r{}", i - ins).into() }
            }, |_: &usize, _| el().w_full().h(50.0));
        col().size(1200.0, 1200.0).child(
            col().key("scroll").size(200.0, 200.0).scale(4.0).origin(0.0, 0.0)
                .scroll(ScrollSpec { stick_bottom: false })
                .child(stack),
        )
    }
    let mut rt = Runtime::new(0usize, view, (1200.0, 1200.0), Env::new(true, 1.0));
    rt.frame(0.0);
    // r4 is at layout y = 200. Scroll to 50 so r1 is the virtual reading anchor while r4 sits
    // 150 unscaled points (= 600 scaled points) below the viewport top.
    rt.update(|_, tx| tx.scroll_to_instant("scroll", 0.0, 50.0));
    rt.frame(1.0 / 60.0);
    let rel_y = |rt: &Runtime<usize>| {
        let s = rt.elements().into_iter().find(|e| e.key.as_deref() == Some("scroll")).unwrap().rect[1];
        rt.elements().into_iter().find(|e| e.key.as_deref() == Some("r4")).map(|e| e.rect[1] - s)
    };
    let before = rel_y(&rt).expect("r4 mounted before commit");
    assert!((before - 600.0).abs() < 0.5, "r4 at 150 unscaled = 600 scaled: {before}");

    // Insert 40 rows after r1 (keeping r1 as the virtual anchor while pushing r4 to index 44,
    // outside the mounted window) and ask to keep r4 on screen: `apply_keeps` must restore r4 via
    // `virtual_row` using unscaled viewport points.
    rt.update(|ins, tx| {
        *ins = 40;
        tx.instant();
        tx.keep_on_screen("r4");
    });
    rt.frame(1.0 / 60.0);
    let after = rel_y(&rt).expect("r4 must remain mounted in view after keep_on_screen");
    assert!((after - before).abs() < 0.5, "scaled keep_on_screen jumped: before={before}, after={after}");
}
