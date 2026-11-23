//! Which backend an Apple app draws with: Rune's native Metal backend (`metal-backend`, on by
//! default, and the default on macOS), or wgpu's.

use std::sync::atomic::{AtomicU8, Ordering};

/// The GPU backend an app draws with on macOS (`App::gpu_backend`) and iOS (`prewarm_gpu_on`, before
/// the first `embed::LayerHost`). Elsewhere wgpu picks one.
///
/// A Mac app that chooses neither draws with `Metal` when Rune's `metal-backend` feature is on (it
/// is by default), and with `Wgpu` when it's off; an iOS app draws with `Wgpu`. `Default` stays `Wgpu`; passing it to
/// `App::gpu_backend` opts out of the native backend.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum GpuBackend {
    /// wgpu's Metal backend.
    #[default]
    Wgpu,
    /// Rune's native Metal backend, with Rune's `metal-backend` feature; without it, wgpu.
    Metal,
}

const UNSET: u8 = 0;
const WGPU: u8 = 1;
const METAL: u8 = 2;

/// What the app chose: nothing yet, `Wgpu` or `Metal`.
#[cfg_attr(not(all(target_vendor = "apple", feature = "metal-backend")), allow(dead_code))]
static CHOSEN: AtomicU8 = AtomicU8::new(UNSET);

/// The backend an app that chooses none draws with: the native one on macOS with the feature.
const DEFAULT: GpuBackend = if cfg!(all(target_os = "macos", feature = "metal-backend")) { GpuBackend::Metal } else { GpuBackend::Wgpu };

/// Makes GPUs from now on with `backend`, process-wide. Returns `true` when the active backend
/// changed, so a caller with a prewarm running on the old backend can start it over.
#[cfg_attr(not(target_vendor = "apple"), allow(unused_variables))]
pub fn choose(backend: GpuBackend) -> bool {
    #[cfg(all(target_vendor = "apple", not(feature = "metal-backend")))]
    if backend == GpuBackend::Metal {
        eprintln!("rune: GpuBackend::Metal needs Rune's `metal-backend` feature; drawing with wgpu");
    }
    #[cfg(all(target_vendor = "apple", feature = "metal-backend"))]
    {
        let before = current();
        CHOSEN.store(match backend { GpuBackend::Wgpu => WGPU, GpuBackend::Metal => METAL }, Ordering::Relaxed);
        return current() != before;
    }
    #[cfg(not(all(target_vendor = "apple", feature = "metal-backend")))]
    false
}

fn chosen() -> Option<GpuBackend> {
    match CHOSEN.load(Ordering::Relaxed) {
        WGPU => Some(GpuBackend::Wgpu),
        METAL => Some(GpuBackend::Metal),
        _ => None,
    }
}

fn env() -> Option<String> {
    rune_core::env::var("RUNE_GPU_BACKEND")
}

/// The backend GPUs are made with: `RUNE_GPU_BACKEND` (`wgpu` or `metal`) over the app's
/// choice, over the default.
pub fn current() -> GpuBackend {
    let backend = gate(resolve(env().as_deref(), chosen()), os_major());
    #[cfg(all(target_vendor = "apple", feature = "metal-backend"))]
    if backend == GpuBackend::Wgpu && resolve(env().as_deref(), chosen()) == GpuBackend::Metal && !defaulted(env().as_deref(), chosen()) {
        static SAID: std::sync::Once = std::sync::Once::new();
        SAID.call_once(|| eprintln!("rune: Rune's Metal backend needs macOS {MACOS_FLOOR} or iOS {IOS_FLOOR}; drawing with wgpu"));
    }
    backend
}

/// The oldest macOS and iOS the native backend runs on: it writes MSL 3.0, and its precompiled
/// libraries are built for macOS 13 and iOS 16.
const MACOS_FLOOR: u32 = 13;
const IOS_FLOOR: u32 = 16;

/// `backend`, or wgpu when it is the native one and the OS (`os_major`) is older than it runs on.
/// An unknown version (`None`) keeps `backend`.
fn gate(backend: GpuBackend, os_major: Option<u32>) -> GpuBackend {
    let floor = if cfg!(target_os = "ios") { IOS_FLOOR } else { MACOS_FLOOR };
    match os_major {
        Some(major) if backend == GpuBackend::Metal && major < floor => GpuBackend::Wgpu,
        _ => backend,
    }
}

