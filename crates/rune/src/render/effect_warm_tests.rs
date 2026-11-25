use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex, mpsc};

use bytemuck::Zeroable;

use super::{Effect, Event, PrewarmEffects, State, pace, step};
use crate::render::{
    Blend, Gpu, Instance, KIND_BACKDROP_BLUR, KIND_EFFECT, KIND_RECT, LayerSpec, NO_CLIP,
    OFFSCREEN_FORMAT, Offscreen, Renderer, breaker, layers, variant,
};

fn rect(bounds: [f32; 4], fill: [f32; 4]) -> Instance {
    Instance { rect: bounds, fill, clip: NO_CLIP, params: [0.0, KIND_RECT, 0.0, 1.0], ..Instance::zeroed() }
}

fn group(bounds: [f32; 4], spec: LayerSpec, fill: [f32; 4]) -> Vec<Instance> {
    vec![Instance::begin_layer(bounds, NO_CLIP, spec), rect(bounds, fill), Instance::end_layer()]
}

fn backdrop_blur_instance(bounds: [f32; 4], sigma: f32) -> Instance {
    Instance {
        rect: bounds,
        fill: [0.0, 0.0, 0.0, 1.0],
        clip: NO_CLIP,
        params: [sigma, KIND_BACKDROP_BLUR, 0.0, 1.0],
        ..Instance::zeroed()
    }
}

#[cfg(feature = "test-gpu-counters")]
fn live_render_pipelines(device: &wgpu::Device) -> isize {
    device.get_internal_counters().hal.render_pipelines.read()
}

fn render_offscreen(
    gpu: &Gpu,
    renderer: &mut Renderer,
    target: &Offscreen,
    atlas: &mut crate::text::Atlas,
    instances: &[Instance],
) -> Vec<u8> {
    renderer.prepare(&gpu.device, &gpu.queue, atlas, instances, (target.width, target.height));
    let mut encoder = gpu.device.create_command_encoder(&Default::default());
    renderer.draw(&mut encoder, &target.view, [0.0; 4]);
    target.copy(&mut encoder);
    renderer.submit(&gpu.queue, encoder);
    target.read(&gpu.device)
}

#[test]
fn state_machine_walks_idle_warming_ready_consumed_and_handles_shader_swap_and_shutdown() {
    let mut state: State<u32, u32> = State::Idle;

    // Idle + FirstFrameFinished { has_work: false } -> Consumed without spawning.
    let fx: Vec<_> = step(&mut state, Event::FirstFrameFinished { has_work: false }).into_iter().collect();
    assert!(fx.is_empty());
    assert!(matches!(state, State::Consumed));

    // ShaderChanged resets Consumed -> Idle.
    let fx: Vec<_> = step(&mut state, Event::ShaderChanged).into_iter().collect();
    assert!(fx.is_empty());
    assert!(matches!(state, State::Idle));

    // Idle + FirstFrameFinished { has_work: true } -> SpawnWorker.
    let fx: Vec<_> = step(&mut state, Event::FirstFrameFinished { has_work: true }).into_iter().collect();
    assert_eq!(fx, [Effect::SpawnWorker]);
    assert!(matches!(state, State::Idle));

    // WorkerSpawned transitions Idle -> Warming.
    let cancel = Arc::new(AtomicBool::new(false));
    let (_tx, rx) = mpsc::channel();
    let fx: Vec<_> = step(&mut state, Event::WorkerSpawned { cancel: cancel.clone(), receiver: rx, thread: 1 }).into_iter().collect();
    assert!(fx.is_empty());
    assert!(matches!(state, State::Warming { .. }));

    // Repeat FirstFrameFinished in Warming is a no-op.
    assert!(step(&mut state, Event::FirstFrameFinished { has_work: true }).into_iter().next().is_none());

    // WorkerFinished(Some) transitions Warming -> Ready and drains handle.
    let fx: Vec<_> = step(&mut state, Event::WorkerFinished(Some(42))).into_iter().collect();
    assert_eq!(fx, [Effect::Drain(1)]);
    assert!(matches!(state, State::Ready(42)));

    // AllConsumed transitions Ready -> Consumed.
    let fx: Vec<_> = step(&mut state, Event::AllConsumed).into_iter().collect();
    assert!(fx.is_empty());
    assert!(matches!(state, State::Consumed));

    // ShaderChanged while Warming cancels the worker, drains it, and returns to Idle.
    let _ = step(&mut state, Event::ShaderChanged);
    let cancel2 = Arc::new(AtomicBool::new(false));
    let (_tx2, rx2) = mpsc::channel();
    let _ = step(&mut state, Event::WorkerSpawned { cancel: cancel2.clone(), receiver: rx2, thread: 2 });
    let fx: Vec<_> = step(&mut state, Event::ShaderChanged).into_iter().collect();
    assert!(cancel2.load(Ordering::Acquire), "shader change must cancel in-flight worker");
    assert_eq!(fx, [Effect::Drain(2)]);
    assert!(matches!(state, State::Idle));

    // Shutdown while Warming cancels the worker, asks to JoinNow, and transitions to Consumed.
    let cancel3 = Arc::new(AtomicBool::new(false));
    let (_tx3, rx3) = mpsc::channel();
    let _ = step(&mut state, Event::WorkerSpawned { cancel: cancel3.clone(), receiver: rx3, thread: 3 });
    let fx: Vec<_> = step(&mut state, Event::Shutdown).into_iter().collect();
    assert!(cancel3.load(Ordering::Acquire), "shutdown must cancel in-flight worker");
    assert_eq!(fx, [Effect::JoinNow(3)]);
    assert!(matches!(state, State::Consumed));

    // Disconnected worker (None) transitions Warming -> Consumed.
    let mut state: State<u32, u32> = State::Idle;
    let cancel4 = Arc::new(AtomicBool::new(false));
    let (_tx4, rx4) = mpsc::channel();
    let _ = step(&mut state, Event::WorkerSpawned { cancel: cancel4, receiver: rx4, thread: 4 });
    let fx: Vec<_> = step(&mut state, Event::WorkerFinished(None)).into_iter().collect();
    assert_eq!(fx, [Effect::Drain(4)]);
    assert!(matches!(state, State::Consumed));

    // SpawnFailed transitions Idle -> Consumed.
    let mut state: State<u32, u32> = State::Idle;
    let fx: Vec<_> = step(&mut state, Event::SpawnFailed).into_iter().collect();
    assert!(fx.is_empty());
    assert!(matches!(state, State::Consumed));
}

