use std::time::Duration;
use super::*;

fn step_one(
    state: &mut GpuStartup,
    seq: &mut crate::machine::Seq,
    event: GpuStartupEvent,
) -> Option<GpuStartupEffect> {
    let mut effects = state.step(seq, event).into_iter();
    let first = effects.next();
    assert!(effects.next().is_none(), "expected at most one effect");
    first
}

#[test]
fn webgpu_probe_failure_waits_for_quiet_before_starting_webgl2() {
    let t0 = crate::time::now();
    let mut seq = crate::machine::Seq::default();
    let mut state = GpuStartup::default();

    // navigator.gpu + createRenderPipelineAsync exist: probe WebGPU on the next turn.
    let Some(GpuStartupEffect::ScheduleCheck { attempt: a1, delay }) = step_one(
        &mut state,
        &mut seq,
        GpuStartupEvent::FirstPresented {
            at: t0,
            has_navigator_gpu: true,
            compiles_async: true,
            webgl_enabled: true,
        },
    ) else {
        panic!("expected ScheduleCheck");
    };
    assert_eq!(delay, Duration::ZERO);

    assert_eq!(
        step_one(&mut state, &mut seq, GpuStartupEvent::CheckDue { attempt: a1, now: t0 + Duration::from_millis(2) }),
        Some(GpuStartupEffect::AskWebGpu),
    );

    // WebGPU returns no adapter 5 ms after the first CPU frame: WebGL2 must NOT start yet.
    let Some(GpuStartupEffect::ScheduleCheck { attempt: a2, delay }) = step_one(
        &mut state,
        &mut seq,
        GpuStartupEvent::WebGpuFailed { now: t0 + Duration::from_millis(5) },
    ) else {
        panic!("WebGPU failure before QUIET must defer WebGL2 with ScheduleCheck");
    };
    assert_eq!(delay, Duration::from_millis(245));

    // A non-ambient CPU frame (e.g. on_ready update or input) at 100 ms pushes quiet out to 350 ms.
    assert_eq!(
        step_one(&mut state, &mut seq, GpuStartupEvent::FrameDrawn { at: t0 + Duration::from_millis(100) }),
        None,
    );
    let Some(GpuStartupEffect::ScheduleCheck { attempt: a3, delay }) = step_one(
        &mut state,
        &mut seq,
        GpuStartupEvent::CheckDue { attempt: a2, now: t0 + Duration::from_millis(250) },
    ) else {
        panic!("expected reschedule while page is still active");
    };
    assert_eq!(delay, Duration::from_millis(100));

    // Stale attempt is ignored.
    assert_eq!(
        step_one(&mut state, &mut seq, GpuStartupEvent::CheckDue { attempt: a2, now: t0 + Duration::from_millis(350) }),
        None,
    );

    // Once QUIET elapses after the last non-ambient frame, WebGL2 starts.
    assert_eq!(
        step_one(&mut state, &mut seq, GpuStartupEvent::CheckDue { attempt: a3, now: t0 + Duration::from_millis(350) }),
        Some(GpuStartupEffect::AskWebGl2),
    );
    assert_eq!(state, GpuStartup::Done);
}

