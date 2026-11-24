// Written by `cargo run -p rune --example regen_dxbc` on Windows, the one way to regenerate the uber bytecode:
// it also writes the pinned fragments and their hashes (dxbc_pinned_hash.rs), which nothing else updates.
pub(super) const UBER_DXBC_WGSL_BLAKE3: &str = "a7871e32e521d0183b78cfce331d937406311d11b39a35beacd5b5a09d771275";
pub(super) const UBER_DXBC_ABI_FNV64: u64 = 0x6b9932e7795ec011;
#[cfg(test)]
pub(super) const UBER_DXBC_HLSL_BLAKE3: &str = "567016ccc0807cf87723ef83a3fb7457e2c340c40d15cc72dfaa843c6435f30a";
#[cfg(test)]
pub(super) const UBER_DXBC_DUAL_HLSL_BLAKE3: &str = "0021e9c4712fa46cac85648c7d13b395c1902f75b71fe79aeb1e364973ccb7f4";
