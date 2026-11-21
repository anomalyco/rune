//! Deterministic builder and freshness verifier for `crates/rune-kit/assets/math/*` and
//! `crates/rune-kit/src/math/wasm_hash.rs`.

use std::path::{Path, PathBuf};
use std::process::Command;

const FONT_NAMES: [&str; 20] = [
    "AMS-Regular",
    "Caligraphic-Bold",
    "Caligraphic-Regular",
    "Fraktur-Bold",
    "Fraktur-Regular",
    "Main-Bold",
    "Main-BoldItalic",
    "Main-Italic",
    "Main-Regular",
    "Math-BoldItalic",
    "Math-Italic",
    "SansSerif-Bold",
    "SansSerif-Italic",
    "SansSerif-Regular",
    "Script-Regular",
    "Size1-Regular",
    "Size2-Regular",
    "Size3-Regular",
    "Size4-Regular",
    "Typewriter-Regular",
];

fn sha256(data: &[u8]) -> [u8; 32] {
    const K: [u32; 64] = [
        0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5,
        0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174,
        0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da,
        0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967,
        0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85,
        0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070,
        0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
        0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
    ];
    let mut h: [u32; 8] = [
        0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a,
        0x510e527f, 0x9b05688c, 0x1f83d9ab, 0x5be0cd19,
    ];
    let bit_len = (data.len() as u64).wrapping_mul(8);
    let mut msg = Vec::with_capacity(((data.len() + 9 + 63) / 64) * 64);
    msg.extend_from_slice(data);
    msg.push(0x80);
    while (msg.len() % 64) != 56 {
        msg.push(0);
    }
    msg.extend_from_slice(&bit_len.to_be_bytes());
    for chunk in msg.chunks_exact(64) {
        let mut w = [0u32; 64];
        for i in 0..16 {
            w[i] = u32::from_be_bytes([chunk[4 * i], chunk[4 * i + 1], chunk[4 * i + 2], chunk[4 * i + 3]]);
        }
        for i in 16..64 {
            let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
            let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16].wrapping_add(s0).wrapping_add(w[i - 7]).wrapping_add(s1);
        }
        let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut hh] = h;
        for i in 0..64 {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let ch = (e & f) ^ ((!e) & g);
            let t1 = hh.wrapping_add(s1).wrapping_add(ch).wrapping_add(K[i]).wrapping_add(w[i]);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let maj = (a & b) ^ (a & c) ^ (b & c);
            let t2 = s0.wrapping_add(maj);
            hh = g;
            g = f;
            f = e;
            e = d.wrapping_add(t1);
            d = c;
            c = b;
            b = a;
            a = t1.wrapping_add(t2);
        }
        for (slot, delta) in h.iter_mut().zip([a, b, c, d, e, f, g, hh]) {
            *slot = slot.wrapping_add(delta);
        }
    }
    let mut out = [0u8; 32];
    for (i, word) in h.iter().enumerate() {
        out[4 * i..4 * i + 4].copy_from_slice(&word.to_be_bytes());
    }
    out
}

