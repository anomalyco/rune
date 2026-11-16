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


/// A strip that selects and closes, `n` tabs in `width` points, `active` selected.
#[derive(Default)]
struct Picker {
    n: usize,
    width: f32,
    vertical: bool,
    active: usize,
    drag: TabDragState,
    selected: Vec<String>,
    closed: Vec<String>,
}

fn picker_view(s: &Picker, _: &mut rune::Cx<Picker>) -> El {
    let k = Kit::neutral(true);
    let orientation = if s.vertical { TabOrientation::Vertical } else { TabOrientation::Horizontal };
    let ids: Vec<String> = (0..s.n).map(|i| format!("tab{i}")).collect();
    let strip = TabStrip::new("strip", orientation)
        .tabs(ids.iter().map(|id| TabItem::new(id.clone(), format!("Tab {id}"))))
        .active(ids.get(s.active).map(String::as_str))
        .viewport_width((!s.vertical).then_some(s.width))
        .drag_state(&s.drag, |s: &mut Picker| &mut s.drag)
        .on_select(|s: &mut Picker, id, _| s.selected.push(id.to_owned()))
        .on_close(|s: &mut Picker, id, _| s.closed.push(id.to_owned()))
        .build(&k);
    col().size(s.width, 300.0).child(strip)
}

fn picker(n: usize, width: f32, vertical: bool) -> rune::Runtime<Picker> {
    let mut rt = rune::Runtime::new(Picker { n, width, vertical, ..Picker::default() }, picker_view, (width, 300.0), rune::Env::new(true, 1.0));
    rt.frame(0.0);
    rt.frame(1.0 / 60.0);
    rt
}

fn centre(rt: &rune::Runtime<Picker>, key: &str) -> (f32, f32) {
    let r = rt.elements().into_iter().find(|e| e.key.as_deref() == Some(key)).expect("the tab").rect;
    (r[0] + r[2] / 2.0, r[1] + r[3] / 2.0)
}

#[test]
fn a_press_selects_the_tab_before_the_release_in_either_orientation() {
    for vertical in [false, true] {
        let mut rt = picker(4, 600.0, vertical);
        let (x, y) = centre(&rt, "t-tab2");
        rt.pointer_move(x, y);
        rt.pointer_down(x, y);
        rt.frame(1.0 / 60.0);
        assert_eq!(rt.state.selected, ["tab2"], "vertical: {vertical}");
        rt.pointer_up(x, y);
        rt.frame(1.0 / 60.0);
        assert_eq!(rt.state.selected, ["tab2"], "the release selects nothing more; vertical: {vertical}");
    }
}

#[test]
fn a_press_on_the_selected_tab_selects_nothing() {
    let mut rt = picker(4, 600.0, false);
    let (x, y) = centre(&rt, "t-tab0");
    rt.pointer_move(x, y);
    rt.pointer_down(x, y);
    rt.pointer_up(x, y);
    rt.frame(1.0 / 60.0);
    assert!(rt.state.selected.is_empty());
}

#[test]
fn a_press_on_a_squeezed_tab_selects_it_and_a_middle_click_closes_it() {
    // 30 tabs in 600 points: each at its 28 point least, its icon alone.
    let mut rt = picker(30, 600.0, false);
    let (x, y) = centre(&rt, "t-tab3");
    rt.pointer_move(x, y);
    rt.frame(1.0 / 60.0);
    rt.pointer_down(x, y);
    rt.pointer_up(x, y);
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.state.selected, ["tab3"]);
    assert!(rt.state.closed.is_empty(), "an inactive squeezed tab has no close button under its centre");
    let (x, y) = centre(&rt, "t-tab5");
    rt.pointer_move(x, y);
    rt.button_down(x, y, rune::PointerButton::Middle);
    rt.button_up(x, y, rune::PointerButton::Middle);
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.state.closed, ["tab5"]);
    assert_eq!(rt.state.selected, ["tab3"], "a middle click selects nothing");
}