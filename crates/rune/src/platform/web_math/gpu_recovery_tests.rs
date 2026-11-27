use std::time::Duration;

use super::*;
use crate::gpu::fallback::FORGIVE_LOSS;
use crate::{App, AppSpec, GpuFallback, GpuFallbackStage, el};

fn adapter_info(name: &str, device_type: wgpu::DeviceType, backend: wgpu::Backend) -> wgpu::AdapterInfo {
    wgpu::AdapterInfo {
        name: name.into(),
        vendor: 0,
        device: 0,
        device_type,
        device_pci_bus_id: String::new(),
        driver: String::new(),
        driver_info: String::new(),
        backend,
        subgroup_min_size: 0,
        subgroup_max_size: 0,
        transient_saves_memory: None,
        limit_bucket: None,
    }
}

fn step_one(state: &mut WebGpuRecovery, seq: &mut Seq, event: WebGpuRecoveryEvent) -> Option<WebGpuRecoveryEffect> {
    let mut fx: Vec<_> = state.step(seq, event).into_iter().collect();
    assert!(fx.len() <= 1, "expected at most one effect, got {fx:?}");
    fx.pop()
}

#[test]
fn software_adapter_rejection_applies_only_to_cpu_first_auto() {
    assert!(reject_software_adapters(WebStart::CpuFirst));
    assert!(!reject_software_adapters(WebStart::GpuFirst));
    assert!(!reject_software_adapters(WebStart::CpuOnly));
}

#[test]
fn software_and_fallback_adapters_are_detected_and_rejected_when_enabled() {
    let fallback_cpu = adapter_info("", wgpu::DeviceType::Cpu, wgpu::Backend::BrowserWebGpu);
    let swiftshader = adapter_info("Google SwiftShader", wgpu::DeviceType::Other, wgpu::Backend::BrowserWebGpu);
    let llvmpipe = adapter_info("llvmpipe (LLVM 19.1.0, 256 bits)", wgpu::DeviceType::Other, wgpu::Backend::BrowserWebGpu);
    let lavapipe = adapter_info("Mesa Lavapipe", wgpu::DeviceType::Other, wgpu::Backend::BrowserWebGpu);
    let warp = adapter_info("Microsoft Basic Render Driver", wgpu::DeviceType::Other, wgpu::Backend::BrowserWebGpu);
    let gl_cpu = adapter_info("ANGLE (Google, Vulkan 1.3.0 (SwiftShader Device))", wgpu::DeviceType::Cpu, wgpu::Backend::Gl);
    let hardware = adapter_info("Apple M3 Pro", wgpu::DeviceType::Other, wgpu::Backend::BrowserWebGpu);
    let opaque_hw = adapter_info("", wgpu::DeviceType::Other, wgpu::Backend::BrowserWebGpu);

    for sw in [&fallback_cpu, &swiftshader, &llvmpipe, &lavapipe, &warp, &gl_cpu] {
        assert!(is_software_adapter(sw), "expected software adapter for {sw:?}");
        assert!(check_adapter(sw, true).is_err(), "CpuFirst must reject {sw:?}");
        assert!(check_adapter(sw, false).is_ok(), "GpuFirst must accept {sw:?}");
    }
    for hw in [&hardware, &opaque_hw] {
        assert!(!is_software_adapter(hw), "hardware adapter misclassified: {hw:?}");
        assert!(check_adapter(hw, true).is_ok());
        assert!(check_adapter(hw, false).is_ok());
    }
}

