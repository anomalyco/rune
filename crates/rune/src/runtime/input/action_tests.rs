//! `Tx::input_waiting`: in `poll`, whether user input is queued for this frame's handlers,
//! in headless `Runtime` calls and scripted runs just as in a live window, without setting
//! wall-clock `Tx::input_time` or counting layout/state observers as input.
use std::path::PathBuf;

use crate::file_dialog::FileDialogOptions;
use crate::{
    ClipboardContent, Cx, El, Env, Ime, Key, KeyEvent, MagnifyEvent, Mods, PhysicalInput,
    PhysicalKeyCode, PointerButton, RotateEvent, Runtime, TouchPhase, Tx, WheelEvent, el,
};

#[derive(Default)]
struct Seen {
    polls: Vec<bool>,
    handler_waiting: Vec<bool>,
    handler_times: Vec<Option<crate::time::Instant>>,
    wheels: usize,
    presses: usize,
    keys: usize,
    layout_calls: usize,
    show_box: bool,
}

fn view(s: &Seen, _: &mut Cx<Seen>) -> El {
    let mut root = el()
        .key("root")
        .size(200.0, 200.0)
        .on_window_focus(|_: &mut Seen, _, _| {})
        .on_wheel(|s: &mut Seen, _, tx: &mut Tx| {
            s.wheels += 1;
            s.handler_waiting.push(tx.input_waiting());
            s.handler_times.push(tx.input_time());
            true
        })
        .on_magnify(|s: &mut Seen, _, tx: &mut Tx| {
            s.handler_waiting.push(tx.input_waiting());
            s.handler_times.push(tx.input_time());
            true
        })
        .on_rotate(|s: &mut Seen, _, tx: &mut Tx| {
            s.handler_waiting.push(tx.input_waiting());
            s.handler_times.push(tx.input_time());
            true
        })
        .on_smart_magnify(|s: &mut Seen, _, _, tx: &mut Tx| {
            s.handler_waiting.push(tx.input_waiting());
            s.handler_times.push(tx.input_time());
            true
        })
        .on_key_up(|s: &mut Seen, _, tx: &mut Tx| {
            s.handler_waiting.push(tx.input_waiting());
            s.handler_times.push(tx.input_time());
            true
        })
        .on_drop_files(|s: &mut Seen, _, tx: &mut Tx| {
            s.handler_waiting.push(tx.input_waiting());
            s.handler_times.push(tx.input_time());
        })
        .child(
            el().key("target")
                .size(80.0, 80.0)
                .on_press_tx(|s: &mut Seen, tx: &mut Tx| {
                    s.presses += 1;
                    s.handler_waiting.push(tx.input_waiting());
                    s.handler_times.push(tx.input_time());
                })
                .on_secondary_press(|s: &mut Seen, _, tx: &mut Tx| {
                    s.handler_waiting.push(tx.input_waiting());
                    s.handler_times.push(tx.input_time());
                })
                .on_middle_press(|s: &mut Seen, tx: &mut Tx| {
                    s.handler_waiting.push(tx.input_waiting());
                    s.handler_times.push(tx.input_time());
                })
                .on_hover_change(|s: &mut Seen, _, tx: &mut Tx| {
                    s.handler_waiting.push(tx.input_waiting());
                    s.handler_times.push(tx.input_time());
                })
                .on_pointer_move_local(|s: &mut Seen, _, tx: &mut Tx| {
                    s.handler_waiting.push(tx.input_waiting());
                    s.handler_times.push(tx.input_time());
                })
                .on_layout_tx(|s: &mut Seen, _, _| {
                    s.layout_calls += 1;
                })
                .on_visible(|_: &mut Seen, _, _| {}),
        );
    if s.show_box {
        root = root.child(
            el().key("ephemeral")
                .size(40.0, 40.0)
                .on_hover_change(|_: &mut Seen, _, _| {}),
        );
    }
    root
}

