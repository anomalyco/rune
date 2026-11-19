// Written by `cargo run -p rune --example regen_dxbc` on Windows with uber.fs-k0/k1/k2.dxbc; nothing else writes them.
/// BLAKE3 of the WGSL each pinned fragment was compiled from (render/variant.rs `pinned_wgsl`, `PINNED` order).
pub(super) const UBER_DXBC_PINNED_WGSL_BLAKE3: [&str; 3] = ["6b070bd774538ec97a20320605cf3c6a67d729d00b83a80080aab5f4d53d1885", "cbc9a8bc1e336ae652bff3f1828bf4ce885f36cb630fe2205a182c847804442b", "983f93163d1c86e722346ef0fb0157c7fb56cfc4b5d3ddecd162172babd50f3e"];
/// BLAKE3 of each pinned fragment's file, and of the `uber.vs.dxbc` it was compiled beside.
pub(super) const UBER_DXBC_PINNED_FS_BLAKE3: [&str; 3] = ["830aa4f8483c49226bb37626f500da2bc6f6dd4f6dc4f90da6620c4bd276b118", "296be353dbc960ab2c66d56bc04738a79b93605a2983ff629328ebcf7445101f", "28b82dca124dcf98b2e28474a2a8e115c2e5982f25da1eb2178e58647c1f763a"];
pub(super) const UBER_DXBC_PINNED_VS_BLAKE3: &str = "e6a25a34357594d88ffdbe2f8f6e242ee9b42aa46005cc71d067b4891bbdd42c";