#[test]
fn webgpu_recovery_retries_twice_with_backoff_then_gives_up_on_third_loss() {
    let mut seq = Seq::default();
    let mut state = WebGpuRecovery::default();
    let t0 = crate::time::now();

    // Loss 1 -> backoff 100 ms -> acquire -> succeeds.
    let Some(WebGpuRecoveryEffect::ScheduleRetry { attempt: a1, delay: d1 }) =
        step_one(&mut state, &mut seq, WebGpuRecoveryEvent::DeviceLost { now: t0, reason: "reset 1".into() })
    else {
        panic!("expected ScheduleRetry on loss 1");
    };
    assert_eq!(d1, WEBGPU_RECOVERY_BACKOFF);

    // Duplicate loss while already waiting in backoff is ignored.
    assert!(step_one(&mut state, &mut seq, WebGpuRecoveryEvent::DeviceLost { now: t0, reason: "dup".into() }).is_none());

    // Stale RetryDue is ignored.
    let stale = seq.next();
    assert!(step_one(&mut state, &mut seq, WebGpuRecoveryEvent::RetryDue { attempt: stale }).is_none());

    // Matching RetryDue starts async acquisition.
    let Some(WebGpuRecoveryEffect::Acquire { attempt: p1 }) =
        step_one(&mut state, &mut seq, WebGpuRecoveryEvent::RetryDue { attempt: a1 })
    else {
        panic!("expected Acquire after RetryDue");
    };
    assert!(state.is_acquiring(p1));
    assert!(!state.is_acquiring(stale));

    // Stale AcquireSucceeded is ignored; matching one returns to Idle preserving loss count.
    assert!(step_one(&mut state, &mut seq, WebGpuRecoveryEvent::AcquireSucceeded { attempt: stale }).is_none());
    assert!(state.is_acquiring(p1));
    assert!(step_one(&mut state, &mut seq, WebGpuRecoveryEvent::AcquireSucceeded { attempt: p1 }).is_none());
    assert!(!state.is_acquiring(p1));
    assert!(!state.is_exhausted());

    // Loss 2 within forgiveness window -> backoff 200 ms -> acquire -> succeeds.
    let t1 = t0 + Duration::from_secs(10);
    let Some(WebGpuRecoveryEffect::ScheduleRetry { attempt: a2, delay: d2 }) =
        step_one(&mut state, &mut seq, WebGpuRecoveryEvent::DeviceLost { now: t1, reason: "reset 2".into() })
    else {
        panic!("expected ScheduleRetry on loss 2");
    };
    assert_eq!(d2, WEBGPU_RECOVERY_BACKOFF * 2);
    let Some(WebGpuRecoveryEffect::Acquire { attempt: p2 }) =
        step_one(&mut state, &mut seq, WebGpuRecoveryEvent::RetryDue { attempt: a2 })
    else {
        panic!("expected Acquire on loss 2");
    };
    assert!(step_one(&mut state, &mut seq, WebGpuRecoveryEvent::AcquireSucceeded { attempt: p2 }).is_none());

    // Loss 3 within forgiveness window -> exhausted immediately.
    let t2 = t1 + Duration::from_secs(10);
    assert_eq!(
        step_one(&mut state, &mut seq, WebGpuRecoveryEvent::DeviceLost { now: t2, reason: "reset 3".into() }),
        Some(WebGpuRecoveryEffect::GiveUp { reason: "reset 3".into() }),
    );
    assert!(state.is_exhausted());

    // Further losses after exhaustion do nothing.
    assert!(step_one(&mut state, &mut seq, WebGpuRecoveryEvent::DeviceLost { now: t2 + Duration::from_secs(600), reason: "late".into() }).is_none());
}

#[test]
fn webgpu_recovery_forgives_losses_after_five_minutes() {
    let mut seq = Seq::default();
    let mut state = WebGpuRecovery::default();
    let t0 = crate::time::now();

    // Two losses -> count = 2.
    for i in 0..2u64 {
        let now = t0 + Duration::from_secs(i * 5);
        let Some(WebGpuRecoveryEffect::ScheduleRetry { attempt, .. }) =
            step_one(&mut state, &mut seq, WebGpuRecoveryEvent::DeviceLost { now, reason: format!("loss {i}") })
        else {
            panic!("expected retry");
        };
        let Some(WebGpuRecoveryEffect::Acquire { attempt: probe }) =
            step_one(&mut state, &mut seq, WebGpuRecoveryEvent::RetryDue { attempt })
        else {
            panic!("expected acquire");
        };
        assert!(step_one(&mut state, &mut seq, WebGpuRecoveryEvent::AcquireSucceeded { attempt: probe }).is_none());
    }

    // After FORGIVE_LOSS (5 minutes), one loss is forgiven so a 3rd loss recovers instead of giving up.
    let later = t0 + Duration::from_secs(5) + FORGIVE_LOSS;
    assert!(matches!(
        step_one(&mut state, &mut seq, WebGpuRecoveryEvent::DeviceLost { now: later, reason: "after sleep".into() }),
        Some(WebGpuRecoveryEffect::ScheduleRetry { .. }),
    ));
    assert!(!state.is_exhausted());
}

