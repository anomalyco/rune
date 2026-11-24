// Written by `cargo run -p rune --example regen_dxbc` on Windows with uber.fs-k0/k1/k2.dxbc; nothing else writes them.
/// BLAKE3 of the WGSL each pinned fragment was compiled from (render/variant.rs `pinned_wgsl`, `PINNED` order).
pub(super) const UBER_DXBC_PINNED_WGSL_BLAKE3: [&str; 3] = ["078ab396284adb181c3effb004cbe1002cd914e3b3dd3d963a84ef7bf3e4d331", "0075cc9e1feafe84dbd96e6f579784920c180a3e162d1ea5bb4f1467083916e5", "e91bd52c979f24a97ae878732edad24045eb11fb9d14d8167f5a9613a5de15ac"];
/// BLAKE3 of each pinned fragment's file, and of the `uber.vs.dxbc` it was compiled beside.
pub(super) const UBER_DXBC_PINNED_FS_BLAKE3: [&str; 3] = ["c36098265655eb235780db672c0dd61ddbe0cb24e8e980157a11c2d0253d8b32", "373bbf31ceaaab776daec4f86934850633e3c84dc76f650cb9afba72756cfcd6", "206a8411f8fc76a2c247a08d8f8f6a3f73c07d666dedf73b7ca8b59eb2b8104e"];
pub(super) const UBER_DXBC_PINNED_VS_BLAKE3: &str = "e6a25a34357594d88ffdbe2f8f6e242ee9b42aa46005cc71d067b4891bbdd42c";
