//! AppKit asks the app delegate `applicationShouldTerminate:` before every quit: ⌘Q, the Dock's
//! Quit, logout, restart and shutdown. winit's delegate doesn't implement it, so Rune adds the
//! method to the delegate's class once (as `platform::reopen` does for reopening).
//!
//! When no live host registers `App::on_quit_request` (or a quit is already under way), the
//! delegate marks the app quitting and returns `NSTerminateNow` immediately — bit-identical to
//! an app without the hook. When at least one live host registers `App::on_quit_request`, the
//! delegate steps `quit::request` to `Asking`, wakes the event loop with `WakeEvent::QuitRequest`,
//! and returns `NSTerminateLater`. AppKit enters `NSModalPanelRunLoopMode`, where winit's
//! common-mode proxy and observers still deliver the wake. Once hooks decide, Rune schedules one
//! `CFRunLoopPerformBlock` in `kCFRunLoopCommonModes` so `replyToApplicationShouldTerminate:`
//! runs outside any winit handler (`Loop::Idle`) and synchronous `applicationWillTerminate:` can
//! never re-enter winit's `EventHandler` borrow.

#![allow(unsafe_code)]

use std::cell::{Cell, RefCell};
use std::ffi::{c_char, c_void};
use std::sync::atomic::{AtomicBool, Ordering};

use block2::{Block, RcBlock};
use objc2::runtime::{AnyClass, AnyObject, Bool, Sel};
use objc2::{MainThreadMarker, msg_send, sel};
use objc2_app_kit::NSApplication;

use super::request::{self, RequestEffect, RequestEvent};

type WakeRequest = Box<dyn Fn() -> bool>;

thread_local! {
    /// The delegate class Rune added the method to.
    static HOOKED: Cell<*const AnyClass> = const { Cell::new(std::ptr::null()) };
    /// Wakes the event loop with `WakeEvent::QuitRequest`; `None` before `install`.
    static WAKE_REQUEST: RefCell<Option<WakeRequest>> = const { RefCell::new(None) };
}

/// True when an existing non-winit delegate already implements `applicationShouldTerminate:`.
static FOREIGN_DELEGATE: AtomicBool = AtomicBool::new(false);
/// Ensures the foreign-delegate warning logs at most once per process.
static WARNED_FOREIGN: AtomicBool = AtomicBool::new(false);

unsafe extern "C" {
    fn class_addMethod(class: *const AnyClass, selector: Sel, implementation: *const c_void, types: *const c_char) -> Bool;
}

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    static kCFRunLoopCommonModes: *const c_void;
    fn CFRunLoopGetMain() -> *mut c_void;
    fn CFRunLoopPerformBlock(run_loop: *mut c_void, mode: *const c_void, block: &Block<dyn Fn()>);
    fn CFRunLoopWakeUp(run_loop: *mut c_void);
}

/// `NSTerminateNow`.
const TERMINATE_NOW: usize = 1;
/// `NSTerminateLater`.
const TERMINATE_LATER: usize = 2;

extern "C" fn should_terminate(_: *mut AnyObject, _: Sel, _: *mut AnyObject) -> usize {
    let effects = request::send(RequestEvent::OsQuitRequested {
        has_hooks: request::has_hooks(),
        quitting: super::quitting(),
    });
    for effect in effects {
        match effect {
            RequestEffect::ReturnNow => {
                // Windows::exiting carries out the quit's effects.
                let _ = super::send(super::Event::Start { ending: false, after: super::After::Exit });
                return TERMINATE_NOW;
            }
            RequestEffect::WakeAndReturnLater => {
                let woke = WAKE_REQUEST
                    .with(|w| w.try_borrow().ok().and_then(|w| w.as_ref().map(|f| f())))
                    .unwrap_or(false);
                if woke {
                    return TERMINATE_LATER;
                }
                let _ = request::send(RequestEvent::WakeFailed);
                let _ = super::send(super::Event::Start { ending: false, after: super::After::Exit });
                return TERMINATE_NOW;
            }
            RequestEffect::ReturnLater => return TERMINATE_LATER,
            RequestEffect::ScheduleReply | RequestEffect::SendReply { .. } | RequestEffect::SuppressExit => {}
        }
    }
    let _ = super::send(super::Event::Start { ending: false, after: super::After::Exit });
    TERMINATE_NOW
}

