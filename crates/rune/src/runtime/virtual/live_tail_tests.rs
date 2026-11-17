use super::*;
use crate::{GrowthSpec, ScrollSpec, col, el, motion_gate::{Run, Recording, gate_rows}};

#[derive(Clone, Copy, Debug, PartialEq)]
enum Case { Delivery, Room, GeometryOnly, Narrow, Rewrapping, BelowList, Unpinned, InstantChild }

#[derive(Clone, Copy, PartialEq)]
enum Phase { Waiting, Rewrapping, Delivered }

struct Feed { case: Case, phase: Phase }

fn view(feed: &Feed, _: &mut Cx<Feed>) -> El {
    let delivered = feed.phase == Phase::Delivered;
    let narrow = matches!(feed.case, Case::Narrow | Case::Rewrapping) && feed.phase != Phase::Waiting;
    let label = el().key("label").w(40.0).h(10.0).fill(if delivered { Color::WHITE } else { Color::BLACK });
    let mut content = col().key("content").w_full().pt(if delivered { 12.0 } else { 4.0 }).child(label);
    if feed.case == Case::InstantChild { content = content.instant_changes(); }
    content = content.child_exit(Transition::opacity()).children((!delivered).then(|| el().key("badge").size(10.0, 10.0).fill(Color::WHITE)));
    let pending = col().key("pending").w_full().h(if delivered { 64.0 } else { 48.0 }).arrives_whole().child(content);
    let working = el().key("working").w(40.0).h(20.0).fill(Color::WHITE);
    if feed.case == Case::Room {
        let rows = col().key("rows").w_full().growth(GrowthSpec::default()).virtualize(48.0)
            .child(el().key("earlier").w_full().h(320.0))
            .children(delivered.then(|| el().key("arrival").w_full().h(64.0).arrives_whole())).child(working);
        return col().w_full().h_full().child(col().key("viewport").w_full().grow(1.0).min_h(0.0)
            .scroll(ScrollSpec { stick_bottom: true }).child(rows))
            .child(el().key("room").w_full().h(if delivered { 0.0 } else { 40.0 }));
    }
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
        if s.case == Case::Room { tx.grow_visible_tail_from("rows", "arrival", "room"); } else { if s.case == Case::GeometryOnly { tx.grow_visible_tail("rows"); } else { tx.grow_visible_tail_with_ink("rows"); } }
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
    let growing = matches!(case, Case::Delivery | Case::GeometryOnly | Case::InstantChild);
    while run.rt.state.phase != Phase::Delivered { assert!(run.rt.t < 0.6, "delivery did not poll"); run.step(); }
    let row = run.rt.nodes.iter().find(|n| n.key.as_deref() == Some("pending"));
    if case == Case::Room {
        let room = run.rt.nodes.iter().position(|n| n.key.as_deref() == Some("room")).unwrap();
        let first = run.rt.drawn[room][3];
        assert!(first > 0.0 && first < 40.0, "room snapped to {first}");
        let row = run.rt.nodes.iter().find(|n| n.key.as_deref() == Some("arrival")).unwrap();
        let remaining = 1.0 - run.rt.grows[&row.id].h / 64.0;
        assert!((first - 40.0 * remaining).abs() < 1.0 / 64.0, "room {first} must follow the drawn row on the first frame");
    }
    if growing {
        let h = run.rt.grows[&row.unwrap().id].h;
        assert!((48.0..64.0).contains(&h), "live row started at {h}");
        if case == Case::Delivery {
            let badge = run.rt.nodes.iter().find(|n| n.named("badge")).expect("the visible badge fades on its row");
            assert!(run.rt.is_exiting(badge.id));
            let label = run.rt.nodes.iter().find(|n| n.named("label")).unwrap();
            assert!(run.rt.scene.store().is_animating(label.id, P_FILL));
        }
        if case == Case::GeometryOnly {
            assert!(!run.rt.nodes.iter().any(|n| n.named("badge")), "geometry-only growth lands child exits");
        }
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
    gate_rows([Case::Delivery, Case::Room, Case::Unpinned].into_iter()
        .map(|case| (format!("live-tail/{case:?}"), Box::new(move || record(case)) as crate::motion_gate::Job)).collect(), &[]);
}

#[test]
fn live_tail_motion_gate_instant_controls() {
    for case in [Case::GeometryOnly, Case::Narrow, Case::Rewrapping, Case::BelowList, Case::InstantChild] { record(case); }
}

#[test]
fn live_tail_motion_gate_joined_corner_retargets_on_adjacent_deliveries() {
    fn grouped(accepted: &usize, _: &mut Cx<usize>) -> El {
        let apart = *accepted == 1;
        let second = el().key("second").w_full().h(30.0)
            .corners([10.0, if apart { 10.0 } else { 4.0 }, 10.0, 10.0])
            .opacity(if apart { 0.75 } else { 1.0 });
        let first = el().key("first").w_full().h(30.0);
        let working = el().key("working").w_full().h(20.0);
        let order = match accepted { 0 => [working, first, second], 1 => [first, working, second], _ => [first, second, working] };
        col().w_full().h_full().scroll(ScrollSpec { stick_bottom: true }).child(col().key("rows").w_full()
            .growth(GrowthSpec::default()).virtualize(30.0).child(el().key("earlier").w_full().h(240.0)).children(order))
    }
    let mut rt = Runtime::new(0, grouped, (280.0, 180.0), Env::new(true, 1.0));
    rt.motion_needs_cause = true;
    rt.poll = Some(|accepted, t, tx| {
        if t < 0.5 || *accepted == 2 { return false; }
        *accepted += 1;
        tx.grow_visible_tail_with_ink("rows");
        true
    });
    let mut run = Run::new(rt, 60.0);
    while run.rt.state == 0 { assert!(run.rt.t < 0.6); run.step(); }
    let id = run.rt.nodes.iter().find(|n| n.named("second")).unwrap().id;
    let radii = |rt: &Runtime<usize>| rt.scene.presented(id).unwrap().radius.0;
    let [tl, first, br, bl] = radii(&run.rt);
    assert_eq!([tl, br, bl], [10.0; 3]);
    assert!(first > 4.0 && first < 6.0, "the corner begins to open: {first}");
    assert_eq!(run.rt.scene.get(id, prop::OPACITY), Some(0.75), "only corners inherit the neighbour's motion");
    run.step();
    assert_eq!(run.rt.state, 2);
    let next = radii(&run.rt)[1];
    assert!(next > 4.0 && (next - first).abs() < 1.0, "retarget from {first}, not the intermediate goal: {next}");
    run.play(1.0);
    assert_eq!(radii(&run.rt), [10.0, 4.0, 10.0, 10.0]);
}

#[test]
fn live_tail_motion_gate_joined_faces_share_their_drawn_and_interactive_seam() {
    fn view(s: &(u8, u8), _: &mut Cx<(u8, u8)>) -> El {
        let split = s.0 == 1;
        let prompt = |key, label, hit| col().key(key).w_full().arrives_whole().joined_row_face(label)
            .pt(if split { 12.0 } else { 4.0 }).pb(if split { 20.0 } else { 0.0 })
            .child(el().key(label).w_full().h(30.0).hit().fill(Color::WHITE).on_press(move |s: &mut (u8,u8)| s.1 = hit));
        let (a,b) = (prompt("a", "top", 1), prompt("b", "bottom", 2));
        let between = el().key("between").w(20.0).h(36.0);
        let order = match s.0 { 0 => [between,a,b], 1 => [a,between,b], _ => [a,b,between] };
        col().w_full().h_full().scroll(ScrollSpec { stick_bottom: true }).child(col().key("rows").w_full()
            .growth(GrowthSpec::default()).virtualize(40.0).child(el().key("older").w_full().h(300.0)).children(order))
    }
    let mut rt = Runtime::new((0,0), view, (280.0,180.0), Env::new(true,1.0));
    rt.motion_needs_cause = true;
    rt.poll = Some(|s,t,tx| { if t < 0.5 || s.0 == 2 { return false; } s.0 += 1; tx.grow_visible_tail("rows"); true });
    let mut run = Run::new(rt,60.0);
    while run.rt.state.0 == 0 { assert!(run.rt.t < 0.6); run.step(); }
    for step in 0..36 {
        let rect = |key| run.rt.drawn[run.rt.nodes.iter().position(|n| n.named(key)).unwrap()];
        let (a,b) = (rect("top"),rect("bottom"));
        assert!((b[1]-a[1]-a[3]-4.0).abs()<0.05, "step {step}: {a:?} {b:?}");
        if step == 4 {
            for (r,hit) in [(a,1),(b,2)] { run.rt.pointer_down(80.0,r[1]+15.0); run.rt.pointer_up(80.0,r[1]+15.0); run.rt.frame(0.0); assert_eq!(run.rt.state.1,hit); }
        }
        run.step();
    }
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
    assert_eq!(pending.live_growth, [("left/rows".into(), false), ("right/rows".into(), false)]);
}
