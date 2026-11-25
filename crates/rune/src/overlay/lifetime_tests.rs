use super::super::{KeyPolicy, Overlay, OverlayOptions, OverlaySpec, Presence};
use crate::{AppSpec, Cx, El, el};

fn view(_: &Vec<bool>, _: &mut Cx<Vec<bool>>) -> El { el().size(20.0, 20.0) }

fn overlay(keys: KeyPolicy) -> Overlay<Vec<bool>> {
    let app = AppSpec::new("launcher", (40.0, 20.0), Vec::new, view);
    let spec = OverlaySpec::new("launcher", app.size, view)
        .options(OverlayOptions { keys, ..Default::default() })
        .on_focus(|seen: &mut Vec<bool>, focused, _| seen.push(focused));
    let (mut overlay, _) = Overlay::new(spec, Vec::new(), &app);
    overlay.rt.frame(0.0);
    overlay
}

/// A Wayland surface is destroyed on hide without `wl_keyboard.leave`: the hide reports the
/// blur, so the next show's `wl_keyboard.enter` is heard as a focus change again.
#[test]
fn a_focused_key_taking_overlay_blurs_on_hide_and_hears_the_next_shows_focus() {
    for keys in [KeyPolicy::OnDemand, KeyPolicy::Exclusive] {
        let mut overlay = overlay(keys);
        for _ in 0..2 {
            overlay.presence = Presence::Shown { occluded: false };
            overlay.rt.window_focus(true);
            overlay.rt.frame(0.0);
            overlay.hide();
            overlay.rt.frame(0.0);
        }
        assert_eq!(overlay.rt.state, [true, false, true, false], "{keys:?}");
    }
}

/// An overlay that never had focus hears no blur when it hides.
#[test]
fn an_unfocused_overlay_hears_no_blur_on_hide() {
    for keys in [KeyPolicy::None, KeyPolicy::OnDemand] {
        let mut overlay = overlay(keys);
        overlay.presence = Presence::Shown { occluded: false };
        overlay.rt.frame(0.0);
        overlay.hide();
        overlay.rt.frame(0.0);
        assert!(overlay.rt.state.is_empty(), "{keys:?}");
    }
}