/// The OS's major version (`kern.osproductversion`), read once.
#[cfg(all(target_vendor = "apple", feature = "metal-backend"))]
fn os_major() -> Option<u32> {
    static MAJOR: std::sync::OnceLock<Option<u32>> = std::sync::OnceLock::new();
    *MAJOR.get_or_init(|| {
        let mut buf = [0u8; 32];
        let mut len = buf.len();
        // SAFETY: sysctlbyname writes at most `len` bytes into `buf` and the length it wrote into `len`.
        #[allow(unsafe_code)]
        let ok = unsafe { libc::sysctlbyname(c"kern.osproductversion".as_ptr(), buf.as_mut_ptr().cast(), &mut len, std::ptr::null_mut(), 0) } == 0;
        if !ok { return None; }
        String::from_utf8_lossy(&buf[..len.saturating_sub(1)]).split('.').next()?.trim().parse().ok()
    })
}

#[cfg(not(all(target_vendor = "apple", feature = "metal-backend")))]
fn os_major() -> Option<u32> { None }

/// What `RUNE_GPU_LOG` calls the backend `adapter` is on.
#[cfg_attr(target_arch = "wasm32", allow(dead_code))]
pub(crate) fn label(adapter: &wgpu::Adapter) -> &'static str {
    #[cfg(all(target_vendor = "apple", feature = "metal-backend"))]
    if crate::metal::owns_adapter(adapter) {
        return "rune backend";
    }
    let _ = adapter;
    "wgpu"
}

/// Whether `device` is the native Metal backend's.
pub fn native(device: &wgpu::Device) -> bool {
    #[cfg(all(target_vendor = "apple", feature = "metal-backend"))]
    return crate::metal::raw_device(device).is_some();
    #[cfg(not(all(target_vendor = "apple", feature = "metal-backend")))]
    {
        let _ = device;
        false
    }
}

/// Features a device on `adapter` gets whether or not a compositor asks: all the native backend
/// offers, which cost it nothing, so a prewarmed device serves any compositor it can draw for.
pub fn free_features(adapter: &wgpu::Adapter) -> wgpu::Features {
    #[cfg(all(target_vendor = "apple", feature = "metal-backend"))]
    if crate::metal::owns_adapter(adapter) {
        return crate::metal::FEATURES;
    }
    let _ = adapter;
    // Dmabuf frames from other processes (`native_surface::linux`): extensions the device enables
    // whenever the driver has them; the feature only lets wgpu import with them.
    #[cfg(target_os = "linux")]
    return wgpu::Features::VULKAN_EXTERNAL_MEMORY_DMA_BUF;
    #[cfg(not(target_os = "linux"))]
    wgpu::Features::empty()
}

/// The features the last compositor that fell back to wgpu wanted and the native backend lacks.
#[cfg_attr(not(all(target_vendor = "apple", feature = "metal-backend")), allow(dead_code))]
static MISSING: std::sync::Mutex<wgpu::Features> = std::sync::Mutex::new(wgpu::Features::empty());

/// Whether a GPU for a compositor that wants `wanted` draws with wgpu instead of the native
/// backend: only when the native backend is the default, neither the app nor the environment
/// asked for it, and it lacks one of `wanted`'s features. Says so once.
#[cfg_attr(not(all(target_vendor = "apple", feature = "metal-backend")), allow(unused_variables))]
pub fn wgpu_for(wanted: wgpu::Features) -> bool {
    #[cfg(all(target_vendor = "apple", feature = "metal-backend"))]
    {
        let missing = wanted - crate::metal::FEATURES;
        if !missing.is_empty() && DEFAULT == GpuBackend::Metal && defaulted(env().as_deref(), chosen()) {
            *MISSING.lock().unwrap_or_else(|e| e.into_inner()) = missing;
            static SAID: std::sync::Once = std::sync::Once::new();
            SAID.call_once(|| eprintln!("rune: the compositor wants {missing}, which Rune's Metal backend lacks; drawing with wgpu"));
            return true;
        }
    }
    false
}

/// Which backend draws on `device`, on which GPU, and why, for an app's log: "Rune's Metal
/// backend on Apple M4 (the default)", or "wgpu Metal on Apple M4 (a compositor wants
/// POLYGON_MODE_LINE, which Rune's Metal backend lacks)". A window's compositor gets its device in
/// `compose::Frame::device`. `RUNE_GPU_LOG=1` prints it for each window and overlay GPU Rune makes.
pub fn describe(device: &wgpu::Device) -> String {
    let info = device.adapter_info();
    let backend = if native(device) { "Rune's Metal backend".to_string() } else { format!("wgpu {:?}", info.backend) };
    let why = reason(native(device));
    if why.is_empty() { format!("{backend} on {}", info.name) } else { format!("{backend} on {} ({why})", info.name) }
}

