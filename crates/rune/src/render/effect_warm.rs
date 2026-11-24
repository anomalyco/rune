//! Post-first-frame background warming of optional effect GPU pipelines.
//!
//! Isolated layers (`Rgba16Float` uber target and composites), `Blend::PlusLighter`, Gaussian
//! layer blur, and backdrop blur are otherwise compiled synchronously on the first frame that uses
//! them, causing a multi-millisecond hitch on the UI thread. After the first frame completes on
//! native targets, `EffectWarm` compiles any still-unbuilt effect pipelines once on a background
//! worker thread from cloned `wgpu` handles inside error scopes, then holds them aside until
//! first use.
//!
//! Holding the warmed `Rgba16Float` uber pipeline inside `EffectWarm` (rather than inserting
//! `Rgba16Float` into `Renderer::extra` ahead of time) ensures glass-only frames never allocate
//! an extra window-sized `Rgba16Float` backdrop texture.

#[cfg(not(target_arch = "wasm32"))]
use std::sync::atomic::{AtomicBool, Ordering};
#[cfg(not(target_arch = "wasm32"))]
use std::sync::{Arc, mpsc};
#[cfg(not(target_arch = "wasm32"))]
use std::thread::JoinHandle;

#[cfg(not(target_arch = "wasm32"))]
use crate::machine::Effects;

use super::{Renderer, backdrop_blur, breaker};
#[cfg(not(target_arch = "wasm32"))]
use super::{Blend, dxbc, layer_blur, layers};

/// Warmed pipelines for isolated layers (`breaker::Feature::Isolation`).
#[cfg(not(target_arch = "wasm32"))]
pub(super) struct WarmedIsolation {
    /// Uber pipeline targeting `layers::FORMAT` (`Rgba16Float`).
    pub(super) target: wgpu::RenderPipeline,
    /// Layer composite pipelines using the `"vs"` vertex entry; empty when `vertex_entry == "vs"`
    /// because the renderer clones its main / `extra` pipelines instead of compiling duplicates.
    pub(super) composites: Vec<(wgpu::TextureFormat, wgpu::RenderPipeline)>,
}

/// Pipelines produced by one background warming pass, held aside until first use.
#[cfg(not(target_arch = "wasm32"))]
pub(super) struct WarmedEffects {
    pub(super) isolation: Option<Result<WarmedIsolation, String>>,
    pub(super) plus_lighter: Option<Result<(wgpu::RenderPipeline, wgpu::RenderPipeline), String>>,
    pub(super) layer_blur: Option<Result<layer_blur::Pipeline, String>>,
    pub(super) backdrop_blur: Option<backdrop_blur::WarmedBackdropBlur>,
    #[cfg(test)]
    pub(super) refused: Option<breaker::Feature>,
}

#[cfg(not(target_arch = "wasm32"))]
impl WarmedEffects {
    pub(super) fn is_empty(&self) -> bool {
        self.isolation.is_none()
            && self.plus_lighter.is_none()
            && self.layer_blur.is_none()
            && self.backdrop_blur.is_none()
    }
}

/// Explicit lifecycle state of post-first-frame effect pipeline warming.
#[cfg(not(target_arch = "wasm32"))]
#[derive(Default)]
pub(super) enum State<W = WarmedEffects, H = JoinHandle<()>> {
    /// Not started for the current shader (before the first frame finishes, or after a
    /// Classic -> Windfoil shader swap).
    #[default]
    Idle,
    /// One background worker is compiling unbuilt effect pipelines for the current shader.
    Warming {
        cancel: Arc<AtomicBool>,
        receiver: mpsc::Receiver<W>,
        thread: H,
    },
    /// Worker finished; unharvested pipelines sit here until their feature is first used.
    Ready(W),
    /// All warmed pipelines were harvested (or warming was not needed / abandoned for this shader).
    Consumed,
}

/// Inputs to [`step`].
#[cfg(not(target_arch = "wasm32"))]
pub(super) enum Event<W = WarmedEffects, H = JoinHandle<()>> {
    /// The first frame (or first frame after a shader swap) finished.
    FirstFrameFinished { has_work: bool },
    /// The background worker thread started.
    WorkerSpawned {
        cancel: Arc<AtomicBool>,
        receiver: mpsc::Receiver<W>,
        thread: H,
    },
    /// Spawning the background worker failed.
    SpawnFailed,
    /// The background worker sent its result (or disconnected without sending).
    WorkerFinished(Option<W>),
    /// Every item in `State::Ready` has been harvested or pruned.
    AllConsumed,
    /// The uber shader changed (Classic -> Windfoil): cancel any in-flight worker, drop stale
    /// pipelines, and return to `Idle`.
    ShaderChanged,
    /// The renderer is dropping: cancel and join any active worker before GPU handles drop.
    Shutdown,
}

