use super::*;

#[test]
fn presses_on_one_element_close_in_time_and_place_count_up() {
    let mut run = PressRun::default();
    assert_eq!(run.press("a", 0.0, (10.0, 10.0), 0.5), 1);
    assert_eq!(run.press("a", 0.3, (12.0, 9.0), 0.5), 2);
    assert_eq!(run.press("a", 0.6, (13.0, 9.0), 0.5), 3, "the interval runs from the last press, not the first");
}

#[test]
fn another_element_a_late_press_or_a_far_one_starts_again() {
    let mut run = PressRun::default();
    run.press("a", 0.0, (10.0, 10.0), 0.5);
    assert_eq!(run.press("b", 0.1, (10.0, 10.0), 0.5), 1);
    assert_eq!(run.press("b", 0.7, (10.0, 10.0), 0.5), 1);
    assert_eq!(run.press("b", 0.8, (10.0 + CLICK_SLOP + 1.0, 10.0), 0.5), 1);
}

#[test]
fn clearing_or_moving_away_ends_the_run() {
    let mut run = PressRun::default();
    run.press("a", 0.0, (0.0, 0.0), 0.5);
    run.keep_on(Some("a"));
    assert_eq!(run.press("a", 0.1, (0.0, 0.0), 0.5), 2);
    run.keep_on(Some("b"));
    assert_eq!(run.press("a", 0.2, (0.0, 0.0), 0.5), 1);
    run.clear();
    assert_eq!(run.press("a", 0.3, (0.0, 0.0), 0.5), 1);
}
