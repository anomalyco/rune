use super::spec::{Draft, Spec, format_number, parse_commit_number, parse_draft, round_to_decimals};
use super::NumberFieldState;

#[test]
fn parse_draft_distinguishes_empty_intermediate_valid_and_invalid() {
    assert_eq!(parse_draft("", None), Draft::Empty);
    assert_eq!(parse_draft("   ", None), Draft::Empty);
    assert_eq!(parse_draft("-", None), Draft::Intermediate);
    assert_eq!(parse_draft("+", None), Draft::Intermediate);
    assert_eq!(parse_draft(".", None), Draft::Intermediate);
    assert_eq!(parse_draft("-.", None), Draft::Intermediate);
    assert_eq!(parse_draft("1.", None), Draft::Intermediate);
    assert_eq!(parse_draft("-12.", Some(2)), Draft::Intermediate);

    assert_eq!(parse_draft("0", None), Draft::Number(0.0));
    assert_eq!(parse_draft("-0", None), Draft::Number(0.0));
    assert_eq!(parse_draft(".5", None), Draft::Number(0.5));
    assert_eq!(parse_draft("-.5", None), Draft::Number(-0.5));
    assert_eq!(parse_draft("42.75", Some(2)), Draft::Number(42.75));

    // Integer mode rejects decimal points while typing.
    assert_eq!(parse_draft(".", Some(0)), Draft::Invalid);
    assert_eq!(parse_draft("1.", Some(0)), Draft::Invalid);
    assert_eq!(parse_draft("-", Some(0)), Draft::Intermediate);

    // Non-numeric and non-finite strings are rejected.
    for bad in ["abc", "1.2.3", "--1", "NaN", "inf", "-inf", "1e5"] {
        assert_eq!(parse_draft(bad, None), Draft::Invalid, "{bad}");
        assert_eq!(parse_commit_number(bad), None, "{bad}");
    }
}

#[test]
fn format_and_rounding_avoid_negative_zero_and_float_noise() {
    assert_eq!(format_number(-0.0, None, false), "0");
    assert_eq!(format_number(-0.0001, Some(2), true), "0.00");
    assert_eq!(format_number(1.5, Some(2), false), "1.5");
    assert_eq!(format_number(1.5, Some(2), true), "1.50");
    assert_eq!(format_number(2.0, Some(2), false), "2");
    assert_eq!(format_number(2.0, Some(0), false), "2");
    assert_eq!(round_to_decimals(0.1 + 0.2, 1), 0.3);
}

#[test]
fn stepping_eliminates_binary_float_drift_and_clamps() {
    let spec = Spec::normalized(None, None, 0.1, None, None, false, true);
    let mut s = NumberFieldState::new(Some(0.1));
    spec.step_by(&mut s, 1);
    assert_eq!((s.value(), s.text()), (Some(0.2), "0.2"));
    spec.step_by(&mut s, 1);
    assert_eq!((s.value(), s.text()), (Some(0.3), "0.3"));
    spec.step_by(&mut s, -1);
    assert_eq!((s.value(), s.text()), (Some(0.2), "0.2"));
}

#[test]
fn stepping_from_empty_follows_bounds() {
    let unbounded = Spec::normalized(None, None, 0.25, None, Some(2), false, true);
    let mut s = NumberFieldState::empty();
    unbounded.step_by(&mut s, 1);
    assert_eq!((s.value(), s.text()), (Some(0.25), "0.25"));

    let mut s = NumberFieldState::empty();
    unbounded.step_by(&mut s, -1);
    assert_eq!((s.value(), s.text()), (Some(-0.25), "-0.25"));

    let bounded = Spec::normalized(Some(10.0), Some(60.0), 1.0, Some(5.0), Some(0), false, true);
    let mut s = NumberFieldState::empty();
    bounded.step_by(&mut s, 1);
    assert_eq!((s.value(), s.text()), (Some(10.0), "10"));
    bounded.step_by(&mut s, 10);
    assert_eq!((s.value(), s.text()), (Some(15.0), "15"));
}

#[test]
fn empty_is_distinct_from_zero_and_required_reverts() {
    let optional = Spec::normalized(None, None, 1.0, None, None, false, true);
    let mut s = NumberFieldState::new(Some(0.0));
    assert_eq!(s.value(), Some(0.0));
    assert!(!s.is_empty());

    s.edit_mut().replace_text("");
    assert_eq!(optional.apply_input_change(&mut s), Some(None));
    assert_eq!(s.value(), None);
    assert!(s.is_empty());

    s.edit_mut().replace_text("0");
    assert_eq!(optional.apply_input_change(&mut s), Some(Some(0.0)));
    assert_eq!(s.value(), Some(0.0));

    let required = Spec::normalized(Some(1.0), Some(10.0), 1.0, None, Some(0), false, false);
    let mut r = NumberFieldState::new(Some(5.0));
    r.edit_mut().replace_text("");
    assert_eq!(required.apply_input_change(&mut r), None);
    assert!(r.is_invalid());
    assert_eq!(r.value(), Some(5.0));
    let out = required.commit(&mut r, true);
    assert_eq!(out.committed, Some(Some(5.0)));
    assert_eq!((r.value(), r.text(), r.is_invalid()), (Some(5.0), "5", false));
}
