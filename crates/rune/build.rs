//! Joins the uber shader's WGSL and extracts shared shader constants once, when Rune builds.
//!
//! Classic is `clip.wgsl` and `shader.wgsl`; Windfoil (render/variant.rs) is Classic with
//! `outline.wgsl`'s glyph branch put before shader.wgsl's atlas branch, then outline.wgsl. Each
//! file's `\r\n` is read as `\n`, so a checkout with CRLF line endings (Git for Windows'
//! `core.autocrlf=true`) builds the same text, and the same hashes, as an LF one. The text and its
//! BLAKE3 go to `$OUT_DIR/uber_wgsl.rs` (render/uber_wgsl.rs includes it). When a hash differs from
//! the one recorded beside a precompiled shader (`src/render/uber/*_hash.rs`), the build says so
//! and names the command that rebuilds it: that shader isn't used until then.
//!
//! Canonical constants and ABI parameters from `src/shaders/*.wgsl` are parsed into
//! `$OUT_DIR/shader_consts.rs` (render/instance.rs includes it), so Rust mirrors share one source
//! of truth with WGSL without perturbing the precompiled shader bytes or hashes.

use std::fmt::Write as _;
use std::path::Path;

/// shader.wgsl's atlas glyph branch, which Windfoil puts the outline branch before.
const GLYPH_BRANCH: &str = "        if i.uv.z > 0.0 || rotated {";
const OUTLINE_BRANCH: &str = "        if i.radii.w > 0.0 {\n            cov = outline_glyph(i, screen);\n        } else if i.radii.x > 0.5 && !(i.uv.z > 0.0 || rotated) {\n            cov = lcd_mask(i, screen);\n        } else if i.uv.z > 0.0 || rotated {";
const FS_ENTRY: &str = "@fragment\nfn fs(i: Out) -> @location(0) vec4<f32> {";

fn main() {
    println!("cargo::rerun-if-changed=build.rs");
    // Paths are from the package's folder, where cargo runs this.
    let read = |path: &str| {
        println!("cargo::rerun-if-changed={path}");
        let text = std::fs::read_to_string(path).unwrap_or_else(|e| panic!("{path}: {e}"));
        text.replace("\r\n", "\n")
    };
    let clip_wgsl = read("src/shaders/clip.wgsl");
    let shader_wgsl = read("src/shaders/shader.wgsl");
    let outline_wgsl = read("src/shaders/outline.wgsl");
    let layer_blur_wgsl = read("src/shaders/layer_blur.wgsl");
    let backdrop_blur_wgsl = read("src/shaders/backdrop_blur.wgsl");
    let backdrop_composite_wgsl = read("src/shaders/backdrop_composite.wgsl");

    let consts = parse_shader_consts(&WgslSources {
        clip: &clip_wgsl,
        shader: &shader_wgsl,
        outline: &outline_wgsl,
        layer_blur: &layer_blur_wgsl,
        backdrop_blur: &backdrop_blur_wgsl,
        backdrop_composite: &backdrop_composite_wgsl,
    });

    let classic = format!("{clip_wgsl}\n{shader_wgsl}");
    assert!(classic.matches(GLYPH_BRANCH).count() == 1,
        "src/shaders/shader.wgsl must hold Windfoil's anchor `{GLYPH_BRANCH}` exactly once (build.rs puts outline.wgsl's glyph branch before it)");
    let windfoil = format!("{}\n{outline_wgsl}", classic.replacen(GLYPH_BRANCH, OUTLINE_BRANCH, 1));
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
    let pinned_off = consts.pinned_kind_off_decl.as_str();
    let pinned_kinds = consts.pinned_kinds;
    let pinned_wgsl: Vec<String> = pinned_kinds.iter().map(|kind| {
        assert!(windfoil.matches(pinned_off).count() == 1, "src/shaders/shader.wgsl must hold `{pinned_off}` exactly once");
        blake3::hash(windfoil.replacen(pinned_off, &format!("const PINNED_KIND: f32 = {kind}.0;"), 1).as_bytes()).to_hex().to_string()
    }).collect();
    let file = |path: &str| {
        println!("cargo::rerun-if-changed={path}");
        blake3::hash(&std::fs::read(path).unwrap_or_else(|e| panic!("{path}: {e}"))).to_hex().to_string()
    };
    let pinned_fs: Vec<String> = pinned_kinds.iter().map(|kind| file(&format!("shaders/dxbc/uber.fs-k{kind}.dxbc"))).collect();
    let pinned_vs = file("shaders/dxbc/uber.vs.dxbc");
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
         /// BLAKE3 of the shipped `uber.vs.dxbc`.\n\
         #[cfg_attr(not(target_os = \"windows\"), allow(dead_code))]\n\
         pub(crate) const PINNED_VS_BLAKE3: &str = \"{pinned_vs}\";\n",
        pinned_wgsl[0], pinned_wgsl[1], pinned_wgsl[2], pinned_fs[0], pinned_fs[1], pinned_fs[2]));
    write("shader_consts.rs", &consts.generated);
}

struct WgslSources<'a> {
    clip: &'a str,
    shader: &'a str,
    outline: &'a str,
    layer_blur: &'a str,
    backdrop_blur: &'a str,
    backdrop_composite: &'a str,
}

struct ParsedShaderConsts {
    pinned_kind_off_decl: String,
    pinned_kinds: [u8; 3],
    generated: String,
}

struct WgslDecl {
    raw_value: String,
    line: String,
}

