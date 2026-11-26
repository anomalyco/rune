use std::cell::RefCell;
use std::collections::VecDeque;
use std::rc::Rc;
use std::time::Duration;

use super::*;
use crate::frame::clock::FrameClock;
use crate::{Cx, El, Env, Runtime, el};

struct TrackedDrawable {
    id: u32,
    log: Rc<RefCell<Vec<String>>>,
    consumed: bool,
}

impl Drop for TrackedDrawable {
    fn drop(&mut self) {
        if !self.consumed {
            self.log.borrow_mut().push(format!("drop_drawable:{}", self.id));
        }
    }
}

struct FakeHost {
    rt: Runtime<u32>,
    clock: Option<FrameClock>,
    log: Rc<RefCell<Vec<String>>>,
    gate: Gate,
    pre_reblend_needed: bool,
    pre_reblend_ok: bool,
    quiet_unchanged: bool,
    acquires: VecDeque<Result<(), Missed>>,
    next_drawable: u32,
    first_frame: bool,
    step_at: Instant,
    submit_at: Instant,
    presents_at: Option<Instant>,
    post_checks: VecDeque<PostStepCheck>,
    reacquire_ok: VecDeque<bool>,
    prepare_abort: bool,
    compositor_animating: bool,
    settled_info: Option<StepInfo>,
}

impl FakeHost {
    fn new(start: Instant) -> Self {
        fn view(w: &u32, _: &mut Cx<u32>) -> El {
            el().child(el().key("box").size(*w as f32 + 12.0, 8.0))
        }
        let mut rt = Runtime::new(0u32, view, (300.0, 40.0), Env::default());
        rt.set_system_reduced_motion(false);
        rt.wall_start = Some(start);
        Self {
            rt,
            clock: Some(FrameClock::new(start)),
            log: Rc::new(RefCell::new(Vec::new())),
            gate: Gate::Draw,
            pre_reblend_needed: false,
            pre_reblend_ok: true,
            quiet_unchanged: false,
            acquires: VecDeque::from([Ok(())]),
            next_drawable: 1,
            first_frame: false,
            step_at: start,
            submit_at: start,
            presents_at: None,
            post_checks: VecDeque::new(),
            reacquire_ok: VecDeque::new(),
            prepare_abort: false,
            compositor_animating: false,
            settled_info: None,
        }
    }

    fn record(&self, item: impl Into<String>) {
        self.log.borrow_mut().push(item.into());
    }

    fn events(&self) -> Vec<String> {
        self.log.borrow().clone()
    }
}

impl FrameHost for FakeHost {
    type Drawable = TrackedDrawable;
    type Prepared = u32;

    fn gate(&mut self) -> Gate {
        self.record(format!("gate:{:?}", self.gate.kind()));
        self.gate
    }

    fn before_acquire(&mut self) {
        self.record("before_acquire");
    }

    fn needs_reblend(&self) -> bool {
        self.record(format!("needs_reblend:{}", self.pre_reblend_needed));
        self.pre_reblend_needed
    }

    fn reblend(&mut self) -> bool {
        self.record(format!("reblend:{}", self.pre_reblend_ok));
        self.pre_reblend_ok
    }

    fn step_quiet(&mut self) -> StepInfo {
        self.record("step_quiet");
        let (core, mut app) = self.rt.split_mut();
        let (animating, dt) = self.clock.as_mut().unwrap().advance_core(core, &mut app, self.step_at, false);
        StepInfo { runtime_animating: animating, animating, dt, update_ms: 0.0, presents_at: self.presents_at }
    }

    fn is_unchanged(&mut self, _info: &StepInfo) -> bool {
        self.record(format!("is_unchanged:{}", self.quiet_unchanged));
        self.quiet_unchanged
    }

    fn on_unchanged(&mut self, _info: &StepInfo) {
        self.record("on_unchanged");
    }

    fn on_quiet_will_draw(&mut self) {
        self.record("on_quiet_will_draw");
    }

