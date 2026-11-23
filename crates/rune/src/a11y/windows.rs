//! UI Automation for a window: AccessKit's adapter answers the window's `WM_GETOBJECT`.
//! AccessKit's own subclassing adapter must exist before the window first shows; Rune's windows
//! open shown, so this subclasses with `SetWindowSubclass` instead, which works at any time.

#![allow(unsafe_code)]

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use accesskit_windows::{HWND, LPARAM, LRESULT, WPARAM};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::GetFocus;
use windows_sys::Win32::UI::Shell::{DefSubclassProc, RemoveWindowSubclass, SetWindowSubclass};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    GetForegroundWindow, WA_INACTIVE, WM_ACTIVATE, WM_ACTIVATEAPP, WM_GETOBJECT, WM_KILLFOCUS,
    WM_NCDESTROY, WM_SETFOCUS, WM_SETTINGCHANGE, WM_SYSCOLORCHANGE, WM_THEMECHANGED,
};
use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};

use crate::machine::Effects;

use super::{Actions, Activation};

const SUBCLASS_ID: usize = 0x5255_4e41; // "RUNA"

/// A live reading of Win32's foreground window and owner-thread keyboard focus for one window.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct NativeFocus {
    /// `GetForegroundWindow() == hwnd`.
    foreground: bool,
    /// `GetFocus() == hwnd`.
    thread_focus: bool,
}

impl NativeFocus {
    const fn from_handles(hwnd: isize, foreground: isize, focus: isize) -> Self {
        Self {
            foreground: hwnd != 0 && foreground == hwnd,
            thread_focus: hwnd != 0 && focus == hwnd,
        }
    }

    const fn is_focused(self) -> bool {
        self.foreground && self.thread_focus
    }
}

/// Whether UI Automation sees this window as holding keyboard focus.
///
/// On Windows, keyboard focus belongs to a window only when it is both the desktop's foreground
/// window (`GetForegroundWindow`) and its owner thread's keyboard-focus window (`GetFocus`). A
/// window opened behind another process can retain thread focus without owning the foreground,
/// and later gain the foreground through `WM_ACTIVATEAPP` without another `WM_SETFOCUS`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum HostFocus {
    Unfocused,
    Focused,
}

impl HostFocus {
    const fn from_native(native: NativeFocus) -> Self {
        if native.is_focused() { Self::Focused } else { Self::Unfocused }
    }