#[test]
fn prewarm_effects_set_names_each_effect_and_converts_from_bool() {
    assert_eq!(PrewarmEffects::default(), PrewarmEffects::NONE);
    assert!(PrewarmEffects::NONE.is_empty());
    assert_eq!(PrewarmEffects::from(true), PrewarmEffects::ALL);
    assert_eq!(PrewarmEffects::from(false), PrewarmEffects::NONE);
    let each = [PrewarmEffects::ISOLATION, PrewarmEffects::PLUS_LIGHTER, PrewarmEffects::LAYER_BLUR, PrewarmEffects::BACKDROP_BLUR];
    assert_eq!(each.into_iter().fold(PrewarmEffects::NONE, |all, one| all | one), PrewarmEffects::ALL);
    for (i, one) in each.into_iter().enumerate() {
        assert!(!one.is_empty() && PrewarmEffects::ALL.contains(one));
        assert!(PrewarmEffects::ALL.without(one).with(one) == PrewarmEffects::ALL);
        for (j, other) in each.into_iter().enumerate() {
            assert_eq!(one.contains(other), i == j, "{one:?} and {other:?} are distinct effects");
        }
    }
    let glass = PrewarmEffects::BACKDROP_BLUR | PrewarmEffects::ISOLATION;
    assert!(glass.contains(PrewarmEffects::BACKDROP_BLUR) && glass.contains(PrewarmEffects::ISOLATION));
    assert!(!glass.contains(PrewarmEffects::PLUS_LIGHTER) && !glass.contains(PrewarmEffects::LAYER_BLUR));
    assert_eq!(glass.without(PrewarmEffects::ISOLATION), PrewarmEffects::BACKDROP_BLUR);
}

#[test]
fn worker_rests_until_the_window_is_still_or_the_limit_passes() {
    use std::time::Duration;
    let ms = Duration::from_millis;
    // Still for less than the quiet time: waits for the rest of it.
    assert_eq!(pace::rest_left(ms(0), ms(0), ms(300), ms(5000)), Some(ms(300)));
    assert_eq!(pace::rest_left(ms(120), ms(900), ms(300), ms(5000)), Some(ms(180)));
    // Still for the quiet time: compiles.
    assert_eq!(pace::rest_left(ms(300), ms(300), ms(300), ms(5000)), None);
    // A window that keeps drawing: never longer than the limit, and the last wait ends on it.
    assert_eq!(pace::rest_left(ms(10), ms(4900), ms(300), ms(5000)), Some(ms(100)));
    assert_eq!(pace::rest_left(ms(10), ms(5000), ms(300), ms(5000)), None);
    // No quiet time (tests): at once.
    assert_eq!(pace::rest_left(ms(0), ms(0), ms(0), ms(5000)), None);
}

#[test]
fn an_effect_is_built_by_the_worker_or_by_the_frame_never_both() {
    let link = pace::Link::default();
    // The frame comes first: it claims the effect and the worker skips it.
    assert!(!link.begun_or_claim(PrewarmEffects::BACKDROP_BLUR), "nothing begun: the frame builds it");
    assert!(!link.start(PrewarmEffects::BACKDROP_BLUR), "the worker skips what a frame claimed");
    // The worker comes first: a frame that needs the effect waits for it instead of building.
    assert!(link.start(PrewarmEffects::ISOLATION));
    assert!(link.begun_or_claim(PrewarmEffects::ISOLATION), "begun: the frame waits for the worker");
    // Claims and starts are per effect.
    assert!(link.start(PrewarmEffects::LAYER_BLUR));
    assert!(!link.begun_or_claim(PrewarmEffects::PLUS_LIGHTER));
}

#[test]
fn app_and_window_spec_prewarm_effects_default_none_and_opt_in() {
    let spec = || crate::AppSpec::new("Test", (320.0, 240.0), || (), |_, _| crate::el());
    let app_default = crate::App::new(spec());
    assert!(app_default.prewarm_effects.is_empty(), "App::prewarm_effects must default to none");

    let app_on = crate::App::new(spec()).prewarm_effects(true);
    assert_eq!(app_on.prewarm_effects, PrewarmEffects::ALL, "App::prewarm_effects(true) names every effect");

    let app_glass = crate::App::new(spec()).prewarm_effects(PrewarmEffects::BACKDROP_BLUR);
    assert_eq!(app_glass.prewarm_effects, PrewarmEffects::BACKDROP_BLUR, "App::prewarm_effects names one effect");

    let win_default = crate::WindowSpec::new("secondary", spec());
    assert!(win_default.app.prewarm_effects.is_empty(), "WindowSpec::prewarm_effects must default to none");

    let win_on = crate::WindowSpec::new("secondary", spec()).prewarm_effects(true);
    assert_eq!(win_on.app.prewarm_effects, PrewarmEffects::ALL, "WindowSpec::prewarm_effects(true) names every effect");

    let win_glass = crate::WindowSpec::new("secondary", spec()).prewarm_effects(PrewarmEffects::BACKDROP_BLUR | PrewarmEffects::ISOLATION);
    assert_eq!(win_glass.app.prewarm_effects, PrewarmEffects::BACKDROP_BLUR | PrewarmEffects::ISOLATION);
}

