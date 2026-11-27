//! Linux Vulkan ICD manifest discovery and direct-driver decision logic.
//!
//! On hybrid Linux laptops (e.g. Intel or AMD iGPU driving the Wayland compositor beside an
//! NVIDIA dGPU), an unlimited `wgpu::Instance` causes the Vulkan loader to `dlopen` every
//! installed ICD during `vkEnumerateInstanceExtensionProperties` / `vkCreateInstance`, which
//! keeps the discrete GPU out of runtime suspend and adds startup latency and memory.
//!
//! This module decides — via a single pure function [`decide`] — whether to try
//! `VK_LUNARG_direct_driver_loading` (exclusive mode) for the compositor's kernel driver alone,
//! or to take today's unlimited `wgpu::Instance::new` path unchanged. On any doubt (override
//! environment variable, debug/validation flags, unpatched `wgpu-hal`, missing extension, unknown
//! kernel driver, unreadable/malformed/oversized matching manifest, or no matching adapter), it
//! falls back completely to today's unlimited path.
//!
//! Manifest discovery runs at most once during instance creation (never on the UI thread during
//! frames) and enforces strict bounds on directories, directory entries, manifest files, and bytes
//! read per file.

use std::ffi::OsString;
use std::io::Read;
use std::path::{Path, PathBuf};

/// Maximum number of unique `vulkan/icd.d` directories scanned per process.
pub(crate) const MAX_MANIFEST_DIRS: usize = 32;
/// Maximum number of directory entries inspected in any single `vulkan/icd.d` directory.
pub(crate) const MAX_DIR_ENTRIES_PER_DIR: usize = 256;
/// Maximum number of matching ICD JSON manifests opened and parsed across all directories.
pub(crate) const MAX_MANIFEST_FILES: usize = 32;
/// Maximum size in bytes of any single matching ICD JSON manifest (real manifests are ~200 B).
pub(crate) const MAX_MANIFEST_BYTES: u64 = 64 * 1024;
/// Maximum number of candidate ICD library paths returned for direct loading.
pub(crate) const MAX_CANDIDATE_LIBS: usize = 8;

/// Process environment variables that explicitly override adapter, driver, or layer selection.
/// When any of these is set and non-empty, direct driver loading is bypassed completely so the
/// user's configuration takes full effect.
pub(crate) const OVERRIDE_ENV_VARS: &[&str] = &[
    "WGPU_ADAPTER_NAME",
    "WGPU_POWER_PREF",
    "VK_ICD_FILENAMES",
    "VK_DRIVER_FILES",
    "VK_ADD_DRIVER_FILES",
    "VK_LOADER_DRIVERS_SELECT",
    "VK_LOADER_DRIVERS_DISABLE",
    "VK_LOADER_SETTINGS_OVERRIDE",
    "VK_INSTANCE_LAYERS",
    "VK_LOADER_LAYERS_ENABLE",
    "VK_LOADER_LAYERS_DISABLE",
];

/// Returns `true` if any variable in [`OVERRIDE_ENV_VARS`] is set to a non-empty value.
pub(crate) fn has_override_env(lookup: impl Fn(&str) -> Option<OsString>) -> bool {
    OVERRIDE_ENV_VARS
        .iter()
        .any(|name| lookup(name).is_some_and(|val| !val.is_empty()))
}

/// Checks [`has_override_env`] against the current process environment.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub(crate) fn has_override_env_process() -> bool {
    has_override_env(rune_core::env::var_os)
}

