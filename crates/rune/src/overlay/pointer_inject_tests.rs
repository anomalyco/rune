use std::time::Duration;

use super::{
    InputSource, MAX_QUEUED, OwnershipEffect, OwnershipEvent, PointerOwnership, PointerQueue,
    button_mask, screen_to_local,
};
use crate::overlay::{KeyPolicy, Overlay, OverlayOptions, OverlaySpec, Presence};
use crate::tooling::AppSpec;
use crate::{
    Cx, Drag, El, GesturePhase, Haptic, Mods, OverlayInput, PointerButton, PointerInput,
    ScrollSpec, Tx, WheelDelta, col, el,
};

fn step_vec(owner: &mut PointerOwnership, event: OwnershipEvent) -> Vec<OwnershipEffect> {
    owner.step(event).into_iter().collect()
}

#[test]
fn pointer_ownership_interleaving_rules_across_every_phase() {
    let mut o = PointerOwnership::default();
    assert_eq!(o, PointerOwnership::Idle);
    assert!(!o.is_injected());

    // In Idle or Hovering, the latest source takes ownership.
    assert_eq!(step_vec(&mut o, OwnershipEvent::Move(InputSource::Native)), [OwnershipEffect::Dispatch]);
    assert_eq!(o, PointerOwnership::Hovering(InputSource::Native));
    assert_eq!(step_vec(&mut o, OwnershipEvent::Move(InputSource::Injected)), [OwnershipEffect::Dispatch]);
    assert_eq!(o, PointerOwnership::Hovering(InputSource::Injected));
    assert!(o.is_injected());

    // Native Leave while Injected is hovering does not yank injected hover.
    assert!(step_vec(&mut o, OwnershipEvent::Leave(InputSource::Native)).is_empty());
    assert_eq!(o, PointerOwnership::Hovering(InputSource::Injected));

    // Injected Down captures until all injected buttons release; native events are ignored.
    assert_eq!(
        step_vec(&mut o, OwnershipEvent::Down { source: InputSource::Injected, button: PointerButton::Primary }),
        [OwnershipEffect::Dispatch]
    );
    assert_eq!(
        o,
        PointerOwnership::Captured { owner: InputSource::Injected, buttons: button_mask(PointerButton::Primary) }
    );
    assert!(o.captured_by_injected());
    for ignored in [
        OwnershipEvent::Move(InputSource::Native),
        OwnershipEvent::Down { source: InputSource::Native, button: PointerButton::Primary },
        OwnershipEvent::Up { source: InputSource::Native, button: PointerButton::Primary },
        OwnershipEvent::Wheel(InputSource::Native),
        OwnershipEvent::Leave(InputSource::Native),
        OwnershipEvent::Cancel(InputSource::Native),
    ] {
        assert!(step_vec(&mut o, ignored).is_empty(), "native {ignored:?} ignored during injected capture");
    }

    // Multi-button capture stays captured until the last button releases.
    assert_eq!(
        step_vec(&mut o, OwnershipEvent::Down { source: InputSource::Injected, button: PointerButton::Secondary }),
        [OwnershipEffect::Dispatch]
    );
    assert_eq!(
        step_vec(&mut o, OwnershipEvent::Up { source: InputSource::Injected, button: PointerButton::Primary }),
        [OwnershipEffect::Dispatch]
    );
    assert!(o.captured_by_injected());
    assert!(step_vec(&mut o, OwnershipEvent::Move(InputSource::Native)).is_empty());
    assert_eq!(
        step_vec(&mut o, OwnershipEvent::Up { source: InputSource::Injected, button: PointerButton::Secondary }),
        [OwnershipEffect::Dispatch]
    );
    assert_eq!(o, PointerOwnership::Hovering(InputSource::Injected));

    // Once released, native input can take ownership and capture; injected is then ignored.
    assert_eq!(
        step_vec(&mut o, OwnershipEvent::Down { source: InputSource::Native, button: PointerButton::Primary }),
        [OwnershipEffect::Dispatch]
    );
    assert!(!o.is_injected());
    for ignored in [
        OwnershipEvent::Move(InputSource::Injected),
        OwnershipEvent::Down { source: InputSource::Injected, button: PointerButton::Primary },
        OwnershipEvent::Up { source: InputSource::Injected, button: PointerButton::Primary },
        OwnershipEvent::Wheel(InputSource::Injected),
        OwnershipEvent::Leave(InputSource::Injected),
        OwnershipEvent::Cancel(InputSource::Injected),
    ] {
        assert!(step_vec(&mut o, ignored).is_empty(), "injected {ignored:?} ignored during native capture");
    }
    assert_eq!(step_vec(&mut o, OwnershipEvent::Cancel(InputSource::Native)), [OwnershipEffect::CancelCapture]);
    assert_eq!(o, PointerOwnership::Idle);

    // Reset (hide/fade) always returns Idle.
    step_vec(&mut o, OwnershipEvent::Down { source: InputSource::Injected, button: PointerButton::Primary });
    assert!(step_vec(&mut o, OwnershipEvent::Reset).is_empty());
    assert_eq!(o, PointerOwnership::Idle);
}

