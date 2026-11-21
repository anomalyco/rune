// Written by `cargo run -p rune --example regen_dxbc` on Windows with uber.fs-k0/k1/k2.dxbc; nothing else writes them.
/// BLAKE3 of the WGSL each pinned fragment was compiled from (render/variant.rs `pinned_wgsl`, `PINNED` order).
pub(super) const UBER_DXBC_PINNED_WGSL_BLAKE3: [&str; 3] = ["5125cf94b7b9b37e498146733e07687c3791ffec0147ee6c403b66a1631db4b7", "152a1efbf0858354cca4c8a13b6f0572ef8fe97e31a39f5526acc585b736c77b", "0837d27c22258dd1e8a47d35819e9f503bd8ea24731acc850cc2af1b02cd5567"];
/// BLAKE3 of each pinned fragment's file, and of the `uber.vs.dxbc` it was compiled beside.
pub(super) const UBER_DXBC_PINNED_FS_BLAKE3: [&str; 3] = ["79f401f11e2380987cbfcfa18851c428c6c128f31d47e3cffc9a7568c49331d6", "c18d15e60f3791bef365ef483839ffb267f563dee64a2e7eda517260aa029c9c", "6efe5251d65fc250b8ac2ea4d1ecfd0ab1887f53433e67bc8b2364cf66d8b0c5"];
pub(super) const UBER_DXBC_PINNED_VS_BLAKE3: &str = "e6a25a34357594d88ffdbe2f8f6e242ee9b42aa46005cc71d067b4891bbdd42c";