#[test]
fn webgpu_probe_failure_after_quiet_still_yields_a_macrotask_before_webgl2() {
    let t0 = crate::time::now();
    let mut seq = crate::machine::Seq::default();
    let mut state = GpuStartup::default();

    let Some(GpuStartupEffect::ScheduleCheck { attempt: a1, .. }) = step_one(
        &mut state,
        &mut seq,
        GpuStartupEvent::FirstPresented {
            at: t0,
            has_navigator_gpu: true,
            compiles_async: true,
            webgl_enabled: true,
        },
    ) else {
        panic!("expected ScheduleCheck");
    };
    assert_eq!(
        step_one(&mut state, &mut seq, GpuStartupEvent::CheckDue { attempt: a1, now: t0 + Duration::from_millis(1) }),
        Some(GpuStartupEffect::AskWebGpu),
    );

    // Even when WebGPU fails after QUIET has already passed, WebGL2 schedules on the next
    // macrotask (Duration::ZERO) instead of running inside requestAdapter's Promise microtask.
    let Some(GpuStartupEffect::ScheduleCheck { attempt: a2, delay }) = step_one(
        &mut state,
        &mut seq,
        GpuStartupEvent::WebGpuFailed { now: t0 + Duration::from_millis(300) },
    ) else {
        panic!("expected macrotask yield before WebGL2");
    };
    assert_eq!(delay, Duration::ZERO);

    // If a non-ambient frame arrives before that timer runs, WebGL2 defers for QUIET again.
    assert_eq!(
        step_one(&mut state, &mut seq, GpuStartupEvent::FrameDrawn { at: t0 + Duration::from_millis(305) }),
        None,
    );
    let Some(GpuStartupEffect::ScheduleCheck { attempt: a3, delay }) = step_one(
        &mut state,
        &mut seq,
        GpuStartupEvent::CheckDue { attempt: a2, now: t0 + Duration::from_millis(310) },
    ) else {
        panic!("expected quiet deferral after late draw");
    };
    assert_eq!(delay, Duration::from_millis(245));

    assert_eq!(
        step_one(&mut state, &mut seq, GpuStartupEvent::CheckDue { attempt: a3, now: t0 + Duration::from_millis(555) }),
        Some(GpuStartupEffect::AskWebGl2),
    );
}

#[test]
fn webgpu_probe_tracks_frames_drawn_while_probing_and_caps_wait_at_quiet_limit() {
    let t0 = crate::time::now();
    let mut seq = crate::machine::Seq::default();
    let mut state = GpuStartup::default();

    let Some(GpuStartupEffect::ScheduleCheck { attempt: a1, .. }) = step_one(
        &mut state,
        &mut seq,
        GpuStartupEvent::FirstPresented {
            at: t0,
            has_navigator_gpu: true,
            compiles_async: true,
            webgl_enabled: true,
        },
    ) else {
        panic!("expected ScheduleCheck");
    };
    assert_eq!(
        step_one(&mut state, &mut seq, GpuStartupEvent::CheckDue { attempt: a1, now: t0 + Duration::from_millis(1) }),
        Some(GpuStartupEffect::AskWebGpu),
    );

    // A frame drawn while requestAdapter is in flight updates drawn_at.
    assert_eq!(
        step_one(&mut state, &mut seq, GpuStartupEvent::FrameDrawn { at: t0 + Duration::from_millis(240) }),
        None,
    );
    let Some(GpuStartupEffect::ScheduleCheck { attempt: a2, delay }) = step_one(
        &mut state,
        &mut seq,
        GpuStartupEvent::WebGpuFailed { now: t0 + Duration::from_millis(260) },
    ) else {
        panic!("expected quiet wait from in-flight draw");
    };
    assert_eq!(delay, Duration::from_millis(230));

    // Continuous activity eventually hits GPU_START_QUIET_LIMIT (3 s) and caps the remaining wait.
    assert_eq!(
        step_one(&mut state, &mut seq, GpuStartupEvent::FrameDrawn { at: t0 + Duration::from_millis(2900) }),
        None,
    );
    let Some(GpuStartupEffect::ScheduleCheck { attempt: a3, delay }) = step_one(
        &mut state,
        &mut seq,
        GpuStartupEvent::CheckDue { attempt: a2, now: t0 + Duration::from_millis(2950) },
    ) else {
        panic!("expected wait capped by QUIET_LIMIT");
    };
    assert_eq!(delay, Duration::from_millis(50));

    assert_eq!(
        step_one(&mut state, &mut seq, GpuStartupEvent::CheckDue { attempt: a3, now: t0 + GPU_START_QUIET_LIMIT }),
        Some(GpuStartupEffect::AskWebGl2),
    );
}

