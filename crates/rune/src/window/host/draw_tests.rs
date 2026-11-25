use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use super::super::render::{finalize_present_retries, should_commit_rebind};
use super::{LostPresent, MissState, apply_lost_present_core, apply_missed_core};
use crate::app::Shared;
use crate::frame::clock::FrameClock;
use crate::frame::encoded::FrameReports;
use crate::frame::host_step::{
    self, Acquire, FrameHost, Gate, Outcome, PostStepCheck, ReacquireReason, RenderPrep, StepInfo,
};
use crate::runtime::{Env, Runtime, RuntimeCore};
use crate::time::Instant;
use crate::window::host::app::{TypedWindowApp, WindowApp};
use crate::window::surface::{Backoff, Missed, Presented};
use crate::{Cx, El, FrameOutcome, Tx, el};

#[derive(Default)]
struct State {
    polls: u32,
    switch_blend: bool,
    copy_on_poll: bool,
}

fn view(_: &State, _: &mut Cx<State>) -> El {
    el().size(40.0, 20.0)
}

fn poll(s: &mut State, _: f64, tx: &mut Tx) -> bool {
    s.polls += 1;
    if std::mem::take(&mut s.switch_blend) {
        tx.set_srgb_blend(false);
    }
    if std::mem::take(&mut s.copy_on_poll) {
        tx.copy("copied-before-reblend-miss");
    }
    true
}

fn surface_config() -> wgpu::SurfaceConfiguration {
    wgpu::SurfaceConfiguration {
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
        format: wgpu::TextureFormat::Bgra8UnormSrgb,
        width: 80,
        height: 40,
        present_mode: wgpu::PresentMode::AutoVsync,
        desired_maximum_frame_latency: 2,
        alpha_mode: wgpu::CompositeAlphaMode::Opaque,
        view_formats: vec![],
        color_space: Default::default(),
    }
}

/// Window-host-shaped adapter running through `host_step::run` and `apply_missed_core` without a
/// native `winit::Window`.
struct MockWindowFrame {
    app: TypedWindowApp<State>,
    clock: FrameClock,
    clipboard: crate::clipboard::Client,
    clipboard_memory: crate::clipboard::MemoryClipboard,
    reports: FrameReports,
    presented: Option<Presented>,
    backoff: Backoff,
    surface_ok: bool,
    renderer_srgb: bool,
    reblend_succeeds: bool,
    now: Instant,
}

impl MockWindowFrame {
    fn new(now: Instant, outcomes: Rc<RefCell<Vec<FrameOutcome>>>) -> Self {
        let mut rt = Runtime::new(State::default(), view, (40.0, 20.0), Env::default());
        rt.poll = Some(poll);
        rt.wall_start = Some(now);
        rt.set_srgb_blend(true);
        rt.frame(0.0);
        let config = surface_config();
        let mut presented = None;
        Presented::record(&mut presented, &rt, &config, false);
        let shared = Arc::new(Shared::default());
        let app = TypedWindowApp {
            rt,
            shared,
            comp: None,
            on_frame_drawn: None,
            on_close_request: None,
            on_quit_request: None,
            on_close: None,
            on_rest: None,
            on_quit: None,
            on_reopen: None,
            on_geometry: None,
            turn_scope: None,
            keep_layout: None,
            kept_layout: None,
        };
        let reports = FrameReports::new(
            Some(Box::new(move |f| outcomes.borrow_mut().push(f.outcome()))),
            Some("main"),
        );
        let clipboard_memory = crate::clipboard::memory();
        let mut clipboard = crate::clipboard::Client::default();
        clipboard.use_memory(&clipboard_memory);
        Self {
            app,
            clock: FrameClock::new(now),
            clipboard,
            clipboard_memory,
            reports,
            presented,
            backoff: Backoff::default(),
            surface_ok: true,
            renderer_srgb: true,
            reblend_succeeds: false,
            now,
        }
    }
}

impl FrameHost for MockWindowFrame {
    type Drawable = ();
    type Prepared = ();

    fn gate(&mut self) -> Gate { Gate::Draw }

    fn needs_reblend(&self) -> bool {
        self.renderer_srgb != self.app.rt().srgb_blend()
    }

    fn reblend(&mut self) -> bool {
        if self.reblend_succeeds {
            self.renderer_srgb = self.app.rt().srgb_blend();
            true
        } else {
            false
        }
    }

    fn acquire(&mut self) -> Acquire<()> { Acquire::Ready(()) }

