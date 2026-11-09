//! Translates `vendor/highlights.wasm` to Rust (`$OUT_DIR/highlights.rs`), which `src/lib.rs`
//! includes. See `codegen/translate.rs`.

#[path = "codegen/translate.rs"]
mod translate;

fn main() {
    println!("cargo:rerun-if-changed=vendor/highlights.wasm");
    println!("cargo:rerun-if-changed=codegen/translate.rs");
    let wasm = std::fs::read("vendor/highlights.wasm").unwrap_or_else(|e| panic!("vendor/highlights.wasm: {e}"));
    let rust = translate::translate(&wasm).unwrap_or_else(|e| panic!("vendor/highlights.wasm: {e}"));
    let out = std::path::PathBuf::from(std::env::var_os("OUT_DIR").unwrap_or_else(|| panic!("OUT_DIR is unset")));
    std::fs::write(out.join("highlights.rs"), rust).unwrap_or_else(|e| panic!("highlights.rs: {e}"));
}
