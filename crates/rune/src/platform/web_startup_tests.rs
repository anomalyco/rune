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
