//! Captures Rune's upstream git revision and `[patch.crates-io]` crate list at compile time so
//! `rune new`, `rune check`, and `rune doctor` stay aligned with the workspace manifest without
//! ever pinning unpushed local or fork-only commits.

#[allow(dead_code)]
#[path = "src/git_rev.rs"]
mod git_rev;

use std::fs;
use std::path::PathBuf;

const FALLBACK_PATCHES: &[&str] = &[
    "wgpu-hal",
    "naga",
    "winit",
    "sctk-adwaita",
    "parley_engine",
    "accesskit_windows",
    "accesskit_atspi_common",
    "accesskit_macos",
];

fn main() {
    println!("cargo:rerun-if-env-changed=RUNE_CLI_REV");

    let manifest_dir = PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").unwrap_or_default());
    let root_manifest = manifest_dir.join("../../Cargo.toml");

    let patches = if root_manifest.is_file() {
        println!("cargo:rerun-if-changed={}", root_manifest.display());
        fs::read_to_string(&root_manifest)
            .ok()
            .map(|text| parse_patch_keys(&text))
            .filter(|keys| !keys.is_empty())
            .unwrap_or_else(|| FALLBACK_PATCHES.iter().map(|s| (*s).to_string()).collect())
    } else {
        FALLBACK_PATCHES.iter().map(|s| (*s).to_string()).collect()
    };

    println!("cargo:rustc-env=RUNE_DEFAULT_PATCHES={}", patches.join(","));

    let env_rev = std::env::var_os("RUNE_CLI_REV")
        .and_then(|v| v.into_string().ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());

    let (build_rev, default_rev) = if let Some(rev) = env_rev {
        (rev.clone(), rev)
    } else {
        let head = git_rev::git_head_rev(&manifest_dir).unwrap_or_default();
        let upstream = git_rev::upstream_dev_rev(&manifest_dir);
        for path in &upstream.watch_paths {
            println!("cargo:rerun-if-changed={}", path.display());
        }
        (head, upstream.rev.unwrap_or_default())
    };

    println!("cargo:rustc-env=RUNE_BUILD_REV={build_rev}");
    println!("cargo:rustc-env=RUNE_DEFAULT_REV={default_rev}");
}

fn parse_patch_keys(toml_text: &str) -> Vec<String> {
    let mut keys = Vec::new();
    let mut in_patch = false;
    for raw in toml_text.lines() {
        let line = raw.split('#').next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        if line.starts_with('[') {
            in_patch = line == "[patch.crates-io]";
            continue;
        }
        if in_patch && let Some((key, _)) = line.split_once('=') {
            let key = key.trim().trim_matches('"').trim_matches('\'');
            if !key.is_empty() {
                keys.push(key.to_string());
            }
        }
    }
    keys
}
