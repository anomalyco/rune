//! Surface-configuring GPU acquisition with adapter fallback (`rune_gpu::fallback`).
//!
//! Picks (or takes a prewarmed) GPU, creates a surface, and runs the caller's configuration/setup
//! closure on it. When device creation, surface configuration, or last-resort shader compilation
//! fails with a fallback-eligible `rune_gpu::fallback::Failure`, drops the failed surface and
//! device, creates a fresh surface, and walks remaining adapters (other hardware first,
//! software/WARP last; a named adapter is never replaced).

use super::{Gpu, Renderer};
#[cfg(not(target_arch = "wasm32"))]
use super::{gpu_events, prewarm};

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

    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn device_lost(message: impl Into<String>) -> Self {
        Self { kind: rune_gpu::fallback::Failure::DeviceLost, message: message.into() }
    }

    #[cfg(not(target_arch = "wasm32"))]
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

/// Appends the explicit DX12 HWND presentation hint when a DirectComposition failure reaches a
/// window error under Wine or Proton. Never switches presentation automatically, and never probes
/// Wine on the happy path.
#[cfg_attr(not(any(target_os = "windows", test)), allow(dead_code))]
pub(crate) fn append_wine_hwnd_hint(
    message: String,
    swapchain: wgpu::Dx12SwapchainKind,
    under_wine: bool,
) -> String {
    if swapchain == wgpu::Dx12SwapchainKind::DxgiFromVisual && under_wine {
        format!(
            "{message}; under Wine/Proton without DirectComposition, set WGPU_DX12_PRESENTATION_SYSTEM=hwnd"
        )
    } else {
        message
    }
}

/// Looks up `wine_get_version` in `ntdll.dll` only on DirectComposition error paths (never at
/// launch) and appends the `WGPU_DX12_PRESENTATION_SYSTEM=hwnd` hint when running under Wine.
#[cfg(target_os = "windows")]
pub(crate) fn with_wine_hwnd_hint(message: String) -> String {
    let swapchain = Gpu::dx12_swapchain_kind();
    if swapchain != wgpu::Dx12SwapchainKind::DxgiFromVisual {
        return message;
    }
    append_wine_hwnd_hint(message, swapchain, crate::platform::win32::running_under_wine())
}

/// Formats a `SurfaceSetupError` for a window error path, appending the Wine HWND hint only when
/// `Surface::configure` was refused on `DxgiFromVisual` under Wine/Proton.
#[cfg_attr(target_arch = "wasm32", allow(dead_code))]
pub(crate) fn format_window_setup_error(err: SurfaceSetupError) -> String {
    #[cfg(target_os = "windows")]
    if err.kind == crate::gpu::fallback::Failure::SurfaceRefused {
        return with_wine_hwnd_hint(err.message);
    }
    err.message
}

impl Gpu {
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
        make_surface: impl FnMut(&wgpu::Instance) -> Result<wgpu::Surface<'w>, String>,
        setup: impl FnMut(&Gpu, &wgpu::Surface<'w>, Option<Renderer>) -> Result<T, SurfaceSetupError>,
    ) -> Result<(Gpu, wgpu::Surface<'w>, T), SurfaceSetupError> {
        Self::for_surface_setup_with_latch(None, wanted, translucent, make_surface, setup)
    }

    /// Like [`Self::for_surface_setup`], but honors a `GpuLossPolicy::Terminal` latch: refuses
    /// immediately if already failed, checks the latch again right after `prewarm::take()` (so a
    /// parked warm device that died while waiting trips the owner's latch and never falls back to
    /// creating a replacement device), and disables adapter fallback under `Terminal`.
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn for_surface_setup_with_latch<'w, T>(
        latch: Option<&std::sync::Arc<super::loss_latch::GpuLossLatch>>,
        wanted: wgpu::Features,
        translucent: bool,
        make_surface: impl FnMut(&wgpu::Instance) -> Result<wgpu::Surface<'w>, String>,
        setup: impl FnMut(&Gpu, &wgpu::Surface<'w>, Option<Renderer>) -> Result<T, SurfaceSetupError>,
    ) -> Result<(Gpu, wgpu::Surface<'w>, T), SurfaceSetupError> {
        if let Some(latch) = latch && latch.is_failed() {
            return Err(SurfaceSetupError::device_lost(
                latch.reason().unwrap_or_else(|| "the GPU device was lost".into()),
            ));
        }
        let early = prewarm::take();
        if let Some(latch) = latch && latch.is_failed() {
            drop(early);
            return Err(SurfaceSetupError::device_lost(
                latch.reason().unwrap_or_else(|| "the warmed GPU device was lost".into()),
            ));
        }
        Self::for_surface_setup_from_policy(early, latch.is_none(), wanted, translucent, make_surface, setup)
    }

