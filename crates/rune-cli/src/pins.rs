//! Verification of Rune git URLs, revisions, `[patch.crates-io]` entries, and `Cargo.lock`.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;
use std::process::Command;

use toml_edit::{DocumentMut, Item, TableLike};

use crate::args::CheckArgs;
use crate::config::{find_manifest, find_workspace_root, parse_toml};
use crate::{CliError, default_patches};

const DEP_TABLES: &[&str] = &["dependencies", "dev-dependencies", "build-dependencies"];

#[derive(Debug, Clone)]
struct DepEntry {
    where_key: String,
    crate_name: String,
    git_url: Option<String>,
    rev: Option<String>,
    is_path: bool,
}

fn norm_url(url: &str) -> String {
    let trimmed = url.trim().trim_end_matches('/');
    trimmed
        .strip_suffix(".git")
        .unwrap_or(trimmed)
        .to_string()
}

fn is_rune_crate(name: &str) -> bool {
    name == "rune" || name == "rune-ui" || name.starts_with("rune-") || name.starts_with("rune_")
}

/// Extracts the required `[patch.crates-io]` crate names from a Rune root `Cargo.toml`.
pub fn required_patches_from_manifest(rune_manifest_text: &str) -> Result<Vec<String>, String> {
    let doc = rune_manifest_text
        .parse::<DocumentMut>()
        .map_err(|err| format!("invalid Rune manifest TOML: {err}"))?;
    let mut out = Vec::new();
    if let Some(table) = doc
        .get("patch")
        .and_then(Item::as_table_like)
        .and_then(|p| p.get("crates-io"))
        .and_then(Item::as_table_like)
    {
        for (key, _) in table.iter() {
            out.push(key.to_string());
        }
    }
    Ok(out)
}

/// Validates Rune dependencies and `[patch.crates-io]` pins in `app_manifest_text`, plus optional
/// `Cargo.lock` consistency and `wasm-bindgen` CLI alignment.
pub fn check_pins(
    app_manifest_text: &str,
    required_patches: &[String],
    lock_text: Option<&str>,
    wasm_bindgen_cli: Option<&str>,
) -> Result<Vec<String>, String> {
    let doc = app_manifest_text
        .parse::<DocumentMut>()
        .map_err(|err| format!("invalid app manifest TOML: {err}"))?;

    let wanted: BTreeSet<String> = required_patches.iter().cloned().collect();
    let entries = collect_dep_entries(&doc);

    let git_urls: BTreeSet<String> = entries
        .iter()
        .filter(|e| is_rune_crate(&e.crate_name))
        .filter_map(|e| e.git_url.clone())
        .collect();

    let git_rune_lines: Vec<&DepEntry> = entries
        .iter()
        .filter(|e| e.git_url.is_some())
        .filter(|e| {
            is_rune_crate(&e.crate_name)
                || e.git_url.as_ref().is_some_and(|u| git_urls.contains(u))
                || (e.where_key == "patch.crates-io" && wanted.contains(&e.crate_name))
        })
        .collect();

    let path_rune_lines: Vec<&DepEntry> = entries
        .iter()
        .filter(|e| e.is_path && is_rune_crate(&e.crate_name))
        .collect();

    if git_rune_lines.is_empty() {
        if !path_rune_lines.is_empty() {
            let patched: BTreeSet<String> = entries
                .iter()
                .filter(|e| e.where_key == "patch.crates-io")
                .map(|e| e.crate_name.clone())
                .collect();
            let mut problems = Vec::new();
            for name in wanted.difference(&patched) {
                problems.push(format!(
                    "patch.crates-io.{name}: missing; Rune patches it at this rev"
                ));
            }
            return Ok(problems);
        }
        return Ok(vec!["no Rune dependency found in the manifest".to_string()]);
    }

    let mut problems = Vec::new();
    for line in &git_rune_lines {
        if line.rev.is_none() {
            problems.push(format!(
                "{}.{}: no `rev`; pin a `dev` rev",
                line.where_key, line.crate_name
            ));
        }
    }

    let pins: BTreeSet<(String, Option<String>)> = git_rune_lines
        .iter()
        .map(|l| (l.git_url.clone().unwrap_or_default(), l.rev.clone()))
        .collect();

    if pins.len() > 1 {
        problems.push(
            "Rune git lines differ; use one `git` URL and one `rev` on all of them:".to_string(),
        );
        for line in &git_rune_lines {
            let url = line.git_url.as_deref().unwrap_or("<none>");
            let rev = line.rev.as_deref().unwrap_or("<none>");
            problems.push(format!(
                "  {}.{}: git = {url} rev = {rev}",
                line.where_key, line.crate_name
            ));
        }
    }

    let mut patched: BTreeSet<String> = git_rune_lines
        .iter()
        .filter(|l| l.where_key == "patch.crates-io")
        .map(|l| l.crate_name.clone())
        .collect();

    let all_rune_urls: BTreeSet<String> = git_rune_lines
        .iter()
        .filter_map(|l| l.git_url.clone())
        .collect();

    if let Some(patch_table) = doc
        .get("patch")
        .and_then(Item::as_table_like)
        .and_then(|p| p.get("crates-io"))
        .and_then(Item::as_table_like)
    {
        for (key, item) in patch_table.iter() {
            if let Some(tbl) = item.as_table_like()
                && let Some(git) = tbl.get("git").and_then(Item::as_str)
                && all_rune_urls.contains(&norm_url(git))
            {
                patched.insert(key.to_string());
            }
        }
    }

    for name in wanted.difference(&patched) {
        problems.push(format!(
            "patch.crates-io.{name}: missing; Rune patches it at this rev"
        ));
    }
    for name in patched.difference(&wanted) {
        problems.push(format!(
            "patch.crates-io.{name}: Rune no longer patches it at this rev; remove it"
        ));
    }

    if let Some(lock_str) = lock_text {
        let revs: BTreeSet<String> = pins.iter().filter_map(|(_, r)| r.clone()).collect();
        problems.extend(check_lock(lock_str, &all_rune_urls, &revs, wasm_bindgen_cli)?);
    }

    Ok(problems)
}