/// Side effects returned by [`step`] for [`EffectWarm`] to execute.
#[cfg(not(target_arch = "wasm32"))]
#[derive(Debug, PartialEq, Eq)]
pub(super) enum Effect<H = JoinHandle<()>> {
    /// Spawn the background warming worker and report `WorkerSpawned` or `SpawnFailed`.
    SpawnWorker,
    /// Retain `H` to reap non-blockingly once finished (and join on `Drop`).
    Drain(H),
    /// Join `H` immediately (used only during `Shutdown` / `Drop`).
    JoinNow(H),
}

/// Pure state transition for [`EffectWarm`].
#[cfg(not(target_arch = "wasm32"))]
pub(super) fn step<W, H>(state: &mut State<W, H>, event: Event<W, H>) -> Effects<Effect<H>> {
    match event {
        Event::FirstFrameFinished { has_work } => match state {
            State::Idle if has_work => Effects::one(Effect::SpawnWorker),
            State::Idle => {
                *state = State::Consumed;
                Effects::none()
            }
            State::Warming { .. } | State::Ready(_) | State::Consumed => Effects::none(),
        },
        Event::WorkerSpawned { cancel, receiver, thread } => match state {
            State::Idle => {
                *state = State::Warming { cancel, receiver, thread };
                Effects::none()
            }
            State::Warming { .. } | State::Ready(_) | State::Consumed => {
                cancel.store(true, Ordering::Release);
                Effects::one(Effect::Drain(thread))
            }
        },
        Event::SpawnFailed => {
            if matches!(state, State::Idle) {
                *state = State::Consumed;
            }
            Effects::none()
        }
        Event::WorkerFinished(result) => {
            if !matches!(state, State::Warming { .. }) {
                return Effects::none();
            }
            let next = match result {
                Some(warmed) => State::Ready(warmed),
                None => State::Consumed,
            };
            let State::Warming { thread, .. } = std::mem::replace(state, next) else { unreachable!() };
            Effects::one(Effect::Drain(thread))
        }
        Event::AllConsumed => {
            if matches!(state, State::Ready(_)) {
                *state = State::Consumed;
            }
            Effects::none()
        }
        Event::ShaderChanged => match std::mem::replace(state, State::Idle) {
            State::Idle | State::Ready(_) | State::Consumed => Effects::none(),
            State::Warming { cancel, thread, .. } => {
                cancel.store(true, Ordering::Release);
                Effects::one(Effect::Drain(thread))
            }
        },
        Event::Shutdown => match std::mem::replace(state, State::Consumed) {
            State::Warming { cancel, thread, .. } => {
                cancel.store(true, Ordering::Release);
                Effects::one(Effect::JoinNow(thread))
            }
            State::Idle | State::Ready(_) | State::Consumed => Effects::none(),
        },
    }
}

/// Which effect pipeline groups still need warming when the worker starts.
#[cfg(not(target_arch = "wasm32"))]
#[derive(Clone, Copy)]
struct WarmPlan {
    isolation: bool,
    plus_lighter: bool,
    layer_blur: bool,
    backdrop_blur: bool,
}

#[cfg(not(target_arch = "wasm32"))]
impl WarmPlan {
    fn any(self) -> bool {
        self.isolation || self.plus_lighter || self.layer_blur || self.backdrop_blur
    }
}

#[cfg(not(target_arch = "wasm32"))]
struct WarmJob {
    device: wgpu::Device,
    layout: wgpu::PipelineLayout,
    bind_layout: wgpu::BindGroupLayout,
    shader: dxbc::Shaders,
    format: wgpu::TextureFormat,
    vertex_entry: &'static str,
    plan: WarmPlan,
    cancel: Arc<AtomicBool>,
    sender: mpsc::Sender<WarmedEffects>,
    #[cfg(test)]
    refuse: Option<breaker::Feature>,
    #[cfg(test)]
    on_step: Option<Arc<dyn Fn() + Send + Sync>>,
}

