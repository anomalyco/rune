// Written by `cargo run -p rune --example regen_dxbc` (or `RUNE_REGENERATE_DXBC=1 cargo test -p rune --lib uber_dxbc_is_fresh`) on Windows.
pub(super) const UBER_DXBC_WGSL_BLAKE3: &str = "79765e88f6ca20b8f542e895873bba6b9c20456f56dfadd65db60d303fc54208";
pub(super) const UBER_DXBC_ABI_FNV64: u64 = 0x6b9932e7795ec011;
#[cfg(test)]
pub(super) const UBER_DXBC_HLSL_BLAKE3: &str = "cc08997b069a57b88027d6f63f955fd0764ba0ace751be26e7e3ef46f5beca91";
#[cfg(test)]
pub(super) const UBER_DXBC_DUAL_HLSL_BLAKE3: &str = "2ede35b31283fbf3d17f790bee248d71754d1114179188e2ec42124e2d881cb9";
