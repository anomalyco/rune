use super::*;
use crate::app::Shared;

#[derive(Default)]
struct State {
    waiting: bool,
    dispatched: usize,
}

fn events() -> DisplayWatch<State> {
    DisplayWatch::new(DisplayEvents {
        pending: |s| s.waiting,
        dispatch: |s, _| {
            s.waiting = false;
            s.dispatched += 1;
            true
        },
    })
}

#[test]
fn nothing_waiting_queues_no_update() {
    let shared = Shared::<State>::default();
    assert!(!events().queue_if_pending(&State::default(), &shared));
    assert!(!shared.has_updates());
}

#[test]
fn waiting_events_queue_one_dispatch_that_reports_its_change() {
    let shared = Shared::<State>::default();
    let mut state = State { waiting: true, dispatched: 0 };
    let watch = events();
    assert!(watch.queue_if_pending(&state, &shared));
    assert!(!watch.queue_if_pending(&state, &shared), "a queued dispatch isn't queued twice");
    let updates = shared.take_updates();
    assert_eq!(updates.len(), 1);
    let mut tx = Tx::default();
    for f in updates { assert!(f(&mut state, &mut tx)); }
    assert_eq!((state.waiting, state.dispatched), (false, 1));
    assert!(!watch.queue_if_pending(&state, &shared));
}

#[test]
fn builder_stores_the_hooks() {
    let app = App::new(crate::AppSpec::new("Display", (320.0, 200.0), State::default, |_, _| crate::el()))
        .on_display_events(|s| s.waiting, |_, _| false);
    let hooks = app.display_events.expect("hooks stored");
    assert!((hooks.pending)(&State { waiting: true, dispatched: 0 }));
}
