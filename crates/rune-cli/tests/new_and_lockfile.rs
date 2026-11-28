use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

use rune_cli::dev::hot::{ensure_workspace_lockfile, ensure_workspace_lockfile_with};
use rune_cli::dev::mirror::{discover_layout, sync_mirror};
use rune_cli::git_rev::{
    is_upstream_rune_url, parse_ls_remote_rev, resolve_git_rev, resolve_remote_dev_rev,
    same_repo_url, upstream_dev_rev,
};
use rune_cli::new::generate_lockfile;
use rune_cli::watch::{FileCache, scan_sources};

static TEMP_SEQ: AtomicU64 = AtomicU64::new(0);

struct TempDir(PathBuf);

impl TempDir {
    #[allow(clippy::expect_used)]
    fn new(tag: &str) -> Self {
        let id = TEMP_SEQ.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "rune-cli-lock-rev-{tag}-{}-{id}",
            std::process::id()
        ));
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

#[allow(clippy::unwrap_used)]
fn git_ok(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .args(args)
        .current_dir(dir)
        .env("GIT_AUTHOR_NAME", "Test")
        .env("GIT_AUTHOR_EMAIL", "test@example.com")
        .env("GIT_COMMITTER_NAME", "Test")
        .env("GIT_COMMITTER_EMAIL", "test@example.com")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?} failed in {}: {}",
        dir.display(),
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

#[allow(clippy::unwrap_used)]
fn make_fake_cargo(dir: &Path, name: &str, succeed: bool) -> PathBuf {
    #[cfg(windows)]
    {
        let p = dir.join(format!("{name}.cmd"));
        let body = if succeed {
            "@echo off\r\necho # fake lockfile > Cargo.lock\r\nexit /b 0\r\n"
        } else {
            "@echo off\r\necho simulated registry failure 1>&2\r\nexit /b 1\r\n"
        };
        fs::write(&p, body).unwrap();
        p
    }
    #[cfg(not(windows))]
    {
        use std::os::unix::fs::PermissionsExt;
        let p = dir.join(name);
        let body = if succeed {
            "#!/bin/sh\necho '# fake lockfile' > Cargo.lock\nexit 0\n"
        } else {
            "#!/bin/sh\necho 'simulated registry failure' >&2\nexit 1\n"
        };
        fs::write(&p, body).unwrap();
        let mut perms = fs::metadata(&p).unwrap().permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&p, perms).unwrap();
        p
    }
}

#[test]
fn git_rev_url_matching_ls_remote_parsing_and_precedence() {
    // Upstream anomalyco/rune URL matching across protocols and formats.
    for valid in [
        "https://github.com/anomalyco/rune",
        "https://github.com/anomalyco/rune.git",
        "https://github.com/anomalyco/rune/",
        "https://x-access-token:tok@github.com/anomalyco/rune.git",
        "http://github.com/anomalyco/rune.git",
        "ssh://git@github.com/anomalyco/rune.git",
        "git@github.com:anomalyco/rune.git",
        "git@github.com:ANOMALYCO/RUNE",
        "git://github.com/anomalyco/rune.git",
    ] {
        assert!(is_upstream_rune_url(valid), "expected upstream match for {valid}");
    }

    for invalid in [
        "https://github.com/someone/rune.git",
        "https://github.com/anomalyco/rune-fork.git",
        "https://github.com/anomalyco/rune/extra",
        "https://evil.example.com/github.com/anomalyco/rune.git",
        "file:///tmp/cargo/git/db/rune-0123456789abcdef",
        r"Z:\work\rune",
        "",
    ] {
        assert!(
            !is_upstream_rune_url(invalid),
            "expected non-upstream for {invalid}"
        );
    }

    assert!(same_repo_url(
        "https://github.com/anomalyco/rune",
        "git@github.com:anomalyco/rune.git"
    ));
    assert!(!same_repo_url(
        "https://github.com/anomalyco/rune",
        "https://github.com/someone/rune"
    ));

    let ls_out = "\
1111111111111111111111111111111111111111\trefs/heads/main\n\
2222222222222222222222222222222222222222\trefs/heads/dev\n\
3333333333333333333333333333333333333333\trefs/heads/dev-other\n";
    assert_eq!(
        parse_ls_remote_rev(ls_out, "dev").as_deref(),
        Some("2222222222222222222222222222222222222222")
    );
    assert_eq!(parse_ls_remote_rev(ls_out, "missing"), None);

    // 1. Explicit --rev always wins without invoking lookup.
    let mut called = false;
    let rev = resolve_git_rev(
        Some("abcdef1234567"),
        "https://github.com/anomalyco/rune",
        "https://github.com/anomalyco/rune",
        "0000000000000000000000000000000000000000",
        |_| {
            called = true;
            None
        },
    );
    assert_eq!(rev.as_deref(), Some("abcdef1234567"));
    assert!(!called);

    // 2. Default URL + verified compile-time DEFAULT_REV uses it without network lookup.
    let rev = resolve_git_rev(
        None,
        "https://github.com/anomalyco/rune.git",
        "https://github.com/anomalyco/rune",
        "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        |_| {
            called = true;
            None
        },
    );
    assert_eq!(rev.as_deref(), Some("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"));
    assert!(!called);

    // 3. Empty DEFAULT_REV (fork/tarball/cargo-git build) queries upstream dev tip.
    let rev = resolve_git_rev(
        None,
        "https://github.com/anomalyco/rune",
        "https://github.com/anomalyco/rune",
        "",
        |url| {
            assert_eq!(url, "https://github.com/anomalyco/rune");
            Some("bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb".to_string())
        },
    );
    assert_eq!(rev.as_deref(), Some("bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"));

    // 4. Empty DEFAULT_REV + offline lookup falls back to None (renders branch = "dev").
    let rev = resolve_git_rev(
        None,
        "https://github.com/anomalyco/rune",
        "https://github.com/anomalyco/rune",
        "",
        |_| None,
    );
    assert_eq!(rev, None);

    // 5. Custom --git URL without --rev must NOT reuse anomalyco/rune DEFAULT_REV.
    let rev = resolve_git_rev(
        None,
        "https://github.com/someone/rune",
        "https://github.com/anomalyco/rune",
        "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        |url| {
            assert_eq!(url, "https://github.com/someone/rune");
            Some("cccccccccccccccccccccccccccccccccccccccc".to_string())
        },
    );
    assert_eq!(rev.as_deref(), Some("cccccccccccccccccccccccccccccccccccccccc"));
}