/// Maps a Linux DRM kernel driver name (the file name of
/// `/sys/dev/char/<maj>:<min>/device/driver`) to the matching Vulkan ICD manifest filename
/// prefixes. Returns `None` for unknown drivers so Rune falls back to loading all drivers.
pub(crate) fn icd_prefixes(kernel_driver: &str) -> Option<&'static [&'static str]> {
    match kernel_driver {
        "i915" | "xe" => Some(&["intel_icd", "intel_hasvk_icd"]),
        "amdgpu" => Some(&["radeon_icd", "amd_icd", "amd_pro_icd"]),
        "nvidia" => Some(&["nvidia_icd"]),
        "nouveau" => Some(&["nouveau_icd"]),
        "msm" => Some(&["freedreno_icd"]),
        "panfrost" | "panthor" => Some(&["panfrost_icd"]),
        "v3d" => Some(&["broadcom_icd"]),
        "asahi" => Some(&["asahi_icd"]),
        "powervr" => Some(&["powervr_mesa_icd", "powervr_icd"]),
        "virtio_gpu" => Some(&["virtio_icd"]),
        _ => None,
    }
}

/// Pure inputs to [`decide`], describing whether direct Vulkan driver loading may be attempted.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct DirectDriverInput<'a> {
    /// Whether `wgpu::Backends::VULKAN` is requested.
    pub vulkan_backend: bool,
    /// Whether any debug/validation `wgpu::InstanceFlags` bit is set.
    pub debug_or_validation: bool,
    /// Whether direct driver loading is bypassed for this call (explicit unlimited request or
    /// latched after an earlier adapter/surface failure on a direct-driver instance).
    pub previously_failed: bool,
    /// Whether any variable in [`OVERRIDE_ENV_VARS`] is set.
    pub env_override: bool,
    /// Whether the vendored `wgpu-hal` direct-driver hook is compiled in.
    pub hal_hook_available: bool,
    /// Whether the Vulkan loader is eligible to support `VK_LUNARG_direct_driver_loading`.
    pub loader_extension_supported: bool,
    /// Kernel driver name of the Wayland compositor's main DRM device, if known.
    pub compositor_driver: Option<&'a str>,
    /// Candidate ICD library paths parsed from matching manifests.
    pub candidate_libraries: &'a [PathBuf],
}

/// Reason [`decide`] chose today's unlimited `wgpu::Instance::new` path.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum FallbackReason {
    NoVulkanBackend,
    DebugOrValidation,
    PreviouslyFailed,
    EnvOverride,
    UnpatchedHal,
    ExtensionAbsent,
    UnknownDriver,
    NoCandidateLibraries,
}

/// Outcome of [`decide`]: either today's unlimited instance path, or direct loading of specific
/// ICD library paths.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum DirectDriverPlan<'a> {
    /// Build the `wgpu::Instance` through today's unlimited path unchanged.
    Default(FallbackReason),
    /// Try `VK_LUNARG_direct_driver_loading` (exclusive mode) with these ICD library paths.
    Direct(&'a [PathBuf]),
}

/// Single pure decision function: chooses direct driver loading only when every precondition
/// holds, and falls back to today's unlimited path on any doubt.
pub(crate) fn decide<'a>(input: &DirectDriverInput<'a>) -> DirectDriverPlan<'a> {
    if !input.vulkan_backend {
        return DirectDriverPlan::Default(FallbackReason::NoVulkanBackend);
    }
    if input.debug_or_validation {
        return DirectDriverPlan::Default(FallbackReason::DebugOrValidation);
    }
    if input.previously_failed {
        return DirectDriverPlan::Default(FallbackReason::PreviouslyFailed);
    }
    if input.env_override {
        return DirectDriverPlan::Default(FallbackReason::EnvOverride);
    }
    if !input.hal_hook_available {
        return DirectDriverPlan::Default(FallbackReason::UnpatchedHal);
    }
    if !input.loader_extension_supported {
        return DirectDriverPlan::Default(FallbackReason::ExtensionAbsent);
    }
    if input.compositor_driver.and_then(icd_prefixes).is_none() {
        return DirectDriverPlan::Default(FallbackReason::UnknownDriver);
    }
    if input.candidate_libraries.is_empty() {
        return DirectDriverPlan::Default(FallbackReason::NoCandidateLibraries);
    }
    DirectDriverPlan::Direct(input.candidate_libraries)
}

