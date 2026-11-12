use super::*;
use std::sync::atomic::AtomicUsize;

#[test]
fn showing_during_publication_wakes_once() {
    for publisher_first in [true, false] {
        let count = Arc::new(AtomicUsize::new(0));
        let wake = Wake::new({
            let count = count.clone();
            Arc::new(move || { count.fetch_add(1, Ordering::Relaxed); })
        });
        wake.visible(false);
        // Pause the first writer immediately after its atomic change. Let the other
        // operation finish (including its callback) before the first writer resumes.
        if publisher_first {
            wake.publish(|| wake.visible(true));
        } else {
            wake.change_visibility(true, || wake.notify());
        }
        // differs from dev: the overlapping writes must deliver one callback, not two.
        assert_eq!(count.load(Ordering::Relaxed), 1, "publisher first: {publisher_first}");
    }
}
