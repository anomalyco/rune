use super::can_minimize;

#[test]
fn can_minimize_policy_matrix() {
    // Resizable borderless windows minimize on Windows and Linux when MINIMIZE is enabled.
    assert_eq!(can_minimize(false, false, false, true, true, true), Ok(()));
    // Non-resizable borderless windows (HUDs, pop-ups, launchers) remain rejected.
    assert_eq!(can_minimize(false, false, false, true, false, true), Err("a window without a minimize button"));
    // Click-through overlays (WindowOptions::overlay) never minimize.
    assert_eq!(can_minimize(false, false, true, true, true, true), Err("an overlay"));
    // Adopted panels and launchers never minimize on any platform.
    assert_eq!(can_minimize(false, true, false, true, true, true), Err("a panel"));
    assert_eq!(can_minimize(true, true, false, false, true, true), Err("a panel"));
    // macOS cannot miniaturize borderless windows even when resizable.
    assert_eq!(can_minimize(true, false, false, true, true, true), Err("a window without a minimize button"));
    // Decorated windows (resizable or fixed-size) minimize on every OS when MINIMIZE is enabled.
    assert_eq!(can_minimize(false, false, false, false, true, true), Ok(()));
    assert_eq!(can_minimize(false, false, false, false, false, true), Ok(()));
    assert_eq!(can_minimize(true, false, false, false, true, true), Ok(()));
    assert_eq!(can_minimize(true, false, false, false, false, true), Ok(()));
    // A window without WindowButtons::MINIMIZE never minimizes.
    assert_eq!(can_minimize(false, false, false, false, true, false), Err("a window without a minimize button"));
    assert_eq!(can_minimize(false, false, false, true, true, false), Err("a window without a minimize button"));
    assert_eq!(can_minimize(true, false, false, false, true, false), Err("a window without a minimize button"));

    // Exhaustive check across all 64 flag combinations.
    for bits in 0u8..64 {
        let is_macos = (bits & 1) != 0;
        let is_panel = (bits & 2) != 0;
        let click_through = (bits & 4) != 0;
        let borderless = (bits & 8) != 0;
        let resizable = (bits & 16) != 0;
        let has_minimize_button = (bits & 32) != 0;
        let expected = if is_panel {
            Err("a panel")
        } else if click_through {
            Err("an overlay")
        } else if !has_minimize_button || (borderless && (is_macos || !resizable)) {
            Err("a window without a minimize button")
        } else {
            Ok(())
        };
        assert_eq!(
            can_minimize(is_macos, is_panel, click_through, borderless, resizable, has_minimize_button),
            expected,
            "is_macos={is_macos} is_panel={is_panel} click_through={click_through} borderless={borderless} resizable={resizable} has_minimize_button={has_minimize_button}",
        );
    }
}
