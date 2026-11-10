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
