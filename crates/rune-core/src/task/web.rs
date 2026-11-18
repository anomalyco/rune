//! The web's stand-in for threads: queued work and served channels run on the main thread.
//!
//! With [`start_runners`] (from a JSPI export) they run on a few fibers: each fiber's work may
//! [`wait`] on a promise, which suspends that fiber alone while frames, input and the other
//! fibers go on. Without it they run from a timer task, a few milliseconds at a time, unable to
//! wait. Either way the queue is never borrowed while work runs, so work may queue more.

use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::task::{Poll, Waker};
use std::time::Duration;

use wasm_bindgen::closure::Closure;
use wasm_bindgen::{JsCast, JsValue};

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

/// The longest a timer task or a fiber runs work before it lets the page draw and take input.
const SLICE: Duration = Duration::from_millis(8);
/// Fibers: enough that a slow request doesn't hold up the next, few enough to stay cheap.
const RUNNERS: usize = 4;

/// A served channel's place in the queue.
enum Slot {
    Ready(Drain),
    /// A fiber is draining it now.
    Out,
    /// Its handler broke, or every sender is gone.
    Gone,
}

#[derive(Default)]
struct Queue {
    work: VecDeque<Work>,
    /// Served channels.
    drains: Vec<Slot>,
    /// The served channels have something to check (an event batch or work ran since).
    drains_due: bool,
    /// A timer task is set and hasn't run yet (without fibers).
    scheduled: bool,
    /// The fibers, once started; the idle ones wait here to be woken.
    fibers: Option<Vec<Waker>>,
}

thread_local! {
    static QUEUE: RefCell<Queue> = RefCell::default();
    /// Work is running on a fiber now (not suspended): only then may it wait.
    static IN_WORK: Cell<bool> = const { Cell::new(false) };
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
    QUEUE.with(|q| {
        let mut q = q.borrow_mut();
        q.drains.push(Slot::Ready(drain));
        q.drains_due = true;
    });
    schedule();
}

/// After a batch of events: the served channels get a look, when there are any.
pub(super) fn pump() {
    let due = QUEUE.with(|q| {
        let mut q = q.borrow_mut();
        q.drains_due |= !q.drains.is_empty();
        q.drains_due
    });
    if due {
        schedule();
    }
}

/// Gets what's waiting run: an idle fiber woken, or one timer task set. With every fiber busy,
/// nothing: each looks at the queue again before it rests.
fn schedule() {
    enum Then {
        Wake(Waker),
        Timer,
        Nothing,
    }
    let then = QUEUE.with(|q| {
        let mut q = q.borrow_mut();
        if let Some(idle) = &mut q.fibers {
            return idle.pop().map_or(Then::Nothing, Then::Wake);
        }
        if std::mem::replace(&mut q.scheduled, true) { Then::Nothing } else { Then::Timer }
    });
    match then {
        Then::Wake(fiber) => fiber.wake(),
        Then::Timer => {
            let callback = Closure::once_into_js(run);
            set_timeout(callback.unchecked_ref(), Duration::ZERO);
        }
        Then::Nothing => {}
    }
}
pub(super) fn start_runners() {
    let started = QUEUE.with(|q| {
        let mut q = q.borrow_mut();
        if q.fibers.is_some() {
            return false;
        }
        q.fibers = Some(Vec::new());
        true
    });
    if started {
        // Spawned from a JSPI context, each fiber's polls are entered as promising: its work can wait.
        for _ in 0..RUNNERS {
            wasm_bindgen_futures::spawn_local(fiber());
        }
    }
}

pub(super) fn can_wait() -> bool {
    IN_WORK.get()
}

pub(super) fn wait(promise: &js_sys::Promise) -> Option<Result<JsValue, JsValue>> {
    if !IN_WORK.get() {
        return None;
    }
    // While this fiber is suspended the page runs other code, which is not task work.
    IN_WORK.set(false);
    #[allow(deprecated)]
    let settled = js_sys::futures::jspi_block_on_promise(promise);
    IN_WORK.set(true);
    Some(settled)
}