fn app() -> Runtime<Seen> {
    let mut rt = Runtime::new(Seen::default(), view, (200.0, 200.0), Env::new(true, 1.0));
    rt.poll = Some(|s, _, tx| {
        s.polls.push(tx.input_waiting());
        false
    });
    rt.on_key = Some(|s, _, tx| {
        s.keys += 1;
        s.handler_waiting.push(tx.input_waiting());
        s.handler_times.push(tx.input_time());
        true
    });
    rt.frame(0.0);
    // Drain initial layout/visible observer callbacks before testing input.
    rt.frame(0.0);
    rt.state.polls.clear();
    rt.state.handler_waiting.clear();
    rt.state.handler_times.clear();
    rt
}

fn poll_step(rt: &mut Runtime<Seen>) -> bool {
    rt.frame(1.0 / 60.0);
    rt.state.polls.pop().expect("poll ran")
}

#[test]
fn headless_wheel_sets_input_waiting_in_poll() {
    let mut rt = app();
    assert!(!poll_step(&mut rt), "idle frame has no input waiting");

    rt.wheel_xy(20.0, 20.0, 0.0, -24.0);
    assert!(poll_step(&mut rt), "wheel_xy must set tx.input_waiting() in poll");
    assert_eq!(rt.state.wheels, 1);
    assert_eq!(rt.state.handler_waiting, vec![false], "input_waiting is false outside poll");
    assert_eq!(rt.state.handler_times, vec![None], "headless wheel keeps input_time None");

    rt.wheel_event(WheelEvent::new(0.0, -12.0).at(20.0, 20.0));
    assert!(poll_step(&mut rt), "wheel_event must set tx.input_waiting() in poll");
    assert_eq!(rt.state.wheels, 2);

    assert!(!poll_step(&mut rt), "following idle frame clears input_waiting");
}

#[test]
fn headless_input_helpers_set_input_waiting_in_poll() {
    let mut rt = app();
    let (x, y) = rt.find("target").unwrap();

    // Pointer move, press, secondary press, extra button, leave.
    rt.pointer_move(x, y);
    assert!(poll_step(&mut rt), "pointer_move");
    assert!(!poll_step(&mut rt), "idle after pointer_move");

    rt.pointer_down(x, y);
    rt.pointer_up(x, y);
    assert!(poll_step(&mut rt), "pointer_down/up");
    assert_eq!(rt.state.presses, 1);

    rt.secondary_down(x, y);
    rt.secondary_up(x, y);
    assert!(poll_step(&mut rt), "secondary_down/up");

    rt.button_down(x, y, PointerButton::Middle);
    rt.button_up(x, y, PointerButton::Middle);
    assert!(poll_step(&mut rt), "button_down/up");

    rt.pointer_leave();
    assert!(poll_step(&mut rt), "pointer_leave");

    // Keyboard, physical key, key up, IME, paste.
    let down = KeyEvent { key: Key::Down, mods: Mods::default() };
    rt.key(down.clone());
    assert!(poll_step(&mut rt), "key");

    rt.repeat_key(down.clone());
    assert!(poll_step(&mut rt), "repeat_key");

    let _ = rt.key_up(down.clone());
    assert!(poll_step(&mut rt), "key_up");

    rt.key_with(down.clone(), PhysicalKeyCode::ArrowDown, None);
    assert!(poll_step(&mut rt), "key_with");

    let phys = PhysicalInput::Key { key: PhysicalKeyCode::KeyA, mods: Mods::default(), pressed: true, repeat: false };
    rt.physical_input(phys);
    assert!(poll_step(&mut rt), "physical_input");

    rt.physical_key_input(phys, down, Some('a'));
    assert!(poll_step(&mut rt), "physical_key_input");

    rt.ime(Ime::Commit("hi".into()));
    assert!(poll_step(&mut rt), "ime");

    rt.paste("hello");
    assert!(poll_step(&mut rt), "paste");

    rt.paste_content(ClipboardContent::new("rich"));
    assert!(poll_step(&mut rt), "paste_content");

    // Trackpad gestures.
    rt.magnify_event(MagnifyEvent::new(0.2).at(x, y));
    assert!(poll_step(&mut rt), "magnify_event");

    rt.rotate_event(RotateEvent::new(15.0).at(x, y));
    assert!(poll_step(&mut rt), "rotate_event");

    rt.smart_magnify(x, y);
    assert!(poll_step(&mut rt), "smart_magnify");

    // Touch tap.
    rt.touch(1, TouchPhase::Started, x, y, 1.0);
    rt.touch(1, TouchPhase::Ended, x, y, 1.05);
    assert!(poll_step(&mut rt), "touch");

    // File drag and drop.
    rt.drag_files(vec![PathBuf::from("a.txt")], Some((x, y)));
    assert!(poll_step(&mut rt), "drag_files");

    rt.drop_files(vec![PathBuf::from("a.txt")], Some((x, y)));
    assert!(poll_step(&mut rt), "drop_files");

    // File dialog answer.
    rt.update(|_, tx| tx.open_file(FileDialogOptions::default(), |_: &mut Seen, _, _| {}));
    rt.frame(1.0 / 60.0);
    rt.state.polls.clear();
    rt.resolve_file_dialog(None).unwrap();
    assert!(poll_step(&mut rt), "resolve_file_dialog");

    // Accessibility action.
    assert!(rt.accessibility_action("target", accesskit::Action::Click, None));
    assert!(poll_step(&mut rt), "accessibility_action");

    // Every handler ran with input_waiting == false and input_time == None.
    assert!(rt.state.handler_waiting.iter().all(|&w| !w), "handlers never see input_waiting");
    assert!(rt.state.handler_times.iter().all(|t| t.is_none()), "headless handlers keep input_time None");
    assert!(!poll_step(&mut rt), "idle frame after all inputs");
}

