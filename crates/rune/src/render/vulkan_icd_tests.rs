use std::collections::HashMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use super::{
    DirectDriverInput, DirectDriverPlan, FallbackReason, MAX_MANIFEST_BYTES, MAX_MANIFEST_FILES,
    OVERRIDE_ENV_VARS, ParsedManifest, decide, direct_adapters_match, has_debug_or_validation,
    has_override_env, icd_prefixes, icd_search_dirs, parse_manifest_json, resolve_library_path,
    scan_candidate_libraries, select_direct_or_default_instance,
};

fn temp_dir(label: &str) -> PathBuf {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!(
        "rune-vulkan-icd-test-{}-{}-{n}",
        std::process::id(),
        label
    ));
    std::fs::remove_dir_all(&dir).ok();
    std::fs::create_dir_all(&dir).expect("create temp test dir");
    dir
}

struct TempDirGuard(PathBuf);
impl Drop for TempDirGuard {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).ok();
    }
}

#[test]
fn decide_table_covers_every_fallback_condition_and_direct_happy_path() {
    let libs = vec![PathBuf::from("/usr/lib64/libvulkan_intel.so")];
    let empty: Vec<PathBuf> = Vec::new();
    let base = DirectDriverInput {
        vulkan_backend: true,
        debug_or_validation: false,
        previously_failed: false,
        env_override: false,
        hal_hook_available: true,
        loader_extension_supported: true,
        compositor_driver: Some("i915"),
        candidate_libraries: &libs,
    };

    let cases: [(&str, DirectDriverInput<'_>, DirectDriverPlan<'_>); 10] = [
        (
            "happy path: Intel i915 with candidate lib",
            base.clone(),
            DirectDriverPlan::Direct(&libs),
        ),
        (
            "happy path: AMD amdgpu with candidate lib",
            DirectDriverInput { compositor_driver: Some("amdgpu"), ..base.clone() },
            DirectDriverPlan::Direct(&libs),
        ),
        (
            "no Vulkan backend",
            DirectDriverInput { vulkan_backend: false, ..base.clone() },
            DirectDriverPlan::Default(FallbackReason::NoVulkanBackend),
        ),
        (
            "debug or validation flag set",
            DirectDriverInput { debug_or_validation: true, ..base.clone() },
            DirectDriverPlan::Default(FallbackReason::DebugOrValidation),
        ),
        (
            "previously failed / unlimited requested",
            DirectDriverInput { previously_failed: true, ..base.clone() },
            DirectDriverPlan::Default(FallbackReason::PreviouslyFailed),
        ),
        (
            "user override env var set",
            DirectDriverInput { env_override: true, ..base.clone() },
            DirectDriverPlan::Default(FallbackReason::EnvOverride),
        ),
        (
            "unpatched wgpu-hal workspace",
            DirectDriverInput { hal_hook_available: false, ..base.clone() },
            DirectDriverPlan::Default(FallbackReason::UnpatchedHal),
        ),
        (
            "direct driver loading extension absent",
            DirectDriverInput { loader_extension_supported: false, ..base.clone() },
            DirectDriverPlan::Default(FallbackReason::ExtensionAbsent),
        ),
        (
            "unknown or missing compositor kernel driver",
            DirectDriverInput { compositor_driver: Some("simpledrm"), ..base.clone() },
            DirectDriverPlan::Default(FallbackReason::UnknownDriver),
        ),
        (
            "no candidate ICD libraries found",
            DirectDriverInput { candidate_libraries: &empty, ..base },
            DirectDriverPlan::Default(FallbackReason::NoCandidateLibraries),
        ),
    ];

    for (label, input, expected) in cases {
        assert_eq!(decide(&input), expected, "case: {label}");
    }
}

#[test]
fn debug_and_validation_flags_trigger_bypass() {
    assert!(!has_debug_or_validation(wgpu::InstanceFlags::empty()));
    assert!(!has_debug_or_validation(
        wgpu::InstanceFlags::ALLOW_UNDERLYING_NONCOMPLIANT_ADAPTER
    ));
    assert!(has_debug_or_validation(wgpu::InstanceFlags::DEBUG));
    assert!(has_debug_or_validation(wgpu::InstanceFlags::VALIDATION));
    assert!(has_debug_or_validation(wgpu::InstanceFlags::GPU_BASED_VALIDATION));
}

