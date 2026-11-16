//! Circuit breakers for the pipelines a renderer builds the first time a frame needs them: an
//! isolated layer's (its `Rgba16Float` target's and the composites), `Blend::PlusLighter`'s, and
//! the layer blur's. A pipeline the device refused poisons the command encoder that sets it,
//! and with it the whole frame, so each is built inside error scopes and used only once the
//! device has taken it. One it refused is dropped and not built again, and frames draw without
//! it: an isolated group straight into its parent, a dissolve as source-over, a blur unblurred.
//!
//! A native device answers at once. A browser's error scopes answer a little later, so on the
//! web a frame or two draw that fallback while the answer comes (`Renderer::checking_gpu`).

use std::sync::{Arc, Mutex};
use crate::machine::{Attempt, Effects, Seq};
use super::{Blend, Renderer, layer_blur, layers::FORMAT};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Feature {
    /// Isolated layers: the uber pipeline for their `Rgba16Float` targets and the composites.
    Isolation,
    /// `Blend::PlusLighter` layers' pipelines.
    PlusLighter,
    /// The layer blur's pipelines.
    LayerBlur,
}

/// A browser's answer to a check, once it comes: the error it caught, or none.
pub(super) type Answer = Arc<Mutex<Option<Option<String>>>>;

/// What the device said about a build.
pub(super) enum Verdict {
    /// It answered at once: the error it caught, or none.
    Now(Option<String>),
    /// It will answer here.
    #[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
    Later(Answer),
}

#[derive(Default)]
pub(super) enum Health {
    /// Not built.
    #[default]
    Untried,
    /// Built; waiting on the browser to say whether the device took it.
    Checking { attempt: Attempt, answer: Answer },
    /// The device took it.
    Working,
    /// The device refused it: dropped, and not built again for this shader.
    Broken,
}

pub(super) enum Event {
    /// A frame needs it.
    Wanted,
    /// It was built, with what the device said.
    Built(Verdict),
    /// The browser answered the check `attempt`.
    Answered { attempt: Attempt, error: Option<String> },
    /// The uber shader changed (`variant::Uber`); what was built from the old one goes.
    ShaderChanged,
}

#[derive(Debug, PartialEq)]
pub(super) enum Effect {
    /// Build it in error scopes and send `Built`.
    Build,
    /// Drop what was built.
    Drop,
    /// Say once that it can't draw here, and why.
    Report(String),
}

impl Health {
    pub(super) fn step(&mut self, seq: &mut Seq, event: Event) -> Effects<Effect> {
        match (&*self, event) {
            (Health::Untried, Event::Wanted) => Effects::one(Effect::Build),
            (Health::Untried, Event::Built(Verdict::Later(answer))) => {
                *self = Health::Checking { attempt: seq.next(), answer };
                Effects::none()
            }
            (Health::Untried, Event::Built(Verdict::Now(error))) => self.settle(error),
            (Health::Checking { attempt, .. }, Event::Answered { attempt: answered, error }) if *attempt == answered => self.settle(error),
            (Health::Untried, Event::ShaderChanged) => Effects::none(),
            (_, Event::ShaderChanged) => {
                *self = Health::Untried;
                Effects::one(Effect::Drop)
            }
            _ => Effects::none(),
        }
    }

    fn settle(&mut self, error: Option<String>) -> Effects<Effect> {
        match error {
            None => { *self = Health::Working; Effects::none() }
            Some(why) => { *self = Health::Broken; Effects::two(Effect::Drop, Effect::Report(why)) }
        }
    }

    pub(super) fn working(&self) -> bool { matches!(self, Health::Working) }

    /// The browser's answer to the check under way, if it has come.
    fn answered(&self) -> Option<Event> {
        let Health::Checking { attempt, answer } = self else { return None };
        let error = answer.lock().unwrap_or_else(|e| e.into_inner()).take()?;
        Some(Event::Answered { attempt: *attempt, error })
    }
}

/// What a renderer builds on first use, compiled on a thread before any frame wants it. A driver
/// compiles each new pipeline: on Iris Xe with an empty shader cache the uber shader for a layer
/// target took 337 ms, which the first frame with a layer (a card entering a list's fade) paid.
#[derive(Default)]
enum Warm {
    #[default]
    Idle,
    #[cfg_attr(any(test, target_arch = "wasm32"), allow(dead_code))]
    Building(std::sync::mpsc::Receiver<Warmed>),
    /// Taken, or not to be made for this shader.
    Done,
}

