// Written by `cargo run -p rune --example regen_dxbc` on Windows, the one way to regenerate the uber bytecode:
// it also writes the pinned fragments and their hashes (dxbc_pinned_hash.rs), which nothing else updates.
pub(super) const UBER_DXBC_WGSL_BLAKE3: &str = "bb514eea8b80991dac8a73cdc6ab550a568a3769bf684dca2b0d04a82c097e0a";
pub(super) const UBER_DXBC_ABI_FNV64: u64 = 0x6b9932e7795ec011;
#[cfg(test)]
pub(super) const UBER_DXBC_HLSL_BLAKE3: &str = "51bb1f13c2be5eaaf1ab4891db5e0f3b4f97905dede8a85c34892cae856303ba";
#[cfg(test)]
pub(super) const UBER_DXBC_DUAL_HLSL_BLAKE3: &str = "4bb1f69a6e81f618f6107a63cfd52827a4cc83f0fc50f13bdf86ef469556abbb";
