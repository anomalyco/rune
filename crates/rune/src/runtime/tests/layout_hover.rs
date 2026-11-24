use super::*;

const DT: f64 = 1.0 / 120.0;

/// What the rows heard: `on_hover` enters and `on_hover_change` changes.
#[derive(Default)]
struct Rows {
    growth: bool, open: bool, gone: bool, modal: bool,
    /// Rows with only a hover fill, and a pressable spot over row 5 that `pan` moves.
    plain: bool, pan: Option<crate::live::Live<[f32; 2]>>, spot: Vec<bool>,
    /// Rows with tooltips, and cursors that differ from their neighbours'.
    tips: bool,
    enters: Vec<usize>, changes: Vec<(usize, bool)>,
}

const CURSORS: [crate::el::Cursor; 3] = [crate::el::Cursor::Pointer, crate::el::Cursor::Move, crate::el::Cursor::Grab];

/// Twelve 20-point rows, each revealing an action on hover and darkening while pressed.
/// Opening row 0 grows it by 200 points, and the rows below slide down under the pointer.
fn view(s: &Rows, _: &mut Cx<Rows>) -> El {
    let rows = (0..12).filter(|&k| !(s.gone && k == 5)).map(|k| {
        let row = crate::row().key(format!("r{k}")).w_full().h(if k == 0 && s.open { 220.0 } else { 20.0 });
        let row = if s.plain { row.hover(Color::WHITE) } else {
            row.hover_group().press(Color::WHITE).press_scale(0.9)
                .on_press(|_: &mut Rows| {})
                .on_hover(move |s: &mut Rows| s.enters.push(k))
                .on_hover_change(move |s: &mut Rows, inside, _| s.changes.push((k, inside)))
                .child(el().key(format!("a{k}")).size(10.0, 10.0).fill(Color::WHITE).group_opacity(0.0, 1.0))
        };
        if s.tips { row.help(format!("row {k}")).cursor(CURSORS[k % 3]) } else { row }
    });
    let mut list = col().key("rows").w_full().children(rows);
    if s.growth { list = list.growth(crate::el::GrowthSpec::default()); }
    let root = if let Some(pan) = &s.pan {
        // Place `list` inside `spot` so `spot` is an ancestor rather than a covering sibling:
        // `spot` holds `hovered` and `hover_within` while `r5` holds the layout-carried hover track.
        col().size(200.0, 400.0).child(
            el().key("spot").absolute(90.0, 100.0).size(20.0, 20.0).live_offset(pan)
                .on_press(|_: &mut Rows| {}).on_hover_change(|s: &mut Rows, inside, _| s.spot.push(inside))
                .child(list.absolute(-90.0, -100.0).w(200.0)),
        )
    } else {
        col().size(200.0, 400.0).child(list)
    };
    if s.modal { root.child(col().key("sheet").absolute(150.0, 350.0).size(40.0, 40.0).modal(true, |s: &mut Rows| s.modal = false)) } else { root }
}

/// The rows whose action shows at all this frame.
fn shown(rt: &Runtime<Rows>) -> Vec<String> {
    rt.elements().into_iter().filter(|e| e.key.as_ref().is_some_and(|k| k.starts_with('a')) && e.opacity > 0.0)
        .filter_map(|e| e.key).collect()
}

/// The row drawn under `y` this frame.
fn row_at(rt: &Runtime<Rows>, y: f32) -> Option<usize> {
    rt.elements().into_iter().filter(|e| e.rect[1] <= y && y < e.rect[1] + e.rect[3])
        .find_map(|e| e.key?.strip_prefix('r')?.parse().ok())
}

fn top(rt: &Runtime<Rows>, key: &str) -> f32 {
    rt.elements().into_iter().find(|e| e.key.as_deref() == Some(key)).unwrap().rect[1]
}

/// The pointer resting over row 5.
fn resting(rows: Rows) -> Runtime<Rows> {
    let mut rt = Runtime::new(rows, view, (200.0, 400.0), Env::new(true, 1.0));
    rt.frame(0.0);
    rt.pointer_move(100.0, 110.0);
    rt.frame(DT);
    rt.frame(DT);
    rt
}