#[test]
fn scripted_input_sets_input_waiting_in_poll() {
    use crate::tooling::{Kind, script_input};

    let mut rt = app();
    let (x, y) = rt.find("target").unwrap();

    {
        let (core, mut driver) = rt.split_mut();
        assert!(script_input(core, &mut driver, &Kind::Wheel(0.0, -24.0), (x, y)));
    }
    assert!(poll_step(&mut rt), "scripted wheel");

    {
        let (core, mut driver) = rt.split_mut();
        assert!(script_input(core, &mut driver, &Kind::Key(KeyEvent { key: Key::Down, mods: Mods::default() }, false), (0.0, 0.0)));
    }
    assert!(poll_step(&mut rt), "scripted key");

    {
        let (core, mut driver) = rt.split_mut();
        assert!(script_input(core, &mut driver, &Kind::Down, (x, y)));
        assert!(script_input(core, &mut driver, &Kind::Up, (x, y)));
    }
    assert!(poll_step(&mut rt), "scripted press");
    assert!(!poll_step(&mut rt), "idle after scripted input");
}

#[test]
fn non_input_observers_keep_input_waiting_false() {
    let mut rt = Runtime::new(Seen::default(), view, (200.0, 200.0), Env::new(true, 1.0));
    rt.poll = Some(|s, _, tx| {
        s.polls.push(tx.input_waiting());
        false
    });
    // Initial frame queues Layout + Visible observers for the next frame.
    rt.frame(0.0);
    assert_eq!(rt.state.polls, vec![false], "first frame");
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.state.polls, vec![false, false], "layout/visible observer frame");
    assert!(rt.state.layout_calls > 0, "layout observer actually ran");

    // Window focus, finish_composition and FocusLost are not user input.
    rt.window_focus(false);
    rt.finish_composition();
    rt.physical_input(PhysicalInput::FocusLost);
    assert!(!poll_step(&mut rt), "focus loss and finish_composition are not user input");

    rt.window_focus(true);
    assert!(!poll_step(&mut rt), "window focus gain is not user input");

    // Stationary pointer re-hit after layout unmount (observe_since) is not user input.
    rt.update(|s, _| s.show_box = true);
    rt.frame(1.0 / 60.0);
    let (ex, ey) = rt.find("ephemeral").unwrap();
    rt.pointer_move(ex, ey);
    assert!(poll_step(&mut rt), "moving pointer onto ephemeral is input");
    rt.update(|s, _| s.show_box = false);
    // Unmounting queues HoverChange(false) via hover_pruned -> observe_since(queued).
    assert!(!poll_step(&mut rt), "unmount frame");
    assert!(!poll_step(&mut rt), "observed hover leave is not user input");
}
