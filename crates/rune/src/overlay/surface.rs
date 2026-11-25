//! An overlay's GPU surface: making it (and the device, for the first), resizing it, and
//! dropping it with the native window's owners.

use super::*;
use crate::machine::Effects;

/// What the overlay holds of its native window. On macOS, and for a Wayland layer or XWayland
/// surface, the window is made with its GPU surface; elsewhere it waits `Parked` for one, as it
/// does on X11 once a hidden overlay gives its GPU back.
///
/// `announced`: `on_created` heard of the native window. Releasing the window's owners
/// (`Overlay::release`) clears it, so the next window made is announced; a hide that destroys a
/// Wayland surface without that release leaves it set.
pub(super) enum Surface<L = Live, B = Backing> {
    /// No native window: none made yet, or released or destroyed since.
    None { announced: bool },
    /// The native window without a GPU surface.
    Parked { backing: B, announced: bool },
    /// The native window and its GPU surface.
    Live { live: L, announced: bool },
}

impl<L, B> Default for Surface<L, B> {
    fn default() -> Self { Surface::None { announced: false } }
}

#[derive(Debug, PartialEq, Eq)]
pub(super) enum SurfaceEvent<L = Live, B = Backing> {
    /// A window was made with its GPU surface, or a parked one got its surface. It holds no
    /// window then.
    Made(L),
    /// A window without a GPU surface waits: just made, given its GPU back, or its surface
    /// failed. It holds no other window then.
    Park(B),
    /// The live window and surface go: parked again, or a Wayland surface destroyed on hide.
    TakeLive,
    /// The parked window goes to be given a surface.
    TakeParked,
    /// `on_created` hears of the native window.
    Announce,
    /// The window's owners are released: the next window made is announced again.
    Release,
}

/// What leaves the overlay's hold, for the caller to keep or drop in its order.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum SurfaceEffect<L = Live, B = Backing> {
    Live(L),
    Parked(B),
}

impl<L, B> Surface<L, B> {
    pub(super) fn step(&mut self, event: SurfaceEvent<L, B>) -> Effects<SurfaceEffect<L, B>> {
        let announced = self.announced();
        match event {
            SurfaceEvent::Made(live) => {
                debug_assert!(matches!(self, Surface::None { .. }), "an overlay makes a window only while it holds none");
                *self = Surface::Live { live, announced };
            }
            SurfaceEvent::Park(backing) => {
                debug_assert!(matches!(self, Surface::None { .. }), "an overlay parks a window only while it holds none");
                *self = Surface::Parked { backing, announced };
            }
            SurfaceEvent::TakeLive if matches!(self, Surface::Live { .. }) => {
                let Surface::Live { live, .. } = std::mem::replace(self, Surface::None { announced }) else { unreachable!() };
                return Effects::one(SurfaceEffect::Live(live));
            }
            SurfaceEvent::TakeParked if matches!(self, Surface::Parked { .. }) => {
                let Surface::Parked { backing, .. } = std::mem::replace(self, Surface::None { announced }) else { unreachable!() };
                return Effects::one(SurfaceEffect::Parked(backing));
            }
            SurfaceEvent::TakeLive | SurfaceEvent::TakeParked => {}
            SurfaceEvent::Announce => match self {
                Surface::None { announced } | Surface::Parked { announced, .. } | Surface::Live { announced, .. } => *announced = true,
            },
            SurfaceEvent::Release => match std::mem::take(self) {
                Surface::Live { live, .. } => return Effects::one(SurfaceEffect::Live(live)),
                Surface::Parked { backing, .. } => return Effects::one(SurfaceEffect::Parked(backing)),
                Surface::None { .. } => {}
            },
        }
        Effects::none()
    }

    pub(super) fn live(&self) -> Option<&L> {
        match self {
            Surface::Live { live, .. } => Some(live),
            _ => None,
        }
    }

    pub(super) fn live_mut(&mut self) -> Option<&mut L> {
        match self {
            Surface::Live { live, .. } => Some(live),
            _ => None,
        }
    }

    pub(super) fn parked(&self) -> Option<&B> {
        match self {
            Surface::Parked { backing, .. } => Some(backing),
            _ => None,
        }
    }

    pub(super) fn announced(&self) -> bool {
        match *self {
            Surface::None { announced } | Surface::Parked { announced, .. } | Surface::Live { announced, .. } => announced,
        }
    }

    pub(super) fn announce(&mut self) { self.step(SurfaceEvent::Announce); }

