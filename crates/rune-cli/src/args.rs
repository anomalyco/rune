//! Argument parser and help text for the `rune` CLI.

use std::path::PathBuf;

use crate::CliError;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DevMode {
    Auto,
    Hot,
    Restart,
}

impl DevMode {
    pub fn parse(value: &str) -> Result<Self, CliError> {
        match value {
            "auto" => Ok(Self::Auto),
            "hot" => Ok(Self::Hot),
            "restart" => Ok(Self::Restart),
            other => Err(CliError::usage(format!(
                "invalid --mode '{other}'; expected one of: auto, hot, restart"
            ))),
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Hot => "hot",
            Self::Restart => "restart",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewArgs {
    pub target: PathBuf,
    pub name: Option<String>,
    pub rev: Option<String>,
    pub git: Option<String>,
    pub rune_path: Option<PathBuf>,
    pub force: bool,
    pub no_git: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DevArgs {
    pub manifest_path: Option<PathBuf>,
    pub package: Option<String>,
    pub bin: Option<String>,
    pub features: Vec<String>,
    pub patch: Vec<String>,
    pub port: Option<u16>,
    pub mode: Option<DevMode>,
    pub jobs: Option<usize>,
    pub target_dir: Option<PathBuf>,
    pub app_args: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckArgs {
    pub manifest_path: Option<PathBuf>,
    pub lock_path: Option<PathBuf>,
    pub rune_manifest: Option<PathBuf>,
    pub wasm_bindgen_cli: Option<String>,
    pub pins_only: bool,
    pub cargo_args: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DoctorArgs {
    pub manifest_path: Option<PathBuf>,
    pub rune_manifest: Option<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    New(NewArgs),
    Dev(DevArgs),
    Check(CheckArgs),
    Doctor(DoctorArgs),
    Help(Option<String>),
    Version,
}

/// Parses command-line arguments (including `argv[0]`).
/// When invoked as a Cargo subcommand (`cargo rune ...`), strips the leading `"rune"` token.
pub fn parse(argv: &[String]) -> Result<Command, CliError> {
    let mut slice = argv.get(1..).unwrap_or(&[]);
    if let Some(exe) = argv.first() {
        let stem = std::path::Path::new(exe)
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("");
        if stem == "cargo-rune" && slice.first().is_some_and(|a| a == "rune") {
            slice = &slice[1..];
        }
    }

    let Some(first) = slice.first().map(String::as_str) else {
        return Ok(Command::Help(None));
    };

    match first {
        "-h" | "--help" => Ok(Command::Help(None)),
        "help" => Ok(Command::Help(slice.get(1).cloned())),
        "-V" | "--version" | "version" => Ok(Command::Version),
        "new" => parse_new(&slice[1..]),
        "dev" => parse_dev(&slice[1..]),
        "check" => parse_check(&slice[1..]),
        "doctor" => parse_doctor(&slice[1..]),
        other => Err(CliError::usage(format!(
            "unknown command '{other}'. Run `rune --help` to see available commands."
        ))),
    }
}

fn next_value<'a>(args: &'a [String], i: &mut usize, flag: &str) -> Result<&'a str, CliError> {
    *i += 1;
    args.get(*i)
        .map(String::as_str)
        .ok_or_else(|| CliError::usage(format!("{flag} requires a value")))
}

fn split_flag(arg: &str) -> (&str, Option<&str>) {
    match arg.split_once('=') {
        Some((k, v)) if k.starts_with("--") => (k, Some(v)),
        _ => (arg, None),
    }
}

fn value_or_next<'a>(
    args: &'a [String],
    i: &mut usize,
    flag: &str,
    inline: Option<&'a str>,
) -> Result<&'a str, CliError> {
    match inline {
        Some(v) => Ok(v),
        None => next_value(args, i, flag),
    }
}

fn parse_new(args: &[String]) -> Result<Command, CliError> {
    let mut target: Option<PathBuf> = None;
    let mut name = None;
    let mut rev = None;
    let mut git = None;
    let mut rune_path = None;
    let mut force = false;
    let mut no_git = false;

    let mut i = 0;
    while i < args.len() {
        let raw = args[i].as_str();
        let (flag, inline) = split_flag(raw);
        match flag {
            "-h" | "--help" => return Ok(Command::Help(Some("new".to_string()))),
            "--name" => name = Some(value_or_next(args, &mut i, "--name", inline)?.to_string()),
            "--rev" => rev = Some(value_or_next(args, &mut i, "--rev", inline)?.to_string()),
            "--git" => git = Some(value_or_next(args, &mut i, "--git", inline)?.to_string()),
            "--path" => {
                rune_path = Some(PathBuf::from(value_or_next(args, &mut i, "--path", inline)?))
            }
            "--force" => force = true,
            "--no-git" => no_git = true,
            other if other.starts_with('-') => {
                return Err(CliError::usage(format!("unknown flag for `rune new`: {other}")));
            }
            positional => {
                if target.is_some() {
                    return Err(CliError::usage(format!(
                        "unexpected extra argument for `rune new`: {positional}"
                    )));
                }
                target = Some(PathBuf::from(positional));
            }
        }
        i += 1;
    }

    if rune_path.is_some() && (rev.is_some() || git.is_some()) {
        return Err(CliError::usage(
            "--path cannot be combined with --rev or --git",
        ));
    }

    let Some(target) = target else {
        return Err(CliError::usage(
            "`rune new` requires a project path or name (for example: `rune new my-app`)",
        ));
    };

    Ok(Command::New(NewArgs {
        target,
        name,
        rev,
        git,
        rune_path,
        force,
        no_git,
    }))
}

fn parse_dev(args: &[String]) -> Result<Command, CliError> {
    let mut opts = DevArgs {
        manifest_path: None,
        package: None,
        bin: None,
        features: Vec::new(),
        patch: Vec::new(),
        port: None,
        mode: None,
        jobs: None,
        target_dir: None,
        app_args: Vec::new(),
    };

    let mut i = 0;
    while i < args.len() {
        let raw = args[i].as_str();
        if raw == "--" {
            opts.app_args.extend(args[i + 1..].iter().cloned());
            break;
        }
        let (flag, inline) = split_flag(raw);
        match flag {
            "-h" | "--help" => return Ok(Command::Help(Some("dev".to_string()))),
            "--manifest-path" => {
                let v = value_or_next(args, &mut i, "--manifest-path", inline)?;
                opts.manifest_path = Some(PathBuf::from(v));
            }
            "-p" | "--package" => {
                let v = value_or_next(args, &mut i, "--package", inline)?;
                opts.package = Some(v.to_string());
            }
            "--bin" => {
                let v = value_or_next(args, &mut i, "--bin", inline)?;
                opts.bin = Some(v.to_string());
            }
            "-F" | "--features" => {
                let v = value_or_next(args, &mut i, "--features", inline)?;
                for feat in v.split(',').map(str::trim).filter(|s| !s.is_empty()) {
                    opts.features.push(feat.to_string());
                }
            }
            "--patch" => {
                let v = value_or_next(args, &mut i, "--patch", inline)?;
                opts.patch.push(v.to_string());
            }
            "--port" => {
                let v = value_or_next(args, &mut i, "--port", inline)?;
                let port = v.parse::<u16>().map_err(|_| {
                    CliError::usage(format!("invalid --port '{v}': expected 1..=65535"))
                })?;
                if port == 0 {
                    return Err(CliError::usage("invalid --port '0': expected 1..=65535"));
                }
                opts.port = Some(port);
            }
            "--mode" => {
                let v = value_or_next(args, &mut i, "--mode", inline)?;
                opts.mode = Some(DevMode::parse(v)?);
            }
            "-j" | "--jobs" => {
                let v = value_or_next(args, &mut i, "--jobs", inline)?;
                let jobs = v.parse::<usize>().map_err(|_| {
                    CliError::usage(format!("invalid --jobs '{v}': expected positive integer"))
                })?;
                if jobs == 0 {
                    return Err(CliError::usage("invalid --jobs '0': expected positive integer"));
                }
                opts.jobs = Some(jobs);
            }
            "--target-dir" => {
                let v = value_or_next(args, &mut i, "--target-dir", inline)?;
                opts.target_dir = Some(PathBuf::from(v));
            }
            other => {
                return Err(CliError::usage(format!(
                    "unknown argument for `rune dev`: {other} (pass app arguments after `--`)"
                )));
            }
        }
        i += 1;
    }

    Ok(Command::Dev(opts))
}

fn parse_check(args: &[String]) -> Result<Command, CliError> {
    let mut opts = CheckArgs {
        manifest_path: None,
        lock_path: None,
        rune_manifest: None,
        wasm_bindgen_cli: None,
        pins_only: false,
        cargo_args: Vec::new(),
    };

    let mut i = 0;
    while i < args.len() {
        let raw = args[i].as_str();
        if raw == "--" {
            opts.cargo_args.extend(args[i + 1..].iter().cloned());
            break;
        }
        let (flag, inline) = split_flag(raw);
        match flag {
            "-h" | "--help" => return Ok(Command::Help(Some("check".to_string()))),
            "--manifest-path" => {
                let v = value_or_next(args, &mut i, "--manifest-path", inline)?;
                opts.manifest_path = Some(PathBuf::from(v));
            }
            "--lock" => {
                let v = value_or_next(args, &mut i, "--lock", inline)?;
                opts.lock_path = Some(PathBuf::from(v));
            }
            "--rune-manifest" => {
                let v = value_or_next(args, &mut i, "--rune-manifest", inline)?;
                opts.rune_manifest = Some(PathBuf::from(v));
            }
            "--wasm-bindgen-cli" => {
                let v = value_or_next(args, &mut i, "--wasm-bindgen-cli", inline)?;
                opts.wasm_bindgen_cli = Some(v.to_string());
            }
            "--pins-only" => opts.pins_only = true,
            other => {
                return Err(CliError::usage(format!(
                    "unknown argument for `rune check`: {other} (pass extra cargo flags after `--`)"
                )));
            }
        }
        i += 1;
    }

    Ok(Command::Check(opts))
}

fn parse_doctor(args: &[String]) -> Result<Command, CliError> {
    let mut opts = DoctorArgs {
        manifest_path: None,
        rune_manifest: None,
    };

    let mut i = 0;
    while i < args.len() {
        let raw = args[i].as_str();
        let (flag, inline) = split_flag(raw);
        match flag {
            "-h" | "--help" => return Ok(Command::Help(Some("doctor".to_string()))),
            "--manifest-path" => {
                let v = value_or_next(args, &mut i, "--manifest-path", inline)?;
                opts.manifest_path = Some(PathBuf::from(v));
            }
            "--rune-manifest" => {
                let v = value_or_next(args, &mut i, "--rune-manifest", inline)?;
                opts.rune_manifest = Some(PathBuf::from(v));
            }
            other => {
                return Err(CliError::usage(format!(
                    "unknown argument for `rune doctor`: {other}"
                )));
            }
        }
        i += 1;
    }

    Ok(Command::Doctor(opts))
}

pub fn help_text(topic: Option<&str>) -> &'static str {
    match topic {
        Some("new") => "Create a new Rune application project.

Usage:
  rune new <path> [options]

Options:
  --name <pkg>        Override Cargo package name (defaults to directory name)
  --rev <rev>         Pin a specific Rune git revision (defaults to this CLI's build rev)
  --git <url>         Use a custom Rune git URL or mirror
  --path <rune-dir>   Depend on a local Rune checkout instead of git
  --force             Write into a non-empty target directory
  --no-git            Do not run `git init` in the new project
  -h, --help          Show this help message
",
        Some("dev") => "Run a Rune application with guarded live hot reload (or watch-and-restart fallback).

Usage:
  rune dev [options] [-- <app-args>...]

Options:
  --manifest-path <Cargo.toml>  Path to the app's Cargo.toml (auto-detected by default)
  -p, --package <name>          Cargo package name
  --bin <name>                  Binary target name
  -F, --features <a,b>          Extra Cargo features (repeatable or comma-separated)
  --patch <glob>                Relative source globs eligible for live fn-body patching (repeatable)
  --port <port>                 Devserver port for dx hot patching (default: 18745)
  --mode <auto|hot|restart>     Dev loop mode (default: auto)
  -j, --jobs <n>                Cargo build jobs (default: RUNE_DEV_JOBS / CARGO_BUILD_JOBS / 2)
  --target-dir <dir>            Override Cargo target directory for hot mirror builds
  -h, --help                    Show this help message

Configuration:
  Defaults can be stored in `rune.toml` under `[dev]` or in `Cargo.toml` under
  `[package.metadata.rune.dev]` (keys: patch, features, bin, package, port, mode).
",
        Some("check") => "Verify Rune git/patch pins and run `cargo check`.

Usage:
  rune check [options] [-- <cargo-args>...]

Options:
  --manifest-path <Cargo.toml>  Path to Cargo.toml (auto-detected by default)
  --lock <Cargo.lock>           Path to Cargo.lock (defaults to workspace Cargo.lock)
  --rune-manifest <Cargo.toml>  Rune root Cargo.toml to read required [patch.crates-io] from
  --wasm-bindgen-cli <version>  Verify Cargo.lock's wasm-bindgen matches this CLI version
  --pins-only                   Validate Rune pins only; skip `cargo check`
  -h, --help                    Show this help message
",
        Some("doctor") => "Inspect local toolchain, `dx` hot-reload readiness, and current project configuration.

Usage:
  rune doctor [options]

Options:
  --manifest-path <Cargo.toml>  Path to a Rune app's Cargo.toml
  --rune-manifest <Cargo.toml>  Rune root Cargo.toml to check [patch.crates-io] against
  -h, --help                    Show this help message
",
        _ => "Rune CLI — project scaffolding, pin checks, and cross-platform guarded hot reload.

Usage:
  rune <command> [options]

Commands:
  new <path>   Create a new Rune application with pinned dependencies and hot reload wired in
  dev          Run the app with guarded hot patching (or automatic watch-and-restart fallback)
  check        Verify Rune git/[patch.crates-io] pins and run `cargo check`
  doctor       Check Rust toolchain, `dx` 0.7.10 status, platform libraries, and project config
  help [cmd]   Show help for a command

Flags:
  -h, --help     Show help
  -V, --version  Show version and pinned Rune revision
",
    }
}
