// Written by `cargo run -p rune --example regen_dxbc` on Windows, the one way to regenerate the uber bytecode:
// it also writes the pinned fragments and their hashes (dxbc_pinned_hash.rs), which nothing else updates.
pub(super) const UBER_DXBC_WGSL_BLAKE3: &str = "7b009be21463342c93146b1c67c68187ba60864286e44da37760ddda3b05c24e";
pub(super) const UBER_DXBC_ABI_FNV64: u64 = 0x6b9932e7795ec011;
#[cfg(test)]
pub(super) const UBER_DXBC_HLSL_BLAKE3: &str = "c2116e4d73ebd0b70aa2644865586c127c4c81c799900e26ddb26eee9df920a8";
#[cfg(test)]
pub(super) const UBER_DXBC_DUAL_HLSL_BLAKE3: &str = "b7cc10e9f7fd642df6d69d4fa79389dee675f0da0b17a5323f83532d5863cb23";
