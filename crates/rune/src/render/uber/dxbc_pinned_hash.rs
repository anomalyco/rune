// Written by `cargo run -p rune --example regen_dxbc` on Windows with uber.fs-k0/k1/k2.dxbc; nothing else writes them.
/// BLAKE3 of the WGSL each pinned fragment was compiled from (render/variant.rs `pinned_wgsl`, `PINNED` order).
pub(super) const UBER_DXBC_PINNED_WGSL_BLAKE3: [&str; 3] = ["98771f80ee5bfabfd0febb93f163e58bd6ecdb81d0e4070d66dc5e06ad2ce7d7", "eda1c84f5294f584c72a2c84212a014f22fad3f7942fe786b98cf5509a12c04c", "32675b1b61fed30aac35f6be5c3e411faba53476f153ce8718e6a3871f68ddaa"];
/// BLAKE3 of the Windfoil WGSL they were made from (build.rs warns when it moves).
#[allow(dead_code)]
pub(super) const UBER_DXBC_PINNED_FROM_BLAKE3: &str = "7b009be21463342c93146b1c67c68187ba60864286e44da37760ddda3b05c24e";