/// Whether `flags` requests any debug or validation feature that requires full loader
/// discovery and layer support.
pub(crate) const fn has_debug_or_validation(flags: wgpu::InstanceFlags) -> bool {
    flags.intersects(
        wgpu::InstanceFlags::DEBUG
            .union(wgpu::InstanceFlags::VALIDATION)
            .union(wgpu::InstanceFlags::GPU_BASED_VALIDATION),
    )
}

/// Standard multiarch Debian/freedesktop SDK triple for Flatpak's GL extension path.
pub(crate) fn flatpak_gl_arch_triple(target_arch: &str) -> Option<&'static str> {
    match target_arch {
        "x86_64" => Some("x86_64-linux-gnu"),
        "aarch64" => Some("aarch64-linux-gnu"),
        "x86" => Some("i386-linux-gnu"),
        "arm" => Some("arm-linux-gnueabihf"),
        "riscv64" => Some("riscv64-linux-gnu"),
        "powerpc64" => Some("powerpc64le-linux-gnu"),
        _ => None,
    }
}

fn is_xdg_path(entry: &str) -> bool {
    !entry.is_empty() && (entry.starts_with('/') || Path::new(entry).is_absolute())
}

fn push_unique_icd_dir(out: &mut Vec<PathBuf>, base: &Path) {
    if out.len() >= MAX_MANIFEST_DIRS {
        return;
    }
    let dir = base.join("vulkan").join("icd.d");
    if !out.contains(&dir) {
        out.push(dir);
    }
}

/// Standard Linux Vulkan ICD manifest directories in Vulkan-Loader priority order, including
/// NixOS (`/run/opengl-driver/share`) and Flatpak (`/usr/lib/<triple>/GL`).
///
/// Parameterized over `env` and `target_arch` so tests can exercise every path rule on any
/// platform without mutating the process environment.
pub(crate) fn icd_search_dirs(
    env: impl Fn(&str) -> Option<String>,
    target_arch: &str,
) -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    let home = env("HOME").filter(|h| is_xdg_path(h));

    // 1. $XDG_CONFIG_HOME or $HOME/.config
    if let Some(cfg_home) = env("XDG_CONFIG_HOME").filter(|p| is_xdg_path(p)) {
        push_unique_icd_dir(&mut dirs, Path::new(&cfg_home));
    } else if let Some(home) = &home {
        push_unique_icd_dir(&mut dirs, &Path::new(home).join(".config"));
    }

    // 2. $XDG_CONFIG_DIRS or /etc/xdg
    let cfg_dirs = env("XDG_CONFIG_DIRS").filter(|s| !s.is_empty());
    let cfg_list = cfg_dirs.as_deref().unwrap_or("/etc/xdg");
    let mut added_cfg = false;
    for part in cfg_list.split(':') {
        if is_xdg_path(part) {
            push_unique_icd_dir(&mut dirs, Path::new(part));
            added_cfg = true;
        }
    }
    if !added_cfg {
        push_unique_icd_dir(&mut dirs, Path::new("/etc/xdg"));
    }

    // 3. /etc
    push_unique_icd_dir(&mut dirs, Path::new("/etc"));

    // 4. $XDG_DATA_HOME or $HOME/.local/share
    if let Some(data_home) = env("XDG_DATA_HOME").filter(|p| is_xdg_path(p)) {
        push_unique_icd_dir(&mut dirs, Path::new(&data_home));
    } else if let Some(home) = &home {
        push_unique_icd_dir(&mut dirs, &Path::new(home).join(".local").join("share"));
    }

    // 5. $XDG_DATA_DIRS or /usr/local/share:/usr/share
    let data_dirs = env("XDG_DATA_DIRS").filter(|s| !s.is_empty());
    let data_list = data_dirs.as_deref().unwrap_or("/usr/local/share:/usr/share");
    let mut added_data = false;
    for part in data_list.split(':') {
        if is_xdg_path(part) {
            push_unique_icd_dir(&mut dirs, Path::new(part));
            added_data = true;
        }
    }
    if !added_data {
        push_unique_icd_dir(&mut dirs, Path::new("/usr/local/share"));
        push_unique_icd_dir(&mut dirs, Path::new("/usr/share"));
    }

    // 6. NixOS (/run/opengl-driver/share) and Flatpak (/usr/lib/<arch>-linux-gnu/GL).
    push_unique_icd_dir(&mut dirs, Path::new("/run/opengl-driver/share"));
    if let Some(triple) = flatpak_gl_arch_triple(target_arch) {
        push_unique_icd_dir(&mut dirs, &Path::new("/usr/lib").join(triple).join("GL"));
    }

    dirs
}

