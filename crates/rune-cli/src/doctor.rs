//! Environment and project health diagnostics for `rune doctor`.

use std::env;
use std::fs;
use std::path::Path;
use std::process::Command;

use crate::args::DoctorArgs;
use crate::config::{find_manifest, load_crate_meta, load_rune_toml};
use crate::dx::{self, DxStatus};
use crate::pins::{
    check_pins, combine_workspace_and_crate_if_needed, resolve_required_patches,
};
use crate::{CliError, REQUIRED_DX_VERSION};

pub fn run(opts: &DoctorArgs) -> Result<(), CliError> {
    let mut errors = 0usize;

    match tool_version("rustc", &["--version"]) {
        Some(ver) => println!("[ok]   rustc: {ver}"),
        None => {
            println!("[err]  rustc: not found on PATH");
            errors += 1;
        }
    }
    match tool_version("cargo", &["--version"]) {
        Some(ver) => println!("[ok]   cargo: {ver}"),
        None => {
            println!("[err]  cargo: not found on PATH");
            errors += 1;
        }
    }

    if let Some(host) = rustc_host_triple() {
        println!("[ok]   host target: {host}");
        check_platform_prereqs(&host);
    }

    match dx::detect() {
        DxStatus::Ready { path, version } => {
            if cfg!(windows) {
                println!(
                    "[warn] dx: {version} ({}) — live dylib patching is experimental on Windows; `rune dev` defaults to watch-and-restart (pass `--mode hot` to opt in)",
                    path.display()
                );
            } else {
                println!("[ok]   dx: {version} ({})", path.display());
            }
        }
        DxStatus::WrongVersion { path, version } => {
            println!(
                "[warn] dx: found '{}' at {}, expected dioxus-cli {REQUIRED_DX_VERSION} (the Deno `dx` command is unrelated). Install with: {}",
                version,
                path.display(),
                DxStatus::install_hint()
            );
        }
        DxStatus::Missing => {
            println!(
                "[warn] dx: not found. `rune dev` will use watch-and-restart mode. For live fn-body patching, install: {}",
                DxStatus::install_hint()
            );
        }
    }

    let cwd = env::current_dir()
        .map_err(|err| CliError::new(format!("cannot read current directory: {err}")))?;
    match find_manifest(opts.manifest_path.as_deref(), &cwd) {
        Ok(manifest) => {
            errors += check_project(&manifest, opts.rune_manifest.as_deref());
        }
        Err(err) => {
            if opts.manifest_path.is_some() {
                return Err(err);
            }
            println!("[info] project: no Cargo.toml found in current directory");
        }
    }

    if errors > 0 {
        Err(CliError::new(format!(
            "doctor reported {errors} error(s)"
        )))
    } else {
        Ok(())
    }
}

fn check_project(manifest: &Path, rune_manifest: Option<&Path>) -> usize {
    let mut errors = 0usize;
    let meta = match load_crate_meta(manifest) {
        Ok(m) => m,
        Err(err) => {
            println!("[info] project: {} ({})", manifest.display(), err.message);
            return 0;
        }
    };

    println!(
        "[ok]   project: {} ({})",
        meta.package_name,
        meta.manifest_path.display()
    );

    if meta.has_hot_feature {
        println!("[ok]   hot feature: [features] hot is defined");
    } else {
        println!(
            "[warn] hot feature: missing in {} — add `hot = [\"rune/hot\"]` under [features] for live patching",
            meta.manifest_path.display()
        );
    }

    if meta.has_hot_profile {
        println!("[ok]   hot profile: [profile.rune-hot] is defined");
    } else {
        println!(
            "[info] hot profile: not in Cargo.toml (`rune dev` supplies one automatically when needed)"
        );
    }

    match load_rune_toml(&meta.crate_dir, &meta.workspace_root) {
        Ok(Some((path, _))) => println!("[ok]   config: {}", path.display()),
        Ok(None) => println!("[info] config: no rune.toml (using Cargo.toml metadata / defaults)"),
        Err(err) => {
            println!("[err]  config: {}", err.message);
            errors += 1;
        }
    }

    let ws_manifest = meta.workspace_root.join("Cargo.toml");
    let required = match resolve_required_patches(rune_manifest) {
        Ok(r) => r,
        Err(err) => {
            println!("[err]  rune manifest: {}", err.message);
            return errors + 1;
        }
    };
    if let (Ok(ws_text), Ok(crate_text)) = (
        fs::read_to_string(&ws_manifest),
        fs::read_to_string(&meta.manifest_path),
    ) {
        let lock_path = meta.workspace_root.join("Cargo.lock");
        let lock_text = fs::read_to_string(&lock_path).ok();
        match combine_workspace_and_crate_if_needed(
            &ws_manifest,
            &ws_text,
            &meta.manifest_path,
            &crate_text,
        )
        .and_then(|combined| {
            check_pins(&combined, &required, lock_text.as_deref(), None).map_err(CliError::new)
        }) {
            Ok(problems) if problems.is_empty() => {
                println!("[ok]   Rune pins: URL, rev, and [patch.crates-io] agree");
            }
            Ok(problems) => {
                for p in problems {
                    println!("[warn] Rune pins: {p}");
                }
            }
            Err(err) => {
                println!("[err]  Rune pins: {}", err.message);
                errors += 1;
            }
        }
    }

    errors
}

fn check_platform_prereqs(host: &str) {
    if host.contains("windows-gnu") {
        let has_dlltool = tool_version("dlltool", &["--version"]).is_some();
        let has_as = tool_version("as", &["--version"]).is_some();
        if has_dlltool && has_as {
            println!("[ok]   windows-gnu binutils: dlltool and as found on PATH");
        } else {
            println!(
                "[warn] windows-gnu binutils: missing dlltool/as on PATH (install MSYS2 mingw-w64-ucrt-x86_64-binutils or switch to x86_64-pc-windows-msvc)"
            );
        }
    }

    #[cfg(target_os = "linux")]
    {
        for lib in ["xkbcommon", "wayland-client"] {
            let ok = Command::new("pkg-config")
                .args(["--exists", lib])
                .status()
                .is_ok_and(|s| s.success());
            if ok {
                println!("[ok]   linux library: {lib} found via pkg-config");
            } else {
                println!(
                    "[warn] linux library: {lib} not found via pkg-config (install libxkbcommon-dev and libwayland-dev)"
                );
            }
        }
    }
}

fn tool_version(bin: &str, args: &[&str]) -> Option<String> {
    let out = Command::new(bin).args(args).output().ok()?;
    if !out.status.success() {
        return None;
    }
    let line = String::from_utf8_lossy(&out.stdout)
        .lines()
        .next()
        .unwrap_or("")
        .trim()
        .to_string();
    if line.is_empty() { None } else { Some(line) }
}

fn rustc_host_triple() -> Option<String> {
    let out = Command::new("rustc").arg("-vV").output().ok()?;
    if !out.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&out.stdout);
    text.lines()
        .find_map(|line| line.strip_prefix("host:").map(|v| v.trim().to_string()))
}
