//! End-to-end headless tests for `rune_kit::NumberField`: integer/decimal entry, optional
//! empty vs zero, editable intermediate text, Enter/blur clamp and revert, Up/Down/Page
//! stepping while Left/Right move the caret, optional stepper buttons, read-only/disabled,
//! prefix/suffix, and spinbutton accessibility.

use rune::accesskit::{Action, ActionData, Role};
use rune::{Cx, El, Env, Key, KeyEvent, Mods, Runtime, col, el};
use rune_kit::{Field, FieldMetrics, Kit, NumberField, NumberFieldState};

struct Form {
    interval: NumberFieldState,
    threshold: NumberFieldState,
    fixed: NumberFieldState,
    read_only_field: NumberFieldState,
    disabled_field: NumberFieldState,
    interval_changes: Vec<Option<f64>>,
    interval_commits: Vec<Option<f64>>,
    threshold_changes: Vec<Option<f64>>,
    threshold_commits: Vec<Option<f64>>,
    focus_events: Vec<bool>,
    custom_metrics: bool,
}

impl Default for Form {
    fn default() -> Self {
        Self {
            interval: NumberFieldState::new(Some(15.0)),
            threshold: NumberFieldState::empty(),
            fixed: NumberFieldState::new(Some(1.5)),
            read_only_field: NumberFieldState::new(Some(42.0)),
            disabled_field: NumberFieldState::new(Some(7.0)),
            interval_changes: Vec::new(),
            interval_commits: Vec::new(),
            threshold_changes: Vec::new(),
            threshold_commits: Vec::new(),
            focus_events: Vec::new(),
            custom_metrics: false,
        }
    }
}

fn view(s: &Form, _: &mut Cx<Form>) -> El {
    let mut k = Kit::neutral(true);
    if s.custom_metrics {
        k.metrics.field = Some(FieldMetrics { height: 34.0, ..FieldMetrics::kit(&k) });
    }
    col()
        .w_full()
        .h_full()
        .padding(12.0)
        .gap(10.0)
        .child(
            Field::new()
                .label("Interval")
                .control(
                    NumberField::new(&s.interval)
                        .label("Interval")
                        .range(10.0..=60.0)
                        .step(1.0)
                        .page_step(5.0)
                        .integer()
                        .allow_empty(false)
                        .suffix("s")
                        .stepper(true)
                        .width(None)
                        .on_edit(|s: &mut Form| &mut s.interval)
                        .on_change(|s: &mut Form, v| s.interval_changes.push(v))
                        .on_commit(|s: &mut Form, v| s.interval_commits.push(v))
                        .on_focus_change(|s: &mut Form, f, _| s.focus_events.push(f))
                        .build(&k)
                        .key("interval"),
                )
                .build(&k)
                .key("interval-field"),
        )
        .child(
            NumberField::new(&s.threshold)
                .label("Threshold")
                .placeholder("Auto")
                .step(0.25)
                .decimals(2)
                .prefix("$")
                .stepper(true)
                .on_edit(|s: &mut Form| &mut s.threshold)
                .on_change(|s: &mut Form, v| s.threshold_changes.push(v))
                .on_commit(|s: &mut Form, v| s.threshold_commits.push(v))
                .build(&k)
                .key("threshold"),
        )
        .child(
            NumberField::new(&s.fixed)
                .label("Fixed")
                .decimals(2)
                .fixed_decimals(true)
                .step(0.5)
                .on_edit(|s: &mut Form| &mut s.fixed)
                .build(&k)
                .key("fixed"),
        )
        .child(
            NumberField::new(&s.read_only_field)
                .label("Read-only")
                .range(0.0..=100.0)
                .stepper(true)
                .read_only(true)
                .on_edit(|s: &mut Form| &mut s.read_only_field)
                .build(&k)
                .key("ro"),
        )
        .child(
            NumberField::new(&s.disabled_field)
                .label("Disabled")
                .range(0.0..=100.0)
                .stepper(true)
                .disabled(true)
                .on_edit(|s: &mut Form| &mut s.disabled_field)
                .build(&k)
                .key("dis"),
        )
        .child(el().key("other").size(80.0, 24.0).focusable(true))
}

