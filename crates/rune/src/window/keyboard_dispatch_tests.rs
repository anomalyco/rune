use std::time::Duration;

use super::{KeyboardDispatch, step_immediate_core};
use crate::frame::clock::{FrameClock, RESUME_STEP};
use crate::input::host_events::{KeyInput, ime_core, keyboard_core};
use crate::runtime::Runtime;
use crate::{El, Env, Ime, Key, KeyEvent, Mods, PhysicalKeyCode, TextEdit, TextStyle, col, el, text_input};

fn step_rt<S: 'static>(
    policy: KeyboardDispatch,
    rt: &mut Runtime<S>,
    clock: &mut FrameClock,
    now: crate::time::Instant,
    compositing: bool,
) -> bool {
    let (core, mut app) = rt.split_mut();
    step_immediate_core(policy, core, &mut app, clock, now, compositing)
}

fn press_char_input(ch: char, code: PhysicalKeyCode) -> KeyInput {
    let s = ch.to_string();
    KeyInput {
        physical: Some(code),
        logical: Some(KeyEvent { key: Key::Text(s.clone()), mods: Mods::default() }),
        base: Some(ch),
        mods: Mods::default(),
        pressed: true,
        repeat: false,
        unhandled: None,
        text: Some(s),
        consumed_mods: Mods::default(),
    }
}

#[test]
fn keyboard_dispatch_defaults_to_batched() {
    assert_eq!(KeyboardDispatch::default(), KeyboardDispatch::Batched);
    assert!(!KeyboardDispatch::Batched.is_immediate());
    assert!(KeyboardDispatch::Immediate.is_immediate());
}

#[test]
fn parse_env_accepts_documented_values_and_rejects_unknown() {
    for raw in ["immediate", "IMMEDIATE", " 1 ", "true", "TRUE", "yes", "on", "On"] {
        assert_eq!(KeyboardDispatch::parse_env(raw), Some(KeyboardDispatch::Immediate), "raw={raw:?}");
    }
    for raw in ["batched", "BATCHED", " 0 ", "false", "FALSE", "no", "off", "Off"] {
        assert_eq!(KeyboardDispatch::parse_env(raw), Some(KeyboardDispatch::Batched), "raw={raw:?}");
    }
    for raw in ["", "   ", "2", "auto", "sync", "bogus"] {
        assert_eq!(KeyboardDispatch::parse_env(raw), None, "raw={raw:?}");
    }
}