/// Row 5 hovered under a still pointer, then row 0 opening.
fn sliding(growth: bool) -> Runtime<Rows> {
    let mut rt = resting(Rows { growth, ..Rows::default() });
    assert_eq!(shown(&rt), ["a5"]);
    assert_eq!((&rt.state.enters[..], &rt.state.changes[..]), (&[5][..], &[(5, true)][..]));
    rt.update(|s, _| s.open = true);
    rt
}

/// Runs frames until row 11 stops moving, returning how many it moved on and every action shown.
fn settle(rt: &mut Runtime<Rows>) -> (usize, Vec<String>) {
    let (mut moved, mut seen) = (0, Vec::new());
    let mut y = top(rt, "r11");
    for _ in 0..600 {
        rt.frame(DT);
        for k in shown(rt) { if !seen.contains(&k) { seen.push(k); } }
        let now = top(rt, "r11");
        if (now - y).abs() < 1e-4 && !rt.animating { break; }
        if now != y { moved += 1; }
        y = now;
    }
    for _ in 0..30 { rt.frame(DT); }
    (moved, seen)
}

#[test]
fn rows_sliding_under_a_still_pointer_dont_take_its_hover() {
    for growth in [false, true] {
        let mut rt = sliding(growth);
        let (moved, seen) = settle(&mut rt);
        assert!(moved > 5, "growth {growth}: the rows slid");
        assert_eq!(seen, ["a5", "a0"], "growth {growth}: no row passing under the pointer lit up");
        assert_eq!(rt.state.enters, [5, 0], "growth {growth}");
        assert_eq!(rt.state.changes, [(5, true), (5, false), (0, true)], "growth {growth}");
        assert_eq!(row_at(&rt, 110.0), Some(0), "growth {growth}");
        assert_eq!(shown(&rt), ["a0"], "growth {growth}: once they settle, the row under the pointer has it");
        assert!(!rt.pointer.hover_hold().held() && !rt.pointer.hover_hold().released());
    }
}

#[test]
fn moving_the_pointer_during_a_slide_retargets_at_once() {
    let mut rt = sliding(false);
    for _ in 0..6 { rt.frame(DT); }
    assert_eq!(shown(&rt), ["a5"]);
    assert!(rt.pointer.hover_hold().held());
    let under = row_at(&rt, 111.0).unwrap();
    assert_ne!(under, 5, "a row slid under the pointer");
    rt.pointer_move(100.0, 111.0);
    rt.frame(DT);
    assert_eq!(rt.state.enters, [5, under]);
    assert_eq!(rt.state.changes, [(5, true), (5, false), (under, true)]);
    let now = format!("a{}", row_at(&rt, 111.0).unwrap());
    assert!(shown(&rt).contains(&now), "{now}: {:?}", shown(&rt));
}

#[test]
fn the_hovered_row_leaving_mid_slide_lets_go() {
    let mut rt = sliding(false);
    for _ in 0..4 { rt.frame(DT); }
    rt.update(|s, _| s.gone = true);
    rt.frame(DT);
    rt.frame(DT);
    assert_eq!(rt.state.changes, [(5, true), (5, false)], "the leaving row hears the pointer go");
    let (_, seen) = settle(&mut rt);
    assert!(seen.iter().all(|k| k == "a5" || k == "a0"), "nothing else lit up: {seen:?}");
    assert_eq!(rt.state.enters, [5, 0]);
    assert_eq!(rt.state.changes, [(5, true), (5, false), (0, true)]);
}

#[test]
fn a_modal_opening_mid_slide_lets_go() {
    let mut rt = sliding(false);
    for _ in 0..4 { rt.frame(DT); }
    rt.update(|s, _| s.modal = true);
    rt.frame(DT);
    rt.frame(DT);
    assert_eq!(rt.state.changes, [(5, true), (5, false)], "the inert row hears the pointer go");
    settle(&mut rt);
    assert_eq!(shown(&rt), Vec::<String>::new());
    assert_eq!(rt.state.enters, [5]);
}

