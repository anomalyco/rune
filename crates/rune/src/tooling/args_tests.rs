use std::collections::HashSet;
use std::path::PathBuf;

use super::args::{Args, HELP, RELAUNCH_DROPS, decode_argv};
use super::flags::{FlagId, FlagValue, flag_names, flags, flags_for, lookup_flag};

fn vec_s(items: &[&str]) -> Vec<String> {
    items.iter().map(|s| (*s).to_owned()).collect()
}

#[test]
fn release_without_tooling_leaves_process_args_untouched() {
    let raw = vec_s(&["--snap", "out.png", "--size", "800x600", "--native", "--trace", "t.jsonl", "--doc", "a.txt"]);
    let (args, extra) = Args::parse_process_argv(raw.clone(), false);
    assert!(!args.headless());
    assert!(args.snap.is_none());
    assert!(args.size.is_none());
    assert!(args.trace.is_none());
    assert!(!args.native);
    assert_eq!(args.fps, 60.0);
    assert_eq!(args.every, 1);
    assert_eq!(args.cols, 10);
    assert_eq!(args.cpu_boot_threads, 6);
    assert_eq!(extra, raw);
}

#[test]
fn release_without_tooling_does_not_exit_on_help_or_malformed_flags() {
    let raw = vec_s(&["--help", "-h", "--size", "large", "--snap"]);
    let (args, extra) = Args::parse_process_argv(raw.clone(), false);
    assert!(!args.headless());
    assert_eq!(extra, raw);
}

#[test]
fn with_tooling_parses_rune_flags_and_keeps_extras() {
    let raw = vec_s(&["--size", "800x600", "--snap", "out.png", "--doc", "a.txt"]);
    let (args, extra) = Args::parse_process_argv(raw, true);
    assert!(args.headless());
    assert_eq!(args.size, Some((800.0, 600.0)));
    assert_eq!(args.snap, Some(PathBuf::from("out.png")));
    assert_eq!(extra, vec_s(&["--doc", "a.txt"]));
}

#[test]
fn explicit_parse_from_still_parses_flags() {
    let args = Args::parse_from(vec_s(&["--size", "640x480", "--theme", "light", "--doc", "b.txt"]));
    assert_eq!(args.size, Some((640.0, 480.0)));
    assert_eq!(args.theme.as_deref(), Some("light"));
}

#[test]
fn settle_flag_requires_snap_or_tree() {
    let snap = Args::parse_from(vec_s(&["--snap", "out.png", "--settle"]));
    assert!(snap.settle && snap.headless());
    let tree = Args::parse_from(vec_s(&["--tree", "out.json", "--settle"]));
    assert!(tree.settle && tree.headless());
    let both = Args::parse_from(vec_s(&["--snap", "out.png", "--tree", "out.json", "--settle"]));
    assert!(both.settle);

    let default = Args::parse_from(vec_s(&["--snap", "out.png"]));
    assert!(!default.settle);

    let alone = Args { settle: true, ..Args::defaults() };
    assert_eq!(alone.validate(), Err("--settle needs --snap or --tree"));

    let with_trace = Args { settle: true, trace: Some(PathBuf::from("t.jsonl")), ..Args::defaults() };
    assert_eq!(with_trace.validate(), Err("--settle needs --snap or --tree"));

    let raw = vec_s(&["--snap", "out.png", "--settle"]);
    let (off, extra) = Args::parse_process_argv(raw.clone(), false);
    assert!(!off.settle && !off.headless());
    assert_eq!(extra, raw);
}

#[test]
fn non_utf8_os_args_decode_lossily_without_panicking() {
    #[cfg(windows)]
    let invalid = {
        use std::os::windows::ffi::OsStringExt;
        std::ffi::OsString::from_wide(&[b'a' as u16, 0xD800, b'b' as u16])
    };
    #[cfg(unix)]
    let invalid = {
        use std::os::unix::ffi::OsStringExt;
        std::ffi::OsString::from_vec(vec![b'a', 0xFF, b'b'])
    };
    let decoded = decode_argv([std::ffi::OsString::from("--file"), invalid]);
    assert_eq!(decoded[0], "--file");
    assert_eq!(decoded[1], "a�b");
}

