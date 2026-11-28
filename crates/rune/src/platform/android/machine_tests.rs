//! Host-side tests for the Android lifecycle state machine: surface loss and recreation,
//! repeated Home/return background trips, rotation, insets and keyboard overlap without viewport
//! resize, dark mode, Back navigation, and Choreographer deduplication.

use super::machine::{Effect, Event, State, Surface, dispatch_back_escape, step};

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
fn back_press_dispatches_and_schedules_frame_only_when_consumed() {
    let mut s = State::default();
    let _ = run(&mut s, Event::Resumed);
    let _ = run(&mut s, Event::SurfaceCreated { width_px: 1080, height_px: 2400, scale: 3.0, dark: true });
    let _ = run(&mut s, Event::Frame);
    let _ = run(&mut s, Event::FrameDone { animating: false, wake_after_secs: None });

    // Back always dispatches to the runtime regardless of focus state.
    assert_eq!(run(&mut s, Event::BackPressed), [Effect::DispatchBack]);
    // Unconsumed Back schedules no frame (Activity is about to leave).
    assert!(run(&mut s, Event::BackDone { consumed: false }).is_empty());

    // Consumed Back schedules a redraw so the dismissed/blurred UI presents.
    assert_eq!(run(&mut s, Event::BackPressed), [Effect::DispatchBack]);
    assert_eq!(run(&mut s, Event::BackDone { consumed: true }), [Effect::ScheduleFrame]);

    // Already-scheduled frame is not duplicated if another consumed Back arrives before vsync.
    assert_eq!(run(&mut s, Event::BackPressed), [Effect::DispatchBack]);
    assert!(run(&mut s, Event::BackDone { consumed: true }).is_empty());

    // While paused, consumed Back records a pending wake instead of scheduling a frame.
    let _ = run(&mut s, Event::Paused);
    s.pending_wake = false;
    assert_eq!(run(&mut s, Event::BackPressed), [Effect::DispatchBack]);
    assert!(run(&mut s, Event::BackDone { consumed: true }).is_empty());
    assert!(s.pending_wake);
    assert_eq!(run(&mut s, Event::Resumed), [Effect::ScheduleFrame]);

    // After Destroy, Back emits nothing.
    let _ = run(&mut s, Event::Destroy);
    assert!(run(&mut s, Event::BackPressed).is_empty());
}

#[test]
fn back_closes_unfocused_panel_then_leaves_app() {
    use crate::{Cx, El, Env, Key, Runtime, col, el};

    struct PanelApp {
        panel_open: bool,
    }
    fn view(s: &PanelApp, _cx: &mut Cx<PanelApp>) -> El {
        let mut root = col().size(360.0, 800.0);
        if s.panel_open {
            root = root.child(el().key("panel").size(300.0, 200.0));
        }
        root
    }

    let mut rt = Runtime::new(PanelApp { panel_open: true }, view, (360.0, 800.0), Env::new(true, 3.0));
    rt.on_key = Some(|s, ev, _| {
        if ev.key == Key::Escape && s.panel_open {
            s.panel_open = false;
            return true;
        }
        false
    });
    rt.frame(0.0);
    assert!(!rt.has_focus(), "panel holds no focus");

    // First Back closes the unfocused panel and is consumed.
    assert!(dispatch_back_escape(&mut rt), "unfocused panel closed by Escape must consume Back");
    assert!(!rt.state.panel_open);

    // Second Back has nothing left to close and falls through to Android.
    assert!(!dispatch_back_escape(&mut rt), "once panel is closed, Back must leave the app");
}

#[test]
fn back_does_not_trap_when_tapped_button_keeps_focus() {
    use crate::{Cx, El, Env, Runtime, col, el};

    struct ButtonApp {
        clicks: usize,
    }
    fn view(_s: &ButtonApp, _cx: &mut Cx<ButtonApp>) -> El {
        col().size(360.0, 800.0).child(
            el().key("btn")
                .role("button")
                .label("Action")
                .focusable(true)
                .size(120.0, 44.0)
                .on_press(|s: &mut ButtonApp| s.clicks += 1),
        )
    }

    let mut rt = Runtime::new(ButtonApp { clicks: 0 }, view, (360.0, 800.0), Env::new(true, 3.0));
    rt.frame(0.0);

    let (x, y) = rt.find("btn").expect("button present");
    rt.pointer_down(x, y);
    rt.pointer_up(x, y);
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.state.clicks, 1);
    assert!(rt.has_focus(), "tapped button keeps focus");

    // A focused button has no use for Escape, so Back must NOT be consumed.
    assert!(!dispatch_back_escape(&mut rt), "focused button must not trap Back inside the app");
    assert!(rt.has_focus(), "button keeps focus when Back falls through");
}

