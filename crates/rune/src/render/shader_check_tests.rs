use super::*;
use crate::render::{Gpu, OFFSCREEN_FORMAT, Renderer};

#[test]
fn classify_scope_error_separates_oom_from_shader_rejection() {
    let oom = wgpu::Error::OutOfMemory { source: Box::new(std::io::Error::other("oom")) };
    let internal = wgpu::Error::Internal {
        source: Box::new(std::io::Error::other("compiler")),
        description: "compiler failed".into(),
    };
    let validation = wgpu::Error::Validation {
        source: Box::new(std::io::Error::other("bad wgsl")),
        description: "bad wgsl".into(),
    };

    assert_eq!(classify_scope_error(None, None, None), None);

    let err = classify_scope_error(Some(oom), Some(internal), None).expect("oom captured");
    assert_eq!(err.kind, ShaderErrorKind::OutOfMemory);
    assert_eq!(rune_gpu::fallback::Failure::from(err.kind), rune_gpu::fallback::Failure::OutOfMemory);

    let internal2 = wgpu::Error::Internal {
        source: Box::new(std::io::Error::other("compiler")),
        description: "compiler failed".into(),
    };
    let err = classify_scope_error(None, Some(internal2), Some(validation)).expect("internal captured");
    assert_eq!(err.kind, ShaderErrorKind::Rejected);
    assert_eq!(rune_gpu::fallback::Failure::from(err.kind), rune_gpu::fallback::Failure::ShaderRejected);

    let validation2 = wgpu::Error::Validation {
        source: Box::new(std::io::Error::other("bad wgsl")),
        description: "bad wgsl".into(),
    };
    let err = classify_scope_error(None, None, Some(validation2)).expect("validation captured");
    assert_eq!(err.kind, ShaderErrorKind::Rejected);
    assert_eq!(rune_gpu::fallback::Failure::from(err.kind), rune_gpu::fallback::Failure::ShaderRejected);
}

#[test]
fn uber_check_state_machine_handles_async_ok_failure_and_stale_answers() {
    let mut seq = Seq::default();
    let mut check = UberCheck::default();
    assert!(!check.checking());
    assert_eq!(check.failed_reason(), None);

    // Async browser check succeeds.
    let answer = Answer::default();
    assert!(check.step(&mut seq, UberCheckEvent::Started(answer.clone())).into_iter().next().is_none());
    assert!(check.checking());
    assert!(check.answered().is_none());
    *answer.lock().unwrap() = Some(None);
    let event = check.answered().expect("answer ready");
    assert!(check.step(&mut seq, event).into_iter().next().is_none());
    assert!(!check.checking());
    assert_eq!(check.failed_reason(), None);

    // Superseding check ignores stale answers from the earlier attempt.
    let stale_answer = Answer::default();
    check.step(&mut seq, UberCheckEvent::Started(stale_answer.clone()));
    *stale_answer.lock().unwrap() = Some(Some("stale failure".into()));
    let stale_event = check.answered().expect("stale event");

    let live_answer = Answer::default();
    check.step(&mut seq, UberCheckEvent::Started(live_answer.clone()));
    assert!(check.step(&mut seq, stale_event).into_iter().next().is_none());
    assert!(check.checking());

    // Live check rejects the shader -> emits Fail once and sticks in Failed.
    *live_answer.lock().unwrap() = Some(Some("GPUValidationError: invalid shader".into()));
    let fail_event = check.answered().expect("fail event");
    let effects: Vec<_> = check.step(&mut seq, fail_event).into_iter().collect();
    assert_eq!(effects, vec![UberCheckEffect::Fail("GPUValidationError: invalid shader".into())]);
    assert!(!check.checking());
    assert_eq!(check.failed_reason(), Some("GPUValidationError: invalid shader"));

    // Subsequent events cannot clear a failed state.
    let later = Answer::default();
    assert!(check.step(&mut seq, UberCheckEvent::Started(later)).into_iter().next().is_none());
    assert_eq!(check.failed_reason(), Some("GPUValidationError: invalid shader"));
}

#[test]
fn rejected_classic_shader_returns_error_without_uncaptured_gpu_error() {
    let Some(gpu) = Gpu::test_headless() else { return };
    let scope = gpu.device.push_error_scope(wgpu::ErrorFilter::Validation);
    let res = with_classic_shader_refused(true, || Renderer::try_new_classic(&gpu.device, OFFSCREEN_FORMAT));
    let leaked = pollster::block_on(scope.pop());
    assert!(leaked.is_none(), "classic shader rejection must not leak outside its error scope: {leaked:?}");
    let err = res.err().expect("refused classic shader must return Err");
    assert!(err.contains("rejected Rune's shader"), "unexpected error message: {err}");
}

#[test]
fn rejected_uber_shaders_return_classified_error_from_try_new() {
    let Some(gpu) = Gpu::test_headless() else { return };
    let scope = gpu.device.push_error_scope(wgpu::ErrorFilter::Validation);
    let res = with_uber_shaders_refused(true, || Renderer::try_new_classified(&gpu.device, OFFSCREEN_FORMAT));
    let leaked = pollster::block_on(scope.pop());
    assert!(leaked.is_none(), "shader rejection must not leak outside its error scope: {leaked:?}");
    let err = res.err().expect("refused uber shaders must return Err");
    assert_eq!(err.kind, ShaderErrorKind::Rejected);
}

#[test]
fn try_set_format_rejects_unrenderable_format_without_poisoning_renderer() {
    let Some(gpu) = Gpu::test_headless_wgpu() else { return };
    let mut renderer = Renderer::try_new(&gpu.device, OFFSCREEN_FORMAT).expect("valid renderer");
    let scope = gpu.device.push_error_scope(wgpu::ErrorFilter::Validation);
    // Rgba8Snorm never allows RENDER_ATTACHMENT, so pipeline creation fails validation.
    let err = renderer
        .try_set_format(&gpu.device, wgpu::TextureFormat::Rgba8Snorm)
        .expect_err("unrenderable format must fail");
    let leaked = pollster::block_on(scope.pop());
    assert!(leaked.is_none(), "format rejection must not leak: {leaked:?}");
    assert_eq!(err.kind, ShaderErrorKind::Rejected);
    assert_eq!(renderer.format(), OFFSCREEN_FORMAT, "failed format switch must leave the renderer unchanged");
}
