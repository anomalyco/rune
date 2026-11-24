use std::path::PathBuf;

use super::{Args, decode_argv};

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