#[test]
fn effect_warm_is_disabled_by_default_and_one_shot_offscreen_never_warms() {
    let Some(gpu) = Gpu::test_headless() else { return };
    let mut renderer = Renderer::new(&gpu.device, OFFSCREEN_FORMAT);
    assert!(renderer.prewarm_effects().is_empty(), "Renderer::prewarm_effects must default to none");
    assert!(renderer.effect_warm.is_idle());

    let target = Offscreen::new(&gpu.device, 32, 16);
    let mut atlas = crate::text::TextSystem::default().atlas;
    let plain = [rect([0.0, 0.0, 32.0, 16.0], [0.2, 0.4, 0.8, 1.0])];
    // Even across multiple frames and an explicit warm_effects call, default-off stays Idle.
    let _ = render_offscreen(&gpu, &mut renderer, &target, &mut atlas, &plain);
    renderer.warm_effects(&gpu.device);
    let _ = render_offscreen(&gpu, &mut renderer, &target, &mut atlas, &plain);
    assert!(renderer.effect_warm.is_idle(), "default-off renderer must never start background warming");

    // One-shot render_runtime_to_offscreen must not trigger background warming even when opted in.
    renderer.set_prewarm_effects(true);
    assert_eq!(renderer.prewarm_effects(), PrewarmEffects::ALL);
    let mut rt = crate::Runtime::new(
        (),
        |_, _| crate::el().w_full().h_full().fill(crate::Color::WHITE),
        (32.0, 16.0),
        crate::Env::new(true, 1.0),
    );
    rt.frame(0.0);
    let mut oneshot = Renderer::new(&gpu.device, OFFSCREEN_FORMAT);
    oneshot.set_prewarm_effects(true);
    oneshot.render_runtime_to_offscreen(&gpu.device, &gpu.queue, &target, &mut rt);
    assert!(oneshot.effect_warm.is_idle(), "one-shot render_runtime_to_offscreen must not spawn background warming");
}

#[test]
fn first_frame_starts_no_effect_compilation_before_completion() {
    let Some(gpu) = Gpu::test_headless() else { return };
    let mut renderer = Renderer::new(&gpu.device, OFFSCREEN_FORMAT);
    renderer.set_prewarm_effects(true);
    assert!(renderer.effect_warm.is_idle(), "new renderer must not start effect warming");

    let mut atlas = crate::text::TextSystem::default().atlas;
    let plain = [rect([0.0, 0.0, 32.0, 16.0], [0.2, 0.4, 0.8, 1.0])];
    renderer.prepare(&gpu.device, &gpu.queue, &mut atlas, &plain, (32, 16));
    assert!(renderer.effect_warm.is_idle(), "first frame prepare must not start effect warming");
    assert!(!renderer.extra.iter().any(|(f, _)| *f == layers::FORMAT));
    assert!(renderer.backdrop_blur.is_none());
    assert!(renderer.layers.additive.is_none());
    assert!(renderer.layers.blur.is_none());

    // After frame 1 finishes, post-first-frame warm_effects starts warming when opted in.
    let target = Offscreen::new(&gpu.device, 32, 16);
    let _ = render_offscreen(&gpu, &mut renderer, &target, &mut atlas, &plain);
    renderer.warm_effects(&gpu.device);
    assert!(!renderer.effect_warm.is_idle(), "effect warming starts after first frame completes when opted in");
    renderer.wait_effect_warm();
    assert!(renderer.effect_warm.is_ready(), "warmed effect pipelines are held aside until first use");

    // Opting back out discards unharvested warmed pipelines and returns to Idle.
    renderer.set_prewarm_effects(false);
    assert!(renderer.prewarm_effects().is_empty());
    assert!(renderer.effect_warm.is_idle());
}

#[test]
fn worker_builds_only_the_named_effects() {
    let Some(gpu) = Gpu::test_headless() else { return };
    let mut renderer = Renderer::new(&gpu.device, OFFSCREEN_FORMAT);
    renderer.set_prewarm_effects(PrewarmEffects::BACKDROP_BLUR);
    let target = Offscreen::new(&gpu.device, 64, 48);
    let mut atlas = crate::text::TextSystem::default().atlas;

    let bg = rect([0.0, 0.0, 64.0, 48.0], [1.0, 0.0, 0.0, 1.0]);
    let _ = render_offscreen(&gpu, &mut renderer, &target, &mut atlas, &[bg]);
    renderer.warm_effects(&gpu.device);
    renderer.wait_effect_warm();
    renderer.wait_pinned();
    let State::Ready(warmed) = &renderer.effect_warm.state else { panic!("the named effect is held aside until first use") };
    assert_eq!(warmed.reported, PrewarmEffects::BACKDROP_BLUR, "the worker answers for the named effect alone");
    assert!(warmed.backdrop_blur.is_some());
    assert!(warmed.isolation.is_none() && warmed.plus_lighter.is_none() && warmed.layer_blur.is_none(), "unnamed effects are never compiled ahead");

    // The first backdrop blur frame takes the warmed pipelines and compiles none.
    #[cfg(feature = "test-gpu-counters")]
    let before = live_render_pipelines(&gpu.device);
    let frosted = [
        rect([0.0, 0.0, 32.0, 48.0], [1.0, 0.0, 0.0, 1.0]),
        rect([32.0, 0.0, 32.0, 48.0], [0.0, 0.0, 1.0, 1.0]),
        backdrop_blur_instance([0.0, 24.0, 64.0, 24.0], 8.0),
    ];
    let pixels = render_offscreen(&gpu, &mut renderer, &target, &mut atlas, &frosted);
    #[cfg(feature = "test-gpu-counters")]
    assert_eq!(live_render_pipelines(&gpu.device) - before, 0, "first backdrop blur frame reuses the warmed pipelines");
    let seam = ((36 * 64 + 32) * 4) as usize;
    assert!(pixels[seam] > 20 && pixels[seam + 2] > 20, "backdrop blur blended across seam: {:?}", &pixels[seam..seam + 4]);
    assert!(renderer.effect_warm.is_consumed(), "the one named effect was harvested");

    // An unnamed effect still compiles on the frame that first draws it.
    let iso = group([8.0, 8.0, 32.0, 16.0], LayerSpec { opacity: 0.5, ..Default::default() }, [1.0; 4]);
    let pixels = render_offscreen(&gpu, &mut renderer, &target, &mut atlas, &iso);
    assert!(renderer.breakers.health(breaker::Feature::Isolation).working());
    let mid = ((16 * 64 + 24) * 4) as usize;
    assert!((pixels[mid + 3] as i16 - 128).abs() <= 2, "half-opacity layer pixel: {:?}", &pixels[mid..mid + 4]);
}

