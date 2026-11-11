//! Work off the UI thread, the same code on every target: natively each piece runs on its own
//! named thread; on the web, which has no threads, the main thread runs it from a task after the
//! current event (as [`Latest`](crate::Latest) does there), so it never runs inside a frame. Work
//! that blocks (a synchronous request) still blocks the page while it runs: keep it short, or
//! give the web its own asynchronous path.

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

/// Runs what's waiting: the platform layer calls this after each batch of events on the web.
#[cfg(target_arch = "wasm32")]
pub(crate) fn pump() {
    web::pump();
}
