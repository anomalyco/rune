#![allow(unsafe_code)]

use std::sync::{Arc, atomic::{AtomicUsize, Ordering}};

use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, HWND_MESSAGE, MSG,
    PM_REMOVE, PeekMessageW, RegisterClassW, WNDCLASSW,
};

use super::{hands_back, start, turn, turns, wide};

unsafe extern "system" fn test_wndproc(hwnd: HWND, msg: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    // SAFETY: plain default window procedure for a test message-only window.
    unsafe { DefWindowProcW(hwnd, msg, w, l) }
}

fn message_window() -> HWND {
    let class = wide("RuneModalLoopTest");
    // SAFETY: registers a simple message-only window class on this thread and creates one window.
    unsafe {
        let instance = GetModuleHandleW(std::ptr::null());
        let wc = WNDCLASSW {
            lpfnWndProc: Some(test_wndproc),
            hInstance: instance,
            lpszClassName: class.as_ptr(),
            ..std::mem::zeroed()
        };
        RegisterClassW(&wc);
        let hwnd = CreateWindowExW(
            0, class.as_ptr(), std::ptr::null(), 0, 0, 0, 0, 0,
            HWND_MESSAGE, std::ptr::null_mut(), instance, std::ptr::null(),
        );
        assert!(!hwnd.is_null(), "CreateWindowExW failed");
        hwnd
    }
}

#[test]
fn modal_turn_trampoline_blocks_reentrancy_and_keeps_outer_timer_alive() {
    let hwnd = message_window();

    // 1. Re-entrancy: a turn callback firing while a turn is already on the stack must be ignored
    //    (and must not panic if `start` is called or if `turn` re-enters).
    let calls = Arc::new(AtomicUsize::new(0));
    let calls_inner = calls.clone();
    let raw_hwnd = hwnd as usize;
    start(Arc::new(move || {
        let n = calls_inner.fetch_add(1, Ordering::SeqCst);
        if n == 0 {
            // Simulate a nested WM_TIMER firing `turn` while the first turn is still running.
            // SAFETY: invoking the Win32 TIMERPROC with benign test arguments on this thread.
            unsafe { turn(raw_hwnd as HWND, 0, 0, 0); }
        }
    }));
    // SAFETY: invoking the Win32 TIMERPROC on this thread.
    unsafe { turn(hwnd, 0, 0, 0); }
    assert_eq!(calls.load(Ordering::SeqCst), 1, "re-entrant timer trampoline must not nest");

    // 2. Nested `Turns` on the same HWND: dropping the inner guard must not kill the outer timer.
    let fired = Arc::new(AtomicUsize::new(0));
    let fired_inner = fired.clone();
    start(Arc::new(move || { fired_inner.fetch_add(1, Ordering::SeqCst); }));
    let outer = turns(hwnd);
    let inner = turns(hwnd);
    drop(inner);

    let deadline = crate::time::now() + std::time::Duration::from_millis(250);
    while fired.load(Ordering::SeqCst) == 0 && crate::time::now() < deadline {
        std::thread::sleep(std::time::Duration::from_millis(10));
        // SAFETY: pumps messages for this thread's test window.
        unsafe {
            let mut msg: MSG = std::mem::zeroed();
            while PeekMessageW(&mut msg, hwnd, 0, 0, PM_REMOVE) != 0 {
                DispatchMessageW(&msg);
            }
        }
    }
    let count = fired.load(Ordering::SeqCst);
    drop(outer);
    // SAFETY: destroys the message-only window created in this test.
    unsafe { DestroyWindow(hwnd); }
    assert!(count > 0, "dropping an inner Turns guard must not kill the outer modal timer");
}

#[test]
fn modal_menu_guard_prevents_reentrant_menu_loops_and_turns_handles_destroyed_hwnd() {
    let outer = super::enter_menu_loop();
    assert!(outer.is_some());
    assert!(super::enter_menu_loop().is_none(), "nested modal menu loop must be refused");
    drop(outer);
    assert!(super::enter_menu_loop().is_some(), "next modal menu loop succeeds once the previous ends");

    let hwnd = message_window();
    let guard = turns(hwnd);
    // Destroy the window while the modal timer guard is still alive; dropping must not fail or
    // touch a dead window handle.
    // SAFETY: destroys the message-only window created above.
    unsafe { DestroyWindow(hwnd); }
    drop(guard);
}

/// An overlay's menu gives the foreground back to the window that had it, whatever the overlay's
/// `KeyPolicy`: the decision reads only whether the owner is an overlay, not its styles.
#[test]
fn an_overlay_menu_hands_the_foreground_back() {
    let (owner, other, third): (HWND, HWND, HWND) = (std::ptr::without_provenance_mut(1), std::ptr::without_provenance_mut(2), std::ptr::without_provenance_mut(3));
    assert!(hands_back(true, other, owner, owner), "took it for the menu");
    assert!(!hands_back(false, other, owner, owner), "an ordinary window keeps what it took");
    assert!(!hands_back(true, owner, owner, owner), "had it before the menu (an OnDemand overlay the user clicked)");
    assert!(!hands_back(true, std::ptr::null_mut(), owner, owner), "nothing to give it back to");
    assert!(!hands_back(true, other, owner, third), "the foreground moved on meanwhile");
}