fn runtime() -> Runtime<Form> {
    let mut rt = Runtime::new(Form::default(), view, (420.0, 360.0), Env::new(true, 1.0));
    rt.frame(0.0);
    rt
}

fn press(rt: &mut Runtime<Form>, key: Key) {
    rt.key(KeyEvent { key, mods: Mods::default() });
    rt.frame(0.0);
}

fn type_str(rt: &mut Runtime<Form>, text: &str) {
    for ch in text.chars() {
        press(rt, Key::Text(ch.to_string()));
    }
}

fn click_path(rt: &mut Runtime<Form>, suffix: &str) {
    let el = rt
        .elements()
        .into_iter()
        .find(|e| e.path.ends_with(suffix))
        .unwrap_or_else(|| panic!("missing element ending with {suffix}"));
    let (x, y) = (el.rect[0] + el.rect[2] * 0.5, el.rect[1] + el.rect[3] * 0.5);
    rt.pointer_down(x, y);
    rt.pointer_up(x, y);
    rt.frame(1.0 / 60.0);
}

fn focus_field(rt: &mut Runtime<Form>, pick: impl FnOnce(&mut Form) -> &mut NumberFieldState) {
    rt.update(|s, _| {
        let st = pick(s);
        st.edit_mut().focused = true;
        let len = st.text().len();
        st.edit_mut().set_caret(len);
    });
    rt.frame(0.0);
}

#[test]
fn optional_decimal_field_supports_intermediate_text_and_empty_vs_zero() {
    let mut rt = runtime();
    assert_eq!(rt.state.threshold.value(), None);
    assert!(rt.state.threshold.is_empty());

    focus_field(&mut rt, |s| &mut s.threshold);

    // Intermediate "-" stays editable without producing a bogus number or invalid ring.
    type_str(&mut rt, "-");
    assert_eq!(rt.state.threshold.text(), "-");
    assert_eq!(rt.state.threshold.value(), None);
    assert!(!rt.state.threshold.is_invalid());
    assert!(rt.state.threshold_changes.is_empty());

    // Completing "-1" emits Some(-1.0); adding "." keeps "-1." editable without clobbering.
    type_str(&mut rt, "1");
    assert_eq!(rt.state.threshold.value(), Some(-1.0));
    assert_eq!(rt.state.threshold_changes, [Some(-1.0)]);
    type_str(&mut rt, ".");
    assert_eq!(rt.state.threshold.text(), "-1.");
    assert_eq!(rt.state.threshold.value(), Some(-1.0));
    assert!(!rt.state.threshold.is_invalid());
    assert_eq!(rt.state.threshold_changes, [Some(-1.0)]);

    // Finishing "-1.5" delivers the decimal value.
    type_str(&mut rt, "5");
    assert_eq!(rt.state.threshold.value(), Some(-1.5));
    assert_eq!(rt.state.threshold_changes, [Some(-1.0), Some(-1.5)]);

    // Clearing to empty emits None (distinct from 0).
    rt.update(|s, _| {
        let len = s.threshold.text().len();
        s.threshold.edit_mut().set_selection(0, len);
    });
    rt.frame(0.0);
    press(&mut rt, Key::Backspace);
    assert_eq!(rt.state.threshold.text(), "");
    assert_eq!(rt.state.threshold.value(), None);
    assert!(rt.state.threshold.is_empty());
    assert_eq!(rt.state.threshold_changes.last(), Some(&None));

    // Typing "0" produces Some(0.0), distinct from None.
    type_str(&mut rt, "0");
    assert_eq!(rt.state.threshold.value(), Some(0.0));
    assert!(!rt.state.threshold.is_empty());
    assert_eq!(rt.state.threshold_changes.last(), Some(&Some(0.0)));
}

