#[test]
fn keyboard_focus_emits_focus_edges_and_releases_on_seat_loss_or_other_surface() {
    use super::keyboard::{FocusEffect, FocusEvent, FocusState, step};
    let mut state = FocusState::default();
    assert!(step(&mut state, FocusEvent::LeaveOurs).is_empty(), "leave while unfocused is a no-op");
    assert_eq!(
        step(&mut state, FocusEvent::EnterOurs).into_iter().collect::<Vec<_>>(),
        [FocusEffect::Focus(true)]
    );
    assert!(step(&mut state, FocusEvent::EnterOurs).is_empty(), "duplicate enter emits nothing");
    assert_eq!(
        step(&mut state, FocusEvent::EnterOther).into_iter().collect::<Vec<_>>(),
        [FocusEffect::Focus(false)],
        "entering another surface loses focus on ours"
    );
    assert_eq!(
        step(&mut state, FocusEvent::EnterOurs).into_iter().collect::<Vec<_>>(),
        [FocusEffect::Focus(true)]
    );
    assert_eq!(
        step(&mut state, FocusEvent::KeyboardGone).into_iter().collect::<Vec<_>>(),
        [FocusEffect::Focus(false)],
        "seat losing keyboard capability notifies focus loss"
    );
    assert!(step(&mut state, FocusEvent::LeaveOurs).is_empty());
}

#[test]
fn released_keys_blur_at_once_and_a_request_refocuses_when_no_leave_came() {
    use super::keyboard::{FocusEffect, FocusEvent, FocusState, step};
    let edges = |state: &mut FocusState, event| step(state, event).into_iter().collect::<Vec<_>>();
    let mut state = FocusState::Focused;
    assert_eq!(edges(&mut state, FocusEvent::Released), [FocusEffect::Focus(false)]);
    assert!(!state.focused(), "keys after a release are dropped");
    // The compositor sent no leave, so it may send no enter either: the request refocuses.
    assert_eq!(edges(&mut state, FocusEvent::Requested), [FocusEffect::Focus(true)]);
    assert!(edges(&mut state, FocusEvent::EnterOurs).is_empty(), "a late enter adds nothing");
    // A compositor that does send leave: no second blur, and the next request waits for enter.
    assert_eq!(edges(&mut state, FocusEvent::Released), [FocusEffect::Focus(false)]);
    assert!(edges(&mut state, FocusEvent::LeaveOurs).is_empty());
    assert!(edges(&mut state, FocusEvent::Requested).is_empty());
    assert_eq!(edges(&mut state, FocusEvent::EnterOurs), [FocusEffect::Focus(true)]);
    // Releasing an unfocused overlay changes nothing.
    let mut state = FocusState::Unfocused;
    assert!(edges(&mut state, FocusEvent::Released).is_empty());
    assert_eq!(state, FocusState::Unfocused);
}

