use super::*;

#[test]
fn fills_the_markers_or_the_head_and_body() {
    let marked = "<head><!-- rune:css --></head><body><nav></nav><!-- rune:body --></body>";
    assert_eq!(fill(marked, ".a{}", "<main></main>"), "<head><style data-rune-static>.a{}</style></head><body><nav></nav><main></main></body>");
    let plain = "<html><head><title>x</title></head><body class=\"k\"></body></html>";
    assert_eq!(
        fill(plain, ".a{}", "<main></main>"),
        "<html><head><title>x</title><style data-rune-static>.a{}</style></head><body class=\"k\"><main></main></body></html>",
    );
}

#[test]
fn the_default_page_has_both_markers() {
    assert!(DEFAULT_TEMPLATE.contains(CSS_MARK) && DEFAULT_TEMPLATE.contains(BODY_MARK));
}

#[test]
fn multi_window_html_exports_the_selected_window_and_reports_write_errors() {
    fn window_spec(key: &str, label: &'static str) -> crate::WindowSpec<&'static str> {
        let spec = AppSpec::new("win", (240.0, 120.0), || "", |s, _| {
            crate::el().key("root").size(240.0, 120.0).child(crate::text(*s).key("label"))
        });
        crate::WindowSpec::from_app(key, crate::App::new(spec).init(move || label))
    }

    let dir = std::env::temp_dir().join(format!("rune-html-multi-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let out = dir.join("second.html");

    crate::try_run_windows_with_args(
        crate::RunOptions::default(),
        ["--html", out.to_str().unwrap(), "--window", "second"].map(String::from),
        |_| [window_spec("first", "alpha window"), window_spec("second", "beta window")],
    ).unwrap();

    let html = std::fs::read_to_string(&out).expect("--html must write the output file");
    assert!(html.contains("beta window"), "selected window content missing: {html}");
    assert!(!html.contains("alpha window"), "unselected window content leaked: {html}");

    let bad = dir.join("missing-dir").join("out.html");
    let err = crate::try_run_windows_with_args(
        crate::RunOptions::default(),
        ["--html", bad.to_str().unwrap(), "--window", "second"].map(String::from),
        |_| [window_spec("first", "alpha window"), window_spec("second", "beta window")],
    ).unwrap_err();
    assert!(err.contains("--html"), "expected write error, got: {err}");

    let app_out = dir.join("secondary.html");
    let main_spec = AppSpec::new("main", (240.0, 120.0), || "main window", |s, _| {
        crate::el().key("root").size(240.0, 120.0).child(crate::text(*s).key("label"))
    });
    let mut app = crate::App::new(main_spec);
    app.window(window_spec("second", "attached secondary"));
    app.args(["--html", app_out.to_str().unwrap(), "--window", "second"].map(String::from))
        .try_run()
        .unwrap();

    let app_html = std::fs::read_to_string(&app_out).expect("--html must write the secondary window");
    assert!(app_html.contains("attached secondary"), "secondary window content missing: {app_html}");
    assert!(!app_html.contains("main window"), "main window content leaked: {app_html}");

    std::fs::remove_dir_all(&dir).unwrap();
}
