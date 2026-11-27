//! Private compiler mirror management for guarded hot-patch builds.

use std::collections::{BTreeMap, BTreeSet};
use std::env;
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::time::Duration;

use toml_edit::{Array, DocumentMut, Item, TableLike, Value};

use crate::CliError;
use crate::config::{canonical_or_self, find_workspace_root, parse_toml};
use crate::glob;
use crate::watch::{Snapshot, walk_files};

const WORKSPACE_TOP_FILES: &[&str] = &[
    "Cargo.toml",
    "Cargo.lock",
    "rust-toolchain",
    "rust-toolchain.toml",
    ".cargo/config.toml",
    ".cargo/config",
];

/// Describes the app crate, its transitive local path dependencies, and their workspace roots.
#[derive(Debug, Clone)]
pub struct ProjectLayout {
    pub base: PathBuf,
    pub crate_key: String,
    pub package_dirs: Vec<String>,
    pub top_files: Vec<String>,
}

impl ProjectLayout {
    /// Collects all `(mirror_key, host_path)` pairs tracked for this project layout.
    pub fn source_entries(&self) -> Vec<(String, PathBuf)> {
        let mut map: BTreeMap<String, PathBuf> = BTreeMap::new();
        for top in &self.top_files {
            let host = join_posix(&self.base, top);
            if host.is_file() {
                map.insert(format!("external/{top}"), host);
            }
        }
        for pkg in &self.package_dirs {
            let pkg_root = if pkg.is_empty() {
                self.base.clone()
            } else {
                join_posix(&self.base, pkg)
            };
            for rel in walk_files(&pkg_root) {
                let key = if pkg.is_empty() {
                    format!("external/{rel}")
                } else {
                    format!("external/{pkg}/{rel}")
                };
                let host = join_posix(&pkg_root, &rel);
                map.insert(key, host);
            }
        }
        map.into_iter().collect()
    }
}

/// Discovers the app crate, every transitive local path dependency, and their workspace roots.
pub fn discover_layout(manifest_path: &Path) -> Result<ProjectLayout, CliError> {
    let manifest_path = canonical_or_self(manifest_path)?;
    let crate_dir = manifest_path
        .parent()
        .ok_or_else(|| CliError::new("manifest path has no parent directory"))?
        .to_path_buf();

    let mut packages: Vec<PathBuf> = Vec::new();
    let mut roots: Vec<PathBuf> = Vec::new();
    let mut pending: Vec<PathBuf> = vec![crate_dir.clone()];

    while let Some(pkg) = pending.pop() {
        let pkg = canonical_or_self(&pkg)?;
        if packages.contains(&pkg) {
            continue;
        }
        let pkg_manifest = pkg.join("Cargo.toml");
        if !pkg_manifest.is_file() {
            return Err(CliError::new(format!(
                "path dependency {} has no Cargo.toml",
                pkg.display()
            )));
        }
        packages.push(pkg.clone());

        let text = fs::read_to_string(&pkg_manifest).map_err(|err| {
            CliError::new(format!(
                "failed to read {}: {err}",
                pkg_manifest.display()
            ))
        })?;
        let doc = parse_toml(&text, &pkg_manifest)?;
        for dep in path_dependencies(&doc, &pkg) {
            if !packages.contains(&dep) {
                pending.push(dep);
            }
        }

        let ws_root = find_workspace_root(&pkg)?;
        if !roots.contains(&ws_root) {
            roots.push(ws_root.clone());
            let ws_manifest = ws_root.join("Cargo.toml");
            if ws_manifest.is_file()
                && let Ok(ws_text) = fs::read_to_string(&ws_manifest)
                && let Ok(ws_doc) = parse_toml(&ws_text, &ws_manifest)
            {
                for dep in path_dependencies(&ws_doc, &ws_root) {
                    if !packages.contains(&dep) {
                        pending.push(dep);
                    }
                }
            }
        }
    }

    let mut all_paths = packages.clone();
    all_paths.extend(roots.iter().cloned());
    let base = common_ancestor(&all_paths).ok_or_else(|| {
        CliError::new(
            "could not compute a common ancestor directory for the app and its local path dependencies (they may live on different Windows drives)",
        )
    })?;

    let rel_posix = |p: &Path| -> String {
        p.strip_prefix(&base)
            .map(|r| glob::normalize_rel(&r.to_string_lossy()))
            .unwrap_or_default()
    };

    let crate_rel = rel_posix(&crate_dir);
    let crate_key = if crate_rel.is_empty() {
        "external".to_string()
    } else {
        format!("external/{crate_rel}")
    };

    let mut pkg_set = BTreeSet::new();
    for p in &packages {
        pkg_set.insert(rel_posix(p));
    }
    let package_dirs: Vec<String> = pkg_set.into_iter().collect();

    let mut top_set = BTreeSet::new();
    for r in &roots {
        for name in WORKSPACE_TOP_FILES {
            let candidate = join_posix(r, name);
            if candidate.is_file() {
                top_set.insert(rel_posix(&candidate));
            }
        }
    }
    let top_files: Vec<String> = top_set.into_iter().collect();

    Ok(ProjectLayout {
        base,
        crate_key,
        package_dirs,
        top_files,
    })
}

