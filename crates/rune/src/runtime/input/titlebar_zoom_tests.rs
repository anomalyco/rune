//! Regression tests for #2355: rapid custom titlebar double-clicks while macOS `NSWindow::zoom:`
//! blocks the main thread must not replay queued drags or zooms, while deliberate double-clicks
//! after the toggle completes still toggle normally.

use std::time::Duration;

use super::{Event, Request, TitlebarPress, step};
use crate::el::{col, el};
use crate::runtime::{Runtime, TitlebarAction};
use crate::{Cx, El, Env};

const POINT: (f32, f32) = (200.0, 20.0);
const INTERVAL: f64 = 0.5;

fn effects(e: crate::machine::Effects<Request>) -> Vec<Request> {
    e.into_iter().collect()
}

#[test]
fn pure_machine_drops_clicks_during_toggling_and_before_until_then_accepts_deliberate_double_click() {
    let mut state = TitlebarPress::Rest;

    // 1. First double-click at t=0.00, 0.06 -> Drag then DoubleClick.
    assert_eq!(
        effects(step(&mut state, Event::Region { now: 0.00, point: POINT, interval: INTERVAL, defer: false })),
        vec![Request::Action(TitlebarAction::Drag)],
    );
    assert_eq!(
        effects(step(&mut state, Event::Region { now: 0.06, point: POINT, interval: INTERVAL, defer: false })),
        vec![Request::Action(TitlebarAction::DoubleClick)],
    );

    // Host starts synchronous 375 ms zoom animation (0.06..0.435).
    assert!(effects(step(&mut state, Event::ToggleStart)).is_empty());
    assert_eq!(state, TitlebarPress::Toggling);

    // Any press arriving re-entrantly while Toggling is dropped.
    assert!(effects(step(&mut state, Event::Region { now: 0.10, point: POINT, interval: INTERVAL, defer: false })).is_empty());
    assert!(effects(step(&mut state, Event::Empty { now: 0.11, point: POINT, interval: INTERVAL, band: true })).is_empty());
    assert_eq!(state, TitlebarPress::Toggling);

    // Host finishes zoom at t=0.435.
    assert!(effects(step(&mut state, Event::ToggleEnd { until: 0.435 })).is_empty());
    assert_eq!(state, TitlebarPress::Settled { until: 0.435 });

    // 2. Four extra double-clicks (8 clicks) whose native timestamps were queued during 0.06..0.435
    //    are ALL dropped with zero Drag and zero DoubleClick.
    for &t in &[0.12, 0.18, 0.24, 0.30, 0.36, 0.40, 0.42, 0.434] {
        assert!(
            effects(step(&mut state, Event::Region { now: t, point: POINT, interval: INTERVAL, defer: false })).is_empty(),
            "stale press at t={t} (< 0.435) must be dropped",
        );
        assert!(effects(step(&mut state, Event::Release)).is_empty());
    }
    assert_eq!(state, TitlebarPress::Settled { until: 0.435 });

    // Focus/pointer cancel during Settled must not wipe the watermark.
    assert!(effects(step(&mut state, Event::Clear)).is_empty());
    assert_eq!(state, TitlebarPress::Settled { until: 0.435 });
    assert!(effects(step(&mut state, Event::Region { now: 0.4349, point: POINT, interval: INTERVAL, defer: false })).is_empty());

    // 3. A click straddling until: stale first click at 0.43 (dropped) + fresh click at 0.44
    //    must NOT pair into a spurious double-click; 0.44 starts a fresh single press.
    assert!(effects(step(&mut state, Event::Region { now: 0.43, point: POINT, interval: INTERVAL, defer: false })).is_empty());
    assert_eq!(
        effects(step(&mut state, Event::Region { now: 0.44, point: POINT, interval: INTERVAL, defer: false })),
        vec![Request::Action(TitlebarAction::Drag)],
    );

    // 4. Deliberate second click at 0.50 (after zoom completed) immediately toggles back!
    assert_eq!(
        effects(step(&mut state, Event::Region { now: 0.50, point: POINT, interval: INTERVAL, defer: false })),
        vec![Request::Action(TitlebarAction::DoubleClick)],
    );
}

