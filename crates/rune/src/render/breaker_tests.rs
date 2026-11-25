use super::*;
use crate::render::{Gpu, Instance, KIND_RECT, LayerSpec, NO_CLIP, OFFSCREEN_FORMAT_LINEAR, Offscreen};
use bytemuck::Zeroable;

fn step(health: &mut Health, seq: &mut Seq, event: Event) -> Vec<Effect> {
    health.step(seq, event).into_iter().collect()
}

#[test]
fn a_build_the_device_takes_works_and_one_it_refuses_is_dropped_for_good() {
    let mut seq = Seq::default();
    let mut health = Health::Untried;
    assert_eq!(step(&mut health, &mut seq, Event::Wanted), [Effect::Build]);
    assert!(step(&mut health, &mut seq, Event::Built(Verdict::Now(None))).is_empty());
    assert!(health.working());
    assert!(step(&mut health, &mut seq, Event::Wanted).is_empty(), "built once");

    let mut health = Health::Untried;
    let effects = step(&mut health, &mut seq, Event::Built(Verdict::Now(Some("invalid".into()))));
    assert_eq!(effects, [Effect::Drop, Effect::Report("invalid".into())]);
    assert!(matches!(health, Health::Broken));
    assert!(step(&mut health, &mut seq, Event::Wanted).is_empty(), "never built again");
    // A new shader gets one more try.
    assert_eq!(step(&mut health, &mut seq, Event::ShaderChanged), [Effect::Drop]);
    assert_eq!(step(&mut health, &mut seq, Event::Wanted), [Effect::Build]);
}

/// The web's answer comes later; until it does the pipeline isn't used, and an answer to a check
/// the state no longer holds changes nothing.
#[test]
fn a_late_answer_settles_only_its_own_check() {
    let mut seq = Seq::default();
    let mut health = Health::Untried;
    let answer = Answer::default();
    assert!(step(&mut health, &mut seq, Event::Built(Verdict::Later(answer.clone()))).is_empty());
    assert!(!health.working());
    assert!(health.answered().is_none(), "no answer yet");
    *answer.lock().unwrap() = Some(Some("GPUInternalError: compile failed".into()));
    let Some(Event::Answered { attempt, error }) = health.answered() else { panic!("the answer") };

    assert_eq!(step(&mut health, &mut seq, Event::ShaderChanged), [Effect::Drop]);
    assert!(step(&mut health, &mut seq, Event::Answered { attempt, error: error.clone() }).is_empty(), "stale");
    assert!(matches!(health, Health::Untried));

    let answer = Answer::default();
    step(&mut health, &mut seq, Event::Built(Verdict::Later(answer.clone())));
    *answer.lock().unwrap() = Some(error);
    let answered = health.answered().expect("the answer");
    assert_eq!(step(&mut health, &mut seq, answered), [Effect::Drop, Effect::Report("GPUInternalError: compile failed".into())]);
    assert!(matches!(health, Health::Broken));
}

fn rect(bounds: [f32; 4]) -> Instance {
    Instance { rect: bounds, fill: [1.0; 4], clip: NO_CLIP, params: [0.0, KIND_RECT, 0.0, 1.0], ..Instance::zeroed() }
}

fn group(spec: LayerSpec) -> Vec<Instance> {
    let bounds = [4.0, 4.0, 24.0, 8.0];
    vec![Instance::begin_layer(bounds, NO_CLIP, spec), rect(bounds), Instance::end_layer()]
}

/// Draws three frames of `instances` with `feature`'s pipelines built broken, and checks that no
/// frame set an invalid pipeline; the pixel at the group's centre.
fn draw_refusing(feature: Feature, instances: &[Instance]) -> Option<(Renderer, [u8; 4])> {
    let gpu = Gpu::test_headless_wgpu()?;
    let mut renderer = Renderer::new(&gpu.device, OFFSCREEN_FORMAT_LINEAR);
    renderer.breakers.refuse = Some(feature);
    let target = Offscreen::with_format(&gpu.device, 32, 16, OFFSCREEN_FORMAT_LINEAR);
    let mut atlas = crate::text::TextSystem::default().atlas;
    let mut out = Vec::new();
    for _ in 0..3 {
        let scope = gpu.device.push_error_scope(wgpu::ErrorFilter::Validation);
        renderer.prepare(&gpu.device, &gpu.queue, &mut atlas, instances, (32, 16));
        let mut encoder = gpu.device.create_command_encoder(&Default::default());
        renderer.draw(&mut encoder, &target.view, [0.0; 4]);
        target.copy(&mut encoder);
        renderer.submit(&gpu.queue, encoder);
        out = target.read(&gpu.device);
        let error = pollster::block_on(scope.pop());
        assert!(error.is_none(), "a frame set a pipeline the device refused: {error:?}");
    }
    assert!(!matches!(renderer.breakers.health(feature), Health::Working | Health::Untried));
    let at = (8 * 32 + 16) * 4;
    Some((renderer, [out[at], out[at + 1], out[at + 2], out[at + 3]]))
}

