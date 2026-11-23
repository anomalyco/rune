// Written by `cargo run -p rune --example regen_dxbc` on Windows, the one way to regenerate the uber bytecode:
// it also writes the pinned fragments and their hashes (dxbc_pinned_hash.rs), which nothing else updates.
pub(super) const UBER_DXBC_WGSL_BLAKE3: &str = "e529b76593c4cc18f0ef5a138619ec1fd2d8a886fc2610ddb375e3eb0552beb9";
pub(super) const UBER_DXBC_ABI_FNV64: u64 = 0x6b9932e7795ec011;
#[cfg(test)]
pub(super) const UBER_DXBC_HLSL_BLAKE3: &str = "28761767072b25f3725dee386cb14c3a22f8b53268699e2e6611b027d93b22f3";
#[cfg(test)]
pub(super) const UBER_DXBC_DUAL_HLSL_BLAKE3: &str = "8889314b0d9155189419b181f26778a4c609170079d420aed5819fc8cfe4c68b";
