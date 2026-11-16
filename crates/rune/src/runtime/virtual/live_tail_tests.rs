use super::*;
use crate::{GrowthSpec, ScrollSpec, col, el, motion_gate::{Run, Recording, gate_rows}};

#[derive(Clone, Copy, Debug, PartialEq)]
enum Case { Delivery, Narrow, Rewrapping, BelowList, Unpinned, InstantChild }

#[derive(Clone, Copy, PartialEq)]
enum Phase { Waiting, Rewrapping, Delivered }

struct Feed { case: Case, phase: Phase }

fn view(feed: &Feed, _: &mut Cx<Feed>) -> El {
    let delivered = feed.phase == Phase::Delivered;
    let narrow = matches!(feed.case, Case::Narrow | Case::Rewrapping) && feed.phase != Phase::Waiting;
    let label = el().key("label").w(40.0).h(10.0).fill(Color::WHITE);
    let mut content = col().key("content").w_full().pt(if delivered { 12.0 } else { 4.0 }).child(label);
    if feed.case == Case::InstantChild { content = content.instant_changes(); }
    let pending = col().key("pending").w_full().h(if delivered { 64.0 } else { 48.0 }).arrives_whole().child(content);
    let working = el().key("working").w(40.0).h(20.0).fill(Color::WHITE);
    let stack = col().key("rows").w_full().growth(GrowthSpec::default()).virtualize(48.0)
        .child(el().key("earlier").w_full().h(320.0))
        .children(if delivered { [pending, working] } else { [working, pending] });
    let view = col().key("viewport").w(if narrow { 200.0 } else { 280.0 }).h(180.0)
        .scroll(ScrollSpec { stick_bottom: feed.case != Case::Unpinned }).child(stack);
    col().w_full().h_full().child(if feed.case == Case::BelowList { view.child(el().key("after").w_full().h(96.0)) } else { view })
}

fn poll(s: &mut Feed, t: f64, tx: &mut Tx) -> bool {
    if s.phase == Phase::Waiting && s.case == Case::Rewrapping && (0.47..0.5).contains(&t) { s.phase = Phase::Rewrapping; return true; }
    if s.phase != Phase::Delivered && t >= 0.5 {
        s.phase = Phase::Delivered;
        tx.grow_visible_tail("rows");
        return true;
    }
    false
}

fn record(case: Case) -> Recording {
    let mut rt = Runtime::new(Feed { case, phase: Phase::Waiting }, view, (300.0, 180.0), Env::new(true, 1.0));
    rt.motion_needs_cause = true;
    rt.poll = Some(poll);
    let mut run = Run::new(rt, 120.0);
    run.idle(0.45);
    run.step();
    let growing = matches!(case, Case::Delivery | Case::InstantChild);
    while run.rt.state.phase != Phase::Delivered { assert!(run.rt.t < 0.6, "delivery did not poll"); run.step(); }
    let row = run.rt.nodes.iter().find(|n| n.key.as_deref() == Some("pending"));
    if growing {
        let h = run.rt.grows[&row.unwrap().id].h;
        assert!((48.0..64.0).contains(&h), "live row started at {h}");
        if case == Case::InstantChild {
            let label = run.rt.nodes.iter().position(|n| n.key.as_deref() == Some("label")).unwrap();
            let group = run.rt.nodes[label].parent.unwrap();
            assert!((run.rt.drawn[label][1] - run.rt.drawn[group][1] - 12.0).abs() < 0.01, "explicitly instant contents must land within the moving row");
        }
    } else if let Some(row) = row {
        let g = &run.rt.grows[&row.id];
        assert_eq!(g.h, 64.0, "{case:?}: the row should land whole");
        assert!(!g.moving(), "{case:?}: no tail growth work");
    }
    run.play(1.0);
    run.quiet();
    run.step_n(4);
    run.rec
}

#[test]
fn live_tail_motion_gate() {
    // A jump is an error in these two traces. In the instant controls below it's required:
    // check the actual landing height instead of allowlisting jumps in a motion recording.
    gate_rows([Case::Delivery, Case::Unpinned].into_iter()
        .map(|case| (format!("live-tail/{case:?}"), Box::new(move || record(case)) as crate::motion_gate::Job)).collect(), &[]);
}

#[test]
fn live_tail_motion_gate_instant_controls() {
    for case in [Case::Narrow, Case::Rewrapping, Case::BelowList, Case::InstantChild] { record(case); }
}

#[test]
fn live_tail_motion_gate_covered_polls_keep_one_request_per_selector() {
    let mut pending = Tx::default();
    for _ in 0..32 {
        let mut poll = Tx::default();
        poll.grow_visible_tail("left/rows");
        poll.grow_visible_tail("right/rows");
        crate::runtime::action::merge(&mut pending, poll);
    }
    assert_eq!(pending.live_growth, ["left/rows", "right/rows"]);
}
