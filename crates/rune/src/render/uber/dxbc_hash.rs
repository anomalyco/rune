// Written by `cargo run -p rune --example regen_dxbc` on Windows, the one way to regenerate the uber bytecode:
// it also writes the pinned fragments and their hashes (dxbc_pinned_hash.rs), which nothing else updates.
pub(super) const UBER_DXBC_WGSL_BLAKE3: &str = "2612df3ed023cd9c354ef7bad21d89ed841204272bdd647649c8a081975ab513";
pub(super) const UBER_DXBC_ABI_FNV64: u64 = 0x6b9932e7795ec011;
#[cfg(test)]
pub(super) const UBER_DXBC_HLSL_BLAKE3: &str = "79d6ddecce1d35c73563f3e192a921b82f96c3809ab706d950ababa4bc385772";
#[cfg(test)]
pub(super) const UBER_DXBC_DUAL_HLSL_BLAKE3: &str = "784260787fb0d0ad9d6c400f0f4ef31e129f639dfbd24ef7ce1fa24b7e670c95";