#[test]
fn layer_shell_key_translation_handles_ctrl_modifiers_space_and_non_ascii_layouts() {
    use super::keyboard::{can_request_keys, key_event_from, supports_keys};
    use crate::{Key, KeyPolicy, Mods, PhysicalKeyCode};

    let ctrl = Mods { ctrl: true, cmd: true, ..Default::default() };
    // Holding Ctrl and pressing Shift/Alt yields utf32 == 0 -> must return None, never Command('\0').
    assert_eq!(key_event_from(None, 0, String::new(), ctrl, Some(PhysicalKeyCode::ShiftLeft)), None);
    // Ctrl+Space stays text space, matching window/keys.rs.
    assert_eq!(
        key_event_from(None, ' ' as u32, " ".into(), ctrl, Some(PhysicalKeyCode::Space)).map(|e| e.key.clone()),
        Some(Key::Text(" ".into()))
    );
    // Ctrl+V on Russian layout ('м' on KeyV) maps to US Command('v'), matching window/keys.rs.
    assert_eq!(
        key_event_from(None, 'м' as u32, "м".into(), ctrl, Some(PhysicalKeyCode::KeyV)).map(|e| e.key.clone()),
        Some(Key::Command('v'))
    );
    // Plain Ctrl+C maps to Command('c').
    assert_eq!(
        key_event_from(None, 'C' as u32, "C".into(), ctrl, Some(PhysicalKeyCode::KeyC)).map(|e| e.key.clone()),
        Some(Key::Command('c'))
    );
    // KeyPolicy::None must never report or request keyboard interactivity even with a seat.
    assert!(!can_request_keys(KeyPolicy::None, 4));
    assert!(!supports_keys(KeyPolicy::None, 4, true));
    assert!(can_request_keys(KeyPolicy::OnDemand, 4));
    assert!(supports_keys(KeyPolicy::OnDemand, 4, true));
    assert!(!supports_keys(KeyPolicy::OnDemand, 3, true));
    assert!(!supports_keys(KeyPolicy::OnDemand, 4, false));
    // Exclusive interactivity is in every layer-shell version; it still needs a seat.
    assert!(can_request_keys(KeyPolicy::Exclusive, 1));
    assert!(supports_keys(KeyPolicy::Exclusive, 3, true));
    assert!(!supports_keys(KeyPolicy::Exclusive, 4, false));
}

#[test]
fn layer_shell_initial_interactivity_honors_hold_keys_policy_and_protocol_version() {
    use super::keyboard::{Interactivity, interactivity};
    use crate::KeyPolicy;

    // When not holding keys, every policy and layer-shell version starts at None.
    for policy in [KeyPolicy::None, KeyPolicy::OnDemand, KeyPolicy::Exclusive] {
        for version in [1, 3, 4] {
            assert_eq!(interactivity(policy, version, false), Interactivity::None);
        }
    }

    // Holding keys picks the policy's interactivity only when the layer-shell version supports it.
    assert_eq!(interactivity(KeyPolicy::None, 4, true), Interactivity::None);
    assert_eq!(interactivity(KeyPolicy::Exclusive, 1, true), Interactivity::Exclusive);
    assert_eq!(interactivity(KeyPolicy::Exclusive, 3, true), Interactivity::Exclusive);
    assert_eq!(interactivity(KeyPolicy::Exclusive, 4, true), Interactivity::Exclusive);
    assert_eq!(interactivity(KeyPolicy::OnDemand, 1, true), Interactivity::None);
    assert_eq!(interactivity(KeyPolicy::OnDemand, 3, true), Interactivity::None);
    assert_eq!(interactivity(KeyPolicy::OnDemand, 4, true), Interactivity::OnDemand);
}

#[test]
fn keymap_decode_reads_at_offset_zero_strips_nul_and_enforces_cap() {
    use super::keyboard::{MAX_KEYMAP_BYTES, decode_keymap};
    use std::io::{Error, ErrorKind};

    let bytes = b"xkb_keymap { xkb_keycodes {}; };\0";
    let mut seen_offset = None;
    let decoded = decode_keymap(bytes.len() as u32, |buf, offset| {
        seen_offset = Some(offset);
        buf.copy_from_slice(bytes);
        Ok(())
    });
    assert_eq!(seen_offset, Some(0));
    assert_eq!(decoded.as_deref(), Some("xkb_keymap { xkb_keycodes {}; };"));

    // Payload without a NUL terminator is accepted verbatim.
    let no_nul = b"xkb_keymap {};";
    let decoded = decode_keymap(no_nul.len() as u32, |buf, offset| {
        assert_eq!(offset, 0);
        buf.copy_from_slice(no_nul);
        Ok(())
    });
    assert_eq!(decoded.as_deref(), Some("xkb_keymap {};"));

    // Trailing padding (even non-UTF-8) after the first NUL terminator is ignored.
    let padded = b"xkb_keymap {};\0\0\xff";
    let decoded = decode_keymap(padded.len() as u32, |buf, _| {
        buf.copy_from_slice(padded);
        Ok(())
    });
    assert_eq!(decoded.as_deref(), Some("xkb_keymap {};"));

    // Empty and over-cap sizes are rejected before calling read_at.
    assert_eq!(decode_keymap(0, |_, _| panic!("must not read empty keymap")), None);
    assert_eq!(decode_keymap(MAX_KEYMAP_BYTES + 1, |_, _| panic!("must not allocate over cap")), None);

    // All-NUL payloads, read errors, and invalid UTF-8 return None.
    assert_eq!(decode_keymap(4, |buf, _| { buf.fill(0); Ok(()) }), None);
    assert_eq!(decode_keymap(8, |_, _| Err(Error::from(ErrorKind::UnexpectedEof))), None);
    assert_eq!(decode_keymap(2, |buf, _| { buf.copy_from_slice(&[0xff, 0xfe]); Ok(()) }), None);
}

