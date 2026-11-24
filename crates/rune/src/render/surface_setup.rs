//! Surface-configuring GPU acquisition with adapter and DX12 presentation fallback
//! (`rune_gpu::fallback`).
//!
//! Picks (or takes a prewarmed) GPU, creates a surface, and runs the caller's configuration/setup
//! closure on it.
//! - When DirectComposition itself is unavailable on Windows (`Failure::DirectCompositionUnavailable`)
//!   and presentation fallback is allowed, latches `Gpu::mark_dcomp_unavailable()`, switches to a
//!   plain HWND `wgpu::Instance` (`Dx12SwapchainKind::DxgiFromHwnd`), drops the failed surface and
//!   device, recreates the surface, and retries from the first adapter without walking remaining
//!   adapters on the broken DirectComposition path first.
//! - When device creation or surface configuration fails with an adapter-specific `MoveOn` failure,
//!   drops the failed surface and device, creates a fresh surface, and walks remaining adapters
//!   (other hardware first, software/WARP last; a named adapter is never replaced).
//!
//! Extension points:
//! - A later task treating a rejected last-resort shader as an adapter fallback returns
//!   `SurfaceSetupError` with `Failure::ShaderRejected` from its `setup` closure.

use super::{Gpu, Renderer};
#[cfg(not(target_arch = "wasm32"))]
use super::{gpu_events, prewarm};

/// Whether a surface caller allows falling back from DirectComposition to a plain HWND swap chain
/// on Windows when DirectComposition is unavailable.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(target_arch = "wasm32", allow(dead_code))]
pub(crate) enum PresentationFallback {
    /// Normal windows: fall back to a plain HWND swap chain (opaque only) unless
    /// `WGPU_DX12_PRESENTATION_SYSTEM` explicitly pinned presentation.
    Allow,
    /// Surfaces that require DirectComposition (Windows transparent overlays): never switch to an
    /// opaque-only HWND swap chain.
    Forbid,
}

impl PresentationFallback {
    #[cfg(not(target_arch = "wasm32"))]
    fn resolve(self) -> (crate::gpu::fallback::Presentation, crate::gpu::fallback::PresentationPolicy) {
        let (presentation, env_policy) = Gpu::dx12_presentation();
        let policy = match self {
            Self::Allow => env_policy,
            Self::Forbid => crate::gpu::fallback::PresentationPolicy::Pinned,
        };
        (presentation, policy)
    }
}

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

    pub(crate) fn from_shader(error: super::ShaderError) -> Self {
        Self { kind: error.kind.into(), message: error.message }
    }
}

#[cfg(not(target_arch = "wasm32"))]
enum ChainOutcome<S, T> {
    Done(Result<(Gpu, S, T), SurfaceSetupError>),
    SwitchToHwnd(String),
}

impl Gpu {
    /// Latches DirectComposition as unavailable, logs once for this switch, and returns the cached
    /// plain HWND `wgpu::Instance`.
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn switch_to_hwnd_instance(wanted: wgpu::Features, reason: &str) -> wgpu::Instance {
        Self::mark_dcomp_unavailable();
        eprintln!(
            "rune: DirectComposition unavailable ({reason}); falling back to a plain HWND swap chain (opaque only)"
        );
        Self::instance_for_presentation(wanted, crate::gpu::fallback::Presentation::PlainHwnd)
    }

    /// Latches direct Linux Vulkan driver loading as bypassed, logs once for this switch, and
    /// returns the cached unlimited `wgpu::Instance` with all installed drivers.
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn widen_to_unlimited_instance(reason: &str) -> wgpu::Instance {
        Self::mark_direct_vulkan_bypassed();
        eprintln!(
            "rune: direct Vulkan driver could not draw this surface ({reason}); retrying with all installed Vulkan drivers"
        );
        Self::wgpu_instance_unlimited()
    }