#[test]
fn upstream_dev_rev_never_pins_fork_or_unpushed_local_commits() {
    let tmp = TempDir::new("git-repos");

    // Create a local repo with commit A (upstream base) and commit B (local/fork work).
    let repo = tmp.path().join("checkout");
    fs::create_dir_all(&repo).unwrap();
    git_ok(&repo, &["init", "-q", "-b", "dev"]);
    fs::write(repo.join("a.txt"), "base").unwrap();
    git_ok(&repo, &["add", "a.txt"]);
    git_ok(&repo, &["commit", "-q", "-m", "base"]);
    let sha_a = git_ok(&repo, &["rev-parse", "HEAD"]);

    // Case 1: Official checkout where origin is anomalyco/rune and HEAD == origin/dev.
    git_ok(
        &repo,
        &["remote", "add", "origin", "https://github.com/anomalyco/rune.git"],
    );
    git_ok(&repo, &["update-ref", "refs/remotes/origin/dev", &sha_a]);
    assert_eq!(upstream_dev_rev(&repo).rev.as_deref(), Some(sha_a.as_str()));

    // Case 2: Unpushed local commit B ahead of origin/dev must resolve to A, never B.
    fs::write(repo.join("b.txt"), "local-change").unwrap();
    git_ok(&repo, &["add", "b.txt"]);
    git_ok(&repo, &["commit", "-q", "-m", "local work"]);
    let sha_b = git_ok(&repo, &["rev-parse", "HEAD"]);
    assert_ne!(sha_a, sha_b);
    assert_eq!(upstream_dev_rev(&repo).rev.as_deref(), Some(sha_a.as_str()));

    // Case 3: Fork with ONLY origin pointing to a fork must return None (never sha_b).
    git_ok(
        &repo,
        &["remote", "set-url", "origin", "https://github.com/fork-user/rune.git"],
    );
    git_ok(&repo, &["update-ref", "refs/remotes/origin/dev", &sha_b]);
    assert_eq!(upstream_dev_rev(&repo).rev, None);

    // Case 4: Fork that also has an upstream remote pointing to anomalyco/rune resolves to A.
    git_ok(
        &repo,
        &["remote", "add", "upstream", "git@github.com:anomalyco/rune.git"],
    );
    git_ok(&repo, &["update-ref", "refs/remotes/upstream/dev", &sha_a]);
    assert_eq!(upstream_dev_rev(&repo).rev.as_deref(), Some(sha_a.as_str()));

    // Case 5: Cargo git-db checkout (origin = file://...) cannot prove upstream -> None.
    let cargo_db = tmp.path().join("cargo-checkout");
    fs::create_dir_all(&cargo_db).unwrap();
    git_ok(&cargo_db, &["init", "-q", "-b", "dev"]);
    fs::write(cargo_db.join("x.txt"), "x").unwrap();
    git_ok(&cargo_db, &["add", "x.txt"]);
    git_ok(&cargo_db, &["commit", "-q", "-m", "commit"]);
    git_ok(
        &cargo_db,
        &[
            "remote",
            "add",
            "origin",
            "file:///tmp/cargo-git-db/rune-0123456789abcdef",
        ],
    );
    assert_eq!(upstream_dev_rev(&cargo_db).rev, None);

    // Case 6: resolve_remote_dev_rev works end-to-end against a local bare repo (zero network).
    let bare = tmp.path().join("upstream.git");
    fs::create_dir_all(&bare).unwrap();
    git_ok(&bare, &["init", "--bare", "-q"]);
    git_ok(
        &repo,
        &["push", "-q", bare.to_str().unwrap(), "HEAD:refs/heads/dev"],
    );
    let resolved = resolve_remote_dev_rev(bare.to_str().unwrap());
    assert_eq!(resolved.as_deref(), Some(sha_b.as_str()));
}

