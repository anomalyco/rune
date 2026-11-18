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

/// shader.wgsl's marker for the kind a fragment draws (render/variant.rs `PINNED_OFF`).
const PINNED_OFF: &str = "const PINNED_KIND: f32 = -1.0;";
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
    // The frame pass's fragments pinned per kind (render/variant.rs `PINNED`, `pinned_wgsl`): the WGSL each compiles
    // from, and the BLAKE3 of the shipped bytecode beside the shared vertex stage. render/dxbc.rs uses a pinned
    // fragment only while all three agree with what regen_dxbc recorded (uber/dxbc_pinned_hash.rs), so nothing is
    // hashed at run time, and a fragment left from another shader, another wgpu or another vertex stage isn't used.
    let pinned_wgsl: Vec<String> = [0, 1, 2].iter().map(|kind| {
        assert!(windfoil.matches(PINNED_OFF).count() == 1, "src/shader.wgsl must hold `{PINNED_OFF}` exactly once");
        blake3::hash(windfoil.replacen(PINNED_OFF, &format!("const PINNED_KIND: f32 = {kind}.0;"), 1).as_bytes()).to_hex().to_string()
    }).collect();
    let file = |path: &str| {
        println!("cargo::rerun-if-changed={path}");
        blake3::hash(&std::fs::read(path).unwrap_or_else(|e| panic!("{path}: {e}"))).to_hex().to_string()
    };
    let pinned_fs: Vec<String> = [0, 1, 2].iter().map(|kind| file(&format!("uber.fs-k{kind}.dxbc"))).collect();
    let pinned_vs = file("uber.vs.dxbc");
    // What regen_dxbc recorded, in `PINNED` order: a list's quoted entries, in place.
    let recorded_list = |name: &str| {
        let file = read("src/render/uber/dxbc_pinned_hash.rs");
        let at = file.find(&format!("{name}: [&str; ")).unwrap_or_else(|| panic!("dxbc_pinned_hash.rs records no {name}"));
        let list = &file[at..];
        let list = &list[list.find("= [").map_or(0, |i| i + 3)..list.find("];").unwrap_or(list.len())];
        list.split('"').skip(1).step_by(2).map(str::to_owned).collect::<Vec<_>>()
    };
    let (recorded_wgsl, recorded_fs) = (recorded_list("UBER_DXBC_PINNED_WGSL_BLAKE3"), recorded_list("UBER_DXBC_PINNED_FS_BLAKE3"));
    let recorded_vs = recorded("src/render/uber/dxbc_pinned_hash.rs", "UBER_DXBC_PINNED_VS_BLAKE3");
    let stale_kinds: Vec<usize> = (0..3).filter(|&at| {
        recorded_wgsl.get(at) != Some(&pinned_wgsl[at]) || recorded_fs.get(at) != Some(&pinned_fs[at]) || recorded_vs != pinned_vs
    }).collect();
    if !stale_kinds.is_empty() {
        println!("cargo::warning=The precompiled DX12 fragments pinned to kinds {stale_kinds:?} (uber.fs-k*.dxbc) weren't compiled from this \
            shader beside this uber.vs.dxbc, or aren't the files regen_dxbc wrote, so they aren't used: Windows scrolling draws on the slow \
            path (the general shader) until `cargo run -p rune --example regen_dxbc` is run on Windows");
    }
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
         pub(crate) const WINDFOIL_BLAKE3: &str = \"{windfoil_hash}\";\n\
         /// BLAKE3 of the WGSL of each fragment pinned to a kind (render/variant.rs `pinned_wgsl`, `PINNED` order).\n\
         #[cfg_attr(not(target_os = \"windows\"), allow(dead_code))]\n\
         pub(crate) const PINNED_WGSL_BLAKE3: [&str; 3] = [\"{}\", \"{}\", \"{}\"];\n\
         /// BLAKE3 of the shipped `uber.fs-k0/k1/k2.dxbc`, and of the `uber.vs.dxbc` they draw beside.\n\
         #[cfg_attr(not(target_os = \"windows\"), allow(dead_code))]\n\
         pub(crate) const PINNED_FS_BLAKE3: [&str; 3] = [\"{}\", \"{}\", \"{}\"];\n\
         #[cfg_attr(not(target_os = \"windows\"), allow(dead_code))]\n\
         pub(crate) const PINNED_VS_BLAKE3: &str = \"{pinned_vs}\";\n",
        pinned_wgsl[0], pinned_wgsl[1], pinned_wgsl[2], pinned_fs[0], pinned_fs[1], pinned_fs[2]));
}