#[test]
fn back_closes_modal_dialog_then_leaves_app_even_with_opener_focused() {
    use crate::{Cx, El, Env, Runtime, col, el};

    struct DialogApp {
        open: bool,
    }
    fn view(s: &DialogApp, _cx: &mut Cx<DialogApp>) -> El {
        let mut root = col().size(360.0, 800.0).child(
            el().key("opener")
                .role("button")
                .label("Open dialog")
                .focusable(true)
                .size(120.0, 44.0)
                .on_press(|s: &mut DialogApp| s.open = true),
        );
        if s.open {
            root = root.child(
                el().key("dialog")
                    .size(280.0, 180.0)
                    .modal(true, |s: &mut DialogApp| s.open = false),
            );
        }
        root
    }

    let mut rt = Runtime::new(DialogApp { open: false }, view, (360.0, 800.0), Env::new(true, 3.0));
    rt.frame(0.0);

    let (x, y) = rt.find("opener").expect("opener present");
    rt.pointer_down(x, y);
    rt.pointer_up(x, y);
    rt.frame(1.0 / 60.0);
    assert!(rt.state.open);

    // First Back dismisses the modal dialog.
    assert!(dispatch_back_escape(&mut rt), "Back must dismiss the active modal");
    assert!(!rt.state.open);
    assert!(rt.has_focus(), "dismissing the modal restores focus to the opener button");

    // Second Back leaves the app even though the opener button still holds focus.
    assert!(!dispatch_back_escape(&mut rt), "next Back must leave the app with the button still focused");
}

#[test]
fn back_blurs_focused_text_field_before_leaving_app() {
    use crate::{Cx, El, Env, Runtime, TextEdit, col, text_input};

    struct FieldApp {
        note: TextEdit,
    }
    fn view(s: &FieldApp, _cx: &mut Cx<FieldApp>) -> El {
        col().size(360.0, 800.0).child(
            text_input(&s.note, crate::tok::type_::BODY)
                .key("note")
                .label("Note")
                .size(240.0, 44.0)
                .on_edit(|s: &mut FieldApp| &mut s.note),
        )
    }

    let mut rt = Runtime::new(FieldApp { note: TextEdit::new("") }, view, (360.0, 800.0), Env::new(true, 3.0));
    rt.frame(0.0);

    let (x, y) = rt.find("note").expect("text field present");
    rt.pointer_down(x, y);
    rt.pointer_up(x, y);
    rt.frame(1.0 / 60.0);
    assert!(rt.has_focus());

    // First Back blurs the focused text field.
    assert!(dispatch_back_escape(&mut rt), "first Back must blur the focused text field");
    assert!(!rt.has_focus());

    // Second Back leaves the app.
    assert!(!dispatch_back_escape(&mut rt), "second Back must leave the app once the field is blurred");
}

#[test]
fn back_cancels_active_drag_before_leaving_app() {
    use crate::{Cx, Drag, El, Env, Phase, Runtime, col, el};

    struct DragApp {
        events: Vec<(Phase, bool)>,
    }
    fn view(_s: &DragApp, _cx: &mut Cx<DragApp>) -> El {
        col().size(360.0, 800.0).child(
            el().key("slider")
                .size(200.0, 44.0)
                .on_drag(|s: &mut DragApp, d: &mut Drag, _| s.events.push((d.phase, d.cancelled))),
        )
    }

    let mut rt = Runtime::new(DragApp { events: Vec::new() }, view, (360.0, 800.0), Env::new(true, 3.0));
    rt.frame(0.0);

    let (x, y) = rt.find("slider").expect("slider present");
    rt.pointer_down(x, y);
    rt.pointer_move(x + 20.0, y);
    rt.frame(1.0 / 60.0);

    // First Back cancels the in-flight drag and is consumed.
    assert!(dispatch_back_escape(&mut rt), "Back during an active drag must cancel the drag");
    assert_eq!(rt.state.events.last(), Some(&(Phase::Up, true)));

    // Second Back leaves the app.
    assert!(!dispatch_back_escape(&mut rt), "Back after drag cancellation must leave the app");
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