    pub(super) fn made(&mut self, live: L) { self.step(SurfaceEvent::Made(live)); }

    pub(super) fn park(&mut self, backing: B) { self.step(SurfaceEvent::Park(backing)); }

    pub(super) fn take_live(&mut self) -> Option<L> {
        match self.step(SurfaceEvent::TakeLive).into_iter().next() {
            Some(SurfaceEffect::Live(live)) => Some(live),
            _ => None,
        }
    }

    pub(super) fn take_parked(&mut self) -> Option<B> {
        match self.step(SurfaceEvent::TakeParked).into_iter().next() {
            Some(SurfaceEffect::Parked(backing)) => Some(backing),
            _ => None,
        }
    }

    /// Everything it held: the next window made is announced again.
    pub(super) fn release(&mut self) -> Option<SurfaceEffect<L, B>> {
        self.step(SurfaceEvent::Release).into_iter().next()
    }
}

impl Surface {
    /// The native window's backing, with a surface or parked.
    pub(super) fn backing(&self) -> Option<&Backing> {
        match self {
            Surface::Live { live, .. } => Some(&live.backing),
            Surface::Parked { backing, .. } => Some(backing),
            Surface::None { .. } => None,
        }
    }
}

impl OverlayCore {
    /// Creates the wgpu surface (and the GPU device, if this is the first surface).
    #[allow(unsafe_code)]
    pub(super) fn attach(&mut self, app: &mut dyn OverlayApp, backing: Backing, scale: f32) -> Option<Live> {
        if self.poll_terminal_gpu_loss(app) { return None; }
        // First, before the surface binds the view (macOS may observe it).
        let a11y = self.shared.wake.lock().unwrap().clone().and_then(|wake| match &backing {
            Backing::Window(window) => crate::a11y::Adapter::new(window, wake),
            #[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
            Backing::Layer(_) | Backing::Xwayland(_) => crate::a11y::Adapter::windowless(wake),
        });
        #[cfg(target_os = "macos")]
        let moved = self.panel.as_ref().map(|p| p.raw_handles());
        #[cfg(all(target_os = "windows", feature = "windows-system-material"))]
        let handover = match &backing {
            Backing::Window(w) if matches!(Gpu::dx12_swapchain_kind(), wgpu::Dx12SwapchainKind::DxgiFromVisual) => {
                crate::platform::win32::hwnd(w)
                    .and_then(|hwnd| crate::platform::win32::dcomp_handover::Handover::new(hwnd as isize).ok())
            }
            _ => None,
        };
        #[cfg(all(target_os = "windows", feature = "windows-system-material"))]
        let dcomp_visual = handover.as_ref().map(|h| h.visual());
        let make = |instance: &wgpu::Instance| -> Option<wgpu::Surface<'static>> {
            match &backing {
                Backing::Window(w) => {
                    #[cfg(target_os = "macos")]
                    if let Some((display, window)) = moved {
                        // SAFETY: the panel keeps the view (and its layer) alive while the
                        // overlay holds this surface; AppKit's display handle carries nothing.
                        return unsafe {
                            instance.create_surface_unsafe(wgpu::SurfaceTargetUnsafe::RawHandle {
                                raw_display_handle: Some(display),
                                raw_window_handle: window,
                            })
                        }
                        .ok();
                    }
                    #[cfg(all(target_os = "windows", feature = "windows-system-material"))]
                    if let Some(visual) = dcomp_visual {
                        // SAFETY: `handover` lives in `Live` and drops strictly after `surface`.
                        return unsafe {
                            instance.create_surface_unsafe(wgpu::SurfaceTargetUnsafe::CompositionVisual(visual))
                        }
                        .ok();
                    }
                    instance.create_surface(w.clone()).ok()
                }
                #[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
                Backing::Layer(layer) => {
                    let (display, window) = layer.raw_handles();
                    // SAFETY: the layer surface outlives this wgpu surface (`Live` drops the
                    // surface first), and winit keeps the display alive.
                    unsafe {
                        instance.create_surface_unsafe(wgpu::SurfaceTargetUnsafe::RawHandle {
                            raw_display_handle: Some(display),
                            raw_window_handle: window,
                        })
                    }
                    .ok()
                }
                #[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
                Backing::Xwayland(surface) => {
                    let (display, window) = surface.raw_handles()?;
                    // SAFETY: the X window and its connection outlive this wgpu surface (`Live`
                    // drops the surface first).
                    unsafe {
                        instance.create_surface_unsafe(wgpu::SurfaceTargetUnsafe::RawHandle {
                            raw_display_handle: Some(display),
                            raw_window_handle: window,
                        })
                    }
                    .ok()
                }
            }
        };
        let features = app.compositor_features();
        // Re-poll any retained GPU and check the terminal latch right after `Compositor::features()`
        // so a device lost or destroyed inside that callback never falls through to replacement.
        if self.poll_device_for_terminal_loss(app) {
            return None;
        }
        // A device that died while the overlay was hidden (a driver reset, a resume from
        // Modern Standby) can configure nothing: the next surface gets a new one.
        if self.gpu.as_ref().is_some_and(|gpu| gpu.is_lost()) {
            if self.is_terminal_gpu_policy() {
                self.latch_terminal_loss_latch(app, crate::render::loss_latch::GpuFailureKind::DeviceLost, "its GPU device was lost");
                return None;
            }
            self.gpu = None;
            self.renderer = None;
            app.release_compositor_gpu();
        }
        // The overlay's own device was already the best for its surface, translucent or not; a
        // device shared from another overlay must composite this one's translucent pixels.
        let pooled = self.gpu.clone().map(|gpu| (gpu, false)).or_else(|| self.gpu_pool.borrow().upgrade().filter(|gpu| {
            !gpu.is_lost() && gpu.device.features().contains(features & gpu.adapter.features())
        }).map(|gpu| (gpu, true)));
        let reused = pooled.and_then(|(gpu, translucent)| make(&gpu.instance).filter(|s| crate::render::translucency::serves(&gpu, s, features, translucent)).map(|s| (gpu, s)));
        let (w, h) = ((self.at.size.0 * scale).round() as u32, (self.at.size.1 * scale).round() as u32);
        let srgb_blend = app.rt().srgb_blend();
        let prewarm_effects = self.prewarm_effects;
        let frame_latency = crate::window::frame_latency(self.frame_latency);
        let make_surface = |instance: &wgpu::Instance| make(instance).ok_or_else(|| "could not create its GPU surface".to_owned());
        let setup_surface = |gpu: &Gpu, surface: &wgpu::Surface<'_>, existing: Option<Renderer>| -> Result<_, crate::render::SurfaceSetupError> {
            let (mut config, translucent) = crate::window::surface_config(surface, &gpu.adapter, (w, h), srgb_blend)
                .ok_or_else(|| crate::render::SurfaceSetupError::unsupported("the GPU adapter can't present to this overlay"))?;
            config.desired_maximum_frame_latency = frame_latency;
            let info = gpu.adapter.get_info();
            match translucent {
                Some(mode) => config.alpha_mode = mode,
                // Windows composites an overlay's pixels only through DirectComposition; drawn
                // opaque on its window they never show.
                None if cfg!(target_os = "windows") => {
                    return Err(crate::render::SurfaceSetupError::unsupported(format!(
                        "{:?} on {} can't composite transparent pixels; Windows overlays need DX12 with DirectComposition",
                        info.backend, info.name
                    )));
                }
                None => {}
            }
            // A surface the driver refuses costs this overlay alone, never the app's other windows
            // (hex#979). On a live device with an automatic pick, the fallback chain tries the next
            // adapter before giving up.
            let prefix = format!("could not configure its GPU surface on {:?} {}", info.backend, info.name);
            crate::window::configure_checked(surface, gpu, &config)
                .map_err(|error| crate::render::SurfaceSetupError::from_configure(&prefix, error))?;
            let target_format = if srgb_blend { config.format.remove_srgb_suffix() } else { config.format };
            let mut renderer = match existing {
                Some(mut renderer) => {
                    renderer.try_set_format(&gpu.device, target_format).map_err(crate::render::SurfaceSetupError::from_shader)?;
                    renderer
                }
                None => Renderer::try_new_classified(&gpu.device, target_format).map_err(crate::render::SurfaceSetupError::from_shader)?,
            };
            renderer.attach_gpu(gpu);
            renderer.set_prewarm_effects(prewarm_effects);
            Ok((config, translucent.is_none(), renderer))
        };
        let (surface, (config, opaque_fallback, mut renderer), reused_renderer) = match reused {
            Some((gpu, surface)) => {
                let already_own = self.gpu.as_ref().is_some_and(|own| Arc::ptr_eq(own, &gpu));
                if !already_own {
                    self.attach_terminal_gpu_latch(&gpu);
                }
                let own_renderer = already_own
                    .then(|| self.renderer.take())
                    .flatten();
                let had_own = own_renderer.is_some();
                match setup_surface(&gpu, &surface, own_renderer) {
                    Ok(configured) => {
                        if self.poll_device_for_terminal_loss(app) {
                            drop((surface, configured));
                            if !already_own && let Some(latch) = &self.gpu_loss_latch {
                                gpu.detach_terminal_latch(latch);
                            }
                            return None;
                        }
                        self.gpu = Some(gpu);
                        (surface, configured, had_own)
                    }
                    Err(err) => {
                        if self.is_terminal_gpu_policy() {
                            let lost = err.kind == crate::gpu::fallback::Failure::DeviceLost || gpu.is_lost();
                            if let Some(latch) = &self.gpu_loss_latch {
                                gpu.detach_terminal_latch(latch);
                            }
                            if lost && self.gpu_pool.borrow().upgrade().is_some_and(|p| Arc::ptr_eq(&p, &gpu)) {
                                *self.gpu_pool.borrow_mut() = Weak::default();
                            }
                            drop((surface, gpu));
                            let kind = if lost {
                                crate::render::loss_latch::GpuFailureKind::DeviceLost
                            } else {
                                crate::render::loss_latch::GpuFailureKind::SurfaceFailed
                            };
                            self.latch_terminal_loss_latch(app, kind, err.message);
                            return None;
                        }
                        if crate::gpu::fallback::decide(Gpu::env_fallback_policy(), err.kind)
                            != crate::gpu::fallback::Decision::MoveOn
                        {
                            if err.kind == crate::gpu::fallback::Failure::DeviceLost {
                                self.gpu = None;
                                self.renderer = None;
                                app.release_compositor_gpu();
                                *self.gpu_pool.borrow_mut() = Weak::default();
                            }
                            self.fail(app, err.message);
                            return None;
                        }
                        let instance = gpu.instance.clone();
                        let first_info = gpu.adapter.get_info();
                        let excluded = Gpu::candidate_info(0, &gpu.adapter, Some(&surface));
                        drop((surface, gpu));
                        self.gpu = None;
                        self.renderer = None;
                        app.release_compositor_gpu();
                        *self.gpu_pool.borrow_mut() = Weak::default();
                        let (gpu, surface, configured) = match Gpu::for_surface_setup_excluding(
                            instance, excluded, first_info, err.kind, err.message, features, true, make_surface,
                            |gpu, surface| setup_surface(gpu, surface, None),
                        ) {
                            Ok(made) => made,
                            Err(err) => {
                                self.fail(app, err.message);
                                return None;
                            }
                        };
                        let gpu = Arc::new(gpu);
                        *self.gpu_pool.borrow_mut() = Arc::downgrade(&gpu);
                        crate::render::surface_gpu(&gpu);
                        self.gpu = Some(gpu);
                        (surface, configured, false)
                    }
                }
            }
            None => {
                if self.is_terminal_gpu_policy() && self.gpu.is_some() {
                    // Never discard an existing Terminal overlay device to make a replacement.
                    self.latch_terminal_loss_latch(
                        app,
                        crate::render::loss_latch::GpuFailureKind::SurfaceFailed,
                        "could not create a surface on the overlay's GPU device",
                    );
                    return None;
                }
                if self.gpu.is_some() {
                    self.gpu = None;
                    self.renderer = None;
                    app.release_compositor_gpu();
                }
                let made = Gpu::for_surface_setup_with_latch(
                    self.gpu_loss_latch.as_ref(),
                    features,
                    true,
                    make_surface,
                    setup_surface,
                );
                let (gpu, surface, configured) = match made {
                    Ok(made) => made,
                    Err(err) => {
                        if self.is_terminal_gpu_policy() {
                            self.latch_terminal_loss_latch(app, crate::render::loss_latch::GpuFailureKind::InitFailed, err.message);
                        } else {
                            self.fail(app, err.message);
                        }
                        return None;
                    }
                };
                // The old renderer's buffers live on the old device; a new one was built above.
                self.renderer = None;
                let gpu = Arc::new(gpu);
                self.attach_terminal_gpu_latch(&gpu);
                *self.gpu_pool.borrow_mut() = Arc::downgrade(&gpu);
                crate::render::surface_gpu(&gpu);
                self.gpu = Some(gpu);
                if self.poll_device_for_terminal_loss(app) {
                    drop((surface, configured));
                    return None;
                }
                (surface, configured, false)
            }
        };
        let gpu = self.gpu.clone()?;
        let rt = app.rt_mut();
        if opaque_fallback {
            eprintln!("transparency unavailable for overlay {}; drawing it opaque", self.name);
        }
        let info = gpu.adapter.get_info();
        *self.shared.surface.lock().unwrap() = Some(format!("{:?} on {} ({:?}), {:?}, {:?}", info.backend, info.name, info.device_type, config.alpha_mode, config.format));
        #[cfg(target_os = "macos")]
        crate::platform::macos::pin_contents_top_left(&surface, self.macos_color_space);
        let target_format = if rt.srgb_blend() { config.format.remove_srgb_suffix() } else { config.format };
        crate::window::rebuild_renderer_core(&gpu.device, &mut renderer, target_format, rt);
        self.renderer = Some(renderer);
        if !reused_renderer {
            rt.text.atlas.invalidate();
            rt.images.invalidate();
        }
        // Core Animation composites premultiplied pixels even in wgpu's PostMultiplied mode;
        // other straight-alpha surfaces get the conversion pass.
        let alpha = (config.alpha_mode == wgpu::CompositeAlphaMode::PostMultiplied && gpu.adapter.get_info().backend != wgpu::Backend::Metal)
            .then(|| crate::render::surface_alpha::AlphaSurface::new(&gpu.device, target_format, (config.width, config.height)));
        rt.transparent = true;
        rt.set_renderer_kind(crate::RendererKind::Gpu);
        rt.set_viewport(self.at.size, scale);
        #[cfg(target_os = "macos")]
        let pacing = self.frame_wake.clone().and_then(|wake| {
            let Backing::Window(window) = &backing;
            crate::platform::display_link::DisplayLink::new(window, wake)
        });
        #[cfg(target_os = "macos")]
        let fn_key = match &backing {
            Backing::Window(window) if rt.modifier_keys() => crate::input::fn_key::FnKey::new(window),
            _ => None,
        };
        #[cfg(target_os = "macos")]
        let media_keys = match &backing {
            Backing::Window(window) if rt.media_keys() => {
                let shared = self.shared.clone();
                crate::input::media_keys::MediaKeys::new(window, Arc::new(move || shared.wake()))
            }
            _ => None,
        };
        #[cfg(all(target_os = "windows", feature = "windows-system-material"))]
        if let Some(h) = &handover && let Err(error) = h.commit() {
            eprintln!("rune: overlay {} DirectComposition commit failed: {error}", self.name);
        }
        Some(Live {
            #[cfg(target_os = "macos")]
            pacing,
            #[cfg(target_os = "macos")]
            materials: crate::system_material::macos::Materials::new(),
            #[cfg(target_os = "macos")]
            fn_key,
            #[cfg(target_os = "macos")]
            media_keys,
            #[cfg(target_os = "macos")]
            present: None,
            #[cfg(all(target_os = "windows", feature = "windows-system-material"))]
            materials: crate::system_material::windows::Materials::new(),
            #[cfg(all(target_os = "windows", feature = "windows-system-material"))]
            handover,
            // A new swap chain holds none of ours, and its waitable object is its own.
            #[cfg(target_os = "windows")]
            slot: Slot::default(),
            #[cfg(target_os = "windows")]
            poll_pace: crate::window::poll_pace::PollPace::default(),
            #[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
            wayland_backdrop: None,
            a11y, alpha, surface, config, backing, scale,
        })
    }

    pub(super) fn rescale(&mut self, app: &mut dyn OverlayApp, gpu: &Gpu, scale: f32) {
        self.assert_no_live_frame_texture();
        let Some(live) = self.surface.live_mut() else { return };
        if (live.scale - scale).abs() < f32::EPSILON {
            return;
        }
        live.scale = scale;
        let max_dim = gpu.device.limits().max_texture_dimension_2d;
        (live.config.width, live.config.height) = overlay_rescale_size(self.at.size, scale, max_dim);
        live.settle();
        // DX12 `configure` calls `ResizeBuffers` on the same swap chain, preserving its waitable
        // semaphore and count; on failure `surface_refused` -> `release` resets the slot.
        if let Err(error) = crate::window::configure(&live.surface, gpu, &live.config) {
            let msg = format!("could not resize its GPU surface: {error}");
            if self.is_terminal_gpu_policy() {
                self.latch_terminal_loss_latch(app, crate::render::loss_latch::GpuFailureKind::SurfaceFailed, msg);
            } else {
                self.surface_refused(app, msg);
            }
            return;
        }
        #[cfg(target_os = "macos")]
        crate::platform::macos::sync_contents_scale(&live.surface, scale);
        if let Some(alpha) = &mut live.alpha {
            alpha.resize(&gpu.device, (live.config.width, live.config.height));
        }
        app.rt_mut().set_viewport(self.at.size, scale);
        self.publish_screen_transform(app.rt());
        self.dirty = true;
        #[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
        self.sync_window_backdrop(app.rt_mut(), false);
    }
}