#[test]
fn key_repeat_uses_default_delay_and_rate_until_repeat_info_and_ignores_stale_or_early_timers() {
    use super::repeat::{Effect, Event, State, step};
    use crate::machine::Seq;
    use std::time::Duration;

    let t0 = crate::time::now();
    let mut seq = Seq::default();
    let mut state = State::default();
    assert_eq!(state.deadline(), None);
    assert_eq!(state.attempt(), None);
    assert_eq!(state.key(), None);

    // Default policy before wl_keyboard.repeat_info arrives: 200 ms delay, 25 Hz (40 ms).
    let effects: Vec<_> = step(&mut seq, &mut state, Event::Press { key: 14, repeats: true, now: t0 }).into_iter().collect();
    let first_due = t0 + Duration::from_millis(200);
    assert_eq!(effects, [Effect::WakeAt(first_due)]);
    assert_eq!(state.deadline(), Some(first_due));
    assert_eq!(state.key(), Some(14));
    let a1 = state.attempt().expect("armed attempt");

    // Early wake changes nothing.
    assert!(step(&mut seq, &mut state, Event::Due { attempt: a1, now: t0 + Duration::from_millis(199) }).is_empty());
    assert_eq!(state.deadline(), Some(first_due));
    assert_eq!(state.attempt(), Some(a1));

    // At the initial delay: fires once and advances by 40 ms with a fresh attempt.
    let effects: Vec<_> = step(&mut seq, &mut state, Event::Due { attempt: a1, now: first_due }).into_iter().collect();
    let second_due = first_due + Duration::from_millis(40);
    assert_eq!(effects, [Effect::Repeat { key: 14 }, Effect::WakeAt(second_due)]);
    let a2 = state.attempt().expect("second armed attempt");
    assert_ne!(a1, a2);
    assert_eq!(state.deadline(), Some(second_due));

    // Stale attempt from before the first repeat is ignored even at or past the new deadline.
    assert!(step(&mut seq, &mut state, Event::Due { attempt: a1, now: second_due }).is_empty());
    assert_eq!(state.deadline(), Some(second_due));

    // Second repeat fires at 240 ms.
    let effects: Vec<_> = step(&mut seq, &mut state, Event::Due { attempt: a2, now: second_due }).into_iter().collect();
    let third_due = second_due + Duration::from_millis(40);
    assert_eq!(effects, [Effect::Repeat { key: 14 }, Effect::WakeAt(third_due)]);
    let a3 = state.attempt().expect("third armed attempt");

    // A stall (wake way past due) emits one repeat without bursting and schedules strictly in the future.
    let stalled_now = t0 + Duration::from_secs(2);
    let effects: Vec<_> = step(&mut seq, &mut state, Event::Due { attempt: a3, now: stalled_now }).into_iter().collect();
    assert_eq!(effects, [Effect::Repeat { key: 14 }, Effect::WakeAt(stalled_now + Duration::from_millis(40))]);
}

