use std::cell::Cell;
use std::rc::Rc;

use crate::window::{WakeEvent, WindowHandle};
use crate::{
    App, AppSpec, Color, Cx, El, Env, OverlayOptions, OverlaySpec, Runtime, WindowBackdrop,
    WindowOptions, WindowSpec, el, memo,
};

fn dummy_spec() -> AppSpec<()> {
    AppSpec::new("Backdrop", (320.0, 240.0), || (), |_, _| el())
}

#[test]
fn window_backdrop_default_and_is_active() {
    assert_eq!(WindowBackdrop::default(), WindowBackdrop::None);
    assert!(!WindowBackdrop::None.is_active());
    assert!(WindowBackdrop::Mica.is_active());
    assert!(WindowBackdrop::MicaAlt.is_active());
    assert!(WindowBackdrop::Acrylic.is_active());
}

#[test]
fn app_window_spec_and_overlay_builders_preserve_window_backdrop() {
    assert_eq!(App::new(dummy_spec()).window_backdrop, WindowBackdrop::None);

    let app = App::new(dummy_spec()).window_backdrop(WindowBackdrop::Mica);
    assert_eq!(app.window_backdrop, WindowBackdrop::Mica);

    let from_app = WindowSpec::from_app("notes", app);
    assert_eq!(from_app.app.window_backdrop, WindowBackdrop::Mica);

    let window = WindowSpec::new("notes", dummy_spec()).window_backdrop(WindowBackdrop::MicaAlt);
    assert_eq!(window.app.window_backdrop, WindowBackdrop::MicaAlt);

    // Replacing WindowOptions keeps portable window_backdrop.
    let with_opts = window.options(WindowOptions { transparent: true, ..Default::default() });
    assert_eq!(with_opts.app.window_backdrop, WindowBackdrop::MicaAlt);

    // Overlay defaults to None and retains configured backdrop without new OverlayOptions fields.
    let default_overlay = OverlaySpec::new("hud", (240.0, 72.0), |_: &(), _| el())
        .options(OverlayOptions::default());
    assert_eq!(default_overlay.window_backdrop, WindowBackdrop::None);

    let configured_overlay = default_overlay.window_backdrop(WindowBackdrop::Acrylic);
    assert_eq!(configured_overlay.window_backdrop, WindowBackdrop::Acrylic);
}

#[test]
fn window_handle_queues_window_backdrop_requests_in_order() {
    let handle = WindowHandle::new("backdrop");
    handle.set_window_backdrop(WindowBackdrop::Mica);
    handle.set_window_backdrop(WindowBackdrop::Acrylic);
    handle.set_window_backdrop(WindowBackdrop::None);

    let events = handle.take_headless();
    #[cfg(not(target_arch = "wasm32"))]
    {
        assert_eq!(events.len(), 3);
        assert!(matches!(events[0], WakeEvent::WindowBackdrop(0, WindowBackdrop::Mica)));
        assert!(matches!(events[1], WakeEvent::WindowBackdrop(0, WindowBackdrop::Acrylic)));
        assert!(matches!(events[2], WakeEvent::WindowBackdrop(0, WindowBackdrop::None)));
    }
    #[cfg(target_arch = "wasm32")]
    assert!(events.is_empty());
}

#[test]
fn overlay_handle_set_window_backdrop_queues_last_wins_request() {
    let mut app = App::new(dummy_spec());
    let overlay = app.overlay(
        OverlaySpec::new("hud", (240.0, 72.0), |_: &(), _| el())
            .options(OverlayOptions::default())
            .window_backdrop(WindowBackdrop::Mica),
        (),
    );
    overlay.set_window_backdrop(WindowBackdrop::MicaAlt);
    overlay.set_window_backdrop(WindowBackdrop::Acrylic);
    overlay.set_window_backdrop(WindowBackdrop::None);
    // Calling set_window_backdrop before the event loop exists must not panic.
    assert_eq!(overlay.availability(), crate::Availability::Unknown);
}