    fn acquire(&mut self) -> Acquire<Self::Drawable> {
        match self.acquires.pop_front().unwrap_or(Ok(())) {
            Ok(()) => {
                let id = self.next_drawable;
                self.next_drawable += 1;
                self.record(format!("acquire:ok:{id}"));
                Acquire::Ready(TrackedDrawable { id, log: self.log.clone(), consumed: false })
            }
            Err(missed) => {
                self.record(format!("acquire:missed:{missed:?}"));
                Acquire::Missed(missed)
            }
        }
    }

    fn step_runtime(&mut self) -> StepInfo {
        self.record("step_runtime");
        let (core, mut app) = self.rt.split_mut();
        let (animating, dt) = if self.first_frame {
            if let Some(clock) = self.clock.as_mut() {
                clock.hold(self.step_at);
            }
            (core.frame(&mut app, 0.0), 0.0)
        } else if let Some(clock) = self.clock.as_mut() {
            clock.advance_core(core, &mut app, self.step_at, self.compositor_animating)
        } else {
            (core.frame(&mut app, 1.0 / 60.0), 1.0 / 60.0)
        };
        StepInfo { runtime_animating: animating, animating, dt, update_ms: 0.5, presents_at: self.presents_at }
    }

    fn post_step_check(&mut self, _info: &mut StepInfo) -> PostStepCheck {
        let check = self.post_checks.pop_front().unwrap_or(PostStepCheck::Keep);
        self.record(format!("post_step_check:{check:?}"));
        check
    }

    fn prepare_reacquire(&mut self, reason: ReacquireReason, _info: &mut StepInfo) -> bool {
        let ok = self.reacquire_ok.pop_front().unwrap_or(true);
        self.record(format!("prepare_reacquire:{reason:?}:{ok}"));
        ok
    }

    fn on_missed(&mut self, missed: Missed, stepped: bool) {
        self.record(format!("on_missed:{missed:?}:stepped={stepped}"));
    }

    fn sync_after_step(&mut self, _info: &mut StepInfo) {
        self.record("sync_after_step");
    }

    fn prepare_render(&mut self, mut drawable: Self::Drawable, _info: &mut StepInfo) -> RenderPrep<Self::Prepared> {
        let id = drawable.id;
        drawable.consumed = true;
        if self.prepare_abort {
            self.record(format!("prepare_render:abort:{id}"));
            RenderPrep::Abort
        } else {
            self.record(format!("prepare_render:ok:{id}"));
            RenderPrep::Ready(id)
        }
    }

    fn render(&mut self, prepared: &mut Self::Prepared, _info: &StepInfo) -> bool {
        self.record(format!("render:{prepared}:comp={}", self.compositor_animating));
        self.compositor_animating
    }

    fn submit_and_present(&mut self, prepared: Self::Prepared, _info: &StepInfo) -> Instant {
        self.record(format!("submit_and_present:{prepared}"));
        self.submit_at
    }

    fn present_targets(&mut self) -> (Option<&mut FrameClock>, &mut RuntimeCore) {
        self.record("present_targets");
        (self.clock.as_mut(), self.rt.core_mut())
    }

    fn settle(&mut self, _submitted: Instant, info: &StepInfo) {
        self.settled_info = Some(*info);
        self.record(format!("settle:rt_anim={}:anim={}", info.runtime_animating, info.animating));
    }
}

#[test]
fn normal_frame_follows_canonical_order() {
    let start = crate::time::now();
    let mut host = FakeHost::new(start);
    assert_eq!(run(&mut host), Outcome::Presented);
    assert_eq!(
        host.events(),
        [
            "gate:Draw",
            "before_acquire",
            "needs_reblend:false",
            "acquire:ok:1",
            "step_runtime",
            "post_step_check:Keep",
            "sync_after_step",
            "prepare_render:ok:1",
            "render:1:comp=false",
            "submit_and_present:1",
            "present_targets",
            "settle:rt_anim=false:anim=false",
        ]
    );
}

#[test]
fn hidden_or_occluded_gate_skips_without_touching_surface() {
    let start = crate::time::now();
    let mut host = FakeHost::new(start);
    host.gate = Gate::Skip;
    assert_eq!(run(&mut host), Outcome::Skipped);
    assert_eq!(host.events(), ["gate:Skip"]);
}