    const fn is_focused(self) -> bool {
        matches!(self, Self::Focused)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum FocusEvent {
    /// `WM_SETFOCUS`: this window gained keyboard focus on its owner thread's input queue.
    SetFocus(NativeFocus),
    /// `WM_KILLFOCUS`: this window is losing keyboard focus.
    KillFocus,
    /// `WM_ACTIVATEAPP` (`wparam != 0`): this process is gaining foreground activation.
    ActivateApp(NativeFocus),
    /// `WM_ACTIVATEAPP` (`wparam == 0`): another process is taking foreground activation.
    DeactivateApp,
    /// `WM_ACTIVATE` (`WA_ACTIVE` / `WA_CLICKACTIVE`, not minimized): this window is activating.
    Activate(NativeFocus),
    /// `WM_ACTIVATE` (`WA_INACTIVE` or minimized): this window is deactivating.
    Deactivate,
    /// Reconcile against live Win32 state (before a tree update, or after a nested message could
    /// not borrow the adapter).
    Reconcile(NativeFocus),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum FocusEffect {
    /// Tell `accesskit_windows::Adapter` the window's new host-focus state and raise any queued
    /// UIA focus-changed events after releasing the adapter borrow.
    UpdateAdapter(bool),
}

fn step(state: &mut HostFocus, event: FocusEvent) -> Effects<FocusEffect> {
    let next = match event {
        FocusEvent::KillFocus | FocusEvent::DeactivateApp | FocusEvent::Deactivate => HostFocus::Unfocused,
        FocusEvent::SetFocus(native)
        | FocusEvent::ActivateApp(native)
        | FocusEvent::Activate(native)
        | FocusEvent::Reconcile(native) => HostFocus::from_native(native),
    };
    if *state == next { return Effects::none(); }
    *state = next;
    Effects::one(FocusEffect::UpdateAdapter(next.is_focused()))
}

fn focus_event_for_message(message: u32, wparam: usize, sample: impl FnOnce() -> NativeFocus) -> Option<FocusEvent> {
    match message {
        WM_SETFOCUS => Some(FocusEvent::SetFocus(sample())),
        WM_KILLFOCUS => Some(FocusEvent::KillFocus),
        WM_ACTIVATEAPP => Some(if wparam != 0 { FocusEvent::ActivateApp(sample()) } else { FocusEvent::DeactivateApp }),
        WM_ACTIVATE => {
            let active = (wparam & 0xFFFF) as u32 != WA_INACTIVE && ((wparam >> 16) & 0xFFFF) == 0;
            Some(if active { FocusEvent::Activate(sample()) } else { FocusEvent::Deactivate })
        }
        _ => None,
    }
}

fn sample_focus(hwnd: isize) -> NativeFocus {
    // SAFETY: reads foreground and thread-local focus handles; `hwnd` belongs to this thread.
    unsafe { NativeFocus::from_handles(hwnd, GetForegroundWindow() as isize, GetFocus() as isize) }
}

fn raise_events(events: accesskit_windows::QueuedEvents) {
    events.raise();
}

struct State {
    adapter: accesskit_windows::Adapter,
    activation: Activation,
    focus: HostFocus,
}

struct SharedState {
    state: RefCell<State>,
    /// A focus or activation message arrived while `state` was already borrowed; reconcile once
    /// the borrow ends.
    reconcile_pending: Cell<bool>,
}

impl SharedState {
    fn new(hwnd: isize, native: NativeFocus, activation: Activation, actions: Actions) -> Self {
        let focus = HostFocus::from_native(native);
        let adapter = accesskit_windows::Adapter::new(HWND(hwnd as *mut _), focus.is_focused(), actions);
        Self {
            state: RefCell::new(State { adapter, activation, focus }),
            reconcile_pending: Cell::new(false),
        }
    }
}

fn step_adapter_focus(state: &mut State, event: FocusEvent) -> Option<accesskit_windows::QueuedEvents> {
    let mut queued = None;
    for effect in step(&mut state.focus, event) {
        match effect {
            FocusEffect::UpdateAdapter(focused) => {
                queued = state.adapter.update_window_focus_state(focused);
            }
        }
    }
    queued
}

fn apply_focus_event(
    shared: &SharedState,
    hwnd: isize,
    event: FocusEvent,
    sample: &impl Fn(isize) -> NativeFocus,
    raise: &mut impl FnMut(accesskit_windows::QueuedEvents),
) {
    let events = match shared.state.try_borrow_mut() {
        Ok(mut s) => step_adapter_focus(&mut s, event),
        Err(_) => {
            shared.reconcile_pending.set(true);
            return;
        }
    };
    if let Some(events) = events { raise(events); }
    flush_pending_focus(shared, hwnd, sample, raise);
}

fn flush_pending_focus(
    shared: &SharedState,
    hwnd: isize,
    sample: &impl Fn(isize) -> NativeFocus,
    raise: &mut impl FnMut(accesskit_windows::QueuedEvents),
) {
    while shared.reconcile_pending.replace(false) {
        let native = sample(hwnd);
        let events = match shared.state.try_borrow_mut() {
            Ok(mut s) => step_adapter_focus(&mut s, FocusEvent::Reconcile(native)),
            Err(_) => {
                shared.reconcile_pending.set(true);
                return;
            }
        };
        if let Some(events) = events { raise(events); }
    }
}

fn handle_focus_message(
    shared: &SharedState,
    hwnd: isize,
    message: u32,
    wparam: usize,
    sample: &impl Fn(isize) -> NativeFocus,
    raise: &mut impl FnMut(accesskit_windows::QueuedEvents),
) {
    let Some(event) = focus_event_for_message(message, wparam, || sample(hwnd)) else { return };
    apply_focus_event(shared, hwnd, event, sample, raise);
}

fn update_with(
    shared: &SharedState,
    hwnd: isize,
    update: accesskit::TreeUpdate,
    sample: &impl Fn(isize) -> NativeFocus,
    raise: &mut impl FnMut(accesskit_windows::QueuedEvents),
) {
    // Reconcile host focus against live Win32 state before delivering the tree update so a
    // missed or nested focus message cannot leave AccessKit's host-focus flag stale.
    shared.reconcile_pending.set(false);
    apply_focus_event(shared, hwnd, FocusEvent::Reconcile(sample(hwnd)), sample, raise);
    // Raised after the borrow ends: raising may bring a nested WM_GETOBJECT or focus message.
    let events = shared.state.try_borrow_mut().ok().and_then(|mut s| s.adapter.update_if_active(|| update));
    if let Some(events) = events { raise(events); }
    flush_pending_focus(shared, hwnd, sample, raise);
}

pub(super) struct Adapter {
    hwnd: isize,
    shared: Rc<SharedState>,
}

unsafe extern "system" fn subclass(hwnd: windows_sys::Win32::Foundation::HWND, message: u32, wparam: usize, lparam: isize, _: usize, data: usize) -> isize {
    let ptr = data as *const SharedState;
    if message == WM_NCDESTROY {
        // SAFETY: removes this subclass; if it was still installed, reclaims the ref `new` gave it.
        unsafe {
            if RemoveWindowSubclass(hwnd, Some(subclass), SUBCLASS_ID) != 0 {
                drop(Rc::from_raw(ptr));
            }
            return DefSubclassProc(hwnd, message, wparam, lparam);
        }
    }
    // Hold a strong ref across UIA callbacks and `DefSubclassProc` so dropping `Adapter`
    // re-entrantly cannot free `SharedState` while this frame is still on the stack.
    // SAFETY: `ptr` is the live `Rc<SharedState>` owned by the subclass until removal.
    let shared: Rc<SharedState> = unsafe {
        Rc::increment_strong_count(ptr);
        Rc::from_raw(ptr)
    };
    match message {
        WM_GETOBJECT => {
            flush_pending_focus(&shared, hwnd as isize, &sample_focus, &mut raise_events);
            // The answer calls into UIA (`UiaReturnRawElementProvider`), which may send a nested
            // `WM_GETOBJECT` or focus message: drop the `RefCell` borrow before converting it.
            let result = shared.state.try_borrow_mut().ok().and_then(|mut s| {
                let s = &mut *s;
                s.adapter.handle_wm_getobject(WPARAM(wparam), LPARAM(lparam), &mut s.activation)
            });
            flush_pending_focus(&shared, hwnd as isize, &sample_focus, &mut raise_events);
            if let Some(result) = result {
                let result: LRESULT = result.into();
                flush_pending_focus(&shared, hwnd as isize, &sample_focus, &mut raise_events);
                return result.0;
            }
        }
        // Reduced motion, a contrast theme or the system colours changed: the runtime reads them
        // again, without waiting for the window's next activation.
        WM_SETTINGCHANGE | WM_SYSCOLORCHANGE | WM_THEMECHANGED => {
            if let Ok(s) = shared.state.try_borrow() { s.activation.0.push(|i| i.settings = true); }
        }
        WM_SETFOCUS | WM_KILLFOCUS | WM_ACTIVATEAPP | WM_ACTIVATE => {
            handle_focus_message(&shared, hwnd as isize, message, wparam, &sample_focus, &mut raise_events);
        }
        _ => {}
    }
    // SAFETY: forwarding the message this subclass received.
    unsafe { DefSubclassProc(hwnd, message, wparam, lparam) }
}

impl Adapter {
    pub(super) fn new(window: &winit::window::Window, activation: Activation, actions: Actions) -> Option<Self> {
        let RawWindowHandle::Win32(h) = window.window_handle().ok()?.as_raw() else { return None };
        let hwnd = h.hwnd.get();
        let shared = Rc::new(SharedState::new(hwnd, sample_focus(hwnd), activation, actions));
        let data = Rc::into_raw(shared.clone());
        // SAFETY: `data` is reclaimed exactly once, either by `Drop` or by `WM_NCDESTROY`,
        // whichever succeeds in removing the subclass.
        let ok = unsafe { SetWindowSubclass(hwnd as _, Some(subclass), SUBCLASS_ID, data as usize) };
        if ok == 0 {
            // SAFETY: `SetWindowSubclass` failed so the subclass never took ownership of `data`;
            // reclaim the `Rc` ref created by `Rc::into_raw` above.
            unsafe { drop(Rc::from_raw(data)); }
            return None;
        }
        Some(Adapter { hwnd, shared })
    }

    /// Announces what UIA events from AccessKit don't: children added, removed or reordered
    /// (`EVENT_OBJECT_REORDER`) and boxes moved by a scroll (`EVENT_OBJECT_LOCATIONCHANGE`), on
    /// the window's client object, as Win32 controls do.
    pub(super) fn notify(&self, changes: super::tree::Changes) {
        use windows_sys::Win32::UI::Accessibility::NotifyWinEvent;
        use windows_sys::Win32::UI::WindowsAndMessaging::{CHILDID_SELF, EVENT_OBJECT_LOCATIONCHANGE, EVENT_OBJECT_REORDER, OBJID_CLIENT};
        // SAFETY: plain notifications about a live window this thread owns.
        unsafe {
            if changes.structure { NotifyWinEvent(EVENT_OBJECT_REORDER, self.hwnd as _, OBJID_CLIENT, CHILDID_SELF as i32); }
            if changes.moved { NotifyWinEvent(EVENT_OBJECT_LOCATIONCHANGE, self.hwnd as _, OBJID_CLIENT, CHILDID_SELF as i32); }
        }
    }

    pub(super) fn update(&mut self, update: accesskit::TreeUpdate) {
        update_with(&self.shared, self.hwnd, update, &sample_focus, &mut raise_events);
    }
}

impl Drop for Adapter {
    fn drop(&mut self) {
        // SAFETY: removes the callback installed in `new`; if it was still installed (not already
        // removed by `WM_NCDESTROY`), reclaims the `Rc` ref owned by the subclass.
        unsafe {
            if RemoveWindowSubclass(self.hwnd as _, Some(subclass), SUBCLASS_ID) != 0 {
                drop(Rc::from_raw(Rc::as_ptr(&self.shared)));
            }
        }
    }
}

#[cfg(test)]
#[path = "windows_tests.rs"]
mod tests;
