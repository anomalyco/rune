use rune::col;

use super::*;

#[derive(Default)]
struct App {
    tabs: usize,
    lone: bool,
    drag: TabDragState,
    /// Each tear move's `torn`.
    torn: Vec<bool>,
}

fn view(s: &App, _: &mut rune::Cx<App>) -> El {
    let k = Kit::neutral(true);
    let tabs = (0..s.tabs).map(|i| TabItem::new(format!("tab{i}"), format!("Tab {i}")));
    let strip = TabStrip::new("strip", TabOrientation::Horizontal)
        .tabs(tabs)
        .viewport_width(Some(600.0))
        .drag_state(&s.drag, |s: &mut App| &mut s.drag)
        .tear_out(Some(TearOutSpec { lone: s.lone, ..TearOutSpec::default() }))
        .on_tear_move(|s: &mut App, m, _| s.torn.push(m.torn))
        .build(&k);
    col().size(600.0, 300.0).child(strip)
}

/// Drags the first tab 80 points down, off the strip, and back.
fn drag_off(tabs: usize, lone: bool) -> rune::Runtime<App> {
    let mut rt = rune::Runtime::new(App { tabs, lone, ..App::default() }, view, (600.0, 300.0), rune::Env::new(true, 1.0));
    rt.frame(0.0);
    rt.frame(1.0 / 60.0);
    let r = rt.elements().into_iter().find(|e| e.key.as_deref() == Some("t-tab0")).expect("the first tab").rect;
    let (x, y) = (r[0] + r[2] / 2.0, r[1] + r[3] / 2.0);
    rt.pointer_move(x, y);
    rt.pointer_down(x, y);
    for dy in [10.0, 40.0, 80.0] {
        rt.pointer_move(x, y + dy);
        rt.frame(1.0 / 60.0);
    }
    rt
}

#[test]
fn a_lone_tab_tears_out_only_when_the_spec_lets_it() {
    let rt = drag_off(1, false);
    assert!(!rt.state.drag.torn_out() && rt.state.torn.is_empty(), "the only tab stays by default");
    let rt = drag_off(1, true);
    assert!(rt.state.drag.torn_out(), "TearOutSpec::lone tears it out");
    assert_eq!(rt.state.torn.last(), Some(&true));
}

#[test]
fn a_tab_among_others_tears_out_either_way() {
    for lone in [false, true] {
        assert!(drag_off(2, lone).state.drag.torn_out(), "lone: {lone}");
    }
}

#[test]
fn a_drag_released_over_its_tab_does_not_swallow_the_next_keyboard_press() {
    #[derive(Default)]
    struct S { drag: TabDragState, selected: Vec<String> }
    fn view(s: &S, _: &mut rune::Cx<S>) -> El {
        let k = Kit::neutral(true);
        let tabs = (0..2).map(|i| TabItem::new(format!("tab{i}"), format!("Tab {i}")));
        let strip = TabStrip::new("strip", TabOrientation::Horizontal)
            .tabs(tabs)
            .viewport_width(Some(600.0))
            .drag_state(&s.drag, |s: &mut S| &mut s.drag)
            .on_select(|s: &mut S, id, _| s.selected.push(id.to_owned()))
            .build(&k);
        col().size(600.0, 300.0).child(strip)
    }
    let mut rt = rune::Runtime::new(S::default(), view, (600.0, 300.0), rune::Env::new(true, 1.0));
    rt.frame(0.0);
    rt.frame(1.0 / 60.0);
    let r = rt.elements().into_iter().find(|e| e.key.as_deref() == Some("t-tab0")).expect("tab0").rect;
    let (x, y) = (r[0] + r[2] / 2.0, r[1] + r[3] / 2.0);
    rt.pointer_move(x, y);
    rt.pointer_down(x, y);
    rt.pointer_move(x + 20.0, y);
    rt.pointer_move(x, y);
    rt.pointer_up(x, y);
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.state.selected, ["tab0"], "select_early ran on down, drag did not fire a second select");
    rt.state.selected.clear();
    // Focusing tab0 and pressing Space must select it, not be swallowed by a stale drag flag.
    rt.key(rune::KeyEvent { key: rune::Key::Text(" ".into()), mods: rune::Mods::default() });
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.state.selected, ["tab0"]);
}