#[test]
fn missed_drawable_before_step_reports_stepped_false() {
    let start = crate::time::now();
    let mut host = FakeHost::new(start);
    host.acquires = VecDeque::from([Err(Missed::Timeout)]);
    assert_eq!(run(&mut host), Outcome::Missed);
    assert_eq!(
        host.events(),
        [
            "gate:Draw",
            "before_acquire",
            "needs_reblend:false",
            "acquire:missed:Timeout",
            "on_missed:Timeout:stepped=false",
        ]
    );
}

#[test]
fn pre_reblend_failure_reports_unconfigured_with_stepped_false() {
    let start = crate::time::now();
    let mut host = FakeHost::new(start);
    host.pre_reblend_needed = true;
    host.pre_reblend_ok = false;
    assert_eq!(run(&mut host), Outcome::Missed);
    assert_eq!(
        host.events(),
        [
            "gate:Draw",
            "before_acquire",
            "needs_reblend:true",
            "reblend:false",
            "on_missed:Unconfigured:stepped=false",
        ]
    );
}

#[test]
fn resize_runs_before_reblend_and_acquire() {
    let start = crate::time::now();
    let mut host = FakeHost::new(start);
    host.pre_reblend_needed = true;
    assert_eq!(run(&mut host), Outcome::Presented);
    let events = host.events();
    let pos = |prefix: &str| events.iter().position(|e| e.starts_with(prefix)).unwrap();
    assert!(pos("before_acquire") < pos("reblend:"), "frame move must run before reblend: {events:?}");
    assert!(pos("reblend:") < pos("acquire:"), "reblend must run before acquire: {events:?}");
    assert!(pos("acquire:") < pos("step_runtime"), "acquire must run before step: {events:?}");
}

#[test]
fn quiet_timer_frame_unchanged_never_acquires_or_presents() {
    let start = crate::time::now();
    let mut host = FakeHost::new(start);
    host.gate = Gate::QuietTimer;
    host.quiet_unchanged = true;
    assert_eq!(run(&mut host), Outcome::Unchanged);
    assert_eq!(
        host.events(),
        [
            "gate:QuietTimer",
            "step_quiet",
            "is_unchanged:true",
            "on_unchanged",
        ]
    );
}

#[test]
fn quiet_timer_frame_changed_draws_without_stepping_twice() {
    let start = crate::time::now();
    let mut host = FakeHost::new(start);
    host.gate = Gate::QuietTimer;
    host.quiet_unchanged = false;
    assert_eq!(run(&mut host), Outcome::Presented);
    assert_eq!(
        host.events(),
        [
            "gate:QuietTimer",
            "step_quiet",
            "is_unchanged:false",
            "on_quiet_will_draw",
            "acquire:ok:1",
            "post_step_check:Keep",
            "sync_after_step",
            "prepare_render:ok:1",
            "render:1:comp=false",
            "submit_and_present:1",
            "present_targets",
            "settle:rt_anim=false:anim=false",
        ]
    );
}

#[test]
fn quiet_timer_frame_that_misses_acquire_reports_stepped_true() {
    let start = crate::time::now();
    let mut host = FakeHost::new(start);
    host.gate = Gate::QuietTimer;
    host.quiet_unchanged = false;
    host.acquires = VecDeque::from([Err(Missed::Occluded)]);
    assert_eq!(run(&mut host), Outcome::Missed);
    assert_eq!(
        host.events(),
        [
            "gate:QuietTimer",
            "step_quiet",
            "is_unchanged:false",
            "on_quiet_will_draw",
            "acquire:missed:Occluded",
            "on_missed:Occluded:stepped=true",
        ]
    );
}