pub(super) fn warn_if_foreign_delegate() {
    if FOREIGN_DELEGATE.load(Ordering::Relaxed) && !WARNED_FOREIGN.swap(true, Ordering::Relaxed) {
        eprintln!(
            "rune: app delegate already implements applicationShouldTerminate:; App::on_quit_request will not be called"
        );
    }
}

pub(super) fn finish_quit_request(allow: bool) {
    for effect in request::send(RequestEvent::Decided { allow }) {
        if effect == RequestEffect::ScheduleReply {
            schedule_reply();
        }
    }
}

pub(super) fn programmatic_quit() -> bool {
    let mut suppress_exit = false;
    for effect in request::send(RequestEvent::ProgrammaticQuit) {
        match effect {
            RequestEffect::ScheduleReply => schedule_reply(),
            RequestEffect::SuppressExit => suppress_exit = true,
            _ => {}
        }
    }
    suppress_exit
}

pub(super) fn on_loop_idle() {
    for effect in request::send(RequestEvent::LoopIdle) {
        if effect == RequestEffect::ScheduleReply {
            schedule_reply();
        }
    }
}

/// Schedules one main-run-loop block in `kCFRunLoopCommonModes` so it runs inside AppKit's
/// `NSModalPanelRunLoopMode` once the active winit handler has returned (`Loop::Idle`).
fn schedule_reply() {
    let ran = Cell::new(false);
    let block = RcBlock::new(move || {
        if ran.replace(true) {
            return;
        }
        flush_reply();
    });
    // SAFETY: called on the main thread; `CFRunLoopGetMain()` and `kCFRunLoopCommonModes` are
    // valid for the process lifetime, and `CFRunLoopPerformBlock` copies `block`.
    unsafe {
        let run_loop = CFRunLoopGetMain();
        CFRunLoopPerformBlock(run_loop, kCFRunLoopCommonModes, &block);
        CFRunLoopWakeUp(run_loop);
    }
}

fn flush_reply() {
    let Some(mtm) = MainThreadMarker::new() else { return };
    for effect in request::send(RequestEvent::FlushReply { loop_state: super::loop_state() }) {
        if let RequestEffect::SendReply { allow } = effect {
            if allow {
                let _ = super::send(super::Event::Start { ending: false, after: super::After::Exit });
            }
            // SAFETY: on the main thread, outside any winit handler (`Loop::Idle`), replying once
            // to a prior `NSTerminateLater` return from `applicationShouldTerminate:`.
            unsafe {
                let app = NSApplication::sharedApplication(mtm);
                let _: () = msg_send![&app, replyToApplicationShouldTerminate: Bool::new(allow)];
            }
        }
    }
}

pub(super) fn install(wake_request: impl Fn() -> bool + 'static) {
    WAKE_REQUEST.with(|w| *w.borrow_mut() = Some(Box::new(wake_request)));
    let Some(mtm) = MainThreadMarker::new() else { return };
    let app = NSApplication::sharedApplication(mtm);
    let selector = sel!(applicationShouldTerminate:);
    // SAFETY: AppKit calls and runtime changes on the main thread. The added function has the
    // selector's ABI (`NSApplicationTerminateReply` is an `NSUInteger`, `Q` on 64-bit), and
    // adding a method the class lacks leaves its other methods and instances alone.
    unsafe {
        let delegate: *mut AnyObject = msg_send![&app, delegate];
        let Some(delegate) = delegate.as_ref() else { return };
        let class = delegate.class();
        if HOOKED.with(|hooked| std::ptr::eq(hooked.get(), class)) {
            return;
        }
        // A later winit, or an app's own delegate, decides itself.
        let answers: bool = msg_send![delegate, respondsToSelector: selector];
        if answers {
            FOREIGN_DELEGATE.store(true, Ordering::Relaxed);
            if request::has_hooks() {
                warn_if_foreign_delegate();
            }
            return;
        }
        let imp = should_terminate as extern "C" fn(*mut AnyObject, Sel, *mut AnyObject) -> usize;
        if !class_addMethod(class, selector, imp as *const c_void, c"Q@:@".as_ptr()).as_bool() {
            eprintln!("rune: could not add the terminate method to the app delegate");
            return;
        }
        HOOKED.with(|hooked| hooked.set(class));
        // AppKit caches which optional methods its delegate answers when it is set.
        let _: () = msg_send![&app, setDelegate: delegate];
    }
}
