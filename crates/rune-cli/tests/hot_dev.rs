use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use rune_cli::args::DevArgs;
use rune_cli::config::resolve_dev_config;
use rune_cli::dev::fingerprint::{
    clear_manifest_fingerprints, is_manifest_change, should_bust_crate,
};
use rune_cli::dev::hot::{build_dx_args, shell_join, shell_quote_arg};
use rune_cli::dev::mirror::{
    default_target_dir_for, discover_layout, sync_generated_lockfile, sync_mirror,
};
use rune_cli::dev::proc::ChildGuard;
use rune_cli::dx::{accepts_version, is_any_dioxus_version, parse_unpinned_flag};
use rune_cli::watch::{FileCache, scan_sources};

static TEMP_SEQ: AtomicU64 = AtomicU64::new(0);

struct TempDir(PathBuf);

impl TempDir {
    #[allow(clippy::expect_used)]
    fn new(tag: &str) -> Self {
        let id = TEMP_SEQ.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir()
            .join(format!("rune-cli-hot-test-{tag}-{}-{id}", std::process::id()));
        fs::remove_dir_all(&dir).ok();
        fs::create_dir_all(&dir).expect("create temp dir");
        Self(dir)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).ok();
    }
}

/// Minimal POSIX shell word splitter matching shell_words::split semantics used by dx for
/// --cargo-args.
fn posix_split(input: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut cur = String::new();
    let mut in_word = false;
    let mut chars = input.chars().peekable();
    while let Some(ch) = chars.next() {
        match ch {
            ' ' | '\t' | '\n' if !in_word => {}
            ' ' | '\t' | '\n' => {
                words.push(std::mem::take(&mut cur));
                in_word = false;
            }
            '\'' => {
                in_word = true;
                for c in chars.by_ref() {
                    if c == '\'' {
                        break;
                    }
                    cur.push(c);
                }
            }
            '"' => {
                in_word = true;
                while let Some(c) = chars.next() {
                    if c == '"' {
                        break;
                    }
                    if c == '\\'
                        && let Some(&next) = chars.peek()
                        && matches!(next, '"' | '\\' | '$' | '\x60')
                    {
                        cur.push(chars.next().unwrap_or(next));
                    } else {
                        cur.push(c);
                    }
                }
            }
            '\\' => {
                in_word = true;
                if let Some(next) = chars.next() {
                    cur.push(next);
                }
            }
            other => {
                in_word = true;
                cur.push(other);
            }
        }
    }
    if in_word {
        words.push(cur);
    }
    words
}