    fn step_runtime(&mut self) -> StepInfo {
        let (core, mut driver) = self.app.rt.split_mut();
        let (animating, dt) = self.clock.advance_core(core, &mut driver, self.now, false);
        StepInfo { runtime_animating: animating, animating, dt, update_ms: 0.0, presents_at: None }
    }

    fn post_step_check(&mut self, _info: &mut StepInfo) -> PostStepCheck {
        if self.needs_reblend() {
            PostStepCheck::Reacquire(ReacquireReason::Reblend)
        } else {
            PostStepCheck::Keep
        }
    }

    fn prepare_reacquire(&mut self, reason: ReacquireReason, _info: &mut StepInfo) -> bool {
        match reason {
            ReacquireReason::Reblend => self.reblend(),
            #[cfg(any(test, target_os = "macos"))]
            ReacquireReason::NativeMaterials => true,
            #[cfg(not(target_arch = "wasm32"))]
            ReacquireReason::ContentResize => true,
        }
    }

    fn on_missed(&mut self, missed: Missed, stepped: bool) {
        let _ = apply_missed_core(
            MissState {
                app: &mut self.app,
                clipboard: &self.clipboard,
                reports: &mut self.reports,
                presented: &mut self.presented,
                backoff: &mut self.backoff,
                surface_ok: &mut self.surface_ok,
            },
            missed,
            false,
            stepped,
            self.now,
        );
    }

    fn prepare_render(&mut self, _drawable: (), _info: &mut StepInfo) -> RenderPrep<()> {
        RenderPrep::Ready(())
    }

    fn render(&mut self, _prepared: &mut (), _info: &StepInfo) -> bool { false }

    fn submit_and_present(&mut self, _prepared: (), _info: &StepInfo) -> Instant { self.now }

    fn present_targets(&mut self) -> (Option<&mut FrameClock>, &mut RuntimeCore) {
        (Some(&mut self.clock), self.app.rt_mut())
    }
}

#[test]
fn post_step_reblend_failure_runs_missed_frame_cleanup_without_double_stepping() {
    let now = crate::time::now();
    let outcomes = Rc::new(RefCell::new(Vec::new()));
    let mut host = MockWindowFrame::new(now, outcomes.clone());
    let polls_before = host.app.rt.state.polls;
    host.app.rt.state.switch_blend = true;
    host.app.rt.state.copy_on_poll = true;

    assert_eq!(host_step::run(&mut host), Outcome::Missed);
    assert!(!host.surface_ok, "failed reblend marks surface unconfigured for replacement");
    assert!(host.presented.is_none(), "failed reblend drops cached presented frame");
    assert!(host.backoff.deadline().is_some(), "failed reblend schedules retry backoff");
    assert_eq!(host.app.rt.state.polls, polls_before + 1, "post-step reblend failure must not step poll twice");
    assert!(host.app.rt.copy_request.is_none(), "clipboard request from the stepped frame must be flushed");
    assert_eq!(
        host.clipboard_memory.text().as_deref().map(String::as_str),
        Some("copied-before-reblend-miss"),
        "clipboard request from the stepped frame must reach the clipboard"
    );
    assert_eq!(*outcomes.borrow(), [FrameOutcome::Skipped], "failed reblend reports a skipped frame");
}

#[test]
fn pre_step_reblend_failure_runs_covered_wake_once_and_schedules_backoff() {
    let now = crate::time::now();
    let outcomes = Rc::new(RefCell::new(Vec::new()));
    let mut host = MockWindowFrame::new(now, outcomes.clone());
    let polls_before = host.app.rt.state.polls;
    host.app.rt.set_srgb_blend(false);
    host.app.rt.state.copy_on_poll = true;

    assert_eq!(host_step::run(&mut host), Outcome::Missed);
    assert!(!host.surface_ok, "pre-step reblend failure marks surface unconfigured");
    assert!(host.presented.is_none(), "pre-step reblend failure clears cached presented frame");
    assert!(host.backoff.deadline().is_some(), "pre-step reblend failure schedules retry backoff");
    assert_eq!(host.app.rt.state.polls, polls_before + 1, "pre-step miss runs covered_wake once");
    assert!(host.app.rt.copy_request.is_none(), "clipboard request from covered_wake must be flushed");
    assert_eq!(
        host.clipboard_memory.text().as_deref().map(String::as_str),
        Some("copied-before-reblend-miss"),
        "clipboard request from covered_wake must reach the clipboard"
    );
    assert_eq!(*outcomes.borrow(), [FrameOutcome::Skipped]);
}

