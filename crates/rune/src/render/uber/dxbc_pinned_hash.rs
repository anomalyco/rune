// Written by `cargo run -p rune --example regen_dxbc` on Windows with uber.fs-k0/k1/k2.dxbc; nothing else writes them.
/// BLAKE3 of the WGSL each pinned fragment was compiled from (render/variant.rs `pinned_wgsl`, `PINNED` order).
pub(super) const UBER_DXBC_PINNED_WGSL_BLAKE3: [&str; 3] = ["9eca875009982a625c9651f1607ea8f1f8555544ebb52540f4d1110cd47593d0", "5292e65d1b1ce7c3aef2fcc4f1b7fb8cf0b5a5f4126bbc174c5f75b89f60f24f", "fe117a1b204692a32edd0135bb780afe2d34c175f3fd6afee73f4c4486fc8ad3"];
/// BLAKE3 of each pinned fragment's file, and of the `uber.vs.dxbc` it was compiled beside.
pub(super) const UBER_DXBC_PINNED_FS_BLAKE3: [&str; 3] = ["eec4c3120cce80805d742a40942b685dada3fd3454b3a56f44c379059f9e8d77", "d65504b8e6fad69bfc30cd9b64a7d7c8025e747f74c4603257d971fa9b59a677", "722a6a3745d70794edbd12fa2490543f46db088edb25c1ea14e0097272c5f0d8"];
pub(super) const UBER_DXBC_PINNED_VS_BLAKE3: &str = "e6a25a34357594d88ffdbe2f8f6e242ee9b42aa46005cc71d067b4891bbdd42c";