/// Returns the user cache directory used for hot-patch mirrors and default build artifacts.
pub fn cache_root() -> PathBuf {
    if let Some(xdg) = env::var_os("XDG_CACHE_HOME").filter(|v| !v.is_empty()) {
        return PathBuf::from(xdg).join("rune-dev");
    }
    #[cfg(windows)]
    if let Some(local_app_data) = env::var_os("LOCALAPPDATA").filter(|v| !v.is_empty()) {
        return PathBuf::from(local_app_data).join("rune-dev");
    }
    if let Some(home) = crate::dx::home_dir() {
        return home.join(".cache").join("rune-dev");
    }
    env::temp_dir().join("rune-dev")
}

/// Computes a stable short hash directory name for `manifest_path`.
pub fn mirror_dir_for(manifest_path: &Path) -> PathBuf {
    let hash = blake3::hash(manifest_path.to_string_lossy().as_bytes());
    let short = &hash.to_hex()[..12];
    cache_root().join(format!("mirror-{short}"))
}

/// Default Cargo target directory for hot-patch builds of `manifest_path`.
pub fn default_target_dir_for(manifest_path: &Path) -> PathBuf {
    let hash = blake3::hash(manifest_path.to_string_lossy().as_bytes());
    let short = &hash.to_hex()[..12];
    cache_root().join(format!("build-{short}"))
}

/// Rewrites `[workspace].members` and `[workspace].default-members` in `ws_toml` so only staged
/// package directories (relative to the workspace root) are listed.
pub fn prune_workspace_manifest(ws_toml: &str, staged_dirs: &[String]) -> Result<String, CliError> {
    let mut doc = ws_toml
        .parse::<DocumentMut>()
        .map_err(|err| CliError::new(format!("invalid workspace Cargo.toml: {err}")))?;

    let Some(ws) = doc.get_mut("workspace").and_then(Item::as_table_like_mut) else {
        return Ok(ws_toml.to_string());
    };

    let excluded: BTreeSet<String> = ws
        .get("exclude")
        .and_then(Item::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(Value::as_str)
                .map(glob::normalize_rel)
                .collect()
        })
        .unwrap_or_default();

    for key in ["default-members", "members"] {
        let Some(patterns) = ws.get(key).and_then(Item::as_array).map(|arr| {
            arr.iter()
                .filter_map(Value::as_str)
                .map(glob::normalize_rel)
                .collect::<Vec<_>>()
        }) else {
            continue;
        };

        let mut kept: Vec<String> = staged_dirs
            .iter()
            .filter(|d| !excluded.contains(*d) && patterns.iter().any(|pat| glob::matches(d, pat)))
            .cloned()
            .collect();
        kept.sort();
        kept.dedup();

        let mut new_arr = Array::new();
        for item in kept {
            new_arr.push(item);
        }
        ws.insert(key, Item::Value(Value::Array(new_arr)));
    }

    Ok(doc.to_string())
}