#[test]
fn key_repeat_applies_compositor_repeat_info_and_disables_on_rate_zero() {
    use super::repeat::{Effect, Event, Policy, State, step};
    use crate::machine::Seq;
    use std::time::Duration;

    let t0 = crate::time::now();
    let mut seq = Seq::default();
    let mut state = State::default();

    // Custom compositor rate (10 Hz -> 100 ms) and delay (500 ms).
    assert!(step(&mut seq, &mut state, Event::RepeatInfo { rate: 10, delay_ms: 500 }).is_empty());
    let effects: Vec<_> = step(&mut seq, &mut state, Event::Press { key: 105, repeats: true, now: t0 }).into_iter().collect();
    let first_due = t0 + Duration::from_millis(500);
    assert_eq!(effects, [Effect::WakeAt(first_due)]);
    let a1 = state.attempt().unwrap();

    // Dynamic RepeatInfo update while held changes subsequent interval (50 Hz -> 20 ms) and future delay (300 ms).
    assert!(step(&mut seq, &mut state, Event::RepeatInfo { rate: 50, delay_ms: 300 }).is_empty());
    let effects: Vec<_> = step(&mut seq, &mut state, Event::Due { attempt: a1, now: first_due }).into_iter().collect();
    assert_eq!(effects, [Effect::Repeat { key: 105 }, Effect::WakeAt(first_due + Duration::from_millis(20))]);
    let a2 = state.attempt().unwrap();

    // rate == 0 stops an active repeat immediately and prevents future repeats.
    assert!(step(&mut seq, &mut state, Event::RepeatInfo { rate: 0, delay_ms: 200 }).is_empty());
    assert_eq!(state.deadline(), None);
    assert_eq!(state.attempt(), None);
    assert!(step(&mut seq, &mut state, Event::Due { attempt: a2, now: first_due + Duration::from_secs(1) }).is_empty());
    assert!(step(&mut seq, &mut state, Event::Press { key: 105, repeats: true, now: t0 + Duration::from_secs(2) }).is_empty());
    assert_eq!(state.deadline(), None);

    // Re-enabling repeat restores repeating with the new values; negative delay clamps to zero
    // and negative rate disables cleanly.
    assert_eq!(Policy::from_wayland(-1, 200), Policy::Disabled);
    assert_eq!(Policy::from_wayland(25, -50), Policy::Repeat { delay: Duration::ZERO, interval: Duration::from_millis(40) });
    assert_eq!(Policy::from_wayland(i32::MAX, 100), Policy::Repeat { delay: Duration::from_millis(100), interval: Duration::from_nanos(1) });

    assert!(step(&mut seq, &mut state, Event::RepeatInfo { rate: 50, delay_ms: 300 }).is_empty());
    let t3 = t0 + Duration::from_secs(3);
    let effects: Vec<_> = step(&mut seq, &mut state, Event::Press { key: 106, repeats: true, now: t3 }).into_iter().collect();
    assert_eq!(effects, [Effect::WakeAt(t3 + Duration::from_millis(300))]);
}

