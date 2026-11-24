//! Accessibility value actions on ranged controls, including degenerate and non-finite inputs.

use accesskit::{Action as Ak, ActionData, ActionRequest, TreeId};

use crate::{Cx, El, Env, Runtime, TextEdit, col, el, text_input};

struct Slider {
    value: f64,
    min: f64,
    max: f64,
    step: f64,
    steps: Vec<i32>,
}

impl Slider {
    fn new(value: f64, min: f64, max: f64, step: f64) -> Self {
        Self { value, min, max, step, steps: Vec::new() }
    }
}

fn slider_view(s: &Slider, _: &mut Cx<Slider>) -> El {
    col().size(200.0, 60.0).child(
        el().key("slider")
            .role("slider")
            .label("Amount")
            .size(160.0, 20.0)
            .range_value(s.value, s.min, s.max)
            .range_step(s.step)
            .on_step(|s: &mut Slider, d, _| {
                s.steps.push(d);
                if s.value.is_finite() && s.step.is_finite() {
                    s.value += f64::from(d) * s.step;
                }
            }),
    )
}

fn slider_rt(s: Slider) -> Runtime<Slider> {
    let mut rt = Runtime::new(s, slider_view, (200.0, 60.0), Env::new(true, 1.0));
    rt.frame(0.0);
    rt
}

fn slider_request(rt: &Runtime<Slider>, action: Ak, data: Option<ActionData>) -> ActionRequest {
    let tree = rt.accessibility_tree();
    let (id, _) = tree.nodes.iter().find(|(_, n)| n.author_id() == Some("slider")).expect("slider node");
    ActionRequest { action, target_tree: TreeId::ROOT, target_node: *id, data }
}

fn supports_set_value(rt: &Runtime<Slider>) -> bool {
    let tree = rt.accessibility_tree();
    let (_, node) = tree.nodes.iter().find(|(_, n)| n.author_id() == Some("slider")).expect("slider node");
    node.supports_action(Ak::SetValue)
}

#[test]
fn set_value_ignores_unusable_ranges_without_panicking() {
    let cases = [
        ("both bounds NaN", Slider::new(5.0, f64::NAN, f64::NAN, 1.0)),
        ("min NaN", Slider::new(5.0, f64::NAN, 10.0, 1.0)),
        ("max NaN", Slider::new(5.0, 0.0, f64::NAN, 1.0)),
        ("min -inf", Slider::new(5.0, f64::NEG_INFINITY, 10.0, 1.0)),
        ("max +inf", Slider::new(5.0, 0.0, f64::INFINITY, 1.0)),
        ("both bounds inf", Slider::new(5.0, f64::NEG_INFINITY, f64::INFINITY, 1.0)),
        ("both bounds +inf", Slider::new(5.0, f64::INFINITY, f64::INFINITY, 1.0)),
        ("min above max", Slider::new(5.0, 10.0, 0.0, 1.0)),
        ("value NaN", Slider::new(f64::NAN, 0.0, 10.0, 1.0)),
        ("value +inf", Slider::new(f64::INFINITY, 0.0, 10.0, 1.0)),
        ("step NaN", Slider::new(5.0, 0.0, 10.0, f64::NAN)),
        ("step +inf", Slider::new(5.0, 0.0, 10.0, f64::INFINITY)),
        ("step zero", Slider::new(5.0, 0.0, 10.0, 0.0)),
        ("step negative", Slider::new(5.0, 0.0, 10.0, -1.0)),
    ];

    for (label, slider) in cases {
        let mut rt = slider_rt(slider);
        for target in [7.0, 0.0, f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            let req = slider_request(&rt, Ak::SetValue, Some(ActionData::NumericValue(target)));
            assert!(!rt.a11y_action(&req), "{label} with target {target}: SetValue must do nothing");
            rt.frame(0.0);
        }
        assert!(!supports_set_value(&rt), "{label}: unusable range must not advertise SetValue");
        assert!(rt.state.steps.is_empty(), "{label}: on_step must not run");
    }
}

#[test]
fn set_value_ignores_non_finite_targets_on_valid_range() {
    let mut rt = slider_rt(Slider::new(5.0, 0.0, 10.0, 1.0));
    assert!(supports_set_value(&rt));
    for target in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
        let req = slider_request(&rt, Ak::SetValue, Some(ActionData::NumericValue(target)));
        assert!(!rt.a11y_action(&req), "target {target} must be ignored");
        rt.frame(0.0);
    }
    assert!(rt.state.steps.is_empty());
    assert_eq!(rt.state.value, 5.0);
}

