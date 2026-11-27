//! Normalizes `src/springs.wgsl` line endings and strips comments/whitespace on `wasm32`.

use std::path::Path;

fn main() {
    println!("cargo::rerun-if-changed=build.rs");
    println!("cargo::rerun-if-changed=src/springs.wgsl");
    let raw = std::fs::read_to_string("src/springs.wgsl")
        .unwrap_or_else(|e| panic!("src/springs.wgsl: {e}"))
        .replace("\r\n", "\n");
    let is_wasm = std::env::var_os("CARGO_CFG_TARGET_ARCH").as_deref() == Some(std::ffi::OsStr::new("wasm32"));
    let out_wgsl = if is_wasm { minify(&raw) } else { raw };
    let out = Path::new(&std::env::var_os("OUT_DIR").expect("cargo sets OUT_DIR")).join("springs.wgsl");
    std::fs::write(&out, out_wgsl).unwrap_or_else(|e| panic!("{}: {e}", out.display()));
}

fn minify(src: &str) -> String {
    let mut out = String::with_capacity(src.len());
    for raw_line in src.lines() {
        let code = raw_line.split_once("//").map_or(raw_line, |(code, _)| code).trim();
        if code.is_empty() {
            continue;
        }
        if !out.is_empty() {
            out.push('\n');
        }
        let mut prev_ws = false;
        for b in code.bytes() {
            if b == b' ' || b == b'\t' {
                if !prev_ws {
                    out.push(' ');
                    prev_ws = true;
                }
            } else {
                out.push(b as char);
                prev_ws = false;
            }
        }
    }
    out
}
