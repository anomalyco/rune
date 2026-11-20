// Written by `cargo run -p rune --example regen_dxbc` on Windows with uber.fs-k0/k1/k2.dxbc; nothing else writes them.
/// BLAKE3 of the WGSL each pinned fragment was compiled from (render/variant.rs `pinned_wgsl`, `PINNED` order).
pub(super) const UBER_DXBC_PINNED_WGSL_BLAKE3: [&str; 3] = ["3ab56dc3ffe0f96d20aab67e98f64bc927d7e4bf4e1f7f7be01371515ca9c4df", "e52696372df923332fc445ff7088ea4371f6db3fe95b87b141253937bae484bc", "5106d7218f2e81400c0a8cb3784d7e631d8a8380e1e7196f6ab1f221970eafe8"];
/// BLAKE3 of each pinned fragment's file, and of the `uber.vs.dxbc` it was compiled beside.
pub(super) const UBER_DXBC_PINNED_FS_BLAKE3: [&str; 3] = ["99211854c0eff57e4285559b0f33014cc7dcac7847756a101c2affb49c021fea", "dc388135a8e95bf50073234d20bc7397ee28c90f78f86e54a03c6e1c36917407", "730bf58e436edda0adabc7b557749e53237ffcf8388c36cc699943004ade062a"];
pub(super) const UBER_DXBC_PINNED_VS_BLAKE3: &str = "e6a25a34357594d88ffdbe2f8f6e242ee9b42aa46005cc71d067b4891bbdd42c";