#[test]
fn ensure_workspace_lockfile_generates_lock_for_mirror_and_reports_failures() {
    let tmp = TempDir::new("hot-lockfile");
    let app = tmp.path().join("app");
    fs::create_dir_all(app.join("src")).unwrap();
    fs::write(
        app.join("Cargo.toml"),
        "[package]\nname = \"lock-probe\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
    )
    .unwrap();
    fs::write(app.join("src/main.rs"), "fn main() {}\n").unwrap();

    let manifest = app.join("Cargo.toml");
    assert!(!app.join("Cargo.lock").exists());

    // First call generates Cargo.lock using real `cargo generate-lockfile`.
    assert!(ensure_workspace_lockfile(&manifest, &app).unwrap());
    assert!(app.join("Cargo.lock").is_file());

    // After generation, discover_layout + sync_mirror stage Cargo.lock into the mirror so dx
    // starts with a pre-resolved lockfile instead of timing out in cargo metadata.
    let layout = discover_layout(&manifest).unwrap();
    let mut cache = FileCache::default();
    let snap = scan_sources(&mut cache, &layout.source_entries()).unwrap();
    assert!(snap.contains_key("external/Cargo.lock"));
    let mirror = tmp.path().join("mirror");
    sync_mirror(&mirror, &layout.crate_key, &snap).unwrap();
    assert!(mirror.join("external/Cargo.lock").is_file());

    // Second call is an immediate no-op (does not invoke cargo at all).
    let skipped = ensure_workspace_lockfile_with(
        OsStr::new("definitely-missing-cargo-binary"),
        &manifest,
        &app,
    )
    .unwrap();
    assert!(!skipped);

    // Direct generate_lockfile helper also works cleanly when Cargo.lock is removed.
    fs::remove_file(app.join("Cargo.lock")).unwrap();
    generate_lockfile(&manifest).unwrap();
    assert!(app.join("Cargo.lock").is_file());

    // Failure on a broken manifest without Cargo.lock fails cleanly without misdirecting to
    // --offline as a universal fix.
    let broken = tmp.path().join("broken");
    fs::create_dir_all(&broken).unwrap();
    let broken_manifest = broken.join("Cargo.toml");
    fs::write(&broken_manifest, "not valid toml = = =").unwrap();
    let err = ensure_workspace_lockfile(&broken_manifest, &broken).unwrap_err();
    assert!(
        err.message.contains("cargo generate-lockfile failed"),
        "unexpected error: {}",
        err.message
    );
}

#[test]
fn rune_new_generates_lockfile_by_default_and_stays_best_effort_on_failure() {
    let exe = env!("CARGO_BIN_EXE_rune");
    let tmp = TempDir::new("new-lock");
    let fake_ok = make_fake_cargo(tmp.path(), "fake-cargo-ok", true);
    let fake_fail = make_fake_cargo(tmp.path(), "fake-cargo-fail", false);
    let rev = "0123456789abcdef0123456789abcdef01234567";

    // 1. Default `rune new` runs `cargo generate-lockfile` and creates Cargo.lock.
    let app_ok = tmp.path().join("app-ok");
    let out_ok = Command::new(exe)
        .args(["new", app_ok.to_str().unwrap(), "--rev", rev, "--no-git"])
        .env("CARGO", &fake_ok)
        .output()
        .unwrap();
    assert!(
        out_ok.status.success(),
        "rune new failed: {}",
        String::from_utf8_lossy(&out_ok.stderr)
    );
    assert!(app_ok.join("Cargo.lock").is_file());
    assert!(String::from_utf8_lossy(&out_ok.stdout).contains("Generating Cargo.lock"));

    // 2. `--no-lockfile` skips `cargo generate-lockfile`.
    let app_skip = tmp.path().join("app-skip");
    let out_skip = Command::new(exe)
        .args([
            "new",
            app_skip.to_str().unwrap(),
            "--rev",
            rev,
            "--no-git",
            "--no-lockfile",
        ])
        .env("CARGO", &fake_ok)
        .output()
        .unwrap();
    assert!(out_skip.status.success());
    assert!(!app_skip.join("Cargo.lock").exists());

    // 3. Failed `cargo generate-lockfile` is best-effort: project still succeeds with a note.
    let app_fail = tmp.path().join("app-fail");
    let out_fail = Command::new(exe)
        .args(["new", app_fail.to_str().unwrap(), "--rev", rev, "--no-git"])
        .env("CARGO", &fake_fail)
        .output()
        .unwrap();
    assert!(
        out_fail.status.success(),
        "rune new should succeed best-effort even if cargo generate-lockfile fails: {}",
        String::from_utf8_lossy(&out_fail.stderr)
    );
    assert!(app_fail.join("Cargo.toml").is_file());
    assert!(!app_fail.join("Cargo.lock").exists());
    let stderr_fail = String::from_utf8_lossy(&out_fail.stderr);
    assert!(
        stderr_fail.contains("could not generate Cargo.lock"),
        "expected best-effort note on stderr; got:\n{stderr_fail}"
    );
}