#[test]
fn worker_compiles_nothing_while_the_window_draws_and_a_drop_ends_its_rest_at_once() {
    use std::time::Duration;
    let Some(gpu) = Gpu::test_headless() else { return };
    let mut renderer = Renderer::new(&gpu.device, OFFSCREEN_FORMAT);
    renderer.set_prewarm_effects(PrewarmEffects::BACKDROP_BLUR);
    // Rest for 400 ms without a frame before compiling.
    renderer.effect_warm.rest = (Duration::from_millis(400), Duration::from_secs(30));
    let steps = Arc::new(AtomicUsize::new(0));
    let steps_worker = steps.clone();
    renderer.effect_warm.on_step = Some(Arc::new(move || {
        steps_worker.fetch_add(1, Ordering::SeqCst);
    }));
    let target = Offscreen::new(&gpu.device, 32, 16);
    let mut atlas = crate::text::TextSystem::default().atlas;
    let plain = [rect([0.0, 0.0, 32.0, 16.0], [0.2, 0.4, 0.8, 1.0])];

    let _ = render_offscreen(&gpu, &mut renderer, &target, &mut atlas, &plain);
    renderer.warm_effects(&gpu.device);
    assert!(renderer.effect_warm.is_warming());
    // Frames keep coming for 600 ms, a few ms apart: the worker never sees the window rest.
    let drawing = crate::time::now();
    while drawing.elapsed() < Duration::from_millis(600) {
        let _ = render_offscreen(&gpu, &mut renderer, &target, &mut atlas, &plain);
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(steps.load(Ordering::SeqCst), 0, "no effect is compiled while the window draws");
    assert!(renderer.effect_warm.is_warming());

    // The window rests: the worker compiles and hands the pipelines over.
    renderer.wait_effect_warm();
    assert_eq!(steps.load(Ordering::SeqCst), 1);
    assert!(renderer.effect_warm.is_ready());

    // A renderer dropped while its worker rests does not wait for the rest to end.
    let mut resting = Renderer::new(&gpu.device, OFFSCREEN_FORMAT);
    resting.set_prewarm_effects(true);
    resting.effect_warm.rest = (Duration::from_secs(60), Duration::from_secs(120));
    let _ = render_offscreen(&gpu, &mut resting, &target, &mut atlas, &plain);
    resting.warm_effects(&gpu.device);
    assert!(resting.effect_warm.is_warming());
    let dropping = crate::time::now();
    drop(resting);
    assert!(dropping.elapsed() < Duration::from_secs(20), "drop woke the resting worker instead of waiting out its rest: {:?}", dropping.elapsed());
}

/// A Windows window that opens on the CPU names its effects when its hand-over to the GPU is
/// committed, not when the GPU arrives: until then no prepare starts the worker, the hand-over
/// frame's own (the second the renderer submits, after the GPU thread's warm frame) among them.
#[test]
fn no_worker_starts_until_the_effects_are_named() {
    let Some(gpu) = Gpu::test_headless() else { return };
    let mut renderer = Renderer::new(&gpu.device, OFFSCREEN_FORMAT);
    let target = Offscreen::new(&gpu.device, 32, 16);
    let mut atlas = crate::text::TextSystem::default().atlas;
    let plain = [rect([0.0, 0.0, 32.0, 16.0], [0.2, 0.4, 0.8, 1.0])];

    let _ = render_offscreen(&gpu, &mut renderer, &target, &mut atlas, &plain);
    let _ = render_offscreen(&gpu, &mut renderer, &target, &mut atlas, &plain);
    renderer.warm_effects(&gpu.device);
    assert!(!renderer.effect_warm.is_warming(), "nothing is named: no worker");

    // The hand-over is committed: the effects are named and the worker starts its rest.
    renderer.set_prewarm_effects(PrewarmEffects::BACKDROP_BLUR);
    renderer.warm_effects(&gpu.device);
    assert!(renderer.effect_warm.is_warming());
    renderer.wait_effect_warm();
    assert!(renderer.effect_warm.is_ready());
}

#[test]
fn frame_that_meets_the_worker_mid_build_waits_for_it_and_compiles_nothing_twice() {
    let Some(gpu) = Gpu::test_headless() else { return };
    let mut renderer = Renderer::new(&gpu.device, OFFSCREEN_FORMAT);
    renderer.set_prewarm_effects(PrewarmEffects::ISOLATION | PrewarmEffects::BACKDROP_BLUR);
    let target = Offscreen::new(&gpu.device, 32, 16);
    let mut atlas = crate::text::TextSystem::default().atlas;

    // Hold the worker after it has begun its first effect (isolation), before it builds it.
    let builds = Arc::new(AtomicUsize::new(0));
    let gate = Arc::new((Mutex::new((false, false)), Condvar::new()));
    let builds_worker = builds.clone();
    let gate_worker = gate.clone();
    renderer.effect_warm.on_build = Some(Arc::new(move || {
        if builds_worker.fetch_add(1, Ordering::SeqCst) > 0 { return; }
        let (lock, cv) = &*gate_worker;
        let mut guard = lock.lock().unwrap();
        guard.0 = true;
        cv.notify_all();
        while !guard.1 {
            guard = cv.wait(guard).unwrap();
        }
    }));

    let _ = render_offscreen(&gpu, &mut renderer, &target, &mut atlas, &[rect([0.0, 0.0, 32.0, 16.0], [0.0; 4])]);
    renderer.wait_pinned();
    renderer.warm_effects(&gpu.device);
    {
        let (lock, cv) = &*gate;
        let mut guard = lock.lock().unwrap();
        while !guard.0 {
            guard = cv.wait(guard).unwrap();
        }
    }
    assert!(renderer.effect_warm.is_warming());
    #[cfg(feature = "test-gpu-counters")]
    let before = live_render_pipelines(&gpu.device);

    // The worker goes on a moment after the frame began to wait for it.
    let releaser = {
        let gate = gate.clone();
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(30));
            let (lock, cv) = &*gate;
            let mut guard = lock.lock().unwrap();
            guard.1 = true;
            cv.notify_all();
        })
    };
    // An isolated layer while the worker is in the middle of isolation: the frame waits for the
    // worker's pipeline, takes it, and draws the exact half-opacity pixel.
    let iso = group([4.0, 4.0, 24.0, 8.0], LayerSpec { opacity: 0.5, ..Default::default() }, [1.0; 4]);
    let pixels = render_offscreen(&gpu, &mut renderer, &target, &mut atlas, &iso);
    releaser.join().unwrap();
    assert!(renderer.breakers.health(breaker::Feature::Isolation).working());
    let mid = ((8 * 32 + 16) * 4) as usize;
    assert!((pixels[mid + 3] as i16 - 128).abs() <= 2, "isolated layer pixel taken from the worker: {:?}", &pixels[mid..mid + 4]);

    // Isolation was compiled once, by the worker: with the `vs` entry that is the one
    // `Rgba16Float` uber pipeline (its composites are clones), then backdrop blur's own.
    renderer.wait_effect_warm();
    assert_eq!(builds.load(Ordering::SeqCst), 2, "the worker built isolation and backdrop blur");
    #[cfg(feature = "test-gpu-counters")]
    {
        let with_wait = live_render_pipelines(&gpu.device) - before;
        // The same two effects warmed with no frame in the way compile the same pipelines.
        let mut plain = Renderer::new(&gpu.device, OFFSCREEN_FORMAT);
        plain.set_prewarm_effects(PrewarmEffects::ISOLATION | PrewarmEffects::BACKDROP_BLUR);
        let _ = render_offscreen(&gpu, &mut plain, &target, &mut atlas, &[rect([0.0, 0.0, 32.0, 16.0], [0.0; 4])]);
        plain.wait_pinned();
        let before_plain = live_render_pipelines(&gpu.device);
        plain.warm_effects(&gpu.device);
        plain.wait_effect_warm();
        let _ = render_offscreen(&gpu, &mut plain, &target, &mut atlas, &iso);
        assert_eq!(with_wait, live_render_pipelines(&gpu.device) - before_plain, "a frame that waits for the worker compiles nothing a second time");
    }
    assert!(renderer.effect_warm.is_ready(), "backdrop blur stays held aside until first use");
}

