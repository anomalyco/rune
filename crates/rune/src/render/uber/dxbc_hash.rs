// Written by `cargo run -p rune --example regen_dxbc` on Windows, the one way to regenerate the uber bytecode:
// it also writes the pinned fragments and their hashes (dxbc_pinned_hash.rs), which nothing else updates.
pub(super) const UBER_DXBC_WGSL_BLAKE3: &str = "54888f45682dd582361a4b0783101cdb88f5d7f4c7c6f315c8dedc406a188f4b";
pub(super) const UBER_DXBC_ABI_FNV64: u64 = 0x6b9932e7795ec011;
#[cfg(test)]
pub(super) const UBER_DXBC_HLSL_BLAKE3: &str = "b9e32c1aa2b9a47c8a2fac7fb3aa86ba73bbe4ed2e719784a6ac27a628979fd1";
#[cfg(test)]
pub(super) const UBER_DXBC_DUAL_HLSL_BLAKE3: &str = "dc7867c147533433b91d1d8358d15388fd2e8d4975c0cab7820a3bdd3312b2ec";