#[test]
fn pointer_queue_coalesces_moves_rejects_non_finite_and_bounds_overflow() {
    let mut q = PointerQueue::default();
    // Non-finite events are ignored and do not wake.
    for bad in [
        PointerInput::move_to(f32::NAN, 1.0),
        PointerInput::move_to(1.0, f32::INFINITY),
        PointerInput::down(f32::NEG_INFINITY, 0.0, PointerButton::Primary),
        PointerInput::up(0.0, f32::NAN, PointerButton::Primary),
        PointerInput::wheel_lines(0.0, 0.0, f32::NAN, -1.0),
        PointerInput::wheel_points(0.0, 0.0, 0.0, f32::INFINITY),
    ] {
        assert!(!q.push(bad));
    }
    assert_eq!(q.len(), 0);

    // Empty-to-non-empty wakes once; consecutive moves coalesce in place.
    assert!(q.push(PointerInput::move_to(1.0, 2.0)));
    assert!(!q.push(PointerInput::move_to(3.0, 4.0)));
    assert!(!q.push(PointerInput::move_to(5.0, 6.0)));
    assert_eq!(q.len(), 1);
    // Moves never coalesce across Down/Up/Wheel/Leave/Cancel.
    assert!(!q.push(PointerInput::down(5.0, 6.0, PointerButton::Primary)));
    assert!(!q.push(PointerInput::move_to(10.0, 12.0)));
    assert!(!q.push(PointerInput::move_to(14.0, 16.0)));
    assert!(!q.push(PointerInput::up(14.0, 16.0, PointerButton::Primary)));
    let (batch, n) = q.drain_to_array().unwrap();
    assert_eq!(
        &batch[..n],
        &[
            PointerInput::move_to(5.0, 6.0),
            PointerInput::down(5.0, 6.0, PointerButton::Primary),
            PointerInput::move_to(14.0, 16.0),
            PointerInput::up(14.0, 16.0, PointerButton::Primary),
        ]
    );
    assert!(q.drain_to_array().is_none());

    // Overflow prefers dropping the oldest Move so Down/Up pairs survive.
    assert!(q.push(PointerInput::move_to(0.0, 0.0)));
    for i in 0..(MAX_QUEUED - 1) {
        let btn = if i % 2 == 0 { PointerInput::down(1.0, 1.0, PointerButton::Primary) } else { PointerInput::up(1.0, 1.0, PointerButton::Primary) };
        assert!(!q.push(btn));
    }
    assert_eq!(q.len(), MAX_QUEUED);
    assert!(!q.push(PointerInput::Leave));
    assert_eq!(q.len(), MAX_QUEUED);
    let (batch, n) = q.drain_to_array().unwrap();
    assert_eq!(n, MAX_QUEUED);
    assert!(matches!(batch[0], PointerInput::Down { .. }), "oldest Move dropped before Down");
    assert_eq!(batch[n - 1], PointerInput::Leave);

    // When full of non-moves, overflow drops the oldest entry cleanly without panicking.
    for i in 0..(MAX_QUEUED + 10) {
        q.push(PointerInput::wheel_points(i as f32, 0.0, 0.0, -1.0));
    }
    let (batch, n) = q.drain_to_array().unwrap();
    assert_eq!(n, MAX_QUEUED);
    assert_eq!(batch[0], PointerInput::wheel_points(10.0, 0.0, 0.0, -1.0));
    assert_eq!(batch[n - 1], PointerInput::wheel_points((MAX_QUEUED + 9) as f32, 0.0, 0.0, -1.0));
}