#[test]
fn tx_set_window_backdrop_latches_last_wins_without_optimistic_cx_flip() {
    struct Seen(Rc<Cell<WindowBackdrop>>);
    let seen = Rc::new(Cell::new(WindowBackdrop::Mica));
    let mut rt = Runtime::new(
        Seen(seen.clone()),
        |s, cx: &mut Cx<Seen>| {
            s.0.set(cx.window_backdrop());
            el()
        },
        (200.0, 120.0),
        Env::default(),
    );
    rt.frame(0.0);
    assert_eq!(seen.get(), WindowBackdrop::None);
    assert_eq!(rt.core().host.window_backdrop_request, None);

    rt.update(|_, tx| {
        tx.set_window_backdrop(WindowBackdrop::Mica);
        tx.set_window_backdrop(WindowBackdrop::Acrylic);
    });
    rt.frame(0.0);
    assert_eq!(
        rt.core_mut().host.window_backdrop_request.take(),
        Some(WindowBackdrop::Acrylic),
        "last Tx::set_window_backdrop in a turn wins",
    );
    assert_eq!(rt.core().host.window_backdrop_request, None);
    assert_eq!(
        seen.get(),
        WindowBackdrop::None,
        "Tx::set_window_backdrop must never flip Cx::window_backdrop() optimistically",
    );

}

#[test]
fn runtime_set_window_backdrop_rebuilds_view_and_memos_and_clear_chrome_resets() {
    struct State {
        view_runs: Rc<Cell<u32>>,
        memo_runs: Rc<Cell<u32>>,
        seen: Rc<Cell<WindowBackdrop>>,
        cleared: Rc<Cell<WindowBackdrop>>,
    }

    fn view(s: &State, cx: &mut Cx<State>) -> El {
        s.view_runs.set(s.view_runs.get() + 1);
        let active = cx.window_backdrop();
        s.seen.set(active);
        let mut sub: Cx<()> = cx.retype();
        sub.clear_chrome();
        s.cleared.set(sub.window_backdrop());
        let memo_runs = s.memo_runs.clone();
        memo("backdrop-memo", 0u32, move || {
            memo_runs.set(memo_runs.get() + 1);
            el().size(if active.is_active() { 120.0 } else { 60.0 }, 20.0)
        })
    }

    let view_runs = Rc::new(Cell::new(0));
    let memo_runs = Rc::new(Cell::new(0));
    let seen = Rc::new(Cell::new(WindowBackdrop::None));
    let cleared = Rc::new(Cell::new(WindowBackdrop::Mica));
    let mut rt = Runtime::new(
        State {
            view_runs: view_runs.clone(),
            memo_runs: memo_runs.clone(),
            seen: seen.clone(),
            cleared: cleared.clone(),
        },
        view,
        (200.0, 100.0),
        Env::default(),
    );

    rt.frame(0.0);
    assert_eq!(view_runs.get(), 1);
    assert_eq!(memo_runs.get(), 1);
    assert_eq!(seen.get(), WindowBackdrop::None);

    // Setting the same value is a no-op (no view rebuild, no memo rebuild).
    rt.set_window_backdrop(WindowBackdrop::None);
    rt.frame(1.0);
    assert_eq!(view_runs.get(), 1);
    assert_eq!(memo_runs.get(), 1);

    // Confirming Mica rebuilds both the view and its memo closure.
    rt.set_window_backdrop(WindowBackdrop::Mica);
    rt.frame(2.0);
    assert_eq!(view_runs.get(), 2);
    assert_eq!(memo_runs.get(), 2);
    assert_eq!(seen.get(), WindowBackdrop::Mica);
    assert_eq!(
        cleared.get(),
        WindowBackdrop::None,
        "clear_chrome resets window_backdrop for staged playground subviews",
    );

    // Switching Mica -> Acrylic invalidates memos again.
    rt.set_window_backdrop(WindowBackdrop::Acrylic);
    rt.frame(3.0);
    assert_eq!(view_runs.get(), 3);
    assert_eq!(memo_runs.get(), 3);
    assert_eq!(seen.get(), WindowBackdrop::Acrylic);

    // Switching back to None rebuilds once more.
    rt.set_window_backdrop(WindowBackdrop::None);
    rt.frame(4.0);
    assert_eq!(view_runs.get(), 4);
    assert_eq!(memo_runs.get(), 4);
    assert_eq!(seen.get(), WindowBackdrop::None);
}

