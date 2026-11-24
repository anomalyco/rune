use std::time::Duration;

use crate::machine::{Attempt, Seq};
use crate::runtime::web_a11y::A11yMirror;
use crate::time::Instant;
use crate::{Cx, El, Env, Runtime, ScrollSpec};

use super::{
    IDLE_TIMEOUT, SCROLL_MAX_DEFER, SCROLL_SETTLE, ScheduleEffect, ScheduleEvent, ScheduleState,
    step,
};

fn effects(seq: &mut Seq, state: &mut ScheduleState, event: ScheduleEvent) -> Vec<ScheduleEffect> {
    step(seq, state, event).into_iter().collect()
}

/// Test harness mirroring `crates/rune/src/platform/web/input.rs` with an injected clock.
/// Drives `ScheduleState` through the exact same `web_a11y_frame_event`, `web_a11y_settle_event`,
/// `web_a11y_idle_event` and `web_a11y_patch` methods used by the web host so tests and host
/// cannot drift apart.
struct HostHarness<S: 'static> {
    rt: Runtime<S>,
    mirror: A11yMirror,
    seq: Seq,
    state: ScheduleState,
    document: bool,
    now: Instant,
    settle_timer: Option<(Attempt, Instant)>,
    idle_timer: Option<(Attempt, Instant)>,
    idle_delay: Duration,
    raf_flushes: usize,
    flushes: Vec<(Instant, String)>,
    timers_armed: usize,
}

impl<S: 'static> HostHarness<S> {
    fn new(rt: Runtime<S>, now: Instant) -> Self {
        Self {
            rt,
            mirror: A11yMirror::default(),
            seq: Seq::default(),
            state: ScheduleState::default(),
            document: false,
            now,
            settle_timer: None,
            idle_timer: None,
            idle_delay: Duration::from_millis(5),
            raf_flushes: 0,
            flushes: Vec::new(),
            timers_armed: 0,
        }
    }

    fn apply_effects(&mut self, effs: Vec<ScheduleEffect>, at: Instant, in_raf: bool) {
        for eff in effs {
            match eff {
                ScheduleEffect::FlushNow => {
                    if in_raf {
                        self.raf_flushes += 1;
                    }
                    if let Some(json) = self.rt.core().web_a11y_patch(&mut self.mirror) {
                        self.flushes.push((at, json.to_owned()));
                    }
                }
                ScheduleEffect::ScheduleSettle { attempt, delay } => {
                    self.idle_timer = None;
                    self.settle_timer = Some((attempt, at + delay));
                    self.timers_armed += 1;
                }
                ScheduleEffect::ScheduleIdle { attempt, timeout } => {
                    self.settle_timer = None;
                    let delay = self.idle_delay.min(timeout);
                    self.idle_timer = Some((attempt, at + delay));
                    self.timers_armed += 1;
                }
                ScheduleEffect::CancelTimers => {
                    self.settle_timer = None;
                    self.idle_timer = None;
                }
            }
        }
    }

    /// Fires every armed timer whose deadline is `<= target` in chronological order.
    fn advance_timers_to(&mut self, target: Instant) {
        loop {
            let next_settle = self.settle_timer.filter(|&(_, at)| at <= target);
            let next_idle = self.idle_timer.filter(|&(_, at)| at <= target);
            match (next_settle, next_idle) {
                (Some((a, at)), None) => {
                    self.settle_timer = None;
                    self.now = at;
                    let ev = self.rt.core().web_a11y_settle_event(a, at);
                    let effs = effects(&mut self.seq, &mut self.state, ev);
                    self.apply_effects(effs, at, false);
                }
                (None, Some((a, at))) => {
                    self.idle_timer = None;
                    self.now = at;
                    let ev = self.rt.core().web_a11y_idle_event(a, at);
                    let effs = effects(&mut self.seq, &mut self.state, ev);
                    self.apply_effects(effs, at, false);
                }
                (Some((sa, sat)), Some((ia, iat))) => {
                    if sat <= iat {
                        self.settle_timer = None;
                        self.now = sat;
                        let ev = self.rt.core().web_a11y_settle_event(sa, sat);
                        let effs = effects(&mut self.seq, &mut self.state, ev);
                        self.apply_effects(effs, sat, false);
                    } else {
                        self.idle_timer = None;
                        self.now = iat;
                        let ev = self.rt.core().web_a11y_idle_event(ia, iat);
                        let effs = effects(&mut self.seq, &mut self.state, ev);
                        self.apply_effects(effs, iat, false);
                    }
                }
                (None, None) => break,
            }
        }
        self.now = target;
    }

    /// Runs one `rAF` frame (`rt.frame(dt)` + `web_a11y_frame_event`) after firing due timers.
    fn step_frame(&mut self, dt: Duration) {
        let target = self.now + dt;
        self.advance_timers_to(target);
        self.rt.frame(dt.as_secs_f64());
        let ev = self.rt.core().web_a11y_frame_event(&mut self.mirror, target, self.document);
        let effs = effects(&mut self.seq, &mut self.state, ev);
        self.apply_effects(effs, target, true);
    }

    fn consumer_demand(&mut self) {
        let effs = effects(&mut self.seq, &mut self.state, ScheduleEvent::ConsumerDemand);
        self.apply_effects(effs, self.now, false);
    }
}

