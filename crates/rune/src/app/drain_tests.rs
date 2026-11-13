use super::*;
use crate::el::el;
use crate::runtime::{Cx, Env, Runtime};
use std::sync::Barrier;

#[test]
fn drain_orders_focus_at_publication_not_at_wake_or_collection() {
    // UI: take updates, call minimize, return from update, collect requests.
    // Writer: publish focus under the mutex, then finish waking the loop.
    // Place both writer steps in every legal pair of gaps in the UI sequence.
    for publish in (0..5).rev() {
        for finish in publish..5 {
            let shared = Arc::new(Shared::default());
            let handle = AppHandle { shared: shared.clone() };
            let barrier = Arc::new(Barrier::new(2));
            let wake = barrier.clone();
            *shared.wake.lock().unwrap() = Some(Arc::new(move || {
                wake.wait(); // Published; the writer no longer holds the requests mutex.
                wake.wait(); // Allow focus() to finish.
            }));
            let writer = barrier.clone();
            let worker = std::thread::spawn(move || {
                writer.wait();
                handle.focus();
                writer.wait();
            });
            let gap = move |i| {
                if i == publish { barrier.wait(); barrier.wait(); }
                if i == finish { barrier.wait(); barrier.wait(); }
            };
            let in_update = gap.clone();
            shared.queue_update(Box::new(move |_, tx| {
                in_update(1);
                tx.minimize();
                in_update(2);
                false
            }));
            let mut rt = Runtime::new((), |_: &(), _: &mut Cx<()>| el(), (100.0, 100.0), Env::default());
            gap(0);
            let batch = shared.drain_window(&mut rt, |_| gap(3));
            gap(4);
            worker.join().unwrap();
            assert_eq!(batch.activation == Some(Activation::Focus), publish < 4);
            // differs from dev: only a later focus collected in this batch cancels the minimize.
            assert_eq!(rt.minimize.field(), !(2..4).contains(&publish), "publish={publish} finish={finish}");
            let next = shared.drain_window(&mut rt, |_| unreachable!());
            assert_eq!(next.activation == Some(Activation::Focus), publish == 4);
        }
    }
}

#[test]
fn drain_orders_restores_even_if_a_show_replaces_the_window_request() {
    for restore in [false, true] {
        let shared = Arc::new(Shared::default());
        let h = AppHandle { shared: shared.clone() };
        let mut rt = Runtime::new((), |_: &(), _: &mut Cx<()>| el(), (100.0, 100.0), Env::default());
        h.update(|_, tx| tx.minimize());
        let batch = shared.drain_window(&mut rt, |_| {
            if restore { h.open(); }
            h.show();
        });
        assert_eq!(batch.window, Some(WindowRequest::Show));
        assert_eq!(rt.minimize.field(), !restore);
    }
}

#[test]
fn a_pick_callback_that_puts_back_its_tx_discards_the_minimize() {
    // queue an update that minimizes, then restores its saved Tx from a pick miss.
    let shared = Arc::new(Shared::default());
    let mut rt = Runtime::new((), |_: &(), _: &mut Cx<()>| el(), (100.0, 100.0), Env::default());
    shared.queue_update(Box::new(|_, tx| {
        let before = tx.clone();
        tx.minimize();
        // Outside the viewport: nothing to pick.
        tx.element_at::<()>(-50.0, -50.0, move |_, hit, tx| {
            assert!(hit.is_none(), "the pick misses");
            *tx = before;
        });
        false
    }));
    shared.drain_window(&mut rt, |_| {});
    assert!(!rt.minimize.field(), "the callback discarded the update's minimize, as on dev");
}