#[test]
fn cursor_moved_skips_redraw_within_same_hovered_row_and_keeps_quiet_timer_and_loop_frames() {
    use crate::el::Cursor;
    use crate::window::host::events::cursor_moved_core;
    use crate::window::surface::{LoopFrame, TimerFrame};
    use crate::{Color, Phase, ScrollSpec, Spring, col};

    struct HoverState {
        live: crate::Live<[f32; 2]>,
        drag_moves: u32,
        pointer_moves: u32,
    }
    fn hover_view(s: &HoverState, _: &mut Cx<HoverState>) -> El {
        col().size(300.0, 260.0)
            .hover_motion_within(Spring::NONE, Spring::NONE)
            .motion(Spring::NONE)
            // 1000 virtual rows, 24 pt each (y = 0..120).
            .child(
                col().key("list").size(300.0, 120.0).scroll(ScrollSpec { stick_bottom: false })
                    .child(col().key("rows").w_full().growth(crate::GrowthSpec::default()).virtual_rows(
                        1000,
                        24.0,
                        |_: &HoverState, i| format!("r{i}").into(),
                        |_: &HoverState, _| el().size(300.0, 24.0).hover(Color::WHITE.alpha(0.08)),
                    )),
            )
            // Cursor-only hit target with no hover fill (y = 120..150).
            .child(el().key("cursor-only").size(300.0, 30.0).hit().cursor(Cursor::Pointer))
            // Element asking for pointer positions via on_pointer_move_local + live_only (y = 150..190).
            .child(
                el().key("live-box").size(300.0, 40.0)
                    .on_pointer_move_local(|s: &mut HoverState, p, tx| {
                        s.pointer_moves += 1;
                        s.live.set([p.x, p.y]);
                        tx.live_only();
                    })
                    .child(el().key("knob").size(8.0, 8.0).fill(Color::WHITE).live_offset(&s.live)),
            )
            // Held drag target (y = 190..230).
            .child(
                el().key("drag-box").size(300.0, 40.0)
                    .on_drag(|s: &mut HoverState, d, _| {
                        if d.phase == Phase::Move { s.drag_moves += 1; }
                    }),
            )
    }

    let now = crate::time::now();
    let live = crate::Live::new([0.0, 0.0]);
    let mut rt = Runtime::new(
        HoverState { live: live.clone(), drag_moves: 0, pointer_moves: 0 },
        hover_view,
        (300.0, 260.0),
        Env::new(true, 1.0),
    );
    rt.frame(0.0);
    let mut app = TypedWindowApp {
        rt,
        shared: Arc::new(Shared::default()),
        comp: None,
        on_frame_drawn: None,
        on_close_request: None,
        on_quit_request: None,
        on_close: None,
        on_rest: None,
        on_quit: None,
        on_reopen: None,
        on_geometry: None,
        turn_scope: None,
        keep_layout: None,
        kept_layout: None,
    };
    let mut timer = TimerFrame::default();
    let mut loops = LoopFrame::default();

    // Sweep 4 pt per move across 5 virtual rows of 24 pt (30 moves total -> 5 redraws).
    let mut redraws = 0u32;
    for step in 0..30 {
        let y = 2.0 + step as f32 * 4.0;
        timer.asked_quiet(now);
        let _ = loops.only_loops(); // reset to untouched
        let (redraw, drag_out) = cursor_moved_core(&mut app, &mut timer, &mut loops, 100.0, y);
        assert!(drag_out.is_none());
        if redraw {
            redraws += 1;
            assert!(!timer.take_quiet(), "a redraw-causing move cancels quiet timer");
            assert!(!loops.only_loops(), "a redraw-causing move touches loop_frame");
            app.rt.frame(0.016);
        } else {
            assert!(timer.take_quiet(), "an unchanged move leaves timer_frame quiet");
            assert!(loops.only_loops(), "an unchanged move leaves loop_frame untouched");
        }
    }
    assert_eq!(redraws, 5, "only moves entering a new hovered row must request a redraw");

    // Moving off the last hovered row onto a cursor-only element repaints once to clear the row hover,
    // then subsequent moves within the cursor-only element skip redraws while keeping Cursor::Pointer.
    let (redraw, _) = cursor_moved_core(&mut app, &mut timer, &mut loops, 50.0, 135.0);
    assert!(redraw, "leaving row 4 clears its hover fill");
    app.rt.frame(0.016);
    assert_eq!(app.rt().cursor(), Cursor::Pointer);
    let (redraw, _) = cursor_moved_core(&mut app, &mut timer, &mut loops, 80.0, 135.0);
    assert!(!redraw, "moving within cursor-only target needs no redraw");
    assert_eq!(app.rt().cursor(), Cursor::Pointer);
    let (redraw, _) = cursor_moved_core(&mut app, &mut timer, &mut loops, 80.0, 245.0);
    assert!(!redraw, "moving from cursor-only target to empty background changes only the OS cursor");
    assert_eq!(app.rt().cursor(), Cursor::Default);

    // An element with on_pointer_move_local + live_only still requests a redraw on every move with 0 commits.
    app.rt.check_live_only(false);
    let commits_before = app.rt.stats.commits;
    let mut live_redraws = 0u32;
    for step in 0..4 {
        let x = 10.0 + step as f32 * 15.0;
        let (redraw, _) = cursor_moved_core(&mut app, &mut timer, &mut loops, x, 170.0);
        assert!(redraw, "on_pointer_move element must request a redraw on every move");
        live_redraws += 1;
        app.rt.frame(0.016);
        assert_eq!(live.get()[0], x);
    }
    assert_eq!(live_redraws, 4);
    assert_eq!(app.rt.state.pointer_moves, 4);
    assert_eq!(app.rt.stats.commits, commits_before, "live_only pointer moves need no view commits");

    // A held drag still requests a redraw on every move.
    app.rt.pointer_move(20.0, 210.0);
    app.rt.pointer_down(20.0, 210.0);
    app.rt.frame(0.016);
    for step in 1..=3 {
        let (redraw, _) = cursor_moved_core(&mut app, &mut timer, &mut loops, 20.0 + step as f32 * 10.0, 210.0);
        assert!(redraw, "held drag must request a redraw on every move");
        app.rt.frame(0.016);
    }
    assert_eq!(app.rt.state.drag_moves, 3);
    app.rt.pointer_up(50.0, 210.0);
}