    /// Picks a headless GPU and runs `setup` (typically building a `Renderer`), walking remaining
    /// adapters (other hardware first, software/WARP last) when `setup` fails with a
    /// fallback-eligible failure such as `Failure::ShaderRejected`.
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn for_headless_setup<T>(
        wanted: wgpu::Features,
        mut setup: impl FnMut(&Gpu) -> Result<T, SurfaceSetupError>,
    ) -> Result<(Gpu, T), SurfaceSetupError> {
        let instance = Self::instance_for(wanted);
        let can_widen = Self::is_direct_driver_instance(&instance);
        let (gpu, (), value) = Self::try_setup_on_with_widen(
            instance,
            (),
            None,
            wanted,
            false,
            |_| Ok(()),
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
        mut make_surface: impl FnMut(&wgpu::Instance) -> Result<wgpu::Surface<'w>, String>,
        mut setup: impl FnMut(&Gpu, &wgpu::Surface<'w>, Option<Renderer>) -> Result<T, SurfaceSetupError>,
    ) -> Result<(Gpu, wgpu::Surface<'w>, T), SurfaceSetupError> {
        use crate::gpu::fallback::Failure;
        let instance = Self::instance_for(wanted);
        let surface = make_surface(&instance)
            .map_err(|message| SurfaceSetupError { kind: Failure::SurfaceCreate, message })?;
        gpu_events::starting();
        let (gpu, surface, value) = Self::try_software_setup_on(
            instance,
            surface,
            wanted,
            |s| Some(s),
            |gpu, surface| setup(gpu, surface, None),
        )?;
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
    /// `Decision::MoveOn`, excluding `excluded` so the failed adapter is not tried again.
    #[cfg(not(target_arch = "wasm32"))]
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn for_surface_setup_excluding<'w, T>(
        instance: wgpu::Instance,
        excluded: crate::gpu::fallback::Candidate,
        first_info: wgpu::AdapterInfo,
        first_kind: crate::gpu::fallback::Failure,
        first_error: String,
        wanted: wgpu::Features,
        translucent: bool,
        mut make_surface: impl FnMut(&wgpu::Instance) -> Result<wgpu::Surface<'w>, String>,
        mut setup: impl FnMut(&Gpu, &wgpu::Surface<'w>) -> Result<T, SurfaceSetupError>,
    ) -> Result<(Gpu, wgpu::Surface<'w>, T), SurfaceSetupError> {
        use crate::gpu::fallback::State;
        let state = State::need_fallbacks(first_error);
        let can_widen = Self::is_direct_driver_instance(&instance);
        let mut widen = |reason: &str| can_widen.then(|| Self::widen_to_unlimited_instance(reason));
        let (gpu, surface, value) = Self::run_fallback_chain(
            instance,
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
        )?;
        Self::log_surface_gpu(&gpu);
        Ok((gpu, surface, value))
    }

    #[cfg(all(test, target_os = "macos"))]
    pub(super) fn for_surface_setup_from<'w, T>(
        early: Option<(Gpu, Renderer)>,
        wanted: wgpu::Features,
        translucent: bool,
        make_surface: impl FnMut(&wgpu::Instance) -> Result<wgpu::Surface<'w>, String>,
        setup: impl FnMut(&Gpu, &wgpu::Surface<'w>, Option<Renderer>) -> Result<T, SurfaceSetupError>,
    ) -> Result<(Gpu, wgpu::Surface<'w>, T), SurfaceSetupError> {
        Self::for_surface_setup_from_policy(early, true, wanted, translucent, make_surface, setup)
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub(super) fn for_surface_setup_from_policy<'w, T>(
        early: Option<(Gpu, Renderer)>,
        allow_fallback: bool,
        wanted: wgpu::Features,
        translucent: bool,
        mut make_surface: impl FnMut(&wgpu::Instance) -> Result<wgpu::Surface<'w>, String>,
        mut setup: impl FnMut(&Gpu, &wgpu::Surface<'w>, Option<Renderer>) -> Result<T, SurfaceSetupError>,
    ) -> Result<(Gpu, wgpu::Surface<'w>, T), SurfaceSetupError> {
        use crate::gpu::fallback::{self, Decision, Failure, State};
        let early = early.filter(|(gpu, _)| {
            !gpu.is_lost() && crate::gpu::backend::serves(&gpu.device, wanted)
        });
        let (instance, initial_surface) = match early {
            Some((early, renderer)) => {
                let surface = make_surface(&early.instance)
                    .map_err(|message| SurfaceSetupError { kind: Failure::SurfaceCreate, message })?;
                if crate::render::translucency::serves(&early, &surface, wanted, translucent) {
                    early.install_surface_error_policy();
                    match setup(&early, &surface, Some(renderer)) {
                        Ok(value) => {
                            Self::log_surface_gpu(&early);
                            return Ok((early, surface, value));
                        }
                        Err(err) => match if allow_fallback { fallback::decide(crate::gpu::pick::env_policy(), err.kind) } else { Decision::Stop } {
                            Decision::Stop => {
                                drop((surface, early));
                                return Err(err);
                            }
                            Decision::MoveOn => {
                                let first_info = early.adapter.get_info();
                                let first_candidate = Self::candidate_info(0, &early.adapter, Some(&surface));
                                let instance = early.instance.clone();
                                drop((surface, early));
                                gpu_events::starting();
                                let mut fresh_setup = |gpu: &Gpu, surface: &wgpu::Surface<'w>| setup(gpu, surface, None);
                                let can_widen = Self::is_direct_driver_instance(&instance);
                                let mut widen = |reason: &str| can_widen.then(|| Self::widen_to_unlimited_instance(reason));
                                let (gpu, surface, value) = Self::run_fallback_chain(
                                    instance,
                                    State::need_fallbacks(err.message),
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
                                )?;
                                Self::log_surface_gpu(&gpu);
                                return Ok((gpu, surface, value));
                            }
                        },
                    }
                } else {
                    let instance = early.instance.clone();
                    drop((renderer, early));
                    (instance, surface)
                }
            }
            None => {
                let instance = Self::instance_for(wanted);
                let surface = make_surface(&instance)
                    .map_err(|message| SurfaceSetupError { kind: Failure::SurfaceCreate, message })?;
                (instance, surface)
            }
        };
        gpu_events::starting();
        let can_widen = Self::is_direct_driver_instance(&instance);
        let policy_override = (!allow_fallback).then_some(crate::gpu::fallback::Policy::Pinned);
        let (gpu, surface, value) = Self::try_setup_on_with_widen(
            instance,
            initial_surface,
            policy_override,
            wanted,
            translucent,
            make_surface,
            |reason| can_widen.then(|| Self::widen_to_unlimited_instance(reason)),
            |s| Some(s),
            |gpu, surface| setup(gpu, surface, None),
        )?;
        Self::log_surface_gpu(&gpu);
        Ok((gpu, surface, value))
    }

    /// Runs the first adapter pick + `setup`, and on a `MoveOn` failure walks the fallback chain
    /// through `rune_gpu::fallback::step`. Parameterized over the surface handle `S` so offscreen
    /// GPU tests can exercise adapter fallback without opening a native window.
    #[cfg(all(test, not(target_arch = "wasm32")))]
    #[allow(clippy::too_many_arguments)]
    pub(super) fn try_setup_on<S, T>(
        instance: wgpu::Instance,
        surface: S,
        policy_override: Option<crate::gpu::fallback::Policy>,
        wanted: wgpu::Features,
        translucent: bool,
        make_surface: impl FnMut(&wgpu::Instance) -> Result<S, String>,
        surface_ref: impl Fn(&S) -> Option<&wgpu::Surface<'_>>,
        setup: impl FnMut(&Gpu, &S) -> Result<T, SurfaceSetupError>,
    ) -> Result<(Gpu, S, T), SurfaceSetupError> {
        Self::try_setup_on_with_widen(
            instance,
            surface,
            policy_override,
            wanted,
            translucent,
            make_surface,
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
        wanted: wgpu::Features,
        translucent: bool,
        mut make_surface: impl FnMut(&wgpu::Instance) -> Result<S, String>,
        mut widen_instance: impl FnMut(&str) -> Option<wgpu::Instance>,
        surface_ref: impl Fn(&S) -> Option<&wgpu::Surface<'_>>,
        mut setup: impl FnMut(&Gpu, &S) -> Result<T, SurfaceSetupError>,
    ) -> Result<(Gpu, S, T), SurfaceSetupError> {
        use crate::gpu::fallback::{self, Effect, Event, Failure, Policy, State};
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
                        surface = make_surface(&instance).map_err(|message| {
                            SurfaceSetupError { kind: Failure::SurfaceCreate, message }
                        })?;
                        continue;
                    }
                    return Err(err);
                }
            };
            let first_info = adapter.get_info();
            let first_candidate = Self::candidate_info(0, &adapter, surface_ref(&surface));
            let mut state = State::first(policy);
            let made = Self::create_device_serialized(&adapter, wanted);
            let (fail_kind, fail_msg, kept_surface) = match made {
                Ok((device, queue)) => {
                    crate::marks::mark("gpu.device");
                    let draws_surfaces = surface_ref(&surface).is_some();
                    let gpu = Gpu::made(
                        instance.clone(),
                        adapter,
                        device,
                        queue,
                        draws_surfaces,
                    );
                    match setup(&gpu, &surface) {
                        Ok(value) => {
                            let _ = fallback::step(&mut state, Event::Succeeded);
                            crate::native_surface::note_device(&gpu.device, draws_surfaces);
                            return Ok((gpu, surface, value));
                        }
                        Err(err) => {
                            // Drop the failed surface and device before trying the next adapter
                            // so any partial DirectComposition target or swap chain is released
                            // first.
                            drop((surface, gpu));
                            (err.kind, err.message, None)
                        }
                    }
                }
                Err(error) => (Failure::DeviceCreate, error, Some(surface)),
            };
            let effects = fallback::step(&mut state, Event::Failed { kind: fail_kind, error: fail_msg });
            match effects.into_iter().next() {
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
                    return Self::run_fallback_chain(
                        instance,
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
                    );
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
        mut state: crate::gpu::fallback::State,
        first_kind: crate::gpu::fallback::Failure,
        mut pending_surface: Option<S>,
        mut tried: Vec<crate::gpu::fallback::Candidate>,
        first_info: wgpu::AdapterInfo,
        compositor: Option<String>,
        wanted: wgpu::Features,
        translucent: bool,
        make_surface: &mut impl FnMut(&wgpu::Instance) -> Result<S, String>,
        widen_instance: &mut impl FnMut(&str) -> Option<wgpu::Instance>,
        surface_ref: impl Fn(&S) -> Option<&wgpu::Surface<'_>>,
        setup: &mut impl FnMut(&Gpu, &S) -> Result<T, SurfaceSetupError>,
    ) -> Result<(Gpu, S, T), SurfaceSetupError> {
        use crate::gpu::fallback::{self, Candidate, Decision, Effect, Event, Failure, Policy, State};
        let surface = match pending_surface.take() {
            Some(surface) => surface,
            None => make_surface(&instance)
                .map_err(|message| SurfaceSetupError { kind: Failure::SurfaceCreate, message })?,
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
        let mut last_stopped = false;
        let mut widened = false;
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
                                let gpu = Gpu::made(
                                    instance.clone(),
                                    candidate.clone(),
                                    device,
                                    queue,
                                    draws_surfaces,
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
                                        return Ok((gpu, surface, value));
                                    }
                                    Err(err) => {
                                        last_kind = err.kind;
                                        last_stopped = fallback::decide(Policy::Automatic, err.kind)
                                            == Decision::Stop;
                                        tried.push(candidates[index].clone());
                                        drop((surface, gpu));
                                        next_event = Some(Event::Failed { kind: err.kind, error: err.message });
                                    }
                                }
                            }
                            Err(error) => {
                                last_kind = Failure::DeviceCreate;
                                last_stopped = false;
                                tried.push(candidates[index].clone());
                                pending_surface = Some(surface);
                                next_event = Some(Event::Failed { kind: Failure::DeviceCreate, error });
                            }
                        }
                    }
                    Effect::FinishErr(message) => {
                        if !last_stopped && !widened
                            && let Some(unlimited) = widen_instance(&message)
                        {
                            widened = true;
                            drop(pending_surface.take());
                            instance = unlimited;
                            let surface = make_surface(&instance).map_err(|err_msg| {
                                SurfaceSetupError { kind: Failure::SurfaceCreate, message: err_msg }
                            })?;
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
                            state = State::need_fallbacks(message);
                            next_event = Some(Event::Candidates(ordered));
                            continue;
                        }
                        let kind = if last_stopped { last_kind } else { first_kind };
                        return Err(SurfaceSetupError { kind, message });
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
    pub(crate) fn for_surface_setup<'w, T>(
        _: wgpu::Features,
        _: bool,
        _: impl FnMut(&wgpu::Instance) -> Result<wgpu::Surface<'w>, String>,
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
        _: impl FnMut(&wgpu::Instance) -> Result<wgpu::Surface<'w>, String>,
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
        _: crate::gpu::fallback::Candidate,
        _: wgpu::AdapterInfo,
        _: crate::gpu::fallback::Failure,
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