/// Synchronizes `snapshot` into `mirror_dir`, pruning removed files from previous runs while
/// preserving build metadata and rewriting any staged workspace manifest's member list.
pub fn sync_mirror(
    mirror_dir: &Path,
    crate_key: &str,
    snapshot: &Snapshot,
) -> Result<(), CliError> {
    fs::create_dir_all(mirror_dir).map_err(|err| {
        CliError::new(format!(
            "failed to create mirror {}: {err}",
            mirror_dir.display()
        ))
    })?;

    let inputs_manifest = mirror_dir.join(".inputs.json");
    if let Ok(old_text) = fs::read_to_string(&inputs_manifest)
        && let Ok(old_keys) = serde_json::from_str::<Vec<String>>(&old_text)
    {
        for old_key in old_keys {
            if !snapshot.contains_key(&old_key) {
                let stale = join_posix(mirror_dir, &old_key);
                fs::remove_file(stale).ok();
            }
        }
    }

    let overrides = compute_workspace_overrides(crate_key, snapshot)?;
    for (key, data) in snapshot {
        let bytes = overrides.get(key).unwrap_or(data);
        let dest = join_posix(mirror_dir, key);
        if let Some(parent) = dest.parent() {
            fs::create_dir_all(parent).map_err(|err| {
                CliError::new(format!(
                    "failed to create {}: {err}",
                    parent.display()
                ))
            })?;
        }
        let unchanged = fs::read(&dest).is_ok_and(|existing| existing == *bytes);
        if !unchanged {
            atomic_write(&dest, bytes)?;
        }
    }

    let keys: Vec<&String> = snapshot.keys().collect();
    let json = serde_json::to_vec(&keys)
        .map_err(|err| CliError::new(format!("failed to serialize mirror index: {err}")))?;
    atomic_write(&inputs_manifest, &json)?;
    Ok(())
}

/// Finds the mirrored crate key owning `package`, falling back to `default_crate_key`.
pub fn resolve_package_crate_key(
    default_crate_key: &str,
    snapshot: &Snapshot,
    package: Option<&str>,
) -> String {
    let default_key = default_crate_key.trim_end_matches('/');
    let Some(target_pkg) = package else {
        return default_key.to_string();
    };
    if manifest_package_name(snapshot, &format!("{default_key}/Cargo.toml")).as_deref()
        == Some(target_pkg)
    {
        return default_key.to_string();
    }
    for key in snapshot.keys() {
        let Some(pkg_key) = key.strip_suffix("/Cargo.toml") else {
            continue;
        };
        if manifest_package_name(snapshot, key).as_deref() == Some(target_pkg) {
            return pkg_key.to_string();
        }
    }
    default_key.to_string()
}

/// Resolves the mirrored entry source file key(s) for the target binary in `crate_key`.
///
/// Honors explicit `[[bin]]` targets (with explicit or Cargo-inferred paths),
/// `[package].default-run`, `[package].autobins`, and Cargo's standard `src/main.rs`,
/// `src/bin/<name>.rs`, and `src/bin/<name>/main.rs` locations.
pub fn resolve_bin_entry_keys(
    crate_key: &str,
    snapshot: &Snapshot,
    bin: Option<&str>,
) -> Vec<String> {
    let prefix = crate_key.trim_end_matches('/');
    let manifest_key = format!("{prefix}/Cargo.toml");
    let doc = snapshot
        .get(&manifest_key)
        .and_then(|bytes| std::str::from_utf8(bytes).ok())
        .and_then(|text| text.parse::<DocumentMut>().ok());

    let pkg_table = doc
        .as_ref()
        .and_then(|d| d.get("package"))
        .and_then(Item::as_table_like);
    let pkg_name = pkg_table
        .and_then(|p| p.get("name"))
        .and_then(Item::as_str)
        .map(ToString::to_string);
    let default_run = pkg_table
        .and_then(|p| p.get("default-run"))
        .and_then(Item::as_str)
        .map(ToString::to_string);
    let autobins = pkg_table
        .and_then(|p| p.get("autobins"))
        .and_then(Item::as_bool)
        .unwrap_or(true);

    let mut candidates: Vec<(Option<String>, String)> = Vec::new();
    let mut seen_keys: BTreeSet<String> = BTreeSet::new();

    if let Some(bins) = doc
        .as_ref()
        .and_then(|d| d.get("bin"))
        .and_then(Item::as_array_of_tables)
    {
        for tbl in bins.iter() {
            let name = tbl
                .get("name")
                .and_then(Item::as_str)
                .map(ToString::to_string);
            let explicit_path = tbl.get("path").and_then(Item::as_str);
            let key = if let Some(rel) = explicit_path {
                let norm = glob::normalize_rel(rel);
                let full = format!("{prefix}/{norm}");
                snapshot.contains_key(&full).then_some(full)
            } else if let Some(ref bin_name) = name {
                infer_named_bin_key(prefix, snapshot, pkg_name.as_deref(), bin_name)
            } else {
                None
            };
            if let Some(entry_key) = key {
                seen_keys.insert(entry_key.clone());
                candidates.push((name, entry_key));
            }
        }
    }

    if autobins {
        let main_key = format!("{prefix}/src/main.rs");
        if snapshot.contains_key(&main_key) && seen_keys.insert(main_key.clone()) {
            candidates.push((pkg_name.clone(), main_key));
        }
        let bin_dir_prefix = format!("{prefix}/src/bin/");
        for key in snapshot.keys() {
            let Some(rest) = key.strip_prefix(&bin_dir_prefix) else {
                continue;
            };
            let inferred_name = if !rest.contains('/') {
                rest.strip_suffix(".rs").map(ToString::to_string)
            } else if let Some((dir, file)) = rest.split_once('/')
                && file == "main.rs"
                && !dir.contains('/')
            {
                Some(dir.to_string())
            } else {
                None
            };
            if let Some(bin_name) = inferred_name
                && seen_keys.insert(key.clone())
            {
                candidates.push((Some(bin_name), key.clone()));
            }
        }
    }

    let wanted = bin.or(default_run.as_deref());
    if let Some(target_name) = wanted {
        let mut matched: Vec<String> = candidates
            .iter()
            .filter(|(name, _)| name.as_deref() == Some(target_name))
            .map(|(_, key)| key.clone())
            .collect();
        matched.sort();
        matched.dedup();
        if !matched.is_empty() {
            return matched;
        }
    }

    if !candidates.is_empty() {
        let mut all: Vec<String> = candidates.into_iter().map(|(_, key)| key).collect();
        all.sort();
        all.dedup();
        return all;
    }

    let fallback = format!("{prefix}/src/main.rs");
    if snapshot.contains_key(&fallback) {
        vec![fallback]
    } else {
        Vec::new()
    }
}

