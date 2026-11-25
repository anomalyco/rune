//! Background warming of the optional effect GPU pipelines an app names ([`PrewarmEffects`]).
//!
//! Isolated layers (`Rgba16Float` uber target and composites), `Blend::PlusLighter`, Gaussian
//! layer blur, and backdrop blur are otherwise compiled synchronously on the first frame that uses
//! them, a hitch of many frames on the UI thread while the driver compiles. On native targets,
//! once a renderer's first frame is done and its window rests, `EffectWarm` compiles the named,
//! still-unbuilt effect pipelines on one low-priority worker thread from cloned `wgpu` handles
//! inside error scopes, and hands each one over as soon as it is built, held aside until first
//! use. A frame that needs an effect earlier takes it if built, builds it itself if the worker
//! has not begun it (the worker then skips it), and waits for the rest of the build if the
//! worker is in the middle of it (`pace`): no pipeline is compiled twice.
//!
//! Holding the warmed `Rgba16Float` uber pipeline inside `EffectWarm` (rather than inserting
//! `Rgba16Float` into `Renderer::extra` ahead of time) ensures glass-only frames never allocate
//! an extra window-sized `Rgba16Float` backdrop texture.

#[cfg(not(target_arch = "wasm32"))]
mod pace;
mod set;

#[cfg(not(target_arch = "wasm32"))]
use std::sync::atomic::{AtomicBool, Ordering};
#[cfg(not(target_arch = "wasm32"))]
use std::sync::{Arc, Mutex, mpsc};
#[cfg(not(target_arch = "wasm32"))]
use std::thread::JoinHandle;

#[cfg(not(target_arch = "wasm32"))]
use crate::machine::Effects;

pub use set::PrewarmEffects;

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

/// Pipelines the background worker has handed over so far, held aside until first use. The
/// worker sends one of these per effect; the renderer merges them as they arrive.
#[cfg(not(target_arch = "wasm32"))]
#[derive(Default)]
pub(super) struct WarmedEffects {
    /// Effects the worker has answered for, built or failed.
    pub(super) reported: PrewarmEffects,
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

    /// Takes in what the worker handed over for one more effect.
    fn merge(&mut self, part: WarmedEffects) {
        self.reported |= part.reported;
        if part.isolation.is_some() { self.isolation = part.isolation; }
        if part.plus_lighter.is_some() { self.plus_lighter = part.plus_lighter; }
        if part.layer_blur.is_some() { self.layer_blur = part.layer_blur; }
        if part.backdrop_blur.is_some() { self.backdrop_blur = part.backdrop_blur; }
        #[cfg(test)]
        if part.refused.is_some() { self.refused = part.refused; }
    }

    /// Drops what the renderer built itself meanwhile.
    fn prune(&mut self, built: PrewarmEffects) {
        if built.contains(PrewarmEffects::ISOLATION) { self.isolation = None; }
        if built.contains(PrewarmEffects::PLUS_LIGHTER) { self.plus_lighter = None; }
        if built.contains(PrewarmEffects::LAYER_BLUR) { self.layer_blur = None; }
        if built.contains(PrewarmEffects::BACKDROP_BLUR) { self.backdrop_blur = None; }
    }
}

