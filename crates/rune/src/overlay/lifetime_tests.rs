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
    for keys in [KeyPolicy::None, KeyPolicy::OnDemand, KeyPolicy::Exclusive] {
        let mut overlay = overlay(keys);
        overlay.presence = Presence::Shown { occluded: false };
        overlay.rt.frame(0.0);
        overlay.hide();
        overlay.rt.frame(0.0);
        assert!(overlay.rt.state.is_empty(), "{keys:?}");
    }
}

/// An `on_focus(false)` handler that closes the overlay on blur must not leave a stale
/// `Shell::Close` behind when the overlay is already hiding (including during a macOS or Windows
/// fade-out, where `hidden_input` does not advance), and a native `Focused(false)` that follows
/// must neither repeat `on_focus(false)` nor re-hide the overlay.
#[test]
fn shell_close_from_hide_blur_does_not_linger_or_abort_a_fade() {
    for keys in [KeyPolicy::OnDemand, KeyPolicy::Exclusive] {
        let app = AppSpec::new("launcher", (40.0, 20.0), Vec::new, view);
        let spec = OverlaySpec::new("launcher", app.size, view)
            .options(OverlayOptions { keys, ..Default::default() })
            .on_focus(|seen: &mut Vec<bool>, focused, tx| {
                seen.push(focused);
                if !focused { tx.close_window(); }
            });
        let (mut overlay, _) = Overlay::new(spec, Vec::new(), &app);
        overlay.rt.frame(0.0);

        // Immediate hide while focused.
        overlay.presence = Presence::Shown { occluded: false };
        overlay.rt.window_focus(true);
        overlay.hide();
        assert_eq!(overlay.rt.state, [true, false], "{keys:?}");
        assert_eq!(overlay.rt.shell_request, None, "{keys:?}: immediate hide discards its own blur's Shell::Close");
        assert!(!overlay.presence.shown(), "{keys:?}");

        // Fading hide while focused (macOS/Windows keep drawing through the fade, so hidden_input
        // does not run): Shell::Close must still be cleared so the subsequent native blur does not
        // abort the fade-out or hide the overlay twice.
        overlay.presence = Presence::Shown { occluded: false };
        overlay.rt.window_focus(true);
        overlay.hide_fading(std::time::Duration::from_millis(150));
        assert_eq!(overlay.rt.state, [true, false, true, false], "{keys:?}");
        assert_eq!(overlay.rt.shell_request, None, "{keys:?}: fade-out hide discards its own blur's Shell::Close");
        if cfg!(any(target_os = "macos", target_os = "windows")) {
            assert!(overlay.presence.fading(), "{keys:?}: blur close request must not abort the fade-out");
        }
        // Native Focused(false) arriving after hide (macOS ResignKey / Windows SW_HIDE).
        overlay.rt.window_focus(false);
        assert_eq!(overlay.rt.state, [true, false, true, false], "{keys:?}: native blur after hide is deduplicated");
        assert_eq!(overlay.rt.shell_request, None, "{keys:?}");

        // Blur-first path (clicking outside): on_focus(false) queues Shell::Close, host takes it
        // and calls hide(), and hide_window does not fire a second blur.
        overlay.presence = Presence::Shown { occluded: false };
        overlay.rt.window_focus(true);
        overlay.rt.window_focus(false);
        assert_eq!(overlay.rt.shell_request.take(), Some(crate::el::Shell::Close), "{keys:?}");
        overlay.hide();
        assert_eq!(overlay.rt.state, [true, false, true, false, true, false], "{keys:?}");
        assert_eq!(overlay.rt.shell_request, None, "{keys:?}");
    }
}

/// Native platforms (macOS, Windows) still deliver `WindowEvent::Focused(false)` when the panel or
/// window hides. Both `ConfiguredOverlay::on_focus` and `El::on_window_focus` must hear each edge
/// exactly once across show -> hide -> native blur -> show.
#[test]
fn native_blur_after_hide_does_not_double_report_focus_loss() {
    #[derive(Default)]
    struct Seen { callback: Vec<bool>, element: Vec<bool> }
    fn focus_view(_: &Seen, _: &mut Cx<Seen>) -> El {
        el().size(20.0, 20.0).on_window_focus(|s: &mut Seen, focused, _| s.element.push(focused))
    }
    let app = AppSpec::new("focus", (40.0, 20.0), Seen::default, focus_view);
    let spec = OverlaySpec::new("focus", app.size, focus_view)
        .options(OverlayOptions { keys: KeyPolicy::OnDemand, ..Default::default() })
        .on_focus(|s: &mut Seen, focused, _| s.callback.push(focused));
    let (mut overlay, _) = Overlay::new(spec, Seen::default(), &app);
    overlay.rt.frame(0.0);

    overlay.presence = Presence::Shown { occluded: false };
    overlay.rt.window_focus(true);
    overlay.advance();
    overlay.hide();
    // Native WindowEvent::Focused(false) after hide_window (macOS ResignKey / Windows SW_HIDE).
    overlay.rt.window_focus(false);
    overlay.hidden_input();
    // Next show regains focus.
    overlay.presence = Presence::Shown { occluded: false };
    overlay.rt.window_focus(true);
    overlay.advance();

    assert_eq!(overlay.rt.state.callback, [true, false, true]);
    assert_eq!(overlay.rt.state.element, [true, false, true]);
}

#[test]
fn a_focused_field_in_a_hiding_overlay_fires_on_focus_lost_once() {
    #[derive(Default)]
    struct S { edit: crate::TextEdit, saved: Vec<String> }
    fn field_view(s: &S, _: &mut Cx<S>) -> El {
        crate::col().size(200.0, 60.0).child(
            crate::text_input(&s.edit, crate::TextStyle::new(14.0, 400.0))
                .key("field")
                .size(180.0, 28.0)
                .on_edit(|s: &mut S| &mut s.edit)
                .on_focus_lost(|s: &mut S, _| s.saved.push(s.edit.text.clone())),
        )
    }
    let app = AppSpec::new("palette", (200.0, 60.0), S::default, field_view);
    let spec = OverlaySpec::new("palette", app.size, field_view)
        .options(OverlayOptions { keys: KeyPolicy::OnDemand, ..Default::default() });
    let (mut overlay, _) = Overlay::new(spec, S::default(), &app);
    overlay.rt.frame(0.0);
    overlay.presence = Presence::Shown { occluded: false };
    overlay.rt.window_focus(true);
    overlay.rt.update(|s, _| s.edit.focused = true);
    overlay.rt.frame(0.0);
    overlay.rt.key(crate::KeyEvent { key: crate::Key::Text("query".into()), mods: crate::Mods::default() });
    overlay.rt.frame(0.0);

    overlay.hide();
    // Native blur arriving after hide must not fire a second blur.
    overlay.rt.window_focus(false);
    overlay.hidden_input();
    assert_eq!(overlay.rt.state.saved, ["query"]);
}
