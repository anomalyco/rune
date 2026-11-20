#[test]
fn parses_appkits_frame_string() {
    assert_eq!(crate::parse_size("1645 0 716 1084 0 0 1728 1084 "), Some((716.0, 1084.0)));
    assert_eq!(crate::parse_size("12 30"), None);
    assert_eq!(crate::parse_size(""), None);
}

#[cfg(target_os = "macos")]
#[test]
#[allow(unsafe_code)]
fn busy_holds_a_marker_named_for_this_process() {
    let home = std::env::temp_dir().join(format!("rune-mac-busy-{}", std::process::id()));
    // SAFETY: the only test that reads or writes the environment.
    unsafe { std::env::set_var("HOME", &home) };
    let marker = home.join("Library/Caches/mac-app/busy").join(std::process::id().to_string());
    crate::busy(true);
    assert!(marker.exists());
    crate::busy(false);
    assert!(!marker.exists());
    std::fs::remove_dir_all(home).ok();
}
