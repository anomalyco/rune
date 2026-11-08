//! On `wasm32-unknown-unknown`, Luau's C++ is compiled by WASI SDK's clang against WASI libc with
//! WebAssembly exception handling (`scripts/web-build.sh` sets the compiler up). The C++ runtime,
//! the unwinder and libc come from the same SDK, so link them here: any app that depends on this
//! crate then links on the web without flags of its own.

fn main() {
    println!("cargo:rerun-if-env-changed=WASI_SDK_PATH");
    println!("cargo::rustc-check-cfg=cfg(luau_jit)");
    let var = |name: &str| std::env::var_os(name).map(|v| v.to_string_lossy().into_owned()).unwrap_or_default();
    let (os, arch) = (var("CARGO_CFG_TARGET_OS"), var("CARGO_CFG_TARGET_ARCH"));
    // Must match the `luau-jit` target condition in Cargo.toml.
    if matches!(os.as_str(), "windows" | "linux") && matches!(arch.as_str(), "x86_64" | "aarch64") {
        println!("cargo:rustc-cfg=luau_jit");
    }
    if var("TARGET") != "wasm32-unknown-unknown" {
        return;
    }
    let Some(sdk) = std::env::var_os("WASI_SDK_PATH") else {
        println!("cargo:warning=rune-luau on wasm32-unknown-unknown needs WASI SDK: set WASI_SDK_PATH (scripts/web-build.sh does)");
        return;
    };
    let lib = std::path::Path::new(&sdk).join("share").join("wasi-sysroot").join("lib").join("wasm32-wasip1");
    println!("cargo:rustc-link-search=native={}", lib.join("eh").display());
    println!("cargo:rustc-link-search=native={}", lib.display());
    // `clock()`, which `os.clock` reads, comes from the emulated process clocks.
    for name in ["c++abi", "unwind", "wasi-emulated-process-clocks", "c"] {
        println!("cargo:rustc-link-lib=static={name}");
    }
}