/// The effect a breaker feature's pipelines belong to.
#[cfg(not(target_arch = "wasm32"))]
fn effect_of(feature: breaker::Feature) -> PrewarmEffects {
    match feature {
        breaker::Feature::Isolation => PrewarmEffects::ISOLATION,
        breaker::Feature::PlusLighter => PrewarmEffects::PLUS_LIGHTER,
        breaker::Feature::LayerBlur => PrewarmEffects::LAYER_BLUR,
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
    /// One background worker waits for the window to rest, then compiles the named, unbuilt
    /// effect pipelines for the current shader, handing each over as it is built.
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
    /// The background worker ended: everything it handed over (or nothing).
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

#[cfg(not(target_arch = "wasm32"))]
struct WarmJob {
    device: wgpu::Device,
    layout: wgpu::PipelineLayout,
    bind_layout: wgpu::BindGroupLayout,
    shader: dxbc::Shaders,
    format: wgpu::TextureFormat,
    vertex_entry: &'static str,
    /// The named effects still unbuilt when the worker starts.
    plan: PrewarmEffects,
    cancel: Arc<AtomicBool>,
    link: Arc<pace::Link>,
    /// How long the window must rest before the worker compiles, and the longest it waits.
    rest: (std::time::Duration, std::time::Duration),
    sender: mpsc::Sender<WarmedEffects>,
    timing: Arc<Mutex<Option<(f64, f64)>>>,
    #[cfg(test)]
    refuse: Option<breaker::Feature>,
    #[cfg(test)]
    on_step: Option<Arc<dyn Fn() + Send + Sync>>,
    #[cfg(test)]
    on_build: Option<Arc<dyn Fn() + Send + Sync>>,
}

/// Owner of background effect-pipeline warming on [`Renderer`].
///
/// Drains any active or cancelled worker threads on [`Drop`] so no background thread outlives
/// the renderer or holds GPU handles across shutdown.
#[cfg(not(target_arch = "wasm32"))]
pub(super) struct EffectWarm {
    /// The effects the app named (`Renderer::set_prewarm_effects`).
    wanted: PrewarmEffects,
    state: State,
    /// `State::Warming` only: what the worker has handed over so far.
    partial: WarmedEffects,
    /// `State::Warming` only: what the frames and that worker share.
    link: Option<Arc<pace::Link>>,
    /// How long the window must rest before a worker compiles, and the longest it waits
    /// (`pace::QUIET`, `pace::LIMIT`; tests start with no wait).
    pub(super) rest: (std::time::Duration, std::time::Duration),
    /// Finished or cancelled workers kept so `Drop` joins every spawned thread without blocking
    /// the UI thread during rendering or shader swaps.
    draining: Vec<JoinHandle<()>>,
    /// Last worker run's `(wall_ms, cpu_ms)` of compiling (the rest before it left out), read by
    /// hidden perf benchmarks.
    timing: Arc<Mutex<Option<(f64, f64)>>>,
    /// Test-only hook invoked on the worker before each build step, while a frame can still
    /// claim the effect (for deterministic fallback and cancellation tests).
    #[cfg(test)]
    pub(super) on_step: Option<Arc<dyn Fn() + Send + Sync>>,
    /// Test-only hook invoked on the worker once it has begun an effect, before it builds it
    /// (for a frame that meets the worker mid-build).
    #[cfg(test)]
    pub(super) on_build: Option<Arc<dyn Fn() + Send + Sync>>,
}

#[cfg(not(target_arch = "wasm32"))]
impl Default for EffectWarm {
    fn default() -> Self {
        Self {
            wanted: PrewarmEffects::NONE,
            state: State::default(),
            partial: WarmedEffects::default(),
            link: None,
            rest: (if cfg!(test) { std::time::Duration::ZERO } else { pace::QUIET }, pace::LIMIT),
            draining: Vec::new(),
            timing: Arc::new(Mutex::new(None)),
            #[cfg(test)]
            on_step: None,
            #[cfg(test)]
            on_build: None,
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl Drop for EffectWarm {
    fn drop(&mut self) {
        for effect in step(&mut self.state, Event::Shutdown) {
            match effect {
                Effect::JoinNow(thread) | Effect::Drain(thread) => {
                    // A worker still waiting for the window to rest ends at once; one inside the
                    // driver ends after the pipeline it is compiling.
                    thread.thread().unpark();
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
                Effect::Drain(thread) => {
                    // Wakes a cancelled worker that still waits for the window to rest.
                    thread.thread().unpark();
                    self.draining.push(thread);
                }
                Effect::JoinNow(thread) => {
                    thread.thread().unpark();
                    thread.join().ok();
                }
            }
        }
        if !matches!(self.state, State::Warming { .. }) {
            self.partial = WarmedEffects::default();
            self.link = None;
        }
        self.reap_finished();
        spawn
    }

    pub(super) fn set_wanted(&mut self, wanted: PrewarmEffects) {
        if self.wanted == wanted { return; }
        self.wanted = wanted;
        // Cancel any in-flight worker and drop unharvested pipelines without blocking the UI
        // thread; the next finished frame plans again from the new set.
        let effects = step(&mut self.state, Event::ShaderChanged);
        self.apply_effects(effects);
    }

    pub(super) fn shader_changed(&mut self) {
        let effects = step(&mut self.state, Event::ShaderChanged);
        self.apply_effects(effects);
    }

    /// The worker ended: everything it handed over becomes `Ready`.
    fn worker_ended(&mut self) {
        let warmed = std::mem::take(&mut self.partial);
        let effects = step(&mut self.state, Event::WorkerFinished(Some(warmed)));
        self.apply_effects(effects);
        self.finish_if_empty();
    }

    /// Takes in, without blocking, what an in-flight worker has handed over, and transitions
    /// `Warming -> Ready` once it has ended.
    fn poll(&mut self) {
        self.reap_finished();
        let State::Warming { receiver, .. } = &self.state else { return };
        let ended = loop {
            match receiver.try_recv() {
                Ok(part) => self.partial.merge(part),
                Err(mpsc::TryRecvError::Empty) => break false,
                Err(mpsc::TryRecvError::Disconnected) => break true,
            }
        };
        if ended { self.worker_ended(); }
    }

    /// The warmed pipelines a frame that needs `effect` now may take it from, if the worker
    /// built it or is building it. `None` leaves the frame to build it itself: the worker never
    /// planned it, or had not begun it and now skips it.
    ///
    /// Blocks only while the worker is in the middle of `effect`, for the rest of that build
    /// and with the frame's priority lent to the worker, since building it a second time here
    /// would take at least as long.
    fn warmed_for(&mut self, effect: PrewarmEffects) -> Option<&mut WarmedEffects> {
        self.poll();
        if let State::Warming { receiver, thread, .. } = &self.state
            && !self.partial.reported.contains(effect)
        {
            if !self.link.as_ref().is_some_and(|link| link.begun_or_claim(effect)) { return None; }
            let ended = {
                let _lift = pace::lift(thread);
                loop {
                    match receiver.recv() {
                        Ok(part) => self.partial.merge(part),
                        Err(_) => break true,
                    }
                    if self.partial.reported.contains(effect) { break false; }
                }
            };
            if ended { self.worker_ended(); }
        }
        match &mut self.state {
            State::Warming { .. } => Some(&mut self.partial),
            State::Ready(warmed) => Some(warmed),
            State::Idle | State::Consumed => None,
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

    /// Blocks until an in-flight worker finishes (tests and hidden perf benchmarks only).
    pub(super) fn wait(&mut self) {
        let State::Warming { receiver, .. } = &self.state else { return };
        while let Ok(part) = receiver.recv() {
            self.partial.merge(part);
        }
        self.worker_ended();
        for handle in self.draining.drain(..) {
            handle.join().ok();
        }
    }
}

#[cfg(target_arch = "wasm32")]
#[derive(Default)]
pub(super) struct EffectWarm {
    wanted: PrewarmEffects,
}

#[cfg(target_arch = "wasm32")]
impl EffectWarm {
    pub(super) fn set_wanted(&mut self, wanted: PrewarmEffects) {
        self.wanted = wanted;
    }

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
    cancelled: &dyn Fn() -> bool,
) -> Option<backdrop_blur::WarmedBackdropBlur> {
    // A cancelled worker stops between the build's steps, so that a renderer that drops waits
    // for one of them at most; what it built so far is dropped with it.
    checked_build(device, || {
        let mut blur = backdrop_blur::WarmedBackdropBlur::new(device, bind_layout);
        if cancelled() { return None; }
        blur.ensure_composite(device, format);
        if format != layers::FORMAT && !cancelled() {
            blur.ensure_composite(device, layers::FORMAT);
        }
        Some(blur)
    })
    .ok()
    .flatten()
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
        link,
        rest,
        sender,
        timing,
        #[cfg(test)]
        refuse,
        #[cfg(test)]
        on_step,
        #[cfg(test)]
        on_build,
    } = job;
    let cancelled = || cancel.load(Ordering::Acquire);
    pace::lower();
    // Nothing is compiled while the window still draws: the launch's frames and whatever the
    // app loads behind them come first.
    if !pace::rest(&link, &cancel, rest.0, rest.1) { return; }
    let wall_start = crate::time::now();
    let cpu_start = crate::frame::host_clock::thread_cpu();

    for effect in [PrewarmEffects::ISOLATION, PrewarmEffects::PLUS_LIGHTER, PrewarmEffects::LAYER_BLUR, PrewarmEffects::BACKDROP_BLUR] {
        if !plan.contains(effect) { continue; }
        if cancelled() { break; }
        // A frame that waited for the effect before lent this thread its priority (`pace::lift`).
        pace::lower();
        #[cfg(test)]
        if let Some(hook) = &on_step { hook(); }
        if cancelled() { break; }
        // A frame that needed the effect first builds it itself.
        if !link.start(effect) { continue; }
        #[cfg(test)]
        if let Some(hook) = &on_build { hook(); }
        let mut part = WarmedEffects {
            reported: effect,
            #[cfg(test)]
            refused: refuse,
            ..WarmedEffects::default()
        };
        if effect == PrewarmEffects::ISOLATION {
            part.isolation = Some(build_isolation(
                &device, &layout, &shader, format, vertex_entry,
                #[cfg(test)] refuse,
            ));
        } else if effect == PrewarmEffects::PLUS_LIGHTER {
            part.plus_lighter = Some(build_plus_lighter(
                &device, &layout, &shader, vertex_entry,
                #[cfg(test)] refuse,
            ));
        } else if effect == PrewarmEffects::LAYER_BLUR {
            part.layer_blur = Some(build_layer_blur(
                &device,
                #[cfg(test)] refuse,
            ));
        } else {
            part.backdrop_blur = build_backdrop_blur(&device, &bind_layout, format, &cancelled);
        }
        // Handed over at once: a frame that waits for this effect waits no longer than its build.
        if cancelled() || sender.send(part).is_err() { break; }
    }
    let wall_ms = wall_start.elapsed().as_secs_f64() * 1e3;
    let cpu_ms = cpu_start
        .zip(crate::frame::host_clock::thread_cpu())
        .and_then(|(a, b)| b.checked_sub(a))
        .and_then(crate::frame::host_clock::cpu_ns)
        .map_or(0.0, |ns| ns / 1e6);
    *timing.lock().unwrap_or_else(|e| e.into_inner()) = Some((wall_ms, cpu_ms));
}

impl Renderer {
    /// Starts the background worker that compiles the named, still-unbuilt effect pipelines
    /// ([`PrewarmEffects`]) once the window rests.
    ///
    /// Called only after the first frame finishes so startup latency is untouched; subsequent
    /// calls are an O(1) no-op until a `Classic -> Windfoil` shader swap resets the state machine.
    #[cfg(all(test, not(target_arch = "wasm32")))]
    pub(crate) fn has_active_effect_warm_worker(&self) -> bool {
        matches!(self.effect_warm.state, State::Warming { .. })
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn warm_effects(&mut self, device: &wgpu::Device) {
        if self.gpu_host.as_ref().is_some_and(super::gpu_loss::GpuHost::is_lost_or_terminal_failed) {
            return;
        }
        let wanted = self.effect_warm.wanted;
        if wanted.is_empty() {
            return;
        }
        let plan = wanted.without(self.built_effects());
        let effects = step(&mut self.effect_warm.state, Event::FirstFrameFinished { has_work: !plan.is_empty() });
        if !self.effect_warm.apply_effects(effects) {
            return;
        }
        let cancel = Arc::new(AtomicBool::new(false));
        let link = Arc::new(pace::Link::default());
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
            link: link.clone(),
            rest: self.effect_warm.rest,
            sender,
            timing: self.effect_warm.timing.clone(),
            #[cfg(test)]
            refuse: self.breakers.refuse,
            #[cfg(test)]
            on_step: self.effect_warm.on_step.clone(),
            #[cfg(test)]
            on_build: self.effect_warm.on_build.clone(),
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
        if matches!(self.effect_warm.state, State::Warming { .. }) {
            self.effect_warm.link = Some(link);
        }
    }

    #[cfg(target_arch = "wasm32")]
    pub(crate) fn warm_effects(&mut self, _device: &wgpu::Device) {}

    /// The effects whose pipelines this renderer has built (or tried to), by any path.
    #[cfg(not(target_arch = "wasm32"))]
    fn built_effects(&self) -> PrewarmEffects {
        let mut built = PrewarmEffects::NONE;
        for feature in [breaker::Feature::Isolation, breaker::Feature::PlusLighter, breaker::Feature::LayerBlur] {
            if !matches!(self.breakers.health(feature), breaker::Health::Untried) { built |= effect_of(feature); }
        }
        if self.backdrop_blur.is_some() { built |= PrewarmEffects::BACKDROP_BLUR; }
        built
    }

    /// Called once per prepared frame: tells an in-flight worker the window still draws,
    /// non-blockingly takes in what it has handed over, drops any warmed pipelines whose feature
    /// was already compiled synchronously, and reaps finished worker handles.
    #[cfg(not(target_arch = "wasm32"))]
    pub(super) fn poll_effect_warm(&mut self) {
        if let Some(link) = &self.effect_warm.link { link.frame(); }
        self.effect_warm.poll();
        let built = self.built_effects();
        match &mut self.effect_warm.state {
            State::Warming { .. } => self.effect_warm.partial.prune(built),
            State::Ready(warmed) => warmed.prune(built),
            State::Idle | State::Consumed => {}
        }
        self.effect_warm.finish_if_empty();
    }

    #[cfg(target_arch = "wasm32")]
    pub(super) fn poll_effect_warm(&mut self) {}

    /// Harvests a warmed feature pipeline for `breaker`, returning its error-scope verdict if the
    /// worker built it (waiting for the rest of a build it is in the middle of,
    /// `EffectWarm::warmed_for`), or `None` to fall back to synchronous compilation.
    #[cfg(not(target_arch = "wasm32"))]
    pub(super) fn harvest_warmed_feature(
        &mut self,
        device: &wgpu::Device,
        feature: breaker::Feature,
    ) -> Option<breaker::Verdict> {
        #[cfg(test)]
        let refuse = self.breakers.refuse;
        let warmed = self.effect_warm.warmed_for(effect_of(feature))?;
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

    /// Harvests pre-compiled `BackdropBlur` pipelines if the worker built them (waiting for the
    /// rest of a build it is in the middle of, `EffectWarm::warmed_for`).
    #[cfg(not(target_arch = "wasm32"))]
    pub(super) fn harvest_warmed_backdrop_blur(&mut self) -> Option<backdrop_blur::WarmedBackdropBlur> {
        let warmed = self.effect_warm.warmed_for(PrewarmEffects::BACKDROP_BLUR)?;
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

    /// Names the optional effect GPU pipelines this renderer compiles in the background before
    /// their first use ([`PrewarmEffects`]): isolated layers (`Rgba16Float`),
    /// `Blend::PlusLighter`, Gaussian layer blur, and `El::backdrop_blur`. `true` names all four.
    ///
    /// Defaults to none, so a renderer pays no background compilation CPU, driver memory, or
    /// worker thread for effects it never draws. On native targets, the named pipelines that
    /// are still unbuilt compile once on a low-priority worker thread after the first frame
    /// completes and the window has drawn no frame for 300 ms (1 s at most; one-shot
    /// `render_runtime_to_offscreen` calls never start it). A frame that draws a named effect
    /// before the worker began it compiles it itself, once; a frame that meets the worker in the
    /// middle of it waits for the rest of that compile. Ignored on `wasm32`, which always
    /// compiles lazily on first use.
    ///
    /// # Example
    ///
    /// ```no_run
    /// use rune::PrewarmEffects;
    /// use rune::render::{Gpu, OFFSCREEN_FORMAT, Renderer};
    ///
    /// let gpu = Gpu::headless();
    /// let mut renderer = Renderer::new(&gpu.device, OFFSCREEN_FORMAT);
    /// renderer.set_prewarm_effects(PrewarmEffects::BACKDROP_BLUR);
    /// assert_eq!(renderer.prewarm_effects(), PrewarmEffects::BACKDROP_BLUR);
    /// ```
    pub fn set_prewarm_effects(&mut self, effects: impl Into<PrewarmEffects>) {
        self.effect_warm.set_wanted(effects.into());
    }

    /// The effects this renderer compiles ahead of their first use
    /// ([`Self::set_prewarm_effects`]); none by default.
    pub fn prewarm_effects(&self) -> PrewarmEffects {
        self.effect_warm.wanted
    }

    /// Hidden benchmark hook: triggers post-first-frame effect warming explicitly.
    #[doc(hidden)]
    #[cfg(not(target_arch = "wasm32"))]
    pub fn bench_trigger_effect_warm(&mut self, device: &wgpu::Device) {
        self.warm_effects(device);
    }

    /// Hidden benchmark hook: waits for any in-flight effect warming worker to finish.
    #[doc(hidden)]
    #[cfg(not(target_arch = "wasm32"))]
    pub fn bench_wait_effect_warm(&mut self) {
        self.effect_warm.wait();
    }

    /// Hidden benchmark hook: waits for any in-flight pinned-pipeline worker to finish.
    #[doc(hidden)]
    #[cfg(not(target_arch = "wasm32"))]
    pub fn bench_wait_pinned(&mut self) {
        self.wait_pinned();
    }

    /// Hidden benchmark hook: returns a shared handle holding the worker's `(wall_ms, cpu_ms)`.
    #[doc(hidden)]
    #[cfg(not(target_arch = "wasm32"))]
    pub fn bench_effect_warm_timing(&self) -> Arc<Mutex<Option<(f64, f64)>>> {
        self.effect_warm.timing.clone()
    }

    /// Hidden benchmark hook: calibrates the Windows thread-cycle clock before measuring worker CPU.
    #[doc(hidden)]
    #[cfg(not(target_arch = "wasm32"))]
    pub fn bench_calibrate_cpu_clock() {
        #[cfg(windows)]
        let _ = crate::frame::host_clock::cycles_per_ns_waiting();
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
#[path = "effect_warm_tests.rs"]
mod tests;