#[test]
fn scrolling_and_virtual_shifts_defer_all_raf_syncs_until_settle_and_idle() {
    let t0 = crate::time::now();
    let mut seq = Seq::default();
    let mut state = ScheduleState::default();

    // First scroll frame arms the 150 ms settle timer and performs zero synchronous flushes.
    let e0 = effects(&mut seq, &mut state, ScheduleEvent::Frame {
        now: t0,
        document: false,
        dirty: true,
        scrolling: true,
        non_virtual_commit: false,
    });
    assert_eq!(e0, vec![ScheduleEffect::ScheduleSettle { attempt: Attempt(1), delay: SCROLL_SETTLE }]);

    // 480 ms swipe at 60 Hz (including CommitWhy::VIRTUAL shifts): zero FlushNow and zero timer spam
    // while before the armed timer fires; when attempt 1 fires mid-swipe, it re-arms cleanly.
    let mut armed = Attempt(1);
    let mut timer_due = t0 + SCROLL_SETTLE;
    for frame in 1..=30u64 {
        let now = t0 + Duration::from_millis(frame * 16);
        let ef = effects(&mut seq, &mut state, ScheduleEvent::Frame {
            now,
            document: false,
            dirty: true,
            scrolling: true,
            non_virtual_commit: false,
        });
        assert!(!ef.contains(&ScheduleEffect::FlushNow), "must not flush during scroll frame {frame}");
        if now >= timer_due {
            let te = effects(&mut seq, &mut state, ScheduleEvent::SettleDue {
                attempt: armed,
                now,
                scrolling: true,
            });
            assert_eq!(te.len(), 1);
            match te[0] {
                ScheduleEffect::ScheduleSettle { attempt, delay } => {
                    assert_eq!(delay, SCROLL_SETTLE);
                    armed = attempt;
                    timer_due = now + delay;
                }
                other => panic!("expected ScheduleSettle mid-swipe, got {other:?}"),
            }
        }
    }

    // Swipe stops at 480 ms. An intermediate frame 16 ms later sees scrolling=false, still waits.
    let stop_at = t0 + Duration::from_millis(480);
    let after_stop = stop_at + Duration::from_millis(16);
    assert_eq!(
        effects(&mut seq, &mut state, ScheduleEvent::Frame {
            now: after_stop,
            document: false,
            dirty: true,
            scrolling: false,
            non_virtual_commit: false,
        }),
        vec![],
    );

    // If the previously armed timer fires before stop_at + 150 ms, it waits the exact remaining time.
    let mid = stop_at + Duration::from_millis(50);
    let rem_ef = effects(&mut seq, &mut state, ScheduleEvent::SettleDue {
        attempt: armed,
        now: mid,
        scrolling: false,
    });
    assert_eq!(rem_ef.len(), 1);
    let final_attempt = match rem_ef[0] {
        ScheduleEffect::ScheduleSettle { attempt, delay } => {
            assert_eq!(delay, Duration::from_millis(100));
            attempt
        }
        other => panic!("expected remaining settle delay, got {other:?}"),
    };

    // Stale attempt from earlier is ignored.
    assert_eq!(
        effects(&mut seq, &mut state, ScheduleEvent::SettleDue {
            attempt: armed,
            now: stop_at + SCROLL_SETTLE,
            scrolling: false,
        }),
        vec![],
    );

    // At stop_at + 150 ms, settle completes and schedules a bounded off-rAF idle flush.
    let settled_at = stop_at + SCROLL_SETTLE;
    let settle_ef = effects(&mut seq, &mut state, ScheduleEvent::SettleDue {
        attempt: final_attempt,
        now: settled_at,
        scrolling: false,
    });
    let idle_attempt = match settle_ef.as_slice() {
        [ScheduleEffect::ScheduleIdle { attempt, timeout }] => {
            assert_eq!(*timeout, IDLE_TIMEOUT);
            *attempt
        }
        other => panic!("expected ScheduleIdle after settle, got {other:?}"),
    };

    // When idle callback fires, mirror flushes once and returns to Clean.
    assert_eq!(
        effects(&mut seq, &mut state, ScheduleEvent::IdleDue {
            attempt: idle_attempt,
            now: settled_at + Duration::from_millis(4),
            scrolling: false,
        }),
        vec![ScheduleEffect::FlushNow],
    );
    assert_eq!(state, ScheduleState::Clean);
}