/// A device lost during `submit` / `present` (or a synchronous `Rebinding` wait) invalidates
/// the surface, schedules a non-zero backoff instead of spinning immediately, falls back from
/// hardware to software after `SURFACE_TRIES` losses within 5 minutes, and gives up after
/// `SURFACE_TRIES` software losses. A single healthy frame does not reset the loss count; losses
/// decay only via `FORGIVE_LOSS` (1 per 5 minutes).
#[test]
fn lost_device_during_present_keeps_retry_count_backs_off_and_gives_up_after_surface_tries() {
    use std::time::Duration;
    use crate::gpu::fallback::{FORGIVE_LOSS, GpuMode, Policy};
    use crate::render::Gpu;
    use crate::window::surface::win::{SURFACE_BACKOFF, SURFACE_TRIES, SurfaceRetries, SurfaceStep};

    let mut now = crate::time::now();
    let outcomes = Rc::new(RefCell::new(Vec::new()));
    let mut host = MockWindowFrame::new(now, outcomes.clone());
    let mut retries = SurfaceRetries::default();
    let mut idle = false;

    let make_lost_gpu = || -> Option<Gpu> {
        let gpu = Gpu::try_new(Gpu::instance(), None).ok()?;
        assert!(!gpu.is_lost());
        gpu.device.destroy();
        gpu.device.poll(wgpu::PollType::wait_indefinitely()).ok();
        assert!(gpu.is_lost(), "a destroyed device must report is_lost()");
        Some(gpu)
    };

    // 1. Steady-state loss during submit/present: keeps count (0), invalidates surface and
    // presented state, marks idle, reports skipped, and arms a non-zero backoff.
    let lost_gpu = make_lost_gpu();
    let is_lost = lost_gpu.as_ref().map_or(true, Gpu::is_lost);
    assert!(!finalize_present_retries(is_lost, &mut retries), "a lost present must return false");
    assert_eq!(
        apply_lost_present_core(
            &retries, Policy::Automatic, true,
            &mut host.surface_ok, &mut host.presented, &mut host.backoff, &mut host.reports, &mut idle, now,
        ),
        LostPresent::Retry,
    );
    assert!(!host.surface_ok, "a lost present marks the surface unconfigured");
    assert!(host.presented.is_none(), "a lost present clears cached presentation");
    assert!(idle, "a lost present stops immediate redraws until backoff lapses");
    assert_eq!(host.backoff.deadline(), Some(now + SURFACE_BACKOFF), "first loss backs off by SURFACE_BACKOFF");
    assert_eq!(*outcomes.borrow(), [FrameOutcome::Skipped]);
    drop(lost_gpu);

    // 2. Consecutive lost presents walk Hardware (2 retries -> switch to Software on 3rd) ->
    // Software (2 retries -> GiveUp on 3rd), with growing hardware backoff waits.
    let mut hw_waits = Vec::new();
    let mut switched = 0u8;
    let mut gave_up = false;
    for _ in 0..10 {
        let step = retries.attempt(now, Policy::Automatic, true);
        let SurfaceStep::Replace { mode, switched_to_software } = step else {
            panic!("expected Replace before GiveUp, got {step:?}");
        };
        if switched_to_software {
            assert_eq!(mode, GpuMode::Software);
            switched += 1;
        }
        host.surface_ok = true;
        idle = false;
        let gpu = make_lost_gpu();
        let is_lost = gpu.as_ref().map_or(true, Gpu::is_lost);
        assert!(!finalize_present_retries(is_lost, &mut retries));
        match apply_lost_present_core(
            &retries, Policy::Automatic, true,
            &mut host.surface_ok, &mut host.presented, &mut host.backoff, &mut host.reports, &mut idle, now,
        ) {
            LostPresent::Retry => {
                let at = host.backoff.deadline().expect("retry must set a backoff deadline");
                if mode == GpuMode::Hardware {
                    hw_waits.push(at - now);
                }
                now = at;
            }
            LostPresent::GiveUp => {
                assert_eq!(mode, GpuMode::Software, "GiveUp must happen only after exhausting Software");
                gave_up = true;
                drop(gpu);
                break;
            }
        }
        drop(gpu);
    }
    assert!(gave_up, "consecutive lost presents must terminate with GiveUp instead of spinning forever");
    assert_eq!(switched, 1, "must switch from Hardware to Software exactly once");
    assert_eq!(hw_waits.len() as u8, SURFACE_TRIES - 1);
    assert!(hw_waits.windows(2).all(|w| w[1] > w[0]), "backoffs must grow across consecutive losses: {hw_waits:?}");
    assert_eq!(retries.attempt(now, Policy::Automatic, true), SurfaceStep::GiveUp, "subsequent replace_lost_surface must give up");

    // 3. A single healthy frame does NOT reset losses; only FORGIVE_LOSS (5 min) forgives one.
    let t0 = crate::time::now();
    let mut recovering = SurfaceRetries::default();
    assert_eq!(
        recovering.attempt(t0, Policy::Automatic, true),
        SurfaceStep::Replace { mode: GpuMode::Hardware, switched_to_software: false },
    );
    assert_eq!(recovering.failures(t0), 1);
    assert!(finalize_present_retries(false, &mut recovering));
    assert_eq!(recovering.failures(t0 + Duration::from_secs(10)), 1, "a single healthy frame must not reset losses");
    assert_eq!(recovering.failures(t0 + FORGIVE_LOSS), 0, "one loss is forgiven after 5 minutes");
}

