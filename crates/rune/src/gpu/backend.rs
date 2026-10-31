//! Which backend a Mac app draws with: wgpu's, or Rune's native Metal backend (`metal-backend`).

use std::sync::atomic::{AtomicBool, Ordering};

/// The GPU backend an app draws with on macOS (`App::gpu_backend`). Elsewhere wgpu picks one.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum GpuBackend {
    /// wgpu's Metal backend.
    #[default]
    Wgpu,
    /// Rune's native Metal backend, with Rune's `metal-backend` feature; without it, wgpu.
    Metal,
}

#[cfg_attr(not(all(target_os = "macos", feature = "metal-backend")), allow(dead_code))]
static METAL: AtomicBool = AtomicBool::new(false);

/// Makes GPUs from now on with `backend`, process-wide. A `prewarm_gpu` running on another
/// backend starts over on this one.
#[cfg_attr(not(target_os = "macos"), allow(unused_variables))]
pub(crate) fn choose(backend: GpuBackend) {
    #[cfg(all(target_os = "macos", not(feature = "metal-backend")))]
    if backend == GpuBackend::Metal {
        eprintln!("rune: GpuBackend::Metal needs Rune's `metal-backend` feature; drawing with wgpu");
    }
    #[cfg(all(target_os = "macos", feature = "metal-backend"))]
    {
        let before = current();
        METAL.store(backend == GpuBackend::Metal, Ordering::Relaxed);
        if current() != before {
            crate::render::restart_prewarm();
        }
    }
}

#[cfg_attr(not(all(target_os = "macos", feature = "metal-backend")), allow(dead_code))]
/// The backend GPUs are made with: `RUNE_GPU_BACKEND` (`wgpu` or `metal`) over the app's choice.
pub(crate) fn current() -> GpuBackend {
    let chosen = if METAL.load(Ordering::Relaxed) { GpuBackend::Metal } else { GpuBackend::Wgpu };
    resolve(std::env::var("RUNE_GPU_BACKEND").ok().as_deref(), chosen)
}

#[cfg_attr(not(all(target_os = "macos", feature = "metal-backend")), allow(dead_code))]
fn resolve(env: Option<&str>, chosen: GpuBackend) -> GpuBackend {
    match env {
        Some("metal") => GpuBackend::Metal,
        Some("wgpu") => GpuBackend::Wgpu,
        _ => chosen,
    }
}

#[cfg(test)]
mod tests {
    use super::{GpuBackend, resolve};

    #[test]
    fn the_environment_overrides_the_app() {
        assert_eq!(resolve(None, GpuBackend::Metal), GpuBackend::Metal);
        assert_eq!(resolve(Some("wgpu"), GpuBackend::Metal), GpuBackend::Wgpu);
        assert_eq!(resolve(Some("metal"), GpuBackend::Wgpu), GpuBackend::Metal);
        assert_eq!(resolve(Some(""), GpuBackend::Metal), GpuBackend::Metal);
    }
}