#[test]
fn fallback_path_executes_today_path_without_calling_direct_builder() {
    let mut direct_calls = 0usize;
    let mut default_calls = 0usize;
    let (chosen, used_direct) = select_direct_or_default_instance(
        DirectDriverPlan::Default(FallbackReason::EnvOverride),
        |_| {
            direct_calls += 1;
            "direct"
        },
        |_| unreachable!("matches must not run when plan is Default"),
        || {
            default_calls += 1;
            "default"
        },
    );
    assert_eq!(chosen, "default");
    assert!(!used_direct);
    assert_eq!(direct_calls, 0);
    assert_eq!(default_calls, 1);
}

#[test]
fn direct_instance_rejection_drops_direct_before_building_default() {
    let log: Arc<Mutex<Vec<&'static str>>> = Arc::new(Mutex::new(Vec::new()));
    struct Tracked {
        label: &'static str,
        log: Arc<Mutex<Vec<&'static str>>>,
    }
    impl Drop for Tracked {
        fn drop(&mut self) {
            self.log.lock().unwrap().push(self.label);
        }
    }

    let libs = vec![PathBuf::from("/usr/lib64/libvulkan_intel.so")];
    let (chosen, used_direct) = select_direct_or_default_instance(
        DirectDriverPlan::Direct(&libs),
        |paths| {
            assert_eq!(paths, libs.as_slice());
            log.lock().unwrap().push("build_direct");
            Tracked { label: "drop_direct", log: Arc::clone(&log) }
        },
        |_| {
            log.lock().unwrap().push("check_direct_false");
            false
        },
        || {
            log.lock().unwrap().push("build_default");
            Tracked { label: "drop_default", log: Arc::clone(&log) }
        },
    );
    assert!(!used_direct);
    assert_eq!(chosen.label, "drop_default");
    assert_eq!(
        *log.lock().unwrap(),
        vec!["build_direct", "check_direct_false", "drop_direct", "build_default"],
        "rejected direct instance must be dropped before building the unlimited fallback instance"
    );
    drop(chosen);
}

#[test]
fn direct_instance_acceptance_skips_default_builder() {
    let libs = vec![PathBuf::from("/usr/lib64/libvulkan_intel.so")];
    let mut default_calls = 0usize;
    let (chosen, used_direct) = select_direct_or_default_instance(
        DirectDriverPlan::Direct(&libs),
        |_| "direct",
        |_| true,
        || {
            default_calls += 1;
            "default"
        },
    );
    assert_eq!(chosen, "direct");
    assert!(used_direct);
    assert_eq!(default_calls, 0);
}

#[test]
fn override_env_vars_bypass_direct_loading() {
    assert!(!has_override_env(|_| None));
    assert!(!has_override_env(|_| Some(OsString::new())));
    assert!(!has_override_env(|name| {
        (name == "VK_LOADER_DEBUG").then(|| OsString::from("all"))
    }));
    for &var in OVERRIDE_ENV_VARS {
        assert!(
            has_override_env(|name| (name == var).then(|| OsString::from("1"))),
            "{var} must bypass direct loading"
        );
    }
}

#[test]
fn kernel_driver_table_maps_known_drivers_and_rejects_unknown() {
    assert_eq!(icd_prefixes("i915"), Some(&["intel_icd", "intel_hasvk_icd"][..]));
    assert_eq!(icd_prefixes("xe"), Some(&["intel_icd", "intel_hasvk_icd"][..]));
    assert_eq!(icd_prefixes("amdgpu"), Some(&["radeon_icd", "amd_icd", "amd_pro_icd"][..]));
    assert_eq!(icd_prefixes("nvidia"), Some(&["nvidia_icd"][..]));
    assert_eq!(icd_prefixes("nouveau"), Some(&["nouveau_icd"][..]));
    assert_eq!(icd_prefixes("msm"), Some(&["freedreno_icd"][..]));
    assert_eq!(icd_prefixes("panfrost"), Some(&["panfrost_icd"][..]));
    assert_eq!(icd_prefixes("panthor"), Some(&["panfrost_icd"][..]));
    assert_eq!(icd_prefixes("v3d"), Some(&["broadcom_icd"][..]));
    assert_eq!(icd_prefixes("asahi"), Some(&["asahi_icd"][..]));
    assert_eq!(icd_prefixes("powervr"), Some(&["powervr_mesa_icd", "powervr_icd"][..]));
    assert_eq!(icd_prefixes("virtio_gpu"), Some(&["virtio_icd"][..]));
    assert_eq!(icd_prefixes("simpledrm"), None);
    assert_eq!(icd_prefixes("radeon"), None);
    assert_eq!(icd_prefixes(""), None);
}

