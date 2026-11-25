// Written by `cargo run -p rune --example regen_dxbc` on Windows with uber.fs-k0/k1/k2.dxbc; nothing else writes them.
/// BLAKE3 of the WGSL each pinned fragment was compiled from (render/variant.rs `pinned_wgsl`, `PINNED` order).
pub(super) const UBER_DXBC_PINNED_WGSL_BLAKE3: [&str; 3] = ["63007443f8344f82f36fd46adb524616ef80038c9dbf9c166cef0d03590ae824", "8c6ac2cdeb21793bf51a264fcc88139000c0a97031a942b82d006fe2996ed481", "90d1efea1b965bbdf7b15a61a2db51d0660d7ba5ec913c0cc5627e7f33075a88"];
/// BLAKE3 of each pinned fragment's file, and of the `uber.vs.dxbc` it was compiled beside.
pub(super) const UBER_DXBC_PINNED_FS_BLAKE3: [&str; 3] = ["c36098265655eb235780db672c0dd61ddbe0cb24e8e980157a11c2d0253d8b32", "373bbf31ceaaab776daec4f86934850633e3c84dc76f650cb9afba72756cfcd6", "206a8411f8fc76a2c247a08d8f8f6a3f73c07d666dedf73b7ca8b59eb2b8104e"];
pub(super) const UBER_DXBC_PINNED_VS_BLAKE3: &str = "e6a25a34357594d88ffdbe2f8f6e242ee9b42aa46005cc71d067b4891bbdd42c";
