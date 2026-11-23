use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use super::{MissState, apply_missed_core};
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
            on_close: None,
            on_rest: None,
            on_quit: None,
            on_reopen: None,
            turn_scope: None,
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
        on_close: None,
        on_rest: None,
        on_quit: None,
        on_reopen: None,
        turn_scope: None,
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
