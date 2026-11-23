//! Headless tests for the Windows UI Automation host-focus state machine and adapter updates.

use std::cell::Cell;
use std::sync::Arc;

use accesskit::{Action as Ak, ActionRequest, TreeId};
use accesskit_windows::{LPARAM, WPARAM};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    OBJID_CLIENT, WA_ACTIVE, WA_CLICKACTIVE, WA_INACTIVE, WM_ACTIVATE, WM_ACTIVATEAPP, WM_KILLFOCUS,
    WM_SETFOCUS,
};

use super::{
    FocusEffect, FocusEvent, HostFocus, NativeFocus, SharedState, flush_pending_focus,
    focus_event_for_message, handle_focus_message, step, update_with,
};
use crate::a11y::{Actions, Activation, AccessibilityLink};
use crate::machine::Effects;
use crate::{Cx, El, Env, Runtime, TextEdit, col, row, text, text_input};

#[derive(Default)]
struct Form {
    name: TextEdit,
    presses: u32,
}

fn form_view(s: &Form, _: &mut Cx<Form>) -> El {
    col().w_full().gap(8.0)
        .child(row().key("save").role("button").focusable(true).on_press(|s: &mut Form| s.presses += 1).child(text("Save")))
        .child(text_input(&s.name, crate::tok::type_::BODY).key("name").label("Name").single_line().w(200.0).on_edit(|s: &mut Form| &mut s.name))
}

fn form_rt() -> Runtime<Form> {
    let mut rt = Runtime::new(Form::default(), form_view, (320.0, 200.0), Env::new(true, 1.0));
    rt.frame(0.0);
    rt
}

fn focus_request(rt: &Runtime<Form>, key: &str) -> ActionRequest {
    let tree = rt.accessibility_tree();
    let (id, _) = tree.nodes.iter().find(|(_, n)| n.author_id() == Some(key)).expect("keyed node");
    ActionRequest { action: Ak::Focus, target_tree: TreeId::ROOT, target_node: *id, data: None }
}

/// Inspects AccessKit's own `is_window_focused` / `is_host_focused` flag directly through its
/// `Debug` representation, without opening a native window.
fn adapter_is_host_focused(shared: &SharedState) -> bool {
    let dbg = format!("{:?}", shared.state.borrow().adapter);
    if dbg.contains("is_window_focused: ") {
        return dbg.contains("is_window_focused: true");
    }
    assert!(dbg.contains("is_host_focused: "), "unexpected Adapter Debug output: {dbg}");
    let is_true = dbg.contains("is_host_focused: true");
    let is_false = dbg.contains("is_host_focused: false");
    assert_ne!(is_true, is_false, "Tree state and next_state disagreed on is_host_focused: {dbg}");
    is_true
}

fn make_shared(hwnd: isize, native: NativeFocus) -> (SharedState, AccessibilityLink) {
    let link = AccessibilityLink::with_wake(Arc::new(|| {}));
    let shared = SharedState::new(hwnd, native, Activation(link.shared.clone()), Actions(link.shared.clone()));
    (shared, link)
}

fn activate_tree(shared: &SharedState, hwnd: isize, link: &mut AccessibilityLink, rt: &mut Runtime<Form>, sample: &impl Fn(isize) -> NativeFocus) -> usize {
    // Transition AccessKit from Inactive -> Placeholder as WM_GETOBJECT does, without calling
    // UiaReturnRawElementProvider on a synthetic HWND.
    let result = {
        let mut s = shared.state.borrow_mut();
        let s = &mut *s;
        s.adapter.handle_wm_getobject(WPARAM(0), LPARAM(OBJID_CLIENT as isize), &mut s.activation)
    };
    assert!(result.is_some(), "OBJID_CLIENT must activate the adapter");
    drop(result);
    let initial = link.service(rt, "Form").expect("initial full tree");
    let mut raised = 0usize;
    update_with(shared, hwnd, initial, sample, &mut |events| {
        assert!(shared.state.try_borrow_mut().is_ok(), "RefCell borrow must be released before raising UIA events");
        raised += 1;
        drop(events);
    });
    raised
}