#[test]
fn first_effect_frames_reuse_warmed_pipelines_without_synchronous_compilation() {
    let Some(gpu) = Gpu::test_headless() else { return };
    let mut renderer = Renderer::new(&gpu.device, OFFSCREEN_FORMAT);
    renderer.set_prewarm_effects(true);
    let target = Offscreen::new(&gpu.device, 64, 48);
    let mut atlas = crate::text::TextSystem::default().atlas;

    // Frame 1: plain frame without any effects, then post-first-frame warm.
    let bg = rect([0.0, 0.0, 64.0, 48.0], [1.0, 0.0, 0.0, 1.0]);
    let _ = render_offscreen(&gpu, &mut renderer, &target, &mut atlas, &[bg]);
    assert!(renderer.effect_warm.is_idle(), "no warming before first frame finishes");
    renderer.warm_effects(&gpu.device);
    renderer.wait_effect_warm();
    renderer.wait_pinned();
    assert!(renderer.effect_warm.is_ready());

    // Frame 2: first backdrop blur (frosted-bar style). Zero synchronous pipeline compiles,
    // and Rgba16Float is NOT inserted into `extra` or `backdrops`.
    #[cfg(feature = "test-gpu-counters")]
    let pipelines_before_backdrop = live_render_pipelines(&gpu.device);
    let frosted = [
        rect([0.0, 0.0, 32.0, 48.0], [1.0, 0.0, 0.0, 1.0]),
        rect([32.0, 0.0, 32.0, 48.0], [0.0, 0.0, 1.0, 1.0]),
        backdrop_blur_instance([0.0, 24.0, 64.0, 24.0], 8.0),
    ];
    let pixels = render_offscreen(&gpu, &mut renderer, &target, &mut atlas, &frosted);
    #[cfg(feature = "test-gpu-counters")]
    assert_eq!(
        live_render_pipelines(&gpu.device) - pipelines_before_backdrop,
        0,
        "first backdrop blur frame must reuse warmed pipelines without compiling"
    );
    assert!(renderer.backdrop_blur.is_some());
    assert!(!renderer.extra.iter().any(|(f, _)| *f == layers::FORMAT), "glass-only frame must not add Rgba16Float to extra");
    assert!(!renderer.backdrops.iter().any(|b| b.format == layers::FORMAT), "glass-only frame must not allocate an Rgba16Float backdrop");
    assert_eq!(renderer.backdrops.len(), 1);
    assert_eq!(renderer.backdrops[0].format, OFFSCREEN_FORMAT);
    // Seam at x=32 inside the blurred bottom bar should blend red and blue.
    let seam = ((36 * 64 + 32) * 4) as usize;
    assert!(pixels[seam] > 20 && pixels[seam + 2] > 20, "backdrop blur blended across seam: {:?}", &pixels[seam..seam + 4]);

    // Frame 3: first isolated layer (opacity 0.5). Zero synchronous pipeline compiles.
    #[cfg(feature = "test-gpu-counters")]
    let pipelines_before_iso = live_render_pipelines(&gpu.device);
    let iso = group([8.0, 8.0, 32.0, 16.0], LayerSpec { opacity: 0.5, ..Default::default() }, [1.0; 4]);
    let pixels = render_offscreen(&gpu, &mut renderer, &target, &mut atlas, &iso);
    #[cfg(feature = "test-gpu-counters")]
    assert_eq!(
        live_render_pipelines(&gpu.device) - pipelines_before_iso,
        0,
        "first isolated layer frame must reuse warmed Rgba16Float pipeline and clone composites"
    );
    assert!(renderer.breakers.health(breaker::Feature::Isolation).working());
    let mid = ((16 * 64 + 24) * 4) as usize;
    assert!((pixels[mid + 3] as i16 - 128).abs() <= 2, "half-opacity layer pixel: {:?}", &pixels[mid..mid + 4]);

    // Frame 4: first Blend::PlusLighter and Gaussian layer blur. Zero synchronous compiles.
    #[cfg(feature = "test-gpu-counters")]
    let pipelines_before_add_blur = live_render_pipelines(&gpu.device);
    let add_blur = group(
        [8.0, 8.0, 32.0, 16.0],
        LayerSpec { opacity: 1.0, blend: Blend::PlusLighter, blur: 4.0, ..Default::default() },
        [1.0; 4],
    );
    let pixels = render_offscreen(&gpu, &mut renderer, &target, &mut atlas, &add_blur);
    #[cfg(feature = "test-gpu-counters")]
    assert_eq!(
        live_render_pipelines(&gpu.device) - pipelines_before_add_blur,
        0,
        "first PlusLighter + layer-blur frame must reuse warmed pipelines without compiling"
    );
    assert!(renderer.breakers.health(breaker::Feature::PlusLighter).working());
    assert!(renderer.breakers.health(breaker::Feature::LayerBlur).working());
    assert!(renderer.effect_warm.is_consumed(), "all warmed pipelines harvested");
    let glow_outside = ((6 * 64 + 24) * 4) as usize;
    assert!(pixels[glow_outside + 3] > 0, "layer blur spreads past rect edge: {:?}", &pixels[glow_outside..glow_outside + 4]);
}

