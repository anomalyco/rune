use rune::accesskit::{Action, ActionData};
use rune::{Cx, Direction, El, Env, Key, KeyEvent, Mods, Runtime, col};

use super::*;

struct Harness {
    value: f32,
    range: RangeInclusive<f32>,
    step: f32,
    disabled: bool,
    rtl: bool,
    reduced_motion: bool,
    hide_label: bool,
    value_text: Option<String>,
    size: ControlSize,
    width: f32,
    events: Vec<(f32, bool)>,
    tx_calls: u32,
}

impl Default for Harness {
    fn default() -> Self {
        Harness {
            value: 0.5,
            range: 0.0..=1.0,
            step: 0.0,
            disabled: false,
            rtl: false,
            reduced_motion: false,
            hide_label: false,
            value_text: None,
            size: ControlSize::Regular,
            width: 180.0,
            events: Vec::new(),
            tx_calls: 0,
        }
    }
}

fn view(s: &Harness, cx: &mut Cx<Harness>) -> El {
    cx.reduced_motion(s.reduced_motion);
    let k = Kit::neutral(true);
    let mut slider = Slider::new("slider", s.value)
        .range(s.range.clone())
        .step(s.step)
        .label("Volume")
        .control_size(s.size)
        .width(s.width)
        .disabled(s.disabled)
        .on_change_tx(|s: &mut Harness, v, done, tx| {
            s.value = v;
            s.events.push((v, done));
            s.tx_calls += 1;
            let _ = tx;
        });
    if s.hide_label {
        slider = slider.hide_label();
    }
    if let Some(vt) = &s.value_text {
        slider = slider.value_text(vt.clone());
    }
    let mut root = col().padding(20.0).child(slider.build(&k));
    if s.rtl {
        root = root.dir(Direction::Rtl);
    }
    root
}

fn mount(s: Harness) -> Runtime<Harness> {
    let mut rt = Runtime::new(s, view, (360.0, 140.0), Env::new(true, 1.0));
    rt.frame(0.0);
    rt.settle_all();
    rt.frame(1.0 / 60.0);
    rt
}

fn rect_of(rt: &Runtime<Harness>, key: &str) -> [f32; 4] {
    rt.elements()
        .into_iter()
        .find(|e| e.key.as_deref() == Some(key))
        .unwrap_or_else(|| panic!("missing element keyed {key:?}"))
        .rect
}

fn press_key(rt: &mut Runtime<Harness>, key: Key) {
    rt.key(KeyEvent { key, mods: Mods::default() });
    rt.frame(1.0 / 60.0);
}

#[test]
fn track_click_jumps_and_reports_done_on_release() {
    let mut rt = mount(Harness { value: 0.2, range: 0.0..=100.0, ..Default::default() });
    let r = rect_of(&rt, "slider");
    let y = r[1] + r[3] * 0.5;
    // Click near the right side of the track (away from the knob at 0.2).
    let target_x = r[0] + r[2] - 9.0;
    rt.pointer_down(target_x, y);
    rt.frame(1.0 / 60.0);
    assert!((rt.state.value - 100.0).abs() < 0.5, "down jumped to max: {}", rt.state.value);
    assert_eq!(rt.state.events.len(), 1);
    assert!(!rt.state.events[0].1, "down is not done yet");

    rt.pointer_up(target_x, y);
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.state.events.len(), 2);
    assert!(rt.state.events[1].1, "release commits with done=true");
    assert!((rt.state.events[1].0 - 100.0).abs() < 0.5);
}

#[test]
fn knob_drag_preserves_grab_offset_and_reports_live_vs_done() {
    let mut rt = mount(Harness { value: 0.5, ..Default::default() });
    let r = rect_of(&rt, "slider");
    let k = rect_of(&rt, "knob");
    let y = r[1] + r[3] * 0.5;
    // Press 5 pt right of knob center: must not jump on Down.
    let grab_x = k[0] + k[2] * 0.5 + 5.0;
    rt.pointer_down(grab_x, y);
    rt.frame(1.0 / 60.0);
    assert!(rt.state.events.is_empty(), "pressing the knob must not jump");
    assert_eq!(rt.state.value, 0.5);

    // Drag right by half the travel (81 pt): value should move to 1.0.
    let travel = r[2] - k[2];
    rt.pointer_move(grab_x + travel * 0.25, y);
    rt.frame(1.0 / 60.0);
    assert!((rt.state.value - 0.75).abs() < 0.01, "drag with grab offset: {}", rt.state.value);
    assert_eq!(rt.state.events.last().map(|e| e.1), Some(false), "move reports done=false");

    // Drag past the right bound clamps to max.
    rt.pointer_move(r[0] + r[2] + 80.0, y);
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.state.value, 1.0);
    assert_eq!(rt.state.events.last(), Some(&(1.0, false)));

    rt.pointer_up(r[0] + r[2] + 80.0, y);
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.state.value, 1.0);
    assert_eq!(rt.state.events.last(), Some(&(1.0, true)));
}