#[test]
fn web_a11y_mirror_exposes_tablist_closable_tabs_and_close_buttons() {
    struct State {
        active: String,
        closable: bool,
        hover_none: bool,
        closed: usize,
    }
    fn strip_view(s: &State, _: &mut rune::Cx<State>) -> El {
        TabStrip::new("tabs", TabOrientation::Horizontal)
            .tabs([
                TabItem::new("a", "Alpha").closable(s.closable),
                TabItem::new("b", "Beta").closable(s.closable),
            ])
            .active(Some(&s.active))
            .close_tab_stop(true)
            .hover_none(s.hover_none)
            .on_select(|s: &mut State, id, _| s.active = id.into())
            .on_close(|s: &mut State, _, _| s.closed += 1)
            .build(&Kit::neutral(false))
    }

    for (closable, hover_none, want_closes) in [(true, true, 2usize), (true, false, 2usize), (false, true, 0usize)] {
        let mut rt = rune::Runtime::new(
            State { active: "a".into(), closable, hover_none, closed: 0 },
            strip_view,
            (800.0, 120.0),
            rune::Env::new(false, 1.0),
        );
        rt.frame(0.0);
        for _ in 0..30 { rt.frame(1.0 / 60.0); }

        let items: Vec<serde_json::Value> = serde_json::from_str(&rt.a11y_json()).unwrap();
        let tablist_idx = items.iter().position(|e| e["k"] == "group" && e["o"] == "tablist")
            .unwrap_or_else(|| panic!("missing tablist (closable={closable}, hover_none={hover_none}): {items:?}"));
        assert_eq!(items[tablist_idx]["g"], "div");
        assert_eq!(items[tablist_idx]["t"], "Tabs");

        let tabs: Vec<_> = items.iter().filter(|e| e["k"] == "button" && e["o"] == "tab").collect();
        assert_eq!(tabs.len(), 2, "closable={closable}, hover_none={hover_none}: {items:?}");
        assert_eq!(tabs[0]["t"], "Alpha");
        assert_eq!(tabs[0]["se"], true);
        assert_eq!(tabs[0]["a"], tablist_idx);
        assert_eq!(tabs[1]["t"], "Beta");
        assert_eq!(tabs[1]["se"], false);
        assert_eq!(tabs[1]["a"], tablist_idx);

        let closes: Vec<_> = items.iter().filter(|e| e["k"] == "button" && e.get("o").is_none()).collect();
        assert_eq!(closes.len(), want_closes, "closable={closable}, hover_none={hover_none}: {items:?}");
        for close in closes {
            assert_eq!(close["t"], "Close tab");
            assert_eq!(close["a"], tablist_idx);
        }
    }
}

#[test]
fn tab_width_handles_inverted_and_nan_bounds_without_panicking() {
    let normal = TabOverflow::HORIZONTAL_DEFAULT;
    assert_eq!(normal.tab_width(1000.0, 2, 0, 6.0), 224.0);
    assert_eq!(normal.tab_width(206.0, 2, 0, 6.0), 100.0);
    assert_eq!(normal.tab_width(20.0, 2, 0, 6.0), 28.0);
    assert_eq!(normal.tab_width(400.0, 0, 1, 6.0), 224.0);

    // min_width above max_width: minimum wins (CSS min-width beats max-width).
    let inverted = TabOverflow::ShrinkThenScroll { min_width: 200.0, max_width: 100.0, pinned_width: 36.0, fade_width: 24.0 };
    assert_eq!(inverted.tab_width(500.0, 2, 0, 6.0), 200.0);
    assert_eq!(inverted.tab_width(100.0, 2, 0, 6.0), 200.0);
    assert_eq!(inverted.tab_width(500.0, 0, 0, 6.0), 200.0);

    // NaN in either or both bounds never panics.
    let nan_min = TabOverflow::ShrinkThenScroll { min_width: f32::NAN, max_width: 224.0, pinned_width: 36.0, fade_width: 24.0 };
    assert_eq!(nan_min.tab_width(206.0, 2, 0, 6.0), 100.0);
    assert_eq!(nan_min.tab_width(1000.0, 2, 0, 6.0), 224.0);
    assert_eq!(nan_min.tab_width(400.0, 0, 0, 6.0), 224.0);

    let nan_max = TabOverflow::ShrinkThenScroll { min_width: 28.0, max_width: f32::NAN, pinned_width: 36.0, fade_width: 24.0 };
    assert_eq!(nan_max.tab_width(500.0, 2, 0, 6.0), 28.0);
    assert_eq!(nan_max.tab_width(400.0, 0, 0, 6.0), 28.0);

    let both_nan = TabOverflow::ShrinkThenScroll { min_width: f32::NAN, max_width: f32::NAN, pinned_width: 36.0, fade_width: 24.0 };
    assert_eq!(both_nan.tab_width(206.0, 2, 0, 6.0), 100.0);
    assert_eq!(normal.tab_width(f32::NAN, 2, 0, 6.0), 28.0);

    // Building and laying out a strip with degenerate overflow bounds does not panic.
    for policy in [inverted, nan_min, nan_max] {
        for viewport in [Some(400.0), None] {
            let mut rt = rune::Runtime::new(
                (policy, viewport),
                |&(policy, viewport), _: &mut rune::Cx<_>| {
                    let k = Kit::neutral(true);
                    let strip = TabStrip::<(TabOverflow, Option<f32>)>::new("strip", TabOrientation::Horizontal)
                        .tabs([TabItem::new("a", "Alpha"), TabItem::new("b", "Beta")])
                        .overflow(policy)
                        .viewport_width(viewport)
                        .build(&k);
                    col().size(400.0, 80.0).child(strip)
                },
                (400.0, 80.0),
                rune::Env::new(true, 1.0),
            );
            rt.frame(0.0);
        }
    }
}

