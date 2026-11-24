//! Surface-configuring GPU acquisition with adapter fallback (`rune_gpu::fallback`).
//!
//! Picks (or takes a prewarmed) GPU, creates a surface, and runs the caller's configuration/setup
//! closure on it. When device creation or surface configuration fails with a fallback-eligible
//! `rune_gpu::fallback::Failure`, drops the failed surface and device, creates a fresh surface,
//! and walks remaining adapters (other hardware first, software/WARP last; a named adapter is
//! never replaced).
//!
//! Extension points:
//! - A later task treating a rejected last-resort shader as an adapter fallback returns
//!   `SurfaceSetupError` with `Failure::ShaderRejected` from its `setup` closure.
//! - A later task falling back to a plain HWND swap chain when DirectComposition is absent
//!   (Wine/Proton) plugs into `run_fallback_chain` when `Effect::FinishErr` is reached after
//!   `Failure::SurfaceRefused` by switching to an HWND `wgpu::Instance` and re-running `make_surface`.

use super::{Gpu, Renderer, gpu_events, prewarm};

/// Failure from configuring or setting up a surface on a candidate GPU (`Gpu::for_surface_setup`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SurfaceSetupError {
    pub kind: rune_gpu::fallback::Failure,
    pub message: String,
}

impl SurfaceSetupError {
    pub(crate) fn unsupported(message: impl Into<String>) -> Self {
        Self { kind: rune_gpu::fallback::Failure::SurfaceUnsupported, message: message.into() }
    }

    pub(crate) fn from_configure(prefix: &str, error: crate::window::ConfigureError) -> Self {
        let message = if prefix.is_empty() {
            error.message
        } else {
            format!("{prefix}: {}", error.message)
        };
        Self { kind: error.kind.into(), message }
    }
}

impl Gpu {
    /// Picks (or takes a prewarmed) GPU, creates a surface via `make_surface`, and runs `setup`
    /// (typically building a `SurfaceConfiguration` and calling `configure_checked`).
    ///
    /// Launch speed: when the prewarmed GPU or first picked adapter succeeds, no other adapters
    /// are enumerated and `make_surface` is called once.
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn for_surface_setup<'w, T>(
        wanted: wgpu::Features,
        translucent: bool,
        make_surface: impl FnMut(&wgpu::Instance) -> Result<wgpu::Surface<'w>, String>,
        setup: impl FnMut(&Gpu, &wgpu::Surface<'w>) -> Result<T, SurfaceSetupError>,
    ) -> Result<(Gpu, wgpu::Surface<'w>, Option<Renderer>, T), SurfaceSetupError> {
        Self::for_surface_setup_from(prewarm::take(), wanted, translucent, make_surface, setup)
    }

