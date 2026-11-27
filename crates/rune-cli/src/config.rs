//! Manifest discovery and `rune.toml` / `[package.metadata.rune.dev]` configuration loading.

use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use toml_edit::{DocumentMut, Item, TableLike};

use crate::CliError;
use crate::args::{DevArgs, DevMode};

pub const DEFAULT_PORT: u16 = 18745;

pub fn default_patch_globs() -> Vec<String> {
    vec![
        "src/view.rs".to_string(),
        "src/ui/**/*.rs".to_string(),
        "src/main.rs".to_string(),
    ]
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FileDevConfig {
    pub patch: Option<Vec<String>>,
    pub features: Option<Vec<String>>,
    pub bin: Option<String>,
    pub package: Option<String>,
    pub port: Option<u16>,
    pub mode: Option<DevMode>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CrateMeta {
    pub manifest_path: PathBuf,
    pub crate_dir: PathBuf,
    pub workspace_root: PathBuf,
    pub package_name: String,
    pub package_version: Option<String>,
    pub has_hot_feature: bool,
    pub has_hot_profile: bool,
    pub metadata_dev: FileDevConfig,
}

impl CrateMeta {
    /// Returns `name@version` when a version is known, or `name` otherwise.
    pub fn package_spec(&self) -> String {
        match &self.package_version {
            Some(ver) if !ver.is_empty() => format!("{}@{ver}", self.package_name),
            _ => self.package_name.clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedDevConfig {
    pub meta: CrateMeta,
    pub config_source: Option<PathBuf>,
    pub package: String,
    pub bin: Option<String>,
    pub features: Vec<String>,
    pub patch: Vec<String>,
    pub port: u16,
    pub mode: DevMode,
    pub jobs: usize,
    pub target_dir: Option<PathBuf>,
    pub app_args: Vec<String>,
}

/// Locates `Cargo.toml` from an explicit `--manifest-path` or by walking up from `start_dir`.
pub fn find_manifest(explicit: Option<&Path>, start_dir: &Path) -> Result<PathBuf, CliError> {
    if let Some(path) = explicit {
        let resolved = if path.is_absolute() {
            path.to_path_buf()
        } else {
            start_dir.join(path)
        };
        if !resolved.is_file() {
            return Err(CliError::new(format!(
                "manifest not found: {}",
                resolved.display()
            )));
        }
        return canonical_or_self(&resolved);
    }

    let mut dir = start_dir.to_path_buf();
    loop {
        let candidate = dir.join("Cargo.toml");
        if candidate.is_file() {
            return canonical_or_self(&candidate);
        }
        if !dir.pop() {
            return Err(CliError::new(format!(
                "could not find Cargo.toml in {} or any parent directory (pass --manifest-path)",
                start_dir.display()
            )));
        }
    }
}

/// Finds the enclosing workspace root for `crate_dir`, or `crate_dir` itself if standalone.
pub fn find_workspace_root(crate_dir: &Path) -> Result<PathBuf, CliError> {
    let manifest_path = crate_dir.join("Cargo.toml");
    let text = fs::read_to_string(&manifest_path).map_err(|err| {
        CliError::new(format!(
            "failed to read {}: {err}",
            manifest_path.display()
        ))
    })?;
    let doc = parse_toml(&text, &manifest_path)?;

    if let Some(explicit) = doc
        .get("package")
        .and_then(Item::as_table_like)
        .and_then(|t| t.get("workspace"))
        .and_then(Item::as_str)
    {
        return canonical_or_self(&crate_dir.join(explicit));
    }

    let mut dir = crate_dir.to_path_buf();
    loop {
        let candidate = dir.join("Cargo.toml");
        if candidate.is_file()
            && let Ok(candidate_text) = fs::read_to_string(&candidate)
            && let Ok(candidate_doc) = candidate_text.parse::<DocumentMut>()
            && candidate_doc.get("workspace").is_some()
        {
            return canonical_or_self(&dir);
        }
        if !dir.pop() {
            return canonical_or_self(crate_dir);
        }
    }
}

/// Reads crate and workspace metadata from `manifest_path`.
pub fn load_crate_meta(manifest_path: &Path) -> Result<CrateMeta, CliError> {
    let manifest_path = canonical_or_self(manifest_path)?;
    let crate_dir = manifest_path
        .parent()
        .ok_or_else(|| CliError::new("manifest path has no parent directory"))?
        .to_path_buf();

    let text = fs::read_to_string(&manifest_path).map_err(|err| {
        CliError::new(format!(
            "failed to read {}: {err}",
            manifest_path.display()
        ))
    })?;
    let doc = parse_toml(&text, &manifest_path)?;

    let package = doc
        .get("package")
        .and_then(Item::as_table_like)
        .ok_or_else(|| {
            CliError::new(format!(
                "{} has no [package] table (pass --manifest-path to an app crate's Cargo.toml)",
                manifest_path.display()
            ))
        })?;

    let package_name = package
        .get("name")
        .and_then(Item::as_str)
        .ok_or_else(|| {
            CliError::new(format!(
                "{} is missing [package].name",
                manifest_path.display()
            ))
        })?
        .to_string();

    let workspace_root = find_workspace_root(&crate_dir)?;
    let ws_manifest = workspace_root.join("Cargo.toml");
    let ws_doc = if ws_manifest == manifest_path {
        None
    } else {
        fs::read_to_string(&ws_manifest)
            .ok()
            .and_then(|s| s.parse::<DocumentMut>().ok())
    };

    let package_version = resolve_version(package, ws_doc.as_ref().unwrap_or(&doc));

    let has_hot_feature = doc
        .get("features")
        .and_then(Item::as_table_like)
        .is_some_and(|t| t.contains_key("hot"));

    let profile_doc = ws_doc.as_ref().unwrap_or(&doc);
    let has_hot_profile = profile_doc
        .get("profile")
        .and_then(Item::as_table_like)
        .is_some_and(|t| t.contains_key("rune-hot"));

    let metadata_dev = package
        .get("metadata")
        .and_then(Item::as_table_like)
        .and_then(|m| m.get("rune"))
        .and_then(Item::as_table_like)
        .and_then(|r| r.get("dev"))
        .and_then(Item::as_table_like)
        .map(|tbl| parse_dev_table(tbl, &manifest_path))
        .transpose()?
        .unwrap_or_default();

    Ok(CrateMeta {
        manifest_path,
        crate_dir,
        workspace_root,
        package_name,
        package_version,
        has_hot_feature,
        has_hot_profile,
        metadata_dev,
    })
}

/// Loads `rune.toml` beside the crate manifest, falling back to the workspace root.
pub fn load_rune_toml(
    crate_dir: &Path,
    workspace_root: &Path,
) -> Result<Option<(PathBuf, FileDevConfig)>, CliError> {
    let crate_cfg = crate_dir.join("rune.toml");
    let chosen = if crate_cfg.is_file() {
        Some(crate_cfg)
    } else {
        let ws_cfg = workspace_root.join("rune.toml");
        if ws_cfg.is_file() { Some(ws_cfg) } else { None }
    };

    let Some(path) = chosen else {
        return Ok(None);
    };
    let text = fs::read_to_string(&path)
        .map_err(|err| CliError::new(format!("failed to read {}: {err}", path.display())))?;
    let doc = parse_toml(&text, &path)?;
    let dev = doc
        .get("dev")
        .and_then(Item::as_table_like)
        .map(|tbl| parse_dev_table(tbl, &path))
        .transpose()?
        .unwrap_or_default();

    Ok(Some((path, dev)))
}

/// Resolves effective `rune dev` settings with precedence:
/// built-in defaults < `Cargo.toml` metadata < `rune.toml` < CLI flags.
pub fn resolve_dev_config(args: &DevArgs, cwd: &Path) -> Result<ResolvedDevConfig, CliError> {
    let manifest_path = find_manifest(args.manifest_path.as_deref(), cwd)?;
    let meta = load_crate_meta(&manifest_path)?;
    let rune_toml = load_rune_toml(&meta.crate_dir, &meta.workspace_root)?;

    let mut merged = meta.metadata_dev.clone();
    let config_source = if let Some((path, file_cfg)) = rune_toml {
        if file_cfg.patch.is_some() {
            merged.patch = file_cfg.patch;
        }
        if file_cfg.features.is_some() {
            merged.features = file_cfg.features;
        }
        if file_cfg.bin.is_some() {
            merged.bin = file_cfg.bin;
        }
        if file_cfg.package.is_some() {
            merged.package = file_cfg.package;
        }
        if file_cfg.port.is_some() {
            merged.port = file_cfg.port;
        }
        if file_cfg.mode.is_some() {
            merged.mode = file_cfg.mode;
        }
        Some(path)
    } else {
        None
    };

    let package = args
        .package
        .clone()
        .or(merged.package)
        .unwrap_or_else(|| meta.package_name.clone());
    let bin = args.bin.clone().or(merged.bin);

    let mut features = merged.features.unwrap_or_default();
    for f in &args.features {
        if !features.contains(f) {
            features.push(f.clone());
        }
    }

    let patch = if !args.patch.is_empty() {
        args.patch.clone()
    } else {
        merged.patch.unwrap_or_else(default_patch_globs)
    };

    let port = args.port.or(merged.port).unwrap_or(DEFAULT_PORT);
    let mode = args.mode.or(merged.mode).unwrap_or(DevMode::Auto);
    let jobs = args.jobs.unwrap_or_else(default_jobs);
    let target_dir = args
        .target_dir
        .clone()
        .or_else(|| {
            env::var_os("RUNE_DEV_TARGET_DIR")
                .filter(|v| !v.is_empty())
                .map(PathBuf::from)
        });

    Ok(ResolvedDevConfig {
        meta,
        config_source,
        package,
        bin,
        features,
        patch,
        port,
        mode,
        jobs,
        target_dir,
        app_args: args.app_args.clone(),
    })
}

fn default_jobs() -> usize {
    for key in ["RUNE_DEV_JOBS", "CARGO_BUILD_JOBS"] {
        if let Some(val) = env::var_os(key).and_then(|v| v.into_string().ok())
            && let Ok(parsed) = val.trim().parse::<usize>()
            && parsed > 0
        {
            return parsed;
        }
    }
    2
}

fn resolve_version(package: &dyn TableLike, ws_doc: &DocumentMut) -> Option<String> {
    let ver_item = package.get("version")?;
    if let Some(s) = ver_item.as_str() {
        return Some(s.to_string());
    }
    if ver_item
        .as_table_like()
        .and_then(|t| t.get("workspace"))
        .and_then(Item::as_bool)
        == Some(true)
    {
        return ws_doc
            .get("workspace")
            .and_then(Item::as_table_like)
            .and_then(|w| w.get("package"))
            .and_then(Item::as_table_like)
            .and_then(|p| p.get("version"))
            .and_then(Item::as_str)
            .map(ToString::to_string);
    }
    None
}

fn parse_dev_table(table: &dyn TableLike, path: &Path) -> Result<FileDevConfig, CliError> {
    let mut cfg = FileDevConfig::default();

    if let Some(item) = table.get("patch") {
        cfg.patch = Some(parse_string_array(item, "patch", path)?);
    }
    if let Some(item) = table.get("features") {
        cfg.features = Some(parse_string_array(item, "features", path)?);
    }
    if let Some(item) = table.get("bin") {
        let s = item.as_str().ok_or_else(|| {
            CliError::new(format!("{}: `bin` must be a string", path.display()))
        })?;
        cfg.bin = Some(s.to_string());
    }
    if let Some(item) = table.get("package") {
        let s = item.as_str().ok_or_else(|| {
            CliError::new(format!("{}: `package` must be a string", path.display()))
        })?;
        cfg.package = Some(s.to_string());
    }
    if let Some(item) = table.get("port") {
        let n = item.as_integer().ok_or_else(|| {
            CliError::new(format!("{}: `port` must be an integer", path.display()))
        })?;
        let port = u16::try_from(n)
            .ok()
            .filter(|p| *p > 0)
            .ok_or_else(|| {
                CliError::new(format!(
                    "{}: `port` must be between 1 and 65535",
                    path.display()
                ))
            })?;
        cfg.port = Some(port);
    }
    if let Some(item) = table.get("mode") {
        let s = item.as_str().ok_or_else(|| {
            CliError::new(format!("{}: `mode` must be a string", path.display()))
        })?;
        cfg.mode = Some(DevMode::parse(s)?);
    }

    Ok(cfg)
}

fn parse_string_array(item: &Item, key: &str, path: &Path) -> Result<Vec<String>, CliError> {
    let arr = item.as_array().ok_or_else(|| {
        CliError::new(format!(
            "{}: `{key}` must be an array of strings",
            path.display()
        ))
    })?;
    let mut values = Vec::with_capacity(arr.len());
    for elem in arr.iter() {
        let s = elem.as_str().ok_or_else(|| {
            CliError::new(format!(
                "{}: every entry in `{key}` must be a string",
                path.display()
            ))
        })?;
        values.push(s.to_string());
    }
    Ok(values)
}

pub(crate) fn parse_toml(text: &str, path: &Path) -> Result<DocumentMut, CliError> {
    text.parse::<DocumentMut>()
        .map_err(|err| CliError::new(format!("invalid TOML in {}: {err}", path.display())))
}

pub(crate) fn canonical_or_self(path: &Path) -> Result<PathBuf, CliError> {
    match fs::canonicalize(path) {
        Ok(canon) => Ok(strip_windows_verbatim(canon)),
        Err(_) => Ok(path.to_path_buf()),
    }
}

/// Strips the `\\?\` verbatim prefix that `std::fs::canonicalize` adds on Windows so paths
/// stay readable in TOML manifests and compatible with external tools like `dx` and `cargo`.
pub(crate) fn strip_windows_verbatim(path: PathBuf) -> PathBuf {
    #[cfg(windows)]
    {
        if let Some(s) = path.to_str()
            && let Some(stripped) = s.strip_prefix(r"\\?\")
        {
            if let Some(unc) = stripped.strip_prefix("UNC\\") {
                return PathBuf::from(format!(r"\\{unc}"));
            }
            return PathBuf::from(stripped);
        }
    }
    path
}