#[test]
fn non_scroll_app_commits_never_flush_synchronously_in_raf() {
    let t0 = crate::time::now();
    let mut seq = Seq::default();
    let mut state = ScheduleState::default();

    // First frame and subsequent non-scroll commits schedule off-rAF idle flush (never FlushNow in rAF).
    let e0 = effects(&mut seq, &mut state, ScheduleEvent::Frame {
        now: t0,
        document: false,
        dirty: true,
        scrolling: false,
        non_virtual_commit: true,
    });
    assert_eq!(e0, vec![ScheduleEffect::ScheduleIdle { attempt: Attempt(1), timeout: IDLE_TIMEOUT }]);

    // Another dirty frame before idle runs coalesces into the same pending idle callback.
    let e1 = effects(&mut seq, &mut state, ScheduleEvent::Frame {
        now: t0 + Duration::from_millis(16),
        document: false,
        dirty: true,
        scrolling: false,
        non_virtual_commit: true,
    });
    assert_eq!(e1, vec![]);

    // If scrolling starts before idle runs, IdleDue defers instead of flushing mid-scroll.
    let e2 = effects(&mut seq, &mut state, ScheduleEvent::IdleDue {
        attempt: Attempt(1),
        now: t0 + Duration::from_millis(20),
        scrolling: true,
    });
    assert_eq!(e2, vec![ScheduleEffect::ScheduleSettle { attempt: Attempt(2), delay: SCROLL_SETTLE }]);
    assert!(matches!(state, ScheduleState::ScrollDefer { attempt: Attempt(2), .. }));
}

#[test]
fn consumer_demand_cancels_pending_timers_and_flushes_immediately() {
    let t0 = crate::time::now();
    let mut seq = Seq::default();
    let mut state = ScheduleState::default();

    // Enter ScrollDefer.
    let _ = effects(&mut seq, &mut state, ScheduleEvent::Frame {
        now: t0,
        document: false,
        dirty: true,
        scrolling: true,
        non_virtual_commit: false,
    });
    assert!(matches!(state, ScheduleState::ScrollDefer { .. }));

    // AT action / focusin / beforeprint / armMenu / Ctrl+F forces immediate flush and cancels timers.
    let demand = effects(&mut seq, &mut state, ScheduleEvent::ConsumerDemand);
    assert_eq!(demand, vec![ScheduleEffect::CancelTimers, ScheduleEffect::FlushNow]);
    assert_eq!(state, ScheduleState::Clean);

    // Late timer from before ConsumerDemand is ignored.
    assert_eq!(
        effects(&mut seq, &mut state, ScheduleEvent::SettleDue {
            attempt: Attempt(1),
            now: t0 + SCROLL_SETTLE,
            scrolling: false,
        }),
        vec![],
    );

    // ConsumerDemand when already Clean still flushes (without redundant CancelTimers).
    assert_eq!(
        effects(&mut seq, &mut state, ScheduleEvent::ConsumerDemand),
        vec![ScheduleEffect::FlushNow],
    );
}

#[test]
fn document_mode_flushes_synchronously_on_first_frame_and_commits() {
    let t0 = crate::time::now();
    let mut seq = Seq::default();
    let mut state = ScheduleState::default();

    assert_eq!(
        effects(&mut seq, &mut state, ScheduleEvent::Frame {
            now: t0,
            document: true,
            dirty: true,
            scrolling: false,
            non_virtual_commit: true,
        }),
        vec![ScheduleEffect::FlushNow],
    );
    assert_eq!(state, ScheduleState::Clean);

    assert_eq!(
        effects(&mut seq, &mut state, ScheduleEvent::Frame {
            now: t0 + Duration::from_millis(16),
            document: true,
            dirty: false,
            scrolling: false,
            non_virtual_commit: false,
        }),
        vec![],
    );
}

