#[test]
fn no_direct_window_chrome_actions_bypassing_win_panel_guard() {
    let host_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/window/host");
    let forbidden = [
        "titlebar_double_click(&",
        "toggle_maximized(&",
        "toggle_full_screen(&",
        "set_full_screen(&",
        ".window.drag_window(",
        ".window.set_maximized(",
        ".window.set_fullscreen(",
        ".window.is_maximized(",
    ];
    let mut bad = Vec::new();
    for entry in std::fs::read_dir(&host_dir).unwrap() {
        let path = entry.unwrap().path();
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else { continue };
        if !name.ends_with(".rs") || name == "tests.rs" || name.ends_with("_tests.rs") { continue; }
        let src = std::fs::read_to_string(&path).unwrap();
        for (idx, line) in src.lines().enumerate() {
            let trimmed = line.trim();
            if trimmed.starts_with("//") { continue; }
            for pat in forbidden {
                if trimmed.contains(pat) {
                    bad.push(format!("src/window/host/{name}:{}: {trimmed}", idx + 1));
                }
            }
        }
    }
    assert!(
        bad.is_empty(),
        "window/host must dispatch titlebar and window-state actions through Win's panel-aware methods:\n{}",
        bad.join("\n"),
    );
}

#[test]
fn adopted_panel_titlebar_target_drags_panel_and_ignores_double_click() {
    use super::{TitlebarTarget, titlebar_target};
    use crate::runtime::TitlebarAction;

    assert_eq!(titlebar_target(TitlebarAction::Drag, true), TitlebarTarget::DragPanel);
    assert_eq!(titlebar_target(TitlebarAction::Drag, false), TitlebarTarget::DragWindow);
    assert_eq!(titlebar_target(TitlebarAction::DoubleClick, true), TitlebarTarget::Ignore);
    assert_eq!(titlebar_target(TitlebarAction::DoubleClick, false), TitlebarTarget::DoubleClickWindow);
}

#[test]
fn no_user_close_path_calls_programmatic_close_directly() {
    let manifest = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let dirs = ["src/runtime/input", "src/window/titlebar", "src/native_menu"];
    let mut bad = Vec::new();
    for rel in dirs {
        let dir = manifest.join(rel);
        for entry in std::fs::read_dir(&dir).unwrap() {
            let path = entry.unwrap().path();
            let Some(name) = path.file_name().and_then(|n| n.to_str()) else { continue };
            if !name.ends_with(".rs") || name == "tests.rs" || name.ends_with("_tests.rs") { continue; }
            let src = std::fs::read_to_string(&path).unwrap();
            for (idx, line) in src.lines().enumerate() {
                let trimmed = line.trim();
                if trimmed.starts_with("//") { continue; }
                if trimmed.contains(".close_window(") {
                    bad.push(format!("{rel}/{name}:{}: {trimmed}", idx + 1));
                }
            }
        }
    }
    assert!(
        bad.is_empty(),
        "user-initiated close paths must call Tx::request_user_close (never programmatic close_window):\n{}",
        bad.join("\n"),
    );

    // Native WindowEvent::CloseRequested must route through Host::close_requested, not Host::close.
    let events_src = std::fs::read_to_string(manifest.join("src/window/host/events.rs")).unwrap();
    let mut in_close_req = false;
    let mut saw_close_requested = false;
    for (idx, line) in events_src.lines().enumerate() {
        let trimmed = line.trim();
        if trimmed.contains("WindowEvent::CloseRequested =>") {
            in_close_req = true;
            continue;
        }
        if in_close_req {
            if trimmed.starts_with("self.close(") {
                panic!("src/window/host/events.rs:{}: WindowEvent::CloseRequested bypassed Host::close_requested", idx + 1);
            }
            if trimmed.contains("self.close_requested(") {
                saw_close_requested = true;
            }
            if trimmed == "}" {
                break;
            }
        }
    }
    assert!(saw_close_requested, "WindowEvent::CloseRequested must dispatch to Host::close_requested");
}