#[test]
fn screen_to_local_handles_scales_negative_monitors_and_hidden_none() {
    assert_eq!(screen_to_local(None, (100, 200)), None);
    assert_eq!(screen_to_local(Some(((0, 0), 0.0)), (10, 10)), None);
    assert_eq!(screen_to_local(Some(((0, 0), -1.0)), (10, 10)), None);
    assert_eq!(screen_to_local(Some(((0, 0), f32::NAN)), (10, 10)), None);

    // 1x primary display.
    assert_eq!(screen_to_local(Some(((100, 200), 1.0)), (150, 240)), Some((50.0, 40.0)));
    // 1.5x fractional DPI Windows secondary monitor to the left (negative origin).
    assert_eq!(screen_to_local(Some(((-1920, -1080), 1.5)), (-1890, -1050)), Some((20.0, 20.0)));
    // 2x Retina display with out-of-bounds point (captured drag leaving overlay).
    assert_eq!(screen_to_local(Some(((500, 300), 2.0)), (476, 360)), Some((-12.0, 30.0)));

    // OverlayHandle::screen_to_local returns None while hidden, fading, or uncreated, and
    // stays accurate when content-sized overlays change size.
    fn view(_: &(), _: &mut Cx<()>) -> El { el().size(100.0, 40.0) }
    let app = AppSpec::new("s2l", (100.0, 40.0), || (), view);
    let (mut overlay, handle) = Overlay::new(OverlaySpec::new("s2l", app.size, view).options(Default::default()), (), &app);
    assert_eq!(handle.screen_to_local((100, 100)), None, "uncreated/hidden returns None");

    overlay.presence = Presence::Shown { occluded: false };
    overlay.set_test_screen_transform(Some((-1920, 40)), 1.25);
    assert_eq!(handle.screen_to_local((-1895, 65)), Some((20.0, 20.0)));
    // Content-sized resize does not break origin/scale mapping.
    overlay.at.size = (240.0, 120.0);
    assert_eq!(handle.screen_to_local((-1895, 65)), Some((20.0, 20.0)));

    // Hiding or fading out clears screen_to_local back to None.
    overlay.hide_fading(Duration::from_millis(120));
    assert_eq!(handle.screen_to_local((-1895, 65)), None);
}

struct Trace {
    zoom: f32,
    events: Vec<String>,
    scroll_y: std::rc::Rc<std::cell::Cell<f32>>,
    magnify: f32,
    had_input_time: bool,
}

impl Trace {
    fn new(zoom: f32) -> Self {
        Self { zoom, events: Vec::new(), scroll_y: std::rc::Rc::new(std::cell::Cell::new(0.0)), magnify: 0.0, had_input_time: false }
    }
}

