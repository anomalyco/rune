//! Guarded live hot-patching loop powered by Dioxus CLI (`dx`) and the embedded AST guard.

use std::path::Path;
use std::process::Command;
use std::thread;
use std::time::Duration;

use crate::CliError;
use crate::config::ResolvedDevConfig;
use crate::dev::fingerprint::{clear_manifest_fingerprints, is_manifest_change};
use crate::dev::mirror::{
    atomic_write, default_target_dir_for, discover_layout, join_posix, mirror_dir_for,
    sync_generated_lockfile, sync_mirror, touch_bin_entries,
};
use crate::dev::proc::ChildGuard;
use crate::glob;
use crate::guard::{self, GuardOutcome};
use crate::watch::{FileCache, Snapshot, diff_snapshots, scan_sources};

const POLL_INTERVAL: Duration = Duration::from_millis(120);
const HOT_PROFILE_NAME: &str = "rune-hot";

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

/// Quotes a single argument using POSIX shell rules compatible with `shell_words::split` (which
/// `dx` uses to parse `--cargo-args`).
pub fn shell_quote_arg(arg: &str) -> String {
    if arg.is_empty() {
        return "''".to_string();
    }
    let safe = arg.bytes().all(|b| {
        b.is_ascii_alphanumeric()
            || matches!(
                b,
                b'_' | b'@' | b'%' | b'+' | b'=' | b':' | b',' | b'.' | b'/' | b'-'
            )
    });
    if safe {
        return arg.to_string();
    }
    let mut out = String::with_capacity(arg.len() + 2);
    out.push('\'');
    for ch in arg.chars() {
        if ch == '\'' {
            out.push_str("'\\''");
        } else {
            out.push(ch);
        }
    }
    out.push('\'');
    out
}

/// Joins arguments with POSIX shell quoting so `shell_words::split` recovers each token verbatim.
pub fn shell_join(args: &[String]) -> String {
    args.iter()
        .map(|a| shell_quote_arg(a))
        .collect::<Vec<_>>()
        .join(" ")
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
        HOT_PROFILE_NAME.to_string(),
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
        args.push(format!("--cargo-args={}", shell_join(&cargo_args)));
    }

    if !cfg.app_args.is_empty() {
        args.push(format!("--args={}", cfg.app_args.join(" ")));
    }

    args
}

/// Runs the guarded hot-patching loop with a fixed config.
pub fn run_hot(cfg: &ResolvedDevConfig, dx_path: &Path) -> Result<(), CliError> {
    let initial = cfg.clone();
    run_hot_with_reloader(initial, dx_path, || Ok(cfg.clone()))
}

/// Runs the guarded hot-patching loop, reloading `ResolvedDevConfig` on each restart so edits to
/// `Cargo.toml` or `rune.toml` take effect without restarting `rune dev`.
pub fn run_hot_with_reloader<F>(
    mut cfg: ResolvedDevConfig,
    dx_path: &Path,
    mut reload_cfg: F,
) -> Result<(), CliError>
where
    F: FnMut() -> Result<ResolvedDevConfig, CliError>,
{
    println!(
        "rune dev: guarded hot reload for `{}` (port {}, patch: {})",
        cfg.package,
        cfg.port,
        cfg.patch.join(", ")
    );

    let mut cache = FileCache::default();
    let mut first_run = true;
    let mut clear_fingerprints_next = false;

    loop {
        if !first_run {
            match reload_cfg() {
                Ok(updated) => cfg = updated,
                Err(err) => {
                    eprintln!(
                        "rune dev: keeping previous config after reload error: {}",
                        err.message
                    );
                }
            }
        }
        first_run = false;

        let mirror_dir = mirror_dir_for(&cfg.meta.manifest_path);
        let target_dir = cfg
            .target_dir
            .clone()
            .unwrap_or_else(|| default_target_dir_for(&cfg.meta.manifest_path));

        if std::mem::take(&mut clear_fingerprints_next) {
            clear_manifest_fingerprints(&target_dir, HOT_PROFILE_NAME, &cfg.package);
        }

        let layout = discover_layout(&cfg.meta.manifest_path)?;
        let sources = layout.source_entries();
        let mut baseline = scan_sources(&mut cache, &sources).map_err(|err| {
            CliError::new(format!("failed to scan project files: {err}"))
        })?;
        sync_mirror(&mirror_dir, &layout.crate_key, &baseline)?;
        touch_bin_entries(
            &mirror_dir,
            &layout.crate_key,
            &baseline,
            Some(&cfg.package),
            cfg.bin.as_deref(),
        )?;

        let dx_cwd = join_posix(&mirror_dir, &layout.crate_key);
        let dx_args = build_dx_args(&cfg);

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
                if let Err(err) = sync_generated_lockfile(
                    &mirror_dir,
                    &layout,
                    &cfg.meta.workspace_root,
                    &baseline,
                ) {
                    eprintln!("rune dev: warning: {}", err.message);
                }
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
                    let changed_keys = diff_snapshots(&baseline, &current).all_changed();
                    clear_fingerprints_next = is_manifest_change(&changed_keys);
                    println!("rune dev: restart needed: {reason}");
                    println!("rune dev: rebuilding and relaunching...");
                    child.stop();
                    if let Err(err) = sync_generated_lockfile(
                        &mirror_dir,
                        &layout,
                        &cfg.meta.workspace_root,
                        &baseline,
                    ) {
                        eprintln!("rune dev: warning: {}", err.message);
                    }
                    break;
                }
            }
        }
    }
}
