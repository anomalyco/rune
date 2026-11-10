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

/// The renderer's breakers, one per lazily built feature.
#[derive(Default)]
pub(super) struct Breakers {
    seq: Seq,
    isolation: Health,
    plus_lighter: Health,
    layer_blur: Health,
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
        for feature in [Feature::Isolation, Feature::PlusLighter] { self.run_breaker(device, feature, Event::ShaderChanged); }
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
