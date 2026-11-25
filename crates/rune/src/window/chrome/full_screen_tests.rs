use crate::machine::Effects;

use super::{Effect, Event, State, can_read_titlebar_inset, step, toggle_target, window_is_full_screen};

#[test]
fn initial_state_maps_os_flags() {
    assert_eq!(State::from_os(false, false), State::Windowed);
    assert!(!State::from_os(false, false).is_full_screen());

    assert_eq!(State::from_os(true, false), State::Entering);
    assert!(State::from_os(true, false).is_full_screen());

    assert_eq!(State::from_os(true, true), State::FullScreen);
    assert!(State::from_os(true, true).is_full_screen());

    assert_eq!(State::from_os(false, true), State::FullScreen);
    assert!(State::from_os(false, true).is_full_screen());
}

#[test]
fn normal_enter_and_exit_flip_at_will_notifications() {
    let mut s = State::Windowed;
    assert!(!s.is_full_screen());

    // Enter starts at WillEnter (green button, Ctrl+Cmd+F, Tx::set_full_screen / toggle).
    assert_eq!(step(&mut s, Event::WillEnter), Effects::one(Effect::Redraw));
    assert_eq!(s, State::Entering);
    assert!(s.is_full_screen());

    // Mid-enter syncs while winit reports fullscreen (before or after styleMask flips) keep Entering.
    assert_eq!(
        step(&mut s, Event::Sync { winit_full_screen: true, style_mask_full_screen: false }),
        Effects::none()
    );
    assert_eq!(s, State::Entering);
    assert_eq!(
        step(&mut s, Event::Sync { winit_full_screen: true, style_mask_full_screen: true }),
        Effects::none()
    );
    assert_eq!(s, State::Entering);

    // DidEnter settles into FullScreen and requests a redraw so live bounds settle.
    assert_eq!(step(&mut s, Event::DidEnter), Effects::one(Effect::Redraw));
    assert_eq!(s, State::FullScreen);
    assert!(s.is_full_screen());

    // Exit flips at WillExit (Esc, green button, window manager, Tx::set_full_screen(false)).
    assert_eq!(step(&mut s, Event::WillExit), Effects::one(Effect::Redraw));
    assert_eq!(s, State::Exiting);
    assert!(!s.is_full_screen());

    // Mid-exit syncs while winit still holds fullscreen until windowDidExitFullScreen: stay Exiting.
    assert_eq!(
        step(&mut s, Event::Sync { winit_full_screen: true, style_mask_full_screen: true }),
        Effects::none()
    );
    assert_eq!(s, State::Exiting);
    assert!(!s.is_full_screen());
    assert_eq!(
        step(&mut s, Event::Sync { winit_full_screen: true, style_mask_full_screen: false }),
        Effects::none()
    );
    assert_eq!(s, State::Exiting);

    // DidExit settles into Windowed and requests a redraw so traffic lights and inset restore.
    assert_eq!(step(&mut s, Event::DidExit), Effects::one(Effect::Redraw));
    assert_eq!(s, State::Windowed);
    assert!(!s.is_full_screen());
}

#[test]
fn failed_enter_recovers_via_did_exit_and_via_os_sync() {
    // Path 1: AppKit posts an unexpected DidExit when entering full screen fails (Chromium).
    let mut s = State::Windowed;
    assert_eq!(step(&mut s, Event::WillEnter), Effects::one(Effect::Redraw));
    assert_eq!(s, State::Entering);
    assert!(s.is_full_screen());

    assert_eq!(step(&mut s, Event::DidExit), Effects::one(Effect::Redraw));
    assert_eq!(s, State::Windowed);
    assert!(!s.is_full_screen());

    // Path 2: winit's windowDidFailToEnterFullScreen: clears fullscreen without DidExit.
    assert_eq!(step(&mut s, Event::WillEnter), Effects::one(Effect::Redraw));
    assert_eq!(s, State::Entering);
    assert!(s.is_full_screen());

    assert_eq!(
        step(&mut s, Event::Sync { winit_full_screen: false, style_mask_full_screen: false }),
        Effects::one(Effect::Redraw)
    );
    assert_eq!(s, State::Windowed);
    assert!(!s.is_full_screen());
}

#[test]
fn failed_exit_recovers_when_staying_full_screen_or_dumped_out() {
    // Path 1: AppKit aborts exit and stays in full screen, posting DidEnter.
    let mut s = State::FullScreen;
    assert_eq!(step(&mut s, Event::WillExit), Effects::one(Effect::Redraw));
    assert_eq!(s, State::Exiting);
    assert!(!s.is_full_screen());

    assert_eq!(step(&mut s, Event::DidEnter), Effects::one(Effect::Redraw));
    assert_eq!(s, State::FullScreen);
    assert!(s.is_full_screen());

    // Path 2: AppKit dumps the window out of full screen without animation and posts DidExit.
    assert_eq!(step(&mut s, Event::WillExit), Effects::one(Effect::Redraw));
    assert_eq!(s, State::Exiting);
    assert!(!s.is_full_screen());

    assert_eq!(step(&mut s, Event::DidExit), Effects::one(Effect::Redraw));
    assert_eq!(s, State::Windowed);
    assert!(!s.is_full_screen());
}