#[test]
fn host_settle_flushes_within_settle_time_when_frames_run_every_100ms_after_scroll_stops() {
    fn view(_: &(), _: &mut Cx<()>) -> El {
        let rows = crate::col().key("rows").w_full().growth(crate::el::GrowthSpec::default()).virtual_rows(
            200,
            40.0,
            |_: &(), i| std::borrow::Cow::Owned(format!("m{i}")),
            |_: &(), i| crate::text(format!("Row {i}")).w_full().h(40.0),
        );
        crate::col().w_full().h_full().child(
            crate::col().key("pane").w_full().h_full().scroll(ScrollSpec { stick_bottom: false }).child(rows),
        )
    }

    let t0 = crate::time::now();
    let rt = Runtime::new((), view, (390.0, 844.0), Env::new(false, 1.0));
    let mut host = HostHarness::new(rt, t0);

    // Initial frame + initial overscan fill build off rAF, never inside rAF.
    host.step_frame(Duration::ZERO);
    for _ in 0..8 {
        host.step_frame(Duration::from_millis(16));
    }
    assert_eq!(host.raf_flushes, 0);
    host.advance_timers_to(host.now + SCROLL_SETTLE + host.idle_delay);
    assert!(!host.flushes.is_empty(), "initial mirror flushed off rAF");
    host.flushes.clear();

    // User wheel scroll triggers CommitWhy::VIRTUAL window shifts while scrolling and filling overscan.
    host.rt.wheel(200.0, 400.0, -320.0);
    let mut last_scroll_at = host.now;
    for _ in 0..60 {
        let commits_before = host.rt.core().commit.commits;
        let digest_before = host.rt.core().scroll_digest_for_test();
        host.step_frame(Duration::from_millis(16));
        let moved = host.rt.core().web_a11y_scroll_active()
            || host.rt.core().commit.commits != commits_before
            || host.rt.core().scroll_digest_for_test() != digest_before;
        if moved {
            last_scroll_at = host.now;
        } else {
            break;
        }
    }
    assert!(!host.rt.core().web_a11y_scroll_active(), "scroll viewport came to rest");
    assert_eq!(host.raf_flushes, 0, "zero synchronous mirror work in rAF during scroll");
    assert!(host.flushes.is_empty(), "scroll has not settled yet");

    // Reporter's case (#1854): frames arrive every 100 ms (< 150 ms SCROLL_SETTLE) for 2 seconds
    // after scrolling stops. The mirror must flush within SCROLL_SETTLE + idle_delay, not starve.
    for _ in 0..20 {
        host.step_frame(Duration::from_millis(100));
    }
    assert_eq!(host.raf_flushes, 0, "flushes still stay off rAF");
    assert_eq!(host.flushes.len(), 1, "mirror flushed once after scroll settled");
    let (flushed_at, patch_json) = &host.flushes[0];
    let delay = flushed_at.saturating_duration_since(last_scroll_at);
    assert_eq!(
        delay,
        SCROLL_SETTLE + host.idle_delay,
        "mirror must flush right after settle + idle without extending on 100 ms frames",
    );
    let patch: serde_json::Value = serde_json::from_str(patch_json).unwrap();
    let scroll_y = patch["u"]
        .as_array()
        .and_then(|arr| arr.iter().find_map(|it| it.get("sc").and_then(|sc| sc[1].as_f64())))
        .expect("flushed patch carries updated scroll offset");
    assert!(scroll_y > 100.0, "expected scrolled offset > 100, got {scroll_y}");

    // Once flushed on an idle page, further non-dirty frames arm zero timers.
    let timers_before = host.timers_armed;
    for _ in 0..5 {
        host.step_frame(Duration::from_millis(100));
    }
    assert_eq!(host.timers_armed, timers_before, "idle page arms no timers");
    assert!(host.settle_timer.is_none() && host.idle_timer.is_none());
}

