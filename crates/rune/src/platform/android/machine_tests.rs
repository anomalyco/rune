//! Host-side tests for the Android lifecycle state machine: surface loss and recreation,
//! repeated Home/return background trips, rotation, insets and keyboard overlap without viewport
//! resize, dark mode, Back navigation, and Choreographer deduplication.

use super::machine::{Effect, Event, State, Surface, consumed_back, step};

fn run(state: &mut State, event: Event) -> Vec<Effect> {
    step(state, event).into_iter().collect()
}

fn has(effects: &[Effect], target: Effect) -> bool {
    effects.contains(&target)
}

#[test]
fn surface_loss_and_recreation_preserves_state_and_redraws() {
    let mut s = State::default();
    let e = run(&mut s, Event::Resumed);
    assert!(e.is_empty(), "resuming before surfaceCreated schedules no frame");

    let e = run(&mut s, Event::SurfaceCreated { width_px: 1080, height_px: 2400, scale: 3.0, dark: true });
    assert_eq!(e[0], Effect::AttachSurface { viewport: (360.0, 800.0), scale: 3.0 });
    assert!(has(&e, Effect::SetDark(true)));
    assert!(has(&e, Effect::ScheduleFrame));
    assert!(s.is_attached());

    // First frame draws and goes idle.
    assert_eq!(run(&mut s, Event::Frame), [Effect::DrawFrame]);
    assert!(run(&mut s, Event::FrameDone { animating: false, wake_after_secs: None }).is_empty());

    // Surface lost: detaches cleanly without destroying host.
    assert_eq!(run(&mut s, Event::SurfaceDestroyed), [Effect::DetachSurface]);
    assert_eq!(s.surface, Surface::Detached);
    assert!(!s.destroyed);

    // A stale Choreographer frame arriving after detach draws nothing.
    assert!(run(&mut s, Event::Frame).is_empty());

    // Surface recreated: reattaches and schedules a redraw immediately.
    let e = run(&mut s, Event::SurfaceCreated { width_px: 1080, height_px: 2400, scale: 3.0, dark: true });
    assert_eq!(e[0], Effect::AttachSurface { viewport: (360.0, 800.0), scale: 3.0 });
    assert!(has(&e, Effect::ScheduleFrame));
    assert_eq!(run(&mut s, Event::Frame), [Effect::DrawFrame]);
}

#[test]
fn twenty_background_home_return_trips_detach_and_reattach_cleanly() {
    let mut s = State::default();
    let _ = run(&mut s, Event::Resumed);
    let _ = run(&mut s, Event::InsetsChanged {
        safe_area_px: [96, 0, 72, 0],
        keyboard_px: 0,
        scale: 3.0,
    });
    let _ = run(&mut s, Event::SurfaceCreated { width_px: 1080, height_px: 2400, scale: 3.0, dark: false });
    assert_eq!(run(&mut s, Event::Frame), [Effect::DrawFrame]);
    let _ = run(&mut s, Event::FrameDone { animating: false, wake_after_secs: None });

    let mut detaches = 0usize;
    let mut attaches = 0usize;
    let mut draws = 0usize;

    for trip in 0..20 {
        // User presses Home: Paused then SurfaceDestroyed.
        let p = run(&mut s, Event::Paused);
        assert!(!has(&p, Effect::DrawFrame));
        let d = run(&mut s, Event::SurfaceDestroyed);
        assert_eq!(d, [Effect::DetachSurface], "trip {trip}: detach once");
        detaches += 1;

        // Background wakes while detached must not draw or schedule frames.
        assert!(run(&mut s, Event::Wake).is_empty());
        assert!(run(&mut s, Event::Frame).is_empty());

        // User returns to the app: Resumed then SurfaceCreated (or reversed order on some OEMs).
        if trip % 2 == 0 {
            assert!(run(&mut s, Event::Resumed).is_empty());
            let c = run(&mut s, Event::SurfaceCreated { width_px: 1080, height_px: 2400, scale: 3.0, dark: false });
            assert_eq!(c[0], Effect::AttachSurface { viewport: (360.0, 800.0), scale: 3.0 });
            assert!(has(&c, Effect::SetSafeArea([32.0, 0.0, 24.0, 0.0])), "trip {trip}: replays cached safe area");
            assert!(has(&c, Effect::ScheduleFrame));
            attaches += 1;
        } else {
            let c = run(&mut s, Event::SurfaceCreated { width_px: 1080, height_px: 2400, scale: 3.0, dark: false });
            assert_eq!(c[0], Effect::AttachSurface { viewport: (360.0, 800.0), scale: 3.0 });
            assert!(!has(&c, Effect::ScheduleFrame), "paused surface waits for Resumed");
            attaches += 1;
            let r = run(&mut s, Event::Resumed);
            assert_eq!(r, [Effect::ScheduleFrame]);
        }

        assert_eq!(run(&mut s, Event::Frame), [Effect::DrawFrame]);
        draws += 1;
        assert!(run(&mut s, Event::FrameDone { animating: false, wake_after_secs: None }).is_empty());
    }

    assert_eq!((detaches, attaches, draws), (20, 20, 20));
    assert!(!s.destroyed);
}