#[test]
fn key_repeat_filters_non_repeating_keys_and_stops_on_release_leave_or_keyboard_loss() {
    use super::repeat::{Effect, Event, State, step};
    use crate::machine::Seq;
    use std::time::Duration;

    let t0 = crate::time::now();
    let mut seq = Seq::default();
    let mut state = State::default();
    step(&mut seq, &mut state, Event::RepeatInfo { rate: 20, delay_ms: 250 });

    // Non-repeating key (e.g. Shift) while idle does not arm a timer.
    assert!(step(&mut seq, &mut state, Event::Press { key: 42, repeats: false, now: t0 }).is_empty());
    assert_eq!(state.deadline(), None);

    // Repeating key A arms a timer; pressing and releasing Shift does not interrupt A.
    step(&mut seq, &mut state, Event::Press { key: 30, repeats: true, now: t0 });
    let a_attempt = state.attempt().unwrap();
    let a_due = t0 + Duration::from_millis(250);
    assert!(step(&mut seq, &mut state, Event::Press { key: 42, repeats: false, now: t0 + Duration::from_millis(50) }).is_empty());
    assert!(step(&mut seq, &mut state, Event::Release { key: 42 }).is_empty());
    assert_eq!(state.attempt(), Some(a_attempt));
    assert_eq!(state.deadline(), Some(a_due));

    // Pressing another repeating key B replaces A and invalidates A's attempt; releasing A keeps B repeating.
    let t1 = t0 + Duration::from_millis(100);
    let effects: Vec<_> = step(&mut seq, &mut state, Event::Press { key: 48, repeats: true, now: t1 }).into_iter().collect();
    let b_due = t1 + Duration::from_millis(250);
    assert_eq!(effects, [Effect::WakeAt(b_due)]);
    let b_attempt = state.attempt().unwrap();
    assert_ne!(a_attempt, b_attempt);
    assert!(step(&mut seq, &mut state, Event::Due { attempt: a_attempt, now: b_due }).is_empty());
    assert!(step(&mut seq, &mut state, Event::Release { key: 30 }).is_empty());
    assert_eq!(state.key(), Some(48));
    let effects: Vec<_> = step(&mut seq, &mut state, Event::Due { attempt: b_attempt, now: b_due }).into_iter().collect();
    assert_eq!(effects, [Effect::Repeat { key: 48 }, Effect::WakeAt(b_due + Duration::from_millis(50))]);

    // Releasing B stops repeating.
    let b_next = state.attempt().unwrap();
    assert!(step(&mut seq, &mut state, Event::Release { key: 48 }).is_empty());
    assert_eq!(state.deadline(), None);
    assert!(step(&mut seq, &mut state, Event::Due { attempt: b_next, now: b_due + Duration::from_secs(1) }).is_empty());

    // Stop (focus leave / enter other / drain) cancels repeat but keeps the compositor's RepeatInfo (250 ms).
    let t2 = t0 + Duration::from_secs(2);
    step(&mut seq, &mut state, Event::Press { key: 14, repeats: true, now: t2 });
    let c_attempt = state.attempt().unwrap();
    assert!(step(&mut seq, &mut state, Event::Stop).is_empty());
    assert_eq!(state.deadline(), None);
    assert!(step(&mut seq, &mut state, Event::Due { attempt: c_attempt, now: t2 + Duration::from_secs(1) }).is_empty());
    let effects: Vec<_> = step(&mut seq, &mut state, Event::Press { key: 14, repeats: true, now: t2 }).into_iter().collect();
    assert_eq!(effects, [Effect::WakeAt(t2 + Duration::from_millis(250))]);

    // KeyboardGone cancels repeat and resets RepeatInfo back to default (200 ms / 40 ms) for the next wl_keyboard.
    step(&mut seq, &mut state, Event::RepeatInfo { rate: 0, delay_ms: 999 });
    step(&mut seq, &mut state, Event::KeyboardGone);
    assert_eq!(state.deadline(), None);
    let effects: Vec<_> = step(&mut seq, &mut state, Event::Press { key: 14, repeats: true, now: t2 }).into_iter().collect();
    assert_eq!(effects, [Effect::WakeAt(t2 + Duration::from_millis(200))]);
}