#[test]
fn increment_and_decrement_still_step_when_range_bounds_are_nan() {
    let mut rt = slider_rt(Slider::new(5.0, f64::NAN, f64::NAN, 1.0));
    let inc = slider_request(&rt, Ak::Increment, None);
    assert!(rt.a11y_action(&inc));
    rt.frame(0.0);
    let dec = slider_request(&rt, Ak::Decrement, None);
    assert!(rt.a11y_action(&dec));
    rt.frame(0.0);
    assert_eq!(rt.state.steps, vec![1, -1]);
}

#[test]
fn set_value_preserves_valid_range_clamping_and_saturation() {
    let mut rt = slider_rt(Slider::new(5.0, 0.0, 10.0, 1.0));
    let req = slider_request(&rt, Ak::SetValue, Some(ActionData::NumericValue(7.2)));
    assert!(rt.a11y_action(&req));
    rt.frame(0.0);
    assert_eq!(rt.state.steps, vec![2]);
    assert_eq!(rt.state.value, 7.0);

    // Out-of-bounds finite targets still clamp to the range ends.
    let req = slider_request(&rt, Ak::SetValue, Some(ActionData::NumericValue(25.0)));
    assert!(rt.a11y_action(&req));
    rt.frame(0.0);
    assert_eq!(rt.state.steps, vec![2, 3]);
    assert_eq!(rt.state.value, 10.0);

    let req = slider_request(&rt, Ak::SetValue, Some(ActionData::NumericValue(-5.0)));
    assert!(rt.a11y_action(&req));
    rt.frame(0.0);
    assert_eq!(rt.state.steps, vec![2, 3, -10]);
    assert_eq!(rt.state.value, 0.0);

    // Same value queues nothing.
    let req = slider_request(&rt, Ak::SetValue, Some(ActionData::NumericValue(0.0)));
    assert!(!rt.a11y_action(&req));

    // A zero-span finite range is valid and does not panic.
    let mut flat = slider_rt(Slider::new(5.0, 5.0, 5.0, 1.0));
    assert!(supports_set_value(&flat));
    let req = slider_request(&flat, Ak::SetValue, Some(ActionData::NumericValue(8.0)));
    assert!(!flat.a11y_action(&req));

    // Step count overflow on a huge finite range saturates to i32::MAX without panicking.
    let mut huge = slider_rt(Slider::new(-1e308, -1e308, 1e308, 1.0));
    let req = slider_request(&huge, Ak::SetValue, Some(ActionData::NumericValue(1e308)));
    assert!(huge.a11y_action(&req));
    huge.frame(0.0);
    assert_eq!(huge.state.steps, vec![i32::MAX]);
}

#[test]
fn queued_set_value_requests_on_step_only_slider_do_not_add_up() {
    let mut rt = slider_rt(Slider::new(20.0, 0.0, 100.0, 10.0));
    let req = slider_request(&rt, Ak::SetValue, Some(ActionData::NumericValue(50.0)));
    assert!(rt.a11y_action(&req));
    assert!(rt.a11y_action(&req));
    rt.frame(0.0);
    assert_eq!(rt.state.steps, vec![3], "repeated SetValue before a frame must not stack steps");
    assert_eq!(rt.state.value, 50.0);

    // Setting back to the last committed value before a frame cancels an earlier pending step.
    let to_80 = slider_request(&rt, Ak::SetValue, Some(ActionData::NumericValue(80.0)));
    let to_50 = slider_request(&rt, Ak::SetValue, Some(ActionData::NumericValue(50.0)));
    assert!(rt.a11y_action(&to_80));
    assert!(rt.a11y_action(&to_50));
    rt.frame(0.0);
    assert_eq!(rt.state.steps, vec![3]);
    assert_eq!(rt.state.value, 50.0);
}

struct DirectSlider {
    value: f64,
    range: Option<(f64, f64)>,
    step: Option<f64>,
    with_on_step: bool,
    with_set_value: bool,
    seen: Vec<f64>,
    steps: Vec<i32>,
}

fn direct_view(s: &DirectSlider, _: &mut Cx<DirectSlider>) -> El {
    let mut item = el().key("slider").role("slider").label("Volume").size(160.0, 20.0);
    if let Some((min, max)) = s.range {
        item = item.range_value(s.value, min, max);
    }
    if let Some(step) = s.step {
        item = item.range_step(step);
    }
    if s.with_on_step {
        let step = s.step.unwrap_or(1.0);
        item = item.on_step(move |s: &mut DirectSlider, d, _| {
            s.steps.push(d);
            s.value += f64::from(d) * step;
        });
    }
    if s.with_set_value {
        item = item.on_set_value(|s: &mut DirectSlider, v, _| {
            s.seen.push(v);
            s.value = v;
        });
    }
    col().size(200.0, 60.0).child(item)
}