#[test]
fn glass_only_effect_backdrop_frame_allocates_zero_rgba16float_backdrops() {
    let Some(gpu) = Gpu::test_headless() else { return };
    let mut renderer = Renderer::new(&gpu.device, OFFSCREEN_FORMAT);
    renderer.set_prewarm_effects(true);
    let mut atlas = crate::text::TextSystem::default().atlas;
    let target = Offscreen::new(&gpu.device, 32, 32);

    let _ = render_offscreen(&gpu, &mut renderer, &target, &mut atlas, &[rect([0.0, 0.0, 32.0, 32.0], [1.0; 4])]);
    renderer.warm_effects(&gpu.device);
    renderer.wait_effect_warm();
    assert!(renderer.effect_warm.is_ready());

    let mut glass_slot = rect([0.0, 0.0, 16.0, 16.0], [0.0, 0.0, 0.0, 1.0]);
    glass_slot.params = [1.0, KIND_EFFECT, 0.0, 1.0];
    renderer.prepare(&gpu.device, &gpu.queue, &mut atlas, &[glass_slot], (32, 32));
    assert!(!renderer.extra.iter().any(|(f, _)| *f == layers::FORMAT));
    assert!(!renderer.backdrops.iter().any(|b| b.format == layers::FORMAT));
    assert_eq!(renderer.backdrops.len(), 1);
    assert_eq!(renderer.backdrops[0].format, OFFSCREEN_FORMAT);
}

#[cfg(feature = "test-gpu-counters")]
#[test]
fn add_layer_format_and_plus_lighter_clone_when_vertex_entry_is_vs() {
    let Some(gpu) = Gpu::test_headless() else { return };
    let mut renderer = Renderer::new(&gpu.device, OFFSCREEN_FORMAT);
    let mut atlas = crate::text::TextSystem::default().atlas;
    // Settle Windows pinned-pipeline background worker first; no submit happened so effect_warm stays Idle.
    renderer.prepare(&gpu.device, &gpu.queue, &mut atlas, &[], (32, 16));
    renderer.wait_pinned();
    assert!(renderer.effect_warm.is_idle());
    // Frame 1 (no warming yet): synchronous isolation should create only 1 pipeline (Rgba16Float
    // target) and clone both the main-format and Rgba16Float layer composites.
    let before_iso = live_render_pipelines(&gpu.device);
    let iso = group([4.0, 4.0, 24.0, 8.0], LayerSpec { opacity: 0.5, ..Default::default() }, [1.0; 4]);
    renderer.prepare(&gpu.device, &gpu.queue, &mut atlas, &iso, (32, 16));
    assert_eq!(
        live_render_pipelines(&gpu.device) - before_iso,
        1,
        "isolation with vs entry compiles only Rgba16Float uber pipeline and clones composites"
    );
    // Synchronous PlusLighter should create only 1 pipeline and clone its composite.
    let before_add = live_render_pipelines(&gpu.device);
    let add = group([4.0, 4.0, 24.0, 8.0], LayerSpec { opacity: 0.5, blend: Blend::PlusLighter, ..Default::default() }, [1.0; 4]);
    renderer.prepare(&gpu.device, &gpu.queue, &mut atlas, &add, (32, 16));
    assert_eq!(
        live_render_pipelines(&gpu.device) - before_add,
        1,
        "PlusLighter with vs entry compiles one pipeline and clones its composite"
    );
}