#[test]
fn escape_and_cancel_pointer_restore_pre_drag_value() {
    // Escape after track-press jump + move restores pre-drag value.
    let mut rt = mount(Harness { value: 0.5, ..Default::default() });
    let r = rect_of(&rt, "slider");
    let y = r[1] + r[3] * 0.5;
    rt.pointer_down(r[0] + r[2] - 20.0, y);
    rt.pointer_move(r[0] + r[2] - 10.0, y);
    rt.frame(1.0 / 60.0);
    assert!(rt.state.value > 0.8);
    press_key(&mut rt, Key::Escape);
    assert_eq!(rt.state.value, 0.5, "Escape must restore pre-drag value");
    assert_eq!(rt.state.events.last(), Some(&(0.5, true)));
    rt.pointer_up(r[0] + r[2] - 10.0, y);
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.state.value, 0.5, "release after cancel must keep restored value");

    // Host/window pointer cancellation restores pre-drag value too.
    rt.pointer_down(r[0] + 15.0, y);
    rt.pointer_move(r[0] + 10.0, y);
    rt.frame(1.0 / 60.0);
    assert!(rt.state.value < 0.2);
    rt.cancel_pointer();
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.state.value, 0.5, "cancel_pointer must restore pre-drag value");
}

#[test]
fn keyboard_stepping_page_and_home_end_work_for_stepped_and_continuous() {
    // Stepped slider: 160..=420 step 20.
    let mut rt = mount(Harness { value: 240.0, range: 160.0..=420.0, step: 20.0, ..Default::default() });
    rt.update(|_, tx| tx.focus("slider", true));
    rt.frame(0.0);

    press_key(&mut rt, Key::Right);
    assert_eq!(rt.state.value, 260.0);
    assert_eq!(rt.state.events.last(), Some(&(260.0, true)));

    press_key(&mut rt, Key::Up);
    assert_eq!(rt.state.value, 280.0);

    press_key(&mut rt, Key::Left);
    assert_eq!(rt.state.value, 260.0);

    press_key(&mut rt, Key::Down);
    assert_eq!(rt.state.value, 240.0);

    // PageUp steps +10 steps (+200 -> clamped at 420).
    press_key(&mut rt, Key::PageUp);
    assert_eq!(rt.state.value, 420.0);

    // PageDown steps -10 steps (-200 -> 220).
    press_key(&mut rt, Key::PageDown);
    assert_eq!(rt.state.value, 220.0);

    press_key(&mut rt, Key::Home);
    assert_eq!(rt.state.value, 160.0);

    press_key(&mut rt, Key::End);
    assert_eq!(rt.state.value, 420.0);

    // Continuous slider (step = 0.0): keyboard steps by 1/20th of range (0.05 on 0..=1).
    let mut cont = mount(Harness { value: 0.5, range: 0.0..=1.0, step: 0.0, ..Default::default() });
    cont.update(|_, tx| tx.focus("slider", true));
    cont.frame(0.0);
    press_key(&mut cont, Key::Right);
    assert!((cont.state.value - 0.55).abs() < 1e-5, "1/20th step: {}", cont.state.value);
    press_key(&mut cont, Key::PageDown);
    assert!((cont.state.value - 0.05).abs() < 1e-5, "PageDown -10/20ths: {}", cont.state.value);
    press_key(&mut cont, Key::Home);
    assert_eq!(cont.state.value, 0.0);
    press_key(&mut cont, Key::End);
    assert_eq!(cont.state.value, 1.0);
}

