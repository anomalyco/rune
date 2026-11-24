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

#[test]
fn tx_set_color_scheme_latches_last_wins_without_forcing_redundant_redraw() {
    let mut rt = Runtime::new((), |_: &(), _: &mut Cx<()>| crate::el(), (200.0, 100.0), Env::default());
    rt.frame(0.0);
    assert_eq!(rt.host.window_color_scheme, None);

    // update_if returning false latches the color scheme without marking the view stale.
    assert!(!rt.update_if(|_, tx| {
        tx.set_color_scheme(crate::ColorScheme::Light);
        tx.set_color_scheme(crate::ColorScheme::Dark);
        false
    }));
    assert!(!rt.stale());
    assert_eq!(rt.host.window_color_scheme.take(), Some(crate::ColorScheme::Dark));
    assert_eq!(rt.host.window_color_scheme, None);

    // Switching back to System latches cleanly.
    rt.update(|_, tx| tx.set_color_scheme(crate::ColorScheme::System));
    assert_eq!(rt.host.window_color_scheme.take(), Some(crate::ColorScheme::System));

    // Merging two Tx values keeps the later color_scheme override.
    let mut base = Tx::default();
    base.set_color_scheme(crate::ColorScheme::Light);
    let mut later = Tx::default();
    later.set_color_scheme(crate::ColorScheme::Dark);
    crate::runtime::input::action::merge(&mut base, later);
    assert_eq!(base.color_scheme, Some(crate::ColorScheme::Dark));
    // Merging a Tx without a color_scheme keeps the earlier one.
    crate::runtime::input::action::merge(&mut base, Tx::default());
    assert_eq!(base.color_scheme, Some(crate::ColorScheme::Dark));
}
