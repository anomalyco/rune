use super::*;
use crate::{Cx, Env, Live, Runtime, el::{El, el}};

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
fn same_handler_and_same_frame_bursts_coalesce_identical_haptics() {
    fn view(_: &u32, _: &mut Cx<u32>) -> El {
        el().key("a").size(20.0, 20.0)
            .on_press_tx(|n: &mut u32, tx| {
                *n += 1;
                for _ in 0..10 { tx.haptic(Haptic::Selection); }
            })
            .on_any_press(|_: &mut u32, _, _, tx| {
                tx.haptic(Haptic::Selection);
            })
    }
    let mut rt = Runtime::new(0u32, view, (50.0, 50.0), Env::default());
    rt.frame(0.0);
    rt.pointer_down(5.0, 5.0);
    rt.pointer_up(5.0, 5.0);
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.state, 1);
    assert_eq!(
        rt.take_haptics(),
        [Haptic::Selection],
        "ten detents plus a second handler in the same frame collapse to one Selection",
    );
}

#[test]
fn cooldown_machine_and_runtime_clock_rate_limit_same_kind_but_never_delay_different_kind() {
    // Pure state-machine checks.
    let mut state = CooldownState::Idle;
    let e1: Vec<_> = step_cooldown(&mut state, CooldownEvent::Request { haptic: Haptic::Selection, now: 1.0 }).into_iter().collect();
    assert_eq!(e1, [CooldownEffect::Emit(Haptic::Selection)]);
    assert!(matches!(state, CooldownState::Cooling { last: Haptic::Selection, .. }));

    let e2: Vec<_> = step_cooldown(&mut state, CooldownEvent::Request { haptic: Haptic::Selection, now: 1.029 }).into_iter().collect();
    assert!(e2.is_empty(), "same kind < 30 ms must be suppressed");

    let e3: Vec<_> = step_cooldown(&mut state, CooldownEvent::Request { haptic: Haptic::Impact(ImpactStrength::Light), now: 1.008 }).into_iter().collect();
    assert_eq!(e3, [CooldownEffect::Emit(Haptic::Impact(ImpactStrength::Light))], "different kind fires immediately");

    let tick = step_cooldown(&mut state, CooldownEvent::Tick { now: 1.038 });
    assert!(tick.is_empty());
    assert_eq!(state, CooldownState::Idle, "Tick returns Cooling to Idle once due");

    // End-to-end through Runtime clock (120 Hz scrubbing + variant switch).
    let mut rt = Runtime::new(0u32, |_, _| el().size(10.0, 10.0), (50.0, 50.0), Env::default());
    rt.frame(0.0);
    rt.poll = Some(|step, _, tx| {
        *step += 1;
        match *step {
            1..=4 => tx.haptic(Haptic::Selection),
            5 => tx.haptic(Haptic::Impact(ImpactStrength::Light)),
            6 => tx.haptic(Haptic::Notify(HapticOutcome::Error)),
            _ => {}
        }
        false
    });
    // t = 0.008: emits Selection (cooldown until 0.038)
    rt.frame(0.008);
    // t = 0.016 (< 0.038): suppressed
    rt.frame(0.008);
    // t = 0.024 (< 0.038): suppressed
    rt.frame(0.008);
    // t = 0.040 (>= 0.038): allowed
    rt.frame(0.016);
    // t = 0.048 (+8 ms): different kind Light fires immediately
    rt.frame(0.008);
    // t = 0.056 (+8 ms): different kind Error fires immediately
    rt.frame(0.008);
    assert_eq!(rt.take_haptics(), [
        Haptic::Selection,
        Haptic::Selection,
        Haptic::Impact(ImpactStrength::Light),
        Haptic::Notify(HapticOutcome::Error),
    ]);
}

#[test]
fn live_only_and_update_if_haptic_trigger_zero_view_commits_and_return_to_idle() {
    struct App { offset: Live<[f32; 2]> }
    fn view(s: &App, _: &mut Cx<App>) -> El {
        el().key("knob").size(20.0, 20.0)
            .live_offset(&s.offset)
            .on_press_tx(|s: &mut App, tx| {
                tx.live_only();
                s.offset.set([12.0, 0.0]);
                tx.haptic(Haptic::Selection);
            })
    }
    let mut rt = Runtime::new(App { offset: Live::new([0.0, 0.0]) }, view, (60.0, 60.0), Env::default());
    rt.frame(0.0);
    let commits0 = rt.stats.commits;

    // Pressing a live_only() handler with Tx::haptic must not rebuild or relayout.
    rt.stats.layout_nodes = 0;
    rt.pointer_down(10.0, 10.0);
    rt.pointer_up(10.0, 10.0);
    rt.frame(0.050);
    assert_eq!(rt.stats.commits, commits0, "live_only + haptic triggers 0 view commits");
    assert_eq!(rt.stats.layout_nodes, 0, "live_only + haptic triggers 0 relayouts");
    assert_eq!(rt.take_haptics(), [Haptic::Selection]);

    // update_if returning false with tx.haptic reports stale once, commits 0 views/layouts,
    // and leaves the runtime idle on the next frame.
    rt.stats.layout_nodes = 0;
    let stale = rt.update_if(|_, tx| {
        tx.haptic(Haptic::Impact(ImpactStrength::Rigid));
        false
    });
    assert!(stale, "haptic-only update_if must mark frame_work stale so live hosts wake");
    assert!(!rt.stale(), "pending haptic lives in HostState, so undrained haptics never spin idle loops");
    let animating = rt.frame(0.050);
    assert!(!animating, "haptic-only frame returns to idle");
    assert_eq!(rt.stats.commits, commits0, "haptic-only update_if triggers 0 view commits");
    assert_eq!(rt.stats.layout_nodes, 0, "haptic-only update_if triggers 0 relayouts");
    assert_eq!(rt.take_haptics(), [Haptic::Impact(ImpactStrength::Rigid)]);
}

