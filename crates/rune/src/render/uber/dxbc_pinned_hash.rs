// Written by `cargo run -p rune --example regen_dxbc` on Windows with uber.fs-k0/k1/k2.dxbc; nothing else writes them.
/// BLAKE3 of the WGSL each pinned fragment was compiled from (render/variant.rs `pinned_wgsl`, `PINNED` order).
pub(super) const UBER_DXBC_PINNED_WGSL_BLAKE3: [&str; 3] = ["423336bbb22de2f3711856c1eb3c2fce89aaeba360e00916548d2f9f7d7d2fb0", "47b8e321fb8100f2d49ae4346e54910943c763e3477f17911abcb1add303277c", "42acc193d2a8768f2f2ecf2c7d3cc5ad8f220e376b6c2f3ba8a7e2364bc12346"];
/// BLAKE3 of each pinned fragment's file, and of the `uber.vs.dxbc` it was compiled beside.
pub(super) const UBER_DXBC_PINNED_FS_BLAKE3: [&str; 3] = ["eec4c3120cce80805d742a40942b685dada3fd3454b3a56f44c379059f9e8d77", "f1cbcc971dd818a8e4147f8e3c5086bb6fd80143d658cd6f3fb7e7668e7696cc", "722a6a3745d70794edbd12fa2490543f46db088edb25c1ea14e0097272c5f0d8"];
pub(super) const UBER_DXBC_PINNED_VS_BLAKE3: &str = "e6a25a34357594d88ffdbe2f8f6e242ee9b42aa46005cc71d067b4891bbdd42c";