fn check_lock(
    lock_text: &str,
    urls: &BTreeSet<String>,
    revs: &BTreeSet<String>,
    wasm_bindgen_cli: Option<&str>,
) -> Result<Vec<String>, String> {
    let doc = lock_text
        .parse::<DocumentMut>()
        .map_err(|err| format!("invalid Cargo.lock TOML: {err}"))?;
    let mut problems = Vec::new();
    let mut commits: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut wasm_versions: BTreeSet<String> = BTreeSet::new();

    if let Some(packages) = doc.get("package").and_then(Item::as_array_of_tables) {
        for pkg in packages.iter() {
            let name = pkg.get("name").and_then(Item::as_str).unwrap_or("");
            if name == "wasm-bindgen"
                && let Some(ver) = pkg.get("version").and_then(Item::as_str)
            {
                wasm_versions.insert(ver.to_string());
            }
            let source = pkg.get("source").and_then(Item::as_str).unwrap_or("");
            if let Some(rest) = source.strip_prefix("git+")
                && let Some((url_part, commit)) = rest.split_once('#')
            {
                let base_url = url_part.split('?').next().unwrap_or(url_part);
                if urls.contains(&norm_url(base_url)) {
                    commits
                        .entry(commit.to_string())
                        .or_default()
                        .push(name.to_string());
                }
            }
        }
    }

    if commits.is_empty() {
        problems.push(
            "Cargo.lock: no package from the Rune git URL; run `cargo update -p rune`".to_string(),
        );
    } else if commits.len() > 1 {
        problems.push(
            "Cargo.lock: Rune's crates resolve to more than one commit (two copies of Rune):"
                .to_string(),
        );
        for (commit, mut names) in commits.clone() {
            names.sort();
            problems.push(format!("  {commit}: {}", names.join(", ")));
        }
    }

    if !revs.is_empty() {
        for commit in commits.keys() {
            if !revs.iter().any(|rev| commit.starts_with(rev.as_str())) {
                problems.push(format!(
                    "Cargo.lock: Rune is locked at {commit}, not the manifest's rev; run `cargo update -p rune`"
                ));
            }
        }
    }

    if let Some(expected_cli) = wasm_bindgen_cli {
        if wasm_versions.is_empty() {
            problems.push(
                "Cargo.lock: no wasm-bindgen package to compare with --wasm-bindgen-cli".to_string(),
            );
        } else {
            let locked: Vec<String> = wasm_versions.into_iter().collect();
            if locked != [expected_cli] {
                problems.push(format!(
                    "wasm-bindgen: Cargo.lock has {}, the CLI pin is {expected_cli}; make them match",
                    locked.join(", ")
                ));
            }
        }
    }

    Ok(problems)
}

fn collect_dep_entries(doc: &DocumentMut) -> Vec<DepEntry> {
    let mut entries = Vec::new();
    for table_name in DEP_TABLES {
        if let Some(tbl) = doc.get(table_name).and_then(Item::as_table_like) {
            push_table_entries(&mut entries, table_name, tbl);
        }
    }
    if let Some(ws_deps) = doc
        .get("workspace")
        .and_then(Item::as_table_like)
        .and_then(|w| w.get("dependencies"))
        .and_then(Item::as_table_like)
    {
        push_table_entries(&mut entries, "workspace.dependencies", ws_deps);
    }
    if let Some(targets) = doc.get("target").and_then(Item::as_table_like) {
        for (cfg, item) in targets.iter() {
            if let Some(target_tbl) = item.as_table_like() {
                for dep_table in DEP_TABLES {
                    if let Some(tbl) = target_tbl.get(dep_table).and_then(Item::as_table_like) {
                        push_table_entries(&mut entries, &format!("target.{cfg}.{dep_table}"), tbl);
                    }
                }
            }
        }
    }
    if let Some(patches) = doc.get("patch").and_then(Item::as_table_like) {
        for (source, item) in patches.iter() {
            if let Some(tbl) = item.as_table_like() {
                push_table_entries(&mut entries, &format!("patch.{source}"), tbl);
            }
        }
    }
    entries
}