fn hex(bytes: &[u8; 32]) -> String {
    let mut s = String::with_capacity(64);
    for b in bytes {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

fn hash_guest_sources(guest_dir: &Path) -> String {
    let mut buf = Vec::new();
    for rel in ["rust-toolchain.toml", "Cargo.toml", "Cargo.lock", "src/lib.rs"] {
        let p = guest_dir.join(rel);
        let raw = std::fs::read(&p).unwrap_or_else(|e| panic!("read {}: {e}", p.display()));
        buf.extend_from_slice(rel.as_bytes());
        buf.push(0);
        // Normalize CRLF -> LF so Windows and Unix checkouts hash identically.
        for &b in &raw {
            if b != b'\r' {
                buf.push(b);
            }
        }
        buf.push(0);
    }
    hex(&sha256(&buf))
}

fn build_wasm_deflate(repo_root: &Path, guest_dir: &Path) -> Vec<u8> {
    let remap_repo = format!("--remap-path-prefix={}=/rune", repo_root.display());
    let cargo_home = std::env::var("CARGO_HOME").unwrap_or_else(|_| {
        let home = std::env::var("USERPROFILE")
            .or_else(|_| std::env::var("HOME"))
            .unwrap_or_default();
        format!("{home}/.cargo")
    });
    let remap_cargo = format!("--remap-path-prefix={cargo_home}=/cargo");
    // Hard-cap linear memory at 16 MiB (256 pages) inside the wasm binary itself as well as
    // host-side `StoreLimits`, so browser Web Workers also enforce the 16 MiB memory cap.
    let rustflags = format!("{remap_repo} {remap_cargo} -C link-arg=--max-memory=16777216");
    let target_dir = guest_dir.join("target-repro");
    let mut cmd = Command::new("cargo");
    cmd.arg("build")
        .arg("--release")
        .arg("--target")
        .arg("wasm32-unknown-unknown")
        .arg("--manifest-path")
        .arg(guest_dir.join("Cargo.toml"))
        .arg("--target-dir")
        .arg(&target_dir)
        .env("RUSTFLAGS", rustflags);
    if guest_dir.join("Cargo.lock").exists() {
        cmd.arg("--locked");
    }
    let status = cmd.status().expect("spawn cargo build for ratex-math-wasm");
    assert!(status.success(), "cargo build for ratex-math-wasm failed");
    let wasm_path = target_dir
        .join("wasm32-unknown-unknown")
        .join("release")
        .join("ratex_math_wasm.wasm");
    let wasm = std::fs::read(&wasm_path).expect("read compiled ratex_math_wasm.wasm");
    let deflated = miniz_oxide::deflate::compress_to_vec(&wasm, 9);
    eprintln!(
        "ratex_math_wasm.wasm: raw={} B, deflated={} B",
        wasm.len(),
        deflated.len()
    );
    deflated
}

fn build_fonts_pack_deflate() -> Vec<u8> {
    let mut pack = Vec::with_capacity(520 * 1024);
    pack.extend_from_slice(b"KF01");
    pack.extend_from_slice(&(FONT_NAMES.len() as u32).to_le_bytes());
    let mut raw_total = 0usize;
    for name in FONT_NAMES {
        let file = format!("KaTeX_{name}.ttf");
        let data = ratex_katex_fonts::ttf_bytes(&file)
            .unwrap_or_else(|| panic!("missing KaTeX font {file}"));
        let data = data.as_ref();
        raw_total += data.len();
        pack.push(name.len() as u8);
        pack.extend_from_slice(name.as_bytes());
        pack.extend_from_slice(&(data.len() as u32).to_le_bytes());
        pack.extend_from_slice(data);
    }
    let deflated = miniz_oxide::deflate::compress_to_vec(&pack, 9);
    eprintln!(
        "katex-fonts.pack: {} fonts, raw={} B, pack={} B, deflated={} B",
        FONT_NAMES.len(),
        raw_total,
        pack.len(),
        deflated.len()
    );
    deflated
}

fn main() {
    let check_only = std::env::args().any(|a| a == "--check");
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let guest_dir = manifest_dir.parent().expect("tools/ratex-math-wasm");
    let repo_root = guest_dir
        .parent()
        .and_then(Path::parent)
        .expect("repo root");

    let wasm_deflate = build_wasm_deflate(repo_root, guest_dir);
    let fonts_deflate = build_fonts_pack_deflate();
    let guest_src_sha = hash_guest_sources(guest_dir);
    let wasm_sha = hex(&sha256(&wasm_deflate));
    let fonts_sha = hex(&sha256(&fonts_deflate));

    let assets_dir = repo_root.join("crates/rune-kit/assets/math");
    let wasm_out = assets_dir.join("ratex-math.wasm.deflate");
    let fonts_out = assets_dir.join("katex-fonts.pack.deflate");
    let hash_rs_out = repo_root.join("crates/rune-kit/src/math/wasm_hash.rs");
    let hash_rs = format!(
        "// Generated by `scripts/build-math-wasm.sh` from `tools/ratex-math-wasm/` and `ratex-katex-fonts 0.1.16`.\n\
         // Do not edit by hand; `crates/rune-kit/build.rs` verifies these digests when `feature = \"math\"` is enabled.\n\
         pub(crate) const GUEST_SOURCE_SHA256: &str = \"{guest_src_sha}\";\n\
         pub(crate) const WASM_DEFLATE_SHA256: &str = \"{wasm_sha}\";\n\
         pub(crate) const KATEX_FONTS_SHA256: &str = \"{fonts_sha}\";\n"
    );

    if check_only {
        let cur_wasm = std::fs::read(&wasm_out).expect("read committed ratex-math.wasm.deflate");
        let cur_fonts = std::fs::read(&fonts_out).expect("read committed katex-fonts.pack.deflate");
        let cur_hash = std::fs::read_to_string(&hash_rs_out).expect("read committed wasm_hash.rs");
        assert_eq!(cur_wasm, wasm_deflate, "ratex-math.wasm.deflate differs from fresh build");
        assert_eq!(cur_fonts, fonts_deflate, "katex-fonts.pack.deflate differs from fresh build");
        assert_eq!(cur_hash.replace("\r\n", "\n"), hash_rs, "wasm_hash.rs differs from fresh build");
        println!("OK: ratex-math.wasm.deflate, katex-fonts.pack.deflate, and wasm_hash.rs match byte-for-byte.");
        return;
    }

    std::fs::create_dir_all(&assets_dir).expect("create assets/math");
    std::fs::create_dir_all(hash_rs_out.parent().unwrap()).expect("create src/math");
    std::fs::write(&wasm_out, &wasm_deflate).expect("write ratex-math.wasm.deflate");
    std::fs::write(&fonts_out, &fonts_deflate).expect("write katex-fonts.pack.deflate");
    std::fs::write(&hash_rs_out, &hash_rs).expect("write wasm_hash.rs");
    println!("Wrote assets and wasm_hash.rs (guest={guest_src_sha}, wasm={wasm_sha}, fonts={fonts_sha})");
}