#[test]
fn flags_metadata_invariants() {
    assert_eq!(Args::flags(), flags());
    let all = flags_for(true);
    assert!(!all.is_empty());

    let mut names = HashSet::new();
    let mut shorts = HashSet::new();
    for spec in all {
        assert!(spec.name.starts_with("--") && spec.name.len() > 2, "bad flag name: {:?}", spec.name);
        assert!(names.insert(spec.name), "duplicate flag name: {}", spec.name);
        if let Some(short) = spec.short {
            assert!(short.starts_with('-') && !short.starts_with("--") && short.len() == 2, "bad short flag: {short:?}");
            assert!(shorts.insert(short), "duplicate short flag: {short}");
        }
        assert!(!spec.description.is_empty() && spec.description.trim() == spec.description, "bad description on {}", spec.name);
        assert!(!spec.description.contains('\n'), "multi-line description on {}", spec.name);
        match spec.value {
            FlagValue::None => {}
            FlagValue::Required(hint) | FlagValue::Optional(hint) => {
                assert!(!hint.is_empty() && hint.trim() == hint, "bad value hint on {}", spec.name);
            }
        }
        assert_eq!(spec.repeatable, spec.name == "--crop", "unexpected repeatable on {}", spec.name);
        if spec.name != "--help" && spec.feature.is_none() {
            assert!(HELP.contains(spec.name), "{} is missing from HELP", spec.name);
        }
    }
    for short in &shorts {
        assert!(!names.contains(short), "short flag {short} collides with a long flag");
    }
    for drop_flag in RELAUNCH_DROPS {
        assert!(all.iter().any(|spec| spec.name == *drop_flag), "RELAUNCH_DROPS entry {drop_flag} missing from flags()");
    }
}

