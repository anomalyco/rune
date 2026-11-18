//! A serial service that borrows a task worker only while it has messages.

use std::ops::ControlFlow;
use std::sync::mpsc::SendError;

/// Like [`super::serve`], with a sender that schedules the drain instead of keeping a thread
/// waiting for a message. Messages run in send order, one at a time. Use `serve` for an
/// existing `Receiver` or a service that needs a dedicated thread.
pub fn queue<T: Send + 'static>(name: impl Into<String>, each: impl FnMut(T) -> ControlFlow<()> + Send + 'static) -> Queue<T> {
    #[cfg(not(target_arch = "wasm32"))]
    { Queue { inner: native::Inner::new(name.into(), Box::new(each)) } }
    #[cfg(target_arch = "wasm32")]
    {
        let (inner, rx) = std::sync::mpsc::channel();
        // Served channels on a page already share the task fibers and never park a thread.
        super::serve(name, rx, each).expect("register a web task");
        Queue { inner }
    }
}

/// Sender returned by [`queue`]. Dropping the last sender lets pending work finish.
pub struct Queue<T> {
    #[cfg(not(target_arch = "wasm32"))]
    inner: std::sync::Arc<native::Inner<T>>,
    #[cfg(target_arch = "wasm32")]
    inner: std::sync::mpsc::Sender<T>,
}

impl<T> Clone for Queue<T> {
    fn clone(&self) -> Self { Self { inner: self.inner.clone() } }
}

impl<T: Send + 'static> Queue<T> {
    /// Sends without waiting for previous work. Returns the message if the service stopped
    /// or no worker could start its drain.
    pub fn send(&self, value: T) -> Result<(), SendError<T>> { self.inner.send(value) }
}

#[cfg(not(target_arch = "wasm32"))]
mod native {
    use super::*;
    use std::collections::VecDeque;
    use std::sync::{Arc, Mutex};

    type Handler<T> = Box<dyn FnMut(T) -> ControlFlow<()> + Send>;
    struct Inbox<T> {
        pending: VecDeque<T>,
        // The drain takes the handler to call it and puts it back when idle. When it stops,
        // neither a handler nor a scheduled drain remains.
        each: Option<Handler<T>>,
        draining: bool,
    }
    pub(super) struct Inner<T> { name: String, state: Mutex<Inbox<T>> }

    impl<T: Send + 'static> Inner<T> {
        pub(super) fn new(name: String, each: Handler<T>) -> Arc<Self> {
            Arc::new(Self { name, state: Mutex::new(Inbox { pending: VecDeque::new(), each: Some(each), draining: false }) })
        }

        pub(super) fn send(self: &Arc<Self>, value: T) -> Result<(), SendError<T>> {
            let mut state = self.state.lock().unwrap();
            if !state.draining {
                if state.each.is_none() { return Err(SendError(value)); }
                let this = self.clone();
                if super::super::spawn(self.name.clone(), move || this.drain()).is_err() { return Err(SendError(value)); }
                state.draining = true;
            }
            state.pending.push_back(value);
            Ok(())
        }

        fn drain(&self) {
            let mut each = self.state.lock().unwrap().each.take().expect("one drain per service");
            loop {
                let value = {
                    let mut state = self.state.lock().unwrap();
                    match state.pending.pop_front() {
                        Some(v) => v,
                        None => { state.each = Some(each); state.draining = false; return; }
                    }
                };
                // No queue lock across user work. As with a disconnected channel, a handler
                // that breaks or panics drops queued messages and all later sends fail.
                if !matches!(std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| each(value))), Ok(ControlFlow::Continue(()))) {
                    let mut state = self.state.lock().unwrap();
                    state.draining = false;
                    let pending = std::mem::take(&mut state.pending);
                    drop(state);
                    drop(pending); // a message's destructor may itself try to send
                    return;
                }
            }
        }
    }
}