fn push_table_entries(entries: &mut Vec<DepEntry>, where_key: &str, tbl: &dyn TableLike) {
    for (key, item) in tbl.iter() {
        let Some(spec) = item.as_table_like() else {
            continue;
        };
        let crate_name = spec
            .get("package")
            .and_then(Item::as_str)
            .unwrap_or(key)
            .to_string();
        let git_url = spec.get("git").and_then(Item::as_str).map(norm_url);
        let rev = spec.get("rev").and_then(Item::as_str).map(ToString::to_string);
        let is_path = spec.get("path").and_then(Item::as_str).is_some();
        if git_url.is_some() || is_path {
            entries.push(DepEntry {
                where_key: where_key.to_string(),
                crate_name,
                git_url,
                rev,
                is_path,
            });
        }
    }
}

/// Resolves the required patch list from `--rune-manifest` or the CLI's built-in list.
pub fn resolve_required_patches(rune_manifest: Option<&Path>) -> Result<Vec<String>, CliError> {
    match rune_manifest {
        Some(path) => {
            let text = fs::read_to_string(path).map_err(|err| {
                CliError::new(format!("failed to read {}: {err}", path.display()))
            })?;
            required_patches_from_manifest(&text).map_err(CliError::new)
        }
        None => Ok(default_patches()),
    }
}

/// Runs `rune check`.
pub fn run_check(opts: &CheckArgs) -> Result<(), CliError> {
    let cwd = std::env::current_dir()
        .map_err(|err| CliError::new(format!("cannot read current directory: {err}")))?;
    let manifest_path = find_manifest(opts.manifest_path.as_deref(), &cwd)?;
    let crate_dir = manifest_path
        .parent()
        .ok_or_else(|| CliError::new("manifest path has no parent directory"))?;
    let ws_root = find_workspace_root(crate_dir)?;
    let ws_manifest = ws_root.join("Cargo.toml");

    let ws_text = fs::read_to_string(&ws_manifest).map_err(|err| {
        CliError::new(format!("failed to read {}: {err}", ws_manifest.display()))
    })?;
    let crate_text = if ws_manifest == manifest_path {
        ws_text.clone()
    } else {
        fs::read_to_string(&manifest_path).map_err(|err| {
            CliError::new(format!("failed to read {}: {err}", manifest_path.display()))
        })?
    };

    let required = resolve_required_patches(opts.rune_manifest.as_deref())?;
    let lock_path = opts
        .lock_path
        .clone()
        .unwrap_or_else(|| ws_root.join("Cargo.lock"));
    let lock_text = if lock_path.is_file() {
        Some(fs::read_to_string(&lock_path).map_err(|err| {
            CliError::new(format!("failed to read {}: {err}", lock_path.display()))
        })?)
    } else {
        None
    };

    let check_text = combine_workspace_and_crate_if_needed(
        &ws_manifest,
        &ws_text,
        &manifest_path,
        &crate_text,
    )?;
    let problems = check_pins(
        &check_text,
        &required,
        lock_text.as_deref(),
        opts.wasm_bindgen_cli.as_deref(),
    )
    .map_err(CliError::new)?;

    if !problems.is_empty() {
        for problem in &problems {
            eprintln!("rune check: {problem}");
        }
        return Err(CliError::new(format!(
            "found {} Rune pin issue(s)",
            problems.len()
        )));
    }
    println!("rune check: Rune pins agree");

    if opts.pins_only {
        return Ok(());
    }

    let mut cmd = Command::new("cargo");
    cmd.arg("check")
        .arg("--manifest-path")
        .arg(&manifest_path)
        .args(&opts.cargo_args);
    let status = cmd
        .status()
        .map_err(|err| CliError::new(format!("failed to invoke cargo check: {err}")))?;
    if !status.success() {
        return Err(CliError::with_code("", status.code().unwrap_or(1)));
    }
    Ok(())
}

/// If the app sits inside a workspace whose root holds `[patch.crates-io]` while the member crate
/// declares the `rune` dependency, inspect both together by copying the member's `[dependencies]`
/// into an in-memory copy of the workspace manifest.
pub(crate) fn combine_workspace_and_crate_if_needed(
    ws_manifest: &Path,
    ws_text: &str,
    crate_manifest: &Path,
    crate_text: &str,
) -> Result<String, CliError> {
    if ws_manifest == crate_manifest {
        return Ok(ws_text.to_string());
    }
    let mut ws_doc = parse_toml(ws_text, ws_manifest)?;
    let crate_doc = parse_toml(crate_text, crate_manifest)?;
    if ws_doc.get("dependencies").is_none()
        && let Some(deps) = crate_doc.get("dependencies")
    {
        ws_doc.insert("dependencies", deps.clone());
    }
    Ok(ws_doc.to_string())
}
