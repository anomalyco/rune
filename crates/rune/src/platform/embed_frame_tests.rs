use std::time::Duration;

use super::*;
use crate::frame::clock::{FrameClock, RESUME_STEP};
use crate::{Cx, El, Env, Runtime, el};

const FRAME_60HZ: Duration = Duration::from_micros(16_667);

#[test]
fn compositor_animating_keeps_real_frame_gap_instead_of_clamping_to_resume_step() {
    fn view(_: &(), _: &mut Cx<()>) -> El { el().size(40.0, 20.0) }
    let start = Instant::now();
    let mut clock = FrameClock::new(start);
    let mut rt = Runtime::new((), view, (40.0, 20.0), Env::default());
    rt.wall_start = Some(start);

    // First frame settles runtime (not animating).
    {
        let (core, mut app) = rt.split_mut();
        step_and_flush(&mut clock, core, &mut app, start, false);
    }
    assert!(!rt.animating);

    // Second frame 16.67 ms later while only a compositor animates must step the real 16.67 ms
    // gap, not clamp to RESUME_STEP (8.33 ms).
    let (_, dt, _) = {
        let (core, mut app) = rt.split_mut();
        step_and_flush(&mut clock, core, &mut app, start + FRAME_60HZ, true)
    };
    assert!(
        (dt - FRAME_60HZ.as_secs_f64()).abs() < 1e-5,
        "compositor animation must keep real 60 Hz dt (~16.67 ms), got {dt} (RESUME_STEP={RESUME_STEP})"
    );
}

#[test]
fn post_present_delays_overrunning_new_motion_and_reports_stale_and_requests_dropped() {
    #[derive(Default)]
    struct S { width: f32 }
    fn view(s: &S, _: &mut Cx<S>) -> El {
        el().child(el().key("box").size(s.width, 8.0))
    }
    let start = Instant::now();
    let overrun = Duration::from_millis(50);

    let run = |with_present: bool| {
        let mut clock = FrameClock::new(start);
        let mut rt = Runtime::new(S { width: 12.0 }, view, (300.0, 40.0), Env::default());
        rt.set_system_reduced_motion(false);
        rt.wall_start = Some(start);
        {
            let (core, mut app) = rt.split_mut();
            let (anim, _, _) = step_and_flush(&mut clock, core, &mut app, start, false);
            if with_present { post_present(&mut clock, core, start, start, anim, false); }
        }
        // Start a spring on frame 1, which overruns by 50 ms before presenting.
        rt.update(|s, tx| {
            s.width = 212.0;
            tx.close_window();
            tx.minimize();
        });
        let f1 = start + FRAME_60HZ;
        {
            let (core, mut app) = rt.split_mut();
            let (anim, _, _) = step_and_flush(&mut clock, core, &mut app, f1, false);
            assert!(core.shell_request.is_none(), "unsupported shell request must be cleared");
            assert!(!core.minimize.field(), "unsupported minimize request must be cleared");
            if with_present { post_present(&mut clock, core, f1 + overrun, f1, anim, false); }
        }
        // Next frame arrives 1 frame after presentation finished.
        let f2 = f1 + overrun + FRAME_60HZ;
        {
            let (core, mut app) = rt.split_mut();
            let (anim, _, _) = step_and_flush(&mut clock, core, &mut app, f2, false);
            if with_present { post_present(&mut clock, core, f2, f2, anim, false); }
        }
        rt.presented("box").unwrap().0[2]
    };

    let delayed = run(true);
    let undelayed = run(false);
    assert!(
        undelayed - delayed > 5.0,
        "post_present must delay newly started motion by the frame overrun: delayed={delayed}, undelayed={undelayed}"
    );
}

#[test]
fn missed_step_runs_poll_without_advancing_frame_clock() {
    #[derive(Default)]
    struct S { polls: u32 }
    fn view(_: &S, _: &mut Cx<S>) -> El { el() }
    let start = Instant::now();
    let mut clock = FrameClock::new(start);
    let mut rt = Runtime::new(S::default(), view, (40.0, 20.0), Env::default());
    rt.poll = Some(|s, _, _| { s.polls += 1; false });
    rt.wall_start = Some(start);

    let summary = {
        let (core, mut app) = rt.split_mut();
        missed_step(core, &mut app, start + FRAME_60HZ, false, true)
    };
    assert!(!summary.drew);
    assert!(summary.animating, "outdated/lost retry asks display link to keep ticking");
    assert_eq!(rt.state.polls, 1, "missed drawable still runs poll/timers");
    assert_eq!(clock.last(), start, "missed drawable must not advance FrameClock");

    // Invalidation marks summary.animating true via rt.stale().
    rt.invalidate();
    let stale_summary = post_present(&mut clock, rt.core_mut(), start, start, false, false);
    assert!(stale_summary.drew && stale_summary.animating, "stale runtime must request another frame");
}
