// Written by `cargo run -p rune --example regen_dxbc` on Windows, the one way to regenerate the uber bytecode:
// it also writes the pinned fragments and their hashes (dxbc_pinned_hash.rs), which nothing else updates.
pub(super) const UBER_DXBC_WGSL_BLAKE3: &str = "b87df1a9418d42c506fcd841f3720ae40c0cd7ec0067300bc2872b101b845309";
pub(super) const UBER_DXBC_ABI_FNV64: u64 = 0x6b9932e7795ec011;
#[cfg(test)]
pub(super) const UBER_DXBC_HLSL_BLAKE3: &str = "fc513886385c54a94abea5ea2937e3f682136cfe223d0248953733ab3b50e556";
#[cfg(test)]
pub(super) const UBER_DXBC_DUAL_HLSL_BLAKE3: &str = "76c7e160e46bd6fcb4f597d7ac0dd9baed84c6e3950623d012c6e0158e447971";
