//! `App::on_display_events` at the window host: when a turn's end checks the app's queue, and
//! when the window keeps winit's wake on display reads.

use std::cell::Cell;
use std::rc::Rc;

use super::any_host::{display_turn_ends, watches_display_reads};
use super::app::test_app_with_display_events;
use crate::app::DisplayEvents;
use crate::runtime::{Env, Runtime};
use crate::{Cx, El, Tx};

#[derive(Default)]
struct State {
    waiting: Rc<Cell<bool>>,
    dispatched: usize,
}

fn view(_: &State, _: &mut Cx<State>) -> El { crate::el() }

fn events() -> DisplayEvents<State> {
    DisplayEvents {
        pending: |s| s.waiting.get(),
        dispatch: |s, _| {
            s.waiting.set(false);
            s.dispatched += 1;
            true
        },
    }
}

fn app(waiting: &Rc<Cell<bool>>) -> (Box<dyn super::WindowApp>, std::sync::Arc<crate::app::Shared<State>>) {
    let state = State { waiting: waiting.clone(), dispatched: 0 };
    test_app_with_display_events(Runtime::new(state, view, (100.0, 100.0), Env::default()), events())
}

#[test]
fn window_not_live_queues_nothing_however_many_turns_end() {
    let waiting = Rc::new(Cell::new(true));
    let (mut app, shared) = app(&waiting);
    // Not started yet, or finished under `keep_running`: its updates never drain.
    for _ in 0..50 { display_turn_ends(&mut *app, false); }
    assert!(!shared.has_updates());
    assert!(!watches_display_reads(&*app, true, false), "a finished window must not keep wake-on-read on");
}

#[test]
fn live_window_queues_one_dispatch_until_it_drains() {
    let waiting = Rc::new(Cell::new(true));
    let (mut app, shared) = app(&waiting);
    assert!(watches_display_reads(&*app, true, true));
    assert!(!watches_display_reads(&*app, false, true), "wake-on-read is Wayland only");

    // Turns that end before the queued dispatch drains don't queue another.
    for _ in 0..10 { display_turn_ends(&mut *app, true); }
    let updates = shared.take_updates();
    assert_eq!(updates.len(), 1);

    let mut drained = State { waiting: waiting.clone(), dispatched: 0 };
    let mut tx = Tx::default();
    for f in updates { assert!(f(&mut drained, &mut tx)); }
    assert_eq!(drained.dispatched, 1);

    // Nothing waits now: no dispatch.
    display_turn_ends(&mut *app, true);
    assert!(!shared.has_updates());

    // New events after the drain get the next dispatch.
    waiting.set(true);
    display_turn_ends(&mut *app, true);
    assert_eq!(shared.take_updates().len(), 1);
}