/// Spare layer targets for faded lists, made on a thread the first time a frame draws a soft
/// edge: on the Iris each new target cost 0.3–1.2 ms (a texture, two buffers, two bind groups),
/// once per chip or card the first time it crossed a fade (the palette's key 1).
#[derive(Default)]
enum Pool {
    #[default]
    Unasked,
    #[cfg_attr(any(test, target_arch = "wasm32"), allow(dead_code))]
    Building(std::sync::mpsc::Receiver<Vec<super::layers::Target>>),
    Done,
}

/// The fade pool in device pixels, as many of each as a fade band holds at once: buckets a chip or a
/// keycap asks for, and list-wide ones for cards (the fading list's width). The planner takes a
/// spare no more than twice a layer's area, so each is a bucket the layers it serves ask for.
const POOL_SMALL: [([u32; 2], usize); 2] = [([128, 64], 6), ([256, 64], 2)];
const POOL_WIDE: (u32, usize) = (128, 2);

/// Renderers whose isolation pipelines a thread is still compiling.
static WARMING: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

/// Whether a thread is still compiling isolation pipelines. Meanwhile paint takes a group's
/// opacity, and a list fade over a stack, on each paint instead of in a layer, so no frame waits
/// for them; the frame after they're built draws the layers.
pub(crate) fn isolation_pending() -> bool {
    WARMING.load(std::sync::atomic::Ordering::Relaxed) > 0
}

/// What the warming thread built, all of which the device took: the uber pipeline for layer
/// targets, the composites into the window's format and into layer targets, and the backdrop blur.
#[cfg_attr(any(test, target_arch = "wasm32"), allow(dead_code))]
struct Warmed {
    isolation: Option<(wgpu::RenderPipeline, [(wgpu::TextureFormat, wgpu::RenderPipeline); 2])>,
    backdrop: Option<super::backdrop_blur::BackdropBlur>,
}

/// The renderer's breakers, one per lazily built feature.
#[derive(Default)]
pub(super) struct Breakers {
    seq: Seq,
    isolation: Health,
    plus_lighter: Health,
    layer_blur: Health,
    /// What the warming thread builds ahead of first use (`Renderer::warm_first_use`).
    warm: Warm,
    /// Default UI frames prepared, up to the one that starts `warm`.
    ui_frames: u8,
    /// Layer targets made ahead for faded lists (`Renderer::warm_fade_pool`).
    pool: Pool,
    /// Tests: build this feature's pipelines broken, as a device that refuses them would.
    #[cfg(test)]
    pub refuse: Option<Feature>,
}

impl Breakers {
    pub(super) fn health(&self, feature: Feature) -> &Health {
        match feature {
            Feature::Isolation => &self.isolation,
            Feature::PlusLighter => &self.plus_lighter,
            Feature::LayerBlur => &self.layer_blur,
        }
    }

    fn step(&mut self, feature: Feature, event: Event) -> Effects<Effect> {
        let health = match feature {
            Feature::Isolation => &mut self.isolation,
            Feature::PlusLighter => &mut self.plus_lighter,
            Feature::LayerBlur => &mut self.layer_blur,
        };
        health.step(&mut self.seq, event)
    }

    /// Whether a check is waiting on the browser, which a later frame picks up.
    pub(super) fn checking(&self) -> bool {
        [&self.isolation, &self.plus_lighter, &self.layer_blur].iter().any(|h| matches!(h, Health::Checking { .. }))
    }
}

const FEATURES: [Feature; 3] = [Feature::Isolation, Feature::PlusLighter, Feature::LayerBlur];

impl Renderer {
    /// Whether a frame drew a fallback while the browser checks a pipeline; draw another soon, and
    /// it draws with the pipeline if the device took it.
    #[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
    pub(crate) fn checking_gpu(&self) -> bool { self.breakers.checking() }