#[test]
fn pure_machine_native_band_empty_drops_stale_clicks_and_non_blocking_toggle_preserves_linux_windows() {
    // 1. Native titlebar band (Event::Empty { band: true }) obeys the same Settled { until } gate.
    let mut band = TitlebarPress::Rest;
    assert!(effects(step(&mut band, Event::Empty { now: 1.00, point: POINT, interval: INTERVAL, band: true })).is_empty());
    assert_eq!(
        effects(step(&mut band, Event::Empty { now: 1.05, point: POINT, interval: INTERVAL, band: true })),
        vec![Request::Action(TitlebarAction::DoubleClick)],
    );
    let _ = step(&mut band, Event::ToggleStart);
    let _ = step(&mut band, Event::ToggleEnd { until: 1.425 });
    for &t in &[1.10, 1.15, 1.30, 1.35] {
        assert!(effects(step(&mut band, Event::Empty { now: t, point: POINT, interval: INTERVAL, band: true })).is_empty());
    }
    // Deliberate double-click after 1.425 works.
    assert!(effects(step(&mut band, Event::Empty { now: 1.50, point: POINT, interval: INTERVAL, band: true })).is_empty());
    assert_eq!(
        effects(step(&mut band, Event::Empty { now: 1.55, point: POINT, interval: INTERVAL, band: true })),
        vec![Request::Action(TitlebarAction::DoubleClick)],
    );

    // 2. Non-blocking toggle (Windows/Linux: until == press time) in deferred-drag mode still
    //    accepts an immediate follow-up double-click with zero Drag.
    let mut linux = TitlebarPress::Rest;
    assert!(effects(step(&mut linux, Event::Region { now: 2.00, point: POINT, interval: INTERVAL, defer: true })).is_empty());
    let _ = step(&mut linux, Event::Release);
    assert_eq!(
        effects(step(&mut linux, Event::Region { now: 2.05, point: POINT, interval: INTERVAL, defer: true })),
        vec![Request::Action(TitlebarAction::DoubleClick)],
    );
    let _ = step(&mut linux, Event::ToggleStart);
    let _ = step(&mut linux, Event::ToggleEnd { until: 2.05 });
    // Immediate click at t=2.05 (== until) is NOT dropped.
    assert!(effects(step(&mut linux, Event::Region { now: 2.05, point: POINT, interval: INTERVAL, defer: true })).is_empty());
    let _ = step(&mut linux, Event::Release);
    assert_eq!(
        effects(step(&mut linux, Event::Region { now: 2.10, point: POINT, interval: INTERVAL, defer: true })),
        vec![Request::Action(TitlebarAction::DoubleClick)],
    );
}