#[test]
fn closing_tab_collapses_its_slot_without_overlapping_neighbours() {
    struct S {
        tabs: Vec<&'static str>,
        orientation: TabOrientation,
    }
    fn view(s: &S, _: &mut rune::Cx<S>) -> El {
        let k = Kit::neutral(true);
        let items = s.tabs.iter().map(|id| TabItem::new(*id, format!("Tab {id}")));
        let strip = TabStrip::<S>::new("strip", s.orientation)
            .tabs(items)
            .viewport_width(Some(600.0))
            .build(&k);
        col().size(600.0, 300.0).child(strip)
    }

    for orientation in [TabOrientation::Horizontal, TabOrientation::Vertical] {
        let mut rt = rune::Runtime::new(
            S { tabs: vec!["a", "b", "c"], orientation },
            view,
            (600.0, 300.0),
            rune::Env::new(true, 1.0),
        );
        for _ in 0..40 {
            rt.frame(1.0 / 60.0);
        }
        let axis = usize::from(orientation == TabOrientation::Vertical);
        let init_len = rt.presented("t-b").expect("tab b present").0[2 + axis];
        assert!(init_len > 20.0);

        rt.update(|s, _| s.tabs.retain(|id| *id != "b"));
        let mut saw_shrunk = false;
        for f in 0..30 {
            rt.frame(1.0 / 60.0);
            let Some((b, _)) = rt.presented("t-b") else { break };
            let (a, _) = rt.presented("t-a").expect("tab a present");
            let (c, _) = rt.presented("t-c").expect("tab c present");
            assert!(a[axis] + a[2 + axis] <= b[axis] + 0.5, "{orientation:?} f={f}: tab a {:?} overlapped closing tab b {:?}", a, b);
            assert!(b[axis] + b[2 + axis] <= c[axis] + 0.5, "{orientation:?} f={f}: closing tab b {:?} overlapped tab c {:?}", b, c);
            if b[2 + axis] < init_len * 0.7 {
                saw_shrunk = true;
            }
        }
        assert!(saw_shrunk, "{orientation:?}: closing tab should collapse along strip main axis");
        for _ in 0..90 {
            if !rt.frame(1.0 / 60.0) {
                break;
            }
        }
        assert!(rt.presented("t-b").is_none(), "{orientation:?}: closed tab unmounts");
        assert!(!rt.frame(1.0 / 60.0), "{orientation:?}: idle after tab close");
    }

    // Verify closing a tab in TabStrip passes rune::motion_gate with zero findings.
    let rt = rune::Runtime::new(
        S { tabs: vec!["a", "b", "c"], orientation: TabOrientation::Horizontal },
        view,
        (600.0, 120.0),
        rune::Env::new(true, 1.0),
    );
    let mut run = rune::motion_gate::Run::new(rt, 120.0);
    run.idle(0.6);
    run.step_n(4);
    run.act("close tab b", |rt| rt.update(|s, _| s.tabs.retain(|id| *id != "b")));
    run.play(1.2);
    run.quiet();
    run.step_n(4);
    rune::motion_gate::gate_rows(vec![("tab-strip/close".to_owned(), Box::new(move || run.rec) as rune::motion_gate::Job)], &[]);
}

