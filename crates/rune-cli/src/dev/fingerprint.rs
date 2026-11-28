//! Selective Cargo fingerprint invalidation after manifest or config edits.
//!
//! When `Cargo.toml`, `Cargo.lock`, or `.cargo/config*` changes during a `rune dev` session
//! (for example enabling a feature on `rune` or the app crate), clearing fingerprints for the app
//! crate and Rune crates ensures Cargo recompiles the updated feature set while keeping all
//! third-party dependencies cached.

use std::fs;
use std::path::Path;

/// Returns `true` if any changed mirror key is a Cargo manifest, lockfile, or Cargo config file.
pub fn is_manifest_change(changed_keys: &[String]) -> bool {
    changed_keys.iter().any(|key| {
        let norm = key.replace('\\', "/");
        norm == "Cargo.toml"
            || norm == "Cargo.lock"
            || norm == ".cargo/config.toml"
            || norm == ".cargo/config"
            || norm.ends_with("/Cargo.toml")
            || norm.ends_with("/Cargo.lock")
            || norm.ends_with("/.cargo/config.toml")
            || norm.ends_with("/.cargo/config")
    })
}

/// Returns `true` if `crate_name` is the app package itself or any `rune` / `rune-*` crate.
pub fn should_bust_crate(crate_name: &str, app_package: &str) -> bool {
    let norm = crate_name.replace('-', "_");
    let app_norm = app_package.replace('-', "_");
    norm == app_norm || norm == "rune" || norm.starts_with("rune_")
}

/// Removes Cargo fingerprint entries for `app_package` and Rune crates under
/// `<target_dir>/<profile>`, supporting both Cargo's legacy `.fingerprint/<pkg>-<hash>`
/// layout and the newer `build/<pkg>/<hash>/fingerprint` layout while leaving third-party
/// dependencies and build-script execution units intact.
///
/// Returns the number of removed fingerprint entries.
pub fn clear_manifest_fingerprints(
    target_dir: &Path,
    profile: &str,
    app_package: &str,
) -> usize {
    let profile_dir = target_dir.join(profile);
    let mut removed = 0usize;
    removed += clear_legacy_fingerprints(&profile_dir.join(".fingerprint"), app_package);
    removed += clear_build_dir_fingerprints(&profile_dir.join("build"), app_package);
    removed
}

fn clear_legacy_fingerprints(fp_root: &Path, app_package: &str) -> usize {
    let Ok(entries) = fs::read_dir(fp_root) else {
        return 0;
    };
    let mut removed = 0usize;
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let name = entry.file_name();
        let name_str = name.to_string_lossy();
        let Some(pkg) = strip_hex_hash_suffix(&name_str) else {
            continue;
        };
        if !should_bust_crate(pkg, app_package) {
            continue;
        }
        if is_legacy_build_script_unit(&path) {
            continue;
        }
        if fs::remove_dir_all(&path).is_ok() {
            removed += 1;
        }
    }
    removed
}

fn clear_build_dir_fingerprints(build_root: &Path, app_package: &str) -> usize {
    let Ok(pkg_entries) = fs::read_dir(build_root) else {
        return 0;
    };
    let mut removed = 0usize;
    for pkg_entry in pkg_entries.flatten() {
        let pkg_path = pkg_entry.path();
        if !pkg_path.is_dir() {
            continue;
        }
        let pkg_name = pkg_entry.file_name();
        let pkg_str = pkg_name.to_string_lossy();
        if !should_bust_crate(&pkg_str, app_package) {
            continue;
        }
        let Ok(hash_entries) = fs::read_dir(&pkg_path) else {
            continue;
        };
        for hash_entry in hash_entries.flatten() {
            let unit_dir = hash_entry.path();
            if !unit_dir.is_dir() {
                continue;
            }
            // Cargo places a sibling `run` directory on build-script execution units; leave those
            // cached so build scripts do not re-run unnecessarily.
            if unit_dir.join("run").is_dir() {
                continue;
            }
            let fp_path = unit_dir.join("fingerprint");
            if fp_path.is_dir() {
                if fs::remove_dir_all(&fp_path).is_ok() {
                    removed += 1;
                }
            } else if fp_path.is_file() && fs::remove_file(&fp_path).is_ok() {
                removed += 1;
            }
        }
    }
    removed
}

fn strip_hex_hash_suffix(dir_name: &str) -> Option<&str> {
    let (prefix, suffix) = dir_name.rsplit_once('-')?;
    if !prefix.is_empty()
        && !suffix.is_empty()
        && suffix.bytes().all(|b| b.is_ascii_hexdigit())
    {
        Some(prefix)
    } else {
        None
    }
}

fn is_legacy_build_script_unit(fp_dir: &Path) -> bool {
    let Ok(entries) = fs::read_dir(fp_dir) else {
        return false;
    };
    let mut saw_any = false;
    let mut saw_non_build_script = false;
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name_str = name.to_string_lossy();
        saw_any = true;
        if !name_str.starts_with("run-build-script")
            && !name_str.starts_with("build-script-")
            && !name_str.starts_with("dep-build-script-")
            && name_str != "invoked.timestamp"
        {
            saw_non_build_script = true;
            break;
        }
    }
    saw_any && !saw_non_build_script
}
