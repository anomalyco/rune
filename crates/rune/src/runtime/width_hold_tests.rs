use super::WidthHold;

#[test]
fn a_width_change_defers_heights_then_lets_a_streaming_row_shrink() {
    let hold = WidthHold::rewrap(2.0);
    assert!(hold.defers(2.0) && hold.defers(2.11));
    assert!(!hold.defers(2.12), "heights land once the width is stable");
    assert!(!hold.may_shrink(2.12) && !hold.may_shrink(2.39));
    assert!(hold.may_shrink(2.4), "a streaming row may shrink once the rewrap is well past");
}

#[test]
fn no_width_change_defers_nothing() {
    for at in [-1.0, 0.0, 5.0] {
        assert!(!WidthHold::Off.defers(at) && WidthHold::Off.may_shrink(at), "at {at}");
    }
}
