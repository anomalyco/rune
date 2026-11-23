use super::*;

fn effects(state: &mut SurfaceState, event: SurfaceEvent) -> Vec<SurfaceEffect> {
    step(state, event).into_iter().collect()
}

#[test]
fn prewarmed_xwayland_surface_stays_hidden_and_inert_until_first_show() {
    for input in [OverlayInput::All, OverlayInput::Regions, OverlayInput::None] {
        let mut state = SurfaceState::new(input, true);
        assert!(!state.shown(), "{input:?}: must start unmapped");
        assert_eq!(state.active_input(), OverlayInput::None, "{input:?}: must ignore events while hidden");
        assert!(!state.accepts_regions(), "{input:?}: warm_frame must not publish hit regions while hidden");

        let on_show = effects(&mut state, SurfaceEvent::Show);
        let expected = match input {
            OverlayInput::None => vec![SurfaceEffect::Drain, SurfaceEffect::MapAbove],
            OverlayInput::All | OverlayInput::Regions => vec![
                SurfaceEffect::Drain,
                SurfaceEffect::ApplyInput(input),
                SurfaceEffect::MapAbove,
                SurfaceEffect::StartWatch,
            ],
            OverlayInput::Legacy => unreachable!(),
        };
        assert_eq!(on_show, expected, "{input:?}: show must drain, apply input, map above, and start watch");
        assert!(state.shown());
        assert_eq!(state.active_input(), input);
        assert_eq!(state.accepts_regions(), input == OverlayInput::Regions);

        // Showing an already-shown surface does nothing.
        assert!(effects(&mut state, SurfaceEvent::Show).is_empty());
    }
}

#[test]
fn set_input_while_hidden_defers_to_show_without_touching_x11() {
    let mut state = SurfaceState::new(OverlayInput::Regions, true);
    assert!(effects(&mut state, SurfaceEvent::SetInput(OverlayInput::All)).is_empty());
    assert!(!state.shown());
    assert_eq!(state.active_input(), OverlayInput::None);
    assert!(!state.accepts_regions());

    assert_eq!(
        effects(&mut state, SurfaceEvent::Show),
        vec![
            SurfaceEffect::Drain,
            SurfaceEffect::ApplyInput(OverlayInput::All),
            SurfaceEffect::MapAbove,
            SurfaceEffect::StartWatch,
        ],
    );
    assert_eq!(state.active_input(), OverlayInput::All);

    // Turning input off before show keeps the watch dormant until input turns back on while shown.
    let mut deferred = SurfaceState::new(OverlayInput::Regions, true);
    assert!(effects(&mut deferred, SurfaceEvent::SetInput(OverlayInput::None)).is_empty());
    assert_eq!(effects(&mut deferred, SurfaceEvent::Show), vec![SurfaceEffect::Drain, SurfaceEffect::MapAbove]);
    assert_eq!(deferred.active_input(), OverlayInput::None);
    assert_eq!(
        effects(&mut deferred, SurfaceEvent::SetInput(OverlayInput::Regions)),
        vec![SurfaceEffect::ApplyInput(OverlayInput::Regions), SurfaceEffect::StartWatch],
    );
    assert!(deferred.accepts_regions());
    // Subsequent shown input changes do not start a second watch.
    assert_eq!(
        effects(&mut deferred, SurfaceEvent::SetInput(OverlayInput::All)),
        vec![SurfaceEffect::ApplyInput(OverlayInput::All)],
    );
}

#[test]
fn surface_made_without_input_or_waker_stays_click_through_on_set_input() {
    for (initial, has_wake) in [(OverlayInput::None, true), (OverlayInput::Regions, false)] {
        let mut state = SurfaceState::new(initial, has_wake);
        assert!(effects(&mut state, SurfaceEvent::SetInput(OverlayInput::All)).is_empty());
        assert_eq!(effects(&mut state, SurfaceEvent::Show), vec![SurfaceEffect::Drain, SurfaceEffect::MapAbove]);
        assert_eq!(state.active_input(), OverlayInput::None);
        assert!(effects(&mut state, SurfaceEvent::SetInput(OverlayInput::Regions)).is_empty());
        assert!(!state.accepts_regions());
    }
}