#[test]
fn post_step_materials_and_reblend_drop_drawable_before_reacquiring() {
    let start = crate::time::now();
    let mut host = FakeHost::new(start);
    host.acquires = VecDeque::from([Ok(()), Ok(()), Ok(())]);
    host.post_checks = VecDeque::from([
        PostStepCheck::Reacquire(ReacquireReason::NativeMaterials),
        PostStepCheck::Reacquire(ReacquireReason::Reblend),
        PostStepCheck::Keep,
    ]);
    assert_eq!(run(&mut host), Outcome::Presented);
    assert_eq!(
        host.events(),
        [
            "gate:Draw",
            "before_acquire",
            "needs_reblend:false",
            "acquire:ok:1",
            "step_runtime",
            "post_step_check:Reacquire(NativeMaterials)",
            "drop_drawable:1",
            "prepare_reacquire:NativeMaterials:true",
            "acquire:ok:2",
            "post_step_check:Reacquire(Reblend)",
            "drop_drawable:2",
            "prepare_reacquire:Reblend:true",
            "acquire:ok:3",
            "post_step_check:Keep",
            "sync_after_step",
            "prepare_render:ok:3",
            "render:3:comp=false",
            "submit_and_present:3",
            "present_targets",
            "settle:rt_anim=false:anim=false",
        ]
    );
}

#[test]
fn post_step_reblend_failure_drops_drawable_and_misses_with_stepped_true() {
    let start = crate::time::now();
    let mut host = FakeHost::new(start);
    host.post_checks = VecDeque::from([PostStepCheck::Reacquire(ReacquireReason::Reblend)]);
    host.reacquire_ok = VecDeque::from([false]);
    assert_eq!(run(&mut host), Outcome::Missed);
    assert_eq!(
        host.events(),
        [
            "gate:Draw",
            "before_acquire",
            "needs_reblend:false",
            "acquire:ok:1",
            "step_runtime",
            "post_step_check:Reacquire(Reblend)",
            "drop_drawable:1",
            "prepare_reacquire:Reblend:false",
            "on_missed:Unconfigured:stepped=true",
        ]
    );
}

#[test]
fn first_frame_and_compositor_preserve_runtime_and_compositor_animating() {
    let start = crate::time::now();
    let mut host = FakeHost::new(start);
    host.first_frame = true;
    host.compositor_animating = true;
    assert_eq!(run(&mut host), Outcome::Presented);
    let info = host.settled_info.unwrap();
    assert!(!info.runtime_animating, "runtime itself is at rest on first frame");
    assert!(info.animating, "compositor animating must be OR'd before settle");
    assert_eq!(info.dt, 0.0, "first frame holds clock at dt 0");
}

#[test]
fn presented_records_clock_overrun_so_new_motion_does_not_jump() {
    let start = crate::time::now();
    let frame = Duration::from_micros(16_667);
    let overrun = Duration::from_millis(50);
    let mut host = FakeHost::new(start);

    // Frame 0: settle initial layout.
    host.step_at = start;
    host.submit_at = start;
    assert_eq!(run(&mut host), Outcome::Presented);

    // Frame 1: start a spring on an overrunning frame.
    host.rt.update(|w, _| *w = 200);
    let f1 = start + frame;
    host.step_at = f1;
    host.submit_at = f1 + overrun;
    assert_eq!(run(&mut host), Outcome::Presented);
    let w1 = host.rt.presented("box").unwrap().0[2];

    // Frame 2: one frame after presentation finished; overrun was delayed by host_step::run.
    let f2 = f1 + overrun + frame;
    host.step_at = f2;
    host.submit_at = f2;
    assert_eq!(run(&mut host), Outcome::Presented);
    let w2 = host.rt.presented("box").unwrap().0[2];

    // Compare with a reference without overrun delay: delayed frame moves much less than 66 ms.
    let mut plain_clock = FrameClock::new(start);
    let mut plain = Runtime::new(0u32, |w: &u32, _: &mut Cx<u32>| el().child(el().key("box").size(*w as f32 + 12.0, 8.0)), (300.0, 40.0), Env::default());
    plain.set_system_reduced_motion(false);
    plain.wall_start = Some(start);
    plain_clock.advance(&mut plain, start, false);
    plain.update(|w, _| *w = 200);
    plain_clock.advance(&mut plain, f1, false);
    plain_clock.advance(&mut plain, f2, false);
    let plain_w2 = plain.presented("box").unwrap().0[2];

    assert!(w2 > w1, "spring advanced on frame 2: w1={w1}, w2={w2}");
    assert!(plain_w2 - w2 > 5.0, "host_step::run must call clock.presented_core to delay overrun: w2={w2}, undelayed={plain_w2}");
}