fn direct_rt(s: DirectSlider) -> Runtime<DirectSlider> {
    let mut rt = Runtime::new(s, direct_view, (200.0, 60.0), Env::new(true, 1.0));
    rt.frame(0.0);
    rt
}

fn direct_node_supports(rt: &Runtime<DirectSlider>, action: Ak) -> bool {
    let tree = rt.accessibility_tree();
    let (_, node) = tree.nodes.iter().find(|(_, n)| n.author_id() == Some("slider")).expect("slider node");
    node.supports_action(action)
}

#[test]
fn on_set_value_receives_exact_clamped_target_off_grid_and_across_queued_requests() {
    let mut rt = direct_rt(DirectSlider {
        value: 33.0,
        range: Some((0.0, 100.0)),
        step: Some(10.0),
        with_on_step: true,
        with_set_value: true,
        seen: Vec::new(),
        steps: Vec::new(),
    });
    assert!(direct_node_supports(&rt, Ak::SetValue));
    assert!(direct_node_supports(&rt, Ak::Increment));
    assert!(direct_node_supports(&rt, Ak::Decrement));

    // Off-grid 33.0 -> SetValue(50.0) delivers 50.0 directly instead of 2 steps (-> 53.0).
    assert!(rt.accessibility_action("slider", Ak::SetValue, Some(ActionData::NumericValue(50.0))));
    rt.frame(0.0);
    assert_eq!(rt.state.value, 50.0);
    assert_eq!(rt.state.seen, vec![50.0]);
    assert!(rt.state.steps.is_empty());

    // Two queued SetValue(50.0) before a frame land on 50.0, not 80.0.
    rt.update(|s, _| s.value = 20.0);
    rt.frame(0.0);
    assert!(rt.accessibility_action("slider", Ak::SetValue, Some(ActionData::NumericValue(50.0))));
    assert!(rt.accessibility_action("slider", Ak::SetValue, Some(ActionData::NumericValue(50.0))));
    rt.frame(0.0);
    assert_eq!(rt.state.value, 50.0);
    assert_eq!(rt.state.seen, vec![50.0, 50.0, 50.0]);

    // Queued SetValue(80.0) followed by SetValue(20.0) ends at 20.0.
    assert!(rt.accessibility_action("slider", Ak::SetValue, Some(ActionData::NumericValue(80.0))));
    assert!(rt.accessibility_action("slider", Ak::SetValue, Some(ActionData::NumericValue(20.0))));
    rt.frame(0.0);
    assert_eq!(rt.state.value, 20.0);

    // Out-of-bounds finite targets clamp to [min, max].
    assert!(rt.accessibility_action("slider", Ak::SetValue, Some(ActionData::NumericValue(250.0))));
    rt.frame(0.0);
    assert_eq!(rt.state.value, 100.0);
    assert!(rt.accessibility_action("slider", Ak::SetValue, Some(ActionData::NumericValue(-10.0))));
    rt.frame(0.0);
    assert_eq!(rt.state.value, 0.0);

    // Increment and Decrement still route to on_step.
    assert!(rt.accessibility_action("slider", Ak::Increment, None));
    rt.frame(0.0);
    assert_eq!(rt.state.steps, vec![1]);
    assert_eq!(rt.state.value, 10.0);
}

#[test]
fn on_set_value_works_without_range_step_and_without_range_value() {
    // Continuous slider: range_value only, no range_step, no on_step.
    let mut continuous = direct_rt(DirectSlider {
        value: 0.25,
        range: Some((0.0, 1.0)),
        step: None,
        with_on_step: false,
        with_set_value: true,
        seen: Vec::new(),
        steps: Vec::new(),
    });
    assert!(direct_node_supports(&continuous, Ak::SetValue));
    assert!(!direct_node_supports(&continuous, Ak::Increment));
    assert!(continuous.accessibility_action("slider", Ak::SetValue, Some(ActionData::NumericValue(0.37))));
    continuous.frame(0.0);
    assert_eq!(continuous.state.value, 0.37);
    assert_eq!(continuous.state.seen, vec![0.37]);

    // Unbounded control: on_set_value with no range_value passes finite targets unclamped.
    let mut unbounded = direct_rt(DirectSlider {
        value: 0.0,
        range: None,
        step: None,
        with_on_step: false,
        with_set_value: true,
        seen: Vec::new(),
        steps: Vec::new(),
    });
    assert!(direct_node_supports(&unbounded, Ak::SetValue));
    assert!(unbounded.accessibility_action("slider", Ak::SetValue, Some(ActionData::NumericValue(-42.5))));
    unbounded.frame(0.0);
    assert_eq!(unbounded.state.value, -42.5);
    for bad in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
        assert!(!unbounded.accessibility_action("slider", Ak::SetValue, Some(ActionData::NumericValue(bad))));
    }
    unbounded.frame(0.0);
    assert_eq!(unbounded.state.seen, vec![-42.5]);
}

