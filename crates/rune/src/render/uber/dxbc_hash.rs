// Written by `cargo run -p rune --example regen_dxbc` on Windows, the one way to regenerate the uber bytecode:
// it also writes the pinned fragments and their hashes (dxbc_pinned_hash.rs), which nothing else updates.
pub(super) const UBER_DXBC_WGSL_BLAKE3: &str = "49bedcc0ca95d26814fadaf2c3644299e1c7957ab1a87401721eccbab61f7d95";
pub(super) const UBER_DXBC_ABI_FNV64: u64 = 0x6b9932e7795ec011;
#[cfg(test)]
pub(super) const UBER_DXBC_HLSL_BLAKE3: &str = "75a7d74f78cfdcf23d4f54b1afb87ba2bc58b7e0eb991f5b60cf574da05eb5e7";
#[cfg(test)]
pub(super) const UBER_DXBC_DUAL_HLSL_BLAKE3: &str = "78a327511a1b7ab0d92df56ce809e09bcaaca02fc93182c13b7b368101490c9f";
