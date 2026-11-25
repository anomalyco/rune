// Written by `cargo run -p rune --example regen_dxbc` on Windows, the one way to regenerate the uber bytecode:
// it also writes the pinned fragments and their hashes (dxbc_pinned_hash.rs), which nothing else updates.
pub(super) const UBER_DXBC_WGSL_BLAKE3: &str = "bc62bc220609b72b425947705315a8de2e25389dff260ab596a0ab5be217f951";
pub(super) const UBER_DXBC_ABI_FNV64: u64 = 0x6b9932e7795ec011;
#[cfg(test)]
pub(super) const UBER_DXBC_HLSL_BLAKE3: &str = "cc5d09dc683bcb62e2e2bfec1a39d650ef20ed8f3c37a9a236748670834e3549";
#[cfg(test)]
pub(super) const UBER_DXBC_DUAL_HLSL_BLAKE3: &str = "70faf34455e644a8fd5dca16ca3498feef7890a3545c0283d6a060dca8e94f32";