#[test]
fn webgpu_success_and_no_webgl_paths_finish_cleanly() {
    let t0 = crate::time::now();
    let mut seq = crate::machine::Seq::default();

    // WebGPU succeeds: moves to Done and never asks for WebGL2.
    let mut state = GpuStartup::default();
    let Some(GpuStartupEffect::ScheduleCheck { attempt: a1, delay }) = step_one(
        &mut state,
        &mut seq,
        GpuStartupEvent::FirstPresented {
            at: t0,
            has_navigator_gpu: true,
            compiles_async: true,
            webgl_enabled: true,
        },
    ) else {
        panic!("expected ScheduleCheck");
    };
    assert_eq!(delay, Duration::ZERO);
    assert_eq!(
        step_one(&mut state, &mut seq, GpuStartupEvent::CheckDue { attempt: a1, now: t0 }),
        Some(GpuStartupEffect::AskWebGpu),
    );
    assert_eq!(step_one(&mut state, &mut seq, GpuStartupEvent::WebGpuSucceeded), None);
    assert_eq!(state, GpuStartup::Done);

    // WebGPU fails with webgl disabled -> FailNoBackend.
    let mut no_gl = GpuStartup::default();
    let Some(GpuStartupEffect::ScheduleCheck { attempt: a2, .. }) = step_one(
        &mut no_gl,
        &mut seq,
        GpuStartupEvent::FirstPresented {
            at: t0,
            has_navigator_gpu: true,
            compiles_async: true,
            webgl_enabled: false,
        },
    ) else {
        panic!("expected ScheduleCheck");
    };
    assert_eq!(
        step_one(&mut no_gl, &mut seq, GpuStartupEvent::CheckDue { attempt: a2, now: t0 }),
        Some(GpuStartupEffect::AskWebGpu),
    );
    assert_eq!(
        step_one(&mut no_gl, &mut seq, GpuStartupEvent::WebGpuFailed { now: t0 + Duration::from_millis(5) }),
        Some(GpuStartupEffect::FailNoBackend),
    );
    assert_eq!(no_gl, GpuStartup::Done);

    // No navigator.gpu and webgl disabled -> FailNoBackend on the next turn.
    let mut none = GpuStartup::default();
    let Some(GpuStartupEffect::ScheduleCheck { attempt: a3, delay }) = step_one(
        &mut none,
        &mut seq,
        GpuStartupEvent::FirstPresented {
            at: t0,
            has_navigator_gpu: false,
            compiles_async: false,
            webgl_enabled: false,
        },
    ) else {
        panic!("expected ScheduleCheck");
    };
    assert_eq!(delay, Duration::ZERO);
    assert_eq!(
        step_one(&mut none, &mut seq, GpuStartupEvent::CheckDue { attempt: a3, now: t0 }),
        Some(GpuStartupEffect::FailNoBackend),
    );
    assert_eq!(none, GpuStartup::Done);
}

#[test]
fn no_navigator_gpu_or_sync_webgpu_waits_for_quiet() {
    let t0 = crate::time::now();
    let mut seq = crate::machine::Seq::default();

    // No navigator.gpu, WebGL2 enabled: waits QUIET then starts WebGL2.
    let mut gl_only = GpuStartup::default();
    let Some(GpuStartupEffect::ScheduleCheck { attempt: a1, delay }) = step_one(
        &mut gl_only,
        &mut seq,
        GpuStartupEvent::FirstPresented {
            at: t0,
            has_navigator_gpu: false,
            compiles_async: false,
            webgl_enabled: true,
        },
    ) else {
        panic!("expected ScheduleCheck");
    };
    assert_eq!(delay, GPU_START_QUIET);
    assert_eq!(
        step_one(&mut gl_only, &mut seq, GpuStartupEvent::CheckDue { attempt: a1, now: t0 + GPU_START_QUIET }),
        Some(GpuStartupEffect::AskWebGl2),
    );

    // navigator.gpu without createRenderPipelineAsync: waits QUIET before probing WebGPU.
    let mut sync_gpu = GpuStartup::default();
    let Some(GpuStartupEffect::ScheduleCheck { attempt: a2, delay }) = step_one(
        &mut sync_gpu,
        &mut seq,
        GpuStartupEvent::FirstPresented {
            at: t0,
            has_navigator_gpu: true,
            compiles_async: false,
            webgl_enabled: true,
        },
    ) else {
        panic!("expected ScheduleCheck");
    };
    assert_eq!(delay, GPU_START_QUIET);
    assert_eq!(
        step_one(&mut sync_gpu, &mut seq, GpuStartupEvent::CheckDue { attempt: a2, now: t0 + GPU_START_QUIET }),
        Some(GpuStartupEffect::AskWebGpu),
    );
}

#[test]
fn webgl2_yields_a_macrotask_before_ready_while_webgpu_does_not() {
    assert!(needs_ready_yield(wgpu::Backend::Gl));
    assert!(!needs_ready_yield(wgpu::Backend::BrowserWebGpu));
}

