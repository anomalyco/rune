//! `Tx::input_time`: each handler sees its own input's arrival time, and only input handlers
//! see one.
use std::time::Duration;

use super::*;
use crate::{Cx, El, Env, Key, KeyEvent, Mods, PhysicalInput, PhysicalKeyCode, Runtime, Tx, el};

#[test]
fn an_event_timestamp_is_the_instant_that_long_before_the_uptime_read() {
    let now = crate::time::now() + Duration::from_secs(10);
    assert_eq!(since_boot(now, 1000.25, 1000.0), now - Duration::from_millis(250));
    assert_eq!(since_boot(now, 5.5, 2.0), now - Duration::from_millis(3500));
    assert_eq!(since_boot(now, 1000.0, 1000.0), now, "stamped as the uptime was read");
    assert_eq!(since_boot(now, 1000.0, 1000.5), now, "a stamp after the read is now");
    assert_eq!(since_boot(now, 1000.0, f64::NAN), now);
}

#[derive(Default)]
struct Seen {
    keys: Vec<(Key, Option<Instant>)>,
    presses: Vec<Option<Instant>>,
    polls: Vec<Option<Instant>>,
}

fn view(_: &Seen, _: &mut Cx<Seen>) -> El {
    el().size(100.0, 100.0)
        .child(el().key("button").size(40.0, 40.0).on_press_tx(|s: &mut Seen, tx: &mut Tx| s.presses.push(tx.input_time())))
}

fn app() -> Runtime<Seen> {
    let mut rt = Runtime::new(Seen::default(), view, (100.0, 100.0), Env::new(true, 1.0));
    rt.on_key = Some(|s, e, tx| { s.keys.push((e.key.clone(), tx.input_time())); true });
    rt.frame(0.0);
    rt
}

fn physical(code: PhysicalKeyCode, key: Key) -> (PhysicalInput, Option<KeyEvent>) {
    (PhysicalInput::Key { key: code, mods: Mods::default(), pressed: true, repeat: false }, Some(KeyEvent { key, mods: Mods::default() }))
}

#[test]
fn input_that_piles_up_between_frames_keeps_each_event_s_time() {
    let mut rt = app();
    let t0 = crate::time::now();
    let [t1, t2, t3] = [1, 2, 3].map(|ms| t0 + Duration::from_millis(ms));
    {
        let _input = scope(Some(t1));
        let (event, key) = physical(PhysicalKeyCode::ArrowDown, Key::Down);
        rt.physical_then(event, key, None, None);
    }
    {
        let _input = scope(Some(t2));
        let (event, key) = physical(PhysicalKeyCode::ArrowUp, Key::Up);
        rt.physical_then(event, key, None, None);
    }
    {
        let _input = scope(Some(t3));
        rt.pointer_move(20.0, 20.0);
        rt.pointer_down(20.0, 20.0);
        rt.pointer_up(20.0, 20.0);
    }
    rt.frame(0.0);
    assert_eq!(rt.state.keys, vec![(Key::Down, Some(t1)), (Key::Up, Some(t2))]);
    assert_eq!(rt.state.presses, vec![Some(t3)]);
}

#[test]
fn handlers_no_input_caused_see_no_input_time() {
    let mut rt = app();
    rt.poll = Some(|s, _, tx| { s.polls.push(tx.input_time()); false });
    {
        // A frame run while a host dispatches input still polls without its time.
        let _input = scope(Some(crate::time::now()));
        rt.frame(0.0);
    }
    assert_eq!(rt.state.polls, vec![None]);
    rt.update(|s, tx| s.polls.push(tx.input_time()));
    assert_eq!(rt.state.polls, vec![None, None], "an update");
    rt.key(KeyEvent { key: Key::Down, mods: Mods::default() });
    rt.frame(0.0);
    assert_eq!(rt.state.keys, vec![(Key::Down, None)], "input a program feeds the runtime itself");
    assert_eq!(current(), None, "every scope restored what it held");
}