/// Rewrites the mirrored binary entry file(s) with identical bytes and advances their mtime so
/// Cargo always recompiles and relinks the target binary on each `dx serve --hot-patch` launch.
///
/// Without a fresh link step, `dx` never intercepts `rustc` into `dx/.captured-args` and fails
/// with `Missing rustc capture` on a warm `CARGO_TARGET_DIR`. Dependencies stay cached.
pub fn touch_bin_entries(
    mirror_dir: &Path,
    crate_key: &str,
    snapshot: &Snapshot,
    package: Option<&str>,
    bin: Option<&str>,
) -> Result<Vec<PathBuf>, CliError> {
    let target_crate = resolve_package_crate_key(crate_key, snapshot, package);
    let keys = resolve_bin_entry_keys(&target_crate, snapshot, bin);
    let mut touched = Vec::with_capacity(keys.len());
    for key in keys {
        let dest = join_posix(mirror_dir, &key);
        if dest.is_file() {
            touch_mirror_file(&dest)?;
            touched.push(dest);
        }
    }
    Ok(touched)
}

fn touch_mirror_file(path: &Path) -> Result<(), CliError> {
    let old_mtime = fs::metadata(path).ok().and_then(|m| m.modified().ok());
    let bytes = fs::read(path)
        .map_err(|err| CliError::new(format!("failed to read {}: {err}", path.display())))?;
    atomic_write(path, &bytes)?;
    let new_mtime = fs::metadata(path).ok().and_then(|m| m.modified().ok());
    if let (Some(old), Some(new)) = (old_mtime, new_mtime)
        && new <= old
    {
        for bump in [Duration::from_millis(10), Duration::from_secs(1)] {
            if let Some(target) = old.checked_add(bump)
                && let Ok(file) = fs::OpenOptions::new().write(true).open(path)
                && file.set_modified(target).is_ok()
                && fs::metadata(path)
                    .ok()
                    .and_then(|m| m.modified().ok())
                    .is_some_and(|m| m > old)
            {
                break;
            }
        }
    }
    Ok(())
}

fn manifest_package_name(snapshot: &Snapshot, manifest_key: &str) -> Option<String> {
    let bytes = snapshot.get(manifest_key)?;
    let text = std::str::from_utf8(bytes).ok()?;
    let doc = text.parse::<DocumentMut>().ok()?;
    doc.get("package")
        .and_then(Item::as_table_like)
        .and_then(|p| p.get("name"))
        .and_then(Item::as_str)
        .map(ToString::to_string)
}

fn infer_named_bin_key(
    prefix: &str,
    snapshot: &Snapshot,
    pkg_name: Option<&str>,
    bin_name: &str,
) -> Option<String> {
    let mut candidates = Vec::with_capacity(3);
    if pkg_name == Some(bin_name) {
        candidates.push(format!("{prefix}/src/main.rs"));
    }
    candidates.push(format!("{prefix}/src/bin/{bin_name}.rs"));
    candidates.push(format!("{prefix}/src/bin/{bin_name}/main.rs"));
    candidates
        .into_iter()
        .find(|key| snapshot.contains_key(key))
}

