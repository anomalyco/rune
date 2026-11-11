//! The web's stand-in for threads: queued work and served channels run on the main thread from
//! a timer task, a few milliseconds at a time, so frames and input keep their turns between.

use std::cell::RefCell;
use std::collections::VecDeque;
use std::time::Duration;

use wasm_bindgen::JsCast;
use wasm_bindgen::closure::Closure;

use crate::time::Instant;

type Work = Box<dyn FnOnce() + Send>;
type Drain = Box<dyn FnMut() -> Served + Send>;

/// What one drain of a served channel found.
pub(super) enum Served {
    /// Nothing waiting (it may have taken some first).
    Idle,
    /// Its handler broke, or every sender is gone: it's dropped.
    Done,
}

/// The longest one task runs work before it lets the page draw and take input.
const SLICE: Duration = Duration::from_millis(8);

#[derive(Default)]
struct Queue {
    work: VecDeque<Work>,
    /// Served channels; a drain is out (`None`) while it runs.
    drains: Vec<Option<Drain>>,
    /// A timer task is set and hasn't run yet.
    scheduled: bool,
}

thread_local! {
    static QUEUE: RefCell<Queue> = RefCell::default();
}

pub(super) fn queue(work: Work) {
    QUEUE.with(|q| q.borrow_mut().work.push_back(work));
    schedule();
}

pub(super) fn queue_after(delay: Duration, work: Work) {
    let callback = Closure::once_into_js(move || queue(work));
    set_timeout(callback.unchecked_ref(), delay);
}

pub(super) fn serve(drain: Drain) {
    QUEUE.with(|q| q.borrow_mut().drains.push(Some(drain)));
    schedule();
}

/// A timer task to check the served channels, when there are any.
pub(super) fn pump() {
    if QUEUE.with(|q| !q.borrow().drains.is_empty()) {
        schedule();
    }
}

/// Sets one timer task to run what's waiting, unless one is set already.
fn schedule() {
    let set = QUEUE.with(|q| !std::mem::replace(&mut q.borrow_mut().scheduled, true));
    if set {
        let callback = Closure::once_into_js(run);
        set_timeout(callback.unchecked_ref(), Duration::ZERO);
    }
}

/// Drains the served channels and runs queued work until both are empty or the slice is spent;
/// what's left gets another task. The queue is never borrowed while work runs, so work may
/// queue more.
fn run() {
    QUEUE.with(|q| q.borrow_mut().scheduled = false);
    let start = Instant::now();
    loop {
        drain_all();
        let next = QUEUE.with(|q| q.borrow_mut().work.pop_front());
        let Some(work) = next else { break };
        work();
        if start.elapsed() >= SLICE {
            break;
        }
    }
    drain_all();
    if QUEUE.with(|q| !q.borrow().work.is_empty()) {
        schedule();
    }
}

fn drain_all() {
    let count = QUEUE.with(|q| q.borrow().drains.len());
    for i in 0..count {
        let Some(mut drain) = QUEUE.with(|q| q.borrow_mut().drains.get_mut(i).and_then(Option::take)) else { continue };
        let served = drain();
        QUEUE.with(|q| {
            let mut q = q.borrow_mut();
            if let (Served::Idle, Some(slot)) = (served, q.drains.get_mut(i)) {
                *slot = Some(drain);
            }
        });
    }
    QUEUE.with(|q| q.borrow_mut().drains.retain(Option::is_some));
}

fn set_timeout(callback: &js_sys::Function, delay: Duration) {
    if let Some(window) = web_sys::window() {
        let _ = window.set_timeout_with_callback_and_timeout_and_arguments_0(callback, delay.as_millis().min(i32::MAX as u128) as i32);
    }
}
