use super::{HiddenDrawable, HiddenDrawableEffect, HiddenDrawableEvent};

fn effects(state: &mut HiddenDrawable, event: HiddenDrawableEvent) -> Vec<HiddenDrawableEffect> {
    state.step(event).into_iter().collect()
}

#[test]
fn hide_shrinks_once_and_wake_restores_once() {
    let mut state = HiddenDrawable::default();
    assert_eq!(state, HiddenDrawable::Full);

    // Waking or configuring while already Full is a no-op.
    assert!(effects(&mut state, HiddenDrawableEvent::Wake).is_empty());
    assert!(effects(&mut state, HiddenDrawableEvent::Configured).is_empty());
    assert_eq!(state, HiddenDrawable::Full);

    // First Hide emits Shrink; repeated Hide is idempotent.
    assert_eq!(effects(&mut state, HiddenDrawableEvent::Hide), vec![HiddenDrawableEffect::Shrink]);
    assert_eq!(state, HiddenDrawable::Shrunk);
    assert!(effects(&mut state, HiddenDrawableEvent::Hide).is_empty());
    assert_eq!(state, HiddenDrawable::Shrunk);

    // First Wake emits Restore; repeated Wake is a no-op.
    assert_eq!(effects(&mut state, HiddenDrawableEvent::Wake), vec![HiddenDrawableEffect::Restore]);
    assert_eq!(state, HiddenDrawable::Full);
    assert!(effects(&mut state, HiddenDrawableEvent::Wake).is_empty());
    assert_eq!(state, HiddenDrawable::Full);
}

#[test]
fn configure_while_shrunk_clears_state_without_redundant_restore() {
    let mut state = HiddenDrawable::Full;
    assert_eq!(effects(&mut state, HiddenDrawableEvent::Hide), vec![HiddenDrawableEffect::Shrink]);
    assert_eq!(state, HiddenDrawable::Shrunk);

    // A resize or reblend configures the surface at its real size, clearing Shrunk.
    assert!(effects(&mut state, HiddenDrawableEvent::Configured).is_empty());
    assert_eq!(state, HiddenDrawable::Full);

    // Subsequent Wake does not redundantly set drawableSize again.
    assert!(effects(&mut state, HiddenDrawableEvent::Wake).is_empty());
    assert_eq!(state, HiddenDrawable::Full);
}

#[test]
fn shrink_policies_exclude_launchers_fading_overlays_and_released_surfaces() {
    // Ordinary hidden windows shrink; launchers and release_gpu_when_closed do not.
    assert!(HiddenDrawable::should_shrink_window(false, false));
    assert!(!HiddenDrawable::should_shrink_window(true, false));
    assert!(!HiddenDrawable::should_shrink_window(false, true));
    assert!(!HiddenDrawable::should_shrink_window(true, true));

    // Overlays shrink only once fully hidden (!drawing) and keeping their surface (!release_now).
    assert!(HiddenDrawable::should_shrink_overlay(false, false));
    assert!(!HiddenDrawable::should_shrink_overlay(true, false));
    assert!(!HiddenDrawable::should_shrink_overlay(false, true));
    assert!(!HiddenDrawable::should_shrink_overlay(true, true));
}
