//! Cross-platform watch-and-restart loop using `cargo run`.

use std::process::Command;
use std::thread;
use std::time::Duration;

use crate::CliError;
use crate::config::ResolvedDevConfig;
use crate::dev::mirror::discover_layout;
use crate::dev::proc::ChildGuard;
use crate::watch::{FileCache, diff_snapshots, scan_sources};

const POLL_INTERVAL: Duration = Duration::from_millis(150);

/// Builds the `cargo run` argument list for watch-and-restart mode.
pub fn build_cargo_run_args(cfg: &ResolvedDevConfig) -> Vec<String> {
    let mut args = vec![
        "run".to_string(),
        "--manifest-path".to_string(),
        cfg.meta.manifest_path.to_string_lossy().to_string(),
        "--package".to_string(),
        cfg.package.clone(),
    ];
    if let Some(bin) = &cfg.bin {
        args.push("--bin".to_string());
        args.push(bin.clone());
    }
    if !cfg.features.is_empty() {
        args.push("--features".to_string());
        args.push(cfg.features.join(","));
    }
    if !cfg.app_args.is_empty() {
        args.push("--".to_string());
        args.extend(cfg.app_args.iter().cloned());
    }
    args
}

/// Runs the app with `cargo run` and automatically rebuilds & relaunches on file edits.
pub fn run_restart(cfg: &ResolvedDevConfig) -> Result<(), CliError> {
    println!("rune dev: watch-and-restart for `{}`", cfg.package);

    let mut cache = FileCache::default();

    loop {
        let layout = discover_layout(&cfg.meta.manifest_path)?;
        let sources = layout.source_entries();
        let baseline = scan_sources(&mut cache, &sources).map_err(|err| {
            CliError::new(format!("failed to scan project files: {err}"))
        })?;

        let cargo_args = build_cargo_run_args(cfg);
        let mut cmd = Command::new("cargo");
        cmd.args(&cargo_args).current_dir(&cfg.meta.crate_dir);
        if let Some(target_dir) = &cfg.target_dir {
            cmd.env("CARGO_TARGET_DIR", target_dir);
        }

        let mut child = ChildGuard::spawn(&mut cmd)
            .map_err(|err| CliError::new(format!("failed to invoke cargo run: {err}")))?;

        let mut waiting_after_error = false;

        loop {
            if !waiting_after_error
                && let Some(status) = child
                    .try_wait()
                    .map_err(|err| CliError::new(format!("failed waiting on cargo run: {err}")))?
            {
                if status.success() {
                    return Ok(());
                }
                eprintln!(
                    "rune dev: process exited with status {}; waiting for file changes (Ctrl+C to quit)...",
                    status.code().unwrap_or(1)
                );
                waiting_after_error = true;
            }

            thread::sleep(POLL_INTERVAL);

            let current_sources = layout.source_entries();
            let Ok(current) = scan_sources(&mut cache, &current_sources) else {
                continue;
            };
            let diff = diff_snapshots(&baseline, &current);
            if !diff.is_empty() {
                let changed = diff.all_changed();
                let preview = if changed.len() <= 3 {
                    changed.join(", ")
                } else {
                    format!(
                        "{} (+{} more)",
                        changed[..3].join(", "),
                        changed.len() - 3
                    )
                };
                println!("rune dev: changed {preview}; rebuilding...");
                child.stop();
                break;
            }
        }
    }
}