#[test]
fn bounded_integer_allows_in_progress_out_of_range_typing_and_clamps_on_enter_and_blur() {
    let mut rt = runtime();
    focus_field(&mut rt, |s| &mut s.interval);

    // Select all and type "2" on the way to "25" (min is 10): must not clamp mid-typing.
    rt.update(|s, _| s.interval.edit_mut().set_selection(0, 2));
    rt.frame(0.0);
    type_str(&mut rt, "2");
    assert_eq!(rt.state.interval.text(), "2");
    assert_eq!(rt.state.interval.value(), Some(15.0), "out-of-range draft holds last valid value");
    assert!(rt.state.interval.is_invalid());

    type_str(&mut rt, "5");
    assert_eq!(rt.state.interval.text(), "25");
    assert_eq!(rt.state.interval.value(), Some(25.0));
    assert!(!rt.state.interval.is_invalid());
    assert_eq!(rt.state.interval_changes, [Some(25.0)]);

    // Typing "999" and pressing Enter clamps to max 60 and formats text.
    rt.update(|s, _| s.interval.edit_mut().set_selection(0, 2));
    rt.frame(0.0);
    type_str(&mut rt, "999");
    assert_eq!(rt.state.interval.text(), "999");
    assert!(rt.state.interval.is_invalid());
    press(&mut rt, Key::Enter);
    assert_eq!((rt.state.interval.text(), rt.state.interval.as_i64(), rt.state.interval.is_invalid()), ("60", Some(60), false));
    assert_eq!(rt.state.interval_commits.last(), Some(&Some(60.0)));

    // Typing "3.7" in integer mode and blurring rounds and clamps to 10.
    rt.update(|s, _| s.interval.edit_mut().set_selection(0, 2));
    rt.frame(0.0);
    type_str(&mut rt, "3.7");
    assert!(rt.state.interval.is_invalid());
    rt.update(|_, tx| tx.focus("other", false));
    rt.frame(0.0);
    assert_eq!((rt.state.interval.text(), rt.state.interval.as_i64(), rt.state.interval.is_invalid()), ("10", Some(10), false));
    assert_eq!(rt.state.interval_commits.last(), Some(&Some(10.0)));

    // Typing garbage on a required field reverts on blur; clearing reverts to last valid value.
    focus_field(&mut rt, |s| &mut s.interval);
    rt.update(|s, _| s.interval.edit_mut().set_selection(0, 2));
    rt.frame(0.0);
    type_str(&mut rt, "abc");
    assert!(rt.state.interval.is_invalid());
    press(&mut rt, Key::Enter);
    assert_eq!((rt.state.interval.text(), rt.state.interval.as_i64(), rt.state.interval.is_invalid()), ("10", Some(10), false));

    rt.update(|s, _| s.interval.edit_mut().set_selection(0, 2));
    rt.frame(0.0);
    press(&mut rt, Key::Backspace);
    assert_eq!(rt.state.interval.text(), "");
    assert!(rt.state.interval.is_invalid());
    rt.update(|_, tx| tx.focus("other", false));
    rt.frame(0.0);
    assert_eq!((rt.state.interval.text(), rt.state.interval.as_i64()), ("10", Some(10)));
}

#[test]
fn up_down_page_step_while_left_right_move_caret() {
    let mut rt = runtime();
    focus_field(&mut rt, |s| &mut s.interval);
    assert_eq!((rt.state.interval.text(), rt.state.interval.edit().focus), ("15", 2));

    // Left and Right move the caret instead of stepping.
    press(&mut rt, Key::Left);
    assert_eq!((rt.state.interval.edit().focus, rt.state.interval.as_i64()), (1, Some(15)));
    press(&mut rt, Key::Right);
    assert_eq!((rt.state.interval.edit().focus, rt.state.interval.as_i64()), (2, Some(15)));

    // Up/Down step by 1, PageUp/PageDown step by custom page_step (5).
    press(&mut rt, Key::Up);
    assert_eq!((rt.state.interval.text(), rt.state.interval.as_i64()), ("16", Some(16)));
    press(&mut rt, Key::Down);
    assert_eq!((rt.state.interval.text(), rt.state.interval.as_i64()), ("15", Some(15)));
    press(&mut rt, Key::PageUp);
    assert_eq!((rt.state.interval.text(), rt.state.interval.as_i64()), ("20", Some(20)));
    press(&mut rt, Key::PageDown);
    assert_eq!((rt.state.interval.text(), rt.state.interval.as_i64()), ("15", Some(15)));
}