#[test]
fn pre_stepped_path_reblends_acquires_renders_and_records_presented() {
    let start = crate::time::now();
    let mut host = FakeHost::new(start);
    host.clock = None;
    host.pre_reblend_needed = true;
    host.gate = Gate::PreStepped(StepInfo {
        runtime_animating: false,
        animating: false,
        dt: 1.0 / 60.0,
        update_ms: 0.4,
        presents_at: None,
    });
    assert_eq!(run(&mut host), Outcome::Presented);
    assert_eq!(
        host.events(),
        [
            "gate:PreStepped",
            "needs_reblend:true",
            "reblend:true",
            "acquire:ok:1",
            "post_step_check:Keep",
            "sync_after_step",
            "prepare_render:ok:1",
            "render:1:comp=false",
            "submit_and_present:1",
            "present_targets",
            "settle:rt_anim=false:anim=false",
        ]
    );
}

#[test]
fn prepare_abort_stops_before_render_or_present() {
    let start = crate::time::now();
    let mut host = FakeHost::new(start);
    host.prepare_abort = true;
    assert_eq!(run(&mut host), Outcome::Aborted);
    assert_eq!(
        host.events(),
        [
            "gate:Draw",
            "before_acquire",
            "needs_reblend:false",
            "acquire:ok:1",
            "step_runtime",
            "post_step_check:Keep",
            "sync_after_step",
            "prepare_render:abort:1",
        ]
    );
}

#[test]
fn post_step_forced_miss_drops_drawable_and_reports_stepped_true() {
    let start = crate::time::now();
    let mut host = FakeHost::new(start);
    host.post_checks = VecDeque::from([PostStepCheck::Missed(Missed::Timeout)]);
    assert_eq!(run(&mut host), Outcome::Missed);
    assert_eq!(
        host.events(),
        [
            "gate:Draw",
            "before_acquire",
            "needs_reblend:false",
            "acquire:ok:1",
            "step_runtime",
            "post_step_check:Missed(Timeout)",
            "drop_drawable:1",
            "on_missed:Timeout:stepped=true",
        ]
    );
}

#[test]
fn post_step_reacquire_miss_reports_stepped_true() {
    let start = crate::time::now();
    let mut host = FakeHost::new(start);
    host.acquires = VecDeque::from([Ok(()), Err(Missed::Outdated)]);
    host.post_checks = VecDeque::from([PostStepCheck::Reacquire(ReacquireReason::Reblend)]);
    assert_eq!(run(&mut host), Outcome::Missed);
    assert_eq!(
        host.events(),
        [
            "gate:Draw",
            "before_acquire",
            "needs_reblend:false",
            "acquire:ok:1",
            "step_runtime",
            "post_step_check:Reacquire(Reblend)",
            "drop_drawable:1",
            "prepare_reacquire:Reblend:true",
            "acquire:missed:Outdated",
            "on_missed:Outdated:stepped=true",
        ]
    );
}

#[test]
fn post_step_reacquire_is_capped_at_two_passes() {
    let mut phase = Phase::PostStepChecking { reacquired: 2 };
    let effects = step(&mut phase, Event::PostStepReacquire(ReacquireReason::Reblend));
    assert_eq!(phase, Phase::Syncing);
    assert_eq!(effects, Effects::one(Effect::Sync));
}

