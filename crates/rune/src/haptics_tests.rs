use super::*;
use crate::{Cx, Env, Runtime, el::{El, el}};

#[test]
fn headless_runs_record_haptics_in_order_until_taken() {
    fn view(_: &u32, _: &mut Cx<u32>) -> El {
        el().key("btn").size(10.0, 10.0).on_press_tx(|taps: &mut u32, tx| {
            *taps += 1;
            tx.haptic(Haptic::Impact(ImpactStrength::Medium));
        })
    }
    let mut rt = Runtime::new(0u32, view, (50.0, 50.0), Env::new(true, 1.0));
    rt.frame(0.0);
    assert!(rt.take_haptics().is_empty());
    rt.update(|_, tx| {
        tx.haptic(Haptic::Selection);
        tx.haptic(Haptic::Notify(HapticOutcome::Success));
    });
    rt.pointer_down(5.0, 5.0);
    rt.pointer_up(5.0, 5.0);
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.state, 1);
    assert_eq!(rt.take_haptics(), [
        Haptic::Selection,
        Haptic::Notify(HapticOutcome::Success),
        Haptic::Impact(ImpactStrength::Medium),
    ]);
    assert!(rt.take_haptics().is_empty(), "taking drains them");
}

#[test]
fn a_live_window_keeps_none_and_an_untaken_queue_stays_bounded() {
    let mut rt = Runtime::new((), |_, _| el().size(10.0, 10.0), (50.0, 50.0), Env::new(true, 1.0));
    rt.frame(0.0);
    rt.update(|_, tx| {
        for _ in 0..MAX_PENDING { tx.haptic(Haptic::Selection); }
        tx.haptic(Haptic::Impact(ImpactStrength::Heavy));
    });
    rt.frame(1.0 / 60.0);
    let kept = rt.take_haptics();
    assert_eq!(kept.len(), MAX_PENDING);
    assert_eq!(kept.last(), Some(&Haptic::Impact(ImpactStrength::Heavy)), "the oldest go first");

    rt.host.open_urls_natively = true;
    rt.update(|_, tx| tx.haptic(Haptic::Selection));
    rt.frame(2.0 / 60.0);
    assert!(rt.take_haptics().is_empty(), "nothing plays them on a desktop window yet");
}