#[test]
fn web_adapter_options_leave_power_preference_unset() {
    let opts = adapter_options(None);
    assert_eq!(opts.power_preference, wgpu::PowerPreference::None);
    assert!(!opts.force_fallback_adapter);
    assert!(!opts.apply_limit_buckets);
    assert!(opts.compatible_surface.is_none());
}

#[test]
fn web_runtime_preserves_on_frame_drawn_hook_and_reads_presented_state() {
    use std::cell::RefCell;
    use std::rc::Rc;
    use std::sync::Arc;
    use crate::{App, AppSpec, Cx, El, Env, el};

    let seen = Rc::new(RefCell::new(Vec::new()));
    let sink = seen.clone();
    let view = |count: &u32, _: &mut Cx<u32>| -> El {
        el().key(format!("frame-{count}")).size(40.0, 20.0)
    };
    let app = App::new(AppSpec::new("web-hook", (120.0, 80.0), || 1u32, view))
        .on_frame_drawn(move |rt| {
            let has_key = rt.elements().iter().any(|e| e.key.as_deref() == Some(&format!("frame-{}", rt.state)));
            sink.borrow_mut().push((rt.state, has_key));
        });

    let (mut rt, _, _, mut on_frame_drawn, _) = start_web_runtime(
        app,
        (120.0, 80.0),
        Env::default(),
        Arc::new(|| {}),
        Arc::new(|| {}),
    );
    assert!(on_frame_drawn.is_some(), "start_web_runtime must preserve App::on_frame_drawn");

    rt.frame(0.0);
    if let Some(hook) = on_frame_drawn.as_mut() {
        hook(&rt);
    }
    rt.update(|s, _| *s = 2);
    rt.frame(1.0 / 60.0);
    if let Some(hook) = on_frame_drawn.as_mut() {
        hook(&rt);
    }
    assert_eq!(*seen.borrow(), vec![(1, true), (2, true)]);

    // Without a hook, start_web_runtime leaves on_frame_drawn as None.
    let plain = App::new(AppSpec::new("web-plain", (120.0, 80.0), || 0u32, view));
    let (_, _, _, plain_hook, _) = start_web_runtime(
        plain,
        (120.0, 80.0),
        Env::default(),
        Arc::new(|| {}),
        Arc::new(|| {}),
    );
    assert!(plain_hook.is_none());
}