/// Four tabs with a break before the first and one before the third.
#[derive(Default)]
struct Broken {
    orientation: TabOrientation,
    drag: TabDragState,
    moves: Vec<(usize, usize)>,
}

/// A vertical break is 20 points high, a horizontal one a point wide.
fn broken_view(s: &Broken, _: &mut rune::Cx<Broken>) -> El {
    let k = Kit::neutral(true);
    let vertical = s.orientation == TabOrientation::Vertical;
    let mark = |key: &'static str| if vertical { rune::el().key(key).w_full().h(20.0) } else { rune::el().key(key).size(1.0, 16.0) };
    let length = if vertical { 20.0 } else { 1.0 };
    let tabs = (0..4).map(|i| TabItem::new(format!("tab{i}"), format!("Tab {i}")));
    let strip = TabStrip::new("strip", s.orientation)
        .tabs(tabs)
        .viewport_width(Some(600.0))
        .breaks([TabBreak::new("head", 0, mark("head"), length), TabBreak::new("mid", 2, mark("mid"), length)])
        .drag_state(&s.drag, |s: &mut Broken| &mut s.drag)
        .on_reorder(|s: &mut Broken, r, _| s.moves.push((r.from, r.to)))
        .build(&k);
    col().size(600.0, 400.0).child(strip)
}

fn broken(orientation: TabOrientation) -> rune::Runtime<Broken> {
    let mut rt = rune::Runtime::new(Broken { orientation, ..Broken::default() }, broken_view, (600.0, 400.0), rune::Env::new(true, 1.0));
    for _ in 0..40 {
        rt.frame(1.0 / 60.0);
    }
    rt
}

/// Where the element keyed `key` is laid out.
fn rect_of(rt: &rune::Runtime<Broken>, key: &str) -> [f32; 4] {
    rt.elements().into_iter().find(|e| e.key.as_deref() == Some(key)).unwrap_or_else(|| panic!("{key} is in the strip")).rect
}

#[test]
fn a_break_stands_before_its_tab_with_the_strips_gap_on_each_side() {
    // Across a titlebar: the gap, the break, the gap.
    let rt = broken(TabOrientation::Horizontal);
    let at = |key: &str| rect_of(&rt, key);
    let (head, mid) = (at("head"), at("mid"));
    let tabs: Vec<[f32; 4]> = (0..4).map(|i| at(&format!("t-tab{i}"))).collect();
    let near = |a: f32, b: f32, what: &str| assert!((a - b).abs() < 0.5, "{what}: {a} against {b}");
    near(tabs[0][0], head[0] + 1.0 + 6.0, "the first tab stands a gap after the leading break");
    near(tabs[1][0], tabs[0][0] + tabs[0][2] + 6.0, "tabs with no break between them keep the gap");
    near(mid[0], tabs[1][0] + tabs[1][2] + 6.0, "the break stands a gap after the tab before it");
    near(tabs[2][0], mid[0] + 1.0 + 6.0, "and a gap before the tab after it");
    near(tabs[3][0] + tabs[3][2], 600.0, "the tabs share what the breaks leave of the strip");

    // Down a sidebar a row reaches half the gap toward whatever stands beside it, and the break
    // pads the other half: a 28 point row a gap from the break on either side.
    let rt = broken(TabOrientation::Vertical);
    let at = |key: &str| rect_of(&rt, key);
    let (head, mid) = (at("head"), at("mid"));
    let rows: Vec<[f32; 4]> = (0..4).map(|i| at(&format!("t-tab{i}"))).collect();
    near(rows[0][1], head[1] + 20.0 + 1.5, "the first row reaches up to half a gap under the leading break");
    near(rows[0][3], 1.5 + 28.0 + 1.5, "and is a row with half a gap either side");
    near(rows[1][1], rows[0][1] + rows[0][3], "rows with no break between them touch, as before");
    near(mid[1], rows[1][1] + rows[1][3] + 1.5, "the break stands half a gap under the reach of the row above it");
    near(rows[2][1], mid[1] + 20.0 + 1.5, "and half a gap over the reach of the row under it");
    near(rows[3][3], 1.5 + 28.0, "the last row reaches no further down, as before");
}

