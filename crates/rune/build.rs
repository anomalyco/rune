//! Joins the uber shader's WGSL once, when Rune builds, so a launch doesn't.
//!
//! Classic is `clip.wgsl` and `shader.wgsl`; Windfoil (render/variant.rs) is Classic with
//! `outline.wgsl`'s glyph branch put before shader.wgsl's atlas branch, then outline.wgsl. Each
//! file's `\r\n` is read as `\n`, so a checkout with CRLF line endings (Git for Windows'
//! `core.autocrlf=true`) builds the same text, and the same hashes, as an LF one. The text and its
//! BLAKE3 go to `$OUT_DIR/uber_wgsl.rs` (render/uber_wgsl.rs includes it). When a hash differs from
//! the one recorded beside a precompiled shader (`src/render/uber/*_hash.rs`), the build says so
//! and names the command that rebuilds it: that shader isn't used until then.

use std::path::Path;

/// shader.wgsl's atlas glyph branch, which Windfoil puts the outline branch before.
const GLYPH_BRANCH: &str = "        if i.uv.z > 0.0 || rotated {";
const OUTLINE_BRANCH: &str = "        if i.radii.w > 0.0 {\n            cov = outline_glyph(i, screen);\n        } else if i.radii.x > 0.5 && !(i.uv.z > 0.0 || rotated) {\n            cov = lcd_mask(i, screen);\n        } else if i.uv.z > 0.0 || rotated {";

fn main() {
    println!("cargo::rerun-if-changed=build.rs");
    // Paths are from the package's folder, where cargo runs this.
    let read = |path: &str| {
        println!("cargo::rerun-if-changed={path}");
        let text = std::fs::read_to_string(path).unwrap_or_else(|e| panic!("{path}: {e}"));
        text.replace("\r\n", "\n")
    };
    let classic = format!("{}\n{}", read("src/clip.wgsl"), read("src/shader.wgsl"));
    assert!(classic.matches(GLYPH_BRANCH).count() == 1,
        "src/shader.wgsl must hold Windfoil's anchor `{GLYPH_BRANCH}` exactly once (build.rs puts outline.wgsl's glyph branch before it)");
    let windfoil = format!("{}\n{}", classic.replacen(GLYPH_BRANCH, OUTLINE_BRANCH, 1), read("src/outline.wgsl"));
    let (classic_hash, windfoil_hash) = (blake3::hash(classic.as_bytes()).to_hex(), blake3::hash(windfoil.as_bytes()).to_hex());

    let recorded = |path: &str, name: &str| {
        let file = read(path);
        let at = file.find(&format!("{name}: &str = \"")).unwrap_or_else(|| panic!("{path} records no {name}"));
        file[at..].split('"').nth(1).unwrap_or_default().to_owned()
    };
    let stale = |what: &str, recorded: String, now: &str, fix: &str| if recorded != now {
        println!("cargo::warning={what} was built from other WGSL (blake3 {recorded}, the sources now hash {now}), so it isn't used: run {fix}");
    };
    stale("The precompiled DX12 uber shader (uber.vs.dxbc, uber.fs.dxbc)", recorded("src/render/uber/dxbc_hash.rs", "UBER_DXBC_WGSL_BLAKE3"),
        &windfoil_hash, "`cargo run -p rune --example regen_dxbc` on Windows; until then every Windows launch compiles it with FXC, which takes seconds");
    stale("The precompiled Metal uber shader (uber.metallib)", recorded("src/render/uber/metallib_hash.rs", "UBER_WGSL_BLAKE3"),
        &classic_hash, "scripts/precompile-metal-shaders.sh on a Mac");
    stale("The precompiled Metal Windfoil shader (uber_windfoil.metallib)", recorded("src/render/uber/windfoil_hash.rs", "WINDFOIL_WGSL_BLAKE3"),
        &windfoil_hash, "scripts/precompile-metal-shaders.sh on a Mac");

    let out = Path::new(&std::env::var_os("OUT_DIR").expect("cargo sets OUT_DIR")).to_owned();
    let write = |name: &str, text: &str| std::fs::write(out.join(name), text).unwrap_or_else(|e| panic!("{name}: {e}"));
    write("uber_classic.wgsl", &classic);
    write("uber_windfoil.wgsl", &windfoil);
    write("uber_wgsl.rs", &format!(
        "/// clip.wgsl and shader.wgsl.\n\
         pub(crate) const CLASSIC: &str = include_str!(concat!(env!(\"OUT_DIR\"), \"/uber_classic.wgsl\"));\n\
         /// BLAKE3 of [`CLASSIC`], which only macOS ships a library for.\n\
         #[cfg_attr(not(target_os = \"macos\"), allow(dead_code))]\n\
         pub(crate) const CLASSIC_BLAKE3: &str = \"{classic_hash}\";\n\
         /// Classic with outline.wgsl's glyph branch, then outline.wgsl.\n\
         pub(crate) const WINDFOIL: &str = include_str!(concat!(env!(\"OUT_DIR\"), \"/uber_windfoil.wgsl\"));\n\
         /// BLAKE3 of [`WINDFOIL`], which Windows and macOS ship bytecode for.\n\
         #[cfg_attr(not(any(target_os = \"macos\", target_os = \"windows\")), allow(dead_code))]\n\
         pub(crate) const WINDFOIL_BLAKE3: &str = \"{windfoil_hash}\";\n"));
}
