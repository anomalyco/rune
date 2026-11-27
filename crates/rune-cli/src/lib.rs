//! Cross-platform CLI for creating, checking, and running Rune apps with guarded hot reload.

#![forbid(unsafe_code)]

pub mod args;
pub mod config;
pub mod dev;
pub mod doctor;
pub mod dx;
pub mod glob;
pub mod guard;
pub mod new;
pub mod pins;
pub mod watch;

use std::fmt;

/// Default Rune git repository URL used when scaffolding new apps.
pub const DEFAULT_REPO_URL: &str = "https://github.com/anomalyco/rune";

/// Git revision captured when `rune-cli` was compiled (empty if built outside a git checkout).
pub const DEFAULT_REV: &str = env!("RUNE_DEFAULT_REV");

/// Required Dioxus CLI version for live function-body hot patching.
pub const REQUIRED_DX_VERSION: &str = "0.7.10";

const DEFAULT_PATCHES_CSV: &str = env!("RUNE_DEFAULT_PATCHES");

/// Returns the `[patch.crates-io]` crate names required by Rune at the revision this CLI was built from.
pub fn default_patches() -> Vec<String> {
    DEFAULT_PATCHES_CSV
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(ToString::to_string)
        .collect()
}

/// Structured CLI error carrying a user-facing message and process exit code.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CliError {
    pub message: String,
    pub exit_code: i32,
}

impl CliError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            exit_code: 1,
        }
    }

    pub fn usage(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            exit_code: 2,
        }
    }

    pub fn with_code(message: impl Into<String>, exit_code: i32) -> Self {
        Self {
            message: message.into(),
            exit_code,
        }
    }
}

impl fmt::Display for CliError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for CliError {}

/// Parses `argv` (including `argv[0]`) and executes the requested subcommand.
pub fn run(argv: &[String]) -> Result<(), CliError> {
    let command = args::parse(argv)?;
    match command {
        args::Command::Help(topic) => {
            print!("{}", args::help_text(topic.as_deref()));
            Ok(())
        }
        args::Command::Version => {
            if DEFAULT_REV.is_empty() {
                println!("rune {}", env!("CARGO_PKG_VERSION"));
            } else {
                let short = &DEFAULT_REV[..DEFAULT_REV.len().min(12)];
                println!("rune {} ({short})", env!("CARGO_PKG_VERSION"));
            }
            Ok(())
        }
        args::Command::New(opts) => new::run(&opts),
        args::Command::Dev(opts) => dev::run(&opts),
        args::Command::Check(opts) => pins::run_check(&opts),
        args::Command::Doctor(opts) => doctor::run(&opts),
    }
}
