//! Git revision and upstream repository resolution shared by `build.rs` and `rune new`.
//!
//! Kept free of third-party crate dependencies so `build.rs` can include it directly without
//! duplicating URL normalization or git-remote verification logic.

use std::path::{Path, PathBuf};
use std::process::Command;

/// Result of resolving an upstream `anomalyco/rune` commit locally without network access.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct UpstreamRevLookup {
    /// Commit SHA guaranteed to exist on an upstream `anomalyco/rune` remote tracking ref, if any.
    pub rev: Option<String>,
    /// Existing git metadata files that `build.rs` should watch via `cargo:rerun-if-changed`.
    pub watch_paths: Vec<PathBuf>,
}

/// Returns `true` if `sha` looks like a Git hexadecimal commit object ID (7..=40 hex chars).
pub fn is_valid_git_sha(sha: &str) -> bool {
    (7..=40).contains(&sha.len()) && sha.bytes().all(|b| b.is_ascii_hexdigit())
}

/// Parses a remote Git URL into canonical lowercase `(host, path)` with trailing `/` and
/// optional `.git` stripped. Returns `None` for local paths and `file://` URLs.
pub fn canonical_git_repo(url: &str) -> Option<(String, String)> {
    let trimmed = url.trim();
    if trimmed.is_empty() {
        return None;
    }

    if let Some((scheme, rest)) = trimmed.split_once("://") {
        if scheme.is_empty()
            || !scheme
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'+' | b'-' | b'.'))
        {
            return None;
        }
        if scheme.eq_ignore_ascii_case("file") {
            return None;
        }
        let (authority, path) = rest.split_once('/')?;
        let host_port = authority.rsplit_once('@').map_or(authority, |(_, h)| h);
        let host = host_port
            .split_once(':')
            .map_or(host_port, |(h, _)| h)
            .trim();
        let norm_path = strip_repo_path(path);
        if host.is_empty() || norm_path.is_empty() {
            return None;
        }
        return Some((host.to_ascii_lowercase(), norm_path.to_ascii_lowercase()));
    }

    // SCP-like syntax: `[user@]host:owner/repo[.git]` (skip single-letter Windows drive prefixes).
    let (host_part, path) = trimmed.split_once(':')?;
    if host_part.len() <= 1 || host_part.contains('/') || host_part.contains('\\') {
        return None;
    }
    let host = host_part.rsplit_once('@').map_or(host_part, |(_, h)| h).trim();
    let norm_path = strip_repo_path(path);
    if host.is_empty() || norm_path.is_empty() {
        return None;
    }
    Some((host.to_ascii_lowercase(), norm_path.to_ascii_lowercase()))
}

fn strip_repo_path(path: &str) -> &str {
    let mut s = path.trim().trim_matches('/');
    if s.len() >= 4 && s[s.len() - 4..].eq_ignore_ascii_case(".git") {
        s = &s[..s.len() - 4];
    }
    s.trim_matches('/')
}

/// Returns `true` when `url` points directly to the upstream `anomalyco/rune` GitHub repository.
pub fn is_upstream_rune_url(url: &str) -> bool {
    canonical_git_repo(url)
        .is_some_and(|(host, path)| host == "github.com" && path == "anomalyco/rune")
}

/// Returns `true` when `a` and `b` identify the same Git repository (ignoring protocol,
/// credentials, trailing slash, and `.git` suffix).
pub fn same_repo_url(a: &str, b: &str) -> bool {
    match (canonical_git_repo(a), canonical_git_repo(b)) {
        (Some(ca), Some(cb)) => ca == cb,
        _ => strip_repo_path(a).eq_ignore_ascii_case(strip_repo_path(b)),
    }
}

/// Reads the current `HEAD` commit SHA of the git repository containing `dir`.
pub fn git_head_rev(dir: &Path) -> Option<String> {
    git_stdout(dir, &["rev-parse", "HEAD"]).filter(|sha| is_valid_git_sha(sha))
}

