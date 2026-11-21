// Written by `cargo run -p rune --example regen_dxbc` on Windows with uber.fs-k0/k1/k2.dxbc; nothing else writes them.
/// BLAKE3 of the WGSL each pinned fragment was compiled from (render/variant.rs `pinned_wgsl`, `PINNED` order).
pub(super) const UBER_DXBC_PINNED_WGSL_BLAKE3: [&str; 3] = ["1167292b3c2b91cdbf76e2f72971eda7922f3983c571a54eb1d3534526e24d9a", "6ccb754d19b24a58a1ae12bca1eb148e5264370d19f77f0fa63e516b44dda2f3", "5b804b75ff560e9da89f98214577fb1d651f3114c6f9c2866d0e1045e84f1077"];
/// BLAKE3 of each pinned fragment's file, and of the `uber.vs.dxbc` it was compiled beside.
pub(super) const UBER_DXBC_PINNED_FS_BLAKE3: [&str; 3] = ["27f1ae9dc85c8a789d573fe175966fb8533e847a80b37ad7f592adb5409abf2a", "daaf0d13c697b270b4d22942414d2624146069b95752962ff8b4b5a4a8219feb", "c5bfbb288a39e79a59bc037d6e3e76f8e29dfbf5513460b6356ebf01e96fc923"];
pub(super) const UBER_DXBC_PINNED_VS_BLAKE3: &str = "e6a25a34357594d88ffdbe2f8f6e242ee9b42aa46005cc71d067b4891bbdd42c";