    /// Takes the answers that came since the last frame, then builds what `wanted` needs.
    pub(super) fn check_features(&mut self, device: &wgpu::Device, wanted: impl Fn(Feature) -> bool) {
        for feature in FEATURES {
            if let Some(answer) = self.breakers.health(feature).answered() { self.run_breaker(device, feature, answer); }
            if wanted(feature) { self.run_breaker(device, feature, Event::Wanted); }
        }
    }

    /// The uber shader changed: the features built from it are built again, and checked again,
    /// when a frame next needs them.
    pub(super) fn shader_changed(&mut self, device: &wgpu::Device) {
        // What a thread is building is from the old shader: let it go, and warm the new one.
        self.breakers.warm = Warm::Idle;
        for feature in [Feature::Isolation, Feature::PlusLighter] { self.run_breaker(device, feature, Event::ShaderChanged); }
    }

    /// A default UI frame: from the second on, what a renderer otherwise builds the first time a
    /// frame needs it compiles on a thread: the isolation pipelines (a list's fade over a card, a
    /// group's opacity) and the backdrop blur (a frosted header over scrolled content). Once per
    /// shader. A frame never waits for it: until it's built, paint draws the layers it can do
    /// without on each paint (`isolation_pending`) and a backdrop blur waits (`backdrop_pending`).
    /// No pipeline is made on a frame that carries input, from the second frame on: something
    /// new a frame builds lazily belongs here.
    pub(super) fn warm_first_use(&mut self, device: &wgpu::Device, instances: &[super::Instance], viewport: (u32, u32)) {
        self.poll_warm(device);
        self.warm_fade_pool(device, instances, viewport);
        if self.breakers.ui_frames < 2 { self.breakers.ui_frames += 1; return; }
        if !matches!(self.breakers.warm, Warm::Idle) { return; }
        self.breakers.warm = Warm::Done;
        // What an earlier frame already built (a layer or a blur on screen at launch) is left.
        let isolation = matches!(self.breakers.isolation, Health::Untried);
        let backdrop = self.backdrop_blur.is_none();
        if !isolation && !backdrop { return; }
        // Tests share one process: a thread warming for one would change what paint isolates in
        // the others (`isolation_pending`), so they build on the frame as before.
        #[cfg(not(any(test, target_arch = "wasm32")))]
        {
            let (device, layout, shader, entry, window) = (device.clone(), self.layout.clone(), self.shader.clone(), self.vertex_entry, self.format);
            let bind_layout = self.bind_layout.clone();
            let wake = self.frame_wake.clone();
            let (sent, done) = std::sync::mpsc::channel();
            if isolation { WARMING.fetch_add(1, std::sync::atomic::Ordering::Relaxed); }
            let spawned = std::thread::Builder::new().name("rune-first-use-pipelines".into()).spawn(move || {
                // Measuring builds: `RUNE_SLOW_ISOLATION_MS` stands in for a driver compiling
                // with an empty shader cache, to show the frames before the pipelines exist.
                #[cfg(any(debug_assertions, feature = "frame-profile"))]
                if let Some(ms) = std::env::var("RUNE_SLOW_ISOLATION_MS").ok().and_then(|v| v.parse().ok()) {
                    std::thread::sleep(std::time::Duration::from_millis(ms));
                }
                let mut built = Warmed { isolation: None, backdrop: None };
                let verdict = checked(&device, || {
                    let pipeline = |format, entry| Self::pipeline(&device, &layout, &shader, format, entry, Blend::SourceOver);
                    if isolation {
                        built.isolation = Some((pipeline(FORMAT, entry), [(window, pipeline(window, "vs")), (FORMAT, pipeline(FORMAT, "vs"))]));
                    }
                    if backdrop {
                        let mut blur = super::backdrop_blur::BackdropBlur::new(&device, &bind_layout);
                        blur.ensure_composite(&device, window);
                        blur.ensure_composite(&device, FORMAT);
                        built.backdrop = Some(blur);
                    }
                });
                if let Verdict::Now(None) = verdict { sent.send(built).ok(); }
                if isolation { WARMING.fetch_sub(1, std::sync::atomic::Ordering::Relaxed); }
                // A window resting with what it drew without them draws with them now.
                if let Some(wake) = wake { wake.notify(); }
            });
            match spawned {
                Ok(_) => self.breakers.warm = Warm::Building(done),
                Err(_) => if isolation { WARMING.fetch_sub(1, std::sync::atomic::Ordering::Relaxed); },
            }
        }
    }

