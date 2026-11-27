//! Captures Rune's current git revision and `[patch.crates-io]` crate list at compile time so
//! `rune new`, `rune check`, and `rune doctor` stay aligned with the workspace manifest.

use std::fs;
use std::path::PathBuf;
use std::process::Command;

const FALLBACK_PATCHES: &[&str] = &[
    "wgpu-hal",
    "winit",
    "sctk-adwaita",
    "parley_engine",
    "accesskit_windows",
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

    let rev = std::env::var_os("RUNE_CLI_REV")
        .and_then(|v| v.into_string().ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .or_else(|| git_head_rev(&manifest_dir))
        .unwrap_or_default();

    println!("cargo:rustc-env=RUNE_DEFAULT_REV={rev}");
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

fn git_head_rev(dir: &PathBuf) -> Option<String> {
    if let Ok(out) = Command::new("git")
        .args(["rev-parse", "--git-path", "HEAD"])
        .current_dir(dir)
        .output()
        && out.status.success()
    {
        let head = String::from_utf8_lossy(&out.stdout).trim().to_string();
        if !head.is_empty() {
            let head_path = PathBuf::from(&head);
            let resolved = if head_path.is_absolute() {
                head_path
            } else {
                dir.join(head_path)
            };
            if resolved.is_file() {
                println!("cargo:rerun-if-changed={}", resolved.display());
            }
        }
    }
    let out = Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(dir)
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let sha = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if sha.len() >= 7 && sha.chars().all(|c| c.is_ascii_hexdigit()) {
        Some(sha)
    } else {
        None
    }
}
