use super::*;
use crate::render::{Gpu, Instance, KIND_RECT, LayerSpec, NO_CLIP, OFFSCREEN_FORMAT, Offscreen};
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
fn draw_refusing(feature: Feature, instances: &[Instance]) -> (Renderer, [u8; 4]) {
    let gpu = Gpu::headless_wgpu();
    let mut renderer = Renderer::new(&gpu.device, OFFSCREEN_FORMAT);
    renderer.breakers.refuse = Some(feature);
    let target = Offscreen::new(&gpu.device, 32, 16);
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
    (renderer, [out[at], out[at + 1], out[at + 2], out[at + 3]])
}

/// A half-faded group whose layer pipeline the device refused draws its contents straight into
/// the frame, at full opacity, rather than leaving the frame blank.
#[test]
fn a_refused_layer_pipeline_draws_the_group_in_its_parent() {
    let (renderer, pixel) = draw_refusing(Feature::Isolation, &group(LayerSpec { opacity: 0.5, ..Default::default() }));
    assert_eq!(pixel, [255; 4]);
    assert_eq!(renderer.layer_stats().targets, 0);
    assert!(!renderer.extra.iter().any(|(f, _)| *f == FORMAT), "the refused pipeline isn't kept");
}

#[test]
fn a_refused_plus_lighter_pipeline_draws_source_over() {
    let (renderer, pixel) = draw_refusing(Feature::PlusLighter, &group(LayerSpec { opacity: 0.5, blend: Blend::PlusLighter, ..Default::default() }));
    let want = [188, 188, 188, 128];
    assert!(pixel.iter().zip(want).all(|(&a, b)| a.abs_diff(b) <= 1), "half white, isolated, source-over: {pixel:?}");
    assert!(renderer.layers.additive.is_none());
}

#[test]
fn a_refused_blur_pipeline_draws_unblurred() {
    let (_, pixel) = draw_refusing(Feature::LayerBlur, &group(LayerSpec { blur: 4.0, ..Default::default() }));
    assert_eq!(pixel, [255; 4]);
}