#[test]
fn cargo_args_are_shell_quoted_and_accepted_by_real_cargo() {
    let tmp = TempDir::new("cargo-args");
    let app = tmp.path().join("app");
    fs::create_dir_all(app.join("src")).unwrap();
    fs::write(
        app.join("Cargo.toml"),
        r#"[package]
name = "quote-probe"
version = "0.1.0"
edition = "2024"

[features]
hot = []
"#,
    )
    .unwrap();
    fs::write(app.join("src/main.rs"), "fn main() {}").unwrap();

    let cfg = resolve_dev_config(
        &DevArgs {
            manifest_path: Some(app.join("Cargo.toml")),
            package: None,
            bin: None,
            features: Vec::new(),
            patch: Vec::new(),
            port: None,
            mode: None,
            jobs: Some(2),
            target_dir: None,
            app_args: Vec::new(),
        },
        &app,
    )
    .unwrap();

    let dx_args = build_dx_args(&cfg);
    let cargo_args_flag = dx_args
        .iter()
        .find_map(|a| a.strip_prefix("--cargo-args="))
        .expect("--cargo-args flag must be emitted when [profile.rune-hot] is absent");

    let split = posix_split(cargo_args_flag);
    assert!(
        split.contains(&r#"profile.rune-hot.inherits="dev""#.to_string()),
        "POSIX shell split must preserve TOML quotes around \"dev\"; raw={cargo_args_flag}, split={split:?}"
    );

    // Feed the exact shell-split flags to real cargo metadata to prove Cargo accepts them.
    let output = Command::new("cargo")
        .arg("metadata")
        .arg("--no-deps")
        .arg("--format-version=1")
        .arg("--manifest-path")
        .arg(app.join("Cargo.toml"))
        .args(&split)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "cargo rejected parsed --cargo-args ({split:?}): {}",
        String::from_utf8_lossy(&output.stderr)
    );

    // Direct shell_quote_arg / shell_join edge cases.
    assert_eq!(shell_quote_arg(""), "''");
    assert_eq!(shell_quote_arg("--config"), "--config");
    assert_eq!(
        shell_quote_arg(r#"profile.rune-hot.inherits="dev""#),
        r#"'profile.rune-hot.inherits="dev"'"#
    );
    let tricky = vec![
        "--flag".to_string(),
        "a b".to_string(),
        "it's".to_string(),
        "".to_string(),
    ];
    assert_eq!(posix_split(&shell_join(&tricky)), tricky);
}

#[test]
fn dx_pin_policy_supports_explicit_rune_dx_and_unpinned_while_rejecting_deno() {
    assert!(is_any_dioxus_version("dioxus 0.7.10"));
    assert!(is_any_dioxus_version("dioxus-cli 0.8.0-alpha.1"));
    assert!(!is_any_dioxus_version("deno 2.1.0"));

    // Auto-detected + pinned: exact 0.7.10 only (including 0.7.10-local).
    assert!(accepts_version("dioxus 0.7.10", false, false));
    assert!(accepts_version("dioxus 0.7.10-local (abc123)", false, false));
    assert!(!accepts_version("dioxus 0.7.11", false, false));
    assert!(!accepts_version("dioxus 0.8.0", false, false));
    assert!(!accepts_version("deno 2.1.0", false, false));

    // Auto-detected + RUNE_DX_UNPINNED=1: any Dioxus version, still skips Deno.
    assert!(accepts_version("dioxus 0.7.11", false, true));
    assert!(accepts_version("dioxus 0.8.0-alpha.0", false, true));
    assert!(!accepts_version("deno 2.1.0", false, true));

    // Explicit RUNE_DX without RUNE_DX_UNPINNED: any Dioxus version, still rejects Deno.
    assert!(accepts_version("dioxus 0.7.10", true, false));
    assert!(accepts_version("dioxus 0.8.0", true, false));
    assert!(!accepts_version("deno 2.1.0", true, false));

    // Explicit RUNE_DX + RUNE_DX_UNPINNED=1: accepts custom version output if executable ran.
    assert!(accepts_version("custom-dx-fork 1.0", true, true));
    assert!(!accepts_version("failed to execute: not found", true, true));

    assert!(parse_unpinned_flag("1"));
    assert!(parse_unpinned_flag("true"));
    assert!(parse_unpinned_flag("YES"));
    assert!(parse_unpinned_flag(" on "));
    assert!(!parse_unpinned_flag("0"));
    assert!(!parse_unpinned_flag("false"));
    assert!(!parse_unpinned_flag(""));
}

#[test]
fn default_target_dir_uses_workspace_target_for_standalone_and_member_crates() {
    let tmp = TempDir::new("target-dir");

    // Standalone crate -> <app>/target
    let standalone = tmp.path().join("standalone");
    fs::create_dir_all(&standalone).unwrap();
    let standalone_manifest = standalone.join("Cargo.toml");
    fs::write(
        &standalone_manifest,
        "[package]\nname = \"solo\"\nversion = \"0.1.0\"\n",
    )
    .unwrap();
    assert_eq!(
        default_target_dir_for(&standalone_manifest),
        fs::canonicalize(&standalone)
            .map(strip_verbatim)
            .unwrap_or(standalone)
            .join("target")
    );

    // Workspace member crate -> <workspace>/target
    let ws = tmp.path().join("ws");
    let member = ws.join("apps/demo");
    fs::create_dir_all(&member).unwrap();
    fs::write(
        ws.join("Cargo.toml"),
        "[workspace]\nmembers = [\"apps/demo\"]\n",
    )
    .unwrap();
    let member_manifest = member.join("Cargo.toml");
    fs::write(
        &member_manifest,
        "[package]\nname = \"demo\"\nversion = \"0.1.0\"\n",
    )
    .unwrap();
    assert_eq!(
        default_target_dir_for(&member_manifest),
        fs::canonicalize(&ws)
            .map(strip_verbatim)
            .unwrap_or(ws)
            .join("target")
    );
}

#[test]
fn manifest_fingerprint_clearing_handles_both_cargo_layouts_and_preserves_third_party() {
    assert!(is_manifest_change(&["external/app/Cargo.toml".to_string()]));
    assert!(is_manifest_change(&["external/Cargo.lock".to_string()]));
    assert!(is_manifest_change(&[
        "external/.cargo/config.toml".to_string()
    ]));
    assert!(!is_manifest_change(&["external/app/src/view.rs".to_string()]));
    assert!(!is_manifest_change(&["external/app/rune.toml".to_string()]));

    assert!(should_bust_crate("my-app", "my-app"));
    assert!(should_bust_crate("my_app", "my-app"));
    assert!(should_bust_crate("rune", "my-app"));
    assert!(should_bust_crate("rune-text", "my-app"));
    assert!(should_bust_crate("rune_core", "my-app"));
    assert!(!should_bust_crate("wgpu", "my-app"));
    assert!(!should_bust_crate("syn", "my-app"));

    let tmp = TempDir::new("fingerprints");
    let target = tmp.path().join("target");
    let legacy = target.join("rune-hot/.fingerprint");
    let build_dir = target.join("rune-hot/build");

    // Legacy layout units
    let app_fp = legacy.join("my-app-0123456789abcdef");
    let rune_fp = legacy.join("rune-text-abcdef0123456789");
    let build_script_fp = legacy.join("rune-1111222233334444");
    let third_party_fp = legacy.join("wgpu-9999888877776666");
    for d in [&app_fp, &rune_fp, &build_script_fp, &third_party_fp] {
        fs::create_dir_all(d).unwrap();
    }
    fs::write(app_fp.join("bin-my-app"), b"x").unwrap();
    fs::write(rune_fp.join("lib-rune_text"), b"x").unwrap();
    fs::write(build_script_fp.join("run-build-script-build"), b"x").unwrap();
    fs::write(build_script_fp.join("invoked.timestamp"), b"x").unwrap();
    fs::write(third_party_fp.join("lib-wgpu"), b"x").unwrap();

    // New Cargo build/<pkg>/<hash>/fingerprint layout units
    let new_app_fp = build_dir.join("my-app/aaaa1111/fingerprint");
    let new_rune_fp = build_dir.join("rune-core/bbbb2222/fingerprint");
    let new_rune_script_unit = build_dir.join("rune/cccc3333");
    let new_third_party_fp = build_dir.join("syn/dddd4444/fingerprint");
    fs::create_dir_all(&new_app_fp).unwrap();
    fs::create_dir_all(&new_rune_fp).unwrap();
    fs::create_dir_all(new_rune_script_unit.join("fingerprint")).unwrap();
    fs::create_dir_all(new_rune_script_unit.join("run")).unwrap();
    fs::create_dir_all(&new_third_party_fp).unwrap();

    let removed = clear_manifest_fingerprints(&target, "rune-hot", "my-app");
    assert_eq!(removed, 4);

    assert!(!app_fp.exists(), "app legacy fingerprint must be cleared");
    assert!(!rune_fp.exists(), "rune-text legacy fingerprint must be cleared");
    assert!(
        build_script_fp.exists(),
        "legacy build-script unit must stay cached"
    );
    assert!(
        third_party_fp.exists(),
        "third-party legacy fingerprint must stay cached"
    );

    assert!(!new_app_fp.exists(), "app build-dir fingerprint must be cleared");
    assert!(
        !new_rune_fp.exists(),
        "rune-core build-dir fingerprint must be cleared"
    );
    assert!(
        new_rune_script_unit.join("fingerprint").exists(),
        "build-dir unit with sibling run/ must stay cached"
    );
    assert!(
        new_third_party_fp.exists(),
        "third-party build-dir fingerprint must stay cached"
    );
}

#[test]
fn sync_generated_lockfile_persists_standalone_lock_and_skips_pruned_workspace() {
    let tmp = TempDir::new("lockfile");

    // 1. Standalone crate without Cargo.lock -> copied back from mirror.
    let app = tmp.path().join("app");
    fs::create_dir_all(app.join("src")).unwrap();
    fs::write(
        app.join("Cargo.toml"),
        "[package]\nname = \"solo\"\nversion = \"0.1.0\"\n",
    )
    .unwrap();
    fs::write(app.join("src/main.rs"), "fn main() {}").unwrap();

    let layout = discover_layout(&app.join("Cargo.toml")).unwrap();
    let mut cache = FileCache::default();
    let snap = scan_sources(&mut cache, &layout.source_entries()).unwrap();
    let mirror = tmp.path().join("mirror");
    sync_mirror(&mirror, &layout.crate_key, &snap).unwrap();

    let generated = b"# generated lock\nversion = 4\n";
    fs::write(mirror.join("external/Cargo.lock"), generated).unwrap();

    let ws_root = fs::canonicalize(&app).map(strip_verbatim).unwrap_or(app.clone());
    assert!(sync_generated_lockfile(&mirror, &layout, &ws_root, &snap).unwrap());
    assert_eq!(fs::read(app.join("Cargo.lock")).unwrap(), generated);

    // 2. Existing Cargo.lock must not be overwritten.
    fs::write(mirror.join("external/Cargo.lock"), b"# newer").unwrap();
    assert!(!sync_generated_lockfile(&mirror, &layout, &ws_root, &snap).unwrap());
    assert_eq!(fs::read(app.join("Cargo.lock")).unwrap(), generated);

    // 3. Multi-member workspace (pruned in mirror) must not write back a partial lockfile,
    //    and workspace-root rune.toml must be tracked in source_entries.
    let ws = tmp.path().join("ws");
    let member = ws.join("crates/app");
    fs::create_dir_all(member.join("src")).unwrap();
    fs::write(
        ws.join("Cargo.toml"),
        "[workspace]\nmembers = [\"crates/*\"]\n",
    )
    .unwrap();
    fs::write(ws.join("rune.toml"), "[dev]\nport = 19000\n").unwrap();
    fs::write(
        member.join("Cargo.toml"),
        "[package]\nname = \"ws-app\"\nversion = \"0.1.0\"\n",
    )
    .unwrap();
    fs::write(member.join("src/main.rs"), "fn main() {}").unwrap();

    let ws_layout = discover_layout(&member.join("Cargo.toml")).unwrap();
    let ws_snap = scan_sources(&mut cache, &ws_layout.source_entries()).unwrap();
    assert!(
        ws_snap.contains_key("external/rune.toml"),
        "workspace-root rune.toml must be tracked; keys={:?}",
        ws_snap.keys().collect::<Vec<_>>()
    );
    let ws_mirror = tmp.path().join("ws-mirror");
    sync_mirror(&ws_mirror, &ws_layout.crate_key, &ws_snap).unwrap();
    fs::write(ws_mirror.join("external/Cargo.lock"), b"# partial").unwrap();
    let ws_canon = fs::canonicalize(&ws).map(strip_verbatim).unwrap_or(ws.clone());
    assert!(!sync_generated_lockfile(&ws_mirror, &ws_layout, &ws_canon, &ws_snap).unwrap());
    assert!(!ws.join("Cargo.lock").exists());
}

#[test]
fn child_guard_isolates_process_group_and_stops_child_without_affecting_parent() {
    #[cfg(windows)]
    let mut cmd = {
        let mut c = Command::new("ping");
        c.args(["-n", "30", "127.0.0.1"]);
        c
    };
    #[cfg(not(windows))]
    let mut cmd = {
        let mut c = Command::new("sleep");
        c.arg("30");
        c
    };

    cmd.stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());

    let mut guard = ChildGuard::spawn(&mut cmd).expect("spawn child");
    assert!(guard.id().is_some());
    assert!(guard.try_wait().unwrap().is_none());

    guard.stop_with_timeout(Duration::from_millis(600));
    assert!(guard.id().is_none());
    assert!(guard.try_wait().unwrap().is_none());
}