#[test]
fn rotation_resizes_surface_updates_insets_and_redraws() {
    let mut s = State::default();
    let _ = run(&mut s, Event::Resumed);
    let _ = run(&mut s, Event::SurfaceCreated { width_px: 1080, height_px: 2400, scale: 3.0, dark: true });
    let _ = run(&mut s, Event::Frame);
    let _ = run(&mut s, Event::FrameDone { animating: false, wake_after_secs: None });

    // Rotate portrait -> landscape.
    let r = run(&mut s, Event::SurfaceChanged { width_px: 2400, height_px: 1080, scale: 3.0 });
    assert_eq!(
        r,
        [
            Effect::ResizeSurface { viewport: (800.0, 360.0), scale: 3.0 },
            Effect::ScheduleFrame,
        ],
    );

    // Cutout moves to the left edge in landscape; frame already scheduled so no duplicate ScheduleFrame.
    let i = run(&mut s, Event::InsetsChanged {
        safe_area_px: [0, 0, 48, 96],
        keyboard_px: 0,
        scale: 3.0,
    });
    assert_eq!(i, [Effect::SetSafeArea([0.0, 0.0, 16.0, 32.0])]);

    assert_eq!(run(&mut s, Event::Frame), [Effect::DrawFrame]);
}

#[test]
fn keyboard_inset_updates_without_resizing_viewport_or_inflating_safe_area() {
    let mut s = State::default();
    let _ = run(&mut s, Event::Resumed);
    let _ = run(&mut s, Event::SurfaceCreated { width_px: 1080, height_px: 2400, scale: 3.0, dark: true });
    let _ = run(&mut s, Event::Frame);
    let _ = run(&mut s, Event::FrameDone { animating: false, wake_after_secs: None });

    // System bars + cutout arrive first.
    let e = run(&mut s, Event::InsetsChanged {
        safe_area_px: [96, 0, 72, 0],
        keyboard_px: 0,
        scale: 3.0,
    });
    assert_eq!(e, [Effect::SetSafeArea([32.0, 0.0, 24.0, 0.0]), Effect::ScheduleFrame]);
    let _ = run(&mut s, Event::Frame);
    let _ = run(&mut s, Event::FrameDone { animating: false, wake_after_secs: None });

    // Soft keyboard opens (900 px = 300 dp) under adjustNothing:
    // - safe_area stays [32, 0, 24, 0] (not folded!)
    // - keyboard_inset becomes 300.0
    // - NO ResizeSurface effect is emitted!
    let e = run(&mut s, Event::InsetsChanged {
        safe_area_px: [96, 0, 72, 0],
        keyboard_px: 900,
        scale: 3.0,
    });
    assert_eq!(e, [Effect::SetKeyboardInset(300.0), Effect::ScheduleFrame]);
    assert_eq!(s.safe_area, [32.0, 0.0, 24.0, 0.0]);
    assert_eq!(s.keyboard_inset, 300.0);
    assert!(!e.iter().any(|eff| matches!(eff, Effect::ResizeSurface { .. })));

    // Identical insets emit nothing.
    assert!(run(&mut s, Event::InsetsChanged {
        safe_area_px: [96, 0, 72, 0],
        keyboard_px: 900,
        scale: 3.0,
    }).is_empty());

    // Soft keyboard closes.
    let _ = run(&mut s, Event::Frame);
    let _ = run(&mut s, Event::FrameDone { animating: false, wake_after_secs: None });
    let e = run(&mut s, Event::InsetsChanged {
        safe_area_px: [96, 0, 72, 0],
        keyboard_px: 0,
        scale: 3.0,
    });
    assert_eq!(e, [Effect::SetKeyboardInset(0.0), Effect::ScheduleFrame]);
    assert_eq!(s.keyboard_inset, 0.0);
}

