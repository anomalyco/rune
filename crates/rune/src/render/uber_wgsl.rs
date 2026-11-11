//! The uber shader's WGSL, Classic and Windfoil, joined when Rune builds (`build.rs`) with every
//! `\r\n` read as `\n`, and the BLAKE3 of each: the precompiled shaders are used when the hash
//! recorded beside them is this one, which the build knows, so a launch hashes nothing.

include!(concat!(env!("OUT_DIR"), "/uber_wgsl.rs"));

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