/// Why the backend is the native one (`native`) or wgpu's; empty where wgpu is the only one.
fn reason(native: bool) -> String {
    #[cfg(all(target_vendor = "apple", feature = "metal-backend"))]
    {
        let (env, chosen) = (env(), chosen());
        let missing = *MISSING.lock().unwrap_or_else(|e| e.into_inner());
        return match (native, env.as_deref(), chosen) {
            (true, Some("metal"), _) => "RUNE_GPU_BACKEND=metal".into(),
            (true, _, Some(GpuBackend::Metal)) => "App::gpu_backend chose it".into(),
            (true, ..) => "the default".into(),
            (false, Some("wgpu"), _) => "RUNE_GPU_BACKEND=wgpu".into(),
            (false, _, Some(GpuBackend::Wgpu)) => "App::gpu_backend chose it".into(),
            (false, ..) if DEFAULT == GpuBackend::Wgpu => "the default here".into(),
            (false, ..) if gate(GpuBackend::Metal, os_major()) == GpuBackend::Wgpu => format!("Rune's Metal backend needs macOS {MACOS_FLOOR} or iOS {IOS_FLOOR}"),
            (false, ..) if !missing.is_empty() => format!("a compositor wants {missing}, which Rune's Metal backend lacks"),
            (false, ..) => String::new(),
        };
    }
    #[cfg(all(target_vendor = "apple", not(feature = "metal-backend")))]
    return if native { String::new() } else { "Rune's metal-backend feature is off".into() };
    #[cfg(not(target_vendor = "apple"))]
    {
        let _ = native;
        String::new()
    }
}

/// Whether a GPU on `device` serves a compositor that wants `wanted`: false for a native
/// backend device when `wgpu_for(wanted)`.
pub fn serves(device: &wgpu::Device, wanted: wgpu::Features) -> bool {
    #[cfg(all(target_vendor = "apple", feature = "metal-backend"))]
    if crate::metal::raw_device(device).is_some() {
        return !wgpu_for(wanted);
    }
    let _ = (device, wanted);
    true
}

#[cfg_attr(not(all(target_vendor = "apple", feature = "metal-backend")), allow(dead_code))]
fn resolve(env: Option<&str>, chosen: Option<GpuBackend>) -> GpuBackend {
    match env {
        Some("metal") => GpuBackend::Metal,
        Some("wgpu") => GpuBackend::Wgpu,
        _ => chosen.unwrap_or(DEFAULT),
    }
}

/// Whether the backend is the default one, which neither the environment nor the app chose.
#[cfg_attr(not(all(target_vendor = "apple", feature = "metal-backend")), allow(dead_code))]
fn defaulted(env: Option<&str>, chosen: Option<GpuBackend>) -> bool {
    !matches!(env, Some("metal" | "wgpu")) && chosen.is_none()
}

#[cfg(test)]
mod tests {
    use super::{DEFAULT, GpuBackend, defaulted, gate, resolve};

    #[test]
    fn the_native_backend_falls_back_to_wgpu_below_its_os_floor() {
        let floor = if cfg!(target_os = "ios") { 16 } else { 13 };
        assert_eq!(gate(GpuBackend::Metal, Some(floor - 2)), GpuBackend::Wgpu, "macOS 11 / iOS 14");
        assert_eq!(gate(GpuBackend::Metal, Some(floor - 1)), GpuBackend::Wgpu, "macOS 12 / iOS 15");
        assert_eq!(gate(GpuBackend::Metal, Some(floor)), GpuBackend::Metal);
        assert_eq!(gate(GpuBackend::Metal, Some(26)), GpuBackend::Metal);
        assert_eq!(gate(GpuBackend::Metal, None), GpuBackend::Metal, "an unknown version keeps the choice");
        assert_eq!(gate(GpuBackend::Wgpu, Some(11)), GpuBackend::Wgpu);
    }

    #[test]
    fn the_environment_overrides_the_app() {
        assert_eq!(resolve(None, Some(GpuBackend::Metal)), GpuBackend::Metal);
        assert_eq!(resolve(Some("wgpu"), Some(GpuBackend::Metal)), GpuBackend::Wgpu);
        assert_eq!(resolve(Some("metal"), Some(GpuBackend::Wgpu)), GpuBackend::Metal);
        assert_eq!(resolve(Some(""), Some(GpuBackend::Metal)), GpuBackend::Metal);
    }

    #[test]
    fn an_app_that_chooses_wgpu_keeps_it_over_the_default() {
        assert_eq!(resolve(None, Some(GpuBackend::Wgpu)), GpuBackend::Wgpu);
        assert_eq!(resolve(None, None), DEFAULT);
        assert_eq!(resolve(Some("bogus"), None), DEFAULT);
        assert!(defaulted(None, None));
        assert!(!defaulted(None, Some(GpuBackend::Metal)));
        assert!(!defaulted(Some("metal"), None));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_draws_with_the_native_backend_by_default_with_the_feature() {
        let expected = if cfg!(feature = "metal-backend") { GpuBackend::Metal } else { GpuBackend::Wgpu };
        assert_eq!(DEFAULT, expected);
    }
}