#[test]
fn background_window_activated_via_activateapp_gains_host_focus_and_raises_event() {
    let hwnd: isize = 0x1001;
    let other: isize = 0x2002;
    let foreground = Cell::new(other);
    let thread_focus = Cell::new(hwnd);
    let sample = |h: isize| NativeFocus::from_handles(h, foreground.get(), thread_focus.get());

    // 1. Window created behind another foreground process while its own thread retained focus.
    let mut rt = form_rt();
    let (shared, mut link) = make_shared(hwnd, sample(hwnd));
    assert_eq!(shared.state.borrow().focus, HostFocus::Unfocused);
    assert!(!adapter_is_host_focused(&shared));

    // 2. UIA queries the background window and receives the initial tree while still unfocused.
    let initial_raises = activate_tree(&shared, hwnd, &mut link, &mut rt, &sample);
    assert_eq!(initial_raises, 0, "unfocused background window must not raise a focus event on activation");
    assert!(!adapter_is_host_focused(&shared));

    // 3. First foreground activation arrives via WM_ACTIVATEAPP(TRUE) with retained thread focus,
    //    and no second WM_SETFOCUS.
    foreground.set(hwnd);
    let mut activate_raises = 0usize;
    handle_focus_message(&shared, hwnd, WM_ACTIVATEAPP, 1, &sample, &mut |events| {
        assert!(shared.state.try_borrow_mut().is_ok(), "RefCell borrow must be released before raising focus event");
        activate_raises += 1;
        drop(events);
    });
    assert_eq!(shared.state.borrow().focus, HostFocus::Focused);
    assert!(adapter_is_host_focused(&shared), "foreground activation must mark AccessKit host focused");
    assert_eq!(activate_raises, 1, "activating into the foreground must raise the UIA focus-changed event");

    // 4. UIA SetFocus on the text input updates the tree while host focus stays true.
    let req = focus_request(&rt, "name");
    assert!(rt.a11y_action(&req));
    rt.frame(0.0);
    let delta = link.service(&mut rt, "Form").expect("focus move delta");
    let mut move_raises = 0usize;
    update_with(&shared, hwnd, delta, &sample, &mut |events| {
        assert!(shared.state.try_borrow_mut().is_ok());
        move_raises += 1;
        drop(events);
    });
    assert!(adapter_is_host_focused(&shared));
    assert_eq!(move_raises, 1);
}

#[test]
fn nested_focus_message_during_borrow_reconciles_on_update_and_raises_outside_borrow() {
    let hwnd: isize = 0x1001;
    let other: isize = 0x2002;
    let foreground = Cell::new(other);
    let thread_focus = Cell::new(hwnd);
    let sample = |h: isize| NativeFocus::from_handles(h, foreground.get(), thread_focus.get());

    let mut rt = form_rt();
    let (shared, mut link) = make_shared(hwnd, sample(hwnd));
    activate_tree(&shared, hwnd, &mut link, &mut rt, &sample);
    assert!(!adapter_is_host_focused(&shared));

    // Hold a mutable borrow to simulate a nested native focus message arriving mid-borrow.
    foreground.set(hwnd);
    {
        let _held = shared.state.borrow_mut();
        handle_focus_message(&shared, hwnd, WM_ACTIVATEAPP, 1, &sample, &mut |events| drop(events));
        assert!(shared.reconcile_pending.get(), "dropped borrow must record pending reconciliation");
    }

    // Delivering the next tree update reconciles host focus first and raises both events after
    // releasing the RefCell borrow.
    let req = focus_request(&rt, "name");
    assert!(rt.a11y_action(&req));
    rt.frame(0.0);
    let delta = link.service(&mut rt, "Form").expect("focus move delta");
    let mut raises = 0usize;
    update_with(&shared, hwnd, delta, &sample, &mut |events| {
        assert!(shared.state.try_borrow_mut().is_ok(), "RefCell borrow must be released before raising");
        raises += 1;
        drop(events);
    });
    assert!(!shared.reconcile_pending.get());
    assert_eq!(shared.state.borrow().focus, HostFocus::Focused);
    assert!(adapter_is_host_focused(&shared));
    assert_eq!(raises, 2, "reconciled host focus and the tree update must both raise");
}