#[test]
fn accesskit_set_value_increment_and_decrement_update_value() {
    // Continuous slider takes exact SetValue and steps by 1/20th.
    let mut rt = mount(Harness { value: 0.5, range: 0.0..=100.0, step: 0.0, value_text: Some("50%".into()), ..Default::default() });
    let tree = rt.accessibility_json();
    let node = find_a11y(&tree, "slider").expect("slider a11y node");
    assert_eq!(node["role"], "slider");
    assert_eq!(node["name"], "Volume");
    assert_eq!(node["value"], "50%");
    assert_eq!(node["range"], serde_json::json!([0.5, 0.0, 100.0, 5.0]));
    assert!(node["children"].is_null(), "decorative children must stay hidden: {node}");

    assert!(rt.accessibility_action("slider", Action::SetValue, Some(ActionData::NumericValue(37.25))));
    rt.frame(1.0 / 60.0);
    assert!((rt.state.value - 37.25).abs() < 1e-4);
    assert_eq!(rt.state.events.last().map(|e| e.1), Some(true));

    assert!(rt.accessibility_action("slider", Action::Increment, None));
    rt.frame(1.0 / 60.0);
    assert!((rt.state.value - 42.25).abs() < 1e-4);

    assert!(rt.accessibility_action("slider", Action::Decrement, None));
    rt.frame(1.0 / 60.0);
    assert!((rt.state.value - 37.25).abs() < 1e-4);

    // Stepped slider snaps SetValue to its step grid and clamps out-of-range targets.
    let mut stepped = mount(Harness { value: 20.0, range: 0.0..=100.0, step: 10.0, ..Default::default() });
    assert!(stepped.accessibility_action("slider", Action::SetValue, Some(ActionData::NumericValue(46.0))));
    stepped.frame(1.0 / 60.0);
    assert_eq!(stepped.state.value, 50.0);
    assert!(stepped.accessibility_action("slider", Action::SetValue, Some(ActionData::NumericValue(999.0))));
    stepped.frame(1.0 / 60.0);
    assert_eq!(stepped.state.value, 100.0);
    assert!(!stepped.accessibility_action("slider", Action::SetValue, Some(ActionData::NumericValue(f64::NAN))));
    stepped.frame(1.0 / 60.0);
    assert_eq!(stepped.state.value, 100.0);
}

#[test]
fn disabled_slider_ignores_pointer_keyboard_and_assistive_actions() {
    let mut rt = mount(Harness { value: 0.4, disabled: true, ..Default::default() });
    let tree = rt.accessibility_json();
    let node = find_a11y(&tree, "slider").expect("disabled slider node");
    let states: Vec<_> = node["states"].as_array().unwrap().iter().filter_map(|v| v.as_str()).collect();
    assert!(states.contains(&"disabled"), "expected disabled state: {node}");
    assert!(node["actions"].as_array().is_none_or(|a| a.is_empty()), "disabled slider exposes no actions: {node}");

    let r = rect_of(&rt, "slider");
    let y = r[1] + r[3] * 0.5;
    rt.pointer_down(r[0] + r[2] - 10.0, y);
    rt.pointer_move(r[0] + 10.0, y);
    rt.pointer_up(r[0] + 10.0, y);
    rt.frame(1.0 / 60.0);
    rt.update(|_, tx| tx.focus("slider", true));
    rt.frame(0.0);
    press_key(&mut rt, Key::Right);
    press_key(&mut rt, Key::End);
    assert!(!rt.accessibility_action("slider", Action::Increment, None));
    assert!(!rt.accessibility_action("slider", Action::SetValue, Some(ActionData::NumericValue(0.9))));
    rt.frame(1.0 / 60.0);

    assert_eq!(rt.state.value, 0.4);
    assert!(rt.state.events.is_empty());
}

#[test]
fn rtl_slider_mirrors_visuals_pointer_drag_and_keyboard_arrows() {
    let mut rt = mount(Harness { value: 0.0, range: 0.0..=100.0, step: 10.0, rtl: true, ..Default::default() });
    let r = rect_of(&rt, "slider");
    let k0 = rect_of(&rt, "knob");
    // At value == min in RTL, the knob sits at the right edge.
    assert!(((k0[0] + k0[2]) - (r[0] + r[2])).abs() < 1.0, "RTL min knob sits at right: knob={k0:?} slider={r:?}");

    // Focus and press Left: in RTL, Left raises the value.
    rt.update(|_, tx| tx.focus("slider", true));
    rt.frame(0.0);
    press_key(&mut rt, Key::Left);
    assert_eq!(rt.state.value, 10.0, "RTL Left raises value");
    press_key(&mut rt, Key::Right);
    assert_eq!(rt.state.value, 0.0, "RTL Right lowers value");

    // Click near the left edge of the track: in RTL, left is max (100).
    let y = r[1] + r[3] * 0.5;
    rt.pointer_down(r[0] + 9.0, y);
    rt.pointer_up(r[0] + 9.0, y);
    rt.frame(1.0 / 60.0);
    rt.settle_all();
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.state.value, 100.0, "RTL left edge maps to max");
    let k_max = rect_of(&rt, "knob");
    assert!((k_max[0] - r[0]).abs() < 1.0, "RTL max knob sits at left: knob={k_max:?} slider={r:?}");

    // Drag rightward from max lowers value; Escape cancels back to 100.
    rt.pointer_down(r[0] + 9.0, y);
    rt.pointer_move(r[0] + r[2] - 9.0, y);
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.state.value, 0.0, "dragging right in RTL lowers to min");
    press_key(&mut rt, Key::Escape);
    assert_eq!(rt.state.value, 100.0, "Escape in RTL restores pre-drag value");
}