/// Counts of filesystem operations performed by [`scan_candidate_libraries`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct ScanStats {
    pub dirs_checked: usize,
    pub manifests_read: usize,
}

/// Outcome of parsing a single ICD JSON manifest.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum ParsedManifest {
    /// Manifest matches this target's pointer width and provides a resolved library path.
    Library(PathBuf),
    /// Valid manifest that intentionally does not apply (different `library_arch` or
    /// `is_portability_driver: true`).
    Skip,
    /// Malformed or incomplete manifest — triggers total fallback so Rune does not guess.
    Invalid,
}

/// Parses one Vulkan ICD manifest JSON body, filtering optional `ICD.library_arch` against
/// `target_pointer_width` (`"64"` or `"32"`) and resolving relative `ICD.library_path`
/// values against `manifest_dir`.
pub(crate) fn parse_manifest_json(
    bytes: &[u8],
    manifest_dir: &Path,
    target_pointer_width: &str,
) -> ParsedManifest {
    let Ok(root) = serde_json::from_slice::<serde_json::Value>(bytes) else {
        return ParsedManifest::Invalid;
    };
    let Some(icd) = root.get("ICD").and_then(serde_json::Value::as_object) else {
        return ParsedManifest::Invalid;
    };
    if let Some(portability) = icd.get("is_portability_driver") {
        let Some(is_portability) = portability.as_bool() else {
            return ParsedManifest::Invalid;
        };
        if is_portability {
            return ParsedManifest::Skip;
        }
    }
    if let Some(arch) = icd.get("library_arch") {
        let Some(arch) = arch.as_str() else {
            return ParsedManifest::Invalid;
        };
        if arch != target_pointer_width {
            return ParsedManifest::Skip;
        }
    }
    let Some(library_path) = icd
        .get("library_path")
        .and_then(serde_json::Value::as_str)
        .filter(|p| !p.is_empty())
    else {
        return ParsedManifest::Invalid;
    };
    ParsedManifest::Library(resolve_library_path(library_path, manifest_dir))
}

/// Resolves an ICD manifest's `library_path` following Vulkan-Loader rules:
/// - Absolute paths are kept as-is.
/// - Paths containing a separator (`/` or `\`) are resolved relative to `manifest_dir`.
/// - Bare library names (e.g. `libvulkan_intel.so`) are kept as-is for `dlopen`'s linker search.
pub(crate) fn resolve_library_path(library_path: &str, manifest_dir: &Path) -> PathBuf {
    let path = Path::new(library_path);
    if path.is_absolute() || library_path.starts_with('/') {
        PathBuf::from(library_path)
    } else if library_path.contains('/') || library_path.contains('\\') {
        manifest_dir.join(library_path)
    } else {
        PathBuf::from(library_path)
    }
}

fn manifest_matches_prefixes(file_name: &str, prefixes: &[&str]) -> bool {
    file_name.ends_with(".json") && prefixes.iter().any(|prefix| file_name.starts_with(prefix))
}