/// Atomically writes `bytes` to `dest` via a sibling temporary file.
pub fn atomic_write(dest: &Path, bytes: &[u8]) -> Result<(), CliError> {
    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent).map_err(|err| {
            CliError::new(format!(
                "failed to create {}: {err}",
                parent.display()
            ))
        })?;
    }
    let tmp = dest.with_file_name(format!(
        ".{}.rune-tmp",
        dest.file_name().and_then(|s| s.to_str()).unwrap_or("file")
    ));
    fs::write(&tmp, bytes)
        .map_err(|err| CliError::new(format!("failed to write {}: {err}", tmp.display())))?;
    fs::rename(&tmp, dest).map_err(|err| {
        fs::remove_file(&tmp).ok();
        CliError::new(format!(
            "failed to replace {}: {err}",
            dest.display()
        ))
    })
}

fn compute_workspace_overrides(
    crate_key: &str,
    snapshot: &Snapshot,
) -> Result<BTreeMap<String, Vec<u8>>, CliError> {
    let mut overrides = BTreeMap::new();
    let Some(ws_key) = find_staged_workspace_key(crate_key, snapshot) else {
        return Ok(overrides);
    };
    let ws_prefix = ws_key.strip_suffix("/Cargo.toml").unwrap_or("");
    let mut staged = Vec::new();
    for (k, bytes) in snapshot {
        if k == &ws_key || !k.ends_with("/Cargo.toml") {
            continue;
        }
        let Some(rest) = k.strip_prefix(&format!("{ws_prefix}/")) else {
            continue;
        };
        let Some(pkg_rel) = rest.strip_suffix("/Cargo.toml") else {
            continue;
        };
        if let Ok(text) = std::str::from_utf8(bytes)
            && let Ok(doc) = text.parse::<DocumentMut>()
            && doc.get("package").is_some()
        {
            staged.push(glob::normalize_rel(pkg_rel));
        }
    }
    if let Some(ws_bytes) = snapshot.get(&ws_key)
        && let Ok(ws_text) = std::str::from_utf8(ws_bytes)
    {
        let pruned = prune_workspace_manifest(ws_text, &staged)?;
        overrides.insert(ws_key, pruned.into_bytes());
    }
    Ok(overrides)
}

fn find_staged_workspace_key(crate_key: &str, snapshot: &Snapshot) -> Option<String> {
    let mut cur = crate_key.to_string();
    loop {
        let key = format!("{cur}/Cargo.toml");
        if let Some(bytes) = snapshot.get(&key)
            && let Ok(text) = std::str::from_utf8(bytes)
            && let Ok(doc) = text.parse::<DocumentMut>()
            && doc.get("workspace").is_some()
        {
            return Some(key);
        }
        if cur == "external" || !cur.contains('/') {
            return None;
        }
        if let Some((parent, _)) = cur.rsplit_once('/') {
            cur = parent.to_string();
        } else {
            return None;
        }
    }
}

fn path_dependencies(doc: &DocumentMut, dir: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    collect_paths_from_table(doc.as_table(), dir, false, &mut found);
    found
}

fn collect_paths_from_table(
    tbl: &dyn TableLike,
    dir: &Path,
    inside: bool,
    out: &mut Vec<PathBuf>,
) {
    if inside
        && let Some(p) = tbl.get("path").and_then(Item::as_str)
        && let Ok(canon) = canonical_or_self(&dir.join(p))
    {
        out.push(canon);
    }
    for (key, item) in tbl.iter() {
        let child_inside = inside
            || key.ends_with("dependencies")
            || key == "patch"
            || key == "replace";
        if let Some(sub) = item.as_table_like() {
            collect_paths_from_table(sub, dir, child_inside, out);
        }
    }
}

fn common_ancestor(paths: &[PathBuf]) -> Option<PathBuf> {
    let first = paths.first()?;
    let mut common: Vec<Component<'_>> = first.components().collect();
    for path in &paths[1..] {
        let comps: Vec<Component<'_>> = path.components().collect();
        let mut matched = 0;
        while matched < common.len() && matched < comps.len() && common[matched] == comps[matched] {
            matched += 1;
        }
        common.truncate(matched);
        if common.is_empty() {
            return None;
        }
    }
    let mut result = PathBuf::new();
    for comp in common {
        result.push(comp.as_os_str());
    }
    Some(result)
}

pub(crate) fn join_posix(base: &Path, posix_rel: &str) -> PathBuf {
    let mut path = base.to_path_buf();
    for seg in posix_rel.split('/').filter(|s| !s.is_empty()) {
        path.push(seg);
    }
    path
}
