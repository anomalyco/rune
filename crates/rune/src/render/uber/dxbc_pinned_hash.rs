// Written by `cargo run -p rune --example regen_dxbc` on Windows with uber.fs-k0/k1/k2.dxbc; nothing else writes them.
/// BLAKE3 of the WGSL each pinned fragment was compiled from (render/variant.rs `pinned_wgsl`, `PINNED` order).
pub(super) const UBER_DXBC_PINNED_WGSL_BLAKE3: [&str; 3] = ["98771f80ee5bfabfd0febb93f163e58bd6ecdb81d0e4070d66dc5e06ad2ce7d7", "eda1c84f5294f584c72a2c84212a014f22fad3f7942fe786b98cf5509a12c04c", "32675b1b61fed30aac35f6be5c3e411faba53476f153ce8718e6a3871f68ddaa"];
/// BLAKE3 of each pinned fragment's file, and of the `uber.vs.dxbc` it was compiled beside.
pub(super) const UBER_DXBC_PINNED_FS_BLAKE3: [&str; 3] = ["830aa4f8483c49226bb37626f500da2bc6f6dd4f6dc4f90da6620c4bd276b118", "296be353dbc960ab2c66d56bc04738a79b93605a2983ff629328ebcf7445101f", "28b82dca124dcf98b2e28474a2a8e115c2e5982f25da1eb2178e58647c1f763a"];
pub(super) const UBER_DXBC_PINNED_VS_BLAKE3: &str = "e6a25a34357594d88ffdbe2f8f6e242ee9b42aa46005cc71d067b4891bbdd42c";