    /// Picks (or takes a prewarmed) GPU, creates a surface via `make_surface`, and runs `setup`
    /// (typically configuring the surface and building or retargeting the `Renderer`). When a
    /// prewarmed GPU is available, its already-compiled `Renderer` is handed to `setup` so a
    /// matching format does no extra shader work.
    ///
    /// Launch speed: when the prewarmed GPU or first picked adapter succeeds, no other adapters
    /// are enumerated, `make_surface` is called once, and no fallback instance is created.
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn for_surface_setup<'w, T>(
        wanted: wgpu::Features,
        translucent: bool,
        presentation_fallback: PresentationFallback,
        make_surface: impl FnMut(
            &wgpu::Instance,
            crate::gpu::fallback::Presentation,
        ) -> Result<wgpu::Surface<'w>, String>,
        setup: impl FnMut(&Gpu, &wgpu::Surface<'w>, Option<Renderer>) -> Result<T, SurfaceSetupError>,
    ) -> Result<(Gpu, wgpu::Surface<'w>, T), SurfaceSetupError> {
        Self::for_surface_setup_from(
            prewarm::take(),
            wanted,
            translucent,
            presentation_fallback,
            make_surface,
            setup,
        )
    }

    /// Picks a headless GPU and runs `setup` (typically building a `Renderer`), walking remaining
    /// adapters (other hardware first, software/WARP last) when `setup` fails with a
    /// fallback-eligible failure such as `Failure::ShaderRejected`.
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn for_headless_setup<T>(
        wanted: wgpu::Features,
        mut setup: impl FnMut(&Gpu) -> Result<T, SurfaceSetupError>,
    ) -> Result<(Gpu, T), SurfaceSetupError> {
        use crate::gpu::fallback::{Presentation, PresentationPolicy};
        let instance = Self::instance_for(wanted);
        let can_widen = Self::is_direct_driver_instance(&instance);
        let (gpu, (), value) = Self::try_setup_on_with_widen(
            instance,
            (),
            None,
            Presentation::DirectComposition,
            PresentationPolicy::Pinned,
            wanted,
            false,
            |_, _| Ok(()),
            |_| unreachable!("headless setup never switches presentation"),
            |reason| can_widen.then(|| Self::widen_to_unlimited_instance(reason)),
            |_| None,
            |gpu, _| setup(gpu),
        )?;
        Ok((gpu, value))
    }

    /// Picks a software adapter (`DeviceType::Cpu`, e.g. DX12 WARP on Windows) for repeated
    /// hardware device-loss fallback, creates a surface via `make_surface`, and runs `setup`.
    /// Never uses a prewarmed hardware GPU and never falls back to a hardware adapter.
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn for_surface_setup_software<'w, T>(
        wanted: wgpu::Features,
        presentation_fallback: PresentationFallback,
        mut make_surface: impl FnMut(
            &wgpu::Instance,
            crate::gpu::fallback::Presentation,
        ) -> Result<wgpu::Surface<'w>, String>,
        mut setup: impl FnMut(&Gpu, &wgpu::Surface<'w>, Option<Renderer>) -> Result<T, SurfaceSetupError>,
    ) -> Result<(Gpu, wgpu::Surface<'w>, T), SurfaceSetupError> {
        use crate::gpu::fallback::{Action, Failure, Presentation};
        let (presentation, pres_policy) = presentation_fallback.resolve();
        let instance = Self::instance_for_presentation(wanted, presentation);
        let surface = make_surface(&instance, presentation)
            .map_err(|message| SurfaceSetupError { kind: Failure::SurfaceCreate, message })?;
        gpu_events::starting();
        let (gpu, surface, value) = match Self::try_software_setup_on(
            instance,
            surface,
            wanted,
            |s| Some(s),
            |gpu, surface| setup(gpu, surface, None),
        ) {
            Ok(made) => made,
            Err(err)
                if crate::gpu::fallback::decide_with_presentation(
                    crate::gpu::fallback::Policy::Pinned,
                    presentation,
                    pres_policy,
                    err.kind,
                ) == Action::SwitchToHwnd =>
            {
                let hwnd_instance = Self::switch_to_hwnd_instance(wanted, &err.message);
                let surface = make_surface(&hwnd_instance, Presentation::PlainHwnd)
                    .map_err(|message| SurfaceSetupError { kind: Failure::SurfaceCreate, message })?;
                Self::try_software_setup_on(hwnd_instance, surface, wanted, |s| Some(s), |gpu, surface| setup(gpu, surface, None))?
            }
            Err(err) => return Err(err),
        };
        Self::log_surface_gpu(&gpu);
        Ok((gpu, surface, value))
    }