#[test]
fn dead_key_compose_emits_composed_text_and_suppresses_composing_and_cancelled() {
    use super::keyboard::{ComposeAction, ComposeOutcome, compose_action, compose_locale, key_event_with_compose};
    use crate::{Key, KeyEvent, Mods, PhysicalKeyCode};

    let none = Mods::default();
    let shift = Mods { shift: true, ..Default::default() };
    let ctrl = Mods { ctrl: true, cmd: true, ..Default::default() };
    let alt = Mods { alt: true, ..Default::default() };
    let logo = Mods { logo: true, ..Default::default() };

    // Only non-repeat presses feed compose; command chords reset and bypass; releases and repeats bypass.
    assert_eq!(compose_action(true, false, none), ComposeAction::Feed);
    assert_eq!(compose_action(true, false, shift), ComposeAction::Feed);
    assert_eq!(compose_action(true, false, alt), ComposeAction::Feed);
    assert_eq!(compose_action(true, false, ctrl), ComposeAction::ResetAndBypass);
    assert_eq!(compose_action(true, false, Mods { ctrl: true, ..Default::default() }), ComposeAction::ResetAndBypass);
    assert_eq!(compose_action(true, false, logo), ComposeAction::ResetAndBypass);
    assert_eq!(compose_action(false, false, none), ComposeAction::Bypass);
    assert_eq!(compose_action(true, true, none), ComposeAction::Bypass);
    assert_eq!(compose_action(false, false, ctrl), ComposeAction::Bypass);

    // Dead key press (Composing) suppresses both bare dead keys and mid-sequence Multi_key letters.
    assert_eq!(key_event_with_compose(ComposeOutcome::Composing, None, 0, String::new(), none, Some(PhysicalKeyCode::BracketLeft)), None);
    assert_eq!(key_event_with_compose(ComposeOutcome::Composing, None, 'a' as u32, "a".into(), none, Some(PhysicalKeyCode::KeyA)), None);

    // Completing dead_acute + a emits "á" instead of uncomposed "a".
    assert_eq!(
        key_event_with_compose(ComposeOutcome::Composed(Some("á".into())), None, 'a' as u32, "a".into(), none, Some(PhysicalKeyCode::KeyA)),
        Some(KeyEvent { key: Key::Text("á".into()), mods: none })
    );
    // Completing dead_acute + Shift+A preserves Shift in mods and emits "Á".
    assert_eq!(
        key_event_with_compose(ComposeOutcome::Composed(Some("Á".into())), None, 'A' as u32, "A".into(), shift, Some(PhysicalKeyCode::KeyA)),
        Some(KeyEvent { key: Key::Text("Á".into()), mods: shift })
    );
    // Double dead key (dead_acute + dead_acute) emits "´" even though raw xkb utf8 is empty.
    assert_eq!(
        key_event_with_compose(ComposeOutcome::Composed(Some("´".into())), None, 0, String::new(), none, Some(PhysicalKeyCode::BracketLeft)),
        Some(KeyEvent { key: Key::Text("´".into()), mods: none })
    );
    // Multi-character compose results emit verbatim; empty, missing, or control results emit None.
    assert_eq!(
        key_event_with_compose(ComposeOutcome::Composed(Some("ff".into())), None, 'f' as u32, "f".into(), none, Some(PhysicalKeyCode::KeyF)),
        Some(KeyEvent { key: Key::Text("ff".into()), mods: none })
    );
    assert_eq!(key_event_with_compose(ComposeOutcome::Composed(None), None, 'a' as u32, "a".into(), none, Some(PhysicalKeyCode::KeyA)), None);
    assert_eq!(key_event_with_compose(ComposeOutcome::Composed(Some(String::new())), None, 'a' as u32, "a".into(), none, Some(PhysicalKeyCode::KeyA)), None);
    assert_eq!(key_event_with_compose(ComposeOutcome::Composed(Some("\x07".into())), None, 'a' as u32, "a".into(), none, Some(PhysicalKeyCode::KeyA)), None);

    // Cancelled compose suppresses uncomposed letters (e.g. dead_acute + q) but still delivers named keys (Escape/Backspace).
    assert_eq!(key_event_with_compose(ComposeOutcome::Cancelled, None, 'q' as u32, "q".into(), none, Some(PhysicalKeyCode::KeyQ)), None);
    assert_eq!(
        key_event_with_compose(ComposeOutcome::Cancelled, Some(Key::Escape), 0x1b, "\x1b".into(), none, Some(PhysicalKeyCode::Escape)),
        Some(KeyEvent { key: Key::Escape, mods: none })
    );
    assert_eq!(
        key_event_with_compose(ComposeOutcome::Cancelled, Some(Key::Backspace), 0x08, "\x08".into(), none, Some(PhysicalKeyCode::Backspace)),
        Some(KeyEvent { key: Key::Backspace, mods: none })
    );

    // Nothing falls back to uncomposed key_event_from (plain text, named keys, and command chords).
    assert_eq!(
        key_event_with_compose(ComposeOutcome::Nothing, None, 'a' as u32, "a".into(), none, Some(PhysicalKeyCode::KeyA)),
        Some(KeyEvent { key: Key::Text("a".into()), mods: none })
    );
    assert_eq!(
        key_event_with_compose(ComposeOutcome::Nothing, None, 'v' as u32, "v".into(), ctrl, Some(PhysicalKeyCode::KeyV)),
        Some(KeyEvent { key: Key::Command('v'), mods: ctrl })
    );

    // Locale precedence: LC_ALL > LC_CTYPE > LANG > "C", skipping empty or NUL-containing values.
    assert_eq!(compose_locale(|k| match k { "LC_ALL" => Some("es_AR.UTF-8".into()), "LC_CTYPE" => Some("fr_FR.UTF-8".into()), "LANG" => Some("en_US.UTF-8".into()), _ => None }), "es_AR.UTF-8");
    assert_eq!(compose_locale(|k| match k { "LC_ALL" => Some(String::new()), "LC_CTYPE" => Some("es_ES.UTF-8".into()), "LANG" => Some("en_US.UTF-8".into()), _ => None }), "es_ES.UTF-8");
    assert_eq!(compose_locale(|k| match k { "LC_ALL" => Some("bad\0loc".into()), "LANG" => Some("en_US.UTF-8".into()), _ => None }), "en_US.UTF-8");
    assert_eq!(compose_locale(|_| None), "C");
}

