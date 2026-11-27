//! Embedded WGSL shader sources and build-time BLAKE3 hashes (`build.rs`).
//!
//! Each shader file is embedded once (normalized LF on native; comment- and whitespace-stripped
//! on `wasm32`). Classic and Windfoil uber sources are assembled on demand and cached in a
//! `OnceLock` so `clip.wgsl` and `shader.wgsl` are never duplicated in `.rodata`.

use std::sync::OnceLock;

include!(concat!(env!("OUT_DIR"), "/uber_wgsl.rs"));

/// `clip.wgsl` followed by `shader.wgsl`, assembled once on first shader compile.
pub(crate) fn classic() -> &'static str {
    static CLASSIC: OnceLock<String> = OnceLock::new();
    CLASSIC.get_or_init(|| format!("{CLIP}\n{SHADER}")).as_str()
}

/// Classic with `outline.wgsl`'s glyph branch spliced in, followed by `outline.wgsl`.
pub(crate) fn windfoil() -> &'static str {
    static WINDFOIL: OnceLock<String> = OnceLock::new();
    WINDFOIL
        .get_or_init(|| format!("{CLIP}\n{}\n{OUTLINE}", SHADER.replacen(GLYPH_BRANCH, OUTLINE_BRANCH, 1)))
        .as_str()
}

/// Whether two strings are equal, for constants.
#[cfg(any(target_os = "macos", target_os = "windows"))]
pub(super) const fn same(a: &str, b: &str) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    if a.len() != b.len() { return false; }
    let mut i = 0;
    while i < a.len() {
        if a[i] != b[i] { return false; }
        i += 1;
    }
    true
}
