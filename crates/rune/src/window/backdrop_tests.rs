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
    assert!(WindowBackdrop::Blur.is_active());
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

#[test]
fn window_backdrop_supported_defaults_memo_invalidates_and_clear_chrome_resets() {
    struct State {
        view_runs: Rc<Cell<u32>>,
        memo_runs: Rc<Cell<u32>>,
        none_sup: Rc<Cell<bool>>,
        blur_sup: Rc<Cell<bool>>,
        mica_sup: Rc<Cell<bool>>,
        cleared_none: Rc<Cell<bool>>,
        cleared_blur: Rc<Cell<bool>>,
    }

    fn view(s: &State, cx: &mut Cx<State>) -> El {
        s.view_runs.set(s.view_runs.get() + 1);
        let none_sup = cx.window_backdrop_supported(WindowBackdrop::None);
        let blur_sup = cx.window_backdrop_supported(WindowBackdrop::Blur);
        let mica_sup = cx.window_backdrop_supported(WindowBackdrop::Mica);
        s.none_sup.set(none_sup);
        s.blur_sup.set(blur_sup);
        s.mica_sup.set(mica_sup);
        let mut sub: Cx<()> = cx.retype();
        sub.clear_chrome();
        s.cleared_none.set(sub.window_backdrop_supported(WindowBackdrop::None));
        s.cleared_blur.set(sub.window_backdrop_supported(WindowBackdrop::Blur));
        let memo_runs = s.memo_runs.clone();
        memo("backdrop-supported-memo", 0u32, move || {
            memo_runs.set(memo_runs.get() + 1);
            el().size(if blur_sup { 140.0 } else { 70.0 }, 24.0)
        })
    }

    let view_runs = Rc::new(Cell::new(0));
    let memo_runs = Rc::new(Cell::new(0));
    let none_sup = Rc::new(Cell::new(false));
    let blur_sup = Rc::new(Cell::new(true));
    let mica_sup = Rc::new(Cell::new(true));
    let cleared_none = Rc::new(Cell::new(false));
    let cleared_blur = Rc::new(Cell::new(true));
    let mut rt = Runtime::new(
        State {
            view_runs: view_runs.clone(),
            memo_runs: memo_runs.clone(),
            none_sup: none_sup.clone(),
            blur_sup: blur_sup.clone(),
            mica_sup: mica_sup.clone(),
            cleared_none: cleared_none.clone(),
            cleared_blur: cleared_blur.clone(),
        },
        view,
        (200.0, 100.0),
        Env::default(),
    );

    rt.frame(0.0);
    assert_eq!(view_runs.get(), 1);
    assert_eq!(memo_runs.get(), 1);
    assert!(none_sup.get(), "WindowBackdrop::None is always supported");
    assert!(!blur_sup.get());
    assert!(!mica_sup.get());

    // Trying to mark None unsupported is a no-op.
    rt.set_window_backdrop_supported(WindowBackdrop::None, false);
    rt.frame(1.0);
    assert_eq!(view_runs.get(), 1);
    assert_eq!(memo_runs.get(), 1);
    assert!(none_sup.get());

    // Setting Blur supported rebuilds both view and memo even while WindowBackdrop::None is active.
    rt.set_window_backdrop_supported(WindowBackdrop::Blur, true);
    rt.frame(2.0);
    assert_eq!(view_runs.get(), 2);
    assert_eq!(memo_runs.get(), 2);
    assert!(blur_sup.get());
    assert!(!mica_sup.get());
    assert!(cleared_none.get());
    assert!(!cleared_blur.get(), "clear_chrome resets supported mask for staged subviews");

    // Setting the same value is a no-op.
    rt.set_window_backdrop_supported(WindowBackdrop::Blur, true);
    rt.frame(3.0);
    assert_eq!(view_runs.get(), 2);
    assert_eq!(memo_runs.get(), 2);

    // Revoking Blur support rebuilds view and memo again.
    rt.set_window_backdrop_supported(WindowBackdrop::Blur, false);
    rt.frame(4.0);
    assert_eq!(view_runs.get(), 3);
    assert_eq!(memo_runs.get(), 3);
    assert!(!blur_sup.get());
}

#[test]
fn translucent_blur_window_redraw_does_not_ghost_previous_text() {
    // Wayland translucent GPU windows draw whole frames with `LoadOp::Clear([0, 0, 0, 0])`
    // (Windows partial damage is not used on Wayland and now also requires `clear[3] == 1.0`).
    // Verify off-screen that updating text over a translucent `WindowBackdrop::Blur` fill
    // (`bg.alpha(0.75)`) leaves zero ghost pixels from the prior frame.
    let Some(gpu) = crate::render::Gpu::test_headless() else { return };
    let size = (240u32, 64u32);
    let mut rt = Runtime::new(
        "First frame text",
        |label: &&'static str, cx| {
            let bg = if cx.window_backdrop() == WindowBackdrop::Blur {
                Color { r: 0.12, g: 0.14, b: 0.18, a: 0.75 }
            } else {
                Color { r: 0.12, g: 0.14, b: 0.18, a: 1.0 }
            };
            el().w_full()
                .h_full()
                .fill(bg)
                .child(crate::text(*label).color(Color::WHITE))
        },
        (size.0 as f32, size.1 as f32),
        Env::new(true, 1.0),
    );
    rt.core_mut().transparent = true;
    rt.set_window_backdrop(WindowBackdrop::Blur);

    let mut renderer = crate::render::Renderer::new(&gpu.device, crate::render::OFFSCREEN_FORMAT);
    let target = crate::render::Offscreen::new(&gpu.device, size.0, size.1);
    let render_once = |rt: &mut Runtime<&'static str>, renderer: &mut crate::render::Renderer| {
        rt.frame(0.0);
        let clear = rt.clear_color();
        assert_eq!(clear, [0.0; 4]);
        let core = rt.core_mut();
        renderer.prepare_ui(&gpu.device, &gpu.queue, &mut core.text.atlas, &core.instances, size);
        let mut enc = gpu.device.create_command_encoder(&Default::default());
        renderer.draw(&mut enc, &target.view, clear);
        target.copy(&mut enc);
        gpu.queue.submit([enc.finish()]);
        target.read(&gpu.device)
    };

    let frame1 = render_once(&mut rt, &mut renderer);
    rt.update(|label, _| *label = "Second");
    let frame2 = render_once(&mut rt, &mut renderer);

    // Compare against a fresh single-frame render of "Second" so any ghosted pixels from
    // "First frame text" in the un-overwritten tail immediately fail equality.
    let mut fresh_rt = Runtime::new(
        "Second",
        |label: &&'static str, cx| {
            let bg = if cx.window_backdrop() == WindowBackdrop::Blur {
                Color { r: 0.12, g: 0.14, b: 0.18, a: 0.75 }
            } else {
                Color { r: 0.12, g: 0.14, b: 0.18, a: 1.0 }
            };
            el().w_full()
                .h_full()
                .fill(bg)
                .child(crate::text(*label).color(Color::WHITE))
        },
        (size.0 as f32, size.1 as f32),
        Env::new(true, 1.0),
    );
    fresh_rt.core_mut().transparent = true;
    fresh_rt.set_window_backdrop(WindowBackdrop::Blur);
    let mut fresh_renderer = crate::render::Renderer::new(&gpu.device, crate::render::OFFSCREEN_FORMAT);
    let expected = render_once(&mut fresh_rt, &mut fresh_renderer);

    assert_ne!(frame1, frame2, "frame content should change");
    assert_eq!(frame2, expected, "translucent blur redraw must not ghost prior frame pixels");
}
