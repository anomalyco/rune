use std::time::Duration;

use super::*;
use crate::frame::clock::{FrameClock, RESUME_STEP};
use crate::{Cx, El, Env, Runtime, el};

const FRAME_60HZ: Duration = Duration::from_micros(16_667);

#[test]
fn compositor_animating_keeps_real_frame_gap_instead_of_clamping_to_resume_step() {
    fn view(_: &(), _: &mut Cx<()>) -> El { el().size(40.0, 20.0) }
    let start = crate::time::now();
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
    let start = crate::time::now();
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
            assert!(!core.host.minimize.field(), "unsupported minimize request must be cleared");
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
    let start = crate::time::now();
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

#[derive(Default)]
struct EmbedTestState {
    polls: u32,
    switch_blend: bool,
    invalidate_on_poll: bool,
}

struct MockEmbedSurface {
    rt: Runtime<EmbedTestState>,
    clock: FrameClock,
    suspended: bool,
    renderer_srgb: bool,
    reblend_succeeds: bool,
    acquires: std::collections::VecDeque<Acquire<u32>>,
    comp_animating: bool,
    log: Vec<String>,
}

impl MockEmbedSurface {
    fn new(start: Instant) -> Self {
        fn view(_: &EmbedTestState, _: &mut Cx<EmbedTestState>) -> El { el().size(40.0, 20.0) }
        fn poll(s: &mut EmbedTestState, _: f64, tx: &mut crate::Tx) -> bool {
            s.polls += 1;
            if std::mem::take(&mut s.switch_blend) {
                tx.set_srgb_blend(false);
            }
            std::mem::take(&mut s.invalidate_on_poll)
        }
        let mut rt = Runtime::new(EmbedTestState::default(), view, (40.0, 20.0), Env::default());
        rt.poll = Some(poll);
        rt.wall_start = Some(start);
        rt.set_srgb_blend(true);
        Self {
            rt,
            clock: FrameClock::new(start),
            suspended: false,
            renderer_srgb: true,
            reblend_succeeds: true,
            acquires: std::collections::VecDeque::from([Acquire::Ready(1)]),
            comp_animating: false,
            log: Vec::new(),
        }
    }
}

impl EmbedSurface for MockEmbedSurface {
    type Drawable = u32;
    type Prepared = u32;

    fn suspended(&self) -> bool { self.suspended }
    fn renderer_srgb_blend(&self) -> bool { self.renderer_srgb }
    fn rt_srgb_blend(&self) -> bool { self.rt.core().srgb_blend() }

    fn reblend(&mut self) -> bool {
        self.log.push(format!("reblend:{}", self.reblend_succeeds));
        if self.reblend_succeeds {
            self.renderer_srgb = self.rt.core().srgb_blend();
            true
        } else {
            false
        }
    }

    fn acquire(&mut self) -> Acquire<u32> {
        let got = self.acquires.pop_front().unwrap_or(Acquire::Ready(99));
        self.log.push(match &got {
            Acquire::Ready(id) => format!("acquire:ok:{id}"),
            Acquire::Missed(m) => format!("acquire:missed:{m:?}"),
        });
        got
    }

    fn prepare(&mut self, drawable: u32) -> u32 {
        self.log.push(format!("prepare:{drawable}"));
        drawable
    }

    fn compositor_animating(&self) -> bool { self.comp_animating }

    fn render(&mut self, prepared: &mut u32, _dt: f64, _update_ms: f64) -> bool {
        self.log.push(format!("render:{prepared}"));
        self.comp_animating
    }

    fn submit_and_present(&mut self, prepared: u32) -> Instant {
        self.log.push(format!("present:{prepared}"));
        crate::time::now()
    }

    fn clock_and_rt_mut(&mut self) -> (&mut FrameClock, &mut RuntimeCore) {
        (&mut self.clock, self.rt.core_mut())
    }

    fn with_driver<R>(&mut self, f: impl FnOnce(&mut FrameClock, &mut RuntimeCore, &mut dyn AppDriver) -> R) -> R {
        self.log.push("step".into());
        let (rt, mut app) = self.rt.split_mut();
        f(&mut self.clock, rt, &mut app)
    }
}

#[test]
fn run_embed_frame_follows_shared_acquire_step_reblend_present_order() {
    let start = crate::time::now();
    let mut surface = MockEmbedSurface::new(start);
    surface.rt.state.switch_blend = true;
    surface.acquires = std::collections::VecDeque::from([Acquire::Ready(1), Acquire::Ready(2)]);
    let out = run_embed_frame(&mut surface);
    assert!(out.drew);
    assert_eq!(
        surface.log,
        ["acquire:ok:1", "step", "reblend:true", "acquire:ok:2", "prepare:2", "render:2", "present:2"]
    );
}

#[test]
fn run_embed_frame_suspended_touches_nothing() {
    let start = crate::time::now();
    let mut surface = MockEmbedSurface::new(start);
    surface.suspended = true;
    let out = run_embed_frame(&mut surface);
    assert_eq!(out, FrameSummary::default());
    assert!(surface.log.is_empty());
}

#[test]
fn run_embed_frame_post_step_reblend_miss_preserves_stale_and_live_pending_without_double_step() {
    let start = crate::time::now();
    let mut surface = MockEmbedSurface::new(start);
    surface.rt.state.switch_blend = true;
    surface.reblend_succeeds = false;
    surface.acquires = std::collections::VecDeque::from([Acquire::Ready(1)]);
    // Run a frame where reblend fails after step while a pending input arrives before miss.
    let out = run_embed_frame(&mut surface);
    assert!(!out.drew);
    assert_eq!(surface.rt.state.polls, 1, "post-step reblend failure must not step poll twice");

    // Verify missed_after_step reports animating when rt.stale() or rt.has_pending_input() is true.
    surface.rt.invalidate();
    let stale_miss = missed_after_step(surface.rt.core(), start, false, false);
    assert!(!stale_miss.drew && stale_miss.animating, "post-step miss must keep animating when rt.stale()");

    // Verify re-acquire Outdated after a post-step blend switch requests an immediate retry tick
    // without running poll a second time.
    let mut surface2 = MockEmbedSurface::new(start);
    surface2.rt.state.switch_blend = true;
    surface2.reblend_succeeds = true;
    surface2.acquires = std::collections::VecDeque::from([Acquire::Ready(1), Acquire::Missed(Missed::Outdated)]);
    let out2 = run_embed_frame(&mut surface2);
    assert!(!out2.drew);
    assert!(out2.animating, "Outdated re-acquire after blend switch keeps ticking");
    assert_eq!(surface2.rt.state.polls, 1, "post-step re-acquire miss must not step poll twice");
}