    /// The first frame that draws a soft edge (a list's fade) asks a thread for the fade pool;
    /// a later frame takes it as spare targets, so the first chip or card to cross the fade makes
    /// none of its own.
    fn warm_fade_pool(&mut self, device: &wgpu::Device, instances: &[super::Instance], _viewport: (u32, u32)) {
        if let Pool::Building(done) = &self.breakers.pool {
            match done.try_recv() {
                Ok(targets) => { self.breakers.pool = Pool::Done; self.adopt_spares(targets); }
                Err(std::sync::mpsc::TryRecvError::Empty) => {}
                Err(std::sync::mpsc::TryRecvError::Disconnected) => self.breakers.pool = Pool::Done,
            }
            return;
        }
        if !matches!(self.breakers.pool, Pool::Unasked) { return; }
        // A fade's widths are the first two words (`pack_fade`); its clip is the list it fades.
        let Some(list) = instances.iter().find(|i| i.fade[0] != 0 || i.fade[1] != 0).map(|i| i.clip) else { return };
        self.breakers.pool = Pool::Done;
        // As `warm_first_use`: a thread in one test would change what the others plan.
        #[cfg(not(any(test, target_arch = "wasm32")))]
        {
            let max = self.max_texture_dim;
            let width = (list[2] - list[0]).max(0.0) as u32;
            let wide = [width.div_ceil(64).saturating_mul(64).clamp(64, max), POOL_WIDE.0];
            let sizes: Vec<[u32; 2]> = POOL_SMALL.iter().flat_map(|&(size, n)| std::iter::repeat_n(size, n)).chain(std::iter::repeat_n(wide, POOL_WIDE.1)).collect();
            let (device, layout, globals, outlines, atlas, binding) = (device.clone(), self.bind_layout.clone(), self.globals.clone(),
                self.uber.outlines().cloned(), self.atlas.clone(), self.layer_bindings());
            let wake = self.frame_wake.clone();
            let (sent, done) = std::sync::mpsc::channel();
            let spawned = std::thread::Builder::new().name("rune-fade-pool".into()).spawn(move || {
                let bindings = (&layout, &globals, outlines.as_ref(), &atlas);
                let targets = sizes.into_iter().map(|size| super::layers::new_target(&device, bindings, size, 0, binding)).collect();
                sent.send(targets).ok();
                if let Some(wake) = wake { wake.notify(); }
            });
            if spawned.is_ok() { self.breakers.pool = Pool::Building(done); }
        }
    }

    /// Puts what the warming thread built where a frame would have built it, once it's done.
    /// Anything the device refused there is left for a frame to build and check as before.
    fn poll_warm(&mut self, device: &wgpu::Device) {
        let Warm::Building(done) = &self.breakers.warm else { return };
        let warmed = match done.try_recv() {
            Ok(warmed) => warmed,
            Err(std::sync::mpsc::TryRecvError::Empty) => return,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => { self.breakers.warm = Warm::Done; return; }
        };
        self.breakers.warm = Warm::Done;
        if let Some((layer, composites)) = warmed.isolation && matches!(self.breakers.isolation, Health::Untried) {
            if !self.extra.iter().any(|(f, _)| *f == FORMAT) { self.extra.push((FORMAT, layer)); }
            for (format, pipeline) in composites {
                if !self.layers.pipelines.iter().any(|(f, _)| *f == format) { self.layers.pipelines.push((format, pipeline)); }
            }
        }
        if let Some(blur) = warmed.backdrop && self.backdrop_blur.is_none() { self.adopt_backdrop_blur(device, blur); }
    }

    /// Whether the backdrop blur is still compiling on the warming thread: a frame draws without
    /// it rather than building it.
    pub(super) fn backdrop_pending(&self) -> bool {
        matches!(self.breakers.warm, Warm::Building(_))
    }
    /// Steps `feature`'s breaker on `event` and carries out what it asks, until nothing is left.
    fn run_breaker(&mut self, device: &wgpu::Device, feature: Feature, event: Event) {
        let mut queue: std::collections::VecDeque<Effect> = self.breakers.step(feature, event).into_iter().collect();
        while let Some(effect) = queue.pop_front() {
            match effect {
                Effect::Build => {
                    let verdict = checked(device, || self.build_feature(device, feature));
                    queue.extend(self.breakers.step(feature, Event::Built(verdict)));
                }
                Effect::Drop => self.drop_feature(feature),
                Effect::Report(why) => report(device, feature, &why),
            }
        }
    }