#[test]
fn dark_mode_updates_emit_set_dark_and_schedule_frame_only_on_change() {
    let mut s = State::default();
    let _ = run(&mut s, Event::Resumed);
    let _ = run(&mut s, Event::SurfaceCreated { width_px: 1080, height_px: 2400, scale: 3.0, dark: true });
    let _ = run(&mut s, Event::Frame);
    let _ = run(&mut s, Event::FrameDone { animating: false, wake_after_secs: None });

    assert!(run(&mut s, Event::DarkModeChanged(true)).is_empty());
    assert_eq!(run(&mut s, Event::DarkModeChanged(false)), [Effect::SetDark(false), Effect::ScheduleFrame]);
}

#[test]
fn back_press_sends_escape_when_focused_and_falls_through_when_unfocused() {
    let mut s = State::default();
    let _ = run(&mut s, Event::Resumed);
    let _ = run(&mut s, Event::SurfaceCreated { width_px: 1080, height_px: 2400, scale: 3.0, dark: true });
    let _ = run(&mut s, Event::Frame);
    let _ = run(&mut s, Event::FrameDone { animating: false, wake_after_secs: None });

    // Nothing focused -> not consumed -> Activity returns to launcher.
    let raw = step(&mut s, Event::BackPressed { has_focus: false });
    assert!(!consumed_back(&raw));
    assert!(raw.is_empty());

    // Something focused -> sends Escape and schedules redraw -> consumed.
    let raw = step(&mut s, Event::BackPressed { has_focus: true });
    assert!(consumed_back(&raw));
    let v: Vec<_> = raw.into_iter().collect();
    assert_eq!(v, [Effect::SendEscapeKey, Effect::ScheduleFrame]);
}

#[test]
fn choreographer_deduplicates_wakes_and_honors_animating_and_timers() {
    let mut s = State::default();
    let _ = run(&mut s, Event::Resumed);
    let _ = run(&mut s, Event::SurfaceCreated { width_px: 720, height_px: 1280, scale: 2.0, dark: true });

    // SurfaceCreated already scheduled a frame; extra wakes do not double-schedule.
    assert!(run(&mut s, Event::Wake).is_empty());
    assert!(run(&mut s, Event::Wake).is_empty());

    // Frame runs and reports active animation -> schedules next vsync immediately.
    assert_eq!(run(&mut s, Event::Frame), [Effect::DrawFrame]);
    assert_eq!(run(&mut s, Event::FrameDone { animating: true, wake_after_secs: Some(0.5) }), [Effect::ScheduleFrame]);

    // Animation settles with a timer due in 0.25 s -> schedules delayed wake.
    assert_eq!(run(&mut s, Event::Frame), [Effect::DrawFrame]);
    assert_eq!(run(&mut s, Event::FrameDone { animating: false, wake_after_secs: Some(0.25) }), [Effect::ScheduleWakeAfter(0.25)]);

    // Pausing while a frame is scheduled cancels it and replays on resume.
    assert_eq!(run(&mut s, Event::Wake), [Effect::ScheduleFrame]);
    assert_eq!(run(&mut s, Event::Paused), [Effect::CancelFrame]);
    assert_eq!(run(&mut s, Event::Resumed), [Effect::ScheduleFrame]);
}

#[test]
fn zero_size_surface_suspends_drawing_until_nonzero_and_destroy_shuts_down() {
    let mut s = State::default();
    let _ = run(&mut s, Event::Resumed);
    let e = run(&mut s, Event::SurfaceCreated { width_px: 0, height_px: 0, scale: 2.0, dark: true });
    assert!(!has(&e, Effect::ScheduleFrame), "zero-size surface must not schedule drawing");

    let e = run(&mut s, Event::SurfaceChanged { width_px: 720, height_px: 1280, scale: 2.0 });
    assert_eq!(
        e,
        [
            Effect::ResizeSurface { viewport: (360.0, 640.0), scale: 2.0 },
            Effect::ScheduleFrame,
        ],
    );

    // Destroy cancels pending frame, detaches surface, and emits Shutdown.
    assert_eq!(run(&mut s, Event::Destroy), [Effect::CancelFrame, Effect::DetachSurface, Effect::Shutdown]);
    assert!(s.destroyed);

    // Post-destroy events are ignored.
    assert!(run(&mut s, Event::Wake).is_empty());
    assert!(run(&mut s, Event::Resumed).is_empty());
}