#[test]
fn window_events_classify_every_live_user_input_kind() {
    use std::path::PathBuf;
    use winit::dpi::{PhysicalPosition, PhysicalSize};
    use winit::event::{
        DeviceId, ElementState, Ime as WinitIme, Modifiers, MouseButton, MouseScrollDelta, Touch,
        TouchPhase as WinitTouchPhase, WindowEvent,
    };

    let device = DeviceId::dummy();
    let touch = |phase| WindowEvent::Touch(Touch {
        device_id: device,
        phase,
        location: PhysicalPosition::new(10.0, 20.0),
        force: None,
        id: 1,
    });

    let stamped = [
        ("touch started", touch(WinitTouchPhase::Started)),
        ("touch moved", touch(WinitTouchPhase::Moved)),
        ("touch ended", touch(WinitTouchPhase::Ended)),
        ("touch cancelled", touch(WinitTouchPhase::Cancelled)),
        ("hovered file", WindowEvent::HoveredFile(PathBuf::from("a.txt"))),
        ("dropped file", WindowEvent::DroppedFile(PathBuf::from("a.txt"))),
        ("hovered file cancelled", WindowEvent::HoveredFileCancelled),
        ("ime commit", WindowEvent::Ime(WinitIme::Commit("hi".into()))),
        ("modifiers changed", WindowEvent::ModifiersChanged(Modifiers::default())),
        ("mouse input", WindowEvent::MouseInput { device_id: device, state: ElementState::Pressed, button: MouseButton::Left }),
        ("cursor moved", WindowEvent::CursorMoved { device_id: device, position: PhysicalPosition::new(5.0, 5.0) }),
        ("cursor left", WindowEvent::CursorLeft { device_id: device }),
        ("mouse wheel", WindowEvent::MouseWheel { device_id: device, delta: MouseScrollDelta::LineDelta(0.0, 1.0), phase: WinitTouchPhase::Moved }),
        ("pinch gesture", WindowEvent::PinchGesture { device_id: device, delta: 0.1, phase: WinitTouchPhase::Moved }),
        ("rotation gesture", WindowEvent::RotationGesture { device_id: device, delta: 1.0, phase: WinitTouchPhase::Moved }),
        ("double tap gesture", WindowEvent::DoubleTapGesture { device_id: device }),
    ];
    for (label, event) in &stamped {
        let guard = arrived(event);
        assert!(guard.is_some(), "{label} must open an arrival-time scope");
        assert!(current().is_some(), "{label} must expose current() while scoped");
        assert!(last_input().is_some(), "{label} must record last_input()");
        drop(guard);
        assert_eq!(current(), None, "{label} must restore current() on drop");
    }

    let unstamped = [
        ("focused true", WindowEvent::Focused(true)),
        ("focused false", WindowEvent::Focused(false)),
        ("resized", WindowEvent::Resized(PhysicalSize::new(100, 100))),
        ("moved", WindowEvent::Moved(PhysicalPosition::new(10, 10))),
        ("close requested", WindowEvent::CloseRequested),
        ("destroyed", WindowEvent::Destroyed),
        ("occluded", WindowEvent::Occluded(false)),
        ("theme changed", WindowEvent::ThemeChanged(winit::window::Theme::Dark)),
        ("redraw requested", WindowEvent::RedrawRequested),
        ("cursor entered", WindowEvent::CursorEntered { device_id: device }),
    ];
    for (label, event) in &unstamped {
        let guard = arrived(event);
        assert!(guard.is_none(), "{label} must not open an arrival-time scope");
        assert_eq!(current(), None, "{label} must leave current() None");
    }
}

#[test]
fn touch_tap_via_arrived_stamps_input_time_while_headless_stays_none() {
    use std::path::PathBuf;
    use winit::dpi::PhysicalPosition;
    use winit::event::{DeviceId, Touch, TouchPhase as WinitTouchPhase, WindowEvent};

    #[derive(Default)]
    struct State {
        press_times: Vec<Option<Instant>>,
        drop_times: Vec<Option<Instant>>,
        polls_waiting: Vec<bool>,
    }

    fn view(_: &State, _: &mut Cx<State>) -> El {
        el().key("root")
            .size(100.0, 100.0)
            .on_drop_files(|s: &mut State, _, tx: &mut Tx| s.drop_times.push(tx.input_time()))
            .child(
                el().key("btn")
                    .size(40.0, 40.0)
                    .on_press_tx(|s: &mut State, tx: &mut Tx| s.press_times.push(tx.input_time())),
            )
    }

    let mut rt = Runtime::new(State::default(), view, (100.0, 100.0), Env::new(true, 1.0));
    rt.poll = Some(|s, _, tx| {
        s.polls_waiting.push(tx.input_waiting());
        false
    });
    rt.frame(0.0);
    rt.state.polls_waiting.clear();

    // Live touch event dispatched through arrived(&WindowEvent::Touch).
    let start_ev = WindowEvent::Touch(Touch {
        device_id: DeviceId::dummy(),
        phase: WinitTouchPhase::Started,
        location: PhysicalPosition::new(20.0, 20.0),
        force: None,
        id: 1,
    });
    let end_ev = WindowEvent::Touch(Touch {
        device_id: DeviceId::dummy(),
        phase: WinitTouchPhase::Ended,
        location: PhysicalPosition::new(20.0, 20.0),
        force: None,
        id: 1,
    });
    {
        let _input = arrived(&start_ev);
        rt.touch(1, crate::TouchPhase::Started, 20.0, 20.0, 0.0);
    }
    {
        let _input = arrived(&end_ev);
        rt.touch(1, crate::TouchPhase::Ended, 20.0, 20.0, 0.05);
    }
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.state.press_times.len(), 1);
    assert!(rt.state.press_times[0].is_some(), "live touch tap must see Some(input_time)");
    assert_eq!(rt.state.polls_waiting.last(), Some(&true));

    // Direct headless touch and file drop stay None for input_time while still setting input_waiting.
    rt.touch(2, crate::TouchPhase::Started, 20.0, 20.0, 1.0);
    rt.touch(2, crate::TouchPhase::Ended, 20.0, 20.0, 1.05);
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.state.press_times.len(), 2);
    assert_eq!(rt.state.press_times[1], None, "headless touch keeps input_time None");
    assert_eq!(rt.state.polls_waiting.last(), Some(&true));

    rt.drop_files(vec![PathBuf::from("a.txt")], Some((20.0, 20.0)));
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.state.drop_times, vec![None], "headless drop_files keeps input_time None");
    assert_eq!(rt.state.polls_waiting.last(), Some(&true));
}

#[test]
fn web_dom_input_classification_covers_every_user_input_kind() {
    for kind in [
        "edit", "preedit", "selection", "secure", "secure_replace",
        "key", "paste", "a11y_press", "a11y_link", "leave", "wheel_page", "hover",
    ] {
        assert!(is_web_dom_input(kind), "web dom kind {kind:?} must stamp arrival time");
    }
    for kind in [
        "viewport", "dpr", "motion", "primary_pointer", "contrast",
        "focus", "visibility", "sync", "secondary_browser", "a11y_flush",
        "menu", "scroll", "scroll_x", "touch_hold_confirm",
    ] {
        assert!(!is_web_dom_input(kind), "web dom kind {kind:?} must not stamp arrival time");
    }
}