    /// Enters the fallback chain directly after an already-open or pooled GPU failed `setup` with
    /// `Decision::MoveOn`, excluding `excluded` so the failed adapter is not tried again.
    #[cfg(not(target_arch = "wasm32"))]
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn for_surface_setup_excluding<'w, T>(
        instance: wgpu::Instance,
        excluded: crate::gpu::fallback::Candidate,
        first_info: wgpu::AdapterInfo,
        first_error: String,
        wanted: wgpu::Features,
        translucent: bool,
        make_surface: impl FnMut(&wgpu::Instance) -> Result<wgpu::Surface<'w>, String>,
        setup: impl FnMut(&Gpu, &wgpu::Surface<'w>) -> Result<T, SurfaceSetupError>,
    ) -> Result<(Gpu, wgpu::Surface<'w>, T), SurfaceSetupError> {
        let state = crate::gpu::fallback::State::need_fallbacks(first_error);
        let (gpu, surface, value) = Self::run_fallback_chain(
            instance,
            state,
            None,
            vec![excluded],
            first_info,
            crate::gpu::pick::compositor_pci(),
            wanted,
            translucent,
            make_surface,
            |s| Some(s),
            setup,
        )?;
        Self::log_surface_gpu(&gpu);
        Ok((gpu, surface, value))
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn for_surface_setup_from<'w, T>(
        early: Option<(Gpu, Renderer)>,
        wanted: wgpu::Features,
        translucent: bool,
        mut make_surface: impl FnMut(&wgpu::Instance) -> Result<wgpu::Surface<'w>, String>,
        mut setup: impl FnMut(&Gpu, &wgpu::Surface<'w>) -> Result<T, SurfaceSetupError>,
    ) -> Result<(Gpu, wgpu::Surface<'w>, Option<Renderer>, T), SurfaceSetupError> {
        use crate::gpu::fallback::{self, Decision, Failure, State};
        let early = early.filter(|(gpu, _)| !gpu.is_lost() && crate::gpu::backend::serves(&gpu.device, wanted));
        let (instance, initial_surface) = match early {
            Some((early, renderer)) => {
                let surface = make_surface(&early.instance)
                    .map_err(|message| SurfaceSetupError { kind: Failure::SurfaceCreate, message })?;
                if crate::render::translucency::serves(&early, &surface, wanted, translucent) {
                    match setup(&early, &surface) {
                        Ok(value) => {
                            Self::log_surface_gpu(&early);
                            return Ok((early, surface, Some(renderer), value));
                        }
                        Err(err) => {
                            if fallback::decide(crate::gpu::pick::env_policy(), err.kind) == Decision::Stop {
                                drop((surface, renderer, early));
                                return Err(err);
                            }
                            let first_info = early.adapter.get_info();
                            let first_candidate = Self::candidate_info(0, &early.adapter, Some(&surface));
                            let instance = early.instance.clone();
                            drop((surface, renderer, early));
                            gpu_events::starting();
                            let (gpu, surface, value) = Self::run_fallback_chain(
                                instance,
                                State::need_fallbacks(err.message),
                                None,
                                vec![first_candidate],
                                first_info,
                                crate::gpu::pick::compositor_pci(),
                                wanted,
                                translucent,
                                make_surface,
                                |s| Some(s),
                                setup,
                            )?;
                            Self::log_surface_gpu(&gpu);
                            return Ok((gpu, surface, None, value));
                        }
                    }
                }
                let instance = early.instance.clone();
                drop((renderer, early));
                (instance, surface)
            }
            None => {
                let instance = Self::instance_for(wanted);
                let surface = make_surface(&instance)
                    .map_err(|message| SurfaceSetupError { kind: Failure::SurfaceCreate, message })?;
                (instance, surface)
            }
        };
        gpu_events::starting();
        let (gpu, surface, value) = Self::try_setup_on(
            instance,
            initial_surface,
            None,
            wanted,
            translucent,
            make_surface,
            |s| Some(s),
            setup,
        )?;
        Self::log_surface_gpu(&gpu);
        Ok((gpu, surface, None, value))
    }

    /// Runs the first adapter pick + `setup`, and on a `MoveOn` failure walks the fallback chain
    /// through `rune_gpu::fallback::step`. Parameterized over the surface handle `S` so offscreen
    /// GPU tests can exercise the real multi-adapter chain without opening a native window.
    #[cfg(not(target_arch = "wasm32"))]
    #[allow(clippy::too_many_arguments)]
    pub(super) fn try_setup_on<S, T>(
        instance: wgpu::Instance,
        surface: S,
        policy_override: Option<crate::gpu::fallback::Policy>,
        wanted: wgpu::Features,
        translucent: bool,
        make_surface: impl FnMut(&wgpu::Instance) -> Result<S, String>,
        surface_ref: impl Fn(&S) -> Option<&wgpu::Surface<'_>>,
        mut setup: impl FnMut(&Gpu, &S) -> Result<T, SurfaceSetupError>,
    ) -> Result<(Gpu, S, T), SurfaceSetupError> {
        use crate::gpu::fallback::{self, Effect, Event, Failure, State};
        let (adapter, policy, compositor) = Self::pick_first_adapter(
            &instance,
            surface_ref(&surface),
            policy_override,
            translucent,
        )?;
        let first_info = adapter.get_info();
        let first_candidate = Self::candidate_info(0, &adapter, surface_ref(&surface));
        let mut state = State::first(policy);
        let made = Self::create_device_serialized(&adapter, wanted);
        let (fail_kind, fail_msg, kept_surface) = match made {
            Ok((device, queue)) => {
                crate::marks::mark("gpu.device");
                let draws_surfaces = surface_ref(&surface).is_some();
                let gpu = Gpu::made(instance.clone(), adapter, device, queue, draws_surfaces);
                match setup(&gpu, &surface) {
                    Ok(value) => {
                        let _ = fallback::step(&mut state, Event::Succeeded);
                        crate::native_surface::note_device(&gpu.device, draws_surfaces);
                        return Ok((gpu, surface, value));
                    }
                    Err(err) => {
                        // Drop the failed surface and device before trying the next adapter so any
                        // partial DirectComposition target or swap chain is released first.
                        drop((surface, gpu));
                        (err.kind, err.message, None)
                    }
                }
            }
            Err(error) => (Failure::DeviceCreate, error, Some(surface)),
        };
        let effects = fallback::step(&mut state, Event::Failed { kind: fail_kind, error: fail_msg });
        match effects.into_iter().next() {
            Some(Effect::EnumerateFallbacks) => Self::run_fallback_chain(
                instance,
                state,
                kept_surface,
                vec![first_candidate],
                first_info,
                compositor,
                wanted,
                translucent,
                make_surface,
                surface_ref,
                setup,
            ),
            Some(Effect::FinishErr(message)) => Err(SurfaceSetupError { kind: fail_kind, message }),
            other => unreachable!("unexpected fallback effect after first failure: {other:?}"),
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[allow(clippy::too_many_arguments)]
    fn run_fallback_chain<S, T>(
        instance: wgpu::Instance,
        mut state: crate::gpu::fallback::State,
        mut pending_surface: Option<S>,
        tried: Vec<crate::gpu::fallback::Candidate>,
        first_info: wgpu::AdapterInfo,
        compositor: Option<String>,
        wanted: wgpu::Features,
        translucent: bool,
        mut make_surface: impl FnMut(&wgpu::Instance) -> Result<S, String>,
        surface_ref: impl Fn(&S) -> Option<&wgpu::Surface<'_>>,
        mut setup: impl FnMut(&Gpu, &S) -> Result<T, SurfaceSetupError>,
    ) -> Result<(Gpu, S, T), SurfaceSetupError> {
        use crate::gpu::fallback::{self, Candidate, Effect, Event, Failure};
        let surface = match pending_surface.take() {
            Some(surface) => surface,
            None => make_surface(&instance)
                .map_err(|message| SurfaceSetupError { kind: Failure::SurfaceCreate, message })?,
        };
        let adapters = Self::enumerate_adapters_serialized(&instance);
        let candidates: Vec<Candidate> = adapters
            .iter()
            .enumerate()
            .map(|(i, adapter)| Self::candidate_info(i, adapter, surface_ref(&surface)))
            .collect();
        let ordered = fallback::order_fallbacks(
            &tried,
            &candidates,
            &crate::gpu::pick::display_devices(),
            compositor.as_deref(),
            translucent,
        );
        pending_surface = Some(surface);
        let mut event = Event::Candidates(ordered);
        let mut last_kind = Failure::SurfaceRefused;
        loop {
            let mut next_event = None;
            for effect in fallback::step(&mut state, event) {
                match effect {
                    Effect::TryCandidate(index) => {
                        let surface = match pending_surface.take() {
                            Some(surface) => surface,
                            None => match make_surface(&instance) {
                                Ok(surface) => surface,
                                Err(message) => {
                                    last_kind = Failure::SurfaceCreate;
                                    next_event = Some(Event::Failed { kind: Failure::SurfaceCreate, error: message });
                                    continue;
                                }
                            },
                        };
                        let candidate = &adapters[index];
                        match Self::create_device_serialized(candidate, wanted) {
                            Ok((device, queue)) => {
                                let draws_surfaces = surface_ref(&surface).is_some();
                                let gpu = Gpu::made(instance.clone(), candidate.clone(), device, queue, draws_surfaces);
                                match setup(&gpu, &surface) {
                                    Ok(value) => {
                                        let first_error = state.first_error().unwrap_or("").to_owned();
                                        let _ = fallback::step(&mut state, Event::Succeeded);
                                        let info = gpu.adapter.get_info();
                                        eprintln!(
                                            "rune: {} ({:?}) could not draw this surface ({first_error}); falling back to {} ({:?}, {:?})",
                                            first_info.name, first_info.backend, info.name, info.backend, info.device_type
                                        );
                                        crate::native_surface::note_device(&gpu.device, draws_surfaces);
                                        return Ok((gpu, surface, value));
                                    }
                                    Err(err) => {
                                        last_kind = err.kind;
                                        drop((surface, gpu));
                                        next_event = Some(Event::Failed { kind: err.kind, error: err.message });
                                    }
                                }
                            }
                            Err(error) => {
                                last_kind = Failure::DeviceCreate;
                                pending_surface = Some(surface);
                                next_event = Some(Event::Failed { kind: Failure::DeviceCreate, error });
                            }
                        }
                    }
                    Effect::FinishErr(message) => return Err(SurfaceSetupError { kind: last_kind, message }),
                    Effect::EnumerateFallbacks | Effect::FinishOk { .. } => {
                        unreachable!("unexpected fallback effect in run_fallback_chain")
                    }
                }
            }
            event = next_event.expect("fallback state machine must emit an effect");
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn log_surface_gpu(gpu: &Gpu) {
        if std::env::var_os("RUNE_GPU_LOG").is_some_and(|v| v != "0") {
            eprintln!("rune: a surface draws with {}", crate::gpu::backend::describe(&gpu.device));
        }
    }

    #[cfg(target_arch = "wasm32")]
    #[allow(dead_code)]
    pub(crate) fn for_surface_setup<'w, T>(
        _: wgpu::Features,
        _: bool,
        _: impl FnMut(&wgpu::Instance) -> Result<wgpu::Surface<'w>, String>,
        _: impl FnMut(&Gpu, &wgpu::Surface<'w>) -> Result<T, SurfaceSetupError>,
    ) -> Result<(Gpu, wgpu::Surface<'w>, Option<Renderer>, T), SurfaceSetupError> {
        Err(SurfaceSetupError {
            kind: rune_gpu::fallback::Failure::DeviceCreate,
            message: "synchronous GPU initialization is unsupported on web; use App::run".into(),
        })
    }

    #[cfg(target_arch = "wasm32")]
    #[allow(dead_code)]
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn for_surface_setup_excluding<'w, T>(
        _: wgpu::Instance,
        _: crate::gpu::fallback::Candidate,
        _: wgpu::AdapterInfo,
        message: String,
        _: wgpu::Features,
        _: bool,
        _: impl FnMut(&wgpu::Instance) -> Result<wgpu::Surface<'w>, String>,
        _: impl FnMut(&Gpu, &wgpu::Surface<'w>) -> Result<T, SurfaceSetupError>,
    ) -> Result<(Gpu, wgpu::Surface<'w>, T), SurfaceSetupError> {
        Err(SurfaceSetupError {
            kind: rune_gpu::fallback::Failure::DeviceCreate,
            message,
        })
    }
}