/// Strips `//` line comments while preserving newlines.
fn strip_comments(src: &str) -> String {
    src.lines()
        .map(|line| line.split_once("//").map_or(line, |(code, _)| code))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Collapses runs of ASCII whitespace into single spaces.
fn normalize_ws(src: &str) -> String {
    src.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Parses a top-level `<keyword> <name>: <expected_ty> = <value>;` declaration, failing the build
/// if missing, duplicated, or typed differently.
fn parse_decl(path: &str, code: &str, keyword: &str, name: &str, expected_ty: &str) -> WgslDecl {
    let prefix = format!("{keyword} ");
    let mut found = Vec::new();
    for raw_line in code.lines() {
        let line = raw_line.trim();
        let Some(rest) = line.strip_prefix(&prefix) else { continue };
        let Some((lhs, rhs)) = rest.split_once(':') else { continue };
        if lhs.trim() != name { continue; }
        let Some((ty, value_part)) = rhs.split_once('=') else {
            panic!("{path}: malformed `{keyword} {name}` declaration: `{line}`");
        };
        assert_eq!(ty.trim(), expected_ty, "{path}: `{keyword} {name}` must have type `{expected_ty}`, found `{}`", ty.trim());
        let raw_value = value_part.trim().strip_suffix(';')
            .unwrap_or_else(|| panic!("{path}: `{keyword} {name}` missing trailing `;`: `{line}`"))
            .trim()
            .to_owned();
        found.push(WgslDecl { raw_value, line: line.to_owned() });
    }
    assert_eq!(found.len(), 1, "{path}: expected exactly one `{keyword} {name}: {expected_ty} = ...;`, found {}", found.len());
    found.pop().unwrap()
}

/// Formats a WGSL float literal as a Rust `f32` literal while validating it parses cleanly.
fn parse_f32(path: &str, what: &str, raw: &str) -> (f32, String) {
    let trimmed = raw.trim().strip_suffix('f').unwrap_or(raw.trim());
    let val: f32 = trimmed.parse().unwrap_or_else(|e| panic!("{path}: invalid f32 for {what} (`{raw}`): {e}"));
    assert!(val.is_finite(), "{path}: non-finite f32 for {what} (`{raw}`)");
    // Emit `core::f32::consts::PI` when the parsed WGSL literal has the exact same `f32` bits
    // (for example `3.14159265_f32.to_bits() == core::f32::consts::PI.to_bits()`), avoiding
    // `clippy::approx_constant` without perturbing shader hashes or bit parity.
    let lit = if val.to_bits() == core::f32::consts::PI.to_bits() {
        "core::f32::consts::PI".to_owned()
    } else if trimmed.contains('.') || trimmed.contains('e') || trimmed.contains('E') {
        format!("{trimmed}_f32")
    } else {
        format!("{trimmed}.0_f32")
    };
    (val, lit)
}

fn parse_u32(path: &str, what: &str, raw: &str) -> u32 {
    let trimmed = raw.trim().strip_suffix('u').unwrap_or(raw.trim());
    if let Some(hex) = trimmed.strip_prefix("0x").or_else(|| trimmed.strip_prefix("0X")) {
        u32::from_str_radix(hex, 16).unwrap_or_else(|e| panic!("{path}: invalid hex u32 for {what} (`{raw}`): {e}"))
    } else {
        trimmed.parse::<u32>().unwrap_or_else(|e| panic!("{path}: invalid u32 for {what} (`{raw}`): {e}"))
    }
}

fn parse_bool(path: &str, what: &str, raw: &str) -> bool {
    match raw.trim() {
        "true" => true,
        "false" => false,
        other => panic!("{path}: invalid bool for {what} (`{other}`)"),
    }
}

/// Extracts the whitespace-normalized body of a unique WGSL `fn <name>` or `struct <name>` block.
fn wgsl_block(path: &str, code: &str, kind: &str, name: &str) -> String {
    let needle = format!("{kind} {name}");
    let bytes = code.as_bytes();
    let mut matches = Vec::new();
    let mut from = 0;
    while let Some(rel) = code[from..].find(&needle) {
        let start = from + rel;
        let after = start + needle.len();
        from = after;
        let prev_ok = start == 0 || !is_ident_byte(bytes[start - 1]);
        let next = code[after..].trim_start().as_bytes().first().copied();
        let next_ok = match kind {
            "fn" => next == Some(b'('),
            "struct" => next == Some(b'{'),
            _ => false,
        };
        if !prev_ok || !next_ok { continue; }
        let open_rel = code[after..].find('{').unwrap_or_else(|| panic!("{path}: `{kind} {name}` has no opening brace"));
        let body_start = after + open_rel + 1;
        let mut depth = 1usize;
        let mut body_end = None;
        for (off, ch) in code[body_start..].char_indices() {
            match ch {
                '{' => depth += 1,
                '}' => {
                    depth -= 1;
                    if depth == 0 {
                        body_end = Some(body_start + off);
                        break;
                    }
                }
                _ => {}
            }
        }
        let end = body_end.unwrap_or_else(|| panic!("{path}: `{kind} {name}` has unbalanced braces"));
        matches.push(normalize_ws(&code[body_start..end]));
    }
    assert_eq!(matches.len(), 1, "{path}: expected exactly one `{kind} {name}`, found {}", matches.len());
    matches.pop().unwrap()
}

fn is_ident_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

/// Matches a template with `{}` holes against whitespace-normalized WGSL code.
/// Holes never span `;`, `{`, or `}` so captures stay within a single expression.
fn capture_all<'a>(haystack: &'a str, pattern: &str) -> Vec<Vec<&'a str>> {
    let parts: Vec<&str> = pattern.split("{}").collect();
    assert!(parts.len() >= 2, "template must contain at least one `{{}}`: {pattern}");
    let mut out = Vec::new();
    let mut pos = 0;
    while let Some(rel) = haystack[pos..].find(parts[0]) {
        let mut cursor = pos + rel + parts[0].len();
        let mut caps = Vec::with_capacity(parts.len() - 1);
        let mut ok = true;
        for part in &parts[1..] {
            let Some(next_rel) = haystack[cursor..].find(part) else { ok = false; break };
            let cap = haystack[cursor..cursor + next_rel].trim();
            if cap.is_empty() || cap.contains(';') || cap.contains('{') || cap.contains('}') {
                ok = false;
                break;
            }
            caps.push(cap);
            cursor += next_rel + part.len();
        }
        if ok {
            out.push(caps);
            pos = cursor;
        } else {
            pos = pos + rel + 1;
        }
    }
    out
}

fn capture_once<'a>(path: &str, label: &str, haystack: &'a str, pattern: &str) -> Vec<&'a str> {
    let mut all = capture_all(haystack, pattern);
    assert_eq!(all.len(), 1, "{path}: expected exactly one `{label}` matching `{pattern}`, found {}", all.len());
    all.pop().unwrap()
}

fn capture_unique<'a>(path: &str, label: &str, haystack: &'a str, pattern: &str) -> Vec<&'a str> {
    let all = capture_all(haystack, pattern);
    assert!(!all.is_empty(), "{path}: expected at least one `{label}` matching `{pattern}`, found none");
    let first = &all[0];
    assert!(all.iter().all(|c| c == first), "{path}: conflicting `{label}` values across shader: {all:?}");
    first.clone()
}

fn kind_between(path: &str, label: &str, lo: f32, hi: f32) -> u8 {
    assert!((hi - lo - 1.0).abs() < 1e-6, "{path}: {label} bounds ({lo}, {hi}) must bracket one integer kind");
    let mid = lo + 0.5;
    assert!(mid >= 0.0 && mid <= u8::MAX as f32 && mid.fract() == 0.0, "{path}: {label} midpoint {mid} is not a valid kind");
    mid as u8
}

fn unique_kind_in(path: &str, label: &str, lo_excl: f32, hi_excl: f32) -> u8 {
    let first = lo_excl.floor() as i32 + 1;
    let last = hi_excl.ceil() as i32 - 1;
    assert!(first == last && first >= 0 && first <= u8::MAX as i32,
        "{path}: {label} bounds ({lo_excl}, {hi_excl}) must enclose a single integer kind");
    first as u8
}

fn parse_vertex_locations(path: &str, struct_body: &str) -> Vec<(String, u32)> {
    capture_all(struct_body, "@location({}) {}:")
        .into_iter()
        .map(|c| (c[1].to_owned(), parse_u32(path, &format!("@location({})", c[1]), c[0])))
        .collect()
}

fn require_location(path: &str, locs: &[(String, u32)], field: &str) -> u32 {
    let found: Vec<u32> = locs.iter().filter(|(name, _)| name == field).map(|(_, loc)| *loc).collect();
    assert_eq!(found.len(), 1, "{path}: struct In must declare `@location(...) {field}:` once");
    found[0]
}