#[test]
fn transparent_opaque_fallback_clear_color_prevents_transparent_hole() {
    let mut rt = Runtime::new(
        Color::TRANSPARENT,
        |fill: &Color, _| el().w_full().h_full().fill(*fill),
        (160.0, 120.0),
        Env::new(true, 1.0),
    );
    rt.core_mut().transparent = true;
    rt.frame(0.0);

    // Normal transparent window (no fallback): clears to [0, 0, 0, 0].
    assert_eq!(rt.clear_color(), [0.0; 4]);

    // When a requested backdrop cannot activate on a Windows non-overlay transparent window,
    // opaque fallback clears to an opaque (alpha 1.0) theme-matched colour in dark mode.
    rt.core_mut().set_backdrop_opaque_fallback(true);
    let dark_clear = rt.clear_color();
    assert_eq!(dark_clear[3], 1.0, "dark fallback clear must be opaque");
    assert!(dark_clear[0] > 0.0 && dark_clear[0] < 0.6, "dark fallback is dark neutral: {dark_clear:?}");

    // Translucent root tint (alpha < 1) still clears to opaque theme neutral rather than turning
    // the tint colour into full opaque white.
    rt.update(|fill, _| *fill = Color::WHITE.alpha(0.08));
    rt.frame(0.0);
    let tint_clear = rt.clear_color();
    assert_eq!(tint_clear, dark_clear);

    // Light mode uses the light neutral fallback.
    rt.set_system_dark(false);
    rt.frame(0.0);
    let light_clear = rt.clear_color();
    assert_eq!(light_clear[3], 1.0, "light fallback clear must be opaque");
    assert!(light_clear[0] > 0.9, "light fallback is light neutral: {light_clear:?}");

    // Linear blending (srgb_blend = false) stays opaque with linear channels.
    rt.set_srgb_blend(false);
    rt.frame(0.0);
    let linear_light = rt.clear_color();
    assert_eq!(linear_light[3], 1.0);
    assert!((linear_light[0] - 0.96).abs() < 1e-5);

    // If the view supplied its own opaque root fill (a >= 1.0), clear_color uses it directly.
    rt.update(|fill, tx| {
        *fill = Color { r: 0.25, g: 0.5, b: 0.75, a: 1.0 };
        tx.instant();
    });
    rt.frame(0.0);
    assert_eq!(rt.clear_color(), [0.25, 0.5, 0.75, 1.0]);

    // Clearing the fallback restores normal transparent clear.
    rt.core_mut().set_backdrop_opaque_fallback(false);
    assert_eq!(rt.clear_color(), [0.0; 4]);
}

#[test]
fn headless_default_render_keeps_opaque_clear_and_none_backdrop() {
    let seen = Rc::new(Cell::new(WindowBackdrop::Mica));
    let seen_view = seen.clone();
    let mut rt = Runtime::new(
        seen_view,
        |seen: &Rc<Cell<WindowBackdrop>>, cx| {
            seen.set(cx.window_backdrop());
            let bg = if cx.window_backdrop().is_active() {
                Color::TRANSPARENT
            } else {
                Color { r: 0.1, g: 0.2, b: 0.3, a: 1.0 }
            };
            el().w_full().h_full().fill(bg)
        },
        (120.0, 80.0),
        Env::new(true, 1.0),
    );
    rt.set_srgb_blend(false);
    rt.frame(0.0);
    assert_eq!(seen.get(), WindowBackdrop::None);
    assert_eq!(rt.clear_color(), [0.1, 0.2, 0.3, 1.0]);
}