#[test]
fn post_step_hold_drops_drawable_and_skips_without_missing() {
    let mut host = FakeHost::new(crate::time::now());
    host.gate = Gate::Draw;
    host.acquires = VecDeque::from([Ok(())]);
    host.post_checks = VecDeque::from([PostStepCheck::Hold]);
    assert_eq!(run(&mut host), Outcome::Skipped);
    let log = host.log.borrow();
    assert!(log.iter().any(|s| s == "drop_drawable:1"));
    assert!(!log.iter().any(|s| s.starts_with("on_missed:")));
    assert!(!log.iter().any(|s| s == "submit_and_present"));
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
fn should_poll_after_encoder_only_on_live_vulkan() {
    assert!(should_poll_after_encoder(wgpu::Backend::Vulkan, false));
    assert!(!should_poll_after_encoder(wgpu::Backend::Vulkan, true));
    for backend in [
        wgpu::Backend::Dx12,
        wgpu::Backend::Metal,
        wgpu::Backend::Gl,
        wgpu::Backend::BrowserWebGpu,
        wgpu::Backend::Noop,
    ] {
        assert!(!should_poll_after_encoder(backend, false), "{backend:?} must skip post-encoder poll");
        assert!(!should_poll_after_encoder(backend, true), "lost {backend:?} must skip post-encoder poll");
    }
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
fn create_frame_encoder_polls_after_encoder_on_live_vulkan_and_skips_lost_or_other_backends() {
    let events = RefCell::new(Vec::new());
    let encoder = create_frame_encoder_with(
        wgpu::Backend::Vulkan,
        false,
        || {
            events.borrow_mut().push("create");
            7u32
        },
        || {
            events.borrow_mut().push("poll");
            Ok(wgpu::PollStatus::QueueEmpty)
        },
        || events.borrow_mut().push("mark_lost"),
    );
    assert_eq!(encoder, 7);
    assert_eq!(*events.borrow(), vec!["create", "poll"], "poll must run strictly after encoder creation");

    events.borrow_mut().clear();
    create_frame_encoder_with(
        wgpu::Backend::Vulkan,
        false,
        || events.borrow_mut().push("create"),
        || {
            events.borrow_mut().push("poll");
            Err(rune_gpu::health::PollFailed::Lost)
        },
        || events.borrow_mut().push("mark_lost"),
    );
    assert_eq!(*events.borrow(), vec!["create", "poll", "mark_lost"]);

    events.borrow_mut().clear();
    create_frame_encoder_with(
        wgpu::Backend::Vulkan,
        true,
        || events.borrow_mut().push("create"),
        || {
            events.borrow_mut().push("poll");
            Ok(wgpu::PollStatus::QueueEmpty)
        },
        || events.borrow_mut().push("mark_lost"),
    );
    assert_eq!(*events.borrow(), vec!["create"], "lost Vulkan device must skip poll");

    for backend in [wgpu::Backend::Dx12, wgpu::Backend::Metal, wgpu::Backend::Gl, wgpu::Backend::Noop] {
        events.borrow_mut().clear();
        create_frame_encoder_with(
            backend,
            false,
            || events.borrow_mut().push("create"),
            || {
                events.borrow_mut().push("poll");
                Ok(wgpu::PollStatus::QueueEmpty)
            },
            || events.borrow_mut().push("mark_lost"),
        );
        assert_eq!(*events.borrow(), vec!["create"], "{backend:?} must skip poll");
    }
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
fn create_frame_encoder_caches_backend_and_retires_finished_vulkan_work() {
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };

    let Some(gpu) = Gpu::test_headless_or_noop() else { return };
    assert_eq!(gpu.backend(), gpu.adapter.get_info().backend);

    let enc = create_frame_encoder(&gpu);
    gpu.queue.submit([enc.finish()]);

    if gpu.backend() == wgpu::Backend::Vulkan {
        let done = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&done);
        gpu.queue.on_submitted_work_done(move || flag.store(true, Ordering::Release));
        let deadline = crate::time::now() + Duration::from_secs(2);
        while !done.load(Ordering::Acquire) && crate::time::now() < deadline {
            let _enc = create_frame_encoder(&gpu);
            if !done.load(Ordering::Acquire) {
                std::thread::sleep(Duration::from_millis(2));
            }
        }
        assert!(done.load(Ordering::Acquire), "create_frame_encoder must retire finished Vulkan work via PollType::Poll");
    }

    gpu.mark_lost();
    let _lost_enc = create_frame_encoder(&gpu);
    assert!(gpu.is_lost());
}