#[test]
fn search_dirs_follow_xdg_priority_and_include_nixos_and_flatpak() {
    let empty_env = HashMap::<&str, &str>::new();
    let dirs = icd_search_dirs(|k| empty_env.get(k).map(|v| (*v).to_owned()), "x86_64");
    assert_eq!(
        dirs,
        vec![
            PathBuf::from("/etc/xdg/vulkan/icd.d"),
            PathBuf::from("/etc/vulkan/icd.d"),
            PathBuf::from("/usr/local/share/vulkan/icd.d"),
            PathBuf::from("/usr/share/vulkan/icd.d"),
            PathBuf::from("/run/opengl-driver/share/vulkan/icd.d"),
            PathBuf::from("/usr/lib/x86_64-linux-gnu/GL/vulkan/icd.d"),
        ]
    );

    let custom_env = HashMap::from([
        ("HOME", "/home/alice"),
        ("XDG_CONFIG_HOME", "/custom/cfg"),
        ("XDG_CONFIG_DIRS", "/cfg1:/etc/xdg"),
        ("XDG_DATA_HOME", "/custom/data"),
        ("XDG_DATA_DIRS", "/data1:/usr/share:/data1"),
    ]);
    let dirs = icd_search_dirs(|k| custom_env.get(k).map(|v| (*v).to_owned()), "aarch64");
    assert_eq!(
        dirs,
        vec![
            PathBuf::from("/custom/cfg/vulkan/icd.d"),
            PathBuf::from("/cfg1/vulkan/icd.d"),
            PathBuf::from("/etc/xdg/vulkan/icd.d"),
            PathBuf::from("/etc/vulkan/icd.d"),
            PathBuf::from("/custom/data/vulkan/icd.d"),
            PathBuf::from("/data1/vulkan/icd.d"),
            PathBuf::from("/usr/share/vulkan/icd.d"),
            PathBuf::from("/run/opengl-driver/share/vulkan/icd.d"),
            PathBuf::from("/usr/lib/aarch64-linux-gnu/GL/vulkan/icd.d"),
        ],
        "priority order preserved, duplicates removed, aarch64 Flatpak triple used"
    );

    let home_only = HashMap::from([("HOME", "/home/bob")]);
    let dirs = icd_search_dirs(|k| home_only.get(k).map(|v| (*v).to_owned()), "x86_64");
    assert_eq!(dirs[0], PathBuf::from("/home/bob/.config/vulkan/icd.d"));
    assert!(dirs.contains(&PathBuf::from("/home/bob/.local/share/vulkan/icd.d")));
}