    /// Parameterized software-adapter setup helper so offscreen GPU tests can verify software
    /// fallback without opening a native window.
    #[cfg(not(target_arch = "wasm32"))]
    pub(super) fn try_software_setup_on<S, T>(
        instance: wgpu::Instance,
        surface: S,
        wanted: wgpu::Features,
        surface_ref: impl Fn(&S) -> Option<&wgpu::Surface<'_>>,
        mut setup: impl FnMut(&Gpu, &S) -> Result<T, SurfaceSetupError>,
    ) -> Result<(Gpu, S, T), SurfaceSetupError> {
        let adapter = Self::pick_software_adapter(&instance, surface_ref(&surface))?;
        let (device, queue) = Self::create_device_serialized(&adapter, wanted)
            .map_err(|message| SurfaceSetupError { kind: crate::gpu::fallback::Failure::DeviceCreate, message })?;
        let draws_surfaces = surface_ref(&surface).is_some();
        let gpu = Gpu::made(instance, adapter, device, queue, draws_surfaces);
        match setup(&gpu, &surface) {
            Ok(value) => {
                crate::native_surface::note_device(&gpu.device, draws_surfaces);
                Ok((gpu, surface, value))
            }
            Err(err) => {
                drop((surface, gpu));
                Err(err)
            }
        }
    }

