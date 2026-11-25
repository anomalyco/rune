use super::*;

fn utf16(s: &str) -> Vec<u16> {
    s.encode_utf16().collect()
}

#[test]
fn color_policy_refresh_matches_immersive_color_set_and_generic_broadcast() {
    // Generic WM_SETTINGCHANGE with lParam == 0 (NULL).
    assert!(should_refresh_color_policy_area(None));
    // Windows Settings broadcast ("ImmersiveColorSet"), any ASCII case.
    assert!(should_refresh_color_policy_area(Some(&utf16("ImmersiveColorSet"))));
    assert!(should_refresh_color_policy_area(Some(&utf16("immersivecolorset"))));
    assert!(should_refresh_color_policy_area(Some(&utf16("IMMERSIVECOLORSET"))));

    // Unrelated WM_SETTINGCHANGE areas are ignored.
    for other in ["", "Environment", "Policy", "intl", "ImmersiveColor", "ImmersiveColorSet2"] {
        assert!(
            !should_refresh_color_policy_area(Some(&utf16(other))),
            "unexpected refresh for WM_SETTINGCHANGE area {other:?}",
        );
    }
}

#[test]
fn setting_change_seam_refreshes_uxtheme_before_winit_reads_theme() {
    // Models uxtheme's process-cached ShouldAppsUseDarkMode: returns cached_dark until
    // RefreshImmersiveColorPolicyState copies registry_dark into cached_dark.
    struct FakeUxtheme {
        registry_dark: bool,
        cached_dark: bool,
        refresh_calls: usize,
    }

    let mut fake = FakeUxtheme { registry_dark: true, cached_dark: true, refresh_calls: 0 };

    // Registry flips to light (AppsUseLightTheme = 1).
    fake.registry_dark = false;
    assert!(fake.cached_dark, "without refresh uxtheme keeps returning stale cached dark");

    // Pre-1809 build (< 17763): never invokes ordinal 104.
    let area = utf16("ImmersiveColorSet");
    assert!(!refresh_color_policy_with(Some(17134), Some(&area), || {
        fake.refresh_calls += 1;
        fake.cached_dark = fake.registry_dark;
    }));
    assert_eq!(fake.refresh_calls, 0);
    assert!(fake.cached_dark);

    // Unrelated area on Win11: zero refresh calls.
    let env = utf16("Environment");
    assert!(!refresh_color_policy_with(Some(26300), Some(&env), || {
        fake.refresh_calls += 1;
        fake.cached_dark = fake.registry_dark;
    }));
    assert_eq!(fake.refresh_calls, 0);
    assert!(fake.cached_dark);

    // Real Settings broadcast ("ImmersiveColorSet") on Win11: refreshes cache BEFORE winit queries.
    assert!(refresh_color_policy_with(Some(26300), Some(&area), || {
        fake.refresh_calls += 1;
        fake.cached_dark = fake.registry_dark;
    }));
    assert_eq!(fake.refresh_calls, 1);
    assert!(!fake.cached_dark, "winit's ShouldAppsUseDarkMode now sees light theme");

    // Flip back to dark and send generic WM_SETTINGCHANGE (lParam == 0): also refreshes.
    fake.registry_dark = true;
    assert!(refresh_color_policy_with(Some(26300), None, || {
        fake.refresh_calls += 1;
        fake.cached_dark = fake.registry_dark;
    }));
    assert_eq!(fake.refresh_calls, 2);
    assert!(fake.cached_dark, "winit's ShouldAppsUseDarkMode now sees dark theme again");
}
