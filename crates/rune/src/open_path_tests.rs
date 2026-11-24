use super::*;
use crate::{Cx, Env, OpenUrlError, Runtime, el::{El, el}};

#[test]
fn validates_path_syntax_without_touching_filesystem() {
    // Nonexistent paths are accepted syntactically (no stat on the UI thread).
    assert_eq!(
        check_for_platform(Path::new("/definitely/does/not/exist/file.txt"), true),
        Ok(PathBuf::from("/definitely/does/not/exist/file.txt")),
    );
    assert_eq!(check_for_platform(Path::new("relative/file.txt"), true), Ok(PathBuf::from("relative/file.txt")));
    assert_eq!(check_for_platform(Path::new(" "), true), Ok(PathBuf::from(" ")));
    assert_eq!(check_for_platform(Path::new("-leading-dash.txt"), true), Ok(PathBuf::from("-leading-dash.txt")));
    assert_eq!(check_for_platform(Path::new("café/日本語.png"), true), Ok(PathBuf::from("café/日本語.png")));

    assert_eq!(check_for_platform(Path::new(""), true), Err(OpenPathError::Empty));
    assert_eq!(check_for_platform(Path::new("a\0b"), true), Err(OpenPathError::Invalid));
    assert_eq!(check_for_platform(Path::new("\0"), true), Err(OpenPathError::Invalid));

    // Unsupported platforms (wasm32, mobile) refuse every path up front.
    assert_eq!(check_for_platform(Path::new("/tmp/a.txt"), false), Err(OpenPathError::Unsupported));
    assert_eq!(check_for_platform(Path::new(""), false), Err(OpenPathError::Unsupported));
}

#[test]
fn error_display_messages_are_clear() {
    assert_eq!(OpenPathError::Empty.to_string(), "empty path");
    assert_eq!(OpenPathError::Invalid.to_string(), "not a valid path");
    assert!(OpenPathError::Unsupported.to_string().contains("not supported"));
    assert_eq!(OpenPathError::Failed("boom".into()).to_string(), "couldn't open the path: boom");
}

#[test]
fn file_uri_encoding_handles_reserved_chars_unicode_and_raw_bytes() {
    assert_eq!(
        file_uri_from_bytes(b"/home/me/notes.txt").as_deref(),
        Some("file:///home/me/notes.txt"),
    );
    assert_eq!(
        file_uri_from_bytes(b"/home/me/My File #1 (100%)?a=b[0].txt").as_deref(),
        Some("file:///home/me/My%20File%20%231%20%28100%25%29%3Fa%3Db%5B0%5D.txt"),
    );
    assert_eq!(
        file_uri_from_bytes("/home/me/café.txt".as_bytes()).as_deref(),
        Some("file:///home/me/caf%C3%A9.txt"),
    );
    assert_eq!(
        file_uri_from_bytes(b"/tmp/\xff\xfe").as_deref(),
        Some("file:///tmp/%FF%FE"),
    );
    assert_eq!(file_uri_from_bytes(b""), None);
    assert_eq!(file_uri_from_bytes(b"relative/path"), None);
    assert_eq!(file_uri_from_bytes(b"/tmp/a\0b"), None);
}

#[test]
fn reveal_fallback_and_xdg_open_arg_guard_edges() {
    assert_eq!(reveal_fallback_target(Path::new("/home/me/doc.pdf")), PathBuf::from("/home/me"));
    assert_eq!(reveal_fallback_target(Path::new("/home/me")), PathBuf::from("/home"));
    assert_eq!(reveal_fallback_target(Path::new("/")), PathBuf::from("/"));
    assert_eq!(reveal_fallback_target(Path::new("notes.txt")), PathBuf::from("."));

    assert_eq!(xdg_open_arg(Path::new("-flag.txt")), OsString::from("./-flag.txt"));
    assert_eq!(xdg_open_arg(Path::new("/tmp/-flag.txt")), OsString::from("/tmp/-flag.txt"));
    assert_eq!(xdg_open_arg(Path::new("notes.txt")), OsString::from("notes.txt"));
}

#[test]
fn windows_shell_wide_strips_verbatim_prefixes_and_normalizes_slashes() {
    let to_units = |s: &str| s.encode_utf16().collect::<Vec<u16>>();
    let from_wide = |w: Vec<u16>| {
        assert_eq!(w.last(), Some(&0), "must be NUL-terminated");
        String::from_utf16(&w[..w.len() - 1]).unwrap()
    };

    assert_eq!(from_wide(windows_shell_wide_from_units(&to_units("C:/dir/sub/a.txt"))), r"C:\dir\sub\a.txt");
    assert_eq!(from_wide(windows_shell_wide_from_units(&to_units(r"\\?\C:\dir\a.txt"))), r"C:\dir\a.txt");
    assert_eq!(from_wide(windows_shell_wide_from_units(&to_units(r"\\?\C:/dir/a.txt"))), r"C:\dir\a.txt");
    assert_eq!(
        from_wide(windows_shell_wide_from_units(&to_units(r"\\?\UNC\server\share/a.txt"))),
        r"\\server\share\a.txt",
    );
    assert_eq!(
        from_wide(windows_shell_wide_from_units(&to_units(r"\\?\Volume{1234}\a.txt"))),
        r"\\?\Volume{1234}\a.txt",
    );
}

