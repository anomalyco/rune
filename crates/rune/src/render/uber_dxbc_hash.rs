// Written by `cargo run -p rune --example regenerate_dxbc`, or by
// `RUNE_REGENERATE_DXBC=1 cargo test -p rune --lib uber_dxbc_is_fresh`, on Windows.
pub(super) const UBER_DXBC_WGSL_BLAKE3: &str = "9baee9d980771e98b0300008f119722ad1ab239d06315ed8faff23a3e3a842bd";
pub(super) const UBER_DXBC_ABI_FNV64: u64 = 0xb53b72e11574bdc1;
#[cfg(test)]
pub(super) const UBER_DXBC_HLSL_BLAKE3: &str = "3637d9772aa83cf5906f4397bf2005996796667481bac06adbd9e42a6342718d";
