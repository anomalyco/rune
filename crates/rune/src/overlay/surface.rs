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

impl<T: 'static> Overlay<T> {
    /// Creates the wgpu surface (and the GPU device, if this is the first surface).
    pub(super) fn attach(&mut self, backing: Backing, scale: f32) -> Option<Live> {
        // First, before the surface binds the view (macOS may observe it).
        let a11y = self.shared.wake.lock().unwrap().clone().and_then(|wake| match &backing {
            Backing::Window(window) => crate::a11y::Adapter::new(window, wake),
            #[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
            Backing::Layer(_) | Backing::Xwayland(_) => crate::a11y::Adapter::windowless(wake),
        });
        #[cfg(target_os = "macos")]
        let moved = self.panel.as_ref().map(|p| p.raw_handles());
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
        let features = self.compositor.as_ref().map_or(wgpu::Features::empty(), |c| c.features());
        // A device that died while the overlay was hidden (a driver reset, a resume from
        // Modern Standby) can configure nothing: the next surface gets a new one.
        if self.gpu.as_ref().is_some_and(|gpu| gpu.is_lost()) {
            self.gpu = None;
            self.renderer = None;
        }
        // The overlay's own device was already the best for its surface, translucent or not; a
        // device shared from another overlay must composite this one's translucent pixels.
        let pooled = self.gpu.clone().map(|gpu| (gpu, false)).or_else(|| self.gpu_pool.borrow().upgrade().filter(|gpu| {
            !gpu.is_lost() && gpu.device.features().contains(features & gpu.adapter.features())
        }).map(|gpu| (gpu, true)));
        let reused = pooled.and_then(|(gpu, translucent)| make(&gpu.instance).filter(|s| crate::render::translucency::serves(&gpu, s, features, translucent)).map(|s| (gpu, s)));
        let mut prewarmed = None;
        let surface = match reused {
            Some((gpu, surface)) => { self.gpu = Some(gpu); surface }
            None => {
                let made = Gpu::for_surface(features, true, |instance| make(instance).ok_or_else(|| "could not create its GPU surface".to_owned()));
                let (gpu, surface, renderer) = match made {
                    Ok(made) => made,
                    Err(error) => {
                        self.fail(error);
                        return None;
                    }
                };
                prewarmed = renderer;
                // The old renderer's buffers live on the old device; a new one is made below.
                self.renderer = None;
                let gpu = Arc::new(gpu);
                *self.gpu_pool.borrow_mut() = Arc::downgrade(&gpu);
                crate::render::surface_gpu(&gpu);
                self.gpu = Some(gpu);
                surface
            }
        };
        let gpu = self.gpu.clone()?;
        let (w, h) = ((self.at.size.0 * scale).round() as u32, (self.at.size.1 * scale).round() as u32);
        let (mut config, translucent) = crate::window::surface_config(&surface, &gpu.adapter, (w, h), self.rt.srgb_blend())?;
        match translucent {
            Some(mode) => config.alpha_mode = mode,
            // Windows composites an overlay's pixels only through DirectComposition; drawn
            // opaque on its window they never show.
            None if cfg!(target_os = "windows") => {
                let info = gpu.adapter.get_info();
                self.fail(format!("{:?} on {} can't composite transparent pixels; Windows overlays need DX12 with DirectComposition", info.backend, info.name));
                return None;
            }
            None => eprintln!("transparency unavailable for overlay {}; drawing it opaque", self.name),
        }
        let info = gpu.adapter.get_info();
        *self.shared.surface.lock().unwrap() = Some(format!("{:?} on {} ({:?}), {:?}, {:?}", info.backend, info.name, info.device_type, config.alpha_mode, config.format));
        // A surface the driver refuses costs this overlay alone, never the app's other windows
        // (hex#979). The surface drops before its backing.
        if let Err(error) = crate::window::configure(&surface, &gpu, &config) {
            self.fail(format!("could not configure its GPU surface on {:?} {}: {error}", info.backend, info.name));
            return None;
        }
        #[cfg(target_os = "macos")]
        crate::platform::macos::pin_contents_top_left(&surface);
        let target_format = if self.rt.srgb_blend() { config.format.remove_srgb_suffix() } else { config.format };
        if let Some(renderer) = &mut self.renderer {
            crate::window::rebuild_renderer(&gpu.device, renderer, target_format, &mut self.rt);
        } else {
            let mut renderer = prewarmed.unwrap_or_else(|| Renderer::new(&gpu.device, target_format));
            crate::window::rebuild_renderer(&gpu.device, &mut renderer, target_format, &mut self.rt);
            self.renderer = Some(renderer);
            self.rt.text.atlas.invalidate();
            self.rt.images.invalidate();
        }
        // Core Animation composites premultiplied pixels even in wgpu's PostMultiplied mode;
        // other straight-alpha surfaces get the conversion pass.
        let alpha = (config.alpha_mode == wgpu::CompositeAlphaMode::PostMultiplied && gpu.adapter.get_info().backend != wgpu::Backend::Metal)
            .then(|| crate::render::surface_alpha::AlphaSurface::new(&gpu.device, target_format, (config.width, config.height)));
        self.rt.transparent = true;
        self.rt.set_viewport(self.at.size, scale);
        #[cfg(target_os = "macos")]
        let pacing = self.frame_wake.clone().and_then(|wake| {
            let Backing::Window(window) = &backing;
            crate::platform::display_link::DisplayLink::new(window, wake)
        });
        #[cfg(target_os = "macos")]
        let fn_key = match &backing {
            Backing::Window(window) if self.rt.modifier_keys() => crate::input::fn_key::FnKey::new(window),
            _ => None,
        };
        Some(Live {
            #[cfg(target_os = "macos")]
            pacing,
            #[cfg(target_os = "macos")]
            materials: crate::system_material::macos::Materials::new(),
            #[cfg(target_os = "macos")]
            fn_key,
            #[cfg(target_os = "macos")]
            present: None,
            a11y, alpha, surface, config, backing, scale,
        })
    }

    pub(super) fn rescale(&mut self, gpu: &Gpu, scale: f32) {
        let Some(live) = self.surface.live_mut() else { return };
        if (live.scale - scale).abs() < f32::EPSILON {
            return;
        }
        live.scale = scale;
        live.config.width = (self.at.size.0 * scale).round().max(1.0) as u32;
        live.config.height = (self.at.size.1 * scale).round().max(1.0) as u32;
        live.settle();
        if let Err(error) = crate::window::configure(&live.surface, &gpu, &live.config) {
            self.surface_refused(format!("could not resize its GPU surface: {error}"));
            return;
        }
        if let Some(alpha) = &mut live.alpha {
            alpha.resize(&gpu.device, (live.config.width, live.config.height));
        }
        self.rt.set_viewport(self.at.size, scale);
        self.dirty = true;
    }
}