#[test]
fn web_frame_host_invokes_on_frame_drawn_only_on_drawn_frames() {
    use std::cell::RefCell;
    use std::rc::Rc;
    use std::sync::Arc;
    use crate::frame::clock::FrameClock;
    use crate::frame::host_step::{self, Acquire, FrameHost, Gate, Outcome, RenderPrep, StepInfo};
    use crate::runtime::RuntimeCore;
    use crate::time::Instant;
    use crate::window::Missed;
    use crate::{App, AppSpec, Color, Cx, Env, Runtime, el};

    struct HookWebHost {
        rt: Runtime<u32>,
        on_frame_drawn: Option<crate::frame::encoded::OnFrameDrawn<u32>>,
        clock: FrameClock,
        suspended: bool,
        draw: WebDrawKind,
        cpu_needs_gpu: bool,
        abort_prepare: bool,
        acquire_status: WebAcquireStatus,
        drawn: bool,
        step_at: Instant,
        last_presented: Option<crate::window::Presented>,
        config: wgpu::SurfaceConfiguration,
    }

    impl FrameHost for HookWebHost {
        type Drawable = ();
        type Prepared = ();

        fn gate(&mut self) -> Gate {
            self.drawn = false;
            let quiet_candidate = self.draw == WebDrawKind::Gpu && self.last_presented.is_some();
            web_frame_gate(self.suspended, self.draw, quiet_candidate)
        }

        fn step_quiet(&mut self) -> StepInfo {
            self.step_runtime()
        }

        fn is_unchanged(&mut self, info: &StepInfo) -> bool {
            self.draw == WebDrawKind::Gpu
                && self.last_presented.as_ref().is_some_and(|p| {
                    p.unchanged_core(self.rt.core(), &self.config, info.animating, None)
                })
        }

        fn acquire(&mut self) -> Acquire<()> {
            match self.draw {
                WebDrawKind::Gpu => match web_acquire_action(self.acquire_status, 0) {
                    WebAcquireAction::UseTexture => Acquire::Ready(()),
                    WebAcquireAction::Miss(m) => Acquire::Missed(m),
                    WebAcquireAction::ReconfigureAndRetry => Acquire::Missed(Missed::Outdated),
                },
                WebDrawKind::Cpu | WebDrawKind::HandingOver => Acquire::Ready(()),
                WebDrawKind::ContextLost | WebDrawKind::TerminalFailed | WebDrawKind::Moving => Acquire::Missed(Missed::Unconfigured),
            }
        }

        fn step_runtime(&mut self) -> StepInfo {
            let (core, mut app) = self.rt.split_mut();
            let (animating, dt) = self.clock.advance_core(core, &mut app, self.step_at, false);
            StepInfo { runtime_animating: animating, animating, dt, update_ms: 0.0, presents_at: None }
        }

        fn prepare_render(&mut self, _: (), _: &mut StepInfo) -> RenderPrep<()> {
            if self.abort_prepare { RenderPrep::Abort } else { RenderPrep::Ready(()) }
        }

        fn render(&mut self, _: &mut (), _: &StepInfo) -> bool {
            if matches!(self.draw, WebDrawKind::Cpu | WebDrawKind::HandingOver) {
                self.drawn = !self.cpu_needs_gpu;
            }
            false
        }

        fn submit_and_present(&mut self, _: (), _: &StepInfo) -> Instant {
            if self.draw == WebDrawKind::Gpu {
                self.drawn = true;
            }
            self.step_at
        }

        fn present_targets(&mut self) -> (Option<&mut FrameClock>, &mut RuntimeCore) {
            (Some(&mut self.clock), self.rt.core_mut())
        }

        fn settle(&mut self, _submitted: Instant, info: &StepInfo) {
            if self.drawn && let Some(hook) = self.on_frame_drawn.as_mut() {
                hook(&self.rt);
            }
            if self.draw == WebDrawKind::Gpu {
                let rt = self.rt.core();
                if should_record_presented(self.drawn, info.animating, rt.stale(), rt.host.window_frame.is_some()) {
                    crate::window::Presented::record_core(&mut self.last_presented, rt, &self.config, false);
                } else {
                    self.last_presented = None;
                }
            } else {
                self.last_presented = None;
            }
        }
    }

    let seen = Rc::new(RefCell::new(Vec::new()));
    let sink = seen.clone();
    let app = App::new(AppSpec::new("web-frame-hook", (160.0, 40.0), || 10u32, |s: &u32, _: &mut Cx<u32>| {
        el().child(el().size(*s as f32 + 10.0, 20.0).fill(Color::WHITE))
    }))
    .on_frame_drawn(move |rt| {
        sink.borrow_mut().push((rt.state, rt.elements().len()));
    });

    let start = crate::time::now();
    let (mut rt, _, _, on_frame_drawn, _) = start_web_runtime(
        app,
        (160.0, 40.0),
        Env::default(),
        Arc::new(|| {}),
        Arc::new(|| {}),
    );
    rt.set_system_reduced_motion(false);
    rt.wall_start = Some(start);

    let mut host = HookWebHost {
        rt,
        on_frame_drawn,
        clock: FrameClock::new(start),
        suspended: false,
        draw: WebDrawKind::Cpu,
        cpu_needs_gpu: false,
        abort_prepare: false,
        acquire_status: WebAcquireStatus::Ready,
        drawn: false,
        step_at: start,
        last_presented: None,
        config: wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format: wgpu::TextureFormat::Bgra8Unorm,
            width: 160,
            height: 40,
            present_mode: wgpu::PresentMode::Fifo,
            desired_maximum_frame_latency: 2,
            alpha_mode: wgpu::CompositeAlphaMode::Opaque,
            view_formats: Vec::new(),
            color_space: Default::default(),
        },
    };

    // 1. First CPU frame paints -> hook runs once.
    assert_eq!(host_step::run(&mut host), Outcome::Presented);
    assert_eq!(seen.borrow().len(), 1);
    assert_eq!(seen.borrow()[0].0, 10);
    assert!(seen.borrow()[0].1 > 0);

    // 2. CPU frame that needs GPU leaves drawn=false -> hook does not run.
    host.cpu_needs_gpu = true;
    host.step_at += Duration::from_millis(16);
    assert_eq!(host_step::run(&mut host), Outcome::Presented);
    assert_eq!(seen.borrow().len(), 1);
    host.cpu_needs_gpu = false;

    // 3. First GPU frame draws -> hook runs once and records last_presented.
    host.draw = WebDrawKind::Gpu;
    host.step_at += Duration::from_millis(16);
    assert_eq!(host_step::run(&mut host), Outcome::Presented);
    assert_eq!(seen.borrow().len(), 2);

    // 4. Resting quiet timer frame with no visual change -> Outcome::Unchanged -> hook does not run.
    host.step_at += Duration::from_millis(16);
    assert_eq!(host_step::run(&mut host), Outcome::Unchanged);
    assert_eq!(seen.borrow().len(), 2);

    // 5. Skipped frames (suspended, ContextLost, Moving) -> hook does not run; HandingOver keeps
    // painting CPU frames (and fires the hook) instead of freezing.
    host.suspended = true;
    assert_eq!(host_step::run(&mut host), Outcome::Skipped);
    host.suspended = false;
    host.draw = WebDrawKind::ContextLost;
    assert_eq!(host_step::run(&mut host), Outcome::Skipped);
    host.draw = WebDrawKind::Moving;
    assert_eq!(host_step::run(&mut host), Outcome::Skipped);
    assert_eq!(seen.borrow().len(), 2);
    host.draw = WebDrawKind::HandingOver;
    host.step_at += Duration::from_millis(16);
    assert_eq!(host_step::run(&mut host), Outcome::Presented);
    assert_eq!(seen.borrow().len(), 3);

    // 6. Missed acquire (Timeout / Occluded) and Aborted prepare -> hook does not run.
    host.draw = WebDrawKind::Gpu;
    host.last_presented = None;
    host.acquire_status = WebAcquireStatus::Timeout;
    assert_eq!(host_step::run(&mut host), Outcome::Missed);
    host.acquire_status = WebAcquireStatus::Occluded;
    assert_eq!(host_step::run(&mut host), Outcome::Missed);
    host.acquire_status = WebAcquireStatus::Ready;
    host.abort_prepare = true;
    assert_eq!(host_step::run(&mut host), Outcome::Aborted);
    host.abort_prepare = false;
    assert_eq!(seen.borrow().len(), 3);

    // 7. State change on GPU draws a new frame -> hook runs once with the updated state.
    host.rt.update(|s, _| *s = 42);
    host.step_at += Duration::from_millis(16);
    assert_eq!(host_step::run(&mut host), Outcome::Presented);
    assert_eq!(seen.borrow().len(), 4);
    assert_eq!(seen.borrow()[3].0, 42);

    // 8. Unset hook runs frames without error.
    host.on_frame_drawn = None;
    host.rt.update(|s, _| *s = 99);
    host.step_at += Duration::from_millis(16);
    assert_eq!(host_step::run(&mut host), Outcome::Presented);
    assert_eq!(seen.borrow().len(), 4);
}

