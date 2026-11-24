#[test]
fn no_direct_window_chrome_actions_bypassing_win_panel_guard() {
    let host_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/window/host");
    let forbidden = [
        "titlebar_double_click(&",
        "toggle_maximized(&",
        "toggle_full_screen(&",
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
    assert_eq!(titlebar_target(TitlebarAction::Resize(crate::ResizeEdge::SouthEast), true), TitlebarTarget::Ignore);
    assert_eq!(titlebar_target(TitlebarAction::Resize(crate::ResizeEdge::SouthEast), false), TitlebarTarget::ResizeWindow(crate::ResizeEdge::SouthEast));
}
