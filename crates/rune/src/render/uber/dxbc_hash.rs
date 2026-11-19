// Written by `cargo run -p rune --example regen_dxbc` on Windows, the one way to regenerate the uber bytecode:
// it also writes the pinned fragments and their hashes (dxbc_pinned_hash.rs), which nothing else updates.
pub(super) const UBER_DXBC_WGSL_BLAKE3: &str = "9aa7c38383028185adec6c1904c03cc5321f70df8364a8c7854e9779af80997a";
pub(super) const UBER_DXBC_ABI_FNV64: u64 = 0x6b9932e7795ec011;
#[cfg(test)]
pub(super) const UBER_DXBC_HLSL_BLAKE3: &str = "ddaceaff671b0c855da37ade8086fe7aa2ddacf146d707862bce525667627c55";
#[cfg(test)]
pub(super) const UBER_DXBC_DUAL_HLSL_BLAKE3: &str = "5c0642d50adedade4bbf996c5c58d944c912d6bca11401e73d47847de19f66f1";