    /// Enters the fallback chain directly after an already-open or pooled GPU failed `setup` with
    /// `Action::MoveOn`, excluding `excluded` so the failed adapter is not tried again.
    #[cfg(not(target_arch = "wasm32"))]
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn for_surface_setup_excluding<'w, T>(
        instance: wgpu::Instance,
        presentation: crate::gpu::fallback::Presentation,
        presentation_fallback: PresentationFallback,
        excluded: crate::gpu::fallback::Candidate,
        first_info: wgpu::AdapterInfo,
        first_kind: crate::gpu::fallback::Failure,
        first_error: String,
        wanted: wgpu::Features,
        translucent: bool,
        mut make_surface: impl FnMut(
            &wgpu::Instance,
            crate::gpu::fallback::Presentation,
        ) -> Result<wgpu::Surface<'w>, String>,
        mut setup: impl FnMut(&Gpu, &wgpu::Surface<'w>) -> Result<T, SurfaceSetupError>,
    ) -> Result<(Gpu, wgpu::Surface<'w>, T), SurfaceSetupError> {
        use crate::gpu::fallback::{Presentation, PresentationPolicy, State};
        let (_, pres_policy) = presentation_fallback.resolve();
        let state = State::need_fallbacks_with_presentation(first_error, presentation, pres_policy);
        let can_widen = Self::is_direct_driver_instance(&instance);
        let mut widen = |reason: &str| can_widen.then(|| Self::widen_to_unlimited_instance(reason));
        let outcome = Self::run_fallback_chain(
            instance,
            presentation,
            pres_policy,
            state,
            first_kind,
            None,
            vec![excluded],
            first_info,
            crate::gpu::pick::compositor_pci(),
            wanted,
            translucent,
            &mut make_surface,
            &mut widen,
            |s| Some(s),
            &mut setup,
        );
        let (gpu, surface, value) = match outcome {
            ChainOutcome::Done(result) => result?,
            ChainOutcome::SwitchToHwnd(reason) => {
                let hwnd_instance = Self::switch_to_hwnd_instance(wanted, &reason);
                let surface = make_surface(&hwnd_instance, Presentation::PlainHwnd)
                    .map_err(|message| SurfaceSetupError {
                        kind: crate::gpu::fallback::Failure::SurfaceCreate,
                        message,
                    })?;
                Self::try_setup_on(
                    hwnd_instance,
                    surface,
                    None,
                    Presentation::PlainHwnd,
                    PresentationPolicy::Pinned,
                    wanted,
                    translucent,
                    make_surface,
                    |_| unreachable!("already on PlainHwnd"),
                    |s| Some(s),
                    setup,
                )?
            }
        };
        Self::log_surface_gpu(&gpu);
        Ok((gpu, surface, value))
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn for_surface_setup_from<'w, T>(
        early: Option<(Gpu, Renderer)>,
        wanted: wgpu::Features,
        translucent: bool,
        presentation_fallback: PresentationFallback,
        mut make_surface: impl FnMut(
            &wgpu::Instance,
            crate::gpu::fallback::Presentation,
        ) -> Result<wgpu::Surface<'w>, String>,
        mut setup: impl FnMut(&Gpu, &wgpu::Surface<'w>, Option<Renderer>) -> Result<T, SurfaceSetupError>,
    ) -> Result<(Gpu, wgpu::Surface<'w>, T), SurfaceSetupError> {
        use crate::gpu::fallback::{self, Action, Failure, Presentation, PresentationPolicy, State};
        let (initial_pres, pres_policy) = presentation_fallback.resolve();
        let early = early.filter(|(gpu, _)| {
            !gpu.is_lost()
                && gpu.presentation() == initial_pres
                && crate::gpu::backend::serves(&gpu.device, wanted)
        });
        let (instance, initial_surface, presentation, pres_policy) = match early {
            Some((early, renderer)) => {
                let early_pres = early.presentation();
                let surface = make_surface(&early.instance, early_pres)
                    .map_err(|message| SurfaceSetupError { kind: Failure::SurfaceCreate, message })?;
                if crate::render::translucency::serves(&early, &surface, wanted, translucent) {
                    match setup(&early, &surface, Some(renderer)) {
                        Ok(value) => {
                            Self::log_surface_gpu(&early);
                            return Ok((early, surface, value));
                        }
                        Err(err) => match fallback::decide_with_presentation(
                            crate::gpu::pick::env_policy(),
                            early_pres,
                            pres_policy,
                            err.kind,
                        ) {
                            Action::Stop => {
                                drop((surface, early));
                                return Err(err);
                            }
                            Action::SwitchToHwnd => {
                                drop((surface, early));
                                gpu_events::starting();
                                let hwnd_instance = Self::switch_to_hwnd_instance(wanted, &err.message);
                                let surface = make_surface(&hwnd_instance, Presentation::PlainHwnd)
                                    .map_err(|message| SurfaceSetupError {
                                        kind: Failure::SurfaceCreate,
                                        message,
                                    })?;
                                (hwnd_instance, surface, Presentation::PlainHwnd, PresentationPolicy::Pinned)
                            }
                            Action::MoveOn => {
                                let first_info = early.adapter.get_info();
                                let first_candidate = Self::candidate_info(0, &early.adapter, Some(&surface));
                                let instance = early.instance.clone();
                                drop((surface, early));
                                gpu_events::starting();
                                let mut fresh_setup = |gpu: &Gpu, surface: &wgpu::Surface<'w>| setup(gpu, surface, None);
                                let can_widen = Self::is_direct_driver_instance(&instance);
                                let mut widen = |reason: &str| can_widen.then(|| Self::widen_to_unlimited_instance(reason));

                                let outcome = Self::run_fallback_chain(
                                    instance,
                                    early_pres,
                                    pres_policy,
                                    State::need_fallbacks_with_presentation(err.message, early_pres, pres_policy),
                                    err.kind,
                                    None,
                                    vec![first_candidate],
                                    first_info,
                                    crate::gpu::pick::compositor_pci(),
                                    wanted,
                                    translucent,
                                    &mut make_surface,
                                    &mut widen,
                                    |s| Some(s),
                                    &mut fresh_setup,
                                );
                                match outcome {
                                    ChainOutcome::Done(result) => {
                                        let (gpu, surface, value) = result?;
                                        Self::log_surface_gpu(&gpu);
                                        return Ok((gpu, surface, value));
                                    }
                                    ChainOutcome::SwitchToHwnd(reason) => {
                                        let hwnd_instance = Self::switch_to_hwnd_instance(wanted, &reason);
                                        let surface = make_surface(&hwnd_instance, Presentation::PlainHwnd)
                                            .map_err(|message| SurfaceSetupError {
                                                kind: Failure::SurfaceCreate,
                                                message,
                                            })?;
                                        (hwnd_instance, surface, Presentation::PlainHwnd, PresentationPolicy::Pinned)
                                    }
                                }
                            }
                        },
                    }
                } else {
                    let instance = early.instance.clone();
                    drop((renderer, early));
                    (instance, surface, early_pres, pres_policy)
                }
            }
            None => {
                let instance = Self::instance_for_presentation(wanted, initial_pres);
                let surface = make_surface(&instance, initial_pres)
                    .map_err(|message| SurfaceSetupError { kind: Failure::SurfaceCreate, message })?;
                (instance, surface, initial_pres, pres_policy)
            }
        };
        gpu_events::starting();
        let can_widen = Self::is_direct_driver_instance(&instance);
        let (gpu, surface, value) = Self::try_setup_on_with_widen(
            instance,
            initial_surface,
            None,
            presentation,
            pres_policy,
            wanted,
            translucent,
            make_surface,
            |reason| Self::switch_to_hwnd_instance(wanted, reason),
            |reason| can_widen.then(|| Self::widen_to_unlimited_instance(reason)),
            |s| Some(s),
            |gpu, surface| setup(gpu, surface, None),
        )?;
        Self::log_surface_gpu(&gpu);
        Ok((gpu, surface, value))
    }

    /// Runs the first adapter pick + `setup`, and on a `MoveOn` or `SwitchToHwnd` failure walks
    /// the unified fallback chain through `rune_gpu::fallback::step`. Parameterized over the
    /// surface handle `S` and `switch_to_hwnd` so offscreen GPU tests can exercise both adapter
    /// and presentation fallback without opening a native window.
    #[cfg(not(target_arch = "wasm32"))]
    #[allow(clippy::too_many_arguments)]
    pub(super) fn try_setup_on<S, T>(
        instance: wgpu::Instance,
        surface: S,
        policy_override: Option<crate::gpu::fallback::Policy>,
        presentation: crate::gpu::fallback::Presentation,
        presentation_policy: crate::gpu::fallback::PresentationPolicy,
        wanted: wgpu::Features,
        translucent: bool,
        make_surface: impl FnMut(
            &wgpu::Instance,
            crate::gpu::fallback::Presentation,
        ) -> Result<S, String>,
        switch_to_hwnd: impl FnMut(&str) -> wgpu::Instance,
        surface_ref: impl Fn(&S) -> Option<&wgpu::Surface<'_>>,
        setup: impl FnMut(&Gpu, &S) -> Result<T, SurfaceSetupError>,
    ) -> Result<(Gpu, S, T), SurfaceSetupError> {
        Self::try_setup_on_with_widen(
            instance,
            surface,
            policy_override,
            presentation,
            presentation_policy,
            wanted,
            translucent,
            make_surface,
            switch_to_hwnd,
            |_| None,
            surface_ref,
            setup,
        )
    }

    /// Like [`Self::try_setup_on`], also allowing a single-driver Linux Vulkan instance to widen
    /// via `widen_instance` to an unlimited instance with all installed drivers if the single
    /// driver fails with an automatic fallback error and leaves no working candidates.
    #[cfg(not(target_arch = "wasm32"))]
    #[allow(clippy::too_many_arguments)]
    pub(super) fn try_setup_on_with_widen<S, T>(
        mut instance: wgpu::Instance,
        mut surface: S,
        policy_override: Option<crate::gpu::fallback::Policy>,
        mut presentation: crate::gpu::fallback::Presentation,
        mut presentation_policy: crate::gpu::fallback::PresentationPolicy,
        wanted: wgpu::Features,
        translucent: bool,
        mut make_surface: impl FnMut(
            &wgpu::Instance,
            crate::gpu::fallback::Presentation,
        ) -> Result<S, String>,
        mut switch_to_hwnd: impl FnMut(&str) -> wgpu::Instance,
        mut widen_instance: impl FnMut(&str) -> Option<wgpu::Instance>,
        surface_ref: impl Fn(&S) -> Option<&wgpu::Surface<'_>>,
        mut setup: impl FnMut(&Gpu, &S) -> Result<T, SurfaceSetupError>,
    ) -> Result<(Gpu, S, T), SurfaceSetupError> {
        use crate::gpu::fallback::{self, Effect, Event, Failure, Policy, Presentation, PresentationPolicy, State};
        let mut widened = false;
        loop {
            let (adapter, policy, compositor) = match Self::pick_first_adapter(
                &instance,
                surface_ref(&surface),
                policy_override,
                translucent,
            ) {
                Ok(picked) => picked,
                Err(err) => {
                    if !widened
                        && policy_override != Some(Policy::Pinned)
                        && Self::env_fallback_policy() == Policy::Automatic
                        && let Some(unlimited) = widen_instance(&err.message)
                    {
                        widened = true;
                        drop(surface);
                        instance = unlimited;
                        surface = make_surface(&instance, presentation).map_err(|message| {
                            SurfaceSetupError { kind: Failure::SurfaceCreate, message }
                        })?;
                        continue;
                    }
                    return Err(err);
                }
            };
            let first_info = adapter.get_info();
            let first_candidate = Self::candidate_info(0, &adapter, surface_ref(&surface));
            let mut state = State::first_with_presentation(policy, presentation, presentation_policy);
            let made = Self::create_device_serialized(&adapter, wanted);
            let (fail_kind, fail_msg, kept_surface) = match made {
                Ok((device, queue)) => {
                    crate::marks::mark("gpu.device");
                    let draws_surfaces = surface_ref(&surface).is_some();
                    let gpu = Gpu::made_with_presentation(
                        instance.clone(),
                        adapter,
                        device,
                        queue,
                        draws_surfaces,
                        presentation,
                    );
                    match setup(&gpu, &surface) {
                        Ok(value) => {
                            let _ = fallback::step(&mut state, Event::Succeeded);
                            crate::native_surface::note_device(&gpu.device, draws_surfaces);
                            return Ok((gpu, surface, value));
                        }
                        Err(err) => {
                            // Drop the failed surface and device before trying the next adapter or
                            // presentation mode so any partial DirectComposition target or swap
                            // chain is released first.
                            drop((surface, gpu));
                            (err.kind, err.message, None)
                        }
                    }
                }
                Err(error) => (Failure::DeviceCreate, error, Some(surface)),
            };
            let effects = fallback::step(&mut state, Event::Failed { kind: fail_kind, error: fail_msg.clone() });
            match effects.into_iter().next() {
                Some(Effect::SwitchToHwnd) => {
                    drop(kept_surface);
                    instance = switch_to_hwnd(&fail_msg);
                    presentation = Presentation::PlainHwnd;
                    presentation_policy = PresentationPolicy::Pinned;
                    surface = make_surface(&instance, presentation)
                        .map_err(|message| SurfaceSetupError { kind: Failure::SurfaceCreate, message })?;
                }
                Some(Effect::EnumerateFallbacks) => {
                    let mut chain_widen = |reason: &str| {
                        if widened {
                            None
                        } else {
                            let next = widen_instance(reason);
                            if next.is_some() {
                                widened = true;
                            }
                            next
                        }
                    };
                    match Self::run_fallback_chain(
                        instance,
                        presentation,
                        presentation_policy,
                        state,
                        fail_kind,
                        kept_surface,
                        vec![first_candidate],
                        first_info,
                        compositor,
                        wanted,
                        translucent,
                        &mut make_surface,
                        &mut chain_widen,
                        &surface_ref,
                        &mut setup,
                    ) {
                        ChainOutcome::Done(result) => return result,
                        ChainOutcome::SwitchToHwnd(reason) => {
                            instance = switch_to_hwnd(&reason);
                            presentation = Presentation::PlainHwnd;
                            presentation_policy = PresentationPolicy::Pinned;
                            surface = make_surface(&instance, presentation)
                                .map_err(|message| SurfaceSetupError {
                                    kind: Failure::SurfaceCreate,
                                    message,
                                })?;
                        }
                    }
                }
                Some(Effect::FinishErr(message)) => return Err(SurfaceSetupError { kind: fail_kind, message }),
                other => unreachable!("unexpected fallback effect after first failure: {other:?}"),
            }
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[allow(clippy::too_many_arguments)]
    fn run_fallback_chain<S, T>(
        mut instance: wgpu::Instance,
        presentation: crate::gpu::fallback::Presentation,
        presentation_policy: crate::gpu::fallback::PresentationPolicy,
        mut state: crate::gpu::fallback::State,
        first_kind: crate::gpu::fallback::Failure,
        mut pending_surface: Option<S>,
        mut tried: Vec<crate::gpu::fallback::Candidate>,
        first_info: wgpu::AdapterInfo,
        compositor: Option<String>,
        wanted: wgpu::Features,
        translucent: bool,
        make_surface: &mut impl FnMut(
            &wgpu::Instance,
            crate::gpu::fallback::Presentation,
        ) -> Result<S, String>,
        widen_instance: &mut impl FnMut(&str) -> Option<wgpu::Instance>,
        surface_ref: impl Fn(&S) -> Option<&wgpu::Surface<'_>>,
        setup: &mut impl FnMut(&Gpu, &S) -> Result<T, SurfaceSetupError>,
    ) -> ChainOutcome<S, T> {
        use crate::gpu::fallback::{self, Action, Candidate, Effect, Event, Failure, Policy, State};
        let surface = match pending_surface.take() {
            Some(surface) => surface,
            None => match make_surface(&instance, presentation) {
                Ok(surface) => surface,
                Err(message) => {
                    return ChainOutcome::Done(Err(SurfaceSetupError {
                        kind: Failure::SurfaceCreate,
                        message,
                    }));
                }
            },
        };
        let mut adapters = Self::enumerate_adapters_serialized(&instance);
        let mut candidates: Vec<Candidate> = adapters
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
        let mut last_kind = first_kind;
        let mut last_error = String::new();
        let mut last_stopped = false;
        let mut widened = false;
        loop {
            let mut next_event = None;
            for effect in fallback::step(&mut state, event) {
                match effect {
                    Effect::TryCandidate(index) => {
                        let surface = match pending_surface.take() {
                            Some(surface) => surface,
                            None => match make_surface(&instance, presentation) {
                                Ok(surface) => surface,
                                Err(message) => {
                                    last_kind = Failure::SurfaceCreate;
                                    last_error = message.clone();
                                    last_stopped = true;
                                    next_event = Some(Event::Failed {
                                        kind: Failure::SurfaceCreate,
                                        error: message,
                                    });
                                    continue;
                                }
                            },
                        };
                        let candidate = &adapters[index];
                        match Self::create_device_serialized(candidate, wanted) {
                            Ok((device, queue)) => {
                                let draws_surfaces = surface_ref(&surface).is_some();
                                let gpu = Gpu::made_with_presentation(
                                    instance.clone(),
                                    candidate.clone(),
                                    device,
                                    queue,
                                    draws_surfaces,
                                    presentation,
                                );
                                match setup(&gpu, &surface) {
                                    Ok(value) => {
                                        let first_error = state.first_error().unwrap_or("").to_owned();
                                        let _ = fallback::step(&mut state, Event::Succeeded);
                                        let info = gpu.adapter.get_info();
                                        let what = if draws_surfaces { "this surface" } else { "offscreen" };
                                        eprintln!(
                                            "rune: {} ({:?}) could not draw {what} ({first_error}); falling back to {} ({:?}, {:?})",
                                            first_info.name, first_info.backend, info.name, info.backend, info.device_type
                                        );
                                        crate::native_surface::note_device(&gpu.device, draws_surfaces);
                                        return ChainOutcome::Done(Ok((gpu, surface, value)));
                                    }
                                    Err(err) => {
                                        last_kind = err.kind;
                                        last_error = err.message.clone();
                                        last_stopped = fallback::decide_with_presentation(
                                            Policy::Automatic,
                                            presentation,
                                            presentation_policy,
                                            err.kind,
                                        ) == Action::Stop;
                                        tried.push(candidates[index].clone());
                                        drop((surface, gpu));
                                        next_event = Some(Event::Failed { kind: err.kind, error: err.message });
                                    }
                                }
                            }
                            Err(error) => {
                                last_kind = Failure::DeviceCreate;
                                last_error = error.clone();
                                last_stopped = false;
                                tried.push(candidates[index].clone());
                                pending_surface = Some(surface);
                                next_event = Some(Event::Failed { kind: Failure::DeviceCreate, error });
                            }
                        }
                    }
                    Effect::SwitchToHwnd => {
                        drop(pending_surface);
                        return ChainOutcome::SwitchToHwnd(last_error);
                    }
                    Effect::FinishErr(message) => {
                        if !last_stopped && !widened
                            && let Some(unlimited) = widen_instance(&message)
                        {
                            widened = true;
                            drop(pending_surface.take());
                            instance = unlimited;
                            let surface = match make_surface(&instance, presentation) {
                                Ok(surface) => surface,
                                Err(err_msg) => {
                                    return ChainOutcome::Done(Err(SurfaceSetupError {
                                        kind: Failure::SurfaceCreate,
                                        message: err_msg,
                                    }));
                                }
                            };
                            adapters = Self::enumerate_adapters_serialized(&instance);
                            candidates = adapters
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
                            state = State::need_fallbacks_with_presentation(
                                message,
                                presentation,
                                presentation_policy,
                            );
                            next_event = Some(Event::Candidates(ordered));
                            continue;
                        }
                        return ChainOutcome::Done(Err(SurfaceSetupError { kind: last_kind, message }));
                    }
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
    pub(crate) fn switch_to_hwnd_instance(wanted: wgpu::Features, _reason: &str) -> wgpu::Instance {
        Self::instance_for(wanted)
    }

    #[cfg(target_arch = "wasm32")]
    #[allow(dead_code)]
    pub(crate) fn for_surface_setup<'w, T>(
        _: wgpu::Features,
        _: bool,
        _: PresentationFallback,
        _: impl FnMut(
            &wgpu::Instance,
            crate::gpu::fallback::Presentation,
        ) -> Result<wgpu::Surface<'w>, String>,
        _: impl FnMut(&Gpu, &wgpu::Surface<'w>, Option<Renderer>) -> Result<T, SurfaceSetupError>,
    ) -> Result<(Gpu, wgpu::Surface<'w>, T), SurfaceSetupError> {
        Err(SurfaceSetupError {
            kind: rune_gpu::fallback::Failure::DeviceCreate,
            message: "synchronous GPU initialization is unsupported on web; use App::run".into(),
        })
    }

    #[cfg(target_arch = "wasm32")]
    #[allow(dead_code)]
    pub(crate) fn for_headless_setup<T>(
        _: wgpu::Features,
        _: impl FnMut(&Gpu) -> Result<T, SurfaceSetupError>,
    ) -> Result<(Gpu, T), SurfaceSetupError> {
        Err(SurfaceSetupError {
            kind: rune_gpu::fallback::Failure::DeviceCreate,
            message: "synchronous GPU initialization is unsupported on web; use App::run".into(),
        })
    }

    #[cfg(target_arch = "wasm32")]
    #[allow(dead_code)]
    pub(crate) fn for_surface_setup_software<'w, T>(
        _: wgpu::Features,
        _: PresentationFallback,
        _: impl FnMut(&wgpu::Instance, crate::gpu::fallback::Presentation) -> Result<wgpu::Surface<'w>, String>,
        _: impl FnMut(&Gpu, &wgpu::Surface<'w>, Option<Renderer>) -> Result<T, SurfaceSetupError>,
    ) -> Result<(Gpu, wgpu::Surface<'w>, T), SurfaceSetupError> {
        Err(SurfaceSetupError::unsupported(
            "synchronous GPU initialization is unsupported on web; use App::run",
        ))
    }

    #[cfg(target_arch = "wasm32")]
    #[allow(dead_code)]
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn for_surface_setup_excluding<'w, T>(
        _: wgpu::Instance,
        _: crate::gpu::fallback::Presentation,
        _: PresentationFallback,
        _: crate::gpu::fallback::Candidate,
        _: wgpu::AdapterInfo,
        _: crate::gpu::fallback::Failure,
        message: String,
        _: wgpu::Features,
        _: bool,
        _: impl FnMut(
            &wgpu::Instance,
            crate::gpu::fallback::Presentation,
        ) -> Result<wgpu::Surface<'w>, String>,
        _: impl FnMut(&Gpu, &wgpu::Surface<'w>) -> Result<T, SurfaceSetupError>,
    ) -> Result<(Gpu, wgpu::Surface<'w>, T), SurfaceSetupError> {
        Err(SurfaceSetupError {
            kind: rune_gpu::fallback::Failure::DeviceCreate,
            message,
        })
    }
}
