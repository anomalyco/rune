//! Guarded live hot-patching loop powered by Dioxus CLI (`dx` 0.7.10) and the embedded AST guard.

use std::path::Path;
use std::process::Command;
use std::thread;
use std::time::Duration;

use crate::CliError;
use crate::config::ResolvedDevConfig;
use crate::dev::mirror::{
    atomic_write, default_target_dir_for, discover_layout, join_posix, mirror_dir_for, sync_mirror,
};
use crate::dev::proc::ChildGuard;
use crate::glob;
use crate::guard::{self, GuardOutcome};
use crate::watch::{FileCache, Snapshot, diff_snapshots, scan_sources};

const POLL_INTERVAL: Duration = Duration::from_millis(120);

const RUNE_HOT_PROFILE_CONFIG: &[&str] = &[
    r#"profile.rune-hot.inherits="dev""#,
    "profile.rune-hot.debug=0",
    "profile.rune-hot.incremental=true",
    "profile.rune-hot.package.png.opt-level=3",
    "profile.rune-hot.package.fdeflate.opt-level=3",
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChangeDecision {
    Patch { files: Vec<String> },
    WaitForSyntax { file: String, error: String },
    Restart { reason: String },
}

/// Evaluates a snapshot diff against `patch_globs` and the embedded AST guard.
pub fn classify_change(
    crate_key: &str,
    patch_globs: &[String],
    before: &Snapshot,
    after: &Snapshot,
) -> Option<ChangeDecision> {
    let diff = diff_snapshots(before, after);
    if diff.is_empty() {
        return None;
    }

    if !diff.added.is_empty() || !diff.removed.is_empty() {
        let mut names = diff.added;
        names.extend(diff.removed);
        names.sort();
        return Some(ChangeDecision::Restart {
            reason: format!("files added/removed: {}", names.join(", ")),
        });
    }

    let prefix = format!("{}/", crate_key.trim_end_matches('/'));
    let mut non_patchable = Vec::new();
    for key in &diff.modified {
        let is_patchable = key.strip_prefix(&prefix).is_some_and(|rel| {
            rel.ends_with(".rs") && glob::matches_any(rel, patch_globs)
        });
        if !is_patchable {
            non_patchable.push(key.clone());
        }
    }
    if !non_patchable.is_empty() {
        return Some(ChangeDecision::Restart {
            reason: format!("outside --patch globs: {}", non_patchable.join(", ")),
        });
    }

    for key in &diff.modified {
        let old_bytes = before.get(key).map(Vec::as_slice).unwrap_or(&[]);
        let new_bytes = after.get(key).map(Vec::as_slice).unwrap_or(&[]);
        let (Ok(old_src), Ok(new_src)) =
            (std::str::from_utf8(old_bytes), std::str::from_utf8(new_bytes))
        else {
            return Some(ChangeDecision::Restart {
                reason: format!("non-UTF-8 edit in {key}"),
            });
        };
        match guard::check_edit(old_src, new_src) {
            GuardOutcome::Compatible => {}
            GuardOutcome::RestartNeeded => {
                return Some(ChangeDecision::Restart {
                    reason: format!(
                        "{key} (declaration, state schema, signature, startup code, or persistent macro changed)"
                    ),
                });
            }
            GuardOutcome::SyntaxError(err) => {
                return Some(ChangeDecision::WaitForSyntax {
                    file: key.clone(),
                    error: err,
                });
            }
        }
    }

    Some(ChangeDecision::Patch {
        files: diff.modified,
    })
}

/// Constructs the exact `dx serve` command-line arguments for a guarded Rune hot-patch session.
pub fn build_dx_args(cfg: &ResolvedDevConfig) -> Vec<String> {
    let mut features = vec!["hot".to_string()];
    for f in &cfg.features {
        if f != "hot" && !features.contains(f) {
            features.push(f.clone());
        }
    }

    let mut args = vec![
        "serve".to_string(),
        "--hot-patch".to_string(),
        "--platform".to_string(),
        "server".to_string(),
        "--open".to_string(),
        "false".to_string(),
        "--interactive".to_string(),
        "false".to_string(),
        "--addr".to_string(),
        "127.0.0.1".to_string(),
        "--port".to_string(),
        cfg.port.to_string(),
        "--package".to_string(),
        cfg.package.clone(),
        "--features".to_string(),
        features.join(","),
        "--profile".to_string(),
        "rune-hot".to_string(),
    ];

    if let Some(bin) = &cfg.bin {
        args.push("--bin".to_string());
        args.push(bin.clone());
    }

    if !cfg.meta.has_hot_profile {
        let mut cargo_args = Vec::new();
        for item in RUNE_HOT_PROFILE_CONFIG {
            cargo_args.push("--config".to_string());
            cargo_args.push((*item).to_string());
        }
        args.push(format!("--cargo-args={}", cargo_args.join(" ")));
    }

    if !cfg.app_args.is_empty() {
        args.push(format!("--args={}", cfg.app_args.join(" ")));
    }

    args
}

/// Runs the guarded hot-patching loop, automatically rebuilding and relaunching on restart-needed
/// edits.
pub fn run_hot(cfg: &ResolvedDevConfig, dx_path: &Path) -> Result<(), CliError> {
    let mirror_dir = mirror_dir_for(&cfg.meta.manifest_path);
    let target_dir = cfg
        .target_dir
        .clone()
        .unwrap_or_else(|| default_target_dir_for(&cfg.meta.manifest_path));

    println!(
        "rune dev: guarded hot reload for `{}` (port {}, patch: {})",
        cfg.package,
        cfg.port,
        cfg.patch.join(", ")
    );

    let mut cache = FileCache::default();

    loop {
        let layout = discover_layout(&cfg.meta.manifest_path)?;
        let sources = layout.source_entries();
        let mut baseline = scan_sources(&mut cache, &sources).map_err(|err| {
            CliError::new(format!("failed to scan project files: {err}"))
        })?;
        sync_mirror(&mirror_dir, &layout.crate_key, &baseline)?;

        let dx_cwd = join_posix(&mirror_dir, &layout.crate_key);
        let dx_args = build_dx_args(cfg);

        let mut cmd = Command::new(dx_path);
        cmd.args(&dx_args)
            .current_dir(&dx_cwd)
            .env("RUSTC_WRAPPER", "")
            .env("CARGO_INCREMENTAL", "1")
            .env("CARGO_BUILD_JOBS", cfg.jobs.to_string())
            .env("CARGO_TARGET_DIR", &target_dir)
            .env("RUNE_DEV_GUARDED", "1")
            .env("RUNE_HOT", "1");

        let mut child = ChildGuard::spawn(&mut cmd).map_err(|err| {
            CliError::new(format!(
                "failed to start {}: {err}",
                dx_path.display()
            ))
        })?;

        let mut last_syntax_notice: Option<String> = None;

        loop {
            if let Some(status) = child
                .try_wait()
                .map_err(|err| CliError::new(format!("failed waiting on dx: {err}")))?
            {
                if status.success() {
                    return Ok(());
                }
                return Err(CliError::with_code(
                    format!("dx exited with status {}", status.code().unwrap_or(1)),
                    status.code().unwrap_or(1),
                ));
            }

            thread::sleep(POLL_INTERVAL);

            let current_sources = layout.source_entries();
            let Ok(current) = scan_sources(&mut cache, &current_sources) else {
                // An editor is mid-save; retry on the next tick.
                continue;
            };

            let Some(decision) =
                classify_change(&layout.crate_key, &cfg.patch, &baseline, &current)
            else {
                continue;
            };

            match decision {
                ChangeDecision::Patch { files } => {
                    last_syntax_notice = None;
                    for key in &files {
                        if let Some(bytes) = current.get(key) {
                            let dest = join_posix(&mirror_dir, key);
                            atomic_write(&dest, bytes)?;
                        }
                    }
                    baseline = current;
                    println!("rune dev: patched {}", files.join(", "));
                }
                ChangeDecision::WaitForSyntax { file, error } => {
                    let msg = format!("{file}: {error}");
                    if last_syntax_notice.as_deref() != Some(msg.as_str()) {
                        eprintln!("rune dev: edit not ready in {file}: {error}");
                        last_syntax_notice = Some(msg);
                    }
                }
                ChangeDecision::Restart { reason } => {
                    println!("rune dev: restart needed: {reason}");
                    println!("rune dev: rebuilding and relaunching...");
                    child.stop();
                    break;
                }
            }
        }
    }
}