/// Resolves an upstream `anomalyco/rune` commit purely from local git state (zero network I/O).
///
/// Finds a local remote whose URL points to `github.com/anomalyco/rune` (preferring `upstream`,
/// then `origin`) and computes `merge-base(HEAD, refs/remotes/<remote>/dev)` (falling back to
/// `refs/remotes/<remote>/HEAD` or the tracking ref itself). This ensures:
/// - Clean or dirty checkouts on `origin/dev` resolve to `HEAD`.
/// - Local feature branches or forks with an `upstream` remote resolve to their upstream base
///   commit rather than an unpushed local or fork-only SHA.
/// - Forks without an `anomalyco/rune` remote, Cargo git-db checkouts, and source tarballs return
///   `None` so `rune new` never writes an unreachable fork SHA.
pub fn upstream_dev_rev(dir: &Path) -> UpstreamRevLookup {
    let mut watch_paths = Vec::new();
    push_git_watch_path(dir, "HEAD", &mut watch_paths);
    if let Some(symref) = git_stdout(dir, &["symbolic-ref", "-q", "HEAD"]) {
        push_git_watch_path(dir, &symref, &mut watch_paths);
    }
    push_git_watch_path(dir, "config", &mut watch_paths);
    push_git_watch_path(dir, "packed-refs", &mut watch_paths);

    let Some(remotes_raw) = git_stdout(dir, &["remote"]) else {
        return UpstreamRevLookup {
            rev: None,
            watch_paths,
        };
    };

    let mut matching: Vec<String> = remotes_raw
        .lines()
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .filter(|name| {
            git_stdout(dir, &["remote", "get-url", name])
                .is_some_and(|url| is_upstream_rune_url(&url))
        })
        .map(ToString::to_string)
        .collect();

    matching.sort_by_key(|name| match name.as_str() {
        "upstream" => 0u8,
        "origin" => 1u8,
        _ => 2u8,
    });

    for remote in &matching {
        for suffix in ["dev", "HEAD"] {
            let ref_name = format!("refs/remotes/{remote}/{suffix}");
            push_git_watch_path(dir, &ref_name, &mut watch_paths);

            let Some(ref_sha) = git_stdout(dir, &["rev-parse", "--verify", "--quiet", &ref_name])
                .filter(|sha| is_valid_git_sha(sha))
            else {
                continue;
            };

            let rev = git_stdout(dir, &["merge-base", "HEAD", &ref_name])
                .filter(|sha| is_valid_git_sha(sha))
                .unwrap_or(ref_sha);

            return UpstreamRevLookup {
                rev: Some(rev),
                watch_paths,
            };
        }
    }

    UpstreamRevLookup {
        rev: None,
        watch_paths,
    }
}

/// Extracts the commit SHA for `refs/heads/<branch>` from `git ls-remote` stdout.
pub fn parse_ls_remote_rev(stdout: &str, branch: &str) -> Option<String> {
    let target_ref = format!("refs/heads/{branch}");
    for line in stdout.lines() {
        let mut parts = line.split_whitespace();
        let Some(sha) = parts.next() else {
            continue;
        };
        let Some(ref_name) = parts.next() else {
            continue;
        };
        if ref_name == target_ref && is_valid_git_sha(sha) {
            return Some(sha.to_string());
        }
    }
    None
}

/// Queries `<url>`'s `dev` branch tip via `git ls-remote` without prompting for credentials.
pub fn resolve_remote_dev_rev(url: &str) -> Option<String> {
    let out = Command::new("git")
        .args([
            "-c",
            "http.lowSpeedLimit=1000",
            "-c",
            "http.lowSpeedTime=10",
            "ls-remote",
            "--heads",
            url,
            "dev",
        ])
        .env("GIT_TERMINAL_PROMPT", "0")
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let stdout = String::from_utf8_lossy(&out.stdout);
    parse_ls_remote_rev(&stdout, "dev")
}

/// Resolves the git `rev` to write into a newly scaffolded `Cargo.toml`.
///
/// Precedence:
/// 1. Explicit `--rev` passed by the user.
/// 2. Compile-time verified upstream `default_rev` when targeting `default_repo_url`.
/// 3. Live `refs/heads/dev` lookup on `target_url` via `lookup_remote_dev`.
/// 4. `None` (renders `branch = "dev"`) when offline or unreachable.
pub fn resolve_git_rev<F>(
    explicit_rev: Option<&str>,
    target_url: &str,
    default_repo_url: &str,
    default_rev: &str,
    lookup_remote_dev: F,
) -> Option<String>
where
    F: FnOnce(&str) -> Option<String>,
{
    if let Some(rev) = explicit_rev.map(str::trim).filter(|s| !s.is_empty()) {
        return Some(rev.to_string());
    }
    if same_repo_url(target_url, default_repo_url) && !default_rev.is_empty() {
        return Some(default_rev.to_string());
    }
    lookup_remote_dev(target_url)
}

fn push_git_watch_path(dir: &Path, git_rel: &str, out: &mut Vec<PathBuf>) {
    let Some(raw) = git_stdout(dir, &["rev-parse", "--git-path", git_rel]) else {
        return;
    };
    let path = PathBuf::from(&raw);
    let resolved = if path.is_absolute() {
        path
    } else {
        dir.join(path)
    };
    if resolved.is_file() && !out.contains(&resolved) {
        out.push(resolved);
    }
}

fn git_stdout(dir: &Path, args: &[&str]) -> Option<String> {
    let out = Command::new("git")
        .args(args)
        .current_dir(dir)
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if text.is_empty() { None } else { Some(text) }
}