#[test]
fn start_web_runtime_lends_window_text_system_during_setup() {
    use std::cell::Cell;
    use std::sync::Arc;

    const LABEL: &str = "Web setup warm text";
    const STYLE: crate::TextStyle = crate::TextStyle::new(14.0, 400.0);

    #[derive(Default)]
    struct WebSetupState {
        setup_width: f32,
        view_width: Cell<f32>,
    }

    fn web_view(s: &WebSetupState, _: &mut crate::Cx<WebSetupState>) -> crate::El {
        s.view_width.set(crate::text::measure(LABEL, &[], STYLE, None).0);
        crate::col().size(320.0, 120.0).child(crate::text(LABEL).font(STYLE))
    }

    // Run on a clean thread so no ambient `with_families` override bypasses `lend`.
    std::thread::spawn(|| {
        let mut spec = crate::AppSpec::new("web-setup", (320.0, 120.0), WebSetupState::default, web_view);
        spec.setup = Some(|s, _| {
            s.setup_width = crate::text::measure(LABEL, &[], STYLE, None).0;
        });
        let app = crate::App::new(spec).font_smoothing(crate::FontSmoothing::Off);
        let (mut rt, _, _, _, _) = crate::platform::web_math::start_web_runtime(
            app,
            (320.0, 120.0),
            crate::Env::new(true, 1.0),
            Arc::new(|| {}),
            Arc::new(|| {}),
        );
        assert_eq!(rt.font_smoothing(), crate::FontSmoothing::Off);
        let shaped_after_setup = rt.text.shaped;
        assert!(shaped_after_setup >= 1, "web setup must shape directly on the window's TextSystem");
        assert!(rt.text.cached_layout_count() >= 1);
        rt.frame(0.0);
        assert_eq!(
            rt.text.shaped, shaped_after_setup,
            "first web frame must reuse the layout shaped during setup",
        );
        assert_eq!(rt.state.setup_width, rt.state.view_width.get());
    })
    .join()
    .unwrap();
}