#[test]
fn doctor_honors_explicit_rune_dx_and_rune_dx_unpinned_end_to_end() {
    let exe = env!("CARGO_BIN_EXE_rune");
    let tmp = TempDir::new("dx-env");

    let make_fake_dx = |name: &str, version_line: &str| -> PathBuf {
        #[cfg(windows)]
        {
            let p = tmp.path().join(format!("{name}.cmd"));
            fs::write(&p, format!("@echo off\r\necho {version_line}\r\n")).unwrap();
            p
        }
        #[cfg(not(windows))]
        {
            use std::os::unix::fs::PermissionsExt;
            let p = tmp.path().join(name);
            fs::write(&p, format!("#!/bin/sh\necho '{version_line}'\n")).unwrap();
            let mut perms = fs::metadata(&p).unwrap().permissions();
            perms.set_mode(0o755);
            fs::set_permissions(&p, perms).unwrap();
            p
        }
    };

    // Patched / newer Dioxus CLI via explicit RUNE_DX is accepted (with unpinned notice).
    let patched_dx = make_fake_dx("patched-dx", "dioxus 0.8.0-patched");
    let out_patched = Command::new(exe)
        .arg("doctor")
        .env("RUNE_DX", &patched_dx)
        .env_remove("RUNE_DX_UNPINNED")
        .current_dir(tmp.path())
        .output()
        .unwrap();
    assert!(out_patched.status.success());
    let stdout_patched = String::from_utf8_lossy(&out_patched.stdout);
    assert!(
        stdout_patched.contains("dioxus 0.8.0-patched")
            && stdout_patched.contains("unpinned; default pin is 0.7.10"),
        "expected patched RUNE_DX to be accepted as unpinned; stdout:\n{stdout_patched}"
    );

    // Unrelated Deno dx via RUNE_DX is still rejected unless RUNE_DX_UNPINNED=1.
    let deno_dx = make_fake_dx("deno-dx", "deno 2.1.0");
    let out_deno = Command::new(exe)
        .arg("doctor")
        .env("RUNE_DX", &deno_dx)
        .env_remove("RUNE_DX_UNPINNED")
        .current_dir(tmp.path())
        .output()
        .unwrap();
    assert!(out_deno.status.success());
    let stdout_deno = String::from_utf8_lossy(&out_deno.stdout);
    assert!(
        stdout_deno.contains("expected dioxus-cli 0.7.10"),
        "expected Deno dx to be rejected even via RUNE_DX; stdout:\n{stdout_deno}"
    );
}

fn strip_verbatim(path: PathBuf) -> PathBuf {
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