/// Reads at most `MAX_MANIFEST_BYTES` from a regular file. Returns `None` if the file cannot
/// be opened, is not a regular file, or exceeds `MAX_MANIFEST_BYTES`.
fn read_bounded_manifest(path: &Path) -> Option<Vec<u8>> {
    let file = std::fs::File::open(path).ok()?;
    let meta = file.metadata().ok()?;
    if !meta.is_file() || meta.len() > MAX_MANIFEST_BYTES {
        return None;
    }
    let mut buf = Vec::with_capacity(meta.len().min(MAX_MANIFEST_BYTES) as usize);
    let mut limited = file.take(MAX_MANIFEST_BYTES + 1);
    limited.read_to_end(&mut buf).ok()?;
    if buf.len() as u64 > MAX_MANIFEST_BYTES {
        return None;
    }
    Some(buf)
}

/// Scans `dirs` (bounded by [`MAX_MANIFEST_DIRS`], [`MAX_DIR_ENTRIES_PER_DIR`],
/// [`MAX_MANIFEST_FILES`], and [`MAX_MANIFEST_BYTES`]) for JSON manifests matching
/// `kernel_driver` and returns deduplicated candidate ICD library paths in directory priority
/// order.
///
/// Returns `None` on any doubt: unknown kernel driver, unreadable or oversized or malformed
/// matching manifest, or exceeding the manifest-file bound. Missing search directories are normal
/// and skipped silently.
pub(crate) fn scan_candidate_libraries(
    kernel_driver: &str,
    dirs: &[PathBuf],
    target_pointer_width: &str,
) -> Option<(Vec<PathBuf>, ScanStats)> {
    let prefixes = icd_prefixes(kernel_driver)?;
    let mut libraries = Vec::new();
    let mut stats = ScanStats::default();

    for dir in dirs.iter().take(MAX_MANIFEST_DIRS) {
        stats.dirs_checked += 1;
        let read_dir = match std::fs::read_dir(dir) {
            Ok(iter) => iter,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => continue,
            Err(err) if err.kind() == std::io::ErrorKind::NotADirectory => continue,
            Err(_) => continue,
        };

        let mut matching_files = Vec::new();
        for (seen, entry) in read_dir.enumerate() {
            if seen >= MAX_DIR_ENTRIES_PER_DIR {
                break;
            }
            let Ok(entry) = entry else { continue };
            let name = entry.file_name();
            let Some(name_str) = name.to_str() else { continue };
            if manifest_matches_prefixes(name_str, prefixes) {
                matching_files.push((name_str.to_owned(), entry.path()));
            }
        }
        // Deterministic ordering within each directory across filesystems.
        matching_files.sort_by(|a, b| a.0.cmp(&b.0));

        for (_, manifest_path) in matching_files {
            if stats.manifests_read >= MAX_MANIFEST_FILES {
                return None;
            }
            stats.manifests_read += 1;
            let bytes = read_bounded_manifest(&manifest_path)?;
            match parse_manifest_json(&bytes, dir, target_pointer_width) {
                ParsedManifest::Library(lib_path) => {
                    if !libraries.contains(&lib_path) && libraries.len() < MAX_CANDIDATE_LIBS {
                        libraries.push(lib_path);
                    }
                }
                ParsedManifest::Skip => {}
                ParsedManifest::Invalid => return None,
            }
        }
    }

    Some((libraries, stats))
}

/// Discovers candidate ICD library paths on the current Linux host for `kernel_driver`.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub(crate) fn discover_candidate_libraries(kernel_driver: &str) -> Option<Vec<PathBuf>> {
    let dirs = icd_search_dirs(rune_core::env::var, std::env::consts::ARCH);
    let pointer_width = if cfg!(target_pointer_width = "64") {
        "64"
    } else if cfg!(target_pointer_width = "32") {
        "32"
    } else {
        return None;
    };
    scan_candidate_libraries(kernel_driver, &dirs, pointer_width).map(|(libs, _)| libs)
}