#[test]
fn synchronous_fallback_works_while_worker_is_in_flight() {
    let Some(gpu) = Gpu::test_headless() else { return };
    let mut renderer = Renderer::new(&gpu.device, OFFSCREEN_FORMAT);
    renderer.set_prewarm_effects(true);
    let target = Offscreen::new(&gpu.device, 32, 16);
    let mut atlas = crate::text::TextSystem::default().atlas;

    // Hold the background worker at a gate so it is guaranteed still Warming during Frame 2.
    let gate = Arc::new((Mutex::new((false, false)), Condvar::new()));
    let gate_worker = gate.clone();
    renderer.effect_warm.on_step = Some(Arc::new(move || {
        let (lock, cv) = &*gate_worker;
        let mut guard = lock.lock().unwrap();
        guard.0 = true;
        cv.notify_all();
        while !guard.1 {
            guard = cv.wait(guard).unwrap();
        }
    }));

    let _ = render_offscreen(&gpu, &mut renderer, &target, &mut atlas, &[rect([0.0, 0.0, 32.0, 16.0], [0.0; 4])]);
    renderer.warm_effects(&gpu.device);
    // Wait until the worker enters the gate.
    {
        let (lock, cv) = &*gate;
        let mut guard = lock.lock().unwrap();
        while !guard.0 {
            guard = cv.wait(guard).unwrap();
        }
    }
    assert!(renderer.effect_warm.is_warming());

    // Render an isolated layer while the worker is still blocked before it began isolation: the
    // frame claims the effect, builds it synchronously without blocking and draws the exact
    // half-opacity pixel.
    let iso = group([4.0, 4.0, 24.0, 8.0], LayerSpec { opacity: 0.5, ..Default::default() }, [1.0; 4]);
    let pixels = render_offscreen(&gpu, &mut renderer, &target, &mut atlas, &iso);
    assert!(renderer.breakers.health(breaker::Feature::Isolation).working());
    let mid = ((8 * 32 + 16) * 4) as usize;
    assert!((pixels[mid + 3] as i16 - 128).abs() <= 2, "fallback isolated layer pixel: {:?}", &pixels[mid..mid + 4]);

    // Release the worker and wait for it: it skips the claimed isolation and builds the rest.
    {
        let (lock, cv) = &*gate;
        let mut guard = lock.lock().unwrap();
        guard.1 = true;
        cv.notify_all();
    }
    renderer.wait_effect_warm();
    renderer.poll_effect_warm();
    let State::Ready(warmed) = &renderer.effect_warm.state else { panic!("remaining unbuilt features stay ready") };
    assert_eq!(warmed.reported, PrewarmEffects::ALL.without(PrewarmEffects::ISOLATION), "the worker never builds what a frame claimed");
    assert!(warmed.isolation.is_none());
}

#[test]
fn breaker_refusal_on_background_worker_drops_invalid_pipelines() {
    let Some(gpu) = Gpu::test_headless_wgpu() else { return };
    let target = Offscreen::new(&gpu.device, 32, 16);

    for (feature, spec) in [
        (breaker::Feature::Isolation, LayerSpec { opacity: 0.5, ..Default::default() }),
        (breaker::Feature::PlusLighter, LayerSpec { opacity: 0.5, blend: Blend::PlusLighter, ..Default::default() }),
        (breaker::Feature::LayerBlur, LayerSpec { blur: 4.0, ..Default::default() }),
    ] {
        let mut renderer = Renderer::new(&gpu.device, OFFSCREEN_FORMAT);
        renderer.set_prewarm_effects(true);
        renderer.breakers.refuse = Some(feature);
        let mut atlas = crate::text::TextSystem::default().atlas;

        // Frame 1: plain frame -> warm with refusal active -> wait for worker.
        let _ = render_offscreen(&gpu, &mut renderer, &target, &mut atlas, &[rect([0.0, 0.0, 32.0, 16.0], [0.0; 4])]);
        renderer.warm_effects(&gpu.device);
        renderer.wait_effect_warm();

        // Frame 2: effect frame harvests the refused verdict, drops invalid pipelines, and
        // never poisons the command encoder.
        let instances = group([4.0, 4.0, 24.0, 8.0], spec, [1.0; 4]);
        for _ in 0..2 {
            let scope = gpu.device.push_error_scope(wgpu::ErrorFilter::Validation);
            let _ = render_offscreen(&gpu, &mut renderer, &target, &mut atlas, &instances);
            let err = pollster::block_on(scope.pop());
            assert!(err.is_none(), "refused warmed {feature:?} poisoned frame: {err:?}");
        }
        assert!(matches!(renderer.breakers.health(feature), breaker::Health::Broken));
        match feature {
            breaker::Feature::Isolation => assert!(!renderer.extra.iter().any(|(f, _)| *f == layers::FORMAT)),
            breaker::Feature::PlusLighter => assert!(renderer.layers.additive.is_none()),
            breaker::Feature::LayerBlur => assert!(renderer.layers.blur.is_none()),
        }
    }
}