#[test]
fn foreground_window_focus_moves_and_lose_regain_keep_working() {
    let hwnd: isize = 0x1001;
    let other: isize = 0x2002;
    let foreground = Cell::new(hwnd);
    let thread_focus = Cell::new(hwnd);
    let sample = |h: isize| NativeFocus::from_handles(h, foreground.get(), thread_focus.get());

    // Made in the foreground: focused from the start.
    let mut rt = form_rt();
    let (shared, mut link) = make_shared(hwnd, sample(hwnd));
    assert_eq!(shared.state.borrow().focus, HostFocus::Focused);
    assert!(adapter_is_host_focused(&shared));
    let initial_raises = activate_tree(&shared, hwnd, &mut link, &mut rt, &sample);
    assert_eq!(initial_raises, 1, "foreground window raises initial focus event");
    assert!(adapter_is_host_focused(&shared));

    // Focus moves between elements while in the foreground.
    for key in ["save", "name"] {
        let req = focus_request(&rt, key);
        assert!(rt.a11y_action(&req));
        rt.frame(0.0);
        let delta = link.service(&mut rt, "Form").expect("focus delta");
        let mut raises = 0usize;
        update_with(&shared, hwnd, delta, &sample, &mut |events| {
            raises += 1;
            drop(events);
        });
        assert!(adapter_is_host_focused(&shared));
        assert_eq!(raises, 1);
    }

    // Window loses foreground to another process.
    foreground.set(other);
    thread_focus.set(0);
    handle_focus_message(&shared, hwnd, WM_KILLFOCUS, 0, &sample, &mut |events| drop(events));
    assert_eq!(shared.state.borrow().focus, HostFocus::Unfocused);
    assert!(!adapter_is_host_focused(&shared));

    // Window regains foreground via WM_ACTIVATEAPP + WM_SETFOCUS: raises once, not twice.
    foreground.set(hwnd);
    thread_focus.set(hwnd);
    let mut raises = 0usize;
    handle_focus_message(&shared, hwnd, WM_ACTIVATEAPP, 1, &sample, &mut |events| {
        raises += 1;
        drop(events);
    });
    handle_focus_message(&shared, hwnd, WM_SETFOCUS, 0, &sample, &mut |events| {
        raises += 1;
        drop(events);
    });
    assert_eq!(shared.state.borrow().focus, HostFocus::Focused);
    assert!(adapter_is_host_focused(&shared));
    assert_eq!(raises, 1, "redundant WM_SETFOCUS after WM_ACTIVATEAPP must not re-update");
}

#[test]
fn second_window_tracks_foreground_and_focus_independently() {
    let h1: isize = 0x1001;
    let h2: isize = 0x1002;
    let other: isize = 0x2002;
    let foreground = Cell::new(h1);
    let thread_focus = Cell::new(h1);
    let sample = |h: isize| NativeFocus::from_handles(h, foreground.get(), thread_focus.get());

    let mut rt1 = form_rt();
    let mut rt2 = form_rt();
    let (s1, mut l1) = make_shared(h1, sample(h1));
    let (s2, mut l2) = make_shared(h2, sample(h2));
    activate_tree(&s1, h1, &mut l1, &mut rt1, &sample);
    activate_tree(&s2, h2, &mut l2, &mut rt2, &sample);
    assert!(adapter_is_host_focused(&s1));
    assert!(!adapter_is_host_focused(&s2));

    // Switch within the same process from window 1 to window 2.
    foreground.set(h2);
    thread_focus.set(h2);
    handle_focus_message(&s1, h1, WM_KILLFOCUS, h2 as usize, &sample, &mut |events| drop(events));
    handle_focus_message(&s2, h2, WM_SETFOCUS, h1 as usize, &sample, &mut |events| drop(events));
    assert!(!adapter_is_host_focused(&s1));
    assert!(adapter_is_host_focused(&s2));

    // Another app takes foreground, then user activates window 2 back: Windows broadcasts
    // WM_ACTIVATEAPP(TRUE) to every top-level window in our process, but only window 2 owns
    // foreground and thread focus.
    foreground.set(other);
    handle_focus_message(&s2, h2, WM_ACTIVATEAPP, 0, &sample, &mut |events| drop(events));
    handle_focus_message(&s1, h1, WM_ACTIVATEAPP, 0, &sample, &mut |events| drop(events));
    assert!(!adapter_is_host_focused(&s1));
    assert!(!adapter_is_host_focused(&s2));

    foreground.set(h2);
    thread_focus.set(h2);
    let mut s1_raises = 0usize;
    let mut s2_raises = 0usize;
    handle_focus_message(&s1, h1, WM_ACTIVATEAPP, 1, &sample, &mut |events| {
        s1_raises += 1;
        drop(events);
    });
    handle_focus_message(&s2, h2, WM_ACTIVATEAPP, 1, &sample, &mut |events| {
        s2_raises += 1;
        drop(events);
    });
    assert!(!adapter_is_host_focused(&s1), "inactive sibling must stay unfocused on broadcast WM_ACTIVATEAPP");
    assert_eq!(s1_raises, 0);
    assert!(adapter_is_host_focused(&s2));
    assert_eq!(s2_raises, 1);
}