#[test]
fn manifest_parsing_resolves_paths_and_filters_arch() {
    let dir = Path::new("/usr/share/vulkan/icd.d");

    // Absolute path
    assert_eq!(
        parse_manifest_json(
            br#"{"file_format_version":"1.0.0","ICD":{"library_path":"/usr/lib64/libvulkan_intel.so","library_arch":"64"}}"#,
            dir,
            "64"
        ),
        ParsedManifest::Library(PathBuf::from("/usr/lib64/libvulkan_intel.so"))
    );

    // Relative path with separator resolved against manifest directory
    assert_eq!(
        parse_manifest_json(
            br#"{"ICD":{"library_path":"../lib/libvulkan_intel.so"}}"#,
            dir,
            "64"
        ),
        ParsedManifest::Library(dir.join("../lib/libvulkan_intel.so"))
    );

    // Bare library name kept as-is for dlopen search
    assert_eq!(
        resolve_library_path("libGLX_nvidia.so.0", dir),
        PathBuf::from("libGLX_nvidia.so.0")
    );
    assert_eq!(
        parse_manifest_json(
            br#"{"ICD":{"library_path":"libvulkan_radeon.so"}}"#,
            dir,
            "64"
        ),
        ParsedManifest::Library(PathBuf::from("libvulkan_radeon.so"))
    );

    // Non-matching library_arch skipped cleanly (multilib 32-bit manifest on 64-bit host)
    assert_eq!(
        parse_manifest_json(
            br#"{"ICD":{"library_path":"/usr/lib/libvulkan_intel.so","library_arch":"32"}}"#,
            dir,
            "64"
        ),
        ParsedManifest::Skip
    );

    // Portability driver skipped
    assert_eq!(
        parse_manifest_json(
            br#"{"ICD":{"library_path":"/usr/lib64/libfoo.so","is_portability_driver":true}}"#,
            dir,
            "64"
        ),
        ParsedManifest::Skip
    );

    // Malformed / missing library_path -> Invalid (triggers total fallback)
    assert_eq!(parse_manifest_json(b"not json", dir, "64"), ParsedManifest::Invalid);
    assert_eq!(parse_manifest_json(br#"{"ICD":{}}"#, dir, "64"), ParsedManifest::Invalid);
    assert_eq!(
        parse_manifest_json(br#"{"ICD":{"library_path":""}}"#, dir, "64"),
        ParsedManifest::Invalid
    );
}

#[test]
fn scan_discovers_priority_ordered_libraries_and_ignores_other_vendors() {
    let root = TempDirGuard(temp_dir("priority"));
    let high = root.0.join("high");
    let low = root.0.join("low");
    let missing = root.0.join("does-not-exist");
    std::fs::create_dir_all(&high).unwrap();
    std::fs::create_dir_all(&low).unwrap();

    // High-priority dir has Intel 64-bit, Intel 32-bit (must be skipped), and NVIDIA (must be ignored).
    std::fs::write(
        high.join("intel_icd.x86_64.json"),
        r#"{"ICD":{"library_path":"./libvulkan_intel_custom.so","library_arch":"64"}}"#,
    )
    .unwrap();
    std::fs::write(
        high.join("intel_icd.i686.json"),
        r#"{"ICD":{"library_path":"/usr/lib32/libvulkan_intel.so","library_arch":"32"}}"#,
    )
    .unwrap();
    std::fs::write(high.join("nvidia_icd.json"), b"CORRUPT BUT UNRELATED VENDOR").unwrap();
    std::fs::write(high.join("lvp_icd.x86_64.json"), b"CORRUPT LAVAPIPE").unwrap();

    // Low-priority dir has Intel hasvk + system Intel.
    std::fs::write(
        low.join("intel_hasvk_icd.x86_64.json"),
        r#"{"ICD":{"library_path":"/usr/lib64/libvulkan_intel_hasvk.so","library_arch":"64"}}"#,
    )
    .unwrap();
    std::fs::write(
        low.join("intel_icd.x86_64.json"),
        r#"{"ICD":{"library_path":"/usr/lib64/libvulkan_intel.so","library_arch":"64"}}"#,
    )
    .unwrap();

    let (libs, stats) = scan_candidate_libraries("i915", &[missing, high.clone(), low], "64")
        .expect("scan succeeds");
    assert_eq!(
        libs,
        vec![
            high.join("./libvulkan_intel_custom.so"),
            PathBuf::from("/usr/lib64/libvulkan_intel_hasvk.so"),
            PathBuf::from("/usr/lib64/libvulkan_intel.so"),
        ]
    );
    assert_eq!(stats.dirs_checked, 3);
    assert_eq!(stats.manifests_read, 4, "only matching intel_* manifests are opened");
}

#[test]
fn scan_discovers_powervr_mesa_and_generic_icd_manifests() {
    let root = TempDirGuard(temp_dir("powervr"));
    let icd_dir = root.0.join("icd.d");
    std::fs::create_dir_all(&icd_dir).unwrap();

    std::fs::write(
        icd_dir.join("powervr_mesa_icd.x86_64.json"),
        r#"{"ICD":{"library_path":"/usr/lib64/libvulkan_powervr_mesa.so","library_arch":"64"}}"#,
    )
    .unwrap();
    std::fs::write(
        icd_dir.join("powervr_icd.aarch64.json"),
        r#"{"ICD":{"library_path":"/usr/lib/libvulkan_powervr.so","library_arch":"64"}}"#,
    )
    .unwrap();
    std::fs::write(icd_dir.join("nvidia_icd.json"), b"UNRELATED VENDOR").unwrap();

    let (libs, stats) = scan_candidate_libraries("powervr", &[icd_dir], "64")
        .expect("powervr scan succeeds");
    assert_eq!(
        libs,
        vec![
            PathBuf::from("/usr/lib/libvulkan_powervr.so"),
            PathBuf::from("/usr/lib64/libvulkan_powervr_mesa.so"),
        ]
    );
    assert_eq!(stats.manifests_read, 2, "only matching powervr manifests are opened");
}

#[test]
fn scan_bails_out_on_oversized_malformed_or_too_many_matching_manifests() {
    let root = TempDirGuard(temp_dir("bounds"));

    // 1. Oversized matching manifest -> None (total fallback)
    let big_dir = root.0.join("big");
    std::fs::create_dir_all(&big_dir).unwrap();
    let oversized = vec![b' '; (MAX_MANIFEST_BYTES + 1) as usize];
    std::fs::write(big_dir.join("intel_icd.x86_64.json"), &oversized).unwrap();
    assert_eq!(scan_candidate_libraries("i915", &[big_dir], "64"), None);

    // 2. Malformed matching manifest -> None (total fallback)
    let bad_dir = root.0.join("bad");
    std::fs::create_dir_all(&bad_dir).unwrap();
    std::fs::write(bad_dir.join("radeon_icd.x86_64.json"), b"{bad json").unwrap();
    assert_eq!(scan_candidate_libraries("amdgpu", &[bad_dir], "64"), None);

    // 3. Too many matching manifests -> None (bounded file reads)
    let many_dir = root.0.join("many");
    std::fs::create_dir_all(&many_dir).unwrap();
    for i in 0..=MAX_MANIFEST_FILES {
        std::fs::write(
            many_dir.join(format!("intel_icd_{i:02}.json")),
            r#"{"ICD":{"library_path":"/usr/lib64/libvulkan_intel.so"}}"#,
        )
        .unwrap();
    }
    assert_eq!(scan_candidate_libraries("i915", &[many_dir], "64"), None);
}

#[test]
fn direct_adapters_match_checks_pci_and_non_pci_socs() {
    let mk = |name: &str, kind: wgpu::DeviceType, pci: &str| wgpu::AdapterInfo {
        name: name.into(),
        device_pci_bus_id: pci.into(),
        ..wgpu::AdapterInfo::new(kind, wgpu::Backend::Vulkan)
    };
    let intel = mk("Intel UHD 630", wgpu::DeviceType::IntegratedGpu, "0000:00:02.0");
    let nvidia = mk("NVIDIA GTX 1050 Ti", wgpu::DeviceType::DiscreteGpu, "0000:01:00.0");
    let asahi = mk("Apple M2 (Honeykrisp)", wgpu::DeviceType::IntegratedGpu, "");
    let llvmpipe = mk("llvmpipe", wgpu::DeviceType::Cpu, "");

    assert!(direct_adapters_match(Some("0000:00:02.0"), &[intel.clone()]));
    assert!(direct_adapters_match(
        Some("0000:0A:00.0"),
        &[mk("AMD", wgpu::DeviceType::IntegratedGpu, "0000:0a:00.0")]
    ));
    assert!(!direct_adapters_match(Some("0000:00:02.0"), &[nvidia]));
    assert!(!direct_adapters_match(Some("0000:00:02.0"), &[llvmpipe.clone()]));
    assert!(!direct_adapters_match(Some("0000:00:02.0"), &[]));

    // Non-PCI SoC: accepts any non-CPU adapter, rejects CPU-only or empty.
    assert!(direct_adapters_match(None, &[asahi]));
    assert!(direct_adapters_match(Some(""), &[intel]));
    assert!(!direct_adapters_match(None, &[llvmpipe]));
    assert!(!direct_adapters_match(None, &[]));
}

/// Proves the exact Rust drop-order invariant relied on by `wgpu_hal::vulkan::InstanceShared`:
/// 1. Every child object (`Adapter`, `Device`, `Surface`) holds `Arc<InstanceShared>`, so
///    dropping the `Instance` handle keeps the `VkInstance` and loaded ICD libraries alive.
/// 2. When the final `Arc<InstanceShared>` drops, `InstanceShared::drop` runs
///    `vkDestroyInstance` BEFORE any struct field drops.
/// 3. Struct fields then drop in declaration order: `_direct_drivers` (releasing handles opened
///    with `RTLD_NODELETE` while `libvulkan.so.1` is still open), then `entry` (`libvulkan.so.1`),
///    then `drop_guard` last.
#[test]
fn direct_driver_libraries_outlive_instance_and_child_objects_without_gpu() {
    let events: Arc<Mutex<Vec<&'static str>>> = Arc::new(Mutex::new(Vec::new()));

    struct Token {
        label: &'static str,
        events: Arc<Mutex<Vec<&'static str>>>,
    }
    impl Drop for Token {
        fn drop(&mut self) {
            self.events.lock().unwrap().push(self.label);
        }
    }

    // Mirrors wgpu_hal::vulkan::InstanceShared field order and Drop implementation.
    struct SharedModel {
        events: Arc<Mutex<Vec<&'static str>>>,
        _direct_drivers: Vec<Token>,
        _entry: Token,
        drop_guard: Option<Token>,
    }
    impl Drop for SharedModel {
        fn drop(&mut self) {
            if self.drop_guard.is_none() {
                self.events.lock().unwrap().push("vkDestroyInstance");
            }
        }
    }

    struct ChildObject {
        label: &'static str,
        events: Arc<Mutex<Vec<&'static str>>>,
        _shared: Arc<SharedModel>,
    }
    impl Drop for ChildObject {
        fn drop(&mut self) {
            self.events.lock().unwrap().push(self.label);
        }
    }

    let shared = Arc::new(SharedModel {
        events: Arc::clone(&events),
        _direct_drivers: vec![
            Token { label: "release_nodelete(libvulkan_intel.so)", events: Arc::clone(&events) },
            Token { label: "release_nodelete(libvulkan_intel_hasvk.so)", events: Arc::clone(&events) },
        ],
        _entry: Token { label: "dlclose(libvulkan.so.1)", events: Arc::clone(&events) },
        drop_guard: None,
    });

    let adapter = ChildObject {
        label: "drop(Adapter)",
        events: Arc::clone(&events),
        _shared: Arc::clone(&shared),
    };
    let surface = ChildObject {
        label: "drop(Surface)",
        events: Arc::clone(&events),
        _shared: Arc::clone(&shared),
    };
    let device = ChildObject {
        label: "drop(Device)",
        events: Arc::clone(&events),
        _shared: Arc::clone(&shared),
    };

    // Drop the top-level Instance first while child objects are still live.
    drop(shared);
    assert!(
        events.lock().unwrap().is_empty(),
        "dropping Instance must not destroy VkInstance or dlclose ICDs while children live"
    );

    drop(surface);
    drop(adapter);
    assert_eq!(
        *events.lock().unwrap(),
        vec!["drop(Surface)", "drop(Adapter)"],
        "VkInstance and ICD libraries must stay alive while Device still lives"
    );

    // Dropping the last child triggers vkDestroyInstance before dlclose of any ICD library.
    drop(device);
    assert_eq!(
        *events.lock().unwrap(),
        vec![
            "drop(Surface)",
            "drop(Adapter)",
            "drop(Device)",
            "vkDestroyInstance",
            "release_nodelete(libvulkan_intel.so)",
            "release_nodelete(libvulkan_intel_hasvk.so)",
            "dlclose(libvulkan.so.1)",
        ]
    );
}