/// Whether a direct-driver instance's enumerated adapters satisfy the compositor's GPU:
/// - When the compositor reported a PCI address, at least one adapter must sit on that exact PCI
///   device (case-insensitive).
/// - On non-PCI SoCs (`compositor_pci` is `None` or empty), at least one non-CPU adapter must
///   be present.
pub(crate) fn direct_adapters_match(
    compositor_pci: Option<&str>,
    adapters: &[wgpu::AdapterInfo],
) -> bool {
    match compositor_pci {
        Some(pci) if !pci.is_empty() => adapters.iter().any(|info| {
            !info.device_pci_bus_id.is_empty() && info.device_pci_bus_id.eq_ignore_ascii_case(pci)
        }),
        _ => adapters
            .iter()
            .any(|info| info.device_type != wgpu::DeviceType::Cpu),
    }
}

/// Executes [`DirectDriverPlan`]:
/// - For [`DirectDriverPlan::Default`], calls `build_default` directly without ever invoking
///   `build_direct` (identical to today's path).
/// - For [`DirectDriverPlan::Direct`], tries `build_direct(paths)`, accepts it only when
///   `matches(&direct)` returns `true`, and otherwise drops `direct` before calling
///   `build_default`.
///
/// Returns `(instance, used_direct)`.
pub(crate) fn select_direct_or_default_instance<I>(
    plan: DirectDriverPlan<'_>,
    mut build_direct: impl FnMut(&[PathBuf]) -> I,
    mut matches: impl FnMut(&I) -> bool,
    mut build_default: impl FnMut() -> I,
) -> (I, bool) {
    match plan {
        DirectDriverPlan::Default(_) => (build_default(), false),
        DirectDriverPlan::Direct(paths) => {
            let direct = build_direct(paths);
            if matches(&direct) {
                (direct, true)
            } else {
                drop(direct);
                (build_default(), false)
            }
        }
    }
}

/// Linux bridge into the vendored `wgpu-hal` direct-driver hook
/// (`wgpu_hal::vulkan::Instance::with_direct_drivers`), using the same `without_patch` trait
/// fallback idiom as Windows `gpu::pick::dxgi` so unpatched downstream workspaces still compile
/// and fall back directly to today's unlimited instance path.
#[cfg(target_os = "linux")]
#[allow(unsafe_code)]
mod direct_vulkan {
    use std::cell::Cell;
    use std::path::PathBuf;

    // Unused when the vendored wgpu-hal's inherent `with_direct_drivers` takes the call.
    #[allow(unused_imports)]
    use without_patch::WithDirectDrivers as _;

    mod without_patch {
        use super::*;

        thread_local! {
            pub(super) static UNPATCHED: Cell<bool> = const { Cell::new(false) };
        }

        #[allow(dead_code)]
        pub(super) trait WithDirectDrivers {
            fn with_direct_drivers<R>(_paths: Vec<PathBuf>, f: impl FnOnce() -> R) -> R {
                UNPATCHED.with(|unpatched| unpatched.set(true));
                f()
            }
        }

        impl WithDirectDrivers for wgpu::hal::vulkan::Instance {}
    }

    pub(super) fn with_direct_drivers<R>(paths: Vec<PathBuf>, f: impl FnOnce() -> R) -> (R, bool) {
        without_patch::UNPATCHED.with(|unpatched| unpatched.set(false));
        let out = wgpu::hal::vulkan::Instance::with_direct_drivers(paths, f);
        let patched = !without_patch::UNPATCHED.with(|unpatched| unpatched.replace(false));
        (out, patched)
    }

    pub(super) fn hook_available() -> bool {
        with_direct_drivers(Vec::new(), || ()).1
    }

    pub(super) fn has_vulkan_hal(instance: &wgpu::Instance) -> bool {
        // SAFETY: borrows the Vulkan HAL instance handle for the duration of this check only.
        unsafe { instance.as_hal::<wgpu::hal::api::Vulkan>() }.is_some()
    }