fn parity_view(s: &Trace, cx: &mut Cx<Trace>) -> El {
    cx.zoom(s.zoom);
    let scroll_y = s.scroll_y.clone();
    let target = el()
        .key("target")
        .size(120.0, 60.0)
        .hit()
        .pointer_buttons([PointerButton::Secondary])
        .on_hover(|t: &mut Trace| t.events.push("hover".into()))
        .on_hover_change(|t: &mut Trace, on, _| t.events.push(format!("hover_change:{on}")))
        .on_press_tx(|t: &mut Trace, tx| {
            t.had_input_time |= tx.input_time().is_some();
            t.events.push("press".into());
        })
        .on_double_press(|t: &mut Trace| t.events.push("double_press".into()))
        .on_secondary_press(|t: &mut Trace, p, _| t.events.push(format!("sec:{:.1},{:.1}", p.x, p.y)))
        .on_button(|t: &mut Trace, btn, _| { t.events.push(format!("btn:{btn:?}")); true })
        .on_drag(|t: &mut Trace, d: &mut Drag, _| {
            t.events.push(format!("drag:{:?}:{:.1},{:.1}:c={}", d.phase, d.x, d.y, d.cancelled()));
        })
        .on_wheel(|t: &mut Trace, w, _| {
            t.events.push(format!("wheel:{:.1},{:.1}:p={}:n={}:ph={:?}", w.dx, w.dy, w.precise, w.notch, w.phase));
            false
        })
        .on_magnify(|t: &mut Trace, m, _| {
            t.magnify += m.delta;
            t.events.push(format!("magnify:{:.2}", m.delta));
            true
        });
    let scroller = col()
        .key("scroll")
        .size(120.0, 80.0)
        .scroll(ScrollSpec::default())
        .on_scroll_offset(move |pos| scroll_y.set(pos.offset[1]))
        .child(target)
        .child(el().size(120.0, 400.0));
    col().size(200.0, 140.0).child(scroller)
}

fn run_parity_sequence(injected: bool, zoom: f32) -> (Vec<String>, f32, f32, bool) {
    let app = AppSpec::new("parity", (200.0, 140.0), || Trace::new(1.0), parity_view);
    let spec = OverlaySpec::new("parity", app.size, parity_view)
        .options(OverlayOptions { input: OverlayInput::All, ..Default::default() });
    let (mut overlay, handle) = Overlay::new(spec, Trace::new(zoom), &app);
    overlay.presence = Presence::Shown { occluded: false };
    overlay.rt.wall_start = None;
    overlay.rt.set_viewport(app.size, 1.0);
    overlay.rt.frame(0.0);

    let mut send = |ev: PointerInput| {
        if injected {
            handle.send_pointer(ev);
            overlay.drain_injected_pointers();
        } else {
            let _stamp = crate::input::time::arrived_now();
            overlay.dispatch_pointer(InputSource::Native, ev);
        }
        overlay.rt.frame(1.0 / 60.0);
    };

    let pt = (30.0 * zoom, 20.0 * zoom);
    // 1. Hover enter & move.
    send(PointerInput::move_to(pt.0, pt.1));
    send(PointerInput::move_to(pt.0 + 4.0, pt.1 + 2.0));
    // 2. Single click + double click within slop and interval.
    send(PointerInput::down(pt.0, pt.1, PointerButton::Primary));
    send(PointerInput::up(pt.0, pt.1, PointerButton::Primary));
    send(PointerInput::down(pt.0, pt.1, PointerButton::Primary));
    send(PointerInput::up(pt.0, pt.1, PointerButton::Primary));
    // 3. Press, leave overlay bounds, return inside, release.
    send(PointerInput::down(pt.0, pt.1, PointerButton::Primary));
    send(PointerInput::move_to(260.0, 20.0));
    send(PointerInput::move_to(pt.0 + 10.0, pt.1));
    send(PointerInput::up(pt.0 + 10.0, pt.1, PointerButton::Primary));
    // 4. Secondary click & secondary drag.
    send(PointerInput::down(pt.0, pt.1, PointerButton::Secondary));
    send(PointerInput::up(pt.0, pt.1, PointerButton::Secondary));
    send(PointerInput::down(pt.0, pt.1, PointerButton::Secondary));
    send(PointerInput::move_to(pt.0 + 20.0, pt.1 + 10.0));
    send(PointerInput::up(pt.0 + 20.0, pt.1 + 10.0, PointerButton::Secondary));
    // 5. Extra buttons: Middle, Back, Forward.
    for btn in [PointerButton::Middle, PointerButton::Back, PointerButton::Forward] {
        send(PointerInput::down(pt.0, pt.1, btn));
        send(PointerInput::up(pt.0, pt.1, btn));
    }
    // 6. Wheel lines, wheel points, Shift+wheel, Ctrl+wheel magnify.
    send(PointerInput::wheel_lines(pt.0, pt.1, 0.0, -1.0));
    send(PointerInput::wheel_points(pt.0, pt.1, 0.0, -24.0));
    send(PointerInput::Wheel {
        x: pt.0, y: pt.1,
        delta: WheelDelta::Lines { dx: 0.0, dy: -1.0 },
        mods: Mods { shift: true, ..Default::default() },
        phase: GesturePhase::Moved,
    });
    send(PointerInput::Wheel {
        x: pt.0, y: pt.1,
        delta: WheelDelta::Lines { dx: 0.0, dy: 1.0 },
        mods: Mods { ctrl: true, cmd: !cfg!(target_os = "macos"), ..Default::default() },
        phase: GesturePhase::Moved,
    });
    // 7. Drag cancelled via Cancel, then Leave.
    send(PointerInput::down(pt.0, pt.1, PointerButton::Primary));
    send(PointerInput::move_to(pt.0 + 16.0, pt.1));
    send(PointerInput::Cancel);
    send(PointerInput::Leave);

    let s = &overlay.rt.state;
    (s.events.clone(), s.scroll_y.get(), s.magnify, s.had_input_time)
}