/// Owner of background effect-pipeline warming on [`Renderer`].
///
/// Drains any active or cancelled worker threads on [`Drop`] so no background thread outlives
/// the renderer or holds GPU handles across shutdown.
#[cfg(not(target_arch = "wasm32"))]
#[derive(Default)]
pub(super) struct EffectWarm {
    state: State,
    /// Finished or cancelled workers kept so `Drop` joins every spawned thread without blocking
    /// the UI thread during rendering or shader swaps.
    draining: Vec<JoinHandle<()>>,
    /// Test-only hook invoked on the worker before each build step (for deterministic fallback
    /// and cancellation tests).
    #[cfg(test)]
    pub(super) on_step: Option<Arc<dyn Fn() + Send + Sync>>,
}

#[cfg(not(target_arch = "wasm32"))]
impl Drop for EffectWarm {
    fn drop(&mut self) {
        for effect in step(&mut self.state, Event::Shutdown) {
            match effect {
                Effect::JoinNow(thread) | Effect::Drain(thread) => {
                    thread.join().ok();
                }
                Effect::SpawnWorker => {}
            }
        }
        for thread in self.draining.drain(..) {
            thread.join().ok();
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl EffectWarm {
    /// Non-blockingly reaps any worker handles that have already finished.
    fn reap_finished(&mut self) {
        let mut i = 0;
        while i < self.draining.len() {
            if self.draining[i].is_finished() {
                let handle = self.draining.swap_remove(i);
                handle.join().ok();
            } else {
                i += 1;
            }
        }
    }

    fn apply_effects(&mut self, effects: Effects<Effect>) -> bool {
        let mut spawn = false;
        for effect in effects {
            match effect {
                Effect::SpawnWorker => spawn = true,
                Effect::Drain(thread) => self.draining.push(thread),
                Effect::JoinNow(thread) => {
                    thread.join().ok();
                }
            }
        }
        self.reap_finished();
        spawn
    }

    pub(super) fn shader_changed(&mut self) {
        let effects = step(&mut self.state, Event::ShaderChanged);
        self.apply_effects(effects);
    }

    /// Polls any in-flight worker non-blockingly and transitions `Warming -> Ready` when done.
    fn poll(&mut self) {
        self.reap_finished();
        let arrived = match &self.state {
            State::Warming { receiver, .. } => match receiver.try_recv() {
                Ok(warmed) => Some(Some(warmed)),
                Err(mpsc::TryRecvError::Disconnected) => Some(None),
                Err(mpsc::TryRecvError::Empty) => None,
            },
            _ => None,
        };
        if let Some(result) = arrived {
            let effects = step(&mut self.state, Event::WorkerFinished(result));
            self.apply_effects(effects);
            if matches!(&self.state, State::Ready(w) if w.is_empty()) {
                let _ = step(&mut self.state, Event::AllConsumed);
            }
        }
    }

    fn ready_mut(&mut self) -> Option<&mut WarmedEffects> {
        self.poll();
        match &mut self.state {
            State::Ready(warmed) => Some(warmed),
            _ => None,
        }
    }

    fn finish_if_empty(&mut self) {
        if matches!(&self.state, State::Ready(w) if w.is_empty()) {
            let _ = step(&mut self.state, Event::AllConsumed);
        }
    }

    #[cfg(test)]
    pub(super) fn is_idle(&self) -> bool {
        matches!(self.state, State::Idle)
    }

    #[cfg(test)]
    pub(super) fn is_warming(&self) -> bool {
        matches!(self.state, State::Warming { .. })
    }

    #[cfg(test)]
    pub(super) fn is_ready(&self) -> bool {
        matches!(self.state, State::Ready(_))
    }

    #[cfg(test)]
    pub(super) fn is_consumed(&self) -> bool {
        matches!(self.state, State::Consumed)
    }

    /// Test-only: blocks until an in-flight worker finishes so GPU counter tests are deterministic.
    #[cfg(test)]
    pub(super) fn wait(&mut self) {
        let arrived = match &self.state {
            State::Warming { receiver, .. } => Some(receiver.recv().ok()),
            _ => None,
        };
        if let Some(result) = arrived {
            let effects = step(&mut self.state, Event::WorkerFinished(result));
            self.apply_effects(effects);
            if matches!(&self.state, State::Ready(w) if w.is_empty()) {
                let _ = step(&mut self.state, Event::AllConsumed);
            }
            for handle in self.draining.drain(..) {
                handle.join().ok();
            }
        }
    }
}

#[cfg(target_arch = "wasm32")]
#[derive(Default)]
pub(super) struct EffectWarm;

#[cfg(target_arch = "wasm32")]
impl EffectWarm {
    pub(super) fn shader_changed(&mut self) {}
}

#[cfg(not(target_arch = "wasm32"))]
fn checked_build<T>(device: &wgpu::Device, build: impl FnOnce() -> T) -> Result<T, String> {
    let (value, errors) = rune_gpu::health::with_error_scopes(device, build);
    match errors.internal.or(errors.memory).or(errors.validation) {
        None => Ok(value),
        Some(err) => Err(err.to_string()),
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn build_isolation(
    device: &wgpu::Device,
    layout: &wgpu::PipelineLayout,
    shader: &dxbc::Shaders,
    format: wgpu::TextureFormat,
    vertex_entry: &'static str,
    #[cfg(test)] refuse: Option<breaker::Feature>,
) -> Result<WarmedIsolation, String> {
    #[cfg(test)]
    let target_format = if refuse == Some(breaker::Feature::Isolation) {
        wgpu::TextureFormat::Rgba8Snorm
    } else {
        layers::FORMAT
    };
    #[cfg(not(test))]
    let target_format = layers::FORMAT;
    checked_build(device, || {
        let target = Renderer::pipeline(device, layout, shader, target_format, vertex_entry, Blend::SourceOver);
        let composites = if vertex_entry == "vs" {
            Vec::new()
        } else {
            let mut list = vec![(format, Renderer::pipeline(device, layout, shader, format, "vs", Blend::SourceOver))];
            if format != layers::FORMAT {
                list.push((layers::FORMAT, Renderer::pipeline(device, layout, shader, target_format, "vs", Blend::SourceOver)));
            }
            list
        };
        WarmedIsolation { target, composites }
    })
}

#[cfg(not(target_arch = "wasm32"))]
fn build_plus_lighter(
    device: &wgpu::Device,
    layout: &wgpu::PipelineLayout,
    shader: &dxbc::Shaders,
    vertex_entry: &'static str,
    #[cfg(test)] refuse: Option<breaker::Feature>,
) -> Result<(wgpu::RenderPipeline, wgpu::RenderPipeline), String> {
    #[cfg(test)]
    let target_format = if refuse == Some(breaker::Feature::PlusLighter) {
        wgpu::TextureFormat::Rgba8Snorm
    } else {
        layers::FORMAT
    };
    #[cfg(not(test))]
    let target_format = layers::FORMAT;
    checked_build(device, || {
        let primary = Renderer::pipeline(device, layout, shader, target_format, vertex_entry, Blend::PlusLighter);
        let composite = if vertex_entry == "vs" {
            primary.clone()
        } else {
            Renderer::pipeline(device, layout, shader, target_format, "vs", Blend::PlusLighter)
        };
        (primary, composite)
    })
}

#[cfg(not(target_arch = "wasm32"))]
fn build_layer_blur(
    device: &wgpu::Device,
    #[cfg(test)] refuse: Option<breaker::Feature>,
) -> Result<layer_blur::Pipeline, String> {
    #[cfg(test)]
    let target_format = if refuse == Some(breaker::Feature::LayerBlur) {
        wgpu::TextureFormat::Rgba8Snorm
    } else {
        layers::FORMAT
    };
    #[cfg(not(test))]
    let target_format = layers::FORMAT;
    checked_build(device, || layer_blur::Pipeline::new(device, target_format))
}

#[cfg(not(target_arch = "wasm32"))]
fn build_backdrop_blur(
    device: &wgpu::Device,
    bind_layout: &wgpu::BindGroupLayout,
    format: wgpu::TextureFormat,
) -> Option<backdrop_blur::WarmedBackdropBlur> {
    checked_build(device, || {
        let mut blur = backdrop_blur::WarmedBackdropBlur::new(device, bind_layout);
        blur.ensure_composite(device, format);
        if format != layers::FORMAT {
            blur.ensure_composite(device, layers::FORMAT);
        }
        blur
    })
    .ok()
}

#[cfg(not(target_arch = "wasm32"))]
fn run_worker(job: WarmJob) {
    let WarmJob {
        device,
        layout,
        bind_layout,
        shader,
        format,
        vertex_entry,
        plan,
        cancel,
        sender,
        #[cfg(test)]
        refuse,
        #[cfg(test)]
        on_step,
    } = job;
    let cancelled = || cancel.load(Ordering::Acquire);
    let mut out = WarmedEffects {
        isolation: None,
        plus_lighter: None,
        layer_blur: None,
        backdrop_blur: None,
        #[cfg(test)]
        refused: refuse,
    };

    if plan.isolation && !cancelled() {
        #[cfg(test)]
        if let Some(hook) = &on_step { hook(); }
        if !cancelled() {
            out.isolation = Some(build_isolation(
                &device, &layout, &shader, format, vertex_entry,
                #[cfg(test)] refuse,
            ));
        }
    }
    if plan.plus_lighter && !cancelled() {
        #[cfg(test)]
        if let Some(hook) = &on_step { hook(); }
        if !cancelled() {
            out.plus_lighter = Some(build_plus_lighter(
                &device, &layout, &shader, vertex_entry,
                #[cfg(test)] refuse,
            ));
        }
    }
    if plan.layer_blur && !cancelled() {
        #[cfg(test)]
        if let Some(hook) = &on_step { hook(); }
        if !cancelled() {
            out.layer_blur = Some(build_layer_blur(
                &device,
                #[cfg(test)] refuse,
            ));
        }
    }
    if plan.backdrop_blur && !cancelled() {
        #[cfg(test)]
        if let Some(hook) = &on_step { hook(); }
        if !cancelled() {
            out.backdrop_blur = build_backdrop_blur(&device, &bind_layout, format);
        }
    }
    if !cancelled() {
        sender.send(out).ok();
    }
}

impl Renderer {
    /// Starts compiling unbuilt effect pipelines (isolated layers, `Blend::PlusLighter`,
    /// layer blur, and backdrop blur) on a background worker thread.
    ///
    /// Called only after the first frame finishes so startup latency is untouched; subsequent
    /// calls are an O(1) no-op until a `Classic -> Windfoil` shader swap resets the state machine.
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn warm_effects(&mut self, device: &wgpu::Device) {
        let plan = WarmPlan {
            isolation: matches!(self.breakers.health(breaker::Feature::Isolation), breaker::Health::Untried),
            plus_lighter: matches!(self.breakers.health(breaker::Feature::PlusLighter), breaker::Health::Untried),
            layer_blur: matches!(self.breakers.health(breaker::Feature::LayerBlur), breaker::Health::Untried),
            backdrop_blur: self.backdrop_blur.is_none(),
        };
        let effects = step(&mut self.effect_warm.state, Event::FirstFrameFinished { has_work: plan.any() });
        if !self.effect_warm.apply_effects(effects) {
            return;
        }
        let cancel = Arc::new(AtomicBool::new(false));
        let (sender, receiver) = mpsc::channel();
        let job = WarmJob {
            device: device.clone(),
            layout: self.layout.clone(),
            bind_layout: self.bind_layout.clone(),
            shader: self.shader.clone(),
            format: self.format,
            vertex_entry: self.vertex_entry,
            plan,
            cancel: cancel.clone(),
            sender,
            #[cfg(test)]
            refuse: self.breakers.refuse,
            #[cfg(test)]
            on_step: self.effect_warm.on_step.clone(),
        };
        let spawned = std::thread::Builder::new()
            .name("rune-effect-warm".into())
            .spawn(move || {
                #[cfg(target_vendor = "apple")]
                objc2::rc::autoreleasepool(|_| run_worker(job));
                #[cfg(not(target_vendor = "apple"))]
                run_worker(job);
            });
        let next = match spawned {
            Ok(thread) => Event::WorkerSpawned { cancel, receiver, thread },
            Err(_) => Event::SpawnFailed,
        };
        let effects = step(&mut self.effect_warm.state, next);
        self.effect_warm.apply_effects(effects);
    }

    #[cfg(target_arch = "wasm32")]
    pub(crate) fn warm_effects(&mut self, _device: &wgpu::Device) {}

    /// Non-blockingly polls background effect warming, drops any warmed pipelines whose feature
    /// was already compiled synchronously while the worker ran, and reaps finished worker handles.
    #[cfg(not(target_arch = "wasm32"))]
    pub(super) fn poll_effect_warm(&mut self) {
        self.effect_warm.poll();
        if let State::Ready(warmed) = &mut self.effect_warm.state {
            if !matches!(self.breakers.health(breaker::Feature::Isolation), breaker::Health::Untried) {
                warmed.isolation = None;
            }
            if !matches!(self.breakers.health(breaker::Feature::PlusLighter), breaker::Health::Untried) {
                warmed.plus_lighter = None;
            }
            if !matches!(self.breakers.health(breaker::Feature::LayerBlur), breaker::Health::Untried) {
                warmed.layer_blur = None;
            }
            if self.backdrop_blur.is_some() {
                warmed.backdrop_blur = None;
            }
        }
        self.effect_warm.finish_if_empty();
    }

    #[cfg(target_arch = "wasm32")]
    pub(super) fn poll_effect_warm(&mut self) {}

    /// Non-blockingly harvests a warmed feature pipeline for `breaker`, returning its error-scope
    /// verdict if ready, or `None` to fall back to synchronous compilation.
    #[cfg(not(target_arch = "wasm32"))]
    pub(super) fn harvest_warmed_feature(
        &mut self,
        device: &wgpu::Device,
        feature: breaker::Feature,
    ) -> Option<breaker::Verdict> {
        #[cfg(test)]
        let refuse = self.breakers.refuse;
        let warmed = self.effect_warm.ready_mut()?;
        // If a test enabled refusal for this feature after the worker already built a valid
        // pipeline, drop the stale warmed entry so synchronous build exercises refusal.
        #[cfg(test)]
        if refuse == Some(feature) && warmed.refused != Some(feature) {
            match feature {
                breaker::Feature::Isolation => { warmed.isolation = None; }
                breaker::Feature::PlusLighter => { warmed.plus_lighter = None; }
                breaker::Feature::LayerBlur => { warmed.layer_blur = None; }
            }
            self.effect_warm.finish_if_empty();
            return None;
        }
        let verdict = match feature {
            breaker::Feature::Isolation => {
                let result = warmed.isolation.take()?;
                match result {
                    Ok(iso) => {
                        if !self.extra.iter().any(|(f, _)| *f == layers::FORMAT) {
                            self.extra.push((layers::FORMAT, iso.target));
                        }
                        for (fmt, pipe) in iso.composites {
                            if !self.layers.pipelines.iter().any(|(f, _)| *f == fmt) {
                                self.layers.pipelines.push((fmt, pipe));
                            }
                        }
                        let formats: Vec<_> = [self.format]
                            .into_iter()
                            .chain(self.extra.iter().map(|(f, _)| *f))
                            .collect();
                        for fmt in formats {
                            self.add_layer_format(device, fmt);
                        }
                        breaker::Verdict::Now(None)
                    }
                    Err(why) => breaker::Verdict::Now(Some(why)),
                }
            }
            breaker::Feature::PlusLighter => {
                let result = warmed.plus_lighter.take()?;
                match result {
                    Ok(pair) => {
                        self.layers.additive = Some(pair);
                        breaker::Verdict::Now(None)
                    }
                    Err(why) => breaker::Verdict::Now(Some(why)),
                }
            }
            breaker::Feature::LayerBlur => {
                let result = warmed.layer_blur.take()?;
                match result {
                    Ok(pipe) => {
                        self.layers.blur = Some(pipe);
                        breaker::Verdict::Now(None)
                    }
                    Err(why) => breaker::Verdict::Now(Some(why)),
                }
            }
        };
        self.effect_warm.finish_if_empty();
        Some(verdict)
    }

    #[cfg(target_arch = "wasm32")]
    pub(super) fn harvest_warmed_feature(
        &mut self,
        _device: &wgpu::Device,
        _feature: breaker::Feature,
    ) -> Option<breaker::Verdict> {
        None
    }

    /// Non-blockingly harvests pre-compiled `BackdropBlur` pipelines if ready.
    #[cfg(not(target_arch = "wasm32"))]
    pub(super) fn harvest_warmed_backdrop_blur(&mut self) -> Option<backdrop_blur::WarmedBackdropBlur> {
        let warmed = self.effect_warm.ready_mut()?;
        let blur = warmed.backdrop_blur.take();
        self.effect_warm.finish_if_empty();
        blur
    }

    #[cfg(target_arch = "wasm32")]
    pub(super) fn harvest_warmed_backdrop_blur(&mut self) -> Option<backdrop_blur::WarmedBackdropBlur> {
        None
    }

    #[cfg(all(test, not(target_arch = "wasm32")))]
    pub(crate) fn wait_effect_warm(&mut self) {
        self.effect_warm.wait();
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
#[path = "effect_warm_tests.rs"]
mod tests;