/// Physical drawable size for an overlay rescale, clamped to `1..=max_dim`.
pub(crate) fn overlay_rescale_size(size: (f32, f32), scale: f32, max_dim: u32) -> (u32, u32) {
    let max_dim = max_dim.max(1);
    let clamp = |v: f32| ((v * scale).round().max(1.0) as u32).clamp(1, max_dim);
    (clamp(size.0), clamp(size.1))
}

impl Drop for OverlayCore {
    fn drop(&mut self) {
        self.prepare_clean_gpu_drop();
        self.close_tokens();
        self.clear_screen_transform();
        self.drop_injected_pointers();
        *self.shared.native.lock().unwrap() = Default::default();
        self.set_availability(Availability::Unavailable);
        self.release();
        // Drop the renderer before polling a shared device so finished work retires its
        // textures and buffers now without waiting on the UI thread.
        self.renderer.take();
        let shared = self.gpu.take().filter(|gpu| Arc::strong_count(gpu) > 1)
            .or_else(|| self.retire.take_gpu());
        if let Some(gpu) = shared && !gpu.is_lost() {
            rune_gpu::health::poll(&gpu.device, wgpu::PollType::Poll).ok();
        }
    }
}

impl OverlayCore {
    /// Its surface refused a new configuration: the overlay hides and drops it, so wgpu never
    /// panics on its next drawable. The next show tries a new one.
    pub(super) fn surface_refused(&mut self, app: &mut dyn OverlayApp, reason: String) {
        self.drain_tokens_none();
        self.hide(app);
        self.release();
        #[cfg(all(target_os = "windows", feature = "windows-system-material"))]
        app.rt_mut().paint.set_windows_acrylic_materials(false);
        self.fail(app, reason);
    }