#[test]
fn native_and_injected_pointer_streams_produce_identical_handler_traces() {
    for zoom in [1.0, 1.5] {
        let native = run_parity_sequence(false, zoom);
        let injected = run_parity_sequence(true, zoom);
        assert_eq!(injected.0, native.0, "handler traces must match at zoom={zoom}");
        assert_eq!(injected.1, native.1, "scroll offsets must match at zoom={zoom}");
        assert_eq!(injected.2, native.2, "magnify deltas must match at zoom={zoom}");
        assert!(injected.3, "injected input stamps arrived_now() for Tx::input_time()");
        assert!(injected.0.iter().any(|e| e == "double_press"), "double click paired");
        assert!(injected.0.iter().any(|e| e.starts_with("sec:")), "secondary click fired");
        assert!(injected.0.iter().any(|e| e == "btn:Middle"), "middle button fired");
        assert!(injected.0.iter().any(|e| e.contains("c=true")), "Cancel delivered cancelled drag");
        assert!(injected.1 > 0.0, "wheel scrolled the ScrollSpec container");
        assert!(injected.2 != 0.0, "Ctrl+wheel triggered on_magnify");
    }
}

#[test]
fn injected_input_bypasses_click_through_and_drops_when_hidden_or_fading() {
    #[derive(Default)]
    struct S { clicks: u32, cancelled: u32 }
    fn view(_: &S, _: &mut Cx<S>) -> El {
        el().key("btn").size(80.0, 40.0).hit()
            .on_press(|s: &mut S| s.clicks += 1)
            .on_drag(|s: &mut S, d: &mut Drag, _| { if d.cancelled() { s.cancelled += 1; } })
    }
    for policy in [OverlayInput::None, OverlayInput::Regions] {
        let app = AppSpec::new("policy", (100.0, 60.0), S::default, view);
        let spec = OverlaySpec::new("policy", app.size, view)
            .options(OverlayOptions { input: policy, keys: KeyPolicy::None, ..Default::default() });
        let (mut overlay, handle) = Overlay::new(spec, S::default(), &app);
        overlay.rt.frame(0.0);

        // Hidden: events are dropped and never replay on later show.
        handle.send_pointer(PointerInput::down(20.0, 20.0, PointerButton::Primary));
        handle.send_pointer(PointerInput::up(20.0, 20.0, PointerButton::Primary));
        overlay.drain_injected_pointers();
        overlay.presence = Presence::Shown { occluded: false };
        overlay.drain_injected_pointers();
        overlay.rt.frame(0.0);
        assert_eq!(overlay.rt.state.clicks, 0, "{policy:?}: hidden events must not replay");

        // Shown with OverlayInput::None or Regions: injected click still hits.
        handle.send_pointer(PointerInput::move_to(20.0, 20.0));
        handle.send_pointer(PointerInput::down(20.0, 20.0, PointerButton::Primary));
        handle.send_pointer(PointerInput::up(20.0, 20.0, PointerButton::Primary));
        overlay.drain_injected_pointers();
        overlay.rt.frame(0.0);
        assert_eq!(overlay.rt.state.clicks, 1, "{policy:?}: injected click must bypass OS input policy");

        // Mid-drag hide cancels capture, resets ownership to Idle, and drops queued events.
        handle.send_pointer(PointerInput::down(20.0, 20.0, PointerButton::Primary));
        overlay.drain_injected_pointers();
        overlay.rt.frame(0.0);
        assert!(overlay.core.pointer_owner.captured_by_injected());
        handle.send_pointer(PointerInput::move_to(40.0, 20.0));
        overlay.hide();
        overlay.rt.frame(0.0);
        assert_eq!(overlay.core.pointer_owner, PointerOwnership::Idle);
        assert_eq!(overlay.shared.core.pointers.lock().unwrap().len(), 0);
        assert_eq!(overlay.rt.state.cancelled, 1, "{policy:?}: hide cancels active injected drag");

        // FadingOut also drops injected events.
        overlay.presence = Presence::Shown { occluded: false };
        overlay.hide_fading(Duration::from_millis(150));
        handle.send_pointer(PointerInput::down(20.0, 20.0, PointerButton::Primary));
        handle.send_pointer(PointerInput::up(20.0, 20.0, PointerButton::Primary));
        overlay.drain_injected_pointers();
        overlay.rt.frame(0.0);
        assert_eq!(overlay.rt.state.clicks, 1, "{policy:?}: fading overlay drops injected input");
    }
}