#[test]
fn duplicate_notifications_do_not_request_spurious_redraws() {
    let mut s = State::Windowed;
    assert_eq!(step(&mut s, Event::WillExit), Effects::none());
    assert_eq!(step(&mut s, Event::DidExit), Effects::none());

    assert_eq!(step(&mut s, Event::WillEnter), Effects::one(Effect::Redraw));
    assert_eq!(step(&mut s, Event::WillEnter), Effects::none());

    assert_eq!(step(&mut s, Event::DidEnter), Effects::one(Effect::Redraw));
    assert_eq!(step(&mut s, Event::DidEnter), Effects::none());
    assert_eq!(step(&mut s, Event::WillEnter), Effects::none());

    assert_eq!(step(&mut s, Event::WillExit), Effects::one(Effect::Redraw));
    assert_eq!(step(&mut s, Event::WillExit), Effects::none());

    assert_eq!(step(&mut s, Event::DidExit), Effects::one(Effect::Redraw));
    assert_eq!(step(&mut s, Event::DidExit), Effects::none());
}

#[test]
fn rapid_reversals_and_missed_notifications_reconcile() {
    // Reversal mid-enter and mid-exit.
    let mut s = State::Entering;
    assert_eq!(step(&mut s, Event::WillExit), Effects::one(Effect::Redraw));
    assert_eq!(s, State::Exiting);
    assert!(!s.is_full_screen());

    assert_eq!(step(&mut s, Event::WillEnter), Effects::one(Effect::Redraw));
    assert_eq!(s, State::Entering);
    assert!(s.is_full_screen());

    // Missed WillEnter / WillExit.
    s = State::Windowed;
    assert_eq!(step(&mut s, Event::DidEnter), Effects::one(Effect::Redraw));
    assert_eq!(s, State::FullScreen);
    assert_eq!(step(&mut s, Event::DidExit), Effects::one(Effect::Redraw));
    assert_eq!(s, State::Windowed);

    // Sync does not flip Windowed prematurely when only winit has set its pending ivar.
    assert_eq!(
        step(&mut s, Event::Sync { winit_full_screen: true, style_mask_full_screen: false }),
        Effects::none()
    );
    assert_eq!(s, State::Windowed);

    // Sync recovers missed DidEnter when styleMask already reports FullScreen.
    assert_eq!(
        step(&mut s, Event::Sync { winit_full_screen: true, style_mask_full_screen: true }),
        Effects::one(Effect::Redraw)
    );
    assert_eq!(s, State::FullScreen);

    // Sync recovers missed DidExit from FullScreen or Exiting when both OS flags clear.
    assert_eq!(
        step(&mut s, Event::Sync { winit_full_screen: false, style_mask_full_screen: false }),
        Effects::one(Effect::Redraw)
    );
    assert_eq!(s, State::Windowed);

    s = State::Exiting;
    assert_eq!(
        step(&mut s, Event::Sync { winit_full_screen: false, style_mask_full_screen: false }),
        Effects::one(Effect::Redraw)
    );
    assert_eq!(s, State::Windowed);
}

#[test]
fn adopted_panels_never_report_or_toggle_full_screen() {
    for state in [State::Windowed, State::Entering, State::FullScreen, State::Exiting] {
        assert!(!window_is_full_screen(state, true));
        assert_eq!(toggle_target(state, true), None);
        assert_eq!(window_is_full_screen(state, false), state.is_full_screen());
        assert_eq!(toggle_target(state, false), Some(!state.is_full_screen()));
    }
    // Mid-transition toggle reverses the target direction rather than reading stale winit state.
    assert_eq!(toggle_target(State::Entering, false), Some(false));
    assert_eq!(toggle_target(State::Exiting, false), Some(true));
}

#[test]
fn titlebar_inset_guard_zeros_entering_and_detached_buttons() {
    // Entering returns false (0.0 inset) immediately even before styleMask flips or buttons hide.
    assert!(!can_read_titlebar_inset(State::Entering, false, true, true, true));
    assert!(!can_read_titlebar_inset(State::FullScreen, false, true, true, true));
    assert!(!can_read_titlebar_inset(State::FullScreen, true, true, true, true));

    // Exiting returns false while styleMask still has FullScreen OR button is detached in
    // AppKit's full-screen titlebar window.
    assert!(!can_read_titlebar_inset(State::Exiting, true, true, true, true));
    assert!(!can_read_titlebar_inset(State::Exiting, false, true, true, false));
    assert!(can_read_titlebar_inset(State::Exiting, false, true, true, true));

    // Windowed requires FullSizeContentView, visible button, and button attached to this window.
    assert!(can_read_titlebar_inset(State::Windowed, false, true, true, true));
    assert!(!can_read_titlebar_inset(State::Windowed, true, true, true, true));
    assert!(!can_read_titlebar_inset(State::Windowed, false, false, true, true));
    assert!(!can_read_titlebar_inset(State::Windowed, false, true, false, true));
    assert!(!can_read_titlebar_inset(State::Windowed, false, true, true, false));
}