pub(super) fn sleep(wait_for: Duration) {
    let timer = js_sys::Promise::new(&mut |resolve, _| {
        if let Some(window) = web_sys::window() {
            let _ = window.set_timeout_with_callback_and_timeout_and_arguments_0(&resolve, millis(wait_for));
        }
    });
    let _ = wait(&timer);
}

/// One fiber: takes what's waiting, a slice at a time, and rests when there's nothing.
async fn fiber() {
    loop {
        let start = Instant::now();
        while start.elapsed() < SLICE {
            if !step() {
                rest().await;
                break;
            }
        }
        if start.elapsed() >= SLICE {
            yield_to_page().await;
        }
    }
}

/// Runs one piece of what's waiting: the served channels when they're due, else the next work.
/// False when there was nothing.
fn step() -> bool {
    let drains = QUEUE.with(|q| std::mem::take(&mut q.borrow_mut().drains_due));
    if drains {
        IN_WORK.set(true);
        drain_all();
        IN_WORK.set(false);
        return true;
    }
    let Some(work) = QUEUE.with(|q| q.borrow_mut().work.pop_front()) else { return false };
    IN_WORK.set(true);
    work();
    IN_WORK.set(false);
    // What the work fed a served channel gets a look next.
    QUEUE.with(|q| {
        let mut q = q.borrow_mut();
        q.drains_due |= !q.drains.is_empty();
    });
    true
}

/// Waits to be woken by `schedule`.
fn rest() -> impl std::future::Future<Output = ()> {
    let mut parked = false;
    std::future::poll_fn(move |cx| {
        if parked {
            return Poll::Ready(());
        }
        parked = true;
        QUEUE.with(|q| {
            if let Some(idle) = &mut q.borrow_mut().fibers {
                idle.push(cx.waker().clone());
            }
        });
        Poll::Pending
    })
}

/// Lets the page draw and take input before the fiber goes on.
fn yield_to_page() -> impl std::future::Future<Output = ()> {
    let mut set = false;
    std::future::poll_fn(move |cx| {
        if set {
            return Poll::Ready(());
        }
        set = true;
        let waker = cx.waker().clone();
        let callback = Closure::once_into_js(move || waker.wake());
        set_timeout(callback.unchecked_ref(), Duration::ZERO);
        Poll::Pending
    })
}

/// Without fibers: drains the served channels and runs queued work until both are empty or the
/// slice is spent; what's left gets another task.
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
        let Some(mut drain) = QUEUE.with(|q| match q.borrow_mut().drains.get_mut(i) {
            Some(slot @ Slot::Ready(_)) => match std::mem::replace(slot, Slot::Out) {
                Slot::Ready(drain) => Some(drain),
                _ => None,
            },
            _ => None,
        }) else { continue };
        let served = drain();
        QUEUE.with(|q| {
            if let Some(slot) = q.borrow_mut().drains.get_mut(i) {
                *slot = match served {
                    Served::Idle => Slot::Ready(drain),
                    Served::Done => Slot::Gone,
                };
            }
        });
    }
    // Slots move only while none is out: a fiber running a drain puts it back by its place.
    QUEUE.with(|q| {
        let mut q = q.borrow_mut();
        if !q.drains.iter().any(|s| matches!(s, Slot::Out)) {
            q.drains.retain(|s| matches!(s, Slot::Ready(_)));
        }
    });
}
fn millis(d: Duration) -> i32 {
    d.as_millis().min(i32::MAX as u128) as i32
}

fn set_timeout(callback: &js_sys::Function, delay: Duration) {
    if let Some(window) = web_sys::window() {
        let _ = window.set_timeout_with_callback_and_timeout_and_arguments_0(callback, millis(delay));
    }
}