#[test]
fn headless_runtime_records_open_and_reveal_requests_and_keeps_open_url_strict() {
    fn view(_: &Vec<&'static str>, _: &mut Cx<Vec<&'static str>>) -> El {
        crate::row().key("row").size(40.0, 20.0)
            .child(el().key("open").size(20.0, 20.0).on_press_tx(|seen: &mut Vec<&'static str>, tx| {
                seen.push("open");
                tx.open_path("/nonexistent/clicked.txt").unwrap();
            }))
            .child(el().key("reveal").size(20.0, 20.0).on_press_tx(|seen: &mut Vec<&'static str>, tx| {
                seen.push("reveal");
                tx.reveal_path("/nonexistent/folder").unwrap();
            }))
    }

    let mut rt = Runtime::new(Vec::new(), view, (80.0, 40.0), Env::new(true, 1.0));
    rt.frame(0.0);

    let mut results = Vec::new();
    let mut file_url_result = None;
    rt.update(|_, tx| {
        results.push(tx.open_path("/nonexistent/docs"));
        results.push(tx.open_path(""));
        results.push(tx.reveal_path("bad\0path"));
        results.push(tx.reveal_path("relative/report.pdf"));
        file_url_result = Some(tx.open_url("file:///etc/hosts"));
    });

    assert_eq!(results, [Ok(()), Err(OpenPathError::Empty), Err(OpenPathError::Invalid), Ok(())]);
    assert_eq!(file_url_result, Some(Err(OpenUrlError::UnsupportedScheme("file".into()))));

    let open_at = rt.find("open").expect("open button");
    rt.pointer_down(open_at.0, open_at.1);
    rt.pointer_up(open_at.0, open_at.1);
    let reveal_at = rt.find("reveal").expect("reveal button");
    rt.pointer_down(reveal_at.0, reveal_at.1);
    rt.pointer_up(reveal_at.0, reveal_at.1);
    rt.frame(1.0 / 60.0);

    assert_eq!(rt.state, ["open", "reveal"]);
    assert_eq!(
        rt.take_path_requests(),
        [
            PathRequest { path: PathBuf::from("/nonexistent/docs"), action: PathAction::Open },
            PathRequest { path: PathBuf::from("relative/report.pdf"), action: PathAction::Reveal },
            PathRequest { path: PathBuf::from("/nonexistent/clicked.txt"), action: PathAction::Open },
            PathRequest { path: PathBuf::from("/nonexistent/folder"), action: PathAction::Reveal },
        ],
    );
    assert!(rt.take_path_requests().is_empty());
    assert!(rt.take_url_requests().is_empty());
}

#[test]
fn idle_app_without_path_requests_stays_idle_and_empty() {
    fn view(_: &(), _: &mut Cx<()>) -> El { el().size(20.0, 20.0) }
    let mut rt = Runtime::new((), view, (20.0, 20.0), Env::new(true, 1.0));
    rt.frame(0.0);
    assert!(!rt.frame(1.0 / 60.0), "idle runtime should not request frames");
    assert!(rt.take_path_requests().is_empty());
}

#[test]
fn native_dispatch_seam_routes_requests_to_launcher_without_recording() {
    let mut rt = Runtime::new((), |_, _| el().size(20.0, 20.0), (100.0, 100.0), Env::new(true, 1.0));
    rt.host.open_urls_natively = true;

    let mut launched = Vec::new();
    let requests = vec![
        PathRequest { path: PathBuf::from("/a/first.txt"), action: PathAction::Open },
        PathRequest { path: PathBuf::from("/a/second.txt"), action: PathAction::Reveal },
    ];
    rt.host.dispatch_paths(requests.clone(), |req| {
        let fail = req.action == PathAction::Reveal;
        launched.push(req);
        if fail { Err(OpenPathError::Failed("simulated failure".into())) } else { Ok(()) }
    });

    assert_eq!(launched, requests);
    assert!(rt.take_path_requests().is_empty(), "native dispatch must not queue into headless buffer");
}

#[cfg(target_os = "macos")]
#[test]
fn macos_file_url_sets_directory_flag_without_stat() {
    use objc2::rc::autoreleasepool;
    autoreleasepool(|_| {
        let file = build_macos_file_url("/nonexistent/path/with space/file.txt", false);
        assert_eq!(
            file.absoluteString().expect("absoluteString").to_string(),
            "file:///nonexistent/path/with%20space/file.txt",
        );
        let dir = build_macos_file_url("/nonexistent/path/folder", true);
        assert_eq!(
            dir.absoluteString().expect("absoluteString").to_string(),
            "file:///nonexistent/path/folder/",
        );
    });
}