#[test]
fn edge_ranges_non_finite_values_reduced_motion_and_sizes_stay_safe() {
    // Zero-span range never panics or emits NaN.
    let mut flat = mount(Harness { value: 5.0, range: 5.0..=5.0, ..Default::default() });
    let r = rect_of(&flat, "slider");
    flat.pointer_down(r[0] + r[2] - 5.0, r[1] + r[3] * 0.5);
    flat.pointer_move(r[0] + 5.0, r[1] + r[3] * 0.5);
    flat.pointer_up(r[0] + 5.0, r[1] + r[3] * 0.5);
    flat.frame(1.0 / 60.0);
    assert_eq!(flat.state.value, 5.0);
    flat.update(|_, tx| tx.focus("slider", true));
    flat.frame(0.0);
    press_key(&mut flat, Key::Right);
    assert_eq!(flat.state.value, 5.0);

    // Non-finite value and huge finite range stay finite.
    let mut nan_rt = mount(Harness { value: f32::NAN, range: 10.0..=20.0, ..Default::default() });
    assert!(rect_of(&nan_rt, "knob")[0].is_finite());
    nan_rt.update(|_, tx| tx.focus("slider", true));
    nan_rt.frame(0.0);
    press_key(&mut nan_rt, Key::Right);
    assert!(nan_rt.state.value.is_finite() && nan_rt.state.value > 10.0);

    let mut huge = mount(Harness { value: 0.0, range: -1e30..=1e30, ..Default::default() });
    huge.update(|_, tx| tx.focus("slider", true));
    huge.frame(0.0);
    press_key(&mut huge, Key::Right);
    assert!(huge.state.value.is_finite() && huge.state.value > 0.0);

    // Reduced motion lands track clicks and keyboard steps in one frame and idles when settled.
    let mut rm = mount(Harness { value: 0.0, reduced_motion: true, ..Default::default() });
    let r = rect_of(&rm, "slider");
    rm.pointer_down(r[0] + r[2] - 9.0, r[1] + r[3] * 0.5);
    rm.pointer_up(r[0] + r[2] - 9.0, r[1] + r[3] * 0.5);
    rm.frame(1.0 / 60.0);
    let k_after = rm.elements().into_iter().find(|e| e.key.as_deref() == Some("knob")).unwrap();
    assert!(((k_after.rect[0] + k_after.rect[2]) - (r[0] + r[2])).abs() < 1.0, "reduced motion lands at end in one frame");
    assert!(!k_after.moving, "knob position must not animate under reduced motion");
    rm.pointer_leave();
    for _ in 0..30 { rm.frame(1.0 / 60.0); }
    assert!(!rm.frame(1.0 / 60.0), "settled slider schedules no idle frames");

    // ControlSize scales hit height cleanly.
    for (size, expected_h) in [
        (ControlSize::Mini, 16.0),
        (ControlSize::Small, 20.0),
        (ControlSize::Regular, 24.0),
        (ControlSize::Large, 28.0),
        (ControlSize::ExtraLarge, 36.0),
    ] {
        let rt = mount(Harness { size, hide_label: true, ..Default::default() });
        assert_eq!(rect_of(&rt, "slider")[3], expected_h, "size {size:?}");
    }
}

#[test]
fn multiple_steps_in_one_frame_accumulate_and_forced_states_render() {
    let mut rt = mount(Harness { value: 20.0, range: 0.0..=100.0, step: 10.0, ..Default::default() });
    rt.update(|_, tx| tx.focus("slider", true));
    rt.frame(0.0);
    rt.key(KeyEvent { key: Key::Right, mods: Mods::default() });
    rt.key(KeyEvent { key: Key::Right, mods: Mods::default() });
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.state.value, 40.0, "two steps in one frame both apply");
    assert_eq!(rt.state.tx_calls, 2);

    fn states_view(_: &(), _: &mut Cx<()>) -> El {
        let k = Kit::neutral(true);
        col()
            .gap(8.0)
            .child(Slider::new("s-rest", 0.5).label("Rest").state(State::Rest).build(&k))
            .child(Slider::new("s-hover", 0.5).label("Hover").state(State::Hover).build(&k))
            .child(Slider::new("s-pressed", 0.5).label("Pressed").state(State::Pressed).build(&k))
            .child(Slider::new("s-focus", 0.5).label("Focus").state(State::Focus).build(&k))
            .child(Slider::new("s-disabled", 0.5).label("Disabled").state(State::Disabled).build(&k))
    }
    let mut st = Runtime::new((), states_view, (300.0, 260.0), Env::new(true, 1.0));
    assert!(!st.frame(0.0), "static sliders schedule no idle animation");
}

fn find_a11y<'a>(v: &'a serde_json::Value, key: &str) -> Option<&'a serde_json::Value> {
    if v["key"] == key {
        return Some(v);
    }
    v["children"].as_array()?.iter().find_map(|c| find_a11y(c, key))
}
