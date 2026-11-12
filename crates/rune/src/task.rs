//! Work off the UI thread, the same code on every target: natively each piece runs on its own
//! named thread; on the web, which has no threads, the main thread runs it from a task after the
//! current event, so it never runs inside a frame.
//!
//! On the web the work can wait without blocking the page: after [`start_runners`] (called from
//! a `#[wasm_bindgen(jspi)]` export), it runs on a few JS Promise Integration fibers, and
//! [`wait`] suspends the fiber on a promise (a `fetch`, a timer) while frames and input carry on.
//! So blocking code written for a thread (a request, a [`sleep`]) runs unchanged and costs the
//! page nothing while it waits.

use std::ops::ControlFlow;
use std::sync::mpsc::Receiver;
use std::time::Duration;

#[cfg(target_arch = "wasm32")]
mod web;

/// Runs `work` off the UI thread: a thread named `name`, or a task after the current event on
/// the web. Fails only when the system has no thread left to give.
pub fn spawn(name: impl Into<String>, work: impl FnOnce() + Send + 'static) -> std::io::Result<()> {
    #[cfg(not(target_arch = "wasm32"))]
    {
        std::thread::Builder::new().name(name.into()).spawn(work).map(drop)
    }
    #[cfg(target_arch = "wasm32")]
    {
        let _ = name;
        web::queue(Box::new(work));
        Ok(())
    }
}

/// [`spawn`] after `delay`: a thread that sleeps first, or a timer on the web.
pub fn spawn_after(name: impl Into<String>, delay: Duration, work: impl FnOnce() + Send + 'static) -> std::io::Result<()> {
    #[cfg(not(target_arch = "wasm32"))]
    {
        spawn(name, move || {
            std::thread::sleep(delay);
            work();
        })
    }
    #[cfg(target_arch = "wasm32")]
    {
        let _ = name;
        web::queue_after(delay, Box::new(work));
        Ok(())
    }
}

/// Serves `rx` off the UI thread: `each` takes every message in order until it breaks or every
/// sender is gone. Natively a thread named `name` waits on `rx`; on the web Rune drains it after
/// each batch of events and after each task, since a page can't wait on a channel.
pub fn serve<T: Send + 'static>(
    name: impl Into<String>,
    rx: Receiver<T>,
    mut each: impl FnMut(T) -> ControlFlow<()> + Send + 'static,
) -> std::io::Result<()> {
    #[cfg(not(target_arch = "wasm32"))]
    {
        spawn(name, move || {
            for message in rx {
                if each(message).is_break() {
                    return;
                }
            }
        })
    }
    #[cfg(target_arch = "wasm32")]
    {
        let _ = name;
        web::serve(Box::new(move || {
            use std::sync::mpsc::TryRecvError;
            loop {
                match rx.try_recv() {
                    Ok(message) => {
                        if each(message).is_break() {
                            return web::Served::Done;
                        }
                    }
                    Err(TryRecvError::Empty) => return web::Served::Idle,
                    Err(TryRecvError::Disconnected) => return web::Served::Done,
                }
            }
        }));
        Ok(())
    }
}

/// Sleeps `wait`: the thread natively; on the web, the task's fiber (the page carries on), or
/// not at all outside task work, where a page can't wait.
pub fn sleep(wait: Duration) {
    #[cfg(not(target_arch = "wasm32"))]
    std::thread::sleep(wait);
    #[cfg(target_arch = "wasm32")]
    web::sleep(wait);
}

/// Starts the web's task fibers: from here on, task work runs where [`wait`] can suspend it.
/// Call it from a `#[wasm_bindgen(jspi)]` export (an app's entry), before the app runs; outside
/// one, nothing changes and the work runs as before, unable to wait.
#[cfg(target_arch = "wasm32")]
pub fn start_runners() {
    web::start_runners();
}

/// Waits for `promise` from task work without blocking the page: the work's fiber suspends
/// until it settles. `None` where nothing can wait (no runners, or not in task work): the caller
/// does it the blocking way. Ask [`can_wait`] before starting what the promise stands for (a
/// request), so code that can't wait doesn't start it and then do it again the blocking way.
#[cfg(target_arch = "wasm32")]
pub fn wait(promise: &js_sys::Promise) -> Option<Result<wasm_bindgen::JsValue, wasm_bindgen::JsValue>> {
    web::wait(promise)
}

/// Runs what's waiting: the platform layer calls this after each batch of events on the web.
#[cfg(target_arch = "wasm32")]
pub(crate) fn pump() {
    web::pump();
}

/// Whether [`wait`] can suspend here: task work on a fiber.
#[cfg(target_arch = "wasm32")]
pub fn can_wait() -> bool {
    web::can_wait()
}
