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
