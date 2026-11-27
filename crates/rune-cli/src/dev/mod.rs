//! Cross-platform `rune dev` orchestration: guarded hot patching and watch-and-restart fallback.

pub mod hot;
pub mod mirror;
pub mod proc;
pub mod restart;

use std::path::PathBuf;

use crate::CliError;
use crate::args::{DevArgs, DevMode};
use crate::config::{ResolvedDevConfig, resolve_dev_config};
use crate::dx::{self, DxStatus};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SelectedMode {
    Hot {
        dx_path: PathBuf,
        windows_opt_in_notice: bool,
    },
    Restart {
        reason: Option<String>,
    },
}

/// Decides whether `rune dev` runs guarded `dx` hot patching or watch-and-restart `cargo run`.
pub fn select_mode(
    requested: DevMode,
    is_windows: bool,
    has_hot_feature: bool,
    dx_status: &DxStatus,
) -> Result<SelectedMode, CliError> {
    match requested {
        DevMode::Restart => Ok(SelectedMode::Restart { reason: None }),
        DevMode::Hot => {
            if !has_hot_feature {
                return Err(CliError::new(
                    "live hot patching requires `hot = [\"rune/hot\"]` under [features] in Cargo.toml (or run `rune dev --mode restart`)",
                ));
            }
            match dx_status {
                DxStatus::Ready { path, .. } => Ok(SelectedMode::Hot {
                    dx_path: path.clone(),
                    windows_opt_in_notice: is_windows,
                }),
                DxStatus::WrongVersion { path, version } => Err(CliError::new(format!(
                    "expected dioxus-cli {} for `--mode hot`, found '{}' at {} (the Deno `dx` command is unrelated). Install with: {}",
                    crate::REQUIRED_DX_VERSION,
                    version,
                    path.display(),
                    DxStatus::install_hint()
                ))),
                DxStatus::Missing => Err(CliError::new(format!(
                    "dioxus-cli {} is required for `--mode hot`. Install with: {}",
                    crate::REQUIRED_DX_VERSION,
                    DxStatus::install_hint()
                ))),
            }
        }
        DevMode::Auto => {
            if is_windows {
                return Ok(SelectedMode::Restart {
                    reason: Some(
                        "running in watch-and-restart mode on Windows (live dylib patching via dx 0.7.10 is experimental on Windows; pass --mode hot to opt in)".to_string(),
                    ),
                });
            }
            if !has_hot_feature {
                return Ok(SelectedMode::Restart {
                    reason: Some(
                        "no `hot` feature in Cargo.toml; running in watch-and-restart mode (add `hot = [\"rune/hot\"]` under [features] to enable live fn-body patching)".to_string(),
                    ),
                });
            }
            match dx_status {
                DxStatus::Ready { path, .. } => Ok(SelectedMode::Hot {
                    dx_path: path.clone(),
                    windows_opt_in_notice: false,
                }),
                DxStatus::WrongVersion { path, version } => Ok(SelectedMode::Restart {
                    reason: Some(format!(
                        "found '{}' at {} instead of dioxus-cli {}; running in watch-and-restart mode (install with: {})",
                        version,
                        path.display(),
                        crate::REQUIRED_DX_VERSION,
                        DxStatus::install_hint()
                    )),
                }),
                DxStatus::Missing => Ok(SelectedMode::Restart {
                    reason: Some(format!(
                        "dioxus-cli {} not found; running in watch-and-restart mode (for live fn-body patching: {})",
                        crate::REQUIRED_DX_VERSION,
                        DxStatus::install_hint()
                    )),
                }),
            }
        }
    }
}

/// Resolves project config and runs `rune dev`.
pub fn run(args: &DevArgs) -> Result<(), CliError> {
    let cwd = std::env::current_dir()
        .map_err(|err| CliError::new(format!("cannot read current directory: {err}")))?;
    let cfg: ResolvedDevConfig = resolve_dev_config(args, &cwd)?;
    let dx_status = if cfg.mode == DevMode::Restart {
        DxStatus::Missing
    } else {
        dx::detect()
    };

    match select_mode(cfg.mode, cfg!(windows), cfg.meta.has_hot_feature, &dx_status)? {
        SelectedMode::Hot {
            dx_path,
            windows_opt_in_notice,
        } => {
            if windows_opt_in_notice {
                eprintln!(
                    "rune dev: --mode hot on Windows is experimental because upstream dx 0.7.10 has known Windows patching issues"
                );
            }
            hot::run_hot(&cfg, &dx_path)
        }
        SelectedMode::Restart { reason } => {
            if let Some(msg) = reason {
                eprintln!("rune dev: {msg}");
            }
            restart::run_restart(&cfg)
        }
    }
}