/// A half-faded group whose layer pipeline the device refused draws its contents straight into
/// the frame, at full opacity, rather than leaving the frame blank.
#[test]
fn a_refused_layer_pipeline_draws_the_group_in_its_parent() {
    let Some((renderer, pixel)) = draw_refusing(Feature::Isolation, &group(LayerSpec { opacity: 0.5, ..Default::default() })) else { return };
    assert_eq!(pixel, [255; 4]);
    assert_eq!(renderer.layer_stats().targets, 0);
    assert!(!renderer.extra.iter().any(|(f, _)| *f == FORMAT), "the refused pipeline isn't kept");
}

#[test]
fn a_refused_plus_lighter_pipeline_draws_source_over() {
    let Some((renderer, pixel)) = draw_refusing(Feature::PlusLighter, &group(LayerSpec { opacity: 0.5, blend: Blend::PlusLighter, ..Default::default() })) else { return };
    let want = [188, 188, 188, 128];
    assert!(pixel.iter().zip(want).all(|(&a, b)| a.abs_diff(b) <= 1), "half white, isolated, source-over: {pixel:?}");
    assert!(renderer.layers.additive.is_none());
}

#[test]
fn a_refused_blur_pipeline_draws_unblurred() {
    let Some((_, pixel)) = draw_refusing(Feature::LayerBlur, &group(LayerSpec { blur: 4.0, ..Default::default() })) else { return };
    assert_eq!(pixel, [255; 4]);
}

#[test]
fn needed_layer_features_detects_visible_isolation_plus_lighter_and_blur_only() {
    use crate::render::layers::needed_layer_features;
    let bounds = [0.0, 0.0, 32.0, 32.0];
    assert_eq!(needed_layer_features(&[rect(bounds)]), (false, false, false));
    // Plain opaque unclipped layer does not isolate.
    assert_eq!(needed_layer_features(&group(LayerSpec::default())), (false, false, false));
    // Hidden (opacity 0) layers and their descendants do not request feature pipelines.
    let hidden_blur = vec![
        Instance::begin_layer(bounds, NO_CLIP, LayerSpec { opacity: 0.0, ..Default::default() }),
        Instance::begin_layer(bounds, NO_CLIP, LayerSpec { blur: 4.0, blend: Blend::PlusLighter, ..Default::default() }),
        rect(bounds),
        Instance::end_layer(),
        Instance::end_layer(),
    ];
    assert_eq!(needed_layer_features(&hidden_blur), (false, false, false));
    // Visible translucent, additive, and blurred layers request their exact features.
    assert_eq!(needed_layer_features(&group(LayerSpec { opacity: 0.5, ..Default::default() })), (true, false, false));
    assert_eq!(needed_layer_features(&group(LayerSpec { blend: Blend::PlusLighter, ..Default::default() })), (true, true, false));
    assert_eq!(needed_layer_features(&group(LayerSpec { blur: 4.0, ..Default::default() })), (true, false, true));
    // Clipped layer isolates.
    let clipped = vec![Instance::begin_layer(bounds, [0.0, 0.0, 16.0, 16.0], LayerSpec::default()), rect(bounds), Instance::end_layer()];
    assert_eq!(needed_layer_features(&clipped), (true, false, false));
}

#[test]
fn poll_breaker_checks_settles_async_answer_without_encoding_a_frame() {
    let Some(gpu) = Gpu::test_headless_wgpu() else { return };
    let mut renderer = Renderer::new(&gpu.device, OFFSCREEN_FORMAT_LINEAR);
    let answer = Answer::default();
    let _ = renderer.breakers.step(Feature::Isolation, Event::Built(Verdict::Later(answer.clone())));
    assert!(renderer.checking_gpu());
    assert!(!renderer.poll_breaker_checks(&gpu.device));
    assert!(renderer.checking_gpu());

    *answer.lock().unwrap() = Some(None);
    assert!(renderer.poll_breaker_checks(&gpu.device));
    assert!(!renderer.checking_gpu());
    assert!(renderer.breakers.health(Feature::Isolation).working());
    assert!(!renderer.poll_breaker_checks(&gpu.device));
}

#[test]
fn precheck_layer_features_builds_needed_feature_without_encoding_a_frame() {
    let Some(gpu) = Gpu::test_headless_wgpu() else { return };
    let mut renderer = Renderer::new(&gpu.device, OFFSCREEN_FORMAT_LINEAR);
    assert!(matches!(renderer.breakers.health(Feature::Isolation), Health::Untried));
    // Plain rects leave layer features untried.
    renderer.precheck_layer_features(&gpu.device, &[rect([0.0, 0.0, 16.0, 16.0])]);
    assert!(matches!(renderer.breakers.health(Feature::Isolation), Health::Untried));
    // A translucent layer prechecks Isolation without submitting a GPU frame.
    renderer.precheck_layer_features(&gpu.device, &group(LayerSpec { opacity: 0.5, ..Default::default() }));
    assert!(renderer.breakers.health(Feature::Isolation).working());
    assert!(matches!(renderer.breakers.health(Feature::LayerBlur), Health::Untried));
}