    fn build_feature(&mut self, device: &wgpu::Device, feature: Feature) {
        #[cfg(test)]
        let format = if self.breakers.refuse == Some(feature) { wgpu::TextureFormat::Rgba32Uint } else { FORMAT };
        #[cfg(not(test))]
        let format = FORMAT;
        match feature {
            Feature::Isolation => {
                if !self.extra.iter().any(|(f, _)| *f == FORMAT) {
                    let pipeline = Self::pipeline(device, &self.layout, &self.shader, format, self.vertex_entry, Blend::SourceOver);
                    self.extra.push((FORMAT, pipeline));
                }
                let formats: Vec<_> = [self.format].into_iter().chain(self.extra.iter().map(|(f, _)| *f)).collect();
                for format in formats { self.add_layer_format(device, format); }
            }
            Feature::PlusLighter => {
                self.layers.additive = Some((
                    Self::pipeline(device, &self.layout, &self.shader, format, self.vertex_entry, Blend::PlusLighter),
                    Self::pipeline(device, &self.layout, &self.shader, format, "vs", Blend::PlusLighter)));
            }
            Feature::LayerBlur => self.layers.blur = Some(layer_blur::Pipeline::new(device, format)),
        }
    }

    fn drop_feature(&mut self, feature: Feature) {
        match feature {
            Feature::Isolation => {
                self.extra.retain(|(f, _)| *f != FORMAT);
                let broken = matches!(self.breakers.isolation, Health::Broken);
                self.drop_layer_pipelines(broken);
            }
            Feature::PlusLighter => self.layers.additive = None,
            Feature::LayerBlur => self.drop_layer_blur(),
        }
    }
}

/// Runs `build` inside error scopes for validation, out-of-memory and internal errors.
#[cfg(not(target_arch = "wasm32"))]
fn checked(device: &wgpu::Device, build: impl FnOnce()) -> Verdict {
    let validation = device.push_error_scope(wgpu::ErrorFilter::Validation);
    let memory = device.push_error_scope(wgpu::ErrorFilter::OutOfMemory);
    let internal = device.push_error_scope(wgpu::ErrorFilter::Internal);
    build();
    let errors = [internal.pop(), memory.pop(), validation.pop()].map(pollster::block_on);
    Verdict::Now(errors.into_iter().flatten().next().map(|e| e.to_string()))
}

/// Runs `build` inside the browser's error scopes, which answer later. wgpu's own scopes would
/// panic on an internal error (`platform::web::health`).
#[cfg(target_arch = "wasm32")]
fn checked(device: &wgpu::Device, build: impl FnOnce()) -> Verdict {
    let Some(scopes) = crate::platform::web::health::Scopes::push(device) else {
        build();
        return Verdict::Now(None);
    };
    build();
    let answer = Answer::default();
    let to = answer.clone();
    scopes.pop(move |error| *to.lock().unwrap_or_else(|e| e.into_inner()) = Some(error));
    Verdict::Later(answer)
}

fn report(device: &wgpu::Device, feature: Feature, why: &str) {
    let fallback = match feature {
        Feature::Isolation => "isolated layers draw straight into their parent, without their opacity, blur, clip or turn",
        Feature::PlusLighter => "plus-lighter layers draw source-over",
        Feature::LayerBlur => "layers draw unblurred",
    };
    #[cfg(target_arch = "wasm32")]
    web_sys::console::error_1(&format!("rune: a pipeline the GPU refused ({why}) on {}; {fallback}", crate::platform::web::health::summary(device)).into());
    #[cfg(not(target_arch = "wasm32"))]
    {
        let _ = device;
        eprintln!("rune: a pipeline the GPU refused ({why}); {fallback}");
    }
}

#[cfg(test)]
#[path = "breaker_tests.rs"]
mod tests;