#[test]
fn physical_text_matches_window_host_for_chords_control_keys_and_compose() {
    use super::keyboard::{ComposeOutcome, key_event_with_compose, physical_text};
    use crate::window::consumed_from;
    use crate::{Key, KeyEvent, Mods, PhysicalKeyCode};

    let none = Mods::default();
    let shift = Mods { shift: true, ..Default::default() };
    let ctrl = Mods { ctrl: true, cmd: true, ..Default::default() };
    let ctrl_shift = Mods { ctrl: true, cmd: true, shift: true, ..Default::default() };
    let alt = Mods { alt: true, ..Default::default() };
    let logo = Mods { logo: true, ..Default::default() };

    // Plain, shifted, Alt, Super, AltGr, and layout-typed characters (including Persian ZWNJ).
    for (label, mods, state_utf8, sym_utf8, unmodified, want_text, want_consumed) in [
        ("a", none, "a", "a", Some("a"), Some("a"), none),
        ("Shift+a", shift, "A", "A", Some("a"), Some("A"), shift),
        ("Shift+2", shift, "@", "@", Some("2"), Some("@"), shift),
        ("Alt+a", alt, "a", "a", Some("a"), Some("a"), none),
        ("Super+a", logo, "a", "a", Some("a"), Some("a"), none),
        ("no AltGr+8", none, "[", "[", Some("8"), Some("["), none),
        ("ir Shift+Space ZWNJ", shift, "\u{200c}", "\u{200c}", Some(" "), Some("\u{200c}"), shift),
        // Ctrl chords: xkb_state_key_get_utf8 returns a control byte, so sym_utf8 supplies the key's text.
        ("Ctrl+c", ctrl, "\x03", "c", Some("c"), Some("c"), none),
        ("Ctrl+Shift+c", ctrl_shift, "\x03", "C", Some("c"), Some("C"), none),
        ("Ctrl+Space (NUL)", ctrl, "\0", " ", Some(" "), Some(" "), none),
        ("Ctrl+Space (empty)", ctrl, "", " ", Some(" "), Some(" "), none),
        ("Russian Ctrl+м", ctrl, "м", "м", Some("м"), Some("м"), none),
        // Control keys keep their control text and consume no modifiers.
        ("Alt+Enter", alt, "\r", "\r", None, Some("\r"), none),
        ("Escape", none, "\u{1b}", "\u{1b}", None, Some("\u{1b}"), none),
        ("Tab", none, "\t", "\t", None, Some("\t"), none),
        ("Backspace", none, "\u{8}", "\u{8}", None, Some("\u{8}"), none),
        ("Delete", none, "\u{7f}", "\u{7f}", None, Some("\u{7f}"), none),
        // Shift+Tab (ISO_Left_Tab) and non-text keys carry None.
        ("Shift+Tab", shift, "", "", None, None, none),
        ("Arrow/F-key", none, "", "", None, None, none),
    ] {
        let text = physical_text(true, &ComposeOutcome::Nothing, state_utf8, sym_utf8);
        assert_eq!(text.as_deref(), want_text, "{label}: physical text");
        assert_eq!(consumed_from(false, mods, text.as_deref(), unmodified), want_consumed, "{label}: consumed_mods");
        // Releases never carry text or consumed_mods.
        assert_eq!(physical_text(false, &ComposeOutcome::Nothing, state_utf8, sym_utf8), None, "{label}: release");
    }

    // Logical fallback stays unchanged: Ctrl+c is Command('c'), Alt+Enter is Enter, Escape is Escape,
    // and Russian Ctrl+м remaps to Command('v') while physical_text keeps "м".
    assert_eq!(
        key_event_with_compose(ComposeOutcome::Nothing, None, 'c' as u32, "\x03".into(), ctrl, Some(PhysicalKeyCode::KeyC)),
        Some(KeyEvent { key: Key::Command('c'), mods: ctrl })
    );
    assert_eq!(
        key_event_with_compose(ComposeOutcome::Nothing, None, 'C' as u32, "\x03".into(), ctrl_shift, Some(PhysicalKeyCode::KeyC)),
        Some(KeyEvent { key: Key::Command('c'), mods: ctrl_shift })
    );
    assert_eq!(
        key_event_with_compose(ComposeOutcome::Nothing, Some(Key::Enter), '\r' as u32, "\r".into(), alt, Some(PhysicalKeyCode::Enter)),
        Some(KeyEvent { key: Key::Enter, mods: alt })
    );
    assert_eq!(
        key_event_with_compose(ComposeOutcome::Nothing, Some(Key::Escape), 0x1b, "\u{1b}".into(), none, Some(PhysicalKeyCode::Escape)),
        Some(KeyEvent { key: Key::Escape, mods: none })
    );
    assert_eq!(
        key_event_with_compose(ComposeOutcome::Nothing, None, 'м' as u32, "м".into(), ctrl, Some(PhysicalKeyCode::KeyV)),
        Some(KeyEvent { key: Key::Command('v'), mods: ctrl })
    );

    // Dead-key compose: Composing and Cancelled produce None; Composed carries the composed string.
    assert_eq!(physical_text(true, &ComposeOutcome::Composing, "", ""), None);
    assert_eq!(physical_text(true, &ComposeOutcome::Composing, "a", "a"), None);
    assert_eq!(physical_text(true, &ComposeOutcome::Cancelled, "q", "q"), None);
    assert_eq!(physical_text(true, &ComposeOutcome::Cancelled, "\u{1b}", "\u{1b}"), None);
    assert_eq!(physical_text(true, &ComposeOutcome::Composed(None), "a", "a"), None);
    assert_eq!(physical_text(true, &ComposeOutcome::Composed(Some(String::new())), "a", "a"), None);
    let composed = physical_text(true, &ComposeOutcome::Composed(Some("é".into())), "e", "e");
    assert_eq!(composed.as_deref(), Some("é"));
    assert_eq!(consumed_from(false, none, composed.as_deref(), Some("e")), none);
    let composed_shift = physical_text(true, &ComposeOutcome::Composed(Some("É".into())), "E", "E");
    assert_eq!(composed_shift.as_deref(), Some("É"));
    assert_eq!(consumed_from(false, shift, composed_shift.as_deref(), Some("e")), shift);
    assert_eq!(physical_text(false, &ComposeOutcome::Composed(Some("é".into())), "e", "e"), None);
}
