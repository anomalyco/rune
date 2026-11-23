use super::Gpu;

#[test]
fn default_instance_descriptor_hides_noncompliant_adapters() {
    let desc = Gpu::instance_descriptor(wgpu::Backends::all(), false);
    assert!(!desc.flags.contains(wgpu::InstanceFlags::ALLOW_UNDERLYING_NONCOMPLIANT_ADAPTER));
    assert!(!desc.flags.contains(wgpu::InstanceFlags::VALIDATION_INDIRECT_CALL));
}

#[test]
fn fallback_instance_descriptor_allows_noncompliant_adapters() {
    let desc = Gpu::instance_descriptor(wgpu::Backends::all(), true);
    assert!(desc.flags.contains(wgpu::InstanceFlags::ALLOW_UNDERLYING_NONCOMPLIANT_ADAPTER));
    assert!(!desc.flags.contains(wgpu::InstanceFlags::VALIDATION_INDIRECT_CALL));
}

#[test]
fn select_wgpu_instance_keeps_compliant_when_adapters_exist() {
    let mut built = Vec::new();
    let chosen = Gpu::select_wgpu_instance(
        |allow_noncompliant| {
            built.push(allow_noncompliant);
            allow_noncompliant
        },
        |&allow_noncompliant| !allow_noncompliant,
    );
    assert!(!chosen, "a machine with a conformant adapter keeps the compliant-only instance");
    assert_eq!(built, vec![false], "non-compliant instance is never built when compliant adapters exist");
}

#[test]
fn select_wgpu_instance_falls_back_when_no_compliant_adapters_exist() {
    let mut built = Vec::new();
    let chosen = Gpu::select_wgpu_instance(
        |allow_noncompliant| {
            built.push(allow_noncompliant);
            allow_noncompliant
        },
        |&allow_noncompliant| allow_noncompliant,
    );
    assert!(chosen, "a machine whose only Vulkan driver reports conformanceVersion 0 falls back to allowing it");
    assert_eq!(built, vec![false, true]);
}

#[test]
fn select_wgpu_instance_keeps_compliant_when_neither_has_adapters() {
    let mut built = Vec::new();
    let chosen = Gpu::select_wgpu_instance(
        |allow_noncompliant| {
            built.push(allow_noncompliant);
            allow_noncompliant
        },
        |_| false,
    );
    assert!(!chosen, "returns the compliant instance when no adapters exist either way");
    assert_eq!(built, vec![false, true]);
}