    pub(super) fn is_direct_driver_instance(instance: &wgpu::Instance) -> bool {
        // SAFETY: borrows the Vulkan HAL instance handle to inspect its enabled extension list.
        unsafe { instance.as_hal::<wgpu::hal::api::Vulkan>() }.is_some_and(|hal| {
            hal.shared_instance()
                .extensions()
                .iter()
                .any(|ext| *ext == c"VK_LUNARG_direct_driver_loading")
        })
    }
}

/// Whether `instance` was created with `VK_LUNARG_direct_driver_loading` on Linux.
#[cfg(target_os = "linux")]
pub(crate) fn is_direct_driver_instance(instance: &wgpu::Instance) -> bool {
    direct_vulkan::is_direct_driver_instance(instance)
}

/// Builds a Linux `wgpu::Instance`: tries `VK_LUNARG_direct_driver_loading` (exclusive mode)
/// for the Wayland compositor's kernel driver when [`decide`] permits it, accepts it only when
/// its adapters include the compositor's PCI device (or any non-CPU adapter on non-PCI SoCs),
/// and otherwise falls back cleanly to `build_unlimited`.
#[cfg(target_os = "linux")]
pub(crate) fn create_linux_instance(
    backends: wgpu::Backends,
    previously_failed: bool,
    make_descriptor: impl Fn(bool) -> wgpu::InstanceDescriptor,
    build_unlimited: impl FnOnce() -> wgpu::Instance,
) -> (wgpu::Instance, bool) {
    let base_desc = make_descriptor(false);
    let debug_or_validation = has_debug_or_validation(base_desc.flags);
    let env_override = has_override_env_process();
    let hal_hook_available = direct_vulkan::hook_available();
    let compositor = if backends.contains(wgpu::Backends::VULKAN)
        && !debug_or_validation
        && !previously_failed
        && !env_override
        && hal_hook_available
    {
        crate::gpu::pick::compositor_device()
    } else {
        None
    };
    let compositor_pci = compositor.as_ref().and_then(|d| d.pci.as_deref());
    let compositor_driver = compositor.as_ref().and_then(|d| d.driver.as_deref());
    let candidate_libraries = compositor_driver
        .and_then(discover_candidate_libraries)
        .unwrap_or_default();
    let plan = decide(&DirectDriverInput {
        vulkan_backend: backends.contains(wgpu::Backends::VULKAN),
        debug_or_validation,
        previously_failed,
        env_override,
        hal_hook_available,
        loader_extension_supported: true,
        compositor_driver,
        candidate_libraries: &candidate_libraries,
    });
    let mut build_unlimited = Some(build_unlimited);
    select_direct_or_default_instance(
        plan,
        |paths| {
            let (instance, _) = direct_vulkan::with_direct_drivers(paths.to_vec(), || {
                let compliant = wgpu::Instance::new(make_descriptor(false));
                if !direct_vulkan::has_vulkan_hal(&compliant) {
                    return compliant;
                }
                if !pollster::block_on(compliant.enumerate_adapters(backends)).is_empty() {
                    return compliant;
                }
                let noncompliant = wgpu::Instance::new(make_descriptor(true));
                if !pollster::block_on(noncompliant.enumerate_adapters(backends)).is_empty() {
                    return noncompliant;
                }
                compliant
            });
            instance
        },
        |instance| {
            if !direct_vulkan::is_direct_driver_instance(instance) {
                return false;
            }
            let infos: Vec<_> = pollster::block_on(instance.enumerate_adapters(backends))
                .into_iter()
                .map(|a| a.get_info())
                .collect();
            direct_adapters_match(compositor_pci, &infos)
        },
        || (build_unlimited.take().expect("build_unlimited called once"))(),
    )
}

#[cfg(test)]
#[path = "vulkan_icd_tests.rs"]
mod tests;
