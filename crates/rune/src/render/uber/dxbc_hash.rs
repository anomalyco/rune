// Written by `cargo run -p rune --example regen_dxbc` on Windows, the one way to regenerate the uber bytecode:
// it also writes the pinned fragments and their hashes (dxbc_pinned_hash.rs), which nothing else updates.
pub(super) const UBER_DXBC_WGSL_BLAKE3: &str = "b9a7b5c15350efb50ed9d119e3f83d1c9b6714725a04a3170c68f0f97e8cb179";
pub(super) const UBER_DXBC_ABI_FNV64: u64 = 0x6b9932e7795ec011;
#[cfg(test)]
pub(super) const UBER_DXBC_HLSL_BLAKE3: &str = "46696a644fb00ac0a5b8cfa1f20069f404770ae0263365332402f99f4751fcd5";
#[cfg(test)]
pub(super) const UBER_DXBC_DUAL_HLSL_BLAKE3: &str = "bb9680bf6e824fb6a706d9246ecc088b075336fd75d0a0aacdf454b99c950b64";