#[test]
fn failed_acquire_consumes_retry_budget_and_cancel_stops_recovery() {
    let mut seq = Seq::default();
    let mut state = WebGpuRecovery::default();
    let t0 = crate::time::now();

    // Loss 1 -> RetryDue -> AcquireFailed (counts as loss 2) -> RetryDue -> AcquireFailed (loss 3 -> GiveUp).
    let Some(WebGpuRecoveryEffect::ScheduleRetry { attempt: a1, .. }) =
        step_one(&mut state, &mut seq, WebGpuRecoveryEvent::DeviceLost { now: t0, reason: "lost".into() })
    else {
        panic!("expected retry 1");
    };
    let Some(WebGpuRecoveryEffect::Acquire { attempt: p1 }) =
        step_one(&mut state, &mut seq, WebGpuRecoveryEvent::RetryDue { attempt: a1 })
    else {
        panic!("expected acquire 1");
    };

    let Some(WebGpuRecoveryEffect::ScheduleRetry { attempt: a2, delay: d2 }) = step_one(
        &mut state,
        &mut seq,
        WebGpuRecoveryEvent::AcquireFailed { attempt: p1, now: t0 + Duration::from_millis(120), reason: "no adapter".into() },
    ) else {
        panic!("expected retry 2 after first acquire failure");
    };
    assert_eq!(d2, WEBGPU_RECOVERY_BACKOFF * 2);

    let Some(WebGpuRecoveryEffect::Acquire { attempt: p2 }) =
        step_one(&mut state, &mut seq, WebGpuRecoveryEvent::RetryDue { attempt: a2 })
    else {
        panic!("expected acquire 2");
    };

    assert_eq!(
        step_one(
            &mut state,
            &mut seq,
            WebGpuRecoveryEvent::AcquireFailed { attempt: p2, now: t0 + Duration::from_millis(350), reason: "still unavailable".into() },
        ),
        Some(WebGpuRecoveryEffect::GiveUp { reason: "still unavailable".into() }),
    );
    assert!(state.is_exhausted());

    // Cancel transitions any active state to Exhausted and ignores subsequent events.
    let mut active = WebGpuRecovery::default();
    let Some(WebGpuRecoveryEffect::ScheduleRetry { attempt: a, .. }) =
        step_one(&mut active, &mut seq, WebGpuRecoveryEvent::DeviceLost { now: t0, reason: "lost".into() })
    else {
        panic!("expected retry");
    };
    assert!(step_one(&mut active, &mut seq, WebGpuRecoveryEvent::Cancel).is_none());
    assert!(active.is_exhausted());
    assert!(step_one(&mut active, &mut seq, WebGpuRecoveryEvent::RetryDue { attempt: a }).is_none());
}

#[test]
fn exhausted_webgpu_loss_is_fatal_only_for_gpu_first_embedded_or_document() {
    assert!(!webgpu_exhausted_is_fatal(false, false, false));
    assert!(webgpu_exhausted_is_fatal(true, false, false));
    assert!(webgpu_exhausted_is_fatal(false, true, false));
    assert!(webgpu_exhausted_is_fatal(false, false, true));
    assert!(webgpu_exhausted_is_fatal(true, true, true));
}

#[test]
fn gpu_fallback_types_and_app_builder_expose_stage_and_reason() {
    use std::cell::RefCell;
    use std::rc::Rc;

    assert_eq!(GpuFallbackStage::Init.as_str(), "init");
    assert_eq!(GpuFallbackStage::Shader.as_str(), "shader");
    assert_eq!(GpuFallbackStage::HandoverTimeout.as_str(), "handover_timeout");
    assert_eq!(GpuFallbackStage::DeviceLost.as_str(), "device_lost");

    let seen = Rc::new(RefCell::new(Vec::new()));
    let seen_cb = seen.clone();
    let mut app = App::new(AppSpec::new("Web", (320.0, 240.0), || (), |_, _| el()))
        .on_gpu_fallback(move |fb: &GpuFallback| {
            seen_cb.borrow_mut().push((fb.stage(), fb.reason().to_owned()));
        });
    let mut hook = app.on_gpu_fallback.take().expect("on_gpu_fallback registered");
    let fb = GpuFallback::new(GpuFallbackStage::Init, "software/fallback GPU adapter rejected");
    assert_eq!(fb.stage(), GpuFallbackStage::Init);
    assert_eq!(fb.reason(), "software/fallback GPU adapter rejected");
    hook(&fb);
    assert_eq!(
        *seen.borrow(),
        vec![(GpuFallbackStage::Init, "software/fallback GPU adapter rejected".into())],
    );
}