#[test]
fn classic_to_windfoil_swap_invalidates_stale_warmed_pipelines_and_rewarms() {
    let Some(gpu) = Gpu::test_headless() else { return };
    let mut renderer = Renderer::new_classic(&gpu.device, OFFSCREEN_FORMAT);
    renderer.set_prewarm_effects(true);
    let target = Offscreen::new(&gpu.device, 64, 48);
    let mut atlas = crate::text::TextSystem::default().atlas;

    // Warm Classic pipelines after Frame 1.
    let _ = render_offscreen(&gpu, &mut renderer, &target, &mut atlas, &[rect([0.0, 0.0, 64.0, 48.0], [0.0; 4])]);
    renderer.warm_effects(&gpu.device);
    renderer.wait_effect_warm();
    assert!(renderer.effect_warm.is_ready());

    // Switch Classic -> Windfoil.
    if !renderer.select(&gpu.device, variant::Event::OutlinesWanted) {
        return;
    }
    assert!(renderer.effect_warm.is_idle(), "shader swap must discard Classic warmed pipelines and reset to Idle");

    // Re-warm Windfoil pipelines and verify isolated layer + backdrop blur render cleanly
    // with zero validation errors and zero synchronous pipeline compiles.
    renderer.warm_effects(&gpu.device);
    renderer.wait_effect_warm();
    renderer.wait_pinned();
    assert!(renderer.effect_warm.is_ready());

    #[cfg(feature = "test-gpu-counters")]
    let before = live_render_pipelines(&gpu.device);
    let mut mixed = vec![
        rect([0.0, 0.0, 32.0, 48.0], [1.0, 0.0, 0.0, 1.0]),
        rect([32.0, 0.0, 32.0, 48.0], [0.0, 0.0, 1.0, 1.0]),
        backdrop_blur_instance([0.0, 24.0, 64.0, 24.0], 6.0),
    ];
    mixed.extend(group([8.0, 4.0, 20.0, 12.0], LayerSpec { opacity: 0.5, ..Default::default() }, [1.0; 4]));
    let scope = gpu.device.push_error_scope(wgpu::ErrorFilter::Validation);
    let _ = render_offscreen(&gpu, &mut renderer, &target, &mut atlas, &mixed);
    let err = pollster::block_on(scope.pop());
    assert!(err.is_none(), "Windfoil frame after re-warm had validation error: {err:?}");
    #[cfg(feature = "test-gpu-counters")]
    assert_eq!(live_render_pipelines(&gpu.device) - before, 0, "Windfoil effect frame must reuse re-warmed pipelines");
}

#[test]
fn renderer_drop_cancels_and_drains_worker_cleanly() {
    let Some(gpu) = Gpu::test_headless() else { return };
    let mut renderer = Renderer::new(&gpu.device, OFFSCREEN_FORMAT);
    renderer.set_prewarm_effects(true);
    let target = Offscreen::new(&gpu.device, 32, 16);
    let mut atlas = crate::text::TextSystem::default().atlas;

    let steps = Arc::new(AtomicUsize::new(0));
    let gate = Arc::new((Mutex::new((false, false)), Condvar::new()));
    let steps_worker = steps.clone();
    let gate_worker = gate.clone();
    renderer.effect_warm.on_step = Some(Arc::new(move || {
        let n = steps_worker.fetch_add(1, Ordering::SeqCst);
        if n == 0 {
            let (lock, cv) = &*gate_worker;
            let mut guard = lock.lock().unwrap();
            guard.0 = true;
            cv.notify_all();
            while !guard.1 {
                guard = cv.wait(guard).unwrap();
            }
        }
    }));

    let _ = render_offscreen(&gpu, &mut renderer, &target, &mut atlas, &[rect([0.0, 0.0, 32.0, 16.0], [0.0; 4])]);
    renderer.warm_effects(&gpu.device);
    // Wait until worker reaches step 0.
    {
        let (lock, cv) = &*gate;
        let mut guard = lock.lock().unwrap();
        while !guard.0 {
            guard = cv.wait(guard).unwrap();
        }
    }
    // Unblock step 0 right before dropping so Drop cancels and joins before steps 1..4 run.
    let releaser = std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(5));
        let (lock, cv) = &*gate;
        let mut guard = lock.lock().unwrap();
        guard.1 = true;
        cv.notify_all();
    });
    drop(renderer);
    releaser.join().unwrap();
    assert_eq!(steps.load(Ordering::SeqCst), 1, "cancelled worker must stop before building remaining steps");
}

#[test]
fn springs_renderer_warms_distinct_vs_layer_composites() {
    let Some(gpu) = Gpu::test_headless() else { return };
    let node = crate::gpu::springs::Node {
        x: crate::gpu::springs::Track::constant(0.0),
        y: crate::gpu::springs::Track::constant(0.0),
        opacity: crate::gpu::springs::Track::constant(1.0),
    };
    let mut renderer = Renderer::with_springs(&gpu.device, OFFSCREEN_FORMAT, &[node], &[0, 0, 0]);
    renderer.set_prewarm_effects(true);
    let target = Offscreen::new(&gpu.device, 32, 16);
    let mut atlas = crate::text::TextSystem::default().atlas;

    let plain = [rect([0.0, 0.0, 32.0, 16.0], [0.0; 4]); 3];
    let _ = render_offscreen(&gpu, &mut renderer, &target, &mut atlas, &plain);
    renderer.warm_effects(&gpu.device);
    renderer.wait_effect_warm();

    #[cfg(feature = "test-gpu-counters")]
    let before = live_render_pipelines(&gpu.device);
    let iso = group([4.0, 4.0, 24.0, 8.0], LayerSpec { opacity: 0.5, ..Default::default() }, [1.0; 4]);
    let pixels = render_offscreen(&gpu, &mut renderer, &target, &mut atlas, &iso);
    #[cfg(feature = "test-gpu-counters")]
    assert_eq!(
        live_render_pipelines(&gpu.device) - before,
        0,
        "springs renderer reuses warmed vs composites with zero synchronous compiles"
    );
    let mid = ((8 * 32 + 16) * 4) as usize;
    assert!((pixels[mid + 3] as i16 - 128).abs() <= 2, "springs isolated layer pixel: {:?}", &pixels[mid..mid + 4]);
}