#[test]
fn flags_and_parser_stay_synchronized() {
    struct TempFile(PathBuf);
    impl Drop for TempFile {
        fn drop(&mut self) {
            // Best-effort temp file cleanup on drop.
            std::fs::remove_file(&self.0).ok();
        }
    }
    let script_path = std::env::temp_dir().join(format!("rune-args-flags-test-{}.json", std::process::id()));
    std::fs::write(&script_path, "[]").unwrap();
    let _guard = TempFile(script_path.clone());
    let script_str = script_path.to_string_lossy().into_owned();

    let sample_for = |name: &str| -> (Vec<&'static str>, Option<&str>) {
        match name {
            "--snap" | "--strip" | "--desktop" => (vec![], Some("out.png")),
            "--snap-on-patch" => (vec![], Some("patch.png")),
            "--hot-control" => (vec![], Some("control.json")),
            "--scene" => (vec![], Some("default")),
            "--playground" => (vec![], Some("Story")),
            "--variant" => (vec!["--playground"], Some("1")),
            "--whole-window" => (vec!["--playground", "--snap", "out.png"], None),
            "--settle" => (vec!["--snap", "out.png"], None),
            "--at" | "--start" => (vec![], Some("0.5")),
            "--size" => (vec![], Some("800x600")),
            "--scale" => (vec![], Some("2")),
            "--theme" => (vec![], Some("dark")),
            "--scroll-bars" => (vec![], Some("always")),
            "--script" => (vec![], Some("script.json")),
            "--frames" | "--every" | "--cols" | "--cpu-boot-threads" => (vec![], Some("4")),
            "--fps" => (vec![], Some("60")),
            "--out" => (vec![], Some("frames")),
            "--video" => (vec![], Some("out.mp4")),
            "--duration" => (vec![], Some("auto")),
            "--keys" => (vec![], Some("top-right")),
            "--keys-theme" => (vec![], Some("light")),
            "--crop" => (vec![], Some("0,0,100,100")),
            "--tree" => (vec![], Some("tree.json")),
            "--html" => (vec![], Some("out.html")),
            "--html-widths" => (vec![], Some("360,640")),
            "--html-template" => (vec![], Some("template.html")),
            "--trace" => (vec![], Some("trace.jsonl")),
            "--window-script" => (vec![], Some(script_str.as_str())),
            "--window" => (vec![], Some("main")),
            "--cpu-boot" => (vec!["--snap", "out.png"], Some("cpu-boot")),
            _ => (vec![], None),
        }
    };

    let all = flags_for(true);

    // Forward direction: every listed flag is accepted by the parser with its advertised FlagValue.
    for spec in all {
        if spec.name == "--help" {
            assert_eq!(spec.short, Some("-h"));
            assert_eq!(spec.value, FlagValue::None);
            assert_eq!(lookup_flag("--help"), Some(FlagId::Help));
            assert_eq!(lookup_flag("-h"), Some(FlagId::Help));
            continue;
        }
        #[cfg(feature = "hot")]
        if spec.name == "--hot" {
            assert_eq!(spec.short, None);
            assert_eq!(spec.value, FlagValue::None);
            assert_eq!(spec.feature, Some("hot"));
            assert_eq!(lookup_flag("--hot"), Some(FlagId::Hot));
            continue;
        }

        let (companions, sample) = sample_for(spec.name);
        match spec.value {
            FlagValue::None => {
                assert!(sample.is_none(), "unexpected sample value for FlagValue::None flag {}", spec.name);
                let mut argv = vec_s(&companions);
                argv.push(spec.name.to_owned());
                let (_, extra) = Args::parse_process_argv(argv, true);
                assert!(extra.is_empty(), "{} left unexpected extra args: {extra:?}", spec.name);
            }
            FlagValue::Required(_) => {
                let value = sample.unwrap_or_else(|| panic!("add a sample value in args_tests.rs for {}", spec.name));
                let mut argv = vec_s(&companions);
                argv.push(spec.name.to_owned());
                argv.push(value.to_owned());
                let (_, extra) = Args::parse_process_argv(argv, true);
                assert!(extra.is_empty(), "{} {value} left unexpected extra args: {extra:?}", spec.name);
                if spec.repeatable {
                    let mut repeated = vec_s(&companions);
                    repeated.extend([spec.name.to_owned(), value.to_owned(), spec.name.to_owned(), value.to_owned()]);
                    let (args, extra) = Args::parse_process_argv(repeated, true);
                    assert!(extra.is_empty(), "repeated {} left unexpected extra args: {extra:?}", spec.name);
                    assert_eq!(args.crops.len(), 2, "repeated {} should accumulate", spec.name);
                }
            }
            FlagValue::Optional(_) => {
                let value = sample.unwrap_or_else(|| panic!("add a sample value in args_tests.rs for {}", spec.name));
                let mut bare = vec_s(&companions);
                bare.push(spec.name.to_owned());
                let (_, extra_bare) = Args::parse_process_argv(bare, true);
                assert!(extra_bare.is_empty(), "bare {} left unexpected extra args: {extra_bare:?}", spec.name);

                let mut bare_then_app = vec_s(&companions);
                bare_then_app.extend([spec.name.to_owned(), "--app-flag".to_owned()]);
                let (_, extra_app) = Args::parse_process_argv(bare_then_app, true);
                assert_eq!(extra_app, vec_s(&["--app-flag"]), "bare {} swallowed app flag", spec.name);

                let mut with_val = vec_s(&companions);
                with_val.extend([spec.name.to_owned(), value.to_owned()]);
                let (_, extra_val) = Args::parse_process_argv(with_val, true);
                assert!(extra_val.is_empty(), "{} {value} left unexpected extra args: {extra_val:?}", spec.name);
            }
        }
    }

    // Reverse direction: every parser-known flag in FLAG_NAMES is listed in flags().
    let known = flag_names();
    assert_eq!(all.len(), known.len());
    for (id, name, short) in known {
        assert_eq!(lookup_flag(name), Some(*id));
        if let Some(short) = short {
            assert_eq!(lookup_flag(short), Some(*id));
        }
        assert!(
            all.iter().any(|spec| spec.name == *name && spec.short == *short),
            "parser flag {name} ({short:?}) missing from flags()"
        );
    }
}

#[test]
fn flags_feature_gating() {
    let all = flags_for(true);
    let hot_flags = ["--hot", "--snap-on-patch", "--hot-control"];
    #[cfg(feature = "hot")]
    {
        for name in hot_flags {
            let spec = all.iter().find(|s| s.name == name).unwrap_or_else(|| panic!("{name} missing under feature hot"));
            assert_eq!(spec.feature, Some("hot"));
        }
        for spec in all {
            if !hot_flags.contains(&spec.name) {
                assert_eq!(spec.feature, None, "unexpected feature on {}", spec.name);
            }
        }
    }
    #[cfg(not(feature = "hot"))]
    {
        for name in hot_flags {
            assert!(!all.iter().any(|s| s.name == name), "{name} must not appear without feature hot");
        }
        for spec in all {
            assert_eq!(spec.feature, None, "unexpected feature on {}", spec.name);
        }
    }
}

#[test]
fn flags_empty_without_dev_tooling() {
    assert!(flags_for(false).is_empty());
    #[cfg(any(feature = "tooling", feature = "hot", debug_assertions))]
    assert_eq!(flags(), flags_for(true));
    #[cfg(not(any(feature = "tooling", feature = "hot", debug_assertions)))]
    assert!(flags().is_empty());
}