#[test]
fn rebinding_does_not_commit_when_poll_fails_or_device_is_lost() {
    let ok = Ok(wgpu::PollStatus::Poll);
    let lost = Err(rune_gpu::health::PollFailed::Lost);
    assert!(should_commit_rebind(&ok, false));
    assert!(!should_commit_rebind(&ok, true), "must not commit a rebinding swap chain on a lost device");
    assert!(!should_commit_rebind(&lost, false), "must not commit when the rebinding wait reports PollFailed::Lost");
    assert!(!should_commit_rebind(&lost, true));
}

#[test]
fn wheel_scroll_settles_into_coasting_with_zero_idle_draws_and_drops_stale_redraw_requested() {
    use std::time::Duration;
    use crate::frame::surface_stats::{Pacing, SurfaceMeter, read};
    use crate::machine::Effects;
    use crate::platform::display_link_machine::{DisplayLinkCore, LinkEffect, LinkState, Opportunity, pacing_of};
    use crate::window::surface::{Redraw, RedrawEvent, timer_deadline_core};
    use crate::{ScrollSpec, Spring, WheelEvent, col};

    fn list_view(_: &(), _: &mut Cx<()>) -> El {
        col().w_full().h_full().motion(Spring::NONE).child(
            col()
                .key("list")
                .w_full()
                .h(400.0)
                .scroll(ScrollSpec { stick_bottom: false })
                .children((0..80).map(|i| el().key(format!("r{i}")).w_full().h(40.0))),
        )
    }

    let t0 = crate::time::now();
    let period = Duration::from_nanos(1_000_000_000 / 120);
    let config = surface_config();
    let mut rt = Runtime::new((), list_view, (600.0, 400.0), Env::new(false, 2.0));
    rt.wall_start = Some(t0);
    rt.frame(0.0);

    let mut presented = None;
    let mut core = DisplayLinkCore::new(period);
    let mut redraw = Redraw::Idle;
    let mut meter = SurfaceMeter::new("main", Arc::default());
    let shared_stats = meter.shared();
    let mut draws = 0u32;
    let mut presents = 0u32;
    let mut hw_starts = 0u32;
    let mut hw_stops = 0u32;

    // Wheel event #1 arrives at rest and queues a RedrawRequested.
    let t1 = t0 + Duration::from_millis(20);
    rt.wheel_event(WheelEvent::new(0.0, -12.0).at(300.0, 200.0).precise(true));
    let (next, _) = redraw.step(RedrawEvent::Ask(t1));
    redraw = next;
    assert!(matches!(redraw, Redraw::Asked { .. }));

    // Paint consumes RedrawRequested, starts the link once, draws, presents, and settles idle -> Coasting.
    let (next, satisfied) = redraw.take_on_paint();
    redraw = next;
    assert!(!satisfied);
    assert_eq!(core.start(), Effects::one(LinkEffect::StartLink));
    hw_starts += 1;
    assert_eq!(core.claim(None, t1), Opportunity::Now);
    let moving = rt.frame(period.as_secs_f64());
    assert!(!moving, "precise wheel delta leaves nothing moving on the next frame (#775)");
    draws += 1;
    presents += 1;
    let submitted1 = t1 + Duration::from_micros(900);
    core.presented(submitted1);
    let idle = Presented::settle_core(&mut presented, rt.core(), &config, moving, None, true, false);
    assert!(idle, "Presented::settle_core marks a wheel frame idle");
    assert_eq!(core.coast(), Effects::one(LinkEffect::ArmCoast { ticks_left: 18 }));
    meter.set_pacing(pacing_of(Some(core.state()), None, false));
    assert_eq!(read(&shared_stats).pacing, Pacing::Idle, "SurfaceMeter reports Pacing::Idle while Coasting");
    assert!(!core.paced(), "paced() is false while Coasting so wake_deadline still fires timers");

    // A timer armed while Coasting must still be exposed by wake_deadline (not suppressed by paced()).
    rt.update_if(|_, tx| { tx.wake_after(Duration::from_millis(80)); false });
    rt.core_mut().schedule_pending_wakes();
    let timer_due = timer_deadline_core(rt.core()).filter(|_| !core.paced());
    assert!(timer_due.is_some(), "wake_deadline must fire timers normally during Coasting");

    // Simulate DisplayTick winning the race against a queued RedrawRequested from Wheel event #2:
    // 1. Wheel #2 arrives -> Redraw::Asked; link is bumped to Active and waits for the tick.
    let t2 = t1 + Duration::from_millis(3);
    rt.wheel_event(WheelEvent::new(0.0, -12.0).at(300.0, 200.0).precise(true));
    let (next, _) = redraw.step(RedrawEvent::Ask(t2));
    redraw = next;
    assert_eq!(core.start(), Effects::one(LinkEffect::ResumeActive), "Coasting -> Active skips CVDisplayLinkStart");
    assert_eq!(core.claim(None, t2), Opportunity::Wait);

    // 2. DisplayTick arrives BEFORE winit drains RedrawRequested: draws & presents wheel #2,
    //    marks Redraw::Satisfied, and settles back into Coasting.
    let t_tick = t1 + period;
    assert!(core.paced());
    assert_eq!(core.claim(Some(t_tick + period), t_tick), Opportunity::Tick(t_tick + period));
    let moving = rt.frame(period.as_secs_f64());
    draws += 1;
    presents += 1;
    let (next, _) = redraw.step(RedrawEvent::DrewOutsidePaint);
    redraw = next;
    assert_eq!(redraw, Redraw::Satisfied);
    core.presented(t_tick + Duration::from_micros(900));
    let idle = Presented::settle_core(&mut presented, rt.core(), &config, moving, None, true, false);
    assert!(idle);
    assert_eq!(core.coast(), Effects::one(LinkEffect::ArmCoast { ticks_left: 18 }));

    // 3. The stale RedrawRequested arrives: paint consumes Satisfied and skips redraw while resting
    //    and clean, so Coasting is NOT bumped back to Active and no duplicate frame draws.
    let (next, satisfied) = redraw.take_on_paint();
    redraw = next;
    assert!(satisfied, "stale RedrawRequested after DisplayTick must be recognized as satisfied");
    let skip_stale = satisfied
        && presented.is_some()
        && !rt.core().stale()
        && !rt.core().live_pending()
        && !rt.core().has_pending_input()
        && rt.core().host.window_frame.is_none();
    assert!(skip_stale, "stale RedrawRequested must be skipped without touching pacing");
    assert_eq!(core.state(), LinkState::Coasting { ticks_left: 18 });
    assert_eq!(draws, 2);
    assert_eq!(presents, 2);

    // 4. A system RedrawRequested while Idle is NOT skipped, and a new Ask clears Satisfied.
    let (next, satisfied) = redraw.take_on_paint();
    redraw = next;
    assert!(!satisfied, "a system RedrawRequested while Idle must draw normally");
    let (next, _) = redraw.step(RedrawEvent::Ask(t_tick));
    let (next, _) = next.step(RedrawEvent::DrewOutsidePaint);
    assert_eq!(next, Redraw::Satisfied);
    let (next, _) = next.step(RedrawEvent::Ask(t_tick + Duration::from_millis(1)));
    let (_, satisfied) = next.take_on_paint();
    assert!(!satisfied, "a new request_redraw after DisplayTick clears Satisfied and draws");

    // 5. All 18 coasting ticks fire with zero draws and zero presents; tick 18 sends WakeToStop
    //    and stop_if_expired stops the link with zero draws and zero presents.
    let mut expiry_wakes = 0u32;
    for _ in 0..18 {
        for effect in core.coast_tick() {
            if effect == LinkEffect::WakeToStop {
                expiry_wakes += 1;
                assert!(!core.paced(), "DisplayTick on expiry must not call redraw()");
                for stop in core.stop_if_expired(true) {
                    if stop == LinkEffect::StopLink { hw_stops += 1; }
                }
            }
        }
    }
    assert_eq!(expiry_wakes, 1);
    assert_eq!(hw_starts, 1);
    assert_eq!(hw_stops, 1);
    assert_eq!(core.state(), LinkState::Stopped);
    assert_eq!(draws, 2, "coasting and expiry must draw nothing");
    assert_eq!(presents, 2, "coasting and expiry must present nothing");
}

