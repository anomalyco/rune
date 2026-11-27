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

fn sample_high_contrast() -> crate::SystemContrast {
    crate::SystemContrast {
        window: crate::Color::BLACK,
        text: crate::Color::WHITE,
        highlight: crate::Color { r: 0.2, g: 0.7, b: 0.9, a: 1.0 },
        highlight_text: crate::Color::BLACK,
        disabled: crate::Color { r: 0.5, g: 0.5, b: 0.5, a: 1.0 },
        button: crate::Color::BLACK,
        button_text: crate::Color::WHITE,
        link: crate::Color { r: 0.4, g: 0.8, b: 1.0, a: 1.0 },
    }
}

/// A non-activating overlay (`KeyPolicy::None`) never receives `WindowEvent::Focused`, so it must
/// reflect an existing high-contrast preference on its first draw without waiting for a
/// settings-change notification; when no contrast preference is active, defaults remain off.
#[test]
fn non_activating_overlay_sees_existing_high_contrast_on_first_draw() {
    use std::cell::RefCell;
    use std::rc::Rc;

    type Seen = Rc<RefCell<Vec<(Option<crate::SystemContrast>, bool)>>>;
    fn contrast_view(seen: &Seen, cx: &mut Cx<Seen>) -> El {
        seen.borrow_mut().push((cx.system_contrast(), cx.system_increased_contrast()));
        el().size(40.0, 20.0)
    }

    let hc = sample_high_contrast();
    {
        let _guard = crate::platform::preferences::test_overrides::set_contrast(Some(hc), true);
        let seen: Seen = Rc::default();
        let app = AppSpec::new("hc-overlay", (40.0, 20.0), Seen::default, contrast_view);
        let spec = OverlaySpec::new("hc-overlay", app.size, contrast_view)
            .options(OverlayOptions { keys: KeyPolicy::None, ..Default::default() });
        let (mut overlay, _) = Overlay::new(spec, seen.clone(), &app);
        // First draw without any focus event or settings-change notification.
        overlay.rt.frame(0.0);
        assert_eq!(seen.borrow().as_slice(), &[(Some(hc), true)]);
        assert_eq!(overlay.rt.core().system_contrast(), Some(hc));
    }
    {
        let _guard = crate::platform::preferences::test_overrides::set_contrast(None, false);
        let seen: Seen = Rc::default();
        let app = AppSpec::new("plain-overlay", (40.0, 20.0), Seen::default, contrast_view);
        let spec = OverlaySpec::new("plain-overlay", app.size, contrast_view)
            .options(OverlayOptions { keys: KeyPolicy::None, ..Default::default() });
        let (mut overlay, _) = Overlay::new(spec, seen.clone(), &app);
        overlay.rt.frame(0.0);
        assert_eq!(seen.borrow().as_slice(), &[(None, false)]);
        assert_eq!(overlay.rt.core().system_contrast(), None);
    }
}

/// An overlay created or parked while hidden misses `WM_SETTINGCHANGE`; revealing it refreshes
/// contrast preferences before its first visible draw and marks retained surfaces dirty.
#[test]
fn hidden_non_activating_overlay_picks_up_high_contrast_before_show() {
    use std::cell::RefCell;
    use std::rc::Rc;

    type Seen = Rc<RefCell<Vec<(Option<crate::SystemContrast>, bool)>>>;
    fn contrast_view(seen: &Seen, cx: &mut Cx<Seen>) -> El {
        seen.borrow_mut().push((cx.system_contrast(), cx.system_increased_contrast()));
        el().size(40.0, 20.0)
    }

    let hc = sample_high_contrast();
    let seen: Seen = Rc::default();
    let app = AppSpec::new("hidden-hc", (40.0, 20.0), Seen::default, contrast_view);
    let spec = OverlaySpec::new("hidden-hc", app.size, contrast_view)
        .options(OverlayOptions { keys: KeyPolicy::None, ..Default::default() });

    let mut overlay = {
        let _off = crate::platform::preferences::test_overrides::set_contrast(None, false);
        let (mut overlay, _) = Overlay::new(spec, seen.clone(), &app);
        overlay.rt.frame(0.0);
        overlay
    };
    assert_eq!(seen.borrow().as_slice(), &[(None, false)]);
    overlay.dirty = false;

    // High contrast turns on while the overlay is hidden (no HWND / no focus event).
    {
        let _on = crate::platform::preferences::test_overrides::set_contrast(Some(hc), true);
        overlay.core.refresh_preferences_for_show(overlay.rt.core_mut());
        assert!(overlay.dirty, "contrast change before show must mark the overlay dirty");
        overlay.rt.frame(0.0);
    }
    assert_eq!(seen.borrow().as_slice(), &[(None, false), (Some(hc), true)]);
    overlay.dirty = false;

    // High contrast turns back off while hidden.
    {
        let _off = crate::platform::preferences::test_overrides::set_contrast(None, false);
        overlay.core.refresh_preferences_for_show(overlay.rt.core_mut());
        assert!(overlay.dirty, "clearing contrast before show must mark the overlay dirty");
        overlay.rt.frame(0.0);
    }
    assert_eq!(seen.borrow().as_slice(), &[(None, false), (Some(hc), true), (None, false)]);
}

/// Non-activating overlays sample Reduce Transparency at creation and refresh it before reveal.
#[test]
fn non_activating_overlay_tracks_reduced_transparency_on_create_and_before_show() {
    use std::cell::RefCell;
    use std::rc::Rc;

    type Seen = Rc<RefCell<Vec<bool>>>;
    fn view(seen: &Seen, cx: &mut Cx<Seen>) -> El {
        seen.borrow_mut().push(cx.system_reduced_transparency());
        el().size(40.0, 20.0)
    }

    let seen: Seen = Rc::default();
    let app = AppSpec::new("rt-overlay", (40.0, 20.0), Seen::default, view);
    let spec = OverlaySpec::new("rt-overlay", app.size, view)
        .options(OverlayOptions { keys: KeyPolicy::None, ..Default::default() });
    let mut overlay = {
        let _on = crate::platform::preferences::test_overrides::set_reduced_transparency(true);
        let (mut overlay, _) = Overlay::new(spec, seen.clone(), &app);
        overlay.rt.frame(0.0);
        overlay
    };
    assert_eq!(seen.borrow().as_slice(), &[true]);
    overlay.dirty = false;

    {
        let _off = crate::platform::preferences::test_overrides::set_reduced_transparency(false);
        overlay.core.refresh_preferences_for_show(overlay.rt.core_mut());
        assert!(overlay.dirty, "reduced transparency change before show must mark overlay dirty");
        overlay.rt.frame(0.0);
    }
    assert_eq!(seen.borrow().as_slice(), &[true, false]);
}