#[test]
fn stepper_buttons_step_keep_focus_skip_tab_and_disable_at_bounds() {
    let mut rt = runtime();
    focus_field(&mut rt, |s| &mut s.interval);
    assert!(rt.state.interval.is_focused());

    // Clicking + and - steps while keeping focus on the editor.
    click_path(&mut rt, "interval/stepper/increment");
    assert_eq!((rt.state.interval.as_i64(), rt.state.interval.text()), (Some(16), "16"));
    assert!(rt.state.interval.is_focused(), "increment button preserves editor focus");

    click_path(&mut rt, "interval/stepper/decrement");
    assert_eq!((rt.state.interval.as_i64(), rt.state.interval.text()), (Some(15), "15"));
    assert!(rt.state.interval.is_focused(), "decrement button preserves editor focus");

    // Tab skips the stepper buttons and lands directly on the next field ("threshold").
    press(&mut rt, Key::Tab);
    assert!(!rt.state.interval.is_focused());
    assert!(rt.state.threshold.is_focused(), "Tab skips stepper buttons to next control");

    // At min bound (10), decrement button is disabled and ignores clicks; increment still works.
    rt.update(|s, _| s.interval.set_value(Some(10.0)));
    rt.frame(0.0);
    click_path(&mut rt, "interval/stepper/decrement");
    assert_eq!(rt.state.interval.as_i64(), Some(10));
    click_path(&mut rt, "interval/stepper/increment");
    assert_eq!(rt.state.interval.as_i64(), Some(11));

    // Stepping empty optional decimal field via + button starts at +step (0.25).
    click_path(&mut rt, "threshold/stepper/increment");
    assert_eq!((rt.state.threshold.value(), rt.state.threshold.text()), (Some(0.25), "0.25"));
}

#[test]
fn fixed_decimals_formats_trailing_zeros_on_commit_and_step() {
    let mut rt = runtime();
    focus_field(&mut rt, |s| &mut s.fixed);
    press(&mut rt, Key::Up);
    assert_eq!((rt.state.fixed.value(), rt.state.fixed.text()), (Some(2.0), "2.00"));
    press(&mut rt, Key::Down);
    assert_eq!((rt.state.fixed.value(), rt.state.fixed.text()), (Some(1.5), "1.50"));
}

#[test]
fn read_only_and_disabled_fields_refuse_mutation() {
    let mut rt = runtime();

    // Read-only accepts focus and caret navigation but rejects typing, Up/Down, and stepper buttons.
    focus_field(&mut rt, |s| &mut s.read_only_field);
    assert!(rt.state.read_only_field.is_focused());
    press(&mut rt, Key::Left);
    assert_eq!(rt.state.read_only_field.edit().focus, 1);
    type_str(&mut rt, "9");
    press(&mut rt, Key::Up);
    click_path(&mut rt, "ro/stepper/increment");
    assert_eq!((rt.state.read_only_field.value(), rt.state.read_only_field.text()), (Some(42.0), "42"));

    // Disabled rejects stepper clicks and assistive Increment/SetValue.
    click_path(&mut rt, "dis/stepper/increment");
    assert_eq!((rt.state.disabled_field.value(), rt.state.disabled_field.text()), (Some(7.0), "7"));
}