impl<T: 'static> Drop for Overlay<T> {
    fn drop(&mut self) {
        *self.shared.native.lock().unwrap() = Default::default();
        self.set_availability(Availability::Unavailable);
        self.release();
    }
}

impl<T: 'static> Overlay<T> {
    /// Its surface refused a new configuration: the overlay hides and drops it, so wgpu never
    /// panics on its next drawable. The next show tries a new one.
    pub(super) fn surface_refused(&mut self, reason: String) {
        self.hide();
        self.release();
        self.fail(reason);
    }

    /// Drops the surface and every native owner of its window; the next show makes new ones.
    pub(super) fn release(&mut self) {
        // The surface must disappear before returning the adopted view or removing subclasses.
        // Keep winit's Window alive until after those native owners have unwound, too.
        let backing = match self.surface.release() {
            #[cfg_attr(not(target_os = "macos"), allow(unused_mut))]
            Some(SurfaceEffect::Live(mut live)) => {
                // The present worker joins, and the display link stops, before the surface goes.
                #[cfg(target_os = "macos")]
                { live.present.take(); live.pacing.take(); }
                let Live { alpha, surface, backing, .. } = live;
                drop(alpha);
                drop(surface);
                Some(backing)
            }
            Some(SurfaceEffect::Parked(backing)) => Some(backing),
            None => None,
        };
        #[cfg(target_os = "macos")]
        self.panel.take();
        #[cfg(target_os = "windows")]
        self.native.take();
        #[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
        self.x11.take();
        drop(backing);
        self.shared.native.lock().unwrap().0 = None;
    }
}