#[test]
fn a_drag_stops_at_a_break() {
    for orientation in [TabOrientation::Horizontal, TabOrientation::Vertical] {
        let mut rt = broken(orientation);
        let axis = usize::from(orientation == TabOrientation::Vertical);
        // Drags tab `index` 300 points along the strip, either way, and lets go.
        let drag = |rt: &mut rune::Runtime<Broken>, index: usize, way: f32| {
            let r = rect_of(rt, &format!("t-tab{index}"));
            let (x, y) = (r[0] + r[2] / 2.0, r[1] + r[3] / 2.0);
            rt.pointer_move(x, y);
            rt.pointer_down(x, y);
            let mut to = (x, y);
            for step in [10.0, 100.0, 300.0] {
                to = if axis == 0 { (x + way * step, y) } else { (x, y + way * step) };
                rt.pointer_move(to.0, to.1);
                rt.frame(1.0 / 60.0);
            }
            rt.pointer_up(to.0, to.1);
            for _ in 0..40 {
                rt.frame(1.0 / 60.0);
            }
        };
        drag(&mut rt, 0, 1.0);
        assert_eq!(rt.state.moves, [(0, 1)], "{orientation:?}: the first tab goes no further than the break before the third");
        drag(&mut rt, 2, -1.0);
        assert_eq!(rt.state.moves, [(0, 1)], "{orientation:?}: the third tab stays behind the break before it");
        drag(&mut rt, 3, -1.0);
        assert_eq!(rt.state.moves, [(0, 1), (3, 2)], "{orientation:?}: the last tab goes as far as the break before the third");
    }
}

#[test]
fn arrow_keys_cross_a_break() {
    #[derive(Default)]
    struct S {
        vertical: bool,
        drag: TabDragState,
        selected: Vec<String>,
    }
    fn view(s: &S, _: &mut rune::Cx<S>) -> El {
        let k = Kit::neutral(true);
        let tabs = (0..4).map(|i| TabItem::new(format!("tab{i}"), format!("Tab {i}")));
        let (orientation, mark, length) = match s.vertical {
            true => (TabOrientation::Vertical, rune::el().key("mid").w_full().h(20.0), 20.0),
            false => (TabOrientation::Horizontal, rune::el().key("mid").size(1.0, 16.0), 1.0),
        };
        let strip = TabStrip::new("strip", orientation)
            .tabs(tabs)
            .viewport_width(Some(600.0))
            .breaks([TabBreak::new("mid", 2, mark, length)])
            .drag_state(&s.drag, |s: &mut S| &mut s.drag)
            .on_select(|s: &mut S, id, _| s.selected.push(id.to_owned()))
            .build(&k);
        col().size(600.0, 400.0).child(strip)
    }
    for vertical in [false, true] {
        let mut rt = rune::Runtime::new(S { vertical, ..S::default() }, view, (600.0, 400.0), rune::Env::new(true, 1.0));
        for _ in 0..40 {
            rt.frame(1.0 / 60.0);
        }
        // A press on the second tab, the last before the break, selects and focuses it.
        let r = rt.elements().into_iter().find(|e| e.key.as_deref() == Some("t-tab1")).expect("tab1").rect;
        let (x, y) = (r[0] + r[2] / 2.0, r[1] + r[3] / 2.0);
        rt.pointer_move(x, y);
        rt.pointer_down(x, y);
        rt.pointer_up(x, y);
        rt.frame(1.0 / 60.0);
        assert_eq!(rt.state.selected, ["tab1"], "vertical: {vertical}");
        rt.state.selected.clear();
        // The arrow takes the focus, and with it the selection, to the third tab, past the break.
        rt.key(rune::KeyEvent { key: if vertical { rune::Key::Down } else { rune::Key::Right }, mods: rune::Mods::default() });
        rt.frame(1.0 / 60.0);
        assert_eq!(rt.state.selected, ["tab2"], "vertical: {vertical}: the arrow crosses the break");
        // And back again.
        rt.key(rune::KeyEvent { key: if vertical { rune::Key::Up } else { rune::Key::Left }, mods: rune::Mods::default() });
        rt.frame(1.0 / 60.0);
        assert_eq!(rt.state.selected, ["tab2", "tab1"], "vertical: {vertical}: and crosses it back");
    }
}