#[test]
fn spinbutton_accessibility_supports_range_increment_decrement_and_set_value() {
    let mut rt = runtime();
    let tree = rt.accessibility_tree();
    let spin_nodes: Vec<_> = tree.nodes.iter().filter(|(_, n)| n.role() == Role::SpinButton).collect();
    assert_eq!(spin_nodes.len(), 5, "all five NumberField editors expose Role::SpinButton");

    let json = rt.accessibility_json();
    let find_input = |root: &serde_json::Value, parent_key: &str| -> serde_json::Value {
        fn walk(n: &serde_json::Value, key: &str) -> Option<serde_json::Value> {
            if n["key"].as_str() == Some(key) {
                return Some(n.clone());
            }
            for c in n["children"].as_array().into_iter().flatten() {
                if let Some(found) = walk(c, key) {
                    return Some(found);
                }
            }
            None
        }
        let host = walk(root, parent_key).unwrap_or_else(|| panic!("missing {parent_key}: {root}"));
        walk(&host, "input").unwrap_or_else(|| panic!("missing input under {parent_key}: {host}"))
    };

    let interval = find_input(&json, "interval");
    assert_eq!(interval["role"], "spin-button");
    assert_eq!(interval["name"], "Interval");
    assert_eq!(interval["value"], "15");
    assert_eq!(interval["range"], serde_json::json!([15.0, 10.0, 60.0, 1.0]));

    let threshold = find_input(&json, "threshold");
    assert_eq!(threshold["role"], "spin-button");
    assert_eq!(threshold["name"], "Threshold");
    assert!(threshold["range"].is_null(), "empty unbounded field omits range bounds");

    // AccessKit Increment, Decrement, numeric SetValue, and text SetValue on bounded spinbutton.
    assert!(rt.accessibility_action("input", Action::Increment, None));
    rt.frame(0.0);
    assert_eq!((rt.state.interval.as_i64(), rt.state.interval.text()), (Some(16), "16"));

    assert!(rt.accessibility_action("input", Action::Decrement, None));
    rt.frame(0.0);
    assert_eq!((rt.state.interval.as_i64(), rt.state.interval.text()), (Some(15), "15"));

    assert!(rt.accessibility_action("input", Action::SetValue, Some(ActionData::NumericValue(48.0))));
    rt.frame(0.0);
    assert_eq!((rt.state.interval.as_i64(), rt.state.interval.text()), (Some(48), "48"));

    assert!(rt.accessibility_action("input", Action::SetValue, Some(ActionData::Value("32".into()))));
    rt.frame(0.0);
    assert_eq!((rt.state.interval.as_i64(), rt.state.interval.text()), (Some(32), "32"));

    // Unbounded spinbutton still accepts numeric SetValue without unusable infinite bounds.
    let mut unbounded = Runtime::new(
        NumberFieldState::empty(),
        |s: &NumberFieldState, _| {
            NumberField::new(s)
                .label("Threshold")
                .step(0.25)
                .decimals(2)
                .on_edit(|s: &mut NumberFieldState| s)
                .build(&Kit::neutral(true))
        },
        (300.0, 80.0),
        Env::new(true, 1.0),
    );
    unbounded.frame(0.0);
    assert!(unbounded.accessibility_action("input", Action::SetValue, Some(ActionData::NumericValue(12.345))));
    unbounded.frame(0.0);
    assert_eq!((unbounded.state.value(), unbounded.state.text()), (Some(12.35), "12.35"));

    // Read-only and disabled spinbuttons reject Increment and numeric SetValue.
    let mut ro = Runtime::new(
        NumberFieldState::new(Some(42.0)),
        |s: &NumberFieldState, _| {
            NumberField::new(s)
                .label("Read-only")
                .range(0.0..=100.0)
                .read_only(true)
                .on_edit(|s: &mut NumberFieldState| s)
                .build(&Kit::neutral(true))
        },
        (300.0, 80.0),
        Env::new(true, 1.0),
    );
    ro.frame(0.0);
    assert!(!ro.accessibility_action("input", Action::Increment, None));
    assert!(!ro.accessibility_action("input", Action::SetValue, Some(ActionData::NumericValue(50.0))));
    assert_eq!(ro.state.value(), Some(42.0));

    let mut dis = Runtime::new(
        NumberFieldState::new(Some(7.0)),
        |s: &NumberFieldState, _| {
            NumberField::new(s)
                .label("Disabled")
                .range(0.0..=100.0)
                .disabled(true)
                .on_edit(|s: &mut NumberFieldState| s)
                .build(&Kit::neutral(true))
        },
        (300.0, 80.0),
        Env::new(true, 1.0),
    );
    dis.frame(0.0);
    assert!(!dis.accessibility_action("input", Action::Increment, None));
    assert_eq!(dis.state.value(), Some(7.0));
}

#[test]
fn follows_field_metrics_and_focuses_on_box_padding_click() {
    let mut rt = runtime();
    let h0 = rt.elements().into_iter().find(|e| e.key.as_deref() == Some("threshold")).unwrap().rect[3];
    assert_eq!(h0, 28.0, "default height matches FieldMetrics::kit");

    rt.update(|s, tx| {
        s.custom_metrics = true;
        tx.instant();
    });
    rt.frame(0.0);
    let h1 = rt.elements().into_iter().find(|e| e.key.as_deref() == Some("threshold")).unwrap().rect[3];
    assert_eq!(h1, 34.0, "NumberField respects Kit::metrics.field height");

    // Clicking the muted prefix inside the box focuses the editor cleanly.
    click_path(&mut rt, "threshold/value/prefix");
    assert!(rt.state.threshold.is_focused());
}