#[test]
fn host_scroll_on_idle_page_flushes_without_follow_up_frame_and_consumer_demand_is_immediate() {
    fn view(_: &(), _: &mut Cx<()>) -> El {
        let mut list = crate::col().key("list").w_full();
        for i in 0..40 {
            list = list.child(crate::text(format!("Row {i}")).key(format!("r{i}")).w_full().h(40.0));
        }
        crate::col().w_full().h_full().child(
            crate::col().key("pane").w_full().h_full().scroll(ScrollSpec { stick_bottom: false }).child(list),
        )
    }

    let t0 = crate::time::now();
    let rt = Runtime::new((), view, (390.0, 400.0), Env::new(false, 1.0));
    let mut host = HostHarness::new(rt, t0);
    host.step_frame(Duration::ZERO);
    host.advance_timers_to(t0 + Duration::from_millis(10));
    host.flushes.clear();

    // Instant scroll that leaves the page completely idle (no follow-up rAF after this frame).
    host.rt.update(|_, tx| tx.scroll_to_instant("pane", 0.0, 180.0));
    host.step_frame(Duration::from_millis(16));
    assert_eq!(host.raf_flushes, 0);
    assert!(host.settle_timer.is_some());

    // Without any follow-up frame, timers alone settle and flush cleanly.
    host.advance_timers_to(host.now + SCROLL_SETTLE + host.idle_delay);
    assert_eq!(host.flushes.len(), 1, "idle page flushed after settle timer without extra frame");
    assert_eq!(host.state, ScheduleState::Clean);
    assert!(host.settle_timer.is_none() && host.idle_timer.is_none());

    // ConsumerDemand during active scroll flushes immediately and cancels pending timers.
    host.flushes.clear();
    host.rt.wheel(200.0, 200.0, -160.0);
    host.step_frame(Duration::from_millis(16));
    assert!(host.settle_timer.is_some());
    assert!(host.flushes.is_empty());
    host.consumer_demand();
    assert_eq!(host.flushes.len(), 1, "AT/consumer demand flushes immediately mid-scroll");
    assert!(host.settle_timer.is_none() && host.idle_timer.is_none());
    assert_eq!(host.state, ScheduleState::Clean);
}

#[test]
fn max_defer_bound_prevents_starvation_under_repeated_scrolling_signals() {
    let t0 = crate::time::now();
    let mut seq = Seq::default();
    let mut state = ScheduleState::default();

    let mut settle_timer: Option<(Attempt, Instant)> = None;
    let mut idle_timer: Option<(Attempt, Instant)> = None;
    let mut flushed_at: Option<Instant> = None;

    let apply = |effs: Vec<ScheduleEffect>,
                 at: Instant,
                 in_raf: bool,
                 settle_timer: &mut Option<(Attempt, Instant)>,
                 idle_timer: &mut Option<(Attempt, Instant)>,
                 flushed_at: &mut Option<Instant>| {
        for eff in effs {
            match eff {
                ScheduleEffect::FlushNow => {
                    assert!(!in_raf, "max-defer bound must still flush off rAF");
                    *flushed_at = Some(at);
                }
                ScheduleEffect::ScheduleSettle { attempt, delay } => {
                    *idle_timer = None;
                    *settle_timer = Some((attempt, at + delay));
                }
                ScheduleEffect::ScheduleIdle { attempt, timeout } => {
                    *settle_timer = None;
                    *idle_timer = Some((attempt, at + timeout));
                }
                ScheduleEffect::CancelTimers => {
                    *settle_timer = None;
                    *idle_timer = None;
                }
            }
        }
    };

    // Even if every 100 ms frame (and every timer callback) reports scrolling=true indefinitely,
    // the hard SCROLL_MAX_DEFER bound forces an off-rAF idle flush by SCROLL_MAX_DEFER + IDLE_TIMEOUT.
    for tick in 0..=20u64 {
        let now = t0 + Duration::from_millis(tick * 100);
        loop {
            let next_s = settle_timer.filter(|&(_, at)| at <= now);
            let next_i = idle_timer.filter(|&(_, at)| at <= now);
            match (next_s, next_i) {
                (Some((a, at)), None) => {
                    settle_timer = None;
                    let ef = effects(&mut seq, &mut state, ScheduleEvent::SettleDue { attempt: a, now: at, scrolling: true });
                    apply(ef, at, false, &mut settle_timer, &mut idle_timer, &mut flushed_at);
                }
                (None, Some((a, at))) => {
                    idle_timer = None;
                    let ef = effects(&mut seq, &mut state, ScheduleEvent::IdleDue { attempt: a, now: at, scrolling: true });
                    apply(ef, at, false, &mut settle_timer, &mut idle_timer, &mut flushed_at);
                }
                _ => break,
            }
        }
        if flushed_at.is_some() {
            break;
        }
        let ef = effects(&mut seq, &mut state, ScheduleEvent::Frame {
            now,
            document: false,
            dirty: true,
            scrolling: true,
            non_virtual_commit: false,
        });
        apply(ef, now, true, &mut settle_timer, &mut idle_timer, &mut flushed_at);
    }

    let at = flushed_at.expect("SCROLL_MAX_DEFER must force an off-rAF flush instead of starving");
    let elapsed = at.saturating_duration_since(t0);
    assert!(
        elapsed >= SCROLL_MAX_DEFER && elapsed <= SCROLL_MAX_DEFER + IDLE_TIMEOUT,
        "expected forced off-rAF flush in [{SCROLL_MAX_DEFER:?}, {:?}], got {elapsed:?}",
        SCROLL_MAX_DEFER + IDLE_TIMEOUT,
    );
}