    /// Drops the surface and every native owner of its window; the next show makes new ones.
    pub(super) fn release(&mut self) {
        self.assert_no_live_frame_texture();
        // The surface must disappear before returning the adopted view or removing subclasses.
        // Keep winit's Window alive until after those native owners have unwound, too.
        let backing = match self.surface.release() {
            #[cfg_attr(not(target_os = "macos"), allow(unused_mut))]
            Some(SurfaceEffect::Live(mut live)) => {
                // The present worker joins, and the display link stops, before the surface goes.
                #[cfg(target_os = "macos")]
                { live.present.take(); live.pacing.take(); }
                #[cfg(all(target_os = "windows", feature = "windows-system-material"))]
                live.materials.teardown(live.handover.as_mut());
                // The wait for the swap chain's room is unregistered before the surface drops.
                #[cfg(target_os = "windows")]
                live.slot.reset();
                let Live {
                    #[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
                    wayland_backdrop,
                    #[cfg(all(target_os = "windows", feature = "windows-system-material"))]
                    materials,
                    #[cfg(all(target_os = "windows", feature = "windows-system-material"))]
                    handover,
                    alpha, surface, backing, ..
                } = live;
                #[cfg(all(target_os = "windows", feature = "windows-system-material"))]
                drop(materials);
                // Destroy per-surface Wayland blur proxy strictly before `backing`'s `wl_surface`.
                #[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
                drop(wayland_backdrop);
                drop(alpha);
                drop(surface);
                #[cfg(all(target_os = "windows", feature = "windows-system-material"))]
                drop(handover);
                Some(backing)
            }
            Some(SurfaceEffect::Parked(backing)) => Some(backing),
            None => None,
        };
        #[cfg(target_os = "macos")]
        self.panel.take();
        #[cfg(target_os = "windows")]
        {
            self.release_cpu();
            self.native.take();
            self.backdrop_tracker = Default::default();
        }
        #[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
        self.x11.take();
        drop(backing);
        self.wayland_resize.step(super::sizing::WaylandResizeEvent::Reset);
        self.position_owner.step(input::PositionEvent::WindowReleased);
        self.clear_screen_transform();
        self.shared.native.lock().unwrap().0 = None;
    }
}