#[test]
fn a_press_during_a_slide_still_shows_and_lets_go() {
    let mut rt = sliding(false);
    for _ in 0..4 { rt.frame(DT); }
    let under = row_at(&rt, 110.0).unwrap();
    let id = rt.tree.nodes.iter().find(|n| n.named(&format!("r{under}"))).unwrap().id;
    rt.pointer_down(100.0, 110.0);
    rt.frame(DT);
    assert_eq!(rt.scene.store().target(id, P_PRESS), Some(1.0), "the press shows mid-slide");
    assert_eq!(rt.scene.store().target(id, P_PRESS_SCALE), Some(0.9));
    rt.pointer_up(100.0, 110.0);
    rt.frame(DT);
    rt.frame(DT);
    assert_eq!(rt.scene.store().target(id, P_PRESS), Some(0.0), "the press lets go mid-slide");
    assert_eq!(rt.scene.store().target(id, P_PRESS_SCALE), Some(1.0));
    assert!(!rt.pointer.hover_hold().held() && rt.pointer.hover_hold().released(), "the press let go of the hold until the slide settles");
}

/// Each row's enters minus its leaves, from `on_hover_change`.
fn inside(rt: &Runtime<Rows>) -> Vec<usize> {
    let mut net = [0i32; 12];
    for &(k, on) in &rt.state.changes { net[k] += if on { 1 } else { -1 }; }
    assert!(net.iter().all(|&n| n == 0 || n == 1), "every enter is heard once before its leave: {:?}", rt.state.changes);
    (0..12).filter(|&k| net[k] == 1).collect()
}

#[test]
fn a_press_mid_slide_follows_the_pointer_and_pairs_each_enter_with_a_leave() {
    let mut rt = sliding(false);
    for _ in 0..4 { rt.frame(DT); }
    rt.pointer_down(100.0, 110.0);
    assert!(rt.pointer.hover_hold().released(), "a press lets go of the hold");
    for _ in 0..40 {
        rt.frame(DT);
        let inside = inside(&rt);
        assert!(inside.len() <= 2, "the pressed row and the one under the pointer: {inside:?}");
        assert!(rt.pointer.hover_within.len() <= 2, "{:?}", rt.pointer.hover_within);
    }
    rt.pointer_up(100.0, 110.0);
    settle(&mut rt);
    assert_eq!(inside(&rt), [0]);
    assert_eq!(rt.pointer.hover_within.len(), 1);
}

#[test]
fn a_move_before_the_next_tick_sees_layout_just_set_moving() {
    // A spring a commit between frames started (a scroll's), not yet ticked.
    let mut rt = resting(Rows::default());
    let id = rt.tree.nodes.iter().find(|n| n.named("r3")).unwrap().id;
    rt.scene.animate(id, prop::OFFSET, rune_motion::Vec2::new(0.0, 40.0), crate::tok::motion::SNAPPY);
    rt.pointer_move(100.0, 111.0);
    assert!(rt.pointer.hover_hold().released(), "an untouched spring");
    // A growing element's height.
    let mut rt = resting(Rows::default());
    let id = rt.tree.nodes.iter().find(|n| n.named("r3")).unwrap().id;
    let mut grow = crate::growth::GrowTrack::default();
    grow.target = 100.0;
    rt.motion.grows.insert(id, grow);
    rt.pointer_move(100.0, 111.0);
    assert!(rt.pointer.hover_hold().released(), "a growth");
}

#[test]
fn a_hold_keeps_all_three_hover_states_together() {
    let pan = crate::live::Live::new([0.0, 0.0]);
    let mut rt = resting(Rows { plain: true, pan: Some(pan.clone()), ..Rows::default() });
    let target = |rt: &Runtime<Rows>, key: &str| {
        let id = rt.tree.nodes.iter().find(|n| n.named(key)).unwrap().id;
        rt.scene.store().target(id, P_HOVER)
    };
    let spot = rt.tree.nodes.iter().find(|n| n.named("spot")).unwrap().path.clone();
    assert_eq!(rt.pointer.hovered.as_deref(), Some(spot.as_str()));
    assert_eq!((&rt.state.spot[..], target(&rt, "r5")), (&[true][..], Some(1.0)));
    rt.update(|s, _| s.open = true);
    for _ in 0..6 { rt.frame(DT); }
    assert_ne!(row_at(&rt, 110.0), Some(5), "a row slid under the pointer");
    assert_eq!(target(&rt, "r5"), Some(1.0), "the rows' hover is held");
    // The spot jumps out from under the pointer while the rows' hover is held.
    pan.set([-80.0, 0.0]);
    rt.frame(DT);
    rt.frame(DT);
    assert_eq!(target(&rt, "r5"), Some(1.0));
    assert_eq!(rt.state.spot, [true], "the spot's leave waits with the rows' hover");
    assert_eq!(rt.pointer.hovered.as_deref(), Some(spot.as_str()), "and so does the hovered element");
    settle(&mut rt);
    assert_eq!(rt.state.spot, [true, false]);
    assert_eq!(rt.pointer.hovered, None);
    assert_eq!((target(&rt, "r5"), target(&rt, "r0")), (Some(0.0), Some(1.0)));
}

