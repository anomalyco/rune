//! AppKit display-change notification observer (`NSApplicationDidChangeScreenParametersNotification`)
//! so display additions, removals, scale, mode, and arrangement changes wake the event loop
//! without polling, even when no window is open.

#![allow(unsafe_code)]

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::{MainThreadMarker, class, msg_send};
use objc2_foundation::NSString;

use crate::runtime::Waker;

/// Registered `NSNotificationCenter` observer token for screen parameter changes.
pub(super) struct MacDisplayWatch {
    token: Retained<AnyObject>,
    queued: Arc<AtomicBool>,
}

impl MacDisplayWatch {
    pub(super) fn new(wake: Waker) -> Option<Self> {
        let _mtm = MainThreadMarker::new()?;
        let queued = Arc::new(AtomicBool::new(false));
        let queued_cb = queued.clone();
        let block = block2::RcBlock::new(move |_: *mut AnyObject| {
            if !queued_cb.swap(true, Ordering::AcqRel) {
                wake();
            }
        });
        // SAFETY: called on the main thread; `NSNotificationCenter.defaultCenter` and
        // `addObserverForName:object:queue:usingBlock:` return a retained opaque observer token.
        let token: Option<Retained<AnyObject>> = unsafe {
            let center: Retained<AnyObject> = msg_send![class!(NSNotificationCenter), defaultCenter];
            let name = NSString::from_str("NSApplicationDidChangeScreenParametersNotification");
            msg_send![
                &*center,
                addObserverForName: &*name,
                object: std::ptr::null::<AnyObject>(),
                queue: std::ptr::null::<AnyObject>(),
                usingBlock: &*block
            ]
        };
        Some(Self { token: token?, queued })
    }

    pub(super) fn clear_queued(&self) {
        self.queued.store(false, Ordering::Release);
    }
}

impl Drop for MacDisplayWatch {
    fn drop(&mut self) {
        // SAFETY: `self.token` is a live observer token returned by `NSNotificationCenter`.
        unsafe {
            let center: Retained<AnyObject> = msg_send![class!(NSNotificationCenter), defaultCenter];
            let _: () = msg_send![&*center, removeObserver: &*self.token];
        }
    }
}
