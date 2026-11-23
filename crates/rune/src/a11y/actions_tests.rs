//! Accessibility value actions on ranged controls, including degenerate and non-finite inputs.

use accesskit::{Action as Ak, ActionData, ActionRequest, TreeId};

use crate::{Cx, El, Env, Runtime, col, el};

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
