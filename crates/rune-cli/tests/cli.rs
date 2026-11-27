use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use rune_cli::args::{Command, DevArgs, DevMode, NewArgs, parse};
use rune_cli::config::{resolve_dev_config};
use rune_cli::dev::hot::{ChangeDecision, build_dx_args, classify_change};
use rune_cli::dev::mirror::{discover_layout, prune_workspace_manifest, sync_mirror};
use rune_cli::dev::restart::build_cargo_run_args;
use rune_cli::dev::{SelectedMode, select_mode};
use rune_cli::dx::{DxStatus, is_required_dioxus_version};
use rune_cli::glob;
use rune_cli::guard::{GuardOutcome, check_edit, contract};
use rune_cli::new::{self, RuneSource, render_cargo_toml, validate_package_name};
use rune_cli::pins::{check_pins, required_patches_from_manifest};
use rune_cli::watch::{FileCache, Snapshot, diff_snapshots, scan_sources};

static TEMP_SEQ: AtomicU64 = AtomicU64::new(0);

struct TempDir(PathBuf);

impl TempDir {
    #[allow(clippy::expect_used)]
    fn new(tag: &str) -> Self {
        let id = TEMP_SEQ.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("rune-cli-test-{tag}-{}-{id}", std::process::id()));
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

fn svec(items: &[&str]) -> Vec<String> {
    items.iter().map(|s| (*s).to_string()).collect()
}

#[test]
fn args_parse_all_commands_and_cargo_subcommand_prefix() {
    let cmd = parse(&svec(&["rune", "new", "my-app", "--rev", "abc1234", "--no-git"])).unwrap();
    assert_eq!(
        cmd,
        Command::New(NewArgs {
            target: PathBuf::from("my-app"),
            name: None,
            rev: Some("abc1234".to_string()),
            git: None,
            rune_path: None,
            force: false,
            no_git: true,
        })
    );

    let cargo_cmd = parse(&svec(&[
        "cargo-rune",
        "rune",
        "dev",
        "--port=19000",
        "--mode",
        "hot",
        "--patch",
        "src/ui/**/*.rs",
        "-F",
        "a,b",
        "--",
        "--scene",
        "on",
    ]))
    .unwrap();
    match cargo_cmd {
        Command::Dev(dev) => {
            assert_eq!(dev.port, Some(19000));
            assert_eq!(dev.mode, Some(DevMode::Hot));
            assert_eq!(dev.patch, svec(&["src/ui/**/*.rs"]));
            assert_eq!(dev.features, svec(&["a", "b"]));
            assert_eq!(dev.app_args, svec(&["--scene", "on"]));
        }
        other => panic!("expected Dev, got {other:?}"),
    }

    assert!(parse(&svec(&["rune", "new", "a", "--path", ".", "--rev", "deadbeef"])).is_err());
    assert!(parse(&svec(&["rune", "dev", "--port", "0"])).is_err());
    assert!(parse(&svec(&["rune", "dev", "--mode", "bogus"])).is_err());
    assert!(parse(&svec(&["rune", "unknown"])).is_err());
}

#[test]
fn glob_handles_double_star_classes_and_windows_separators() {
    assert!(glob::matches("src/ui/button.rs", "src/ui/**/*.rs"));
    assert!(glob::matches("src/ui/nested/deep/button.rs", "src/ui/**/*.rs"));
    assert!(glob::matches(r"src\ui\nested\button.rs", "src/ui/**/*.rs"));
    assert!(!glob::matches("src/main.rs", "src/ui/**/*.rs"));
    assert!(glob::matches("src/view.rs", "./src/view.rs"));
    assert!(glob::matches("a/b1.rs", "a/b[0-9].rs"));
    assert!(!glob::matches("a/bx.rs", "a/b[0-9].rs"));
    assert!(glob::matches("a/bx.rs", "a/b[!0-9].rs"));
    assert!(!glob::matches("src/a/b.rs", "src/*.rs"));
}

#[test]
fn guard_matches_rune_dev_guard_contract_rules() {
    let base = "struct S { a: u32, b: u32 } fn view(s: &S) -> u32 { s.a + 1 } fn main() { let s = S { a: 1, b: 2 }; }";
    assert_eq!(
        check_edit(base, &base.replace("s.a + 1", "s.b + 20")),
        GuardOutcome::Compatible
    );
    for edit in [
        base.replace("a: u32, b: u32", "b: u32, a: u32"),
        base.replace("s: &S", "s: &mut S"),
        base.replace("a: 1", "a: 9"),
        base.replace("s.a + 1", "static X: u32 = 3; s.a + X"),
        base.replace("s.a + 1", "mystery!(s)"),
    ] {
        assert_eq!(check_edit(base, &edit), GuardOutcome::RestartNeeded);
    }

    let inherent = "struct Layout; impl Layout { fn main(&self) -> &'static str { \"Home\" } } fn main() { let x = 1; }";
    assert_eq!(
        check_edit(inherent, &inherent.replace("\"Home\"", "\"Start\"")),
        GuardOutcome::Compatible
    );
    assert_eq!(
        check_edit(inherent, &inherent.replace("let x = 1;", "let x = 2;")),
        GuardOutcome::RestartNeeded
    );

    for kept in [
        "impl Layout { const fn n() -> usize { 1 } }",
        "impl Layout { #[persistent] fn n(&self) -> usize { 1 } }",
        "#[persistent] impl Layout { fn n(&self) -> usize { 1 } }",
        "impl T for Layout { fn n(&self) -> usize { 1 } }",
    ] {
        assert_ne!(
            contract(kept).unwrap(),
            contract(&kept.replace("{ 1 }", "{ 2 }")).unwrap()
        );
    }

    let inert_attrs = "#[doc = \"view\"] #[inline(never)] #[cold] #[allow(unused)] fn view() { let n = 1; }";
    assert_eq!(
        check_edit(inert_attrs, &inert_attrs.replace("n = 1", "n = 2")),
        GuardOutcome::Compatible
    );
    assert_eq!(
        check_edit(inert_attrs, &inert_attrs.replace("inline(never)", "inline(always)")),
        GuardOutcome::RestartNeeded
    );

    assert_eq!(
        check_edit("fn view() { vec![1, 2]; }", "fn view() { vec![3; 4]; }"),
        GuardOutcome::Compatible
    );
    assert_eq!(
        check_edit(
            "fn view() { vec![{ static X: u32 = 1; X }]; }",
            "fn view() { vec![{ static X: f32 = 1; X }]; }",
        ),
        GuardOutcome::RestartNeeded
    );
    assert!(matches!(
        check_edit("fn view() {}", "fn view( {"),
        GuardOutcome::SyntaxError(_)
    ));
}

#[test]
fn dx_version_parser_distinguishes_dioxus_0_7_10_from_deno_and_other_versions() {
    assert!(is_required_dioxus_version("dioxus 0.7.10 (abc123)"));
    assert!(is_required_dioxus_version("dioxus-cli v0.7.10"));
    assert!(!is_required_dioxus_version("dioxus 0.7.100"));
    assert!(!is_required_dioxus_version("dioxus 0.7.9"));
    assert!(!is_required_dioxus_version("deno 2.1.0"));
}

#[test]
fn dev_mode_selection_handles_windows_missing_dx_and_explicit_modes() {
    let ready = DxStatus::Ready {
        path: PathBuf::from("/usr/local/bin/dx"),
        version: "dioxus 0.7.10".to_string(),
    };
    let wrong = DxStatus::WrongVersion {
        path: PathBuf::from("/usr/local/bin/dx"),
        version: "deno 2.0".to_string(),
    };

    // Auto on macOS/Linux with dx 0.7.10 + hot feature -> Hot
    assert_eq!(
        select_mode(DevMode::Auto, false, true, &ready).unwrap(),
        SelectedMode::Hot {
            dx_path: PathBuf::from("/usr/local/bin/dx"),
            windows_opt_in_notice: false,
        }
    );

    // Auto on Windows defaults to watch-and-restart even if dx is installed
    assert!(matches!(
        select_mode(DevMode::Auto, true, true, &ready).unwrap(),
        SelectedMode::Restart { reason: Some(_) }
    ));

    // Explicit --mode hot on Windows honors opt-in with notice
    assert_eq!(
        select_mode(DevMode::Hot, true, true, &ready).unwrap(),
        SelectedMode::Hot {
            dx_path: PathBuf::from("/usr/local/bin/dx"),
            windows_opt_in_notice: true,
        }
    );

    // Auto without dx or with wrong dx falls back to Restart
    assert!(matches!(
        select_mode(DevMode::Auto, false, true, &DxStatus::Missing).unwrap(),
        SelectedMode::Restart { reason: Some(_) }
    ));
    assert!(matches!(
        select_mode(DevMode::Auto, false, true, &wrong).unwrap(),
        SelectedMode::Restart { reason: Some(_) }
    ));
    assert!(matches!(
        select_mode(DevMode::Auto, false, false, &ready).unwrap(),
        SelectedMode::Restart { reason: Some(_) }
    ));

    // Explicit --mode hot errors if dx or hot feature is missing
    assert!(select_mode(DevMode::Hot, false, true, &DxStatus::Missing).is_err());
    assert!(select_mode(DevMode::Hot, false, true, &wrong).is_err());
    assert!(select_mode(DevMode::Hot, false, false, &ready).is_err());
}

#[test]
fn rune_new_scaffolds_valid_pinned_project_and_passes_pin_checks() {
    let tmp = TempDir::new("new-git");
    let app_dir = tmp.path().join("my-rune-app");
    let rev = "0123456789abcdef0123456789abcdef01234567";
    new::run(&NewArgs {
        target: app_dir.clone(),
        name: None,
        rev: Some(rev.to_string()),
        git: None,
        rune_path: None,
        force: false,
        no_git: true,
    })
    .unwrap();

    assert!(app_dir.join("Cargo.toml").is_file());
    assert!(app_dir.join("rune.toml").is_file());
    assert!(app_dir.join("src/main.rs").is_file());
    assert!(app_dir.join("src/view.rs").is_file());
    assert!(app_dir.join(".gitignore").is_file());
    assert!(app_dir.join("README.md").is_file());

    let manifest = fs::read_to_string(app_dir.join("Cargo.toml")).unwrap();
    assert!(manifest.contains(&format!("rev = \"{rev}\"")));
    assert!(manifest.contains("hot = [\"rune/hot\"]"));
    assert!(manifest.contains("[profile.rune-hot]"));

    // Generated Cargo.toml must match Rune's root [patch.crates-io] exactly.
    let root_manifest = fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../Cargo.toml")).unwrap();
    let required = required_patches_from_manifest(&root_manifest).unwrap();
    assert!(!required.is_empty());
    let problems = check_pins(&manifest, &required, None, None).unwrap();
    assert!(problems.is_empty(), "unexpected pin problems: {problems:?}");

    // Re-running without --force must refuse non-empty directory.
    assert!(new::run(&NewArgs {
        target: app_dir.clone(),
        name: None,
        rev: Some(rev.to_string()),
        git: None,
        rune_path: None,
        force: false,
        no_git: true,
    }).is_err());

    assert!(validate_package_name("good-app_1").is_ok());
    assert!(validate_package_name("1bad").is_err());
    assert!(validate_package_name("match").is_err());
    assert!(validate_package_name("bad name").is_err());
}

#[test]
fn config_merges_cargo_metadata_rune_toml_and_cli_flags_in_order() {
    let tmp = TempDir::new("config");
    let dir = tmp.path();
    fs::write(
        dir.join("Cargo.toml"),
        r#"[package]
name = "demo"
version = "0.2.0"
edition = "2024"

[features]
hot = ["rune/hot"]

[package.metadata.rune.dev]
patch = ["src/meta.rs"]
features = ["from-meta"]
bin = "meta-bin"
port = 18001
"#,
    )
    .unwrap();

    let empty_args = DevArgs {
        manifest_path: None,
        package: None,
        bin: None,
        features: Vec::new(),
        patch: Vec::new(),
        port: None,
        mode: None,
        jobs: Some(2),
        target_dir: None,
        app_args: Vec::new(),
    };

    let cfg1 = resolve_dev_config(&empty_args, dir).unwrap();
    assert_eq!(cfg1.patch, svec(&["src/meta.rs"]));
    assert_eq!(cfg1.features, svec(&["from-meta"]));
    assert_eq!(cfg1.bin.as_deref(), Some("meta-bin"));
    assert_eq!(cfg1.port, 18001);

    // rune.toml overrides Cargo.toml metadata
    fs::write(
        dir.join("rune.toml"),
        r#"[dev]
patch = ["src/view.rs"]
port = 18002
mode = "restart"
"#,
    )
    .unwrap();
    let cfg2 = resolve_dev_config(&empty_args, dir).unwrap();
    assert_eq!(cfg2.patch, svec(&["src/view.rs"]));
    assert_eq!(cfg2.port, 18002);
    assert_eq!(cfg2.mode, DevMode::Restart);
    // Unspecified fields in rune.toml inherit from metadata
    assert_eq!(cfg2.bin.as_deref(), Some("meta-bin"));

    // CLI flags override rune.toml
    let cli_args = DevArgs {
        bin: Some("cli-bin".to_string()),
        features: svec(&["extra"]),
        patch: svec(&["src/custom/**/*.rs"]),
        port: Some(18003),
        mode: Some(DevMode::Hot),
        app_args: svec(&["--scene", "on"]),
        ..empty_args
    };
    let cfg3 = resolve_dev_config(&cli_args, dir).unwrap();
    assert_eq!(cfg3.bin.as_deref(), Some("cli-bin"));
    assert_eq!(cfg3.features, svec(&["from-meta", "extra"]));
    assert_eq!(cfg3.patch, svec(&["src/custom/**/*.rs"]));
    assert_eq!(cfg3.port, 18003);
    assert_eq!(cfg3.mode, DevMode::Hot);

    let dx_args = build_dx_args(&cfg3);
    assert!(dx_args.contains(&"--hot-patch".to_string()));
    assert!(dx_args.contains(&"hot,from-meta,extra".to_string()));
    assert!(dx_args.iter().any(|a| a.starts_with("--cargo-args=")));
    assert!(dx_args.contains(&"--args=--scene on".to_string()));

    let run_args = build_cargo_run_args(&cfg3);
    assert_eq!(run_args[0], "run");
    assert!(run_args.contains(&"--bin".to_string()));
    assert!(run_args.contains(&"cli-bin".to_string()));
    assert!(run_args.ends_with(&svec(&["--", "--scene", "on"])));
}

#[test]
fn pins_detects_missing_patch_mismatched_rev_and_lock_drift() {
    let required = svec(&["wgpu-hal", "winit"]);
    let manifest = render_cargo_toml(
        "demo",
        &RuneSource::Git {
            url: "https://github.com/anomalyco/rune".to_string(),
            rev: Some("aaaaaaaaaaaa".to_string()),
        },
        &required,
    );
    assert!(check_pins(&manifest, &required, None, None).unwrap().is_empty());

    let missing = manifest.replace(
        "winit = { git = \"https://github.com/anomalyco/rune\", rev = \"aaaaaaaaaaaa\" }\n",
        "",
    );
    let probs = check_pins(&missing, &required, None, None).unwrap();
    assert!(probs.iter().any(|p| p.contains("patch.crates-io.winit: missing")));

    let mismatched = manifest.replace(
        "winit = { git = \"https://github.com/anomalyco/rune\", rev = \"aaaaaaaaaaaa\" }",
        "winit = { git = \"https://github.com/anomalyco/rune\", rev = \"bbbbbbbbbbbb\" }",
    );
    let probs = check_pins(&mismatched, &required, None, None).unwrap();
    assert!(probs.iter().any(|p| p.contains("Rune git lines differ")));

    let bad_lock = r#"[[package]]
name = "rune"
version = "0.0.1"
source = "git+https://github.com/anomalyco/rune?rev=cccccccccccc#cccccccccccc1111111111111111111111111111"
"#;
    let probs = check_pins(&manifest, &required, Some(bad_lock), None).unwrap();
    assert!(probs.iter().any(|p| p.contains("Cargo.lock: Rune is locked at")));
}

#[test]
fn hot_change_classifier_patches_fn_body_waits_on_syntax_and_restarts_on_schema_or_unpatched_file() {
    let mut before: Snapshot = BTreeMap::new();
    before.insert(
        "external/app/src/view.rs".to_string(),
        b"struct S { x: u32 } pub fn view(s: &S) -> u32 { s.x + 1 }".to_vec(),
    );
    before.insert(
        "external/app/Cargo.toml".to_string(),
        b"[package]\nname = \"app\"\n".to_vec(),
    );
    let globs = svec(&["src/view.rs", "src/ui/**/*.rs"]);

    // Compatible body edit -> Patch
    let mut after_patch = before.clone();
    after_patch.insert(
        "external/app/src/view.rs".to_string(),
        b"struct S { x: u32 } pub fn view(s: &S) -> u32 { s.x + 2 }".to_vec(),
    );
    assert_eq!(
        classify_change("external/app", &globs, &before, &after_patch),
        Some(ChangeDecision::Patch {
            files: svec(&["external/app/src/view.rs"]),
        })
    );

    // Syntax error -> WaitForSyntax (no restart)
    let mut after_syntax = before.clone();
    after_syntax.insert(
        "external/app/src/view.rs".to_string(),
        b"pub fn view( {".to_vec(),
    );
    assert!(matches!(
        classify_change("external/app", &globs, &before, &after_syntax),
        Some(ChangeDecision::WaitForSyntax { .. })
    ));

    // Struct change -> Restart
    let mut after_struct = before.clone();
    after_struct.insert(
        "external/app/src/view.rs".to_string(),
        b"struct S { x: i64 } pub fn view(s: &S) -> u32 { s.x as u32 + 1 }".to_vec(),
    );
    assert!(matches!(
        classify_change("external/app", &globs, &before, &after_struct),
        Some(ChangeDecision::Restart { .. })
    ));

    // Non-patchable file -> Restart
    let mut after_manifest = before.clone();
    after_manifest.insert(
        "external/app/Cargo.toml".to_string(),
        b"[package]\nname = \"app\"\nversion = \"0.2.0\"\n".to_vec(),
    );
    assert!(matches!(
        classify_change("external/app", &globs, &before, &after_manifest),
        Some(ChangeDecision::Restart { .. })
    ));
}

#[test]
fn mirror_prunes_workspace_members_and_syncs_stale_files() {
    let ws = r#"[workspace]
members = ["apps/*", "crates/*"]
default-members = ["apps/*"]
exclude = ["apps/ignored"]
"#;
    let pruned = prune_workspace_manifest(
        ws,
        &svec(&["apps/demo", "apps/ignored", "crates/shared"]),
    )
    .unwrap();
    let members_line = pruned
        .lines()
        .find(|l| l.trim_start().starts_with("members"))
        .expect("members line");
    assert!(members_line.contains("apps/demo"));
    assert!(members_line.contains("crates/shared"));
    assert!(!members_line.contains("apps/ignored"));
    let default_line = pruned
        .lines()
        .find(|l| l.trim_start().starts_with("default-members"))
        .expect("default-members line");
    assert!(default_line.contains("apps/demo"));
    assert!(!default_line.contains("apps/ignored"));

    let tmp = TempDir::new("mirror");
    let src = tmp.path().join("proj");
    fs::create_dir_all(src.join("src")).unwrap();
    fs::write(src.join("Cargo.toml"), "[package]\nname = \"proj\"\nversion = \"0.1.0\"\n").unwrap();
    fs::write(src.join("src/main.rs"), "fn main() {}").unwrap();
    fs::write(src.join(".env"), "SECRET=1").unwrap();

    let layout = discover_layout(&src.join("Cargo.toml")).unwrap();
    let mut cache = FileCache::default();
    let snap1 = scan_sources(&mut cache, &layout.source_entries()).unwrap();
    assert!(!snap1.keys().any(|k| k.ends_with(".env")), ".env must never be mirrored");

    let mirror = tmp.path().join("mirror");
    sync_mirror(&mirror, &layout.crate_key, &snap1).unwrap();
    assert!(mirror.join("external/src/main.rs").is_file());

    // Remove a file and verify diff + mirror cleanup
    fs::write(src.join("src/extra.rs"), "pub fn x() {}").unwrap();
    let snap2 = scan_sources(&mut cache, &layout.source_entries()).unwrap();
    assert_eq!(diff_snapshots(&snap1, &snap2).added, svec(&["external/src/extra.rs"]));
    sync_mirror(&mirror, &layout.crate_key, &snap2).unwrap();
    assert!(mirror.join("external/src/extra.rs").is_file());

    fs::remove_file(src.join("src/extra.rs")).unwrap();
    let snap3 = scan_sources(&mut cache, &layout.source_entries()).unwrap();
    sync_mirror(&mirror, &layout.crate_key, &snap3).unwrap();
    assert!(!mirror.join("external/src/extra.rs").exists());
}

#[test]
fn rune_binary_new_check_and_doctor_work_end_to_end() {
    let exe = env!("CARGO_BIN_EXE_rune");
    let tmp = TempDir::new("e2e-bin");
    let app_dir = tmp.path().join("smoke-app");
    let rev = "1111111111111111111111111111111111111111";

    let ver = std::process::Command::new(exe)
        .arg("--version")
        .output()
        .unwrap();
    assert!(ver.status.success());
    assert!(String::from_utf8_lossy(&ver.stdout).starts_with("rune "));

    let created = std::process::Command::new(exe)
        .args(["new", app_dir.to_str().unwrap(), "--rev", rev, "--no-git"])
        .output()
        .unwrap();
    assert!(
        created.status.success(),
        "rune new failed: {}",
        String::from_utf8_lossy(&created.stderr)
    );

    let checked = std::process::Command::new(exe)
        .args([
            "check",
            "--pins-only",
            "--manifest-path",
            app_dir.join("Cargo.toml").to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(
        checked.status.success(),
        "rune check --pins-only failed: {}",
        String::from_utf8_lossy(&checked.stderr)
    );
    assert!(String::from_utf8_lossy(&checked.stdout).contains("Rune pins agree"));

    let doctor = std::process::Command::new(exe)
        .args([
            "doctor",
            "--manifest-path",
            app_dir.join("Cargo.toml").to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(
        doctor.status.success(),
        "rune doctor failed: {}",
        String::from_utf8_lossy(&doctor.stderr)
    );
    let doc_out = String::from_utf8_lossy(&doctor.stdout);
    assert!(doc_out.contains("project: smoke-app"));
    assert!(doc_out.contains("Rune pins: URL, rev, and [patch.crates-io] agree"));
}