#[test]
fn resolve_env_override_and_kill_switch() {
    for configured in [KeyboardDispatch::Batched, KeyboardDispatch::Immediate] {
        assert_eq!(configured.resolve(None), configured);
        assert_eq!(configured.resolve(Some("")), configured);
        assert_eq!(configured.resolve(Some("unknown")), configured);
        for raw in ["immediate", "1", "true", "yes", "on"] {
            assert_eq!(configured.resolve(Some(raw)), KeyboardDispatch::Immediate);
        }
        // Kill switch disables even an app built with KeyboardDispatch::Immediate.
        for raw in ["batched", "0", "false", "no", "off"] {
            assert_eq!(configured.resolve(Some(raw)), KeyboardDispatch::Batched);
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
#[ignore = "invoked by effective_reads_process_env_override_without_mutating_parent"]
fn effective_env_child() {
    let expected = match crate::env::var("RUNE_TEST_KBD_DISPATCH_CASE").as_deref() {
        Some("unset") | Some("bogus") => (KeyboardDispatch::Batched, KeyboardDispatch::Immediate),
        Some("immediate") | Some("one") | Some("on") | Some("true") | Some("yes") => {
            (KeyboardDispatch::Immediate, KeyboardDispatch::Immediate)
        }
        Some("batched") | Some("zero") | Some("off") | Some("false") | Some("no") => {
            (KeyboardDispatch::Batched, KeyboardDispatch::Batched)
        }
        other => panic!("unexpected test case: {other:?}"),
    };
    assert_eq!(KeyboardDispatch::Batched.effective(), expected.0);
    assert_eq!(KeyboardDispatch::Immediate.effective(), expected.1);
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
fn effective_reads_process_env_override_without_mutating_parent() {
    let exe = std::env::current_exe().expect("test binary path");
    for (case, env_value) in [
        ("unset", None),
        ("immediate", Some("immediate")),
        ("one", Some("1")),
        ("on", Some("on")),
        ("true", Some("TRUE")),
        ("yes", Some("yes")),
        ("batched", Some("batched")),
        ("zero", Some("0")),
        ("off", Some("off")),
        ("false", Some("false")),
        ("no", Some("NO")),
        ("bogus", Some("bogus")),
    ] {
        let mut cmd = std::process::Command::new(&exe);
        cmd.arg("--exact")
            .arg("window::keyboard_dispatch::tests::effective_env_child")
            .arg("--ignored")
            .arg("--nocapture")
            .env("RUNE_TEST_KBD_DISPATCH_CASE", case);
        match env_value {
            Some(v) => { cmd.env(KeyboardDispatch::ENV_VAR, v); }
            None => { cmd.env_remove(KeyboardDispatch::ENV_VAR); }
        }
        let output = cmd.output().expect("spawn keyboard_dispatch env child");
        assert!(
            output.status.success(),
            "case={case} env={env_value:?} failed:\nstdout: {}\nstderr: {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
        );
    }
}

#[test]
fn batched_policy_leaves_queued_keys_for_next_frame() {
    let mut rt = Runtime::new(
        Vec::<Key>::new(),
        |_, _| el().size(200.0, 100.0),
        (200.0, 100.0),
        Env::default(),
    );
    rt.on_key = Some(|keys, ev, _| {
        keys.push(ev.key.clone());
        true
    });
    let t0 = crate::time::now();
    let mut clock = FrameClock::new(t0);
    clock.advance(&mut rt, t0, false);

    let queued = {
        let (core, mut app) = rt.split_mut();
        keyboard_core(core, &mut app, press_char_input('a', PhysicalKeyCode::KeyA))
    };
    assert!(queued);
    assert!(rt.has_input_waiting());
    assert!(!step_rt(KeyboardDispatch::Batched, &mut rt, &mut clock, t0 + Duration::from_millis(2), false));
    assert!(rt.state.is_empty(), "Batched policy must not drain before the next frame");
    assert!(rt.has_input_waiting(), "Batched policy keeps input queued for the next frame");

    clock.advance(&mut rt, t0 + Duration::from_millis(16), false);
    assert_eq!(rt.state, vec![Key::Text("a".into())]);
    assert!(!rt.has_input_waiting());
}

#[test]
fn immediate_policy_drains_press_repeat_and_release_without_double_delivery() {
    #[derive(Default)]
    struct S {
        log: Vec<&'static str>,
    }
    let view = |_: &S, _: &mut crate::Cx<S>| -> El {
        el().key("root")
            .size(200.0, 100.0)
            .focusable(true)
            .on_key(|s: &mut S, ev, _| {
                if ev.key == Key::Text("x".into()) {
                    s.log.push("press");
                    return true;
                }
                false
            })
            .on_key_up(|s: &mut S, ev, _| {
                if ev.key == Key::Text("x".into()) {
                    s.log.push("release");
                    return true;
                }
                false
            })
    };
    let mut rt = Runtime::new(S::default(), view, (200.0, 100.0), Env::default());
    rt.update(|_, tx| tx.focus("root", false));
    let t0 = crate::time::now();
    let mut clock = FrameClock::new(t0);
    clock.advance(&mut rt, t0, false);

    // 1. Press drains immediately, not again on the next paced draw.
    let queued = {
        let (core, mut app) = rt.split_mut();
        keyboard_core(core, &mut app, press_char_input('x', PhysicalKeyCode::KeyX))
    };
    assert!(queued);
    assert!(step_rt(KeyboardDispatch::Immediate, &mut rt, &mut clock, t0 + Duration::from_millis(2), false));
    assert_eq!(rt.state.log, vec!["press"]);
    assert!(!rt.has_input_waiting());

    clock.advance(&mut rt, t0 + Duration::from_millis(16), false);
    assert_eq!(rt.state.log, vec!["press"], "next paced frame must not redeliver the press");

    // 2. Repeat drains immediately without double delivery.
    let mut repeat = press_char_input('x', PhysicalKeyCode::KeyX);
    repeat.repeat = true;
    let queued = {
        let (core, mut app) = rt.split_mut();
        keyboard_core(core, &mut app, repeat)
    };
    assert!(queued);
    assert!(step_rt(KeyboardDispatch::Immediate, &mut rt, &mut clock, t0 + Duration::from_millis(20), false));
    assert_eq!(rt.state.log, vec!["press", "press"]);
    clock.advance(&mut rt, t0 + Duration::from_millis(32), false);
    assert_eq!(rt.state.log, vec!["press", "press"]);

    // 3. Release drains immediately without double delivery.
    let release = KeyInput {
        physical: Some(PhysicalKeyCode::KeyX),
        logical: Some(KeyEvent { key: Key::Text("x".into()), mods: Mods::default() }),
        base: Some('x'),
        mods: Mods::default(),
        pressed: false,
        repeat: false,
        unhandled: None,
        text: None,
        consumed_mods: Mods::default(),
    };
    let queued = {
        let (core, mut app) = rt.split_mut();
        keyboard_core(core, &mut app, release)
    };
    assert!(queued);
    assert!(step_rt(KeyboardDispatch::Immediate, &mut rt, &mut clock, t0 + Duration::from_millis(35), false));
    assert_eq!(rt.state.log, vec!["press", "press", "release"]);
    clock.advance(&mut rt, t0 + Duration::from_millis(48), false);
    assert_eq!(rt.state.log, vec!["press", "press", "release"]);
}

#[test]
fn immediate_dispatch_preserves_fifo_order_with_earlier_pointer_and_ime_events() {
    #[derive(Default)]
    struct S {
        edit: TextEdit,
        events: Vec<String>,
    }
    let view = |s: &S, _: &mut crate::Cx<S>| -> El {
        col().size(300.0, 200.0).children([
            el().key("btn")
                .size(120.0, 40.0)
                .on_press_tx(|s: &mut S, tx| {
                    s.events.push("click".into());
                    tx.focus("field", false);
                }),
            text_input(&s.edit, TextStyle::new(14.0, 400.0))
                .key("field")
                .size(200.0, 40.0)
                .on_edit(|s: &mut S| &mut s.edit),
        ])
    };
    let mut rt = Runtime::new(S::default(), view, (300.0, 200.0), Env::default());
    let t0 = crate::time::now();
    let mut clock = FrameClock::new(t0);
    clock.advance(&mut rt, t0, false);

    // Queue a pointer click that focuses the field, then an IME preedit, then an IME commit.
    rt.pointer_move(20.0, 20.0);
    rt.pointer_down(20.0, 20.0);
    rt.pointer_up(20.0, 20.0);
    ime_core(rt.core_mut(), None, Ime::Preedit("ni".into(), Some((2, 2)), Vec::new()));
    ime_core(rt.core_mut(), None, Ime::Commit("日".into()));

    assert!(step_rt(KeyboardDispatch::Immediate, &mut rt, &mut clock, t0 + Duration::from_millis(3), false));
    assert_eq!(rt.state.edit.text, "日", "click focused field before IME commit landed");
    assert_eq!(rt.state.events, vec!["click"], "click ran once before IME commit");
    assert!(!rt.has_input_waiting());

    // Subsequent paced frame must not replay the click or commit.
    clock.advance(&mut rt, t0 + Duration::from_millis(16), false);
    assert_eq!(rt.state.edit.text, "日");
    assert_eq!(rt.state.events, vec!["click"]);
}

#[test]
fn immediate_dispatch_drains_chained_send_key_and_caps_self_requeue() {
    // 1. Finite chain in a non-polling app drains immediately in the same step.
    #[derive(Default)]
    struct Chain {
        seen: Vec<Key>,
    }
    let mut rt = Runtime::new(Chain::default(), |_, _| el().size(100.0, 100.0), (100.0, 100.0), Env::default());
    rt.on_key = Some(|s, ev, tx| {
        s.seen.push(ev.key.clone());
        if ev.key == Key::Text("a".into()) {
            tx.send_key(KeyEvent { key: Key::Text("b".into()), mods: Mods::default() });
        }
        true
    });
    let t0 = crate::time::now();
    let mut clock = FrameClock::new(t0);
    clock.advance(&mut rt, t0, false);

    rt.key(KeyEvent { key: Key::Text("a".into()), mods: Mods::default() });
    assert!(step_rt(KeyboardDispatch::Immediate, &mut rt, &mut clock, t0 + Duration::from_millis(2), false));
    assert_eq!(rt.state.seen, vec![Key::Text("a".into()), Key::Text("b".into())]);
    assert!(!rt.has_input_waiting());

    // 2. Self-requeueing key handler is bounded and schedules a continuation wake instead of spinning.
    let mut spin = Runtime::new(0usize, |_, _| el().size(100.0, 100.0), (100.0, 100.0), Env::default());
    spin.on_key = Some(|count, _, tx| {
        *count += 1;
        tx.send_key(KeyEvent { key: Key::Text("k".into()), mods: Mods::default() });
        true
    });
    let mut spin_clock = FrameClock::new(t0);
    spin_clock.advance(&mut spin, t0, false);

    spin.key(KeyEvent { key: Key::Text("k".into()), mods: Mods::default() });
    assert!(step_rt(KeyboardDispatch::Immediate, &mut spin, &mut spin_clock, t0 + Duration::from_millis(2), false));
    // Initial pass + 4 bounded follow-up rounds = 5 deliveries, then capped with a continuation wake.
    assert_eq!(spin.state, 5);
    assert!(spin.wake_deadline().is_some(), "capped drain must schedule a continuation wake");
}

#[test]
fn immediate_dispatch_holds_frame_clock_when_idle_starts_animation() {
    let view = |open: &bool, _: &mut crate::Cx<bool>| -> El {
        col().children([el().key("box").size(if *open { 200.0 } else { 40.0 }, 40.0)])
    };
    let mut rt = Runtime::new(false, view, (300.0, 100.0), Env::default());
    rt.on_key = Some(|open, ev, _| {
        if ev.key == Key::Enter {
            *open = true;
            return true;
        }
        false
    });
    let t0 = crate::time::now();
    let mut clock = FrameClock::new(t0);
    clock.advance(&mut rt, t0, false);
    clock.presented(&mut rt, t0);
    assert!(!rt.animating);

    // Idle for 5 full seconds before pressing Enter.
    let key_at = t0 + Duration::from_secs(5);
    rt.key(KeyEvent { key: Key::Enter, mods: Mods::default() });
    assert!(step_rt(KeyboardDispatch::Immediate, &mut rt, &mut clock, key_at, false));
    assert!(rt.state, "Enter handled immediately before the next frame");
    assert!(rt.animating, "view commit started the layout spring animation");
    assert_eq!(clock.last(), key_at, "clock held at key receipt so idle gap is not charged to new motion");

    // Next display-paced frame 8 ms later must step only ~8 ms, NOT MAX_GAP (250 ms).
    let draw_at = key_at + Duration::from_millis(8);
    let (_, dt) = clock.advance(&mut rt, draw_at, false);
    assert!(
        (dt - 0.008).abs() < 1e-6,
        "expected ~8 ms step on next paced frame, got {dt:.6}s"
    );
    assert!(dt <= RESUME_STEP + 1e-6, "must not charge idle gap to newly started animation");
}

#[test]
fn immediate_dispatch_preserves_frame_clock_when_already_animating() {
    let view = |open: &bool, _: &mut crate::Cx<bool>| -> El {
        col().children([el().key("box").size(if *open { 200.0 } else { 40.0 }, 40.0)])
    };
    let mut rt = Runtime::new(false, view, (300.0, 100.0), Env::default());
    rt.on_key = Some(|open, ev, _| {
        if ev.key == Key::Enter {
            *open = !*open;
            return true;
        }
        false
    });
    let t0 = crate::time::now();
    let mut clock = FrameClock::new(t0);
    clock.advance(&mut rt, t0, false);
    clock.presented(&mut rt, t0);

    // Start an animation on frame 1 at t1.
    let t1 = t0 + Duration::from_millis(16);
    rt.update(|open, _| *open = true);
    let (animating, _) = clock.advance(&mut rt, t1, false);
    clock.presented(&mut rt, t1);
    assert!(animating && rt.animating);

    // Mid-frame key at t1 + 4ms while animation is already running.
    let mid = t1 + Duration::from_millis(4);
    rt.key(KeyEvent { key: Key::Escape, mods: Mods::default() });
    assert!(step_rt(KeyboardDispatch::Immediate, &mut rt, &mut clock, mid, false));
    assert_eq!(clock.last(), t1, "already-running motion must not lose elapsed time");

    // Next paced frame at t1 + 16ms receives the full 16ms step.
    let t2 = t1 + Duration::from_millis(16);
    let (_, dt) = clock.advance(&mut rt, t2, false);
    assert!(
        (dt - 0.016).abs() < 1e-6,
        "running animation must advance full 16 ms gap, got {dt:.6}s"
    );
}

#[test]
fn immediate_dispatch_surfaces_clipboard_shell_urgent_and_ime_focus_before_draw() {
    let view = |edit: &TextEdit, _: &mut crate::Cx<TextEdit>| -> El {
        text_input(edit, TextStyle::new(14.0, 400.0))
            .key("editor")
            .size(240.0, 40.0)
            .on_edit(|s: &mut TextEdit| s)
    };
    let mut rt = Runtime::new(TextEdit::new("hello"), view, (300.0, 100.0), Env::default());
    rt.on_key = Some(|_, ev, tx| {
        if ev.key == Key::Text("c".into()) {
            tx.copy("copied-now");
            tx.set_title("Updated Title");
            tx.request_urgent_frame();
            tx.focus("editor", false);
            return true;
        }
        false
    });
    let t0 = crate::time::now();
    let mut clock = FrameClock::new(t0);
    clock.advance(&mut rt, t0, false);
    assert!(rt.ime_area.is_none());

    let memory = crate::clipboard::memory();
    let mut clipboard = crate::clipboard::Client::default();
    clipboard.use_memory(&memory);

    rt.key(KeyEvent { key: Key::Text("c".into()), mods: Mods::default() });
    assert!(step_rt(KeyboardDispatch::Immediate, &mut rt, &mut clock, t0 + Duration::from_millis(1), false));
    clipboard.flush_core(rt.core_mut());
    assert_eq!(memory.text().as_deref().map(String::as_str), Some("copied-now"));
    assert_eq!(rt.core_mut().host.window_title.take_change(), Some("Updated Title"));
    assert!(rt.core_mut().take_urgent_frame());
    assert!(rt.ime_area.is_some(), "Tx::focus during immediate step updates IME cursor area before draw");
}

#[test]
fn immediate_dispatch_noop_when_nothing_queued() {
    let mut rt = Runtime::new(0usize, |_, _| el().size(100.0, 100.0), (100.0, 100.0), Env::default());
    let t0 = crate::time::now();
    let mut clock = FrameClock::new(t0);
    clock.advance(&mut rt, t0, false);
    let commits_before = rt.stats.commits;

    assert!(!step_rt(KeyboardDispatch::Immediate, &mut rt, &mut clock, t0 + Duration::from_millis(5), false));
    assert_eq!(rt.stats.commits, commits_before);
}