#[test]
fn disabled_inert_and_unused_elements_emit_nothing_and_stay_idle() {
    fn view(_: &u32, _: &mut Cx<u32>) -> El {
        crate::row().size(80.0, 40.0)
            .child(
                el().key("disabled").size(20.0, 20.0).role("button").disabled(true),
            )
            .child(
                el().key("inert").size(20.0, 20.0).inert()
                    .on_press_tx(|n: &mut u32, tx| {
                        *n += 1;
                        tx.haptic(Haptic::Impact(ImpactStrength::Heavy));
                    }),
            )
            .child(
                el().key("uninvoked").size(20.0, 20.0)
                    .on_press_tx(|n: &mut u32, tx| {
                        *n += 1;
                        tx.haptic(Haptic::Selection);
                    }),
            )
    }
    let mut rt = Runtime::new(0u32, view, (80.0, 40.0), Env::default());
    rt.frame(0.0);
    let commits0 = rt.stats.commits;
    // Click disabled element, click inert element, and never click uninvoked element.
    rt.pointer_down(5.0, 5.0);
    rt.pointer_up(5.0, 5.0);
    rt.pointer_down(25.0, 5.0);
    rt.pointer_up(25.0, 5.0);
    let animating = rt.frame(1.0 / 60.0);
    assert!(!animating);
    assert_eq!(rt.state, 0);
    assert_eq!(rt.stats.commits, commits0);
    assert!(rt.take_haptics().is_empty(), "disabled, inert and uninvoked elements emit no haptics");
}

#[test]
fn inline_ring_buffer_stays_bounded_dropping_oldest_and_flush_with_respects_visibility() {
    let mut rt = Runtime::new((), |_, _| el().size(10.0, 10.0), (50.0, 50.0), Env::new(true, 1.0));
    rt.frame(0.0);
    // Alternate distinct variants so cooldown does not suppress them, overflowing MAX_PENDING.
    let seq = [
        Haptic::Selection,
        Haptic::Impact(ImpactStrength::Light),
        Haptic::Impact(ImpactStrength::Medium),
    ];
    rt.update(|_, tx| {
        for i in 0..MAX_PENDING { tx.haptic(seq[i % seq.len()]); }
        tx.haptic(Haptic::Impact(ImpactStrength::Heavy));
    });
    rt.frame(1.0 / 60.0);
    let kept = rt.take_haptics();
    assert_eq!(kept.len(), MAX_PENDING);
    assert_eq!(kept.first(), Some(&seq[1]), "oldest entry dropped on overflow");
    assert_eq!(kept.last(), Some(&Haptic::Impact(ImpactStrength::Heavy)), "newest entry kept");
    assert!(rt.take_haptics().is_empty());

    // Live visible window dispatches through flush_with; covered/hidden or non-native discards.
    rt.host.open_urls_natively = true;
    rt.update(|_, tx| tx.haptic(Haptic::Selection));
    let mut played = Vec::new();
    flush_with(rt.core_mut(), true, |h| played.push(h));
    assert_eq!(played, [Haptic::Selection]);
    assert!(rt.take_haptics().is_empty());

    // Covered/hidden frame discards without calling the performer.
    played.clear();
    rt.update(|_, tx| tx.haptic(Haptic::Impact(ImpactStrength::Medium)));
    flush_with(rt.core_mut(), false, |h| played.push(h));
    assert!(played.is_empty(), "hidden/covered frame must never play haptics");
    assert!(rt.take_haptics().is_empty(), "hidden/covered frame empties pending queue");

    // Scripted/non-native run discards without calling hardware performer.
    rt.host.open_urls_natively = false;
    rt.update(|_, tx| tx.haptic(Haptic::Notify(HapticOutcome::Success)));
    flush_with(rt.core_mut(), true, |h| played.push(h));
    assert!(played.is_empty(), "scripted/headless run must not call hardware performer");
    assert!(rt.take_haptics().is_empty());
}

#[test]
fn on_mount_haptics_commit_at_end_of_frame() {
    fn view(_: &(), _: &mut Cx<()>) -> El {
        el().key("mounted").size(10.0, 10.0).on_mount(|_: &mut (), tx| {
            tx.haptic(Haptic::Notify(HapticOutcome::Success));
        })
    }
    let mut rt = Runtime::new((), view, (40.0, 40.0), Env::default());
    rt.frame(0.0);
    assert_eq!(rt.take_haptics(), [Haptic::Notify(HapticOutcome::Success)]);
}