#[test]
fn injected_press_refuses_window_drag_and_drag_out_suppresses_cursor_and_keeps_haptics() {
    #[derive(Default)]
    struct S { pressed: u32, dragged: u32 }
    fn view(_: &S, _: &mut Cx<S>) -> El {
        col().size(120.0, 80.0)
            .child(el().key("bar").size(120.0, 30.0).hit().window_drag())
            .child(
                el().key("card").size(120.0, 50.0).hit()
                    .drag_out(|_: &S| Some(crate::DragItem::new().text("item")))
                    .on_press_tx(|s: &mut S, tx: &mut Tx| { s.pressed += 1; tx.haptic(Haptic::Selection); })
                    .on_drag(|s: &mut S, _, _| s.dragged += 1),
            )
    }
    let app = AppSpec::new("refuse", (120.0, 80.0), S::default, view);
    let spec = OverlaySpec::new("refuse", app.size, view)
        .options(OverlayOptions { input: OverlayInput::All, ..Default::default() });
    let (mut overlay, handle) = Overlay::new(spec, S::default(), &app);
    overlay.core.test_window_backing = true;
    overlay.presence = Presence::Shown { occluded: false };
    overlay.rt.set_viewport(app.size, 1.0);
    overlay.rt.frame(0.0);

    // 1. Injected press + drag on a window_drag region refuses native window drag.
    handle.send_pointer(PointerInput::move_to(20.0, 15.0));
    handle.send_pointer(PointerInput::down(20.0, 15.0, PointerButton::Primary));
    handle.send_pointer(PointerInput::move_to(80.0, 15.0));
    handle.send_pointer(PointerInput::up(80.0, 15.0, PointerButton::Primary));
    overlay.drain_injected_pointers();
    overlay.rt.frame(0.0);
    assert_eq!(overlay.core.test_window_drags, 0, "injected press must never start native window_drag");
    assert_eq!(overlay.core.position_owner, super::super::input::PositionOwner::Managed);

    // 2. Injected press + drag on a drag_out + on_press element refuses OS drag_out,
    // keeps pointer_owner.is_injected() (suppressing apply_cursor), and runs in-app press/haptic.
    handle.send_pointer(PointerInput::move_to(20.0, 50.0));
    handle.send_pointer(PointerInput::down(20.0, 50.0, PointerButton::Primary));
    handle.send_pointer(PointerInput::move_to(80.0, 50.0));
    handle.send_pointer(PointerInput::up(80.0, 50.0, PointerButton::Primary));
    overlay.drain_injected_pointers();
    assert!(overlay.core.pointer_owner.is_injected(), "apply_cursor suppressed while injected owns pointer");
    overlay.rt.frame(0.0);
    assert!(overlay.rt.take_drag_out().is_none(), "injected drag must never start OS drag_out");
    assert_eq!(overlay.rt.state.pressed, 1, "in-app on_press still runs");
    assert_eq!(overlay.rt.take_haptics(), [Haptic::Selection], "handler haptics still work");

    // 3. Control: a native press + drag on the same window_drag bar (outside double-click slop)
    // still starts a native drag.
    overlay.dispatch_pointer(InputSource::Native, PointerInput::move_to(40.0, 15.0));
    overlay.dispatch_pointer(InputSource::Native, PointerInput::down(40.0, 15.0, PointerButton::Primary));
    overlay.dispatch_pointer(InputSource::Native, PointerInput::move_to(90.0, 15.0));
    overlay.dispatch_pointer(InputSource::Native, PointerInput::up(90.0, 15.0, PointerButton::Primary));
    assert_eq!(overlay.core.test_window_drags, 1, "native press still starts window_drag");
    assert!(!overlay.core.pointer_owner.is_injected(), "native input reclaimed pointer ownership");

    // Cross-thread Send + Sync check: another thread can call screen_to_local + send_pointer.
    overlay.set_test_screen_transform(Some((100, 100)), 1.0);
    let bg = handle.clone();
    std::thread::spawn(move || {
        let local = bg.screen_to_local((120, 120)).expect("transform published");
        assert_eq!(local, (20.0, 20.0));
        bg.send_pointer(PointerInput::move_to(local.0, local.1));
    }).join().unwrap();
    overlay.drain_injected_pointers();
}

#[test]
fn steady_state_send_pointer_moves_allocate_zero_heap_memory() {
    fn view(_: &(), _: &mut Cx<()>) -> El { el().key("box").size(100.0, 40.0).hit() }
    let app = AppSpec::new("alloc", (100.0, 40.0), || (), view);
    let (mut overlay, handle) = Overlay::new(OverlaySpec::new("alloc", app.size, view).options(Default::default()), (), &app);
    overlay.presence = Presence::Shown { occluded: false };
    overlay.rt.frame(0.0);
    // Warm one move/drain first.
    handle.send_pointer(PointerInput::move_to(10.0, 10.0));
    overlay.drain_injected_pointers();
    overlay.rt.frame(0.0);

    let before = crate::tooling::test_alloc::allocs();
    for i in 0..200 {
        handle.send_pointer(PointerInput::move_to((i % 90) as f32, 15.0));
    }
    overlay.drain_injected_pointers();
    assert_eq!(crate::tooling::test_alloc::allocs() - before, 0, "steady-state send_pointer moves must not allocate");
}