#[test]
fn on_set_value_ignores_unusable_bounds_and_non_finite_targets_without_panicking() {
    let bad_ranges = [
        ("both NaN", (f64::NAN, f64::NAN)),
        ("min NaN", (f64::NAN, 10.0)),
        ("max NaN", (0.0, f64::NAN)),
        ("min -inf", (f64::NEG_INFINITY, 10.0)),
        ("max +inf", (0.0, f64::INFINITY)),
        ("min above max", (10.0, 0.0)),
    ];
    for (label, bounds) in bad_ranges {
        let mut rt = direct_rt(DirectSlider {
            value: 5.0,
            range: Some(bounds),
            step: None,
            with_on_step: false,
            with_set_value: true,
            seen: Vec::new(),
            steps: Vec::new(),
        });
        assert!(!direct_node_supports(&rt, Ak::SetValue), "{label}: unusable bounds must not advertise SetValue");
        for target in [5.0, f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            assert!(!rt.accessibility_action("slider", Ak::SetValue, Some(ActionData::NumericValue(target))), "{label} target {target}");
        }
        rt.frame(0.0);
        assert!(rt.state.seen.is_empty(), "{label}: on_set_value must not run");
    }

    // Valid bounds with non-finite targets are ignored without calling on_set_value.
    let mut valid = direct_rt(DirectSlider {
        value: 5.0,
        range: Some((0.0, 10.0)),
        step: None,
        with_on_step: false,
        with_set_value: true,
        seen: Vec::new(),
        steps: Vec::new(),
    });
    for target in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
        assert!(!valid.accessibility_action("slider", Ak::SetValue, Some(ActionData::NumericValue(target))));
    }
    valid.frame(0.0);
    assert!(valid.state.seen.is_empty());

    // Toggling on_set_value off invalidates the cached accessibility node's advertised actions.
    valid.a11y_activate("Slider");
    let first = valid.a11y_update().expect("initial tree");
    let (_, node) = first.nodes.iter().find(|(_, n)| n.author_id() == Some("slider")).expect("slider node");
    assert!(node.supports_action(Ak::SetValue));
    valid.update(|s, _| s.with_set_value = false);
    valid.frame(1.0 / 60.0);
    let delta = valid.a11y_update().expect("toggling on_set_value must emit an incremental update");
    let (_, updated) = delta.nodes.iter().find(|(_, n)| n.author_id() == Some("slider")).expect("updated slider node");
    assert!(!updated.supports_action(Ak::SetValue));
}

#[test]
fn spinbutton_with_on_set_value_handles_both_numeric_and_text_set_value() {
    struct Spin {
        text: TextEdit,
        value: f64,
        steps: Vec<i32>,
    }
    fn view(s: &Spin, _: &mut Cx<Spin>) -> El {
        col().size(200.0, 60.0).child(
            text_input(&s.text, crate::tok::type_::BODY)
                .key("spin")
                .single_line()
                .w(120.0)
                .role("spinbutton")
                .label("Font size")
                .on_edit(|s: &mut Spin| &mut s.text)
                .range_value(s.value, 1.0, 64.0)
                .range_step(1.0)
                .on_step(|s: &mut Spin, d, _| {
                    s.steps.push(d);
                    s.value = (s.value + f64::from(d)).clamp(1.0, 64.0);
                    s.text = TextEdit::new(&s.value.to_string());
                })
                .on_set_value(|s: &mut Spin, v, _| {
                    s.value = v;
                    s.text = TextEdit::new(&v.to_string());
                }),
        )
    }
    let mut rt = Runtime::new(Spin { text: TextEdit::new("10.5"), value: 10.5, steps: Vec::new() }, view, (200.0, 60.0), Env::new(true, 1.0));
    rt.frame(0.0);
    assert!(rt.accessibility_action("spin", Ak::SetValue, Some(ActionData::NumericValue(14.0))));
    rt.frame(0.0);
    assert_eq!(rt.state.value, 14.0);
    assert_eq!(rt.state.text.text, "14");
    assert!(rt.state.steps.is_empty());

    assert!(rt.accessibility_action("spin", Ak::SetValue, Some(ActionData::Value("24".into()))));
    rt.frame(0.0);
    assert_eq!(rt.state.text.text, "24");
}
