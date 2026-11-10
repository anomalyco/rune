use super::*;

#[test]
fn a_fitting_window_asks_once_per_height() {
    let mut fit = FitWindow::default();
    assert!(!fit.ask(300.0, 300.0), "never asked, and already that tall");
    assert!(fit.ask(420.0, 300.0));
    assert!(!fit.ask(420.0, 300.0), "asked already, though the window hasn't moved yet");
    assert!(fit.ask(300.0, 360.0));
}

#[test]
fn a_resize_from_outside_forgets_what_it_asked() {
    let mut fit = FitWindow::default();
    assert!(fit.ask(420.0, 300.0));
    assert!(!fit.resized(420.3, 300.0), "the window reached what it asked, within half a point");
    assert!(fit.resized(500.0, 420.0));
    assert!(fit.ask(420.0, 500.0), "the next commit asks for the content's height again");
    assert!(!FitWindow::default().resized(300.0, 300.0), "never asked: only a change of height counts");
}

#[test]
fn a_hold_lasts_the_frame_that_asked_and_ends_when_the_host_takes_it() {
    let mut fit = FitWindow::default();
    assert!(!fit.frame_holds() && !fit.take_held(), "nothing asked");
    fit.hold = FitHold::Asked;
    assert!(fit.frame_holds());
    assert!(fit.take_held());
    assert!(!fit.take_held(), "taken once");
    // A host that doesn't take it: the next frame doesn't hold, and neither is it reported.
    fit.hold = FitHold::Asked;
    assert!(fit.frame_holds());
    assert!(!fit.frame_holds());
    assert!(!fit.take_held());
}
