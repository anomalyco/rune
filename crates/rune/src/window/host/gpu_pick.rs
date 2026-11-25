//! The GPU and surface a new window draws with (`Host::try_create`): the window's own GPU from
//! before it closed, the shared one, or one picked for it, with the fallback chain's next adapter
//! when one can't set the surface up.

use std::sync::{Arc, Weak};

use super::Host;
use crate::render::{Gpu, Renderer, SurfaceSetupError, format_window_setup_error};

impl Host {
    /// The GPU, surface and `setup_surface`'s result for a new window. `Err`: no GPU could draw
    /// it, with why.
    pub(super) fn pick_surface_gpu<'w, T>(
        &mut self,
        features: wgpu::Features,
        transparent: bool,
        make_surface: impl Fn(&wgpu::Instance) -> Result<wgpu::Surface<'w>, String>,
        setup_surface: impl Fn(&Gpu, &wgpu::Surface<'w>, Option<Renderer>) -> Result<T, SurfaceSetupError>,
    ) -> Result<(Arc<Gpu>, wgpu::Surface<'w>, T), String> {
        // A device that died while this window was closed draws nothing: release it and any
        // compositor resources on it before making another (D3D12 keeps a removed device alive per
        // adapter until every resource made on it is dropped).
        if self.gpu.as_ref().is_some_and(|(gpu, _)| gpu.is_lost()) {
            self.gpu = None;
            self.app.release_compositor_gpu();
        }
        Ok(match self.gpu.take() {
            Some((gpu, renderer)) => {
                let surface = match make_surface(&gpu.instance) {
                    Ok(surface) => surface,
                    Err(error) => {
                        drop((renderer, gpu));
                        self.app.release_compositor_gpu();
                        return Err(error);
                    }
                };
                match setup_surface(&gpu, &surface, Some(renderer)) {
                    Ok(configured) => (gpu, surface, configured),
                    Err(err) => {
                        use crate::gpu::fallback::Decision;
                        match crate::gpu::fallback::decide(Gpu::env_fallback_policy(), err.kind) {
                            Decision::Stop => {
                                drop((surface, gpu));
                                self.app.release_compositor_gpu();
                                if err.kind == crate::gpu::fallback::Failure::DeviceLost {
                                    *self.gpu_pool.borrow_mut() = Weak::default();
                                }
                                return Err(format_window_setup_error(err));
                            }
                            Decision::MoveOn => {
                                let instance = gpu.instance.clone();
                                let first_info = gpu.adapter.get_info();
                                let excluded = Gpu::candidate_info(0, &gpu.adapter, Some(&surface));
                                drop((surface, gpu));
                                self.app.release_compositor_gpu();
                                *self.gpu_pool.borrow_mut() = Weak::default();
                                let (gpu, surface, configured) = Gpu::for_surface_setup_excluding(
                                    instance, excluded, first_info, err.kind, err.message, features, transparent, &make_surface,
                                    |gpu, surface| setup_surface(gpu, surface, None),
                                ).map_err(format_window_setup_error)?;
                                let gpu = Arc::new(gpu);
                                *self.gpu_pool.borrow_mut() = Arc::downgrade(&gpu);
                                crate::render::surface_gpu(&gpu);
                                (gpu, surface, configured)
                            }
                        }
                    }
                }
            }
            None => {
                // A shared device that can't composite this window's translucent pixels gets its own.
                let cached = self.gpu_pool.borrow().upgrade().filter(|gpu| !gpu.is_lost()).and_then(|gpu| {
                    let surface = make_surface(&gpu.instance).ok()?;
                    crate::render::translucency::serves(&gpu, &surface, features, transparent).then_some((gpu, surface))
                });
                if let Some((gpu, surface)) = cached {
                    match setup_surface(&gpu, &surface, None) {
                        Ok(configured) => (gpu, surface, configured),
                        Err(err) => {
                            use crate::gpu::fallback::Decision;
                            match crate::gpu::fallback::decide(Gpu::env_fallback_policy(), err.kind) {
                                Decision::Stop => {
                                    if err.kind == crate::gpu::fallback::Failure::DeviceLost {
                                        *self.gpu_pool.borrow_mut() = Weak::default();
                                    }
                                    return Err(format_window_setup_error(err));
                                }
                                Decision::MoveOn => {
                                    let instance = gpu.instance.clone();
                                    let first_info = gpu.adapter.get_info();
                                    let excluded = Gpu::candidate_info(0, &gpu.adapter, Some(&surface));
                                    drop((surface, gpu));
                                    *self.gpu_pool.borrow_mut() = Weak::default();
                                    let (gpu, surface, configured) = Gpu::for_surface_setup_excluding(
                                        instance, excluded, first_info, err.kind, err.message, features, transparent, &make_surface,
                                        |gpu, surface| setup_surface(gpu, surface, None),
                                    ).map_err(format_window_setup_error)?;
                                    let gpu = Arc::new(gpu);
                                    *self.gpu_pool.borrow_mut() = Arc::downgrade(&gpu);
                                    crate::render::surface_gpu(&gpu);
                                    (gpu, surface, configured)
                                }
                            }
                        }
                    }
                } else {
                    let (gpu, surface, configured) = Gpu::for_surface_setup(
                        features, transparent, &make_surface, setup_surface,
                    ).map_err(format_window_setup_error)?;
                    let gpu = Arc::new(gpu);
                    *self.gpu_pool.borrow_mut() = Arc::downgrade(&gpu);
                    crate::render::surface_gpu(&gpu);
                    (gpu, surface, configured)
                }
            }
        })
    }
}