#[test]
fn gpu_startup_needs_gpu_starts_immediately_from_idle_and_bypasses_quiet_wait() {
    let t0 = crate::time::now();

    // 1. Non-degradable NeedsGpu on frame 1 (Idle): probes WebGPU on the next turn (0 ms) and
    // skips quiet wait if WebGPU fails over to WebGL2.
    let mut seq = crate::machine::Seq::default();
    let mut state = GpuStartup::default();
    let Some(GpuStartupEffect::ScheduleCheck { attempt: a1, delay }) = step_one(
        &mut state,
        &mut seq,
        GpuStartupEvent::NeedsGpu { at: t0, has_navigator_gpu: true, webgl_enabled: true },
    ) else {
        panic!("expected immediate ScheduleCheck");
    };
    assert_eq!(delay, Duration::ZERO);
    assert_eq!(
        step_one(&mut state, &mut seq, GpuStartupEvent::CheckDue { attempt: a1, now: t0 + Duration::from_millis(1) }),
        Some(GpuStartupEffect::AskWebGpu),
    );
    let Some(GpuStartupEffect::ScheduleCheck { attempt: a2, delay: gl_delay }) = step_one(
        &mut state,
        &mut seq,
        GpuStartupEvent::WebGpuFailed { now: t0 + Duration::from_millis(5) },
    ) else {
        panic!("expected immediate macrotask WebGL2 escalation");
    };
    assert_eq!(gl_delay, Duration::ZERO);
    assert_eq!(
        step_one(&mut state, &mut seq, GpuStartupEvent::CheckDue { attempt: a2, now: t0 + Duration::from_millis(6) }),
        Some(GpuStartupEffect::AskWebGl2),
    );
    assert_eq!(state, GpuStartup::Done);

    // 2. Non-degradable NeedsGpu arriving while waiting in WaitQuietForWebGl2 cancels the quiet
    // timer and fires AskWebGl2 on the next turn.
    let mut seq = crate::machine::Seq::default();
    let mut state = GpuStartup::default();
    let Some(GpuStartupEffect::ScheduleCheck { attempt: quiet_attempt, delay }) = step_one(
        &mut state,
        &mut seq,
        GpuStartupEvent::FirstPresented {
            at: t0,
            has_navigator_gpu: false,
            compiles_async: false,
            webgl_enabled: true,
        },
    ) else {
        panic!("expected quiet ScheduleCheck");
    };
    assert_eq!(delay, GPU_START_QUIET);
    let Some(GpuStartupEffect::ScheduleCheck { attempt: urgent_attempt, delay: urgent_delay }) = step_one(
        &mut state,
        &mut seq,
        GpuStartupEvent::NeedsGpu { at: t0 + Duration::from_millis(20), has_navigator_gpu: false, webgl_enabled: true },
    ) else {
        panic!("expected urgent escalation");
    };
    assert_eq!(urgent_delay, Duration::ZERO);
    // Stale quiet timer is ignored.
    assert_eq!(
        step_one(&mut state, &mut seq, GpuStartupEvent::CheckDue { attempt: quiet_attempt, now: t0 + Duration::from_millis(300) }),
        None,
    );
    assert_eq!(
        step_one(&mut state, &mut seq, GpuStartupEvent::CheckDue { attempt: urgent_attempt, now: t0 + Duration::from_millis(21) }),
        Some(GpuStartupEffect::AskWebGl2),
    );
    assert_eq!(state, GpuStartup::Done);
}