fn parse_shader_consts(src: &WgslSources<'_>) -> ParsedShaderConsts {
    // 1. clip.wgsl
    let clip_path = "src/shaders/clip.wgsl";
    let clip_code = strip_comments(src.clip);
    let continuous_k = parse_decl(clip_path, &clip_code, "const", "CONTINUOUS_K", "f32");
    let (_, continuous_k_lit) = parse_f32(clip_path, "CONTINUOUS_K", &continuous_k.raw_value);
    let sd_cont = wgsl_block(clip_path, &clip_code, "fn", "sd_continuous");
    let (_, continuous_pi_lit) = parse_f32(
        clip_path,
        "CONTINUOUS_PI",
        capture_once(clip_path, "sd_continuous pi", &sd_cont, "atan2(v.y, v.x) * (4.0 / {}) - 1.0;")[0],
    );
    let poly_caps = capture_once(clip_path, "sd_continuous poly", &sd_cont, "e * ({} + u2 * ({} + u2 * ({} - {} * u2)))");
    let (_, cp0) = parse_f32(clip_path, "CONTINUOUS_POLY[0]", poly_caps[0]);
    let (_, cp1) = parse_f32(clip_path, "CONTINUOUS_POLY[1]", poly_caps[1]);
    let (_, cp2) = parse_f32(clip_path, "CONTINUOUS_POLY[2]", poly_caps[2]);
    let (cp3_pos, _) = parse_f32(clip_path, "CONTINUOUS_POLY[3]", poly_caps[3]);
    assert!(cp3_pos > 0.0, "{clip_path}: expected positive subtracted term in sd_continuous poly");
    let (_, cp3) = parse_f32(clip_path, "CONTINUOUS_POLY[3]", &format!("-{}", poly_caps[3]));

    let unpack = wgsl_block(clip_path, &clip_code, "fn", "unpack_clip_radii");
    let tag_caps = capture_once(clip_path, "clip radii mixed tag", &unpack, "((w & {}) != {})");
    let clip_radii_mixed_tag = parse_u32(clip_path, "CLIP_RADII_MIXED_TAG mask", tag_caps[0]);
    let tag_cmp = parse_u32(clip_path, "CLIP_RADII_MIXED_TAG value", tag_caps[1]);
    assert_eq!(clip_radii_mixed_tag, tag_cmp, "{clip_path}: unpack_clip_radii tag mask and compare differ");
    let step_caps = capture_once(clip_path, "clip radii step", &unpack, "let step = select({}, {}, (w & {}) != 0u);");
    let (_, clip_fine_lit) = parse_f32(clip_path, "CLIP_RADII_FINE_STEP", step_caps[0]);
    let (_, clip_coarse_lit) = parse_f32(clip_path, "CLIP_RADII_COARSE_STEP", step_caps[1]);
    let clip_radii_coarse_bit = parse_u32(clip_path, "CLIP_RADII_COARSE_BIT", step_caps[2]);
    let clip_radii_continuous_bit = parse_u32(
        clip_path,
        "CLIP_RADII_CONTINUOUS_BIT",
        capture_once(clip_path, "clip radii sign", &unpack, "let sgn = select(1.0, -1.0, (w & {}) != 0u);")[0],
    );
    let lane_caps = capture_once(
        clip_path,
        "clip radii lanes",
        &unpack,
        "f32(w & {}), f32((w >> {}) & {}), f32((w >> {}) & {}), f32((w >> {}) & {}),",
    );
    let lane_masks = [
        parse_u32(clip_path, "lane 0 mask", lane_caps[0]),
        parse_u32(clip_path, "lane 1 mask", lane_caps[2]),
        parse_u32(clip_path, "lane 2 mask", lane_caps[4]),
        parse_u32(clip_path, "lane 3 mask", lane_caps[6]),
    ];
    let lane_shifts = [
        parse_u32(clip_path, "lane 1 shift", lane_caps[1]),
        parse_u32(clip_path, "lane 2 shift", lane_caps[3]),
        parse_u32(clip_path, "lane 3 shift", lane_caps[5]),
    ];
    let clip_radii_lane_mask = lane_masks[0];
    let clip_radii_lane_bits = lane_shifts[0];
    assert!(lane_masks.iter().all(|&m| m == clip_radii_lane_mask), "{clip_path}: inconsistent clip radii lane masks");
    assert_eq!(lane_shifts, [clip_radii_lane_bits, clip_radii_lane_bits * 2, clip_radii_lane_bits * 3],
        "{clip_path}: inconsistent clip radii lane shifts");
    assert_eq!((1u32 << clip_radii_lane_bits) - 1, clip_radii_lane_mask, "{clip_path}: clip radii lane mask/width mismatch");
    let payload_bits = (1u32 << (4 * clip_radii_lane_bits)) - 1;
    assert_eq!(clip_radii_mixed_tag & clip_radii_coarse_bit, 0);
    assert_eq!(clip_radii_mixed_tag & clip_radii_continuous_bit, 0);
    assert_eq!(clip_radii_coarse_bit & clip_radii_continuous_bit, 0);
    assert_eq!((clip_radii_mixed_tag | clip_radii_coarse_bit | clip_radii_continuous_bit) & payload_bits, 0);

    // 2. shader.wgsl
    let shader_path = "src/shaders/shader.wgsl";
    let shader_code = strip_comments(src.shader);
    let shadow_inside = parse_decl(shader_path, &shader_code, "const", "SHADOW_INSIDE", "f32");
    let (_, shadow_inside_lit) = parse_f32(shader_path, "SHADOW_INSIDE", &shadow_inside.raw_value);
    let pinned_kind = parse_decl(shader_path, &shader_code, "const", "PINNED_KIND", "f32");
    let (_, pinned_kind_lit) = parse_f32(shader_path, "PINNED_KIND", &pinned_kind.raw_value);
    assert_eq!(src.shader.matches(FS_ENTRY).count(), 1, "{shader_path}: missing unique fragment entry `{FS_ENTRY}`");

    let struct_in = wgsl_block(shader_path, &shader_code, "struct", "In");
    let in_locs = parse_vertex_locations(shader_path, &struct_in);
    let attr_rect = require_location(shader_path, &in_locs, "rect");
    let attr_radii = require_location(shader_path, &in_locs, "radii");
    let attr_fill = require_location(shader_path, &in_locs, "fill");
    let attr_border = require_location(shader_path, &in_locs, "border");
    let attr_params = require_location(shader_path, &in_locs, "params");
    let attr_uv = require_location(shader_path, &in_locs, "uv");
    let attr_clip = require_location(shader_path, &in_locs, "clip");
    let attr_fade = require_location(shader_path, &in_locs, "fade");
    let uber_attr_count = in_locs.len();

    let base_vs = wgsl_block(shader_path, &shader_code, "fn", "base_vs");
    let rect_grow = capture_once(shader_path, "base_vs default grow", &base_vs, "var grow = {} + i.params.z * {};");
    let (_, grow_rect_pad_lit) = parse_f32(shader_path, "GROW_RECT_PAD", rect_grow[0]);
    let (_, grow_rect_soft_lit) = parse_f32(shader_path, "GROW_RECT_SOFT", rect_grow[1]);
    let shadow_grow = capture_once(shader_path, "base_vs shadow grow", &base_vs, "if (kind > {} && kind < {}) { grow = i.params.z * {} + {}; }");
    let kind_shadow = kind_between(
        shader_path,
        "KIND_SHADOW",
        parse_f32(shader_path, "shadow lo", shadow_grow[0]).0,
        parse_f32(shader_path, "shadow hi", shadow_grow[1]).0,
    );
    let (_, grow_shadow_sigma_lit) = parse_f32(shader_path, "GROW_SHADOW_SIGMA", shadow_grow[2]);
    let (_, grow_shadow_pad_lit) = parse_f32(shader_path, "GROW_SHADOW_PAD", shadow_grow[3]);
    let vs_grow_branches = capture_all(&base_vs, "else if (kind > {} && kind < {}) { grow = {}; }");
    assert_eq!(vs_grow_branches.len(), 3, "{shader_path}: expected 3 grow else-if branches in base_vs");
    let kind_glyph = kind_between(
        shader_path,
        "KIND_GLYPH",
        parse_f32(shader_path, "glyph lo", vs_grow_branches[0][0]).0,
        parse_f32(shader_path, "glyph hi", vs_grow_branches[0][1]).0,
    );
    let (_, grow_glyph_lit) = parse_f32(shader_path, "GROW_GLYPH", vs_grow_branches[0][2]);
    let kind_inset = kind_between(
        shader_path,
        "KIND_INSET",
        parse_f32(shader_path, "inset lo", vs_grow_branches[1][0]).0,
        parse_f32(shader_path, "inset hi", vs_grow_branches[1][1]).0,
    );
    let (grow_flat, grow_flat_lit) = parse_f32(shader_path, "GROW_FLAT", vs_grow_branches[1][2]);
    let flat_span_lo = parse_f32(shader_path, "flat span lo", vs_grow_branches[2][0]).0;
    let flat_span_hi = parse_f32(shader_path, "flat span hi", vs_grow_branches[2][1]).0;
    assert_eq!(parse_f32(shader_path, "flat span grow", vs_grow_branches[2][2]).0, grow_flat);
    let (uber_max_kind, uber_max_kind_lit) = parse_f32(
        shader_path,
        "UBER_MAX_KIND_THRESHOLD",
        capture_once(shader_path, "base_vs max kind", &base_vs, "vec2<f32>(0.0), kind > {});")[0],
    );
    let rot_glyph = capture_once(shader_path, "base_vs rotated glyph", &base_vs, "if (kind > {} && kind < {} && i.params.x != 0.0)");
    assert_eq!(
        kind_between(shader_path, "rotated glyph", parse_f32(shader_path, "rot glyph lo", rot_glyph[0]).0, parse_f32(shader_path, "rot glyph hi", rot_glyph[1]).0),
        kind_glyph,
    );
    let turned_layer = capture_once(shader_path, "base_vs turned layer", &base_vs, "if (kind > {} && kind < {} && i.border.y != 0.0)");
    let kind_layer = kind_between(
        shader_path,
        "KIND_LAYER",
        parse_f32(shader_path, "layer lo", turned_layer[0]).0,
        parse_f32(shader_path, "layer hi", turned_layer[1]).0,
    );

    let fs = wgsl_block(shader_path, &shader_code, "fn", "fs");
    let fs_layer_k = parse_f32(shader_path, "fs layer kind", capture_once(shader_path, "fs layer branch", &fs, "if (fragment_kind(i) == {})")[0]).0;
    assert_eq!(fs_layer_k, f32::from(kind_layer), "{shader_path}: fs layer kind differs from base_vs");
    let (_, layer_additive_lit) = parse_f32(
        shader_path,
        "LAYER_ADDITIVE_PARAM",
        capture_once(shader_path, "fs additive flag", &fs, "let additive = i.params.x == {};")[0],
    );
    let fs_max_kind = parse_f32(shader_path, "fs max kind", capture_once(shader_path, "fs discard threshold", &fs, "if (kind > {}) { discard; }")[0]).0;
    assert_eq!(fs_max_kind, uber_max_kind, "{shader_path}: base_vs and fs max kind thresholds differ");
    let rect_thr = parse_f32(shader_path, "fs rect threshold", capture_once(shader_path, "fs rect branch", &fs, "if (kind < {}) { let d = sd_rounded")[0]).0;
    let kind_rect_f = parse_f32(shader_path, "fs flat_fill kind", capture_once(shader_path, "fs flat_fill kind", &fs, "let flat_fill = kind == {} &&")[0]).0;
    assert!(kind_rect_f >= 0.0 && kind_rect_f < rect_thr && kind_rect_f.fract() == 0.0, "{shader_path}: inconsistent KIND_RECT");
    let kind_rect = kind_rect_f as u8;
    let kind_glyph_fs = parse_f32(shader_path, "fs gray_mask kind", capture_once(shader_path, "fs gray_mask kind", &fs, "let gray_mask = kind == {} &&")[0]).0;
    assert_eq!(kind_glyph_fs, f32::from(kind_glyph), "{shader_path}: fs glyph kind differs from base_vs");
    let shadow_fs = capture_once(shader_path, "fs shadow branch", &fs, "else if (kind < {}) { let sigma = max(i.params.z, {});");
    assert_eq!(parse_f32(shader_path, "fs shadow threshold", shadow_fs[0]).0, f32::from(kind_shadow) + 0.5);
    let (_, shadow_min_sigma_lit) = parse_f32(shader_path, "SHADOW_MIN_SIGMA", shadow_fs[1]);
    let hi_branch = capture_once(shader_path, "fs high kinds", &fs, "else if (kind > {}) { let d = sd_rounded(screen - center, half, i.radii); if (kind > {}) {");
    let image_lo = parse_f32(shader_path, "fs image lo", hi_branch[0]).0;
    let radial_lo = parse_f32(shader_path, "fs radial lo", hi_branch[1]).0;
    let kind_image = unique_kind_in(shader_path, "KIND_IMAGE", image_lo, f32::from(kind_layer) - 0.5);
    let kind_radial = unique_kind_in(shader_path, "KIND_RADIAL", radial_lo, uber_max_kind);
    let dashed_caps = capture_once(
        shader_path,
        "fs dashed branch",
        &fs,
        "else if (kind > {}) { let soft = max(i.params.z, {}); let bw = i.params.x; let ring =",
    );
    let dashed_lo = parse_f32(shader_path, "fs dashed lo", dashed_caps[0]).0;
    let kind_dashed = unique_kind_in(shader_path, "KIND_DASHED", dashed_lo, radial_lo);
    let kind_layer_end = unique_kind_in(shader_path, "KIND_LAYER_END", f32::from(kind_layer) + 0.5, dashed_lo);
    assert_eq!(flat_span_hi, f32::from(kind_layer_end) + 0.5, "{shader_path}: base_vs flat span upper bound does not match KIND_LAYER_END");
    let seg_caps = capture_once(shader_path, "fs segment branch", &fs, "else if (kind > {} && kind < {}) { col = i.fill * segment(");
    let kind_segment = kind_between(
        shader_path,
        "KIND_SEGMENT",
        parse_f32(shader_path, "segment lo", seg_caps[0]).0,
        parse_f32(shader_path, "segment hi", seg_caps[1]).0,
    );
    assert_eq!(flat_span_lo, f32::from(kind_segment) - 0.5, "{shader_path}: base_vs flat span lower bound does not match KIND_SEGMENT");
    let tri_caps = capture_once(shader_path, "fs triangle branch", &fs, "else if (kind > {} && kind < {}) { col = i.fill * triangle(");
    let kind_triangle = kind_between(
        shader_path,
        "KIND_TRIANGLE",
        parse_f32(shader_path, "triangle lo", tri_caps[0]).0,
        parse_f32(shader_path, "triangle hi", tri_caps[1]).0,
    );
    let inset_grad = capture_once(shader_path, "fs inset/gradient branch", &fs, "else if (kind > {}) { if (kind < {}) { let spread =");
    let inset_lo = parse_f32(shader_path, "fs inset lo", inset_grad[0]).0;
    let inset_hi = parse_f32(shader_path, "fs inset hi", inset_grad[1]).0;
    assert_eq!(kind_between(shader_path, "fs inset", inset_lo, inset_hi), kind_inset);
    let kind_gradient = unique_kind_in(shader_path, "KIND_GRADIENT", inset_hi, f32::from(kind_segment) - 0.5);

    let (_, soft_min_lit) = parse_f32(shader_path, "SOFT_MIN", capture_unique(shader_path, "soft min", &fs, "let soft = max(i.params.z, {});")[0]);
    let radial_open = capture_once(
        shader_path,
        "radial open stop",
        &fs,
        "let lower_open = t0_raw >= {}; let upper_open = t1_raw >= {}; let t0 = select(t0_raw, t0_raw - {}, lower_open); let t1 = select(t1_raw, t1_raw - {}, upper_open);",
    );
    assert_eq!(radial_open[0], radial_open[1], "{shader_path}: radial lower/upper open thresholds differ");
    assert_eq!(radial_open[2], radial_open[3], "{shader_path}: radial lower/upper open offsets differ");
    let (_, radial_open_thr_lit) = parse_f32(shader_path, "RADIAL_OPEN_THRESHOLD", radial_open[0]);
    let (_, radial_open_off_lit) = parse_f32(shader_path, "RADIAL_OPEN_OFFSET", radial_open[2]);
    let (_, radial_stop_eps_lit) = parse_f32(shader_path, "RADIAL_STOP_EPSILON", capture_once(shader_path, "radial stop epsilon", &fs, "t1 <= t0 + {})")[0]);
    let (_, inset_sharp_lit) = parse_f32(shader_path, "INSET_SHARP_SIGMA", capture_once(shader_path, "inset sharp sigma", &fs, "if (i.params.z < {}) { lit = box_cov")[0]);
    let (flag_on, flag_on_lit) = parse_f32(shader_path, "FLAG_ON_THRESHOLD", capture_once(shader_path, "segment round-cap flag", &fs, "segment(screen, i.radii.xy, i.radii.zw, i.params.x, i.params.z > {})")[0]);
    let img_texel = wgsl_block(shader_path, &shader_code, "fn", "image_texel");
    assert_eq!(parse_f32(shader_path, "g.flags.x", capture_once(shader_path, "g.flags.x", &img_texel, "if (g.flags.x > {})")[0]).0, flag_on);
    assert_eq!(parse_f32(shader_path, "g.flags.y", capture_once(shader_path, "g.flags.y", &fs, "if (g.flags.y > {} &&")[0]).0, flag_on);
    let image_flag_premul = parse_u32(
        shader_path,
        "IMAGE_FLAG_PREMULTIPLIED",
        capture_once(shader_path, "image premultiplied bit", &fs, "let premultiplied = (flags & {}) != 0u;")[0],
    );
    let image_flag_tint = parse_u32(
        shader_path,
        "IMAGE_FLAG_TINT",
        capture_once(shader_path, "image tint bit", &fs, "let tinted = (flags & {}) != 0u;")[0],
    );
    let image_flag_nearest = parse_u32(
        shader_path,
        "IMAGE_FLAG_NEAREST",
        capture_once(shader_path, "image nearest bit", &fs, "let nearest = (flags & {}) != 0u;")[0],
    );
    let wrap_fn = wgsl_block(shader_path, &shader_code, "fn", "wrap_texel");
    let image_wrap_extend = parse_u32(
        shader_path,
        "IMAGE_WRAP_EXTEND",
        capture_once(shader_path, "image wrap extend", &fs, "if (wrap == {}) {")[0],
    );
    let image_wrap_repeat = parse_u32(
        shader_path,
        "IMAGE_WRAP_REPEAT",
        capture_once(shader_path, "image wrap repeat", &wrap_fn, "if (mode == {}) { var m = k % n;")[0],
    );
    let image_wrap_mirror = parse_u32(
        shader_path,
        "IMAGE_WRAP_MIRROR",
        capture_once(shader_path, "image wrap mirror", &wrap_fn, "if (mode == {}) { let p = 2 * n;")[0],
    );
    assert_eq!(
        parse_u32(
            shader_path,
            "fs mirror period",
            capture_once(shader_path, "fs mirror period", &fs, "select(1.0, 2.0, wrap == {})")[0],
        ),
        image_wrap_mirror,
    );

    let erf2 = wgsl_block(shader_path, &shader_code, "fn", "erf2");
    let erf_caps = capture_once(shader_path, "erf2 poly", &erf2, "var r = 1.0 + ({} + ({} + {} * (a * a)) * a) * a;");
    let (_, erf0) = parse_f32(shader_path, "ERF_POLY[0]", erf_caps[0]);
    let (_, erf1) = parse_f32(shader_path, "ERF_POLY[1]", erf_caps[1]);
    let (_, erf2_c) = parse_f32(shader_path, "ERF_POLY[2]", erf_caps[2]);

    let shadow_fn = wgsl_block(shader_path, &shader_code, "fn", "shadow");
    let s_in = capture_once(shader_path, "shadow inside sigma", &shadow_fn, "abs(p.x) <= half.x - {} * sigma && abs(p.y) <= half.y - corner - {} * sigma");
    assert_eq!(s_in[0], s_in[1]);
    let (_, shadow_inside_sigma_lit) = parse_f32(shader_path, "SHADOW_INSIDE_SIGMA", s_in[0]);
    let s_win = capture_once(shader_path, "shadow window sigma", &shadow_fn, "let start = clamp(-{} * sigma, lo, hi); let end = clamp({} * sigma, lo, hi);");
    assert_eq!(s_win[0], s_win[1]);
    let (_, shadow_window_sigma_lit) = parse_f32(shader_path, "SHADOW_WINDOW_SIGMA", s_win[0]);
    let shadow_div = parse_f32(shader_path, "shadow step div", capture_once(shader_path, "shadow step", &shadow_fn, "let step = (end - start) / {};")[0]).0;
    let shadow_samples = parse_u32(shader_path, "SHADOW_SAMPLES", capture_once(shader_path, "shadow loop", &shadow_fn, "for (var i = 0; i < {}; i = i + 1)")[0]) as usize;
    assert_eq!(shadow_div, shadow_samples as f32, "{shader_path}: shadow step divisor and loop count differ");

    let seg_fn = wgsl_block(shader_path, &shader_code, "fn", "segment");
    let (_, seg_min_len_lit) = parse_f32(shader_path, "SEGMENT_MIN_LEN", capture_once(shader_path, "segment min len", &seg_fn, "if (len < {})")[0]);
    let edge_fn = wgsl_block(shader_path, &shader_code, "fn", "edge");
    let soft_fn = wgsl_block(shader_path, &shader_code, "fn", "soft");
    let (edge_eps, edge_eps_lit) = parse_f32(shader_path, "EDGE_EPSILON", capture_once(shader_path, "edge eps", &edge_fn, "max(length(e), {})")[0]);
    assert_eq!(parse_f32(shader_path, "soft eps", capture_once(shader_path, "soft eps", &soft_fn, "max(dot(e, e), {})")[0]).0, edge_eps);
    let tri_fn = wgsl_block(shader_path, &shader_code, "fn", "triangle");
    let tri_bits = capture_all(&tri_fn, "if ((aa & {}) == {})");
    assert_eq!(tri_bits.len(), 3, "{shader_path}: expected 3 triangle AA bit tests");
    let tri_aa: Vec<u32> = tri_bits.iter().map(|c| {
        let mask = parse_u32(shader_path, "triangle AA mask", c[0]);
        assert_eq!(mask, parse_u32(shader_path, "triangle AA val", c[1]));
        mask
    }).collect();
    let ring_fn = wgsl_block(shader_path, &shader_code, "fn", "ring_arc");
    let (_, quarter_turn_lit) = parse_f32(shader_path, "QUARTER_TURN", capture_once(shader_path, "ring_arc quarter turn", &ring_fn, "let arc = rc * {};")[0]);
    let dash_fn = wgsl_block(shader_path, &shader_code, "fn", "dash_cov");
    let (_, dash_min_total_lit) = parse_f32(shader_path, "DASH_MIN_TOTAL", capture_once(shader_path, "dash_cov min total", &dash_fn, "max(total, {})")[0]);
    let border_fn = wgsl_block(shader_path, &shader_code, "fn", "border_over");
    let (_, opacity_eps_lit) = parse_f32(shader_path, "OPACITY_EPSILON", capture_once(shader_path, "border_over opacity eps", &border_fn, "clamp(opacity, {}, 1.0)")[0]);
    let (_, opaque_hide_lit) = parse_f32(shader_path, "OPAQUE_HIDE_THRESHOLD", capture_once(shader_path, "border_over hide threshold", &border_fn, "shrink && hide > {})")[0]);

    // 3. outline.wgsl
    let outline_path = "src/shaders/outline.wgsl";
    let outline_code = strip_comments(src.outline);
    let outline_hint = parse_decl(outline_path, &outline_code, "const", "OUTLINE_HINT_REACH", "f32");
    let (_, outline_hint_lit) = parse_f32(outline_path, "OUTLINE_HINT_REACH", &outline_hint.raw_value);
    let outline_subpixel_decl = parse_decl(outline_path, &outline_code, "const", "OUTLINE_SUBPIXEL", "bool");
    let outline_subpixel = parse_bool(outline_path, "OUTLINE_SUBPIXEL", &outline_subpixel_decl.raw_value);
    let outline_texel_fn = wgsl_block(outline_path, &outline_code, "fn", "outline_texel");
    let texel_caps = capture_once(outline_path, "outline_texel coords", &outline_texel_fn, "vec2<i32>(i32(i & {}), i32(i >> {}))");
    let tex_mask = parse_u32(outline_path, "outline texel mask", texel_caps[0]);
    let tex_shift = parse_u32(outline_path, "outline texel shift", texel_caps[1]);
    let outline_tex_width = 1u32 << tex_shift;
    assert_eq!(outline_tex_width, tex_mask + 1, "{outline_path}: outline_texel mask/shift mismatch");
    let outline_glyph_fn = wgsl_block(outline_path, &outline_code, "fn", "outline_glyph");
    let across_caps = capture_once(outline_path, "outline_glyph subpixel mask", &outline_glyph_fn, "let across = select(1.0, {}, mode == {});");
    let (outline_stride, outline_stride_lit) = parse_f32(outline_path, "OUTLINE_SUBPIXEL_STRIDE", across_caps[0]);
    let outline_mode_subpixel_mask = parse_u32(outline_path, "OUTLINE_MODE_SUBPIXEL_MASK", across_caps[1]);
    let outline_mode_slug = parse_u32(
        outline_path,
        "OUTLINE_MODE_SLUG",
        capture_once(outline_path, "outline_glyph slug", &outline_glyph_fn, "if (mode == {}) { return outline_slug(")[0],
    );
    let outline_mode_rgb = parse_u32(
        outline_path,
        "OUTLINE_MODE_RGB",
        capture_once(outline_path, "outline_glyph rgb", &outline_glyph_fn, "if (mode == {} && OUTLINE_SUBPIXEL) { return outline_rgb(")[0],
    );
    let lcd_mask_fn = wgsl_block(outline_path, &outline_code, "fn", "lcd_mask");
    let lcd_stride = parse_u32(outline_path, "lcd_mask stride", capture_once(outline_path, "lcd_mask stride", &lcd_mask_fn, "i32(p.x) * {},")[0]);
    assert_eq!(lcd_stride as f32, outline_stride, "{outline_path}: lcd_mask and outline_glyph subpixel strides differ");

    // 4. layer_blur.wgsl
    let lb_path = "src/shaders/layer_blur.wgsl";
    let lb_code = strip_comments(src.layer_blur);
    let lb_mode_decl = parse_decl(lb_path, &lb_code, "override", "MODE", "u32");
    let lb_mode_blur = parse_u32(lb_path, "LAYER_BLUR_MODE_BLUR", &lb_mode_decl.raw_value);
    let lb_add_decl = parse_decl(lb_path, &lb_code, "override", "ADDITIVE", "bool");
    assert!(!parse_bool(lb_path, "ADDITIVE", &lb_add_decl.raw_value), "{lb_path}: override ADDITIVE must default to false");
    let lb_uniforms = wgsl_block(lb_path, &lb_code, "struct", "Uniforms");
    let lb_taps = parse_u32(lb_path, "LAYER_BLUR_TAPS", capture_once(lb_path, "Uniforms.taps", &lb_uniforms, "taps: array<vec4<f32>, {}>,")[0]) as usize;
    let lb_fs = wgsl_block(lb_path, &lb_code, "fn", "fs");
    let lb_mode_prefilter = parse_u32(lb_path, "LAYER_BLUR_MODE_PREFILTER", capture_once(lb_path, "layer_blur prefilter", &lb_fs, "if (MODE == {}) { let lo =")[0]);
    let lb_mode_reconstruct = parse_u32(
        lb_path,
        "LAYER_BLUR_MODE_RECONSTRUCT",
        capture_once(lb_path, "layer_blur reconstruct", &lb_fs, "if (MODE == {}) { return sample_zero(p.xy * u.mapping.xy); }")[0],
    );

    // 5. backdrop_blur.wgsl
    let bb_path = "src/shaders/backdrop_blur.wgsl";
    let bb_code = strip_comments(src.backdrop_blur);
    let bb_down = wgsl_block(bb_path, &bb_code, "fn", "fs_down");
    let down_caps = capture_once(
        bb_path,
        "fs_down weights",
        &bb_down,
        "return (d + e + i + j) * {} + (a + b + g + f) * {} + (b + c + h + g) * {} + (f + g + l + k) * {} + (g + h + m + l) * {};",
    );
    let (_, bb_down_inner_lit) = parse_f32(bb_path, "BACKDROP_DOWN_INNER_WEIGHT", down_caps[0]);
    assert!(down_caps[1..].iter().all(|&w| w == down_caps[1]), "{bb_path}: fs_down outer quadrant weights differ");
    let (_, bb_down_outer_lit) = parse_f32(bb_path, "BACKDROP_DOWN_OUTER_WEIGHT", down_caps[1]);
    let bb_up = wgsl_block(bb_path, &bb_code, "fn", "fs_up");
    let (_, bb_up_center_lit) = parse_f32(bb_path, "BACKDROP_UP_CENTER_WEIGHT", capture_once(bb_path, "fs_up center", &bb_up, "var c = s(uv) * {};")[0]);
    let (_, bb_up_axial_lit) = parse_f32(bb_path, "BACKDROP_UP_AXIAL_WEIGHT", capture_once(bb_path, "fs_up axial", &bb_up, "s(uv - vec2<f32>(0.0, t.y))) * {};")[0]);
    let (_, bb_up_div_lit) = parse_f32(bb_path, "BACKDROP_UP_DIVISOR", capture_once(bb_path, "fs_up divisor", &bb_up, "c /= {};")[0]);

    // 6. backdrop_composite.wgsl
    let bc_path = "src/shaders/backdrop_composite.wgsl";
    let bc_code = strip_comments(src.backdrop_composite);
    let bc_in = wgsl_block(bc_path, &bc_code, "struct", "In");
    let bc_locs = parse_vertex_locations(bc_path, &bc_in);
    assert_eq!(require_location(bc_path, &bc_locs, "rect"), attr_rect);
    assert_eq!(require_location(bc_path, &bc_locs, "radii"), attr_radii);
    assert_eq!(require_location(bc_path, &bc_locs, "fill"), attr_fill);
    assert_eq!(require_location(bc_path, &bc_locs, "clip"), attr_clip);
    assert_eq!(require_location(bc_path, &bc_locs, "fade"), attr_fade);
    let bc_vs = wgsl_block(bc_path, &bc_code, "fn", "vs");
    let bc_grow_caps = capture_once(bc_path, "backdrop_composite grow", &bc_vs, "i.rect.xy - vec2<f32>({}) + corner * (i.rect.zw + vec2<f32>({}));");
    let (bc_grow, bc_grow_lit) = parse_f32(bc_path, "BACKDROP_COMPOSITE_GROW", bc_grow_caps[0]);
    let (bc_expand, _) = parse_f32(bc_path, "BACKDROP_COMPOSITE_GROW * 2", bc_grow_caps[1]);
    assert_eq!(bc_expand, bc_grow * 2.0, "{bc_path}: vertex grow and size expansion mismatch");

    let mut out = String::with_capacity(4096);
    writeln!(out, "// Generated by build.rs from crates/rune/src/shaders/*.wgsl. Do not edit by hand.").unwrap();
    writeln!(out, "pub const CONTINUOUS_K: f32 = {continuous_k_lit};").unwrap();
    writeln!(out, "pub const CONTINUOUS_PI: f32 = {continuous_pi_lit};").unwrap();
    writeln!(out, "pub const CONTINUOUS_POLY: [f32; 4] = [{cp0}, {cp1}, {cp2}, {cp3}];").unwrap();
    writeln!(out, "pub const CLIP_RADII_MIXED_TAG: u32 = {clip_radii_mixed_tag:#010x};").unwrap();
    writeln!(out, "pub const CLIP_RADII_COARSE_BIT: u32 = {clip_radii_coarse_bit:#010x};").unwrap();
    writeln!(out, "pub const CLIP_RADII_CONTINUOUS_BIT: u32 = {clip_radii_continuous_bit:#010x};").unwrap();
    writeln!(out, "pub const CLIP_RADII_FINE_STEP: f32 = {clip_fine_lit};").unwrap();
    writeln!(out, "pub const CLIP_RADII_COARSE_STEP: f32 = {clip_coarse_lit};").unwrap();
    writeln!(out, "pub const CLIP_RADII_LANE_MASK: u32 = {clip_radii_lane_mask:#x};").unwrap();
    writeln!(out, "pub const CLIP_RADII_LANE_BITS: u32 = {clip_radii_lane_bits};").unwrap();

    writeln!(out, "pub const SHADOW_INSIDE: f32 = {shadow_inside_lit};").unwrap();
    writeln!(out, "pub const PINNED_KIND: f32 = {pinned_kind_lit};").unwrap();
    writeln!(out, "pub const PINNED_KIND_OFF_DECL: &str = {:?};", pinned_kind.line).unwrap();
    writeln!(out, "pub const FS_ENTRY_DECL: &str = {:?};", FS_ENTRY).unwrap();

    writeln!(out, "pub const ATTR_RECT_LOC: u32 = {attr_rect};").unwrap();
    writeln!(out, "pub const ATTR_RADII_LOC: u32 = {attr_radii};").unwrap();
    writeln!(out, "pub const ATTR_FILL_LOC: u32 = {attr_fill};").unwrap();
    writeln!(out, "pub const ATTR_BORDER_LOC: u32 = {attr_border};").unwrap();
    writeln!(out, "pub const ATTR_PARAMS_LOC: u32 = {attr_params};").unwrap();
    writeln!(out, "pub const ATTR_UV_LOC: u32 = {attr_uv};").unwrap();
    writeln!(out, "pub const ATTR_CLIP_LOC: u32 = {attr_clip};").unwrap();
    writeln!(out, "pub const ATTR_FADE_LOC: u32 = {attr_fade};").unwrap();
    writeln!(out, "pub const UBER_ATTR_COUNT: usize = {uber_attr_count};").unwrap();

    writeln!(out, "pub const KIND_RECT_U8: u8 = {kind_rect};").unwrap();
    writeln!(out, "pub const KIND_SHADOW_U8: u8 = {kind_shadow};").unwrap();
    writeln!(out, "pub const KIND_GLYPH_U8: u8 = {kind_glyph};").unwrap();
    writeln!(out, "pub const KIND_INSET_U8: u8 = {kind_inset};").unwrap();
    writeln!(out, "pub const KIND_GRADIENT_U8: u8 = {kind_gradient};").unwrap();
    writeln!(out, "pub const KIND_SEGMENT_U8: u8 = {kind_segment};").unwrap();
    writeln!(out, "pub const KIND_TRIANGLE_U8: u8 = {kind_triangle};").unwrap();
    writeln!(out, "pub const KIND_IMAGE_U8: u8 = {kind_image};").unwrap();
    writeln!(out, "pub const KIND_LAYER_U8: u8 = {kind_layer};").unwrap();
    writeln!(out, "pub const KIND_LAYER_END_U8: u8 = {kind_layer_end};").unwrap();
    writeln!(out, "pub const KIND_DASHED_U8: u8 = {kind_dashed};").unwrap();
    writeln!(out, "pub const KIND_RADIAL_U8: u8 = {kind_radial};").unwrap();
    writeln!(out, "pub const UBER_MAX_KIND_THRESHOLD: f32 = {uber_max_kind_lit};").unwrap();

    writeln!(out, "pub const GROW_RECT_PAD: f32 = {grow_rect_pad_lit};").unwrap();
    writeln!(out, "pub const GROW_RECT_SOFT: f32 = {grow_rect_soft_lit};").unwrap();
    writeln!(out, "pub const GROW_SHADOW_SIGMA: f32 = {grow_shadow_sigma_lit};").unwrap();
    writeln!(out, "pub const GROW_SHADOW_PAD: f32 = {grow_shadow_pad_lit};").unwrap();
    writeln!(out, "pub const GROW_GLYPH: f32 = {grow_glyph_lit};").unwrap();
    writeln!(out, "pub const GROW_FLAT: f32 = {grow_flat_lit};").unwrap();

    writeln!(out, "pub const ERF_POLY: [f32; 3] = [{erf0}, {erf1}, {erf2_c}];").unwrap();
    writeln!(out, "pub const SHADOW_INSIDE_SIGMA: f32 = {shadow_inside_sigma_lit};").unwrap();
    writeln!(out, "pub const SHADOW_WINDOW_SIGMA: f32 = {shadow_window_sigma_lit};").unwrap();
    writeln!(out, "pub const SHADOW_SAMPLES: usize = {shadow_samples};").unwrap();
    writeln!(out, "pub const SHADOW_MIN_SIGMA: f32 = {shadow_min_sigma_lit};").unwrap();

    writeln!(out, "pub const SEGMENT_MIN_LEN: f32 = {seg_min_len_lit};").unwrap();
    writeln!(out, "pub const EDGE_EPSILON: f32 = {edge_eps_lit};").unwrap();
    writeln!(out, "pub const TRIANGLE_AA_AB: u32 = {};", tri_aa[0]).unwrap();
    writeln!(out, "pub const TRIANGLE_AA_BC: u32 = {};", tri_aa[1]).unwrap();
    writeln!(out, "pub const TRIANGLE_AA_CA: u32 = {};", tri_aa[2]).unwrap();
    writeln!(out, "pub const QUARTER_TURN: f32 = {quarter_turn_lit};").unwrap();
    writeln!(out, "pub const DASH_MIN_TOTAL: f32 = {dash_min_total_lit};").unwrap();
    writeln!(out, "pub const SOFT_MIN: f32 = {soft_min_lit};").unwrap();
    writeln!(out, "pub const OPACITY_EPSILON: f32 = {opacity_eps_lit};").unwrap();
    writeln!(out, "pub const OPAQUE_HIDE_THRESHOLD: f32 = {opaque_hide_lit};").unwrap();
    writeln!(out, "pub const LAYER_ADDITIVE_PARAM: f32 = {layer_additive_lit};").unwrap();
    writeln!(out, "pub const INSET_SHARP_SIGMA: f32 = {inset_sharp_lit};").unwrap();
    writeln!(out, "pub const RADIAL_OPEN_THRESHOLD: f32 = {radial_open_thr_lit};").unwrap();
    writeln!(out, "pub const RADIAL_OPEN_OFFSET: f32 = {radial_open_off_lit};").unwrap();
    writeln!(out, "pub const RADIAL_STOP_EPSILON: f32 = {radial_stop_eps_lit};").unwrap();
    writeln!(out, "pub const FLAG_ON_THRESHOLD: f32 = {flag_on_lit};").unwrap();
    writeln!(out, "pub const IMAGE_FLAG_PREMULTIPLIED: u32 = {image_flag_premul};").unwrap();
    writeln!(out, "pub const IMAGE_FLAG_TINT: u32 = {image_flag_tint};").unwrap();
    writeln!(out, "pub const IMAGE_FLAG_NEAREST: u32 = {image_flag_nearest};").unwrap();
    writeln!(out, "pub const IMAGE_WRAP_EXTEND: u32 = {image_wrap_extend};").unwrap();
    writeln!(out, "pub const IMAGE_WRAP_REPEAT: u32 = {image_wrap_repeat};").unwrap();
    writeln!(out, "pub const IMAGE_WRAP_MIRROR: u32 = {image_wrap_mirror};").unwrap();

    writeln!(out, "pub const OUTLINE_HINT_REACH: f32 = {outline_hint_lit};").unwrap();
    writeln!(out, "pub const OUTLINE_SUBPIXEL: bool = {outline_subpixel};").unwrap();
    writeln!(out, "pub const OUTLINE_SUBPIXEL_OFF_DECL: &str = {:?};", outline_subpixel_decl.line).unwrap();
    writeln!(out, "pub const OUTLINE_TEX_WIDTH: u32 = {outline_tex_width};").unwrap();
    writeln!(out, "pub const OUTLINE_MODE_SLUG: u32 = {outline_mode_slug};").unwrap();
    writeln!(out, "pub const OUTLINE_MODE_RGB: u32 = {outline_mode_rgb};").unwrap();
    writeln!(out, "pub const OUTLINE_MODE_SUBPIXEL_MASK: u32 = {outline_mode_subpixel_mask};").unwrap();
    writeln!(out, "pub const OUTLINE_SUBPIXEL_STRIDE: f32 = {outline_stride_lit};").unwrap();

    writeln!(out, "pub const LAYER_BLUR_TAPS: usize = {lb_taps};").unwrap();
    writeln!(out, "pub const LAYER_BLUR_OVERRIDE_MODE: &str = \"MODE\";").unwrap();
    writeln!(out, "pub const LAYER_BLUR_OVERRIDE_ADDITIVE: &str = \"ADDITIVE\";").unwrap();
    writeln!(out, "pub const LAYER_BLUR_MODE_BLUR: u32 = {lb_mode_blur};").unwrap();
    writeln!(out, "pub const LAYER_BLUR_MODE_PREFILTER: u32 = {lb_mode_prefilter};").unwrap();
    writeln!(out, "pub const LAYER_BLUR_MODE_RECONSTRUCT: u32 = {lb_mode_reconstruct};").unwrap();

    writeln!(out, "pub const BACKDROP_DOWN_INNER_WEIGHT: f32 = {bb_down_inner_lit};").unwrap();
    writeln!(out, "pub const BACKDROP_DOWN_OUTER_WEIGHT: f32 = {bb_down_outer_lit};").unwrap();
    writeln!(out, "pub const BACKDROP_UP_CENTER_WEIGHT: f32 = {bb_up_center_lit};").unwrap();
    writeln!(out, "pub const BACKDROP_UP_AXIAL_WEIGHT: f32 = {bb_up_axial_lit};").unwrap();
    writeln!(out, "pub const BACKDROP_UP_DIVISOR: f32 = {bb_up_div_lit};").unwrap();
    writeln!(out, "pub const BACKDROP_COMPOSITE_GROW: f32 = {bc_grow_lit};").unwrap();

    ParsedShaderConsts {
        pinned_kind_off_decl: pinned_kind.line,
        pinned_kinds: [kind_rect, kind_shadow, kind_glyph],
        generated: out,
    }
}