#[test]
fn host_seam_counts_one_zoom_and_one_drag_for_five_queued_double_clicks_then_allows_deliberate_toggle() {
    fn view(_: &(), _: &mut Cx<()>) -> El {
        col().size(400.0, 200.0)
            .child(el().key("bar").size(400.0, 40.0).window_drag())
            .child(el().key("body").size(400.0, 160.0))
    }

    let mut rt = Runtime::new((), view, (400.0, 200.0), Env::new(true, 2.0));
    let wall_start = crate::time::now();
    rt.wall_start = Some(wall_start);
    rt.frame.set_test_clock(crate::ManualClock::new(60.0));
    rt.double_click_interval = 0.5;
    // Exercise macOS immediate-drag mode on every platform.
    rt.host.titlebar.defer_drag_override = Some(false);
    rt.frame(0.0);

    let mut drags = 0usize;
    let mut zooms = 0usize;

    // Helper: deliver one mouse-down + mouse-up at native hardware timestamp `native_s` while
    // the main thread's delivery clock is `delivery_s`. On DoubleClick, simulate a synchronous
    // 375 ms NSWindow::zoom: blocking the main thread.
    let mut deliver_click = |rt: &mut Runtime<()>, native_s: f64, delivery_s: &mut f64| {
        if *delivery_s < native_s {
            *delivery_s = native_s;
        }
        rt.frame.test_clock_mut().unwrap().seek(*delivery_s);
        let native_instant = wall_start + Duration::from_secs_f64(native_s);
        {
            let _scope = crate::input::time::scope(Some(native_instant));
            rt.pointer_move(200.0, 20.0);
            rt.pointer_down(200.0, 20.0);
        }
        if let Some(action) = rt.take_titlebar_action() {
            // Advance the manual clock inside DoubleClick to model synchronous 375 ms NSWindow::zoom:.
            let toggling = action == TitlebarAction::DoubleClick;
            if toggling {
                rt.core_mut().begin_titlebar_toggle();
            }
            match action {
                TitlebarAction::Drag => drags += 1,
                TitlebarAction::DoubleClick => {
                    zooms += 1;
                    *delivery_s += 0.375;
                    rt.frame.test_clock_mut().unwrap().seek(*delivery_s);
                }
            }
            if toggling {
                let until = rt.core().now_clock();
                rt.core_mut().end_titlebar_toggle(until);
            }
        }
        {
            let _scope = crate::input::time::scope(Some(native_instant + Duration::from_millis(10)));
            rt.pointer_up(200.0, 20.0);
        }
    };

    // Five rapid double-clicks (10 clicks, 35 ms apart -> 0.00..0.315 s):
    // Pair 1 (0.00, 0.035) triggers zoom #1 which blocks delivery until t=0.410 s.
    // Pairs 2..5 (0.070..0.315) all have native timestamps < 0.410 s and are delivered right
    // after zoom #1 returns -> ALL 8 queued clicks must be dropped!
    let mut delivery = 0.0;
    for i in 0..10 {
        deliver_click(&mut rt, f64::from(i) * 0.035, &mut delivery);
    }
    assert_eq!(zooms, 1, "5 rapid double-clicks queued during zoom must toggle at most once");
    assert_eq!(drags, 1, "stale queued clicks must not fire Drag (avoids AppKit move-without-beginning warning)");

    // Also verify run_titlebar_action_with directly on a deliberate post-zoom double-click
    // at native timestamps 0.50 s and 0.55 s (both >= 0.410 s) -> toggles a second time!
    for &native_s in &[0.50, 0.55] {
        delivery = delivery.max(native_s);
        rt.frame.test_clock_mut().unwrap().seek(delivery);
        let native_instant = wall_start + Duration::from_secs_f64(native_s);
        {
            let _scope = crate::input::time::scope(Some(native_instant));
            rt.pointer_down(200.0, 20.0);
        }
        if let Some(action) = rt.take_titlebar_action() {
            rt.core_mut().run_titlebar_action_with(action, |a| match a {
                TitlebarAction::Drag => drags += 1,
                TitlebarAction::DoubleClick => zooms += 1,
            });
        }
        rt.pointer_up(200.0, 20.0);
    }
    assert_eq!(zooms, 2, "deliberate double-click after zoom ends must still toggle");
    assert_eq!(drags, 2);
}

#[cfg(target_os = "macos")]
#[test]
fn macos_nsevent_timestamps_separate_queued_zoom_clicks_from_post_zoom_clicks() {
    use objc2_foundation::NSProcessInfo;

    let wall_start = crate::time::now() - Duration::from_secs(10);
    let now = crate::time::now();
    let uptime = NSProcessInfo::processInfo().systemUptime();
    // Zoom finished at `now` (10.0 s since wall_start).
    let until = now.saturating_duration_since(wall_start).as_secs_f64();

    // A queued NSEvent stamped 200 ms earlier during zoom maps to < until.
    let stale_age = (uptime - (uptime - 0.200)).max(0.0);
    let stale_instant = now.checked_sub(Duration::from_secs_f64(stale_age)).unwrap();
    let mut rt = Runtime::new((), |_: &(), _: &mut Cx<()>| el(), (100.0, 100.0), Env::default());
    rt.wall_start = Some(wall_start);
    let stale_clock = {
        let _s = crate::input::time::scope(Some(stale_instant));
        rt.core().input_clock()
    };
    assert!(stale_clock < until, "stale macOS event clock {stale_clock} must be < {until}");

    // A fresh NSEvent stamped 5 ms after zoom finished maps to >= until.
    let fresh_instant = now + Duration::from_millis(5);
    let fresh_clock = {
        let _s = crate::input::time::scope(Some(fresh_instant));
        rt.core().input_clock()
    };
    assert!(fresh_clock >= until, "fresh macOS event clock {fresh_clock} must be >= {until}");
}
