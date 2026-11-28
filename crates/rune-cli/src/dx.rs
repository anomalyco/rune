//! Detection and version verification for Dioxus CLI (`dx`).

use std::env;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::REQUIRED_DX_VERSION;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DxStatus {
    Ready {
        path: PathBuf,
        version: String,
    },
    WrongVersion {
        path: PathBuf,
        version: String,
    },
    Missing,
}

impl DxStatus {
    pub fn install_hint() -> String {
        format!("cargo install dioxus-cli --version {REQUIRED_DX_VERSION} --locked")
    }
}

/// Locates `dx` (checking `RUNE_DX`, `CARGO_HOME/bin`, `~/.cargo/bin`, and `PATH`) and checks
/// that it is a supported Dioxus CLI binary.
///
/// By default, auto-detected binaries must match `REQUIRED_DX_VERSION` (`0.7.10`) so unrelated
/// tools like Deno's `dx` and untested versions are skipped. Setting `RUNE_DX=/path/to/dx`
/// accepts any Dioxus CLI build (relaxing the exact version pin), and setting
/// `RUNE_DX_UNPINNED=1` also relaxes the exact version pin on auto-detected paths (still skipping
/// non-Dioxus binaries such as Deno's `dx`).
pub fn detect() -> DxStatus {
    let unpinned = is_unpinned_env();
    let explicit = env::var_os("RUNE_DX")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from);
    if let Some(path) = explicit {
        if !path.is_file() {
            return DxStatus::Missing;
        }
        let version = read_version(&path);
        if accepts_version(&version, true, unpinned) {
            return DxStatus::Ready { path, version };
        }
        return DxStatus::WrongVersion { path, version };
    }

    let candidates = auto_candidate_paths();
    let mut fallback_wrong: Option<(PathBuf, String)> = None;

    for path in candidates {
        if !path.is_file() {
            continue;
        }
        let version = read_version(&path);
        if accepts_version(&version, false, unpinned) {
            return DxStatus::Ready { path, version };
        }
        if fallback_wrong.is_none() {
            fallback_wrong = Some((path, version));
        }
    }

    match fallback_wrong {
        Some((path, version)) => DxStatus::WrongVersion { path, version },
        None => DxStatus::Missing,
    }
}

/// Returns `true` if `version_output` identifies Dioxus CLI `REQUIRED_DX_VERSION`.
pub fn is_required_dioxus_version(version_output: &str) -> bool {
    if !is_any_dioxus_version(version_output) {
        return false;
    }
    let lower = version_output.to_ascii_lowercase();
    // Match exact token `0.7.10` (avoiding e.g. `0.7.100`).
    lower
        .split(|c: char| !c.is_ascii_alphanumeric() && c != '.')
        .any(|tok| {
            tok == REQUIRED_DX_VERSION || tok.strip_prefix('v') == Some(REQUIRED_DX_VERSION)
        })
}

/// Returns `true` if `version_output` identifies any Dioxus CLI build (excluding unrelated
/// binaries such as Deno's `dx`).
pub fn is_any_dioxus_version(version_output: &str) -> bool {
    version_output.to_ascii_lowercase().contains("dioxus")
}

/// Decides whether a `dx --version` string is accepted under the active pin policy.
pub fn accepts_version(version_output: &str, explicit_override: bool, unpinned: bool) -> bool {
    if explicit_override && unpinned {
        return !version_output.starts_with("failed to execute:");
    }
    if explicit_override || unpinned {
        return is_any_dioxus_version(version_output);
    }
    is_required_dioxus_version(version_output)
}

/// Parses an env-var flag value for `RUNE_DX_UNPINNED` (`1`, `true`, `yes`, `on`).
pub fn parse_unpinned_flag(value: &str) -> bool {
    matches!(
        value.trim().to_ascii_lowercase().as_str(),
        "1" | "true" | "yes" | "on"
    )
}

fn is_unpinned_env() -> bool {
    env::var_os("RUNE_DX_UNPINNED")
        .and_then(|v| v.into_string().ok())
        .is_some_and(|v| parse_unpinned_flag(&v))
}

fn read_version(path: &Path) -> String {
    match Command::new(path).arg("--version").output() {
        Ok(out) => {
            let stdout = String::from_utf8_lossy(&out.stdout).trim().to_string();
            let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
            if !stdout.is_empty() {
                stdout.lines().next().unwrap_or("").to_string()
            } else if !stderr.is_empty() {
                stderr.lines().next().unwrap_or("").to_string()
            } else {
                format!("exit {}", out.status.code().unwrap_or(-1))
            }
        }
        Err(err) => format!("failed to execute: {err}"),
    }
}

fn auto_candidate_paths() -> Vec<PathBuf> {
    let mut out = Vec::new();
    let bin_name = if cfg!(windows) { "dx.exe" } else { "dx" };

    if let Some(cargo_home) = env::var_os("CARGO_HOME")
        && !cargo_home.is_empty()
    {
        out.push(PathBuf::from(cargo_home).join("bin").join(bin_name));
    }

    if let Some(home) = home_dir() {
        let p = home.join(".cargo").join("bin").join(bin_name);
        if !out.contains(&p) {
            out.push(p);
        }
    }

    if let Some(path_var) = env::var_os("PATH") {
        for dir in env::split_paths(&path_var) {
            let candidate = dir.join(bin_name);
            if !out.contains(&candidate) {
                out.push(candidate);
            }
        }
    }

    out
}

pub(crate) fn home_dir() -> Option<PathBuf> {
    env::var_os("HOME")
        .or_else(|| env::var_os("USERPROFILE"))
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
}