#[test]
fn the_cursor_and_tooltip_stay_with_the_held_row() {
    let mut rt = resting(Rows { tips: true, ..Rows::default() });
    rt.help_delay = 0.0;
    rt.frame(DT);
    rt.frame(DT);
    assert_eq!((rt.cursor(), rt.help_tooltip().map(|t| t.0.to_owned())), (CURSORS[2], Some("row 5".to_owned())));
    rt.update(|s, _| s.open = true);
    let (mut cursors, mut tips) = (Vec::new(), Vec::new());
    for _ in 0..120 {
        rt.frame(DT);
        if !cursors.contains(&rt.cursor()) { cursors.push(rt.cursor()); }
        let tip = rt.help_tooltip().map(|t| t.0.to_owned());
        if !tips.contains(&tip) { tips.push(tip); }
    }
    settle(&mut rt);
    assert_eq!(cursors, [CURSORS[2], CURSORS[0]], "row 5's cursor, then row 0's");
    assert!(tips.iter().flatten().all(|t| t == "row 5" || t == "row 0"), "no tooltip for a passing row: {tips:?}");
    assert_eq!((rt.cursor(), rt.help_tooltip().map(|t| t.0.to_owned())), (CURSORS[0], Some("row 0".to_owned())));
}

#[test]
fn stationary_child_hovers_while_ancestor_only_resizes() {
    #[derive(Default)]
    struct Card {
        open: bool,
        wide: bool,
        swap: bool,
        changes: Vec<(&'static str, bool)>,
    }
    fn card_view(s: &Card, _: &mut Cx<Card>) -> El {
        let tile = if s.swap {
            el().key("t1").size(80.0, 20.0).hover(Color::WHITE)
                .on_press(|_: &mut Card| {})
                .on_hover_change(|s: &mut Card, inside, _| s.changes.push(("t1", inside)))
        } else {
            el().key("t0").size(80.0, 20.0).hover(Color::WHITE)
                .on_press(|_: &mut Card| {})
                .on_hover_change(|s: &mut Card, inside, _| s.changes.push(("t0", inside)))
        };
        let mut card = col().key("card").w(if s.wide { 320.0 } else { 200.0 }).child(tile);
        if s.open {
            card = card.child(el().key("panel").w_full().h(160.0));
        }
        col().size(400.0, 400.0).child(card)
    }
    for (wide, open, label) in [(true, false, "width"), (false, true, "height"), (true, true, "both")] {
        let mut rt = Runtime::new(Card::default(), card_view, (400.0, 400.0), Env::new(true, 1.0));
        rt.frame(0.0);
        rt.pointer_move(40.0, 10.0);
        rt.frame(DT);
        rt.frame(DT);
        let id = |rt: &Runtime<Card>, key: &str| rt.tree.nodes.iter().find(|n| n.named(key)).unwrap().id;
        assert_eq!(rt.scene.store().target(id(&rt, "t0"), P_HOVER), Some(1.0), "{label}: initial tile hovered");
        assert_eq!(rt.state.changes, [("t0", true)], "{label}");

        // First: keeping the same stationary child while the ancestor resizes keeps it hovered every frame.
        rt.update(move |s, _| { s.wide = wide; s.open = open; });
        let mut steady_frames = 0;
        for f in 0..120 {
            rt.frame(DT);
            let card_id = id(&rt, "card");
            let t0_id = id(&rt, "t0");
            assert!(!rt.pointer.hover_hold().held(), "{label} steady frame {f}");
            assert_eq!(rt.scene.store().target(t0_id, P_HOVER), Some(1.0), "{label} steady frame {f}");
            if rt.scene.store().is_animating(card_id, prop::FRAME) { steady_frames += 1; } else { break; }
        }
        assert!(steady_frames > 5, "{label}: ancestor sprang its size");

        // Second: resizing back while a stationary child replaces the tile under the still pointer
        // must let the new child hover on frame 1 instead of holding hover until the resize settles.
        rt.update(|s, _| { s.wide = false; s.open = false; s.swap = true; });
        let mut resizing_frames = 0;
        for f in 0..120 {
            rt.frame(DT);
            let card_id = id(&rt, "card");
            let t1_id = id(&rt, "t1");
            let t1_rect = rt.elements().into_iter().find(|e| e.key.as_deref() == Some("t1")).unwrap().rect;
            assert_eq!(t1_rect, [0.0, 0.0, 80.0, 20.0], "{label} frame {f}: child stayed stationary");
            assert!(!rt.pointer.hover_hold().held(), "{label} frame {f}: resizing ancestor must not hold hover");
            assert_eq!(rt.scene.store().target(t1_id, P_HOVER), Some(1.0), "{label} frame {f}: stationary child hovered");
            if rt.scene.store().is_animating(card_id, prop::FRAME) { resizing_frames += 1; } else { break; }
        }
        assert!(resizing_frames > 5, "{label}: ancestor actually sprang its size");
        rt.frame(DT);
        assert_eq!(rt.state.changes, [("t0", true), ("t0", false), ("t1", true)], "{label}");
    }
}

#[test]
fn moving_far_edge_child_still_holds_hover_while_ancestor_widens() {
    #[derive(Default)]
    struct Wide { wide: bool, ride_trailing: bool, changes: Vec<(usize, bool)> }
    fn wide_view(s: &Wide, _: &mut Cx<Wide>) -> El {
        let w = if s.wide { 260.0 } else { 120.0 };
        let tile = |k: usize, x: f32| {
            let e = el().key(format!("t{k}")).size(40.0, 20.0).hover(Color::WHITE)
                .on_press(|_: &mut Wide| {})
                .on_hover_change(move |s: &mut Wide, inside, _| s.changes.push((k, inside)));
            if s.ride_trailing { e.absolute(x, 0.0).ride_trailing() } else { e }
        };
        let card = if s.ride_trailing {
            col().key("card").size(w, 40.0).child(tile(0, w - 80.0)).child(tile(1, w - 40.0))
        } else {
            crate::row().key("card").size(w, 40.0).justify_end().child(tile(0, 0.0)).child(tile(1, 0.0))
        };
        col().size(300.0, 100.0).child(card)
    }
    for ride_trailing in [false, true] {
        let mut rt = Runtime::new(Wide { ride_trailing, ..Wide::default() }, wide_view, (300.0, 100.0), Env::new(true, 1.0));
        rt.frame(0.0);
        rt.pointer_move(100.0, 10.0);
        rt.frame(DT);
        rt.frame(DT);
        assert_eq!(rt.state.changes, [(1, true)], "ride_trailing={ride_trailing}");

        rt.update(|s, _| s.wide = true);
        let mut saw_t0_under_pointer = false;
        let mut held_any = false;
        for _ in 0..300 {
            rt.frame(DT);
            let t0 = rt.elements().into_iter().find(|e| e.key.as_deref() == Some("t0")).unwrap();
            let t0_id = rt.tree.nodes.iter().find(|n| n.named("t0")).unwrap().id;
            if ride_trailing {
                assert!(!rt.scene.store().is_animating(t0_id, prop::FRAME), "ride_trailing child rides parent's trailing edge without its own frame spring");
            }
            if t0.rect[0] <= 100.0 && 100.0 < t0.rect[0] + t0.rect[2] {
                saw_t0_under_pointer = true;
                assert!(rt.pointer.hover_hold().held(), "ride_trailing={ride_trailing}: moving child holds hover");
                assert_eq!(rt.scene.store().target(t0_id, P_HOVER).unwrap_or(0.0), 0.0, "ride_trailing={ride_trailing}: passing tile must not light up");
            }
            held_any |= rt.pointer.hover_hold().held();
            if !rt.animating { break; }
        }
        for _ in 0..20 { rt.frame(DT); }
        assert!(saw_t0_under_pointer && held_any, "ride_trailing={ride_trailing}: t0 slid across the pointer");
        assert_eq!(rt.state.changes, [(1, true), (1, false)], "ride_trailing={ride_trailing}: t0 never heard hover while passing");
    }
}

#[test]
fn ride_trailing_child_hovers_while_ancestor_widens_left_from_fixed_trailing_edge() {
    #[derive(Default)]
    struct LeftWide { wide: bool, swap: bool, changes: Vec<(&'static str, bool)> }
    fn view(s: &LeftWide, _: &mut Cx<LeftWide>) -> El {
        let w = if s.wide { 240.0 } else { 120.0 };
        let tile = if s.swap {
            el().key("t1").size(40.0, 20.0).absolute(w - 40.0, 0.0).ride_trailing().hover(Color::WHITE)
                .on_press(|_: &mut LeftWide| {})
                .on_hover_change(|s: &mut LeftWide, inside, _| s.changes.push(("t1", inside)))
        } else {
            el().key("t0").size(40.0, 20.0).absolute(w - 40.0, 0.0).ride_trailing().hover(Color::WHITE)
                .on_press(|_: &mut LeftWide| {})
                .on_hover_change(|s: &mut LeftWide, inside, _| s.changes.push(("t0", inside)))
        };
        crate::row().size(260.0, 60.0).justify_end().child(col().key("card").size(w, 40.0).child(tile))
    }
    let mut rt = Runtime::new(LeftWide::default(), view, (260.0, 60.0), Env::new(true, 1.0));
    rt.frame(0.0);
    rt.pointer_move(240.0, 10.0);
    rt.frame(DT);
    rt.frame(DT);
    assert_eq!(rt.state.changes, [("t0", true)]);

    rt.update(|s, _| { s.wide = true; s.swap = true; });
    let mut animating_frames = 0;
    for f in 0..120 {
        rt.frame(DT);
        let card_id = rt.tree.nodes.iter().find(|n| n.named("card")).unwrap().id;
        let t1_id = rt.tree.nodes.iter().find(|n| n.named("t1")).unwrap().id;
        let t1_rect = rt.elements().into_iter().find(|e| e.key.as_deref() == Some("t1")).unwrap().rect;
        assert!((t1_rect[0] - 220.0).abs() < 1e-3, "frame {f}: trailing child stayed at x=220: {t1_rect:?}");
        assert!(!rt.pointer.hover_hold().held(), "frame {f}: fixed trailing edge must not hold hover");
        assert_eq!(rt.scene.store().target(t1_id, P_HOVER), Some(1.0), "frame {f}: stationary trailing child hovered");
        if rt.scene.store().is_animating(card_id, prop::FRAME) { animating_frames += 1; } else { break; }
    }
    assert!(animating_frames > 5, "card widened to the left");
    rt.frame(DT);
    assert_eq!(rt.state.changes, [("t0", true), ("t0", false), ("t1", true)]);
}

#[test]
fn resizing_element_itself_still_holds_hover_while_its_edge_crosses_pointer() {
    #[derive(Default)]
    struct BoxState { wide: bool, changes: Vec<bool> }
    fn view(s: &BoxState, _: &mut Cx<BoxState>) -> El {
        col().size(300.0, 100.0).child(
            el().key("box").size(if s.wide { 240.0 } else { 100.0 }, 40.0).hover(Color::WHITE)
                .on_press(|_: &mut BoxState| {})
                .on_hover_change(|s: &mut BoxState, inside, _| s.changes.push(inside)),
        )
    }
    let mut rt = Runtime::new(BoxState::default(), view, (300.0, 100.0), Env::new(true, 1.0));
    rt.frame(0.0);
    rt.pointer_move(150.0, 20.0);
    rt.frame(DT);
    rt.frame(DT);
    assert!(rt.state.changes.is_empty());

    rt.update(|s, _| s.wide = true);
    let mut saw_crossing = false;
    for _ in 0..300 {
        rt.frame(DT);
        let id = rt.tree.nodes.iter().find(|n| n.named("box")).unwrap().id;
        let rect = rt.elements().into_iter().find(|e| e.key.as_deref() == Some("box")).unwrap().rect;
        let animating = rt.scene.store().is_animating(id, prop::FRAME);
        if animating && rect[0] + rect[2] > 150.0 {
            saw_crossing = true;
            assert!(rt.pointer.hover_hold().held(), "own resizing frame holds hover while crossing pointer");
            assert_eq!(rt.scene.store().target(id, P_HOVER).unwrap_or(0.0), 0.0, "box must not light up mid-resize");
        }
        if !rt.animating { break; }
    }
    for _ in 0..20 { rt.frame(DT); }
    assert!(saw_crossing, "box's right edge crossed the pointer while still animating");
    assert_eq!(rt.state.changes, [true], "once settled, the box hears hover");
}