#[test]
fn window_app_forwards_release_compositor_gpu() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use crate::compose::{Compositor, Frame};
    use crate::window::host::app::{TypedWindowApp, WindowApp};

    struct DefaultComp;
    impl Compositor<()> for DefaultComp {
        fn render(&mut self, f: &mut Frame<'_, ()>) { f.draw_ui(); }
    }

    struct CountingComp(Arc<AtomicUsize>);
    impl Compositor<()> for CountingComp {
        fn render(&mut self, f: &mut Frame<'_, ()>) { f.draw_ui(); }
        fn release_gpu(&mut self) { self.0.fetch_add(1, Ordering::Relaxed); }
    }

    fn view(_: &(), _: &mut crate::Cx<()>) -> crate::El { crate::col().size(100.0, 100.0) }

    // Default body compiles and is a no-op.
    let mut default_comp = DefaultComp;
    default_comp.release_gpu();

    let releases = Arc::new(AtomicUsize::new(0));
    let mut app: Box<dyn WindowApp> = Box::new(TypedWindowApp {
        rt: crate::runtime::Runtime::new((), view, (100.0, 100.0), crate::Env::default()),
        shared: Arc::new(crate::app::Shared::default()),
        comp: Some(Box::new(CountingComp(releases.clone()))),
        on_frame_drawn: None,
        on_close_request: None,
        on_quit_request: None,
        on_close: None,
        on_rest: None,
        on_quit: None,
        on_reopen: None,
        on_geometry: None,
        turn_scope: None,
        keep_layout: None,
        kept_layout: None,
    });
    app.release_compositor_gpu();
    assert_eq!(releases.load(Ordering::Relaxed), 1);
}

#[test]
fn turn_end_guard_requests_redraw_when_user_input_is_queued_at_rest() {
    use crate::platform::display_link_machine::{DisplayLinkCore, LinkState};
    use crate::window::host::any_host::queued_input_needs_redraw;
    use crate::window::surface::{Redraw, RedrawEffect, RedrawEvent};

    let now = crate::time::now();
    // Pure state matrix: Idle and Satisfied request a redraw when input waits on an unpaced,
    // drawable window; already-pending redraws, active pacing, non-drawable windows, and
    // empty input queues do not.
    assert!(queued_input_needs_redraw(true, true, false, Redraw::Idle));
    assert!(queued_input_needs_redraw(true, true, false, Redraw::Satisfied));
    assert!(!queued_input_needs_redraw(true, true, false, Redraw::Asked { at: now }));
    assert!(!queued_input_needs_redraw(true, true, false, Redraw::Painting));
    assert!(!queued_input_needs_redraw(true, true, false, Redraw::PaintingAgain));
    assert!(!queued_input_needs_redraw(true, true, false, Redraw::Wants));
    assert!(!queued_input_needs_redraw(true, true, true, Redraw::Idle));
    assert!(!queued_input_needs_redraw(true, false, false, Redraw::Idle));
    assert!(!queued_input_needs_redraw(false, true, false, Redraw::Idle));

    // End-to-end invariant: a resting timer poll sends a key via Tx::send_key. Even if the
    // window reaches turn end with Redraw::Idle or Redraw::Satisfied and the display link
    // Stopped or Coasting, the turn-end guard requests a redraw until the frame delivers it.
    let mut rt = Runtime::new(false, |_: &bool, _: &mut Cx<bool>| {
        crate::col().child(crate::el().key("b").role("button").focusable(true).size(40.0, 20.0))
    }, (200.0, 100.0), Env::default());
    rt.set_tab_visits_controls(true);
    rt.poll = Some(|sent, _, tx| {
        if std::mem::take(sent) {
            tx.send_key(crate::KeyEvent { key: crate::Key::Tab, mods: crate::Mods::default() });
            true
        } else {
            false
        }
    });
    rt.wall_start = Some(now);
    rt.frame(0.0);
    rt.frame(crate::frame::clock::RESUME_STEP);
    assert!(rt.rests());

    rt.state = true;
    assert_eq!(rt.resting_wake(), Some(true));
    rt.resting_wake_frame(true);
    assert!(rt.core().has_input_waiting());

    let mut link = DisplayLinkCore::new(std::time::Duration::from_nanos(1_000_000_000 / 120));
    assert_eq!(link.state(), LinkState::Stopped);
    let mut redraw = Redraw::Satisfied;
    assert!(queued_input_needs_redraw(rt.core().has_input_waiting(), true, link.paced(), redraw));
    let (next, effect) = redraw.step(RedrawEvent::Ask(now));
    redraw = next;
    assert_eq!(effect, RedrawEffect::Request);
    assert!(!queued_input_needs_redraw(rt.core().has_input_waiting(), true, link.paced(), redraw));

    // Same while coasting (link not actively pacing).
    let _ = link.start();
    let _ = link.coast();
    assert!(!link.paced());
    assert!(queued_input_needs_redraw(rt.core().has_input_waiting(), true, link.paced(), Redraw::Idle));

    // Once the frame runs and delivers the key, the guard lets the window rest.
    rt.frame(crate::frame::clock::RESUME_STEP);
    assert!(!rt.core().has_input_waiting());
    assert!(!queued_input_needs_redraw(rt.core().has_input_waiting(), true, link.paced(), Redraw::Idle));
}

#[test]
fn cursor_moved_core_wakes_and_produces_drag_when_deferred_titlebar_press_crosses_threshold() {
    use crate::window::host::events::cursor_moved_core;
    use crate::window::surface::{LoopFrame, TimerFrame};
    use crate::{Cx, El, Env, Runtime, TitlebarAction, el};

    fn view(_: &(), _: &mut Cx<()>) -> El {
        el().key("bar").size(200.0, 40.0).window_drag()
    }
    let mut rt = Runtime::new((), view, (200.0, 100.0), Env::new(true, 1.0));
    rt.core_mut().set_defer_titlebar_drag(true);
    rt.frame(0.0);
    let mut app = TypedWindowApp {
        rt,
        shared: Arc::new(Shared::default()),
        comp: None,
        on_frame_drawn: None,
        on_close_request: None,
        on_quit_request: None,
        on_close: None,
        on_rest: None,
        on_quit: None,
        on_reopen: None,
        on_geometry: None,
        turn_scope: None,
        keep_layout: None,
        kept_layout: None,
    };
    let mut timer = TimerFrame::default();
    let mut loops = LoopFrame::default();

    app.rt.pointer_move(100.0, 20.0);
    app.rt.pointer_down(100.0, 20.0);
    assert_eq!(app.rt_mut().take_titlebar_action(), None);

    // Motion inside the 8 pt drag threshold stays quiet and emits no action.
    let (redraw, drag_out) = cursor_moved_core(&mut app, &mut timer, &mut loops, 105.0, 20.0);
    assert!(!redraw);
    assert!(drag_out.is_none());
    assert_eq!(app.rt_mut().take_titlebar_action(), None);

    // Motion reaching 8 pt emits TitlebarAction::Drag and wakes the host loop.
    let (redraw, drag_out) = cursor_moved_core(&mut app, &mut timer, &mut loops, 108.0, 20.0);
    assert!(redraw);
    assert!(drag_out.is_none());
    assert_eq!(app.rt_mut().take_titlebar_action(), Some(TitlebarAction::Drag));
    assert_eq!(app.rt_mut().take_titlebar_action(), None);
}