#[test]
fn state_machine_requires_both_foreground_and_thread_focus() {
    let fg_only = NativeFocus { foreground: true, thread_focus: false };
    let thread_only = NativeFocus { foreground: false, thread_focus: true };
    let both = NativeFocus { foreground: true, thread_focus: true };

    assert_eq!(HostFocus::from_native(fg_only), HostFocus::Unfocused);
    assert_eq!(HostFocus::from_native(thread_only), HostFocus::Unfocused);
    assert_eq!(HostFocus::from_native(both), HostFocus::Focused);

    // Background WM_SETFOCUS must not claim host focus while another process is foreground.
    let mut state = HostFocus::Unfocused;
    assert_eq!(step(&mut state, FocusEvent::SetFocus(thread_only)), Effects::none());
    assert_eq!(state, HostFocus::Unfocused);

    // WM_ACTIVATE(WA_ACTIVE) while foreground & retained thread focus moves to Focused.
    let ev = focus_event_for_message(WM_ACTIVATE, WA_ACTIVE as usize, || both).unwrap();
    assert_eq!(step(&mut state, ev), Effects::one(FocusEffect::UpdateAdapter(true)));
    assert_eq!(state, HostFocus::Focused);

    // Minimized WM_ACTIVATE (HIWORD != 0) or WA_INACTIVE moves to Unfocused.
    let minimized = (WA_CLICKACTIVE as usize) | (1usize << 16);
    let ev = focus_event_for_message(WM_ACTIVATE, minimized, || both).unwrap();
    assert_eq!(step(&mut state, ev), Effects::one(FocusEffect::UpdateAdapter(false)));
    assert_eq!(state, HostFocus::Unfocused);

    // Re-focus, then WM_ACTIVATE(WA_INACTIVE) moves to Unfocused.
    assert_eq!(step(&mut state, FocusEvent::Reconcile(both)), Effects::one(FocusEffect::UpdateAdapter(true)));
    let ev = focus_event_for_message(WM_ACTIVATE, WA_INACTIVE as usize, || both).unwrap();
    assert_eq!(step(&mut state, ev), Effects::one(FocusEffect::UpdateAdapter(false)));
    assert_eq!(state, HostFocus::Unfocused);
}

#[test]
fn deferred_focus_flushes_after_borrow_and_reentrant_killfocus_during_raise_applies() {
    let hwnd: isize = 0x1001;
    let other: isize = 0x2002;
    let foreground = Cell::new(other);
    let thread_focus = Cell::new(hwnd);
    let sample = |h: isize| NativeFocus::from_handles(h, foreground.get(), thread_focus.get());

    let mut rt = form_rt();
    let (shared, mut link) = make_shared(hwnd, sample(hwnd));
    activate_tree(&shared, hwnd, &mut link, &mut rt, &sample);

    // Nested WM_ACTIVATEAPP(TRUE) during a borrow defers reconciliation.
    foreground.set(hwnd);
    {
        let _held = shared.state.borrow_mut();
        handle_focus_message(&shared, hwnd, WM_ACTIVATEAPP, 1, &sample, &mut |events| drop(events));
        assert!(shared.reconcile_pending.get());
    }

    // Flushing after the borrow raises outside the borrow, so a re-entrant WM_KILLFOCUS pumped
    // by UIA during raise applies immediately without dropping or panicking.
    let mut raises = 0usize;
    flush_pending_focus(&shared, hwnd, &sample, &mut |events| {
        raises += 1;
        drop(events);
        foreground.set(other);
        thread_focus.set(0);
        handle_focus_message(&shared, hwnd, WM_KILLFOCUS, 0, &sample, &mut |nested| drop(nested));
    });
    assert_eq!(raises, 1);
    assert!(!shared.reconcile_pending.get());
    assert_eq!(shared.state.borrow().focus, HostFocus::Unfocused);
    assert!(!adapter_is_host_focused(&shared));
}
